# strk20-indexer architecture — adoption-compat proposal

LENS: ecosystem-adoption-first. Optimize the path from "wallet team hears about us" to "notes flow in their app": official-SDK drop-in with zero code changes, compatible /v1/sync as a first-class mode, TS/wasm keyless client as the upgrade path, hosted multi-tenant operation. Accept operational complexity.

## 0. Pitch

One binary, three integration tiers on one engine and one database:

- **Tier 0 (zero code): compat mode.** We mount the unmodified reference `discovery-service` `ApiServer` over our local DB backend. Any app already using `@starkware-libs/starknet-privacy-sdk`'s `IndexerDiscoveryProvider` sets `INDEXER_URL` to us and works today (issues #121/#221 are literally teams asking for this URL). Wire format frozen RC.0→RC.5; 409=BLOCK_REORGED preserved. Explicitly labeled keyed mode (viewing key visible to the operator).
- **Tier 1 (npm install): keyless drop-in.** `@strk20/discovery-provider` — a TS class implementing the SDK's 3-method `DiscoveryProviderInterface`, driving upstream `discovery-core` compiled to wasm, fed by our public feed. Same `createPrivateTransfers({discoveryProvider})` slot; key never leaves the page.
- **Tier 2 (protocol): the feed.** Content-addressed epoch bundles + live tail — the CDN-cacheable, mirrorable, verifiable public artifact any third party can consume without us.

Everything reuses upstream verbatim: discovery-core engine (git tag `CONTRACT_V2_DEPLOYED_MAINNET_2026-07-08` = rev `74841caf…`, starknet-rust fork rev `7caedfe`), discovery-service ApiServer, upstream fixtures as conformance oracles. Zero forked discovery logic anywhere.

## 1. Components (cargo workspace + ts/)

```
strk20-indexer/
├─ crates/
│  ├─ strk20-types        # Felt/hex canonicalization, config structs, class-hash→decoder map.
│  │                      # Pins: starknet-types-core =0.2.x (deny 1.x — different Felt), fork starknet-core rev 7caedfe.
│  ├─ strk20-store        # Store trait + SQLite impl (rusqlite bundled, WAL, spawn_blocking).
│  │                      # feature "postgres" = roadmap impl behind the same trait. deps: types.
│  ├─ strk20-feed         # Epoch/snapshot wire format: serializer + deserializer + manifest + hash chain.
│  │                      # SHARED by server and client → format drift structurally impossible. deps: types, zstd, sha2.
│  ├─ strk20-ingest       # Raw JSON-RPC client (reqwest + own serde structs; real UA for lava),
│  │                      # pipeline states, reorg walkback, upgrade watch, storage-proof spot-check.
│  │                      # deps: store, feed, types.
│  ├─ strk20-backend      # DbBackend/DbSnapshot: impl RawStorageAccess(3 fns)+RawEventAccess+StorageSnapshot+
│  │                      # StorageBackend + service ChainState over strk20-store. The trait bridge that makes
│  │                      # the unmodified engine + reference ApiServer run on our DB. deps: store, discovery-core.
│  ├─ strk20-api          # axum 0.8: feed/raw/stats/health/metrics routes; feature "compat" (default on)
│  │                      # mounts discovery-service::ApiServer<DbBackend>. deps: backend, feed, store, tower-http.
│  ├─ strk20d             # bin: clap CLI (run/backfill/status/snapshot/epoch/mirror/verify-proof/sync/bench).
│  ├─ strk20-client-core  # Keyless client: wraps discovery-core; 3 transports (RPC / RawApi / Mirror+FeedSync);
│  │                      # cursor persistence. deps: discovery-core, feed, reqwest(native)/js(wasm).
│  ├─ strk20-client-wasm  # wasm-bindgen wrapper; SendWrapper for ?Send; JSON shapes = reference api/types.rs.
│  └─ e2e-tests           # mock RPC + recording proxy + THE acceptance test (spawns real strk20d binary).
├─ ts/packages/discovery-provider   # LocalDiscoveryProvider implements DiscoveryProviderInterface (3 methods)
│                                   # + fetchHistory; cursor conversion mirroring SDK's notesCursorToApiCursor etc.
└─ fixtures/              # vendored upstream devnet-state.json, cairo-reference-data.json, devnet-dump.json.gz
                          # + provenance file (tag + sha256) — insurance against mutable upstream tags.
```

