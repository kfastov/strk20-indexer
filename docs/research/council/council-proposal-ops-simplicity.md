# strk20-indexer architecture — ops-simplicity lens

Lens: operational-simplicity-first. One static server binary, one SQLite file + one directory of static feed files, deterministic backfill, mirroring = copying files, smallest dependency tree that still reuses upstream discovery-core unmodified.

## 0. Core thesis

The product is a **directory of content-addressed static files** (the feed). The SQLite DB is a local index/cache derived from the same ingest; the feed is canonical and fully reproducible from any archive RPC. Everything else — HTTP server, compat mode, stats — is a thin veneer over those two stores. A mirror is `wget -r` plus a hash check. A self-host is one binary + `STARKNET_RPC_URL`.

Two binaries, deliberately split by secret exposure:
- `strk20` — server/ingester. Never handles viewing keys except in explicitly-flagged compat mode.
- `strk20-sync` — keyless client CLI (also the acceptance-test client). The only secret-bearing binary.

## 1. Workspace / crate layout

```
strk20-indexer/            (cargo workspace, pinned starknet-rust fork rev 7caedfe, starknet-types-core =0.2.x denied 1.x)
  crates/
    feed/        lib strk20-feed      — epoch wire format: canonical encode/decode, sha256 content addressing,
                                        hash-chain verify, manifest types. Deps: serde, serde_json, sha2, hex,
                                        thiserror; zstd behind non-default "compress" feature → core is wasm-clean.
                                        No async, no IO traits beyond &[u8] in/out (IO lives in callers).
    indexerd/    bin strk20           — ingest pipeline, SQLite store, epoch cutter, HTTP server (axum 0.8.9),
                                        compat mode, CLI. Deps: strk20-feed(compress), rusqlite(bundled),
                                        tokio, axum, tower-http(fs,trace), reqwest(json), clap(derive),
                                        tracing(+subscriber), serde, zstd, anyhow/thiserror,
                                        discovery-core(git tag CONTRACT_V2_DEPLOYED_MAINNET_2026-07-08, rev 74841caf in comment),
                                        starknet-core/-crypto fork rev 7caedfe (compat types + verify-root pedersen/poseidon).
    client/      lib strk20-client
                 bin strk20-sync      — keyless client over unmodified discovery-core: FeedStore (local SQLite mirror
                                        built from feed files), transports, cursor persistence, note/spent-state model.
                                        Deps: strk20-feed(compress), discovery-core, rusqlite(bundled), reqwest, tokio,
                                        clap, serde, zeroize (via core).
    (roadmap) client-wasm/            — wasm-bindgen wrapper + TS DiscoveryProviderInterface adapter. Not in branch;
                                        client/ isolates transports behind a trait and feed core is wasm-clean, so this
                                        is additive, no re-architecture.
  vendor/fixtures/                    — vendored copies: devnet-state.json, cairo-reference-data.json,
                                        devnet-dump.json.gz + metadata (upstream Apache-2.0, provenance noted).
  tests/e2e/                          — acceptance test (see §7).
```

Dependency edges: `client → feed, discovery-core`; `indexerd → feed, discovery-core (compat only at runtime, compiled in — no feature matrix)`; `feed → nothing heavy`. No crate depends on `indexerd`.

## 2. Data model

### 2.1 SQLite (server: `strk20.db`; client uses tables 2+3 only in `sync.db`)

Felts are 32-byte big-endian BLOBs in SQLite, `0x`-lowercase-minimal hex in JSON. `PRAGMA journal_mode=WAL; synchronous=NORMAL; foreign_keys=ON;`

