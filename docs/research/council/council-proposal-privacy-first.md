# strk20-indexer — Privacy-first architecture proposal

Lens: privacy-and-verifiability-first. The server must be able to prove it *cannot* know who is syncing: the headline product is a content-addressed, verifiable, user-independent feed; everything user-addressed is opt-in, flagged, and honestly labeled.

## 1. Pitch

STRK20 discovery as a public verified sync feed. The indexer mirrors the pool's storage diffs and events from ordinary JSON-RPC, cuts them into immutable content-addressed epoch bundles (~6 MB full history, ~KB/day), and serves them over dumb GETs. Every wallet downloads the same bytes and runs the unmodified upstream `discovery-core` engine locally over its own mirror: the server learns neither the viewing key, nor the queried slots, nor even *who* is syncing — requests are user-independent and CDN-cacheable. Mirrors reproduce byte-identical bundles from any archive RPC, so omission becomes a visible hash fork; per-epoch `storage_root` anchors from `starknet_getStorageProof` give auditors a chain-rooted spot check. Lower-privacy conveniences (raw slot reads, the reference viewing-key API for SDK drop-in) exist behind explicit flags with their leakage documented, never as the default.

## 2. Components (workspace)

Single cargo workspace, single deliverable binary `strk20`. Workspace-wide pins: `starknet-types-core = "=0.2.4"` (deny 1.x — different `Felt`), starknet-rust fork rev `7caedfe`, `discovery-core = { git = "https://github.com/starkware-libs/starknet-privacy.git", tag = "CONTRACT_V2_DEPLOYED_MAINNET_2026-07-08" }` (# rev 74841caf0466d122117945e28ed983e2864c8fc1).

```
crates/
  strk20-types    # Felt hex canonicalization, chain config (pool addr, genesis block,
                  # class-hash→decoder map), error types. Pure, wasm-clean.
  strk20-feed     # Epoch wire format: encode/decode/verify, content addressing,
                  # hash chain, manifest. Pure, wasm-clean. deps: types, serde, sha2, (zstd behind feature).
  strk20-store    # SQLite (rusqlite bundled, WAL) store; DbBackend implementing upstream
                  # RawStorageAccess + RawEventAccess + StorageSnapshot + StorageBackend + ChainState.
                  # deps: types, discovery-core, rusqlite, tokio (spawn_blocking).
  strk20-ingest   # Plain reqwest JSON-RPC client (own serde structs for getEvents/getStateUpdate/
                  # getBlockWithTxHashes/getStorageProof/getClassHashAt), pipeline state machine,
                  # reorg walker, upgrade watch, epoch cutter. deps: types, feed, store.
  strk20-client   # KEYLESS client: FeedTransport trait (http / dir / in-mem), LocalMirror
                  # (verify + apply epochs → slot map + events), wallet sync driver over
                  # unmodified discovery-core, spent-state tracker, local cursor persistence.
                  # deps: types, feed, discovery-core. NO rusqlite, NO store → wasm path stays clean.
  strk20-server   # axum 0.8: /feed/*, /raw/* (flagged), /metrics, /health, SSE stream.
                  # feature "compat": mounts the reference discovery-service ApiServer over DbBackend.
                  # deps: store, feed, types, (discovery-service behind feature).
  strk20-cli      # bin `strk20`: run|backfill|serve|status|verify|snapshot|sync|bench.
e2e/              # acceptance test crate: MockRpc, capture proxy, the client test.
fixtures/         # vendored upstream devnet-state.json + cairo-reference-data.json (+ loader copy).
```

Dependency edges: `types ← feed ← {ingest, client, server}`; `store ← {ingest, server}`; `discovery-core ← {store, client}`; `cli ← all`. The client deliberately does **not** depend on the store: its mirror is a plain `HashMap<Felt,(Felt,u64)>`-backed structure fed by `strk20-feed`, which keeps the wasm/SDK-adapter path (U2 roadmap) free of sqlite and tokio.

## 3. Data model

### 3.1 SQLite (server-side local index; the DB is an index, the epoch files are the product)

```sql
PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;

CREATE TABLE meta (            -- schema_version, chain_id, pool_address, genesis_block,
  key TEXT PRIMARY KEY,        -- ingest_cursor_number, ingest_cursor_hash, l1_accepted_number,
  value TEXT NOT NULL          -- decode_status ('ok'|'degraded'), current_class_hash
);

CREATE TABLE blocks (          -- only ACTIVE blocks (pool touched) + checkpoint blocks
  number    INTEGER PRIMARY KEY,
  hash      TEXT NOT NULL UNIQUE,      -- 0x-hex, lowercase, minimal (canonical felt)
  parent_hash TEXT NOT NULL,
  new_root  TEXT NOT NULL,
  timestamp INTEGER NOT NULL,
  finality  INTEGER NOT NULL           -- 0 = ACCEPTED_ON_L2, 1 = ACCEPTED_ON_L1
);

CREATE TABLE slot_writes (     -- append-only pool storage diffs
  slot  TEXT NOT NULL,
  block INTEGER NOT NULL REFERENCES blocks(number) ON DELETE CASCADE,
  value TEXT NOT NULL,
  PRIMARY KEY (slot, block)
) WITHOUT ROWID;
CREATE INDEX slot_writes_block ON slot_writes(block);

CREATE TABLE events (
  block       INTEGER NOT NULL REFERENCES blocks(number) ON DELETE CASCADE,
  event_index INTEGER NOT NULL,        -- within-block emission order (from getEvents ordering)
  tx_index    INTEGER NOT NULL,        -- position of tx_hash in getBlockWithTxHashes
  tx_hash     TEXT NOT NULL,
  key0        TEXT NOT NULL,           -- selector, for filtered scans
  keys        TEXT NOT NULL,           -- JSON array of canonical hex felts
  data        TEXT NOT NULL,           -- JSON array
  PRIMARY KEY (block, event_index)
) WITHOUT ROWID;
CREATE INDEX events_key0 ON events(key0, block);

CREATE TABLE class_changes (   -- replaced_classes / ImplementationReplaced observations
  block INTEGER PRIMARY KEY,
  class_hash TEXT NOT NULL,
  decoder TEXT                          -- NULL = unknown → decode_status degraded from here
);

CREATE TABLE epochs (
  epoch       INTEGER PRIMARY KEY,      -- fixed alignment index (see 3.2)
  start_block INTEGER NOT NULL,
  end_block   INTEGER NOT NULL,
  content_hash TEXT NOT NULL,           -- sha256 over canonical uncompressed NDJSON
  prev_hash   TEXT NOT NULL,            -- chain link ("" for epoch 0)
  end_block_hash TEXT NOT NULL,
  anchor_storage_root TEXT,             -- pool storage_root from getStorageProof at end_block
  anchor_json TEXT,                     -- full stored proof response (auditable while fresh)
  raw_bytes INTEGER NOT NULL, zstd_bytes INTEGER NOT NULL,
  created_at INTEGER NOT NULL
);
```

Point read as-of-block (drives `RawStorageAccess`):
`SELECT value, block FROM slot_writes WHERE slot=?1 AND block<=?2 ORDER BY block DESC LIMIT 1` → absent = `Felt::ZERO` (Cairo map semantics, mirroring upstream MockBackend). `read_slots_with_block` returns `StorageResult{value, last_update_block=block}` — the fork-only RPC extension for free, which is the compat mode's structural advantage.

Rollback (reorg) = `DELETE FROM blocks WHERE number > ?rollback_to` (cascades), rewrite cursor. Rows ≤ l1_accepted are never deleted.

Epoch files live in `<data-dir>/feed/epoch-<E>-<start>-<end>.ndjson.zst` + `<data-dir>/feed/manifest.json`. `strk20 serve --feed-dir` can serve a feed with no DB at all (pure mirror).

Postgres (roadmap, U4-hosted-scale): the store is behind our own `Store` trait (blocking, small: put_block/put_diffs/put_events/rollback/read_slot_asof/scan_events/head); a `store-postgres` impl slots in without touching ingest/server. Not default; measurements don't justify it.

### 3.2 Epoch bundle wire format (the canonical feed — frozen at v1)

Deterministic by construction so any mirror reproduces identical bytes from any archive RPC.

- **Alignment:** epoch `E` covers absolute block range `[8_978_970 + E*10_000, 8_978_970 + (E+1)*10_000 - 1]` (mainnet; genesis and size come from chain config; tests use small sizes). An epoch is cut only when its end block ≤ current `l1_accepted` → immutable by construction.
- **File:** zstd(level 19) of canonical NDJSON. Content identity = `sha256:` over the **uncompressed** canonical bytes (compression level never changes identity).
- **Line 1 — header:**

```json
{"v":1,"kind":"strk20-epoch","chain_id":"SN_MAIN","pool":"0x40337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a","epoch":12,"start_block":9098970,"end_block":9108969,"prev":"sha256:ab12…","class_hash_at_end":"0x67dddd…","anchor":{"block":9108969,"block_hash":"0x…","storage_root":"0x…"}}
```

- **Then one line per ACTIVE block, ascending block number:**

```json
{"b":9099001,"h":"0x4a…","p":"0x3f…","t":1745700000,
 "d":[["0x1cf1a2…","0x1"],["0x5e77b3…","0x2f0a…"]],
 "e":[[0,0,"0x7d3…tx",["0x9149d2…Deposit_selector","0xuser"],["0xtoken","0xamount","0x0"]]],
 "rc":"0x67dddd…"}
```

  - `d` = pool `storage_diffs` entries, **sorted by slot ascending** (numeric felt order); `e` = pool events in emission order, each `[tx_index, event_index, tx_hash, keys[], data[]]`; `rc` present only when `replaced_classes` touched the pool that block.
  - Canonical felt encoding everywhere: lowercase `0x`-hex, no leading zeros (`0x0` for zero). JSON: no whitespace, keys in the fixed order shown, `\n` line terminator, UTF-8.
- **Anchor:** at cut time the indexer calls `getStorageProof(end_block_or_latest,…,[pool])` and stores the pool `storage_root` + `class_hash` (proof retention on lava is only ~25–55k blocks → anchor is captured promptly, kept forever in `anchor_json`). If the proof window was missed (backfill of old epochs) `anchor` is `null` — verifiability degrades honestly rather than fabricating.
  Note: because the anchor depends on when the proof was fetched, it lives in the header but is **excluded from the content hash** — the content hash covers header fields `v,kind,chain_id,pool,epoch,start_block,end_block,prev,class_hash_at_end` plus all block lines (the hash is computed over the header line serialized with `"anchor":null`). Two mirrors with differently-timed anchors still agree on `content_hash`.
- **Manifest** (`GET /feed/v1/manifest`):

```json
{"v":1,"chain_id":"SN_MAIN","pool":"0x4033…","genesis_block":8978970,"epoch_size":10000,
 "epochs":[{"epoch":0,"hash":"sha256:…","start_block":8978970,"end_block":8988969,"raw_bytes":412345,"zstd_bytes":130912,"anchor":null},
           {"epoch":1,"hash":"sha256:…", "…":"…"}],
 "head":{"l1_accepted":{"number":14056429,"hash":"0x…"},"latest":{"number":14062100,"hash":"0x…"},
         "class_hash":"0x67dddd…","decode_status":"ok"}}
```

- **Snapshot** (U4/U1 cold-start convenience): `snapshot-<block>-<hash8>.sqlite.zst`, content-addressed; import verifies by replaying nothing — it re-derives the slot map from epochs when `--verify` is passed, or trusts the content hash + spot-checks the anchor otherwise. Snapshots are convenience; the epochs are canon.

## 4. API surface

All feed endpoints are **GET, user-independent, cacheable** (ETag = content hash, `Cache-Control: public, max-age=31536000, immutable` for epochs). This is the privacy mechanism: the server cannot distinguish wallets because every wallet's requests are identical.

### 4.1 Feed (headline, always on)

| # | Method+Path | Request | Response |
|---|---|---|---|
| 1 | `GET /feed/v1/manifest` | — | manifest JSON (3.2). `Cache-Control: max-age=30`. |
| 2 | `GET /feed/v1/epoch/{E}` | — | epoch bundle, `Content-Type: application/x-strk20-epoch+zstd`, `ETag: "sha256:…"`, immutable. 404 if not yet cut. |
| 3 | `GET /feed/v1/epoch/by-hash/{sha256hex}` | — | same body, content-addressed (mirror-neutral fetching). |
| 4 | `GET /feed/v1/tail?after=14056429&hash=0x3f…` | cursor = last known (number,hash) | `200` NDJSON: block lines exactly as in 3.2 plus `"fin":"L2"\|"L1"` per line, covering `(after, latest]` not yet in a cut epoch. `409` `{"error":"REORGED","rollback_to":{"number":14056000,"hash":"0x…","l1_checkpoint":{"number":14051200,"hash":"0x…"}}}` when `hash` is no longer canonical. |
| 5 | `GET /feed/v1/head` | — | `{"l1_accepted":{…},"latest":{…},"last_epoch":507,"class_hash":"0x…","decode_status":"ok"}` |
| 6 | `GET /feed/v1/stream` | SSE | events: `block` (a 3.2 line + `fin`), `finality` (`{"l1_accepted":{number,hash}}`), `rollback` (`{"to":{number,hash}}`), `epoch` (`{"epoch":E,"hash":"sha256:…"}`). Global stream only — the privacy-optimal push (Q16); **no per-user subscription endpoint exists**. |
| 7 | `GET /feed/v1/anchor/{E}` | — | stored full `getStorageProof` response for the epoch head (`anchor_json`), or 404. |
| 8 | `GET /feed/v1/snapshot/latest` | — | `302` → content-addressed snapshot file + `{"block","hash","sha256"}` in `/feed/v1/snapshot/meta`. |

Example tail line:
`{"b":14062099,"h":"0x9c…","p":"0x1b…","t":1756400000,"d":[["0x2ee0…","0x5"]],"e":[[1,0,"0x88…",["0x1a2b…NoteUsed","0x7fnullifier…"],[]]],"fin":"L2"}`

### 4.2 Raw (targeted; **off by default**, `--enable-raw`, honestly labeled)

Every response carries header `X-Strk20-Privacy: targeted-mode-leaks-queried-slots` and the docs state: keyless-targeted equals direct RPC in leakage — the incoming path reveals the requesting address (Q7).

| Method+Path | Request | Response |
|---|---|---|
| `POST /raw/v1/slots` | `{"block":"l1_accepted","slots":["0x1cf1…","0x5e77…"]}` (`block`: number \| `"latest"` \| `"l1_accepted"`) | `{"block":{"number":14056429,"hash":"0x…"},"results":[{"slot":"0x1cf1…","value":"0x1","write_block":9099001},{"slot":"0x5e77…","value":"0x0","write_block":null}]}` |
| `GET /raw/v1/slots?prefix=1c&bits=8&block=l1_accepted` | prefix-bucket (the near-free PIR halfway, Q9): all slots whose felt's top `bits` match | NDJSON `{"slot","value","write_block"}` — Pedersen-uniform buckets; `bits=0` degenerates to the full slot dump (= perfect privacy). |
| `POST /raw/v1/events` | `{"from_block":9000000,"to_block":9100000,"keys":[["0x…NoteUsed"]],"limit":1000}` (per-position key filter, upstream `RawEventAccess` semantics) | `{"events":[{"block":…,"tx_index":…,"event_index":…,"tx_hash":"0x…","keys":[…],"data":[…]}],"continuation":null}` |

### 4.3 Compat mode (U8; feature `compat`, `--enable-compat`, loud startup warning)

Mounts the **unmodified reference `discovery-service::ApiServer`** (max-reuse depth from Q14) over our `DbBackend` (which implements `RawStorageAccess`/`RawEventAccess`/`StorageSnapshot`/`StorageBackend`/`ChainState`). Exact reference wire format, verbatim:

- `GET /health` → `{"status":"OK","chain_head":{"block_number":…,"block_hash":"0x…","timestamp":…},"lag_secs":2}`
- `POST /v1/sync/incoming_state` — body `{"contract_address":"0x4033…","viewing_key":"0x…RAW KEY (labeled!)","recipient_address":"0x…","last_known_block":"0x…?","block_ref":"l1_accepted?","cursor":{…}}` → `{"block_ref":…,"channels":[…],"subchannels":[…],"notes":[…],"cursor":{…}}`; 409 reserved for `BLOCK_REORGED` (SDK contract).
- `POST /v1/sync/outgoing_state`, `POST /v1/sync/preflight_check`, `POST /v1/history` — reference shapes, unchanged.

Privacy hygiene in compat mode: request bodies and `DiscoveryCursor`s are never logged (cursors embed `SecretFelt` channel-key material — treated as key-adjacent); `PublicKeyCache` memory-only; no request persistence; docs say plainly "this mode sees your viewing key — run it only for yourself." Note-`block_number` comes from `write_block` natively (no forked RPC needed) — compat mode on public RPC is something the reference service itself cannot do.

### 4.4 Metrics/health (U7, honest set only)

`GET /metrics/v1/stats` → per-token gross deposits/withdrawals + TVL, cumulative note count (global anonymity set), spend count, `ExternalContractInvoked` breakdown, registrations, upgrade history, indexer health (head lag, class-hash match, decode-error counters, epoch coverage). Excluded by policy (Q18): any Deposit↔Withdrawal join, per-tx timelines, per-token enc-note splits, nullifier "linkage", channel-count estimates. Plus Prometheus `GET /metrics` for ops.

## 5. Ingest pipeline

States: `Init → Backfill → Steady ⇄ Degraded → (Halted only on operator stop)`.

1. **Init:** open DB, load chain config (mainnet defaults: pool `0x4033…`, genesis 8,978,970, decoder map `{0x30b8c540…: v1, 0x67dddd89…: v2}` — the 7 discovery events are byte-identical, so one decoder actually covers both; the map exists for the *next* upgrade). Probe RPC (`starknet_specVersion`, real User-Agent — lava 403s default UAs).
2. **Backfill (deterministic):** events-first — `getEvents(address=pool, from=cursor, chunk=1024)` paged by continuation token to find active blocks (~0.23% of blocks; full history ≈ 5 min on lava). For each active block: `getStateUpdate` → filter `storage_diffs[pool]` + `replaced_classes`; `getBlockWithTxHashes` → hash/parent/timestamp/status + tx_hash→tx_index map; assign `event_index` from within-block getEvents order (fork `EmittedEvent` needs it — recorded at ingest, per the inventory risk note). One SQLite tx per block. Alternative seed: `strk20 backfill --from-feed URL` imports epochs from a mirror, verifies hash chain + anchors, then switches to RPC at the feed head.
   - **Events-first soundness net:** the assumption "every pool write lands in a block with ≥1 pool event" is checked, not trusted — channel/subchannel writes are event-silent but occur inside note-creating txs. At every epoch cut the indexer recomputes nothing less than the anchor check: fetch `getStorageProof` → compare pool `storage_root` derivability is roadmap, but the class_hash + a spot `getStorageAt` sample of K random mirrored slots against RPC (`--selfcheck-sample`, default 16) runs now. On mismatch → binary-search the epoch's block range with per-block `getStateUpdate` (whole-block diffs are ~4 KB median) to find the silent write, ingest it, log loudly. Falsifies the assumption instead of silently missing state.
3. **Steady:** poll `getBlockWithTxHashes("latest")` + `getBlockWithTxHashes("l1_accepted")` every ~2 s (or block cadence); run the same per-active-block routine over the new range; push lines to SSE; advance cursor `(number, hash)` with parent-hash linkage check.
4. **Reorg handling:** cursor mismatch (parent_hash ≠ stored hash) → walk back through stored blocks to the last canonical hash (bounded below by the l1_accepted checkpoint — never crosses it), `DELETE` rows above it, emit `rollback{to}` on SSE, re-ingest forward. Sized for the Grinta precedent (~4,000+ blocks en-masse L2 revocation). Epochs are unaffected by construction (cut ≤ l1_accepted). `/feed/v1/tail` answers 409 with both `rollback_to` and the last `l1_checkpoint` so clients rewind minimally (improvement over upstream's "re-sync from scratch").
5. **Epoch cutting:** when `l1_accepted ≥` an uncut epoch's end block → serialize canonical NDJSON from DB, sha256, zstd, write file, fetch + store anchor proof, append to manifest, emit `epoch` SSE event. Deterministic: two independent operators produce byte-identical files (acceptance-tested, A6).
6. **Degraded (unknown class):** on `replaced_classes`/`ImplementationReplaced` with a class hash not in the decoder map: keep ingesting and archiving RAW diffs + events (layout-agnostic → the feed and mirrors stay alive and reproducible), keep cutting epochs, set `decode_status:"degraded"` in head/manifest, compat mode answers 503 `SERVICE_UNAVAILABLE`, metrics freeze typed counters. Resume to Steady when a human maps the class (config update). Strictly better than upstream's hard 503-everything.
7. **Upgrade insurance:** `upgrade_delay = 0` on-chain → this path is not theoretical; it is exactly how the one real upgrade happened. Periodic `getClassHashAt` cross-check (each epoch anchor also carries the class hash for free).

## 6. Keyless client (`strk20-client`)

Transport is deliberately incapable of expressing a targeted query:

```rust
#[async_trait] pub trait FeedTransport: Send + Sync {
    async fn manifest(&self) -> Result<Manifest>;
    async fn epoch(&self, e: u64) -> Result<Bytes>;               // by index or content hash
    async fn tail(&self, cursor: FeedCursor) -> Result<TailPage>; // 409 → Rollback(to, l1_checkpoint)
}
```

No method takes an address, key, slot, or any user-derived value — the **type system is the privacy boundary** (and `SecretFelt` is `!Serialize` upstream, enforced by a compile-fail test). Impls: `HttpTransport` (reqwest), `DirTransport` (a mirror on disk), `MemTransport` (tests).

- **`LocalMirror`:** downloads/verifies epochs (content hash vs manifest, `prev` chain, per-block parent linkage) + tail; applies lines to `slot → (value, write_block)` + event log; persists optionally to a compact file (`mirror.bin`, postcard) so cold start is once (~6 MB zstd today). Implements `RawStorageAccess` (3 methods; map lookups) → upstream blanket `impl<T: RawStorageAccess> IViews for T` drives the **unmodified** engine; implements `RawEventAccess` from the event log (per-position key filters, MockEventBackend semantics) for `/history`-equivalent flows.
- **`WalletSync`:** holds `(address, SecretFelt viewing_key)`; `sync()` = transport catch-up → `sync_incoming_state` + `sync_outgoing_state` (+`preflight_check` on demand) with `IoBudget`/`CursorLimits` mirroring reference defaults → computes the nullifier set for discovered notes → spent-state machine `unknown → unspent → spent` driven incrementally by `NoteUsed` events (nullifier emitted verbatim) and/or nullifier-slot diffs in subsequent tail lines. On `Rollback`: rewind mirror + `DiscoveryCursor` to the l1 checkpoint (both are persisted snapshots), resync.
- **Cursor persistence:** `client-state/` dir, files chmod 0600: `feed_cursor.json` (number, hash, epoch), `discovery_cursor.bin` (serde of upstream `DiscoveryCursor` — contains `SecretFelt`-derived channel keys → documented sensitive, local-only, never transmitted; feed mode has no server-side cursor at all), `nullifiers.bin`.
- **U3 (payment backend):** same crate, `WalletSync::watch(stream)` consumes `/feed/v1/stream` SSE; incoming detection = channel-count slot for its own address changing in a diff line (computed locally — the server never learns which slots matter) + trial-decrypt of new channel elements; spend detection = `NoteUsed` nullifier match. Latency = block cadence + SSE push; zero per-user polling; the server sees one long-lived global-stream connection indistinguishable from any mirror.
- **U6 (auditor):** `strk20 verify` — for a sample of mirrored slots and for the wallet's own note/nullifier slots, fetch `starknet_getStorageProof` from an independent RPC and compare values + pool storage_root vs the epoch anchor; per-note **non-membership** (nullifier slot = 0) is provable. Full MPT verification (pedersen walk → contract leaf → poseidon state commitment → header `new_root`) is a defined roadmap crate (`strk20-proofs`) — the hook (anchors stored per epoch, prompt fetch inside the retention window) is in v1, the math lands without re-architecture.
- **U2 (SDK drop-in), roadmap without re-architecture:** `strk20-client` is wasm-clean by construction (no sqlite/tokio; discovery-core verified to compile wasm32 untouched). Path: `strk20-client-wasm` (wasm-bindgen, key in as `Uint8Array`, `?Send` via `SendWrapper`) → TS class `LocalDiscoveryProvider implements DiscoveryProviderInterface` (3 methods + `fetchHistory` mirror), reusing the SDK's exported cursor-mapping functions' semantics so `NotesCursor` round-trips. Near-term U2 fallback that ships in v1: self-hosted compat mode + the stock `IndexerDiscoveryProvider` (key goes to *your own* box — labeled).
- **CLI reference client (ships in v1, used by the acceptance test):** `strk20 sync --address 0x… --viewing-key-file k.hex --feed-url http://… [--json]` → prints channels/subchannels/notes (token, amount, note_index, maturity block, spent?) and persists cursors.

## 7. CLI

```
strk20 run        --rpc-url URL [--db PATH] [--data-dir DIR] [--listen 0.0.0.0:8080]
                  [--enable-raw] [--enable-compat] [--chain mainnet|custom.toml]
strk20 backfill   --rpc-url URL [--from-feed URL] [--until l1_accepted]
strk20 serve      --feed-dir DIR [--listen …]          # DB-less pure mirror
strk20 status     [--url http://localhost:8080]        # head, lag, epochs, decode_status
strk20 verify     [--sample N] [--epoch E] [--cross URL2] [--rpc-url URL]
strk20 snapshot   export|import [--verify]
strk20 sync       --address 0x… --viewing-key-file F [--feed-url URL] [--watch]
strk20 bench      b1|b3|b5|b8 …
```

U4 = `strk20 run --rpc-url https://rpc.starknet.lava.build` — one binary, one flag, SQLite file appears next to it. U5 = `strk20 backfill --from-feed` + `strk20 verify --cross`.

## 8. Acceptance E2E client test (the branch's definition of done)

`e2e/tests/acceptance.rs` — runs in CI, no network, exercises the **real compiled binary** over **real HTTP** with the client using the public API exactly as a wallet would.

**Seed data.** Upstream fixture `fixtures/devnet-state.json` (vendored; 48 slot→value pairs, pool `0x66292db2…`, alice `0x34ba56f9…`/key `0xa11ce`, bob `0x2939f2dc…`/key `0xb0b`, eth+strk tokens, block 46). A test-only `MockRpc` (in-process axum JSON-RPC server) synthesizes a chain from it:

- Blocks 1–46, synthetic hashes `h(n) = poseidon(n)`-style deterministic felts, correct parent links; `l1_accepted` initially = 40, `latest` = 46 (so the tail path is exercised too).
- The 48 slots are partitioned deterministically into diffs at blocks {10, 20, 46}: registration slots (`public_key`/`enc_private_key`) at 10, channel/counter slots at 20, note/subchannel remainder at 46. The partition function is committed in the test helper so ground truth `write_block` per slot is known.
- Synthesized events per active block so the production events-first path runs unmodified: `ViewingKeySet` at 10, `EncNoteCreated` (dummy commitment) at 20 and 46. No `NoteUsed` initially.
- Serves: `getEvents` (with continuation-token pagination — chunk size 2 to exercise paging), `getStateUpdate`, `getBlockWithTxHashes` (incl. `"l1_accepted"`/`"latest"` tags), `getClassHashAt`, `getStorageProof` (canned anchor). Epoch size set to 16 via `--chain custom.toml` → epochs {1–16}, {17–32} cut, 33–46 in tail: multi-epoch + tail coverage.
- **Capture:** MockRpc and a recording reverse-proxy in front of the indexer's HTTP listener log every request: method, path, query, full body bytes.

**Ground truth.** Computed in-process by running the unmodified `discovery-core` engine over upstream's own `MockBackend` loaded with the same 48 slots (upstream's canonical path): `SyncIncomingStateResult`/`SyncOutgoingStateResult` for alice and bob. Not hand-written expectations — the reference engine over the reference fixture is the oracle; our stack must reproduce it **bit-identically**.

**Procedure.**
1. Start MockRpc (ephemeral port).
2. Spawn the real `strk20` binary: `strk20 run --rpc-url http://127.0.0.1:P --chain e2e.toml --db $TMP/ix.db --data-dir $TMP --listen 127.0.0.1:0`.
3. Poll `GET /feed/v1/head` until `l1_accepted.number == 40` and `last_epoch == 1`.
4. Test client = `strk20-client` with `HttpTransport` at the indexer URL (also once via the spawned `strk20 sync --json` CLI to cover the shipped client path): manifest → epochs → tail → `WalletSync::sync()` for alice, then bob.

**Assertions.**
- **A1 (keyless discovery correctness):** alice's and bob's incoming + outgoing results == MockBackend ground truth, field-for-field: every note's `(token, amount, note_index, note_id, nullifier)`, channel/subchannel sets, `cursor.is_complete() == true`. Exact equality, no tolerance.
- **A2 (write-block provenance):** each discovered note's `block_number` equals the block where the test's partition function placed its slot ({10,20,46}) — proves diff-derived `last_update_block`, the capability plain RPC lacks.
- **A3 (incremental resume):** phase 1 runs with MockRpc truncated at block 20 (subset of slots); client syncs, persists cursors, is dropped. MockRpc extends to 46; a fresh client with the persisted state syncs again. Assert: final result == full ground truth AND the capture shows the second client fetched **no epoch file it already had** (only manifest/head/tail + the new epoch) — resume is O(delta).
- **A4 (the negative assertion — no key ever reached any server, mechanical):**
  1. Byte-scan every captured request (both servers: indexer HTTP and MockRpc) for every encoding of the secrets: `0xa11ce`/`0xb0b` as minimal hex, 64-char zero-padded hex, decimal ASCII, raw 32-byte big- and little-endian; likewise both addresses (alice/bob) in any indexer-bound request. Assert zero matches.
  2. Assert the client issued **only** `GET`s to the indexer, every path ∈ {`/feed/v1/manifest`, `/feed/v1/epoch/*`, `/feed/v1/tail`, `/feed/v1/head`}, and every query string ∈ {`after`,`hash`} with values that are (block_number, block_hash) pairs published by the server itself — i.e. request contents are a function of public data only.
  3. Compile-time: a `compile_fail` (trybuild) test proving `SecretFelt: !serde::Serialize` and that `FeedTransport` has no method accepting `SecretFelt`/address — regression-locks the type-level boundary.
- **A5 (reorg):** MockRpc replaces blocks 44–46 with a fork (new hashes, one changed diff), `latest` → 47. Indexer detects parent mismatch, rolls back to 43, tail/SSE emit `rollback`; a syncing client receives 409 with `rollback_to`, rewinds to its l1 checkpoint (40), resyncs; final result == post-fork ground truth (recomputed over MockBackend with the forked slot set). Epoch files unchanged (all ≤ l1_accepted).
- **A6 (mirror determinism, U5):** a second `strk20 run` instance, fresh DB, same MockRpc → assert its epoch files are **byte-identical** (sha256 equal) to the first's, and `strk20 verify --cross` reports agreement.
- **A7 (spent-state):** append block 48: diff writes `nullifiers[n] = 1` for one of alice's ground-truth nullifiers + `NoteUsed{nullifier: n}` event; `l1_accepted` → 48. Client tail-syncs; assert exactly that note flips to spent, all others unspent.
- **A8 (compat, U8):** restart indexer with `--enable-compat`; POST reference-wire `/v1/sync/incoming_state` with alice's raw key; response notes == ground truth; then POST with `last_known_block` = a forked-out hash → 409 `BLOCK_REORGED`. (Complements a ported subset of upstream's 11 HTTP tests run as conformance layer.)