Dependency edges: types ← store ← {feed-ish none, ingest, backend} ; backend ← api ; {store,feed,ingest,backend,api} ← strk20d ; {discovery-core, feed} ← client-core ← client-wasm ← ts adapter. e2e-tests dev-depends on strk20d (assert_cmd), client-core, discovery-core (MockBackend as oracle).

Compat reuse depth: **max reuse** — depend on `discovery-service` as a lib and instantiate `ApiServer<DbBackend>`. We get validators (viewing-key pubkey check), error mapping, OHTTP envelope, and byte-exact wire behavior for free; that byte-exactness is the adoption lens's whole point. Match its tower-http/axum minors to avoid duplicate layers. Fallback (feature `compat-thin`) = own axum layer + copied `api/types.rs` (~350 lines) if the dep tree ever breaks.

## 2. Data model (SQLite, canonical DDL)

Felts stored as 32-byte big-endian BLOBs; hex only at the API edge.

```sql
PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;

CREATE TABLE meta(key TEXT PRIMARY KEY, value TEXT NOT NULL);
-- keys: schema_version, chain_id, pool_address, genesis_block(8978970), epoch_size

CREATE TABLE blocks(
  number      INTEGER PRIMARY KEY,
  hash        BLOB NOT NULL UNIQUE,
  parent_hash BLOB NOT NULL,
  new_root    BLOB NOT NULL,
  timestamp   INTEGER NOT NULL,
  finality    TEXT NOT NULL CHECK(finality IN ('L2','L1'))
);  -- only pool-active blocks + head/l1 checkpoints; (number,hash) is the cursor currency

CREATE TABLE storage_diffs(          -- pool-only, append-only history
  slot BLOB NOT NULL, block_number INTEGER NOT NULL REFERENCES blocks(number) ON DELETE CASCADE,
  value BLOB NOT NULL,
  PRIMARY KEY(slot, block_number)
) WITHOUT ROWID;                     -- as-of-block read = max(block_number)<=snapshot per slot

CREATE TABLE storage_latest(         -- hot-path cache for read_slot(latest)
  slot BLOB PRIMARY KEY, value BLOB NOT NULL, write_block INTEGER NOT NULL
) WITHOUT ROWID;

CREATE TABLE events(                 -- pool-only
  block_number INTEGER NOT NULL REFERENCES blocks(number) ON DELETE CASCADE,
  event_index  INTEGER NOT NULL,     -- within-block order, assigned at ingest (fork EmittedEvent needs it)
  tx_index     INTEGER NOT NULL,     -- position of tx_hash in getBlockWithTxHashes.transactions
  tx_hash      BLOB NOT NULL,
  key0 BLOB, key1 BLOB, key2 BLOB, key3 BLOB,   -- first 4 keys denormalized for filters
  keys BLOB NOT NULL, data BLOB NOT NULL,        -- concatenated 32B felts (len%32==0)
  PRIMARY KEY(block_number, event_index)
) WITHOUT ROWID;
CREATE INDEX idx_events_key0 ON events(key0, block_number);

CREATE TABLE class_history(
  from_block INTEGER PRIMARY KEY, class_hash BLOB NOT NULL,
  decoder TEXT                      -- 'v1' | 'v2' | NULL = unknown → typed layers degraded from here
);

CREATE TABLE epochs(
  epoch INTEGER PRIMARY KEY, from_block INTEGER NOT NULL, to_block INTEGER NOT NULL,
  raw_sha256 BLOB NOT NULL,         -- content address = hash of UNCOMPRESSED NDJSON (zstd is not stable across versions)
  zst_sha256 BLOB NOT NULL, bytes INTEGER NOT NULL,
  prev_raw_sha256 BLOB NOT NULL,    -- hash chain
  anchor_block INTEGER, anchor_storage_root BLOB, anchor_class_hash BLOB,
  created_at INTEGER NOT NULL
);

CREATE TABLE cursors(id TEXT PRIMARY KEY, value TEXT NOT NULL);
-- ids: backfill_events_token, backfill_block, follow_head, l1_head
```