```sql
CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
-- schema_version, pool_address, chain_id, genesis_block(8978970), head_number, head_hash,
-- l1_accepted_number, decode_state('ok'|'degraded'), degraded_since_block

CREATE TABLE blocks(              -- only pool-active blocks + the running head checkpoint
  number INTEGER PRIMARY KEY,
  hash BLOB NOT NULL,
  parent_hash BLOB NOT NULL,
  timestamp INTEGER NOT NULL,
  status INTEGER NOT NULL         -- 0 = ACCEPTED_ON_L2, 1 = ACCEPTED_ON_L1
);

CREATE TABLE storage_log(         -- raw pool storage diffs, append-only, layout-agnostic
  slot BLOB NOT NULL,
  block INTEGER NOT NULL,
  value BLOB NOT NULL,
  PRIMARY KEY(slot, block)
) WITHOUT ROWID;
-- read_slot(s, at) = value where block = (SELECT max(block) FROM storage_log WHERE slot=s AND block<=at), else ZERO
-- read_slots_with_block returns (value, write_block) natively — replaces the fork-only RPC extension.

CREATE TABLE events(              -- raw pool events with within-block order (fork EmittedEvent needs it)
  block INTEGER NOT NULL,
  event_index INTEGER NOT NULL,   -- position among pool events in block, from getEvents page order
  tx_index INTEGER NOT NULL,
  tx_hash BLOB NOT NULL,
  keys BLOB NOT NULL,             -- concat 32B felts
  data BLOB NOT NULL,             -- concat 32B felts
  key0 BLOB NOT NULL,             -- selector (denormalized for filters)
  key1 BLOB,                      -- first keyed felt (nullifier for NoteUsed etc.)
  PRIMARY KEY(block, event_index)
);
CREATE INDEX ev_key0 ON events(key0, block);
CREATE INDEX ev_key1 ON events(key1) WHERE key1 IS NOT NULL;

CREATE TABLE class_history(       -- upgrade tracking (Q13)
  block INTEGER PRIMARY KEY,      -- block where class became active (deploy or replaced_classes)
  class_hash BLOB NOT NULL,
  known INTEGER NOT NULL          -- 1 iff mapped to a decoder version in config
);

CREATE TABLE epochs(              -- index over cut feed files
  idx INTEGER PRIMARY KEY,
  from_block INTEGER NOT NULL, to_block INTEGER NOT NULL,
  content_hash BLOB NOT NULL,     -- sha256 of UNCOMPRESSED canonical payload (zstd-version-independent)
  file_size INTEGER NOT NULL,
  storage_root BLOB,              -- pool storage root at to_block from getStorageProof; NULL if window missed
  cut_at INTEGER NOT NULL
);

CREATE TABLE ingest_cursor(
  id INTEGER PRIMARY KEY CHECK(id=1),
  scan_frontier INTEGER NOT NULL,     -- getEvents scan position (block)
  events_continuation TEXT            -- mid-page continuation token, resumable
);
```

Client-side extras in `sync.db`: `meta(discovery_cursor_json, feed_url, last_epoch, head_block)`; file perms 0600 and a loud doc note — `DiscoveryCursor` embeds `channel_key` (`SecretFelt`) so the cursor file is key-adjacent material.

### 2.2 Feed directory (the canonical product)

```
feed/
  genesis.json                 -- immutable: {format:1, pool, chain_id, genesis_block:8978970, epoch_size:50000}
  manifest.json                -- mutable head pointer (atomic tmp+rename), see below
  epochs/00000000.strk20e.zst  -- epoch i covers blocks [genesis + i*epoch_size, genesis + (i+1)*epoch_size)
  epochs/00000001.strk20e.zst
  head.ndjson                  -- uncut tail (blocks > last epoch, ≤ chain head, L2-accepted); regenerated wholesale
  snapshots/snapshot-<block>.sqlite.zst   -- optional convenience artifacts
```

**Epoch payload** = canonical NDJSON (debuggable with `zstd -d | jq`), then zstd(level 19). Canonicalization: serde struct field order fixed, compact separators, felts as minimal lowercase hex. **Content hash = sha256 of the uncompressed NDJSON bytes** — mirrors reproduce it regardless of zstd version; the .zst file hash is informational only.

Line grammar (one JSON object per line):

```
header: {"v":1,"epoch":12,"pool":"0x040337…","chain_id":"SN_MAIN","from":9578970,"to":9628969,
         "prev":"<hex sha256 of epoch 11 content>"}                       -- prev="" for epoch 0
block:  {"b":9578990,"h":"0x…","p":"0x…","t":1745531234,
         "d":[["0x<slot>","0x<value>"],…],                                -- pool storage diffs, slot-sorted
         "e":[[<tx_index>,<event_index>,"0x<tx_hash>",["0x<key0>",…],["0x<data0>",…]],…],
         "rc":"0x<new_class_hash>"}                                       -- "rc" present only on upgrade blocks
        (blocks with no pool activity are omitted; block lines ascending by "b")
footer: {"end":true,"n_blocks":123,"class":"0x67dddd…","root":"0x<pool storage_root at to>","root_block":9628969}
        ("root" omitted if the proof window was missed; verify-root can backfill nothing — documented gap)
```