**Also in the suite (not the acceptance gate):** `#[ignore]`d mainnet smoke (`backfill` first 200k blocks on lava, spot-check anchor + `getClassHashAt`), run manually/nightly.

## 9. Testing strategy (layers under the acceptance test)

1. **Unit:** `strk20-feed` canonical-encoding golden vectors (felt canonicalization, line ordering, content-hash stability incl. the anchor-exclusion rule); `strk20-store` as-of-block reads, rollback cascade (property test: apply random diffs + rollbacks == naive model); ingest continuation-token paging.
2. **Conformance (upstream assets, vendored):** (a) engine-over-`DbBackend` == engine-over-`MockBackend` on `devnet-state.json` — proves our storage traits are semantically identical to upstream's reference impl; (b) `cairo-reference-data.json` decryption vectors run through `LocalMirror` (slots loaded from the fixture's 14 slot addresses); (c) compat wire format: ported request/response bodies from upstream's 11 HTTP tests replayed against our compat mount.
3. **Acceptance:** §8, gating CI.
4. **Bench harness (`strk20 bench`)**: B1 backfill wall-clock, B3 RPC-reads-per-user-sync (must print 0), B5 slab bytes/time vs gap, B8 crossover — pinned block hash, p50/p95 over ≥20 runs (Q19).