Rationale vs research §Q17: unchanged (SQLite + epoch files canonical; Postgres behind the Store trait for hosted multi-instance, roadmap). The epoch files are the product; the DB is an index over them. `read_slot` semantics = value at max write ≤ snapshot block, default `Felt::ZERO` (Cairo map semantics, mirrors MockBackend). `read_slots_with_block` → `StorageResult{value, last_update_block=write_block}` — the fork-only RPC extension for free. Event filter = per-position key-set match (MockEventBackend semantics).

## 3. Epoch / feed wire format (frozen, versioned)

Epochs are absolute-aligned windows of `epoch_size = 10_000` blocks: epoch *e* covers `[e*10000, (e+1)*10000)`; first pool epoch = 897. An epoch is cut only when `to_block ≤ l1_accepted` → immutable by construction. File `epoch-<e>.ndjson.zst` = zstd(level 19) over canonical NDJSON:

```jsonl
{"t":"hdr","v":1,"chain":"SN_MAIN","pool":"0x040337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a","epoch":1163,"from":11630000,"to":11639999,"prev":"<hex sha256 of previous epoch raw bytes; 64 zeros for first>"}
{"t":"blk","n":11632886,"h":"0x…","p":"0x…","ts":1751980000,"d":[["0x<slot>","0x<value>"],…],"e":[{"tx":"0x…","ti":0,"ei":0,"k":["0x…"],"d":["0x…"]}],"rc":"0x67dddd89…76b554d"}
{"t":"end","blocks":121,"diffs":804,"events":530,"anchor":{"block":11639999,"block_hash":"0x…","storage_root":"0x…","class_hash":"0x…"}}
```

- Only pool-active blocks appear (pool storage_diffs ∪ pool events ∪ `rc`); `rc` present only when `replaced_classes` touched the pool. `d` sorted by slot asc; `e` sorted by `ei`; blocks ascending.
- Canonical JSON: exact field order as above, no whitespace, lowercase minimal hex (`0x0` for zero), `\n` line endings. Determinism rule: same chain data ⇒ byte-identical raw NDJSON on every mirror ⇒ identical `raw_sha256`. Omission attack ⇒ visible hash fork (Q11).
- Manifest `feed.json` (mutable head, small):
```json
{"v":1,"chain":"SN_MAIN","pool":"0x040337…","epoch_size":10000,"genesis_epoch":897,
 "head_epoch":1405,"l1_block":14056429,
 "epochs":[{"e":897,"raw":"<sha256>","zst":"<sha256>","bytes":10312},…],
 "snapshot":{"block":14050000,"raw":"<sha256>","zst":"<sha256>","bytes":492113}}
```
- Snapshot format (cold-start convenience; feed replay is the ground truth): `snapshot-<block>.ndjson.zst` = `{"t":"snap-hdr",…}` + one `{"t":"slot","s":"0x…","v":"0x…","w":<write_block>}` per slot (sorted by slot) + `{"t":"end","anchor":{…}}`. Import verifies against the epoch chain or a storage proof.
- Live tail (past last cut epoch): same `blk` line schema plus `"fin":"l2"|"l1"`, heartbeat `{"t":"hb","head":N,"l1":M}`, and `{"t":"rollback","to":N,"to_hash":"0x…"}` on reorg.

## 4. API surface

All bodies JSON unless noted; errors `{"error":{"code","message","details?"}}`; 409 reserved exclusively for `BLOCK_REORGED` (SDK contract).