Epochs are cut **only when to_block ≤ l1_accepted** → immutable by construction (Q12). The `prev` field hash-chains epochs, so `manifest.latest_content_hash` commits the entire history; an omission attack by a mirror is a visible fork (Q11). `head.ndjson` uses the same block-line grammar with a one-line header `{"v":1,"tail_from":N,"head":M,"head_hash":"0x…","l1_accepted":K}`; it is regenerated in full on every change and on reorg — clients refetch it wholesale (≤ ~100 KB at current activity), which deletes the need for any rollback protocol on the wire.

manifest.json:
```json
{"format":1,"pool":"0x040337…","chain_id":"SN_MAIN","genesis_block":8978970,"epoch_size":50000,
 "latest_epoch":101,"latest_content_hash":"9f3c…","head_block":14056430,"head_hash":"0x7be…",
 "l1_accepted":14049912,"generated_at":1756458000,
 "epochs":[{"i":0,"from":8978970,"to":9028969,"content_hash":"ab12…","file":"epochs/00000000.strk20e.zst",
            "size":58231,"root":"0x05f…"}, …]}
```

## 3. API surface

Everything under `/feed/*` is literal files on disk served by `tower_http::services::ServeDir` with strong ETags — identical bytes from any static server, CDN, or rsync mirror.

| Method/Path | Req | Resp | Notes |
|---|---|---|---|
| GET /feed/genesis.json | — | genesis doc | immutable |
| GET /feed/manifest.json | — | manifest (above) | poll target; ETag |
| GET /feed/epochs/{idx:08}.strk20e.zst | — | zstd epoch bundle | immutable, cache-forever |
| GET /feed/head.ndjson | — | tail bundle | ETag = head_hash; U3 polls this |
| GET /feed/snapshots/latest.sqlite.zst | — | sqlite snapshot | optional (`strk20 snapshot`) |
| GET /health | — | `{"status":"OK","head":{"number":14056430,"hash":"0x…","timestamp":…},"l1_accepted":14049912,"lag_secs":4,"latest_epoch":101,"class_hash":"0x67dddd…","decode_state":"ok"}` | 503 when UNHEALTHY |
| GET /v1/stats | — | honest metrics per Q18: `{"deposits":{"0x49d3…":{"count":812,"amount":"0x…"}},"withdrawals":{…},"note_count":118372,"spend_count":41230,"open_note_deposits":…,"external_calls":{"0x<target>":n},"registrations":…,"upgrades":[{"block":11632886,"class":"0x67dddd…"}],"health":{…}}` | U7; nothing joining deposit/withdraw addresses |
| POST /v1/raw/read_slots | `{"block":14056000,"slots":["0x1a2b…","0x3c4d…"]}` (block also `"head"`; ≤1000 slots) | `{"block":14056000,"block_hash":"0x…","values":[{"slot":"0x1a2b…","value":"0x0567…","write_block":13990001},…]}` (absent slot → value `"0x0"`, write_block null) | targeted mode; documented leaky (Q7); powers U6 clients and remote compat backends |
| GET /v1/raw/events?from=N&to=M&key0=0x…&key1=0x…&limit=1000&cursor=C | — | `{"events":[{"block":…,"tx_index":…,"event_index":…,"tx_hash":"0x…","keys":[…],"data":[…]}],"cursor":"<block>-<idx>"}` | position-keyed filter semantics mirror RawEventAccess |