## 10. What this lens sacrifices (honest)

- **Cold-start latency and client CPU.** No targeted lookups by default → a new wallet downloads ~6 MB (today) and trial-decrypts locally. A targeted server round-trip would be faster and lighter; we make the user pay bandwidth + CPU for not being identified. At ~100× growth mobile cold start needs the snapshot path; at ~8×10⁵ records the documented PIR trigger fires and this design needs a new mode (hook exists: prefix-bucket endpoint).
- **U3 convenience.** No per-user push, ever, by default. Payment backends must run the trial-decryption loop themselves off the global stream instead of getting "your address got a note" webhooks. That is a real integration cost, accepted deliberately (per-user subscriptions are a durable fingerprint).
- **U2 lands in two steps.** The pure-privacy TS drop-in (wasm `LocalDiscoveryProvider`) is roadmap; v1 offers Rust client + CLI + self-hosted compat mode for SDK users. A convenience-first design would ship the hosted keyed API on day one and win integrations faster.
- **U8 friction.** Compat mode behind a feature flag + loud warning + no hosted default deliberately makes the convenient thing harder, because its convenience is exactly the leak.
- **Format rigidity.** Deterministic canonical encoding + fixed epoch alignment mean any format change is a feed-identity break (v2 namespace, dual-serve migration). Verifiability buys inflexibility.
- **Trust honesty, not trust elimination.** Completeness of the feed remains unprovable; we ship audits (anchors, cross-mirror hashes, spot proofs) and say "delegated trust with random audits; trustless fallback is self-hosting" instead of claiming trustlessness. The full MPT proof verifier is roadmap — until it lands, anchor checks compare values, not proofs.
- **More moving parts than a plain query API.** Epoch cutter, manifest, canonicalization, tail/rollback protocol — a keyed lookup service would be a fraction of the code. The extra machinery *is* the product, but it is honest to count it as cost.