### Feed (keyless bulk — headline privacy mode)
- `GET /v1/feed/manifest` → manifest above. `Cache-Control: max-age=30`.
- `GET /v1/feed/epoch/{e}` → `application/zstd` file. Headers: `ETag: "<raw_sha256>"`, `X-Content-Sha256-Raw`, `Cache-Control: public, max-age=31536000, immutable`. CDN-perfect: identical for every user.
- `GET /v1/feed/snapshot/latest` → snapshot file, same header discipline.
- `GET /v1/feed/delta?from_block=14050000&from_hash=0x…` → one-shot `application/x-ndjson` of `blk` lines after the cursor (with `fin` tags). `404 UNKNOWN_CURSOR` if pruned; **409 BLOCK_REORGED** if `from_hash` non-canonical, body includes `{"details":{"rewind_to":{"number":N,"hash":"0x…"}}}` = last L1-final checkpoint.
- `GET /v1/feed/live?from_block&from_hash` → chunked NDJSON stream: backlog then follow; heartbeats every 15 s; `rollback` lines on reorg. (SSE flavor at `/v1/feed/live/sse` for browsers, same payloads in `data:`.)

### Raw keyless targeted (documented lower-privacy convenience; = direct RPC in leakage)
- `POST /v1/raw/slots`
  req `{"block_ref":"latest"|{"block_number":N}|{"block_hash":"0x…"},"slots":["0x…", …]}` (≤256)
  → `{"block_ref":{"number":14056429,"hash":"0x…","finality":"L1"},"results":[{"slot":"0x…","value":"0x…","write_block":11632901},{"slot":"0x…","value":"0x0","write_block":null}]}`