**Compat mode (U8)** — runtime flag `--compat`, off by default; startup banner + per-request `warn!` that viewing keys are visible to this process; cursor values never logged. Medium-reuse implementation: copied `api/types.rs` serde types (~350 lines, wire frozen RC.0→RC.5) + our axum handlers calling the unmodified `discovery-core` engine over `DbBackend` (SQLite impl of `RawStorageAccess`+`RawEventAccess`+`StorageSnapshot`+`StorageBackend`+`ChainState`). Chosen over max-reuse (mounting reference `ApiServer`) to avoid the tower_ohttp git dep, rustls, and the tower-http 0.6/0.7 split — smallest tree wins; conformance is proven by replaying upstream's 11 HTTP tests (§7). Routes and shapes exactly the reference: `POST /v1/sync/incoming_state` (`{contract_address, viewing_key, recipient_address, last_known_block?, block_ref?, cursor}` → `{block_ref, channels, subchannels, notes, cursor}`), `POST /v1/sync/outgoing_state`, `POST /v1/sync/preflight_check`, `POST /v1/history`; error shape `{"error":{"code","message"}}`; HTTP 409 reserved exclusively for `BLOCK_REORGED`; viewing-key-vs-registered-pubkey validation reproduced; reference validation limits (max_channels 256 etc.) honored. The published SDK's `IndexerDiscoveryProvider` pointed at `--compat` is the zero-code U2 path today.

No proof proxying: U6 clients fetch `starknet_getStorageProof` from their own RPC and check it against the epoch footer `root` — the server stays out of the trust path by design.

## 4. Ingest pipeline

One sequential tokio task; deterministic; every step resumable from `ingest_cursor`. States: `Init → Backfill → Follow` (identical code path; Backfill is Follow with a distant target).

Per cycle:
1. **Finality poll**: `getBlockWithTxHashes("l1_accepted")` and `("latest")` → update meta head/l1_accepted; mark `blocks.status=1` for numbers ≤ l1_accepted.
2. **Canonicity check (reorg)**: `getBlockWithTxHashes(stored_head.number)`; if hash mismatch, walk back through stored active blocks until stored hash matches the chain, floor = last cut epoch's `to_block` (epochs are L1-final, never crossed). Delete `blocks/storage_log/events` rows above the match point in one transaction, rewind `scan_frontier`, regenerate `head.ndjson`. En-masse L2 revocation (Grinta-scale) is just a deeper walk to the same floor.
3. **Event scan** (events-first, Q5): `getEvents(address=pool, from=scan_frontier, to=min(latest, frontier+step), chunk 1000)` with persisted continuation token → ordered active-block set + per-block event lists (order within page = `event_index`).
4. **Per active block**: `getStateUpdate(block)` → filter `storage_diffs[pool]`, detect `replaced_classes[pool]`/`deployed_contracts[pool]`; `getBlockWithTxHashes(block)` → hash/parent/timestamp/status. One SQLite transaction: insert block, diffs, events, advance cursor. Crash anywhere = clean resume.
5. **Upgrade handling (Q13)**: `replaced_classes` hit → insert `class_history`; if the class hash is not in the config map → `decode_state=degraded` from that block: raw ingest, feed, `/feed/*`, `/v1/raw/*` continue untouched (they are layout-agnostic); compat endpoints answer `SERVICE_UNAVAILABLE` for `block_ref` past the boundary; `/v1/stats` freezes event-decode counters at the boundary; `/health` says degraded. Human maps the class → flip config → resume typed behavior. Mirrors never fork.
6. **Epoch cut**: when a full epoch range is ≤ l1_accepted → export rows to canonical NDJSON, sha256, zstd, write `epochs/NNNNNNNN.strk20e.zst` (tmp+rename), fetch `getStorageProof` at `to_block` for the footer `root` (best-effort within the ~25–55k-block window), insert `epochs` row, rewrite `manifest.json` atomically. Cutting is a pure function of DB rows → re-running produces byte-identical payloads (determinism is unit-tested).
7. **Tail regen**: on any head change, rewrite `head.ndjson` from DB (blocks > last epoch).

**Completeness reconciliation** (closes the events-first hole — a hypothetical storage write with no event): `strk20 verify-root` recomputes the pool's storage MPT root from the SQLite mirror (pedersen trie over all slots, ~200 lines over starknet-crypto, cheap at current N) and compares against `getStorageProof`. Run automatically at each epoch cut; mismatch → alarm + slow-path fallback: full-range `getStateUpdate` rescan of the epoch's blocks. Today's classes always emit events with writes; this check makes that an invariant we verify rather than assume.