- `POST /v1/raw/events`
  req `{"from_block":8978970,"to_block":9100000,"keys":[["0x<selector>"],[]],"chunk_size":1000,"continuation_token":null}` (per-position key-set semantics)
  → `{"events":[{"block_number":…,"block_hash":"0x…","transaction_hash":"0x…","transaction_index":3,"event_index":7,"keys":[…],"data":[…]}],"continuation_token":"9100000-0"}` — note `transaction_index`/`event_index` present (fork `EmittedEvent` shape; standard RPC can't serve this).
- `GET /v1/raw/slots/prefix?bits=12&prefix=0xabc&block_ref=latest` → `{"block_ref":…,"slots":[["0x<slot>","0x<value>",write_block],…]}` — the Q9 prefix-bucket halfway (k client-chosen; k=0 = full range).

### Compat (Tier 0 — reference wire, first-class)
Mounted unmodified from `discovery-service`: `GET /health`; `POST /v1/sync/incoming_state`, `/v1/sync/outgoing_state`, `/v1/sync/preflight_check`, `/v1/history`; optional OHTTP envelope fallback. Shapes exactly `api/types.rs` (e.g. incoming req = `{contract_address, viewing_key, last_known_block?, block_ref?, cursor, recipient_address}` → `{block_ref, channels, subchannels, notes, cursor}`). Additions, non-breaking: response header `X-Strk20-Mode: compat-keyed` on every compat route; `compat.enabled` config (default true, off ⇒ 404); hard default `log_bodies=false` on these routes (bodies carry viewing keys; cursors carry SecretFelt channel_keys — treat as key-adjacent, never log/persist).

### Ops / explorer (honest set only, Q18)
- `GET /health` (reference shape: `{status, chain_head, lag_secs}`) — shared with compat.
- `GET /v1/status` → `{head:{number,hash,finality},l1_head,backfill:{done,current,target},epochs:{head_epoch,count},class:{hash,decoder,degraded_since?},providers:[{url,ok,latency_ms}]}`.
- `GET /v1/stats` → per-token deposits/withdrawals/TVL, cumulative note count (global anonymity set), spend count, `ExternalContractInvoked` breakdown, registrations, upgrade history. Nothing joining deposit/withdraw addresses.
- `GET /metrics` → Prometheus.

### Hosted multi-tenant layer (adoption lens; config-gated, default off)
`[hosted]` config: optional bearer API keys (per-key rate limits via tower layer), anonymous tier for feed routes (they're CDN-cacheable anyway — keys never required there), per-route quotas for compat/raw, permissive CORS. Multi-pool ready: all tables already key implicitly by the single configured pool; hosted multi-pool = one DB+feed dir per pool, one process, path prefix `/pools/{addr}/v1/…` (roadmap flag, no schema change).

## 5. Ingest pipeline

States: `INIT → BACKFILL → FOLLOW` with side jobs `EPOCH_CUT`, `PROMOTE_L1`, `VERIFY`, and interrupts `REORG`, `UPGRADE`.

1. **INIT**: open DB, check `meta` (chain_id via `starknet_chainId`, pool address, schema_version); verify current class hash via `getClassHashAt` against `class_history`/config map.
2. **BACKFILL** (events-first, deterministic): `getEvents(address=pool, chunk 1000)` from block 8,978,970 → active-block set (this also catches the upgrade: `ImplementationReplaced` is an event). For each active block (bounded concurrency 8): `getStateUpdate` (pool storage_diffs + `replaced_classes` + `deployed_contracts`) + `getBlockWithTxHashes` (hash/parent/timestamp/tx order → `tx_index`; `event_index` assigned by within-block event order from getEvents). Write blocks/diffs/events transactionally per block; update `storage_latest`. Cursors `backfill_events_token`, `backfill_block` make it resumable. UA header set (lava 403s default UAs); provider failover list (lava primary, publicnode secondary); retry w/ backoff. Determinism: DB content is keyed by block, independent of fetch order ⇒ epoch bytes identical across runs/mirrors.
3. **FOLLOW**: poll `getBlockWithTxHashes("latest")` every 2 s. New head → `getEvents` over the gap window → state updates for active blocks → append + stream to `/v1/feed/live` subscribers with `fin:"l2"`. Every 60 s poll `getBlockWithTxHashes("l1_accepted")` → `PROMOTE_L1`: mark `blocks.finality='L1'` up to it, emit `fin` upgrades on the live stream.
4. **EPOCH_CUT**: when `l1_head ≥ (e+1)*10000-1` for the next uncut epoch e: serialize from DB (sorted queries → canonical NDJSON), `getStorageProof(anchor_block…)`→ anchor (proof window is ~25–55k blocks: anchor fetched at cut time, never archivally), hash, chain to `prev`, write file + manifest atomically.
5. **REORG** (FOLLOW-time parent-hash mismatch or fetched-block hash change): walk back through `blocks` to the fork ancestor (only L2 blocks are rewritable); delete blocks > ancestor (cascades diffs/events), rebuild `storage_latest` for touched slots from `storage_diffs`; emit `{"t":"rollback","to":…}`; resume FOLLOW. Cursors are (number,hash) everywhere; epoch files unaffected (≤ L1 by construction). Compat mode: `last_known_block` canonicity check → 409, unchanged reference behavior.
6. **UPGRADE**: `replaced_classes` touching the pool at block b → insert `class_history(b, hash, decoder?)`. Known hash (config map: `0x30b8c540…`→v1, `0x67dddd89…`→v2; the 7 discovery events byte-identical, one decoder) → continue. Unknown → **raw ingest and feed continue untouched** (layout-agnostic, mirrors stay reproducible); typed layers (stats, history decode) halt at b; compat sync + client engine refuse `block_ref ≥ b` with `SERVICE_UNAVAILABLE` + `/health` degraded (storage layout may have changed; answers ≤ b remain valid). Human maps the class → set decoder → resume.
7. **VERIFY** (background, every 30 min): `getStorageProof(latest, pool, sample of 16 recently-written + 16 random slots)` → compare values and pool `storage_root` against DB reconstruction; mismatch → alarm metric + halt EPOCH_CUT (never publish a divergent epoch). Second-provider sampling optional.

Backfill cost today: ~28k active blocks, ~47 min on lava (measured upstream research). `strk20d mirror pull <url>` replaces steps 2–3's RPC with another instance's feed (verify hash chain + spot proofs) — U5 in one command.

## 6. Keyless client

**strk20-client-core** (Rust): wraps unmodified discovery-core. Because `impl<T: RawStorageAccess> IViews for T`, each transport only implements `read_slot / read_slots / read_slots_with_block` (+ `RawEventAccess::get_events` for history):
- `MirrorTransport` (default, bulk mode — the honest privacy mode): `FeedSync` pulls manifest → missing epochs → delta/live; verifies hash chain + canonical-bytes sha256; applies into a local mirror (native: SQLite via strk20-store; wasm: in-memory map + IndexedDB persistence adapter). All reads answered locally; `write_block` native from the feed. Server sees only epoch GETs — no key, no slots, no address.
- `RawApiTransport` (targeted, documented leakage = direct RPC): batches engine reads into `POST /v1/raw/slots`.
- `RpcTransport` (no indexer at all): `starknet_getStorageAt` against any public node; `last_update_block` degraded to 0 (documented: maturity via events fallback).
Public API mirrors engine entrypoints: `sync_incoming(addr, &SecretFelt, cursor) -> SyncIncomingStateResult`, `sync_outgoing`, `preflight`, `history`. Cursor persistence: serde JSON identical to reference `api/types.rs` cursor schema ⇒ cursors interop with compat mode (a wallet can migrate Tier 0 → Tier 1 without resync); stored encrypted-at-rest by the caller (contains SecretFelt-derived channel_key — documented sensitive).

**strk20-client-wasm**: wasm-bindgen (`?Send` friction handled via `SendWrapper`; discovery-core compiles to wasm32 untouched — verified upstream). Key in as `Uint8Array` (zeroization limits at the JS boundary stated honestly). JSON in/out = reference wire shapes.

**ts/packages/discovery-provider**: `LocalDiscoveryProvider implements DiscoveryProviderInterface` — `discoverNotes` / `discoverChannels` / `discoverRequirement` (+ `fetchHistory`), cursor mapping reusing the SDK's exported conversion semantics (`notesCursorToApiCursor`, `apiCursorToNotesCursor`, `buildSubchannelCursors`, `convertIncomingNotes`) so `NotesCursor`/`ChannelCursor` round-trip identically to `IndexerDiscoveryProvider`. Constructor: `new LocalDiscoveryProvider({feedUrl, mode: "bulk"|"targeted", storage?: IndexedDBAdapter})`. Drops into `createPrivateTransfers({discoveryProvider})` with zero SDK changes. Fixes all three `ContractDiscoveryProvider` gaps (importability, PoolContractInterface, maturity-blindness — `write_block` gives the 10-block rule natively).

## 7. CLI (`strk20d`)

- `strk20d run [--rpc-url URL[,URL2]] [--db path] [--feed-dir path] [--listen addr] [--epoch-size N] [--no-compat]` — ingest + API, one command; env-var equivalents (`STARKNET_RPC_URL`, …). U4.
- `strk20d backfill [--to-block N]` — backfill only, exit.
- `strk20d status` — human /v1/status.
- `strk20d snapshot create|import <file>` — import verifies hash chain/anchor.
- `strk20d epoch verify [--all|--epoch e]` — recompute raw bytes from DB, compare sha256 vs manifest (mirror divergence detector).
- `strk20d mirror pull <upstream-feed-url>` — U5: build/refresh from another instance's feed instead of RPC; serve identical feed.
- `strk20d verify-proof [--slot 0x…|--nullifier 0x…]` — U6 spot check via `getStorageProof` (incl. non-membership: nullifier slot = 0 ⇒ unspent at block).
- `strk20d sync --keyless --address 0x… --viewing-key-file k.hex [--mode bulk|targeted]` — the client-core demo path; key from file/stdin, never in argv.
- `strk20d bench <b1|b3|b4|b5|b8>` — research §Q19 benchmark table.

## 8. Acceptance E2E client test (the branch gate)

Location `crates/e2e-tests/tests/acceptance.rs`. Fully offline; exercises the **real compiled `strk20d` binary over real HTTP** with a real keyless client. 

**Topology**: `[test client (strk20-client-core)] → HTTP → [recording proxy] → HTTP → [strk20d run] → HTTP → [fixture RPC server]` — all in-process/child-process on 127.0.0.1, ephemeral ports.

**Seed data**: vendored upstream `devnet-state.json` (48 slot→value pairs, alice/bob addresses + viewing keys 0xa11ce/0xb0b, block 46). The fixture RPC server (small axum app in e2e-tests) synthesizes a deterministic chain: blocks 0–46 with computed hashes/parent links; the 48 slot writes distributed as `storage_diffs` across blocks 40–46 (all discovery slots are write-once, so any distribution ≤ 46 is semantically valid); serves standard `starknet_chainId`, `getBlockWithTxHashes` (incl. tags `latest`→46, `l1_accepted`→46), `getStateUpdate`, `getEvents` (empty pool event set is fine — sync engine is IViews-only), `getClassHashAt`, `getStorageProof` (canned anchor). Epoch size set to 16 via flag ⇒ epochs 0,1 get cut (blocks ≤ l1=46), tail 32–46 in live/delta.

**Ground truth (oracle)**: computed in-test by running `discovery_core::sync_incoming_state` / `sync_outgoing_state` directly over upstream `MockBackend` loaded from the same `devnet-state.json` (upstream's own test pattern) — for alice and bob. No hand-written expected values: the oracle is the unmodified engine over the unmodified fixture.

**Steps and assertions**:
1. Spawn fixture RPC; spawn `strk20d run --rpc-url http://127.0.0.1:P --epoch-size 16 --db tmp/idx.sqlite --feed-dir tmp/feed --listen 127.0.0.1:0`; poll `/v1/status` until `backfill.done && head==46 && l1_head==46 && epochs.head_epoch==1`.
2. **Keyless bulk leg (headline)**: client-core `MirrorTransport` pulls `/v1/feed/manifest` + epochs + `/v1/feed/delta` through the proxy, verifies the hash chain, builds a local mirror, runs the engine with alice's key locally. Assert `SyncIncomingStateResult` for alice **equals** oracle output exactly (canonical sort, full struct equality: channels, subchannels, notes with note_id/token/amount/note_index/write-block-derived fields; `cursor.is_complete() == true`). Same for alice outgoing and for bob.
3. **Keyless targeted leg**: `RawApiTransport` against `POST /v1/raw/slots` → identical equality assertions.
4. **Key-sensitivity control**: run bulk leg with a wrong key (0xdead) → zero channels/notes (proves results derive from the key client-side, not from server state).
5. **Compat leg (Tier 0 conformance)**: raw `reqwest` POST to `/v1/sync/incoming_state` with the reference wire body `{contract_address, viewing_key: "0xa11ce", recipient_address, cursor:{}}` → response notes equal oracle in reference JSON shape; cursor JSON round-trips into client-core's cursor type (interop assertion). `/health` returns reference shape OK.
6. **Mechanical no-key-on-the-wire assertion**: the recording proxy (tiny hyper forwarder in e2e-tests) captured every byte the client actually sent during legs 2–4. Assert for each request (URL + headers + body): none contains the viewing key in ANY encoding — lowercase/uppercase hex with/without `0x`, zero-padded and minimal, decimal string, raw 32-byte BE and LE substrings, base64(BE) — nor any oracle-derived channel_key felt in the same encodings. Additionally for the bulk leg: alice's **address** absent from every request (bulk mode's stronger claim). **Detector self-test**: the same scanner run over the compat-leg capture MUST find the key (proves the detector isn't vacuous). Belt-and-suspenders: scan strk20d's DB file, feed dir, and logs for the same patterns after the run — the key must not exist server-side at all in keyless legs.
7. **Reorg leg**: fixture RPC forks — replaces block 45–46 with 45'–47' (different hashes, one changed slot write, l1 stays 44). Assert: `/v1/feed/delta` with the stale `(46, old_hash)` cursor → 409 with `rewind_to`; live stream emitted `rollback`; client rewinds, resyncs, final result equals the new oracle (MockBackend re-seeded with post-fork slots). Compat: `last_known_block=old_46_hash` → 409 BLOCK_REORGED.
8. **Determinism/mirror leg**: run `strk20d backfill` into a second fresh DB + feed dir against the same fixture RPC; assert epoch files' raw sha256 are byte-identical; `strk20d epoch verify --all` passes on both.
9. **Upgrade leg (secondary)**: fixture serves `replaced_classes` with an unknown hash at block 45 → `/v1/status` degraded, compat returns `SERVICE_UNAVAILABLE` for `block_ref: latest`, feed still serves raw lines incl. `"rc"`.

Pass = all of the above in one `cargo test -p e2e-tests --test acceptance` run in CI.

## 9. Testing strategy (below the acceptance gate)

- **Unit**: store (as-of-block reads incl. zero-default; event per-position filter vs a copied MockEventBackend oracle; reorg walkback + storage_latest rebuild), feed (canonical serialization golden bytes; hash chain; round-trip serialize→parse), ingest (continuation-token paging, tx_index/event_index assignment, provider failover).
- **Conformance** (upstream fixtures as oracles): (a) engine-over-DbBackend ≡ engine-over-MockBackend on `devnet-state.json` — proves the trait bridge; (b) `cairo-reference-data.json` vectors exercised via the vendored loader (crypto identity Rust↔Cairo); (c) reference `devnet-dump.json.gz` HTTP suite pointed at our compat server (the 11 upstream API tests as black-box); (d) TS side: SDK wire-format fixtures replayed against `LocalDiscoveryProvider` and against our compat endpoint with the real `IndexerDiscoveryProvider`.
- **wasm**: `wasm-pack test --headless` smoke of client-wasm over a fetch-mocked feed; cursor JSON interop test TS↔Rust.
- **Bench** (`strk20d bench`): B1 backfill wall-clock, B3 RPC-reads-per-user-sync (=0), B4/B5 latency/bytes, B8 crossover — filled against the fixture RPC and optionally live lava.

## 10. What this lens sacrifices (honest)

1. **Privacy-story dilution**: compat mode first-class means our flagship binary happily receives raw viewing keys, and hosted operators may default users into it. Mitigations (X-Strk20-Mode header, docs, log_bodies=false, config off-switch) are labels, not guarantees. A privacy-first design would ship compat off-by-default or as a separate binary.
- 2. **Supply-chain surface**: max-reuse depends on `discovery-service` as a lib → inherits its axum/rustls/tower_ohttp (git dep on starkware-libs/sequencer) tree; upstream tags are mutable; builds are slower and CI colder. `compat-thin` feature is the escape hatch, but it's the fallback, not the default.
3. **Operational complexity accepted**: hosted layer (keys, quotas, multi-pool pathing), SSE+chunked live streams, and the TS/wasm packaging pipeline (wasm-pack + npm publishing) are real ops weight beyond U4's single binary. U4 still holds (`strk20d run` with defaults), but the codebase carries hosted machinery a pure self-host design wouldn't.
4. **Targeted raw mode retained** for API ergonomics despite the proven address leak on the incoming path — documented as equal-to-RPC, but its existence invites misuse by integrators who don't read docs.
5. **U6 is spot-check, not full verification**: end-to-end MPT/poseidon proof verifier stays roadmap; we ship `verify-proof` sampling + content-addressed mirrors. U7 is deliberately minimal (honest stats only).
6. **Postgres deferred**: hosted multi-instance would want it; v1 hosted scaling leans on the feed's CDN-cacheability + SQLite read path. If compat-mode QPS grows, the trait boundary is the re-entry point — no re-architecture, but the work is unpaid.
7. **wasm bulk mode holds the whole mirror client-side** (~6 MB compressed history today, fine; at 100× growth cold-start UX on mobile browsers degrades → snapshot endpoint and prefix buckets become load-bearing sooner than the privacy lens would like).

## Use-case coverage map

U1 keyless sync: client-core/wasm bulk mode (headline) — cold start via snapshot or epoch replay, resume via (number,hash)+cursor. U2 SDK drop-in: Tier 0 (INDEXER_URL, zero code) and Tier 1 (`LocalDiscoveryProvider`). U3 backend/bot: `/v1/feed/live` global stream (<1 MB/day) + precomputed nullifier watch client-side — low latency, no per-user polling. U4 self-host: `strk20d run`, single binary, SQLite. U5 mirror: `mirror pull` + deterministic epochs + `epoch verify`. U6 auditor: `verify-proof` spot-checks (incl. nullifier non-membership), anchors in every epoch footer; full verifier roadmap slot. U7 explorer: `/v1/stats` honest set. U8 migrator: compat mode is Tier 0, byte-exact via reference ApiServer, conformance-tested against upstream's own suites.