**Backfill** (`strk20 backfill`): same pipeline, target = current l1_accepted, then exit. ~19 MB / ~47 min against lava from pool genesis; deterministic: two operators backfilling from different archive RPCs produce identical epoch content hashes (this is U5's foundation and is asserted by the ignored-by-default live smoke test). RPC client is plain reqwest+serde_json with our own response structs (5 methods; no starknet-providers), real User-Agent set (lava 403s the default).

## 5. Keyless client (`strk20-client` / `strk20-sync`)

Layers:
1. **Transport** (trait `FeedTransport`: `fetch_manifest`, `fetch_epoch(i)`, `fetch_head`): `HttpTransport` (reqwest, the headline), `DirTransport` (local path — mirrors, tests, air-gap). Separately `RpcSlotTransport` (plain `starknet_getStorageAt` batches) and `RawApiTransport` (`POST /v1/raw/read_slots`) exist for documented-leaky targeted mode; the default and the doc headline is feed mode.
2. **FeedStore**: downloads manifest → verifies epoch hash-chain (`prev` links + content sha256 of every payload) → applies block lines into local `sync.db` (`storage_log` + `events` subset) → applies `head.ndjson` tail (idempotent: tail rows are deleted and reapplied on refresh; a reorged tail simply gets replaced). Any hash mismatch = hard error naming epoch and expected/actual hash (U5 divergence detection is a client-side property, not server trust).
3. **Engine adapter**: `impl RawStorageAccess for FeedStore` (3 methods: `read_slot`, `read_slots`, `read_slots_with_block` — `write_block` comes free from `storage_log`) → blanket `IViews` → **unmodified** `discovery_core::sync::{sync_incoming_state, sync_outgoing_state, preflight_check}`; `impl RawEventAccess` over the local events table → `history::fetch_transactions`. Spent-state per Q10: nullifiers precomputed by the engine, matched against `NoteUsed` `key1` index locally.
4. **Cursor persistence**: engine `DiscoveryCursor` (Serialize) stored in `sync.db` meta; resume = feed-refresh (tail + any new epochs) then re-run engine with stored cursor — incremental by construction (U1). Cursor file documented sensitive (contains `SecretFelt` channel keys), 0600.
5. **CLI** `strk20-sync`: `--feed URL|DIR --address 0x… --key-file k.hex [--db sync.db] [--json] [--full-resync]` → prints channels/notes/balances/spent-state; exit code 0 only on complete cursor. Key read from file/stdin only, never argv (ps leak), zeroized after parse into `SecretFelt`.
6. **U6 spot-check**: `strk20-sync verify --rpc URL` — for each discovered note/nullifier, fetch `starknet_getStorageProof` from the user's own RPC, verify the pedersen MPT walk (shared verifier module with `verify-root`), compare against feed values; non-membership of a nullifier slot proves un-spent-ness at that block.
7. **SDK path (U2)**: today — compat endpoint with the stock `IndexerDiscoveryProvider` (self-hosted key custody), or the Rust lib directly. Roadmap without re-architecture: `client-wasm` crate wraps `strk20-client` (feed core is wasm-clean; discovery-core compiles to wasm32 untouched; `?Send`/`SendWrapper` friction is contained in the transport trait), plus a ~200-line TS class implementing the 3-method `DiscoveryProviderInterface`, reusing the SDK's exported cursor-mapping semantics so `NotesCursor` round-trips with the hosted wire format.

Privacy property, mechanical: in feed mode the client's request set is `{manifest, missing epochs, head}` — a pure function of the client's *download progress*, independent of key and address. That independence is asserted in the acceptance test (§7).

## 6. CLI (server binary `strk20`)

```
strk20 run        --rpc-url URL --db strk20.db --feed-dir ./feed --listen 0.0.0.0:8080
                  [--compat] [--epoch-size 50000] [--pool 0x040337…]      # ingest + serve; the one command (U4)
strk20 backfill   … same flags …                                          # ingest to l1_accepted, cut epochs, exit
strk20 status     --db strk20.db                                          # head, lag, epochs, decode_state
strk20 epoch verify [--dir ./feed | --url https://…]                      # recompute hashes + chain; U5 mirror audit
strk20 verify-root --db strk20.db --rpc-url URL [--block N]               # mirror MPT root vs getStorageProof
strk20 snapshot   --db strk20.db --out feed/snapshots/                    # content-addressed sqlite snapshot
strk20 mirror     --from URL --dir ./feed                                 # pull + verify a feed (wget also works; this adds verification)
strk20 stats      --db strk20.db [--json]                                 # the Q18 honest set, printed
```

Config = flags or matching env vars (`STRK20_RPC_URL`, …). No config file required; one is accepted (`--config strk20.toml`) but every default works without it.

## 7. Acceptance E2E client test (the branch's definition of done)

`tests/e2e/keyless_discovery.rs` — spawns **real binaries over real HTTP**; no in-process shortcuts on the asserted path.

**Seed data.** Vendored upstream `devnet-state.json` (48 slot→value pairs, alice `0xa11ce` / bob `0xb0b` keys, contract constants, block 46). The test synthesizes a deterministic chain from it: all 48 writes placed at block 30 (satisfies the 10-block maturity rule against head 46), synthetic hashes `h(n)=poseidon(n)`-style deterministic felts, head at 46, `l1_accepted`=40, epoch_size=32 → epoch 0 finalized `[0,31]`, tail `[32,46]`. A **mock Starknet RPC** (~150-line hyper server inside the test) serves exactly `getEvents` / `getStateUpdate` / `getBlockWithTxHashes` / `getStorageProof`(root stub) / `getClassHashAt` from this synthetic chain.

**Expected values.** Computed two ways so a shared bug cannot self-confirm: (a) engine-over-`MockBackend` loaded with the same fixture (upstream's own conformance pattern — MockBackend is pub); (b) a small hand-pinned golden JSON (alice's note count, token, value, note_id, nullifier for one note) checked once against upstream's fixture-driven tests.

**Steps.**
1. Spawn `strk20 run --rpc-url http://mock --db tmp/i.db --feed-dir tmp/feed --listen 127.0.0.1:0` → wait for `/health` OK and `latest_epoch=0`. This exercises the full pipeline: RPC ingest → SQLite → epoch cut → static serving.
2. Start a **capturing reverse proxy** (in-test hyper, ~60 lines): records every request line, all headers, and full body bytes of everything the client sends.
3. Run real binary `strk20-sync --feed http://proxy/feed --address <alice> --key-file tmp/alice.key --db tmp/alice.db --json --out tmp/alice.json`. Repeat for bob, and for a fresh unused address (expects empty).
4. Advance the mock chain: one new block 47 with one additional note write for alice (values derived via discovery-core helpers from the fixture keys), head 50, l1_accepted 47. Wait for the server tail to update; rerun `strk20-sync` for alice **with the existing sync.db** (resume path).

**Assertions.**
- *Correctness (U1)*: `alice.json` == engine-over-MockBackend output field-for-field (channels, subchannels, notes incl. token/value/note_id/nullifier, spent flags, cursor with `is_complete()==true`) AND matches the golden pins; bob likewise; unused address → empty, complete cursor. After step 4, the resume run returns exactly the one new note in addition, and its cursor advanced — cold start and incremental resume both proven.
- *Spent-state (Q10)*: the fixture variant sets one nullifier slot nonzero + a matching `NoteUsed` event → that note reported spent / excluded from unspent, matching reference semantics.
- *Keyless, mechanical (the negative assertion)*: from the proxy capture — (1) **every request is a GET with empty body**; (2) the URL multiset ⊆ `{/feed/manifest.json, /feed/epochs/00000000.strk20e.zst, /feed/head.ndjson}`; (3) the byte concatenation of all request lines + headers + bodies contains **none of**: the viewing key as minimal hex, zero-padded hex, decimal, byte-reversed hex, or base64 of the 32-byte BE felt; nor the derived public key; nor any channel_key felt (all computed in-test via starknet-crypto/discovery-core) — catching key-equivalent leaks, not just the literal key; (4) **address-blindness**: alice's and bob's request URL multisets are identical (the request stream is independent of who is syncing).
- *Feed integrity (U5)*: flip one byte in the epoch file on disk → fresh client run fails with the specific hash-mismatch error naming epoch 0; restore → passes. Separately `strk20 epoch verify --dir` passes on the server's own feed.
- *Determinism (U4/U5)*: run `strk20 backfill` a second time into a fresh DB/feed dir from the same mock → epoch 0 `content_hash` byte-identical.
- *Compat conformance (U8, same harness, secondary)*: restart server with `--compat`; replay upstream's 11 HTTP-level tests (request/response fixtures from `devnet-dump` + `test_api.rs` shapes) against it; additionally assert 409 is returned exactly for the reorged `last_known_block` case.

CI: the whole thing runs offline (mock RPC), < 2 min. A separate `#[ignore]` live smoke test backfills the first ~200 active mainnet blocks from lava and asserts epoch-0-prefix content hash against a pinned constant — determinism against reality, run manually/nightly.

## 8. Test strategy (full pyramid)

- **Unit**: feed codec round-trip + canonical-encoding vectors (hex minimality, field order, hash stability pinned constants); SQLite `read_slot` as-of-block semantics differential-tested against `MockBackend` over `devnet-state.json`; reorg rewind property (never crosses epoch floor); event position-filter semantics vs copied `MockEventBackend` behavior.
- **Conformance**: vendored `cairo-reference-data.json` driven through the client path (engine outputs equal vector outputs); upstream's 11 service HTTP tests vs compat mode; SDK wire-format shapes replayed (serve recorded SDK request bodies from `indexer-discovery.test.ts`, assert accepted + response shape).
- **Acceptance**: §7.
- **Bench** (`bench/`, fills Q19): B1 backfill wall-clock, B3=0 by construction (assert via proxy), B5 slab fetch bytes/time vs gap, B8 crossover table.

## 9. What this lens sacrifices (honest)

- **No Postgres, no horizontal ingest.** Single-writer SQLite; scale-out story is "mirror the static files", not DB clustering. The internal `Store` seam is one impl; adding Postgres later is new code behind the same trait but is *not* in the branch. Multi-instance hosted compat mode at high QPS would need it.
- **No push.** U3 gets ETag-polled `head.ndjson` (~1 req/min, address-blind). No WebSocket/SSE; at ~1000× activity polling gets clumsy — SSE over the same tail file is the roadmap, additive.
- **No wasm/TS adapter in the branch.** U2 today = compat endpoint (stock SDK provider, self-hosted key custody) or the Rust lib. The JS-native keyless provider is roadmap; seams (wasm-clean feed core, transport trait) are pre-cut.
- **No OHTTP** in compat mode (medium reuse dropped it) — IP privacy for compat users is BYO (proxy). Documented.
- **No PIR / no prefix-bucket endpoint** initially; targeted mode is plain `read_slots` with a leak warning. Prefix-bucketing is a ~50-line addition on the events/slots endpoints when wanted (Q9 trigger far away).
- **Cold start = whole history** (~6 MB zstd today). No filtered/partial sync — that is also the privacy feature, but at sustained 100× activity, cold cost grows to ~600 MB and epoch-granular pruning/snapshot-start becomes necessary (snapshots/ dir is the escape hatch, already in the format).
- **head.ndjson refetched wholesale** — trivial now, wasteful at 1000× activity (then: range requests or chunked tail, additive).
- **Events-first ingest assumes writes co-occur with events** — verified for both deployed classes, continuously reconciled by verify-root at epoch cuts, but a silent-writing future class costs a slow full-range rescan of the affected epoch.
- **Feature breadth**: no explorer UI (JSON stats only), no tx CLI, no hosted multi-tenant features, no auth/quotas beyond a body-size and slot-count cap.

## 10. Use-case coverage map

- U1 wallet: `strk20-sync` feed mode; cold + resume both in the acceptance test. Key never serialized.
- U2 SDK dev: compat endpoint now (stock `IndexerDiscoveryProvider`); wasm provider roadmap, seams pre-cut.
- U3 backend/bot: poll `head.ndjson` ETag (address-blind, <100 KB, ~KB deltas); decode NoteUsed/EncNote events locally with its own keys.
- U4 self-hoster: `strk20 run` + one env var; one file + one dir of state; deterministic `backfill`.
- U5 mirror: copy `feed/` by any means; `strk20 epoch verify`/`mirror` re-verify; content hashes make divergence a visible fork; backfill-from-scratch reproduces identical hashes.
- U6 auditor: epoch footer roots + `strk20-sync verify` storage-proof spot checks against own RPC; `verify-root` for operators.
- U7 explorer: `/v1/stats` — the Q18 honest set, nothing more.
- U8 migrator: `--compat`, reference wire exact, conformance-proven, loudly labeled key-visible.
