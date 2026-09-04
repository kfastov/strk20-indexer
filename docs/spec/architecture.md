# strk20-indexer — Final Unified Architecture Spec (v1)

Status: FINAL DRAFT for implementation. Backbone = Proposal 2 (operational-simplicity-first), the judges' consensus winner (2 of 3 verdicts). Every judge-mandated graft is incorporated below; conflicts between judges are resolved in §0. Ground truth for all upstream facts: `/Users/konstantinfastov/Projects/strk20-indexer/docs/research-answers.md`.

---

## 0. Judge-conflict resolutions

Judges 2 and 3 selected P2 as winner; Judge 1 selected P3. P2 is the backbone by majority. Judge 1's grafts are incorporated where they do not contradict the majority. Explicit resolutions:

**R1 — Backbone: P2, not P3.** Judge 1 chose P3 for its integration tiers. Both P2-judges scored P3's default-on keyed compat and hosted machinery as disqualifying (privacy honeypot, supply-chain inheritance, largest component count). Resolution: P2's shape (static-file feed, two binaries, one sequential ingest loop), with P3's integration value recovered as follows: compat mode ships in v1 (off by default, §6.4), cursor JSON interop ships in v1 (§7.4), and the TS `LocalDiscoveryProvider` is adopted verbatim as the fully-specified top roadmap item behind pre-cut seams (§12.1). Wallet teams get Tier 0 today (self-hosted `--enable-compat`) and Tier 1 as the first post-branch deliverable.

**R2 — Compat reuse level: medium reuse (copied wire types + unmodified engine), not the mounted reference `ApiServer`.** Judge 1's P3 mounted the unmodified upstream service; Judges 2 and 3 both flagged the supply-chain cost (discovery-service pulls axum/rustls/tower_ohttp and a git dep on starkware-libs/sequencer) and endorsed P2's medium reuse. Resolution: copy the reference `api/types.rs` serde types (~350 lines, Apache-2.0, provenance-noted), write our own axum handlers over the unmodified `discovery-core` engine, and prove byte-exact wire equivalence by replaying upstream's own 11 HTTP tests (devnet-dump fixtures) against our mount (§10.2). Byte-exactness is guaranteed by test, not by linking.

**R3 — Tail protocol: `head.ndjson` wholesale regeneration, no SSE/409 wire protocol in v1.** Judge 2 explicitly demoted streaming to roadmap ("P2's 1/min head.ndjson polling is the v1 baseline"); Judge 3 praised deleting the protocol class. Judge 1's P3 preference for live streams loses 2-1. Resolution: v1 tail = `head.ndjson` refetched wholesale on ETag change; roadmap SSE is a single GLOBAL stream only, never per-user (§12.2). Judge 1/3's mandatory client-reorg-with-cursor e2e leg is preserved and adapted to this protocol: reorg is detected by tail replacement, and the client-side `DiscoveryCursor` rewind rule (§7.5) is asserted in the acceptance test (§10.3 leg g).

**R4 — Prefix-bucket endpoint: wire spec frozen in this document; implementation is roadmap.** Judge 2's graft reads "gated off by default with leakage header"; Judge 3's reads "documented ~50-line Q9 hook, not implemented in v1". Resolution: the endpoint's exact wire shape is specified in §6.3 (frozen so later implementation cannot drift), but no code ships in the branch. The flag/header machinery it will reuse (`--enable-raw`, `X-Strk20-Privacy`) DOES ship in v1 with the plain targeted-raw endpoints. This satisfies Judge 3 literally and Judge 2's substance (the leakage-labeling machinery exists and is tested).

**R5 — Secret-exposure split: two binaries (P2) satisfies Judge 1's "split OR feature-gate at minimum".** `strk20-sync` (keyless client) does not link `strk20-indexerd` code at all — enforced by crate dependency direction (nothing depends on the server crate). Compat code inside the server binary is compiled in but runtime-gated by `--enable-compat` (off by default, loud startup warning). We deliberately keep P2's no-feature-matrix stance: one compiled server artifact, behavior gated at runtime. The default posture — flagship binary never receives viewing keys unless explicitly switched — is exactly Judge 1's graft.

**R6 — Acceptance-fixture write placement: partition across blocks {10, 20, 30}, head 46, l1_accepted 40.** Judge 2's example partition {10,20,46} conflicts with Judge 3's finding that writes at block 46 = head collide with the 10-block maturity rule (the MockBackend oracle has no block semantics and cannot arbitrate). Resolution: multiple partition blocks (Judges 1+2's mandate: per-note `block_number` asserted per-partition) all ≥ 16 blocks clear of head (Judge 3's mandate). A post-phase block 47/48 extension exercises resume and spent-state separately (§10.3).

**R7 — Anchor placement: excluded from the content hash AND from the hashed bytes entirely.** All three judges mandate P1's anchor-exclusion rule. P1's mechanism ("hash computed with anchor:null") still leaves an anchor field inside the file whose value varies by cut time. Resolution (strictly stronger, simpler): the epoch file contains NO anchor field at all; the storage-proof anchor lives in `manifest.json` (summary) and a per-epoch sidecar file (full proof JSON), both outside content addressing (§4.3, §4.4). Mirrors whose endpoint could not serve a proof for the epoch's end block produce byte-identical epoch files with an absent/`null` anchor in their manifest — a visible metadata gap, never a hash fork. (The "~25–55k-block proof window" this clause originally cited is retracted: proofs answer for any block on retry — §5.3, `docs/research/live/proof-window.md`. The anchor-exclusion rule is unaffected, and the null case is now a capability gap rather than the normal outcome.)

**R8 — Completeness mechanism: `verify-root` full MPT recomputation is THE completeness check; random-slot spot sampling is demoted to a cheap liveness cross-check.** Judges 2 and 3 both established that K-random-mirrored-slot sampling cannot detect a silent write creating a previously-unknown slot (channel writes are write-once fresh slots), and must not be presented as a completeness check. Resolution: §5.6.

---

## 1. Goals & non-goals

Product: open, self-hostable note indexer for the STRK20 privacy pool on Starknet (mainnet pool `0x040337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a`, deployed at block 8,978,970, one class upgrade at 11,632,886). The canonical product is a directory of content-addressed, deterministically reproducible static files (the feed). Discovery logic is never reimplemented: the unmodified upstream `discovery-core` engine runs client-side over a verified local mirror of pool storage + events.

Use-case → mechanism map (every U gets a mechanism or an explicit roadmap slot):

| Use case | v1 mechanism |
|---|---|
| U1 wallet user, keyless sync | `strk20-sync` / `strk20-client`: download feed, verify, run unmodified engine locally; viewing key never leaves the process. Cold start ≈ 6 MB zstd; incremental resume via `DiscoveryCursor` + feed cursor (§7). |
| U2 wallet/app developer | v1: (a) Rust lib `strk20-client`; (b) self-hosted `--enable-compat` + stock SDK `IndexerDiscoveryProvider` (key goes to your own box, labeled). Roadmap TOP item: wasm + npm `LocalDiscoveryProvider` implementing the SDK's 3-method `DiscoveryProviderInterface`, fully specified in §12.1 — seams pre-cut in v1 (§7.6). |
| U3 payment backend/bot | Poll `GET /feed/head.ndjson` with ETag (~1 req/min, address-blind); client-side nullifier watch + trial decryption (§7.5). No per-user push ever (durable-fingerprint hazard). Roadmap: global SSE tail. |
| U4 self-hoster | One binary `strk20`, one command `strk20 run`, one SQLite file + one feed dir; DB is a rebuildable cache; deterministic backfill ≈ 19 MB raw / ~47 min on lava (§5). |
| U5 mirror operator | `strk20 mirror pull <url>` or plain file copy + `strk20 epoch verify`; byte-identical epoch content hashes across operators and RPC providers (test-asserted, incl. nightly live smoke); hash chain makes omission a visible fork (§4.3). |
| U6 auditor/paranoid client | `strk20-sync verify --rpc <own-node>`: real pedersen-MPT proof verification of the user's own note/nullifier slots via `starknet_getStorageProof` against the user's OWN RPC, including non-membership = unspent proof. Server-side `verify-root` at every epoch cut. Shared MPT module (§5.6, §7.7). |
| U7 explorer/analytics | `GET /v1/stats`: honest set only (per-token deposits/withdrawals/TVL, global note count = anonymity set, spend count, ExternalContractInvoked breakdown, registrations, upgrade history). No deposit↔withdraw joins, no per-tx timelines, no nullifier linkage — by policy (§6.2). |
| U8 compat migrator | `--enable-compat`: exact reference `/v1/sync/*` + `/v1/history` wire over our SQLite-backed engine bridge; loudly labeled key-visible; conformance-proven by upstream's 11 HTTP tests (§6.4). |

Non-goals for this branch: explorer UI, transaction CLI, auth/quota/multi-tenant hosting features, Postgres impl (trait seam only), PIR, OHTTP, per-user push of any kind, horizontal ingest scaling (mirroring static files IS the scale-out).

Constraint honored: no deadline reasoning shaped any decision; everything below is argued on merit.

---

## 2. System overview

```
                    ┌──────────────────────── strk20 (server binary; never sees keys by default)
Starknet RPC ──────►│ ingest loop ──► SQLite strk20.db (rebuildable cache)
(lava primary,      │      │
 publicnode fb)     │      ├──► epoch cutter ──► feed dir (CANONICAL PRODUCT)
                    │      │       genesis.json / manifest.json / epochs/*.strk20e.zst
                    │      │       epochs/*.anchor.json / head.ndjson / snapshots/
                    │      └──► verify-root (MPT recompute vs getStorageProof) at every cut
                    │
                    │ axum: GET feed files (ServeDir) │ /health │ /v1/stats │ /metrics
                    │ [--enable-raw]   POST /v1/raw/read_slots, GET /v1/raw/events   (labeled leaky)
                    │ [--enable-compat] reference wire /v1/sync/* + /v1/history      (labeled keyed)
                    └───────────────────────────────
                              ▲ GETs only (manifest, epochs, head) — request stream
                              │ provably independent of key and address
                    ┌─────────┴────────── strk20-sync (client binary; holds the key; links NO server code)
                    │ FeedTransport (http|dir) ─► FeedStore (verify hash chain ─► sync.db mirror)
                    │ ─► RawStorageAccess/RawEventAccess ─► blanket IViews ─► UNMODIFIED discovery-core
                    │ ─► notes/channels/spent state; DiscoveryCursor persisted (reference JSON schema)
                    │ verify subcommand: pedersen-MPT proofs from the USER'S own RPC
                    └──────────────────────
Mirrors: copy feed dir (or `strk20 mirror pull`) ─► serve identical bytes; divergence = hash mismatch.
```

Dataflow invariants:
1. DB content is keyed by block and independent of fetch order → epoch bytes are a pure function of chain data + format version → byte-identical across operators, runs, and RPC providers.
2. Epochs are cut only ≤ l1_accepted → immutable by construction; reorgs only ever touch the tail.
3. A feed-mode client emits only GETs for {manifest, missing epochs, head} — a function of download progress only. The keyless property is enforced by the type system (§7.2) and by mechanical wire capture in the acceptance test (§10.3).

---

## 3. Crate layout

Cargo workspace. Workspace-wide pins (in root `Cargo.toml` `[workspace.dependencies]`):

```toml
# Engine — record rev in a comment; tag is mutable upstream, Cargo.lock pins the rev.
discovery-core = { git = "https://github.com/starkware-libs/starknet-privacy.git", tag = "CONTRACT_V2_DEPLOYED_MAINNET_2026-07-08" } # rev 74841caf0466d122117945e28ed983e2864c8fc1
# Type identity with the engine (fork EmittedEvent, BlockId, StorageResult):
starknet-core   = { package = "starknet-rust-core",   git = "https://github.com/software-mansion/starknet-rust.git", rev = "7caedfe" } # 7caedfef85a4d748f8e9e5a159c87c31b6fe9d71
starknet-crypto = { package = "starknet-rust-crypto", git = "https://github.com/software-mansion/starknet-rust.git", rev = "7caedfe" } # pedersen for MPT module
starknet-types-core = { version = "=0.2.4", features = ["curve", "serde"] } # crates.io 1.0.0 is a DIFFERENT Felt — denied
```

Add a CI deny (`cargo deny` or a lockfile grep test) that fails the build if `starknet-types-core >= 1.0` enters the tree (two Felt types = silent trait-bound failures).

Members and dependency edges (nothing depends on `strk20-indexerd`; `strk20-client` links no server code):

**`crates/feed` — lib `strk20-feed`.** Epoch wire format: canonical encode/decode, sha256 content addressing, hash-chain + manifest verification, head.ndjson grammar. Pure `&[u8]`-in/out, no async, no IO, wasm-clean. Deps: serde 1.0.229, serde_json 1.0.151, sha2, hex, thiserror 2.0.20; `zstd 0.13.3` behind non-default feature `compress`.

**`crates/indexerd` — bin `strk20`.** Ingest pipeline, SQLite store, epoch cutter, verify-root, axum HTTP server, compat mode, CLI. Deps: strk20-feed(+compress), discovery-core, starknet-core/-crypto (fork), rusqlite 0.40.2 `["bundled"]` (spawn_blocking around all DB IO), tokio 1.53.1 `[rt-multi-thread,macros,signal,time,sync]`, axum 0.8.9, tower-http 0.7.0 `[fs,trace,set-header]`, reqwest 0.13.4 `[json]` (hand-rolled JSON-RPC structs — no starknet-providers), clap 4.6.6 `[derive]`, tracing 0.1.44 + tracing-subscriber 0.3.23, zstd 0.13.3, anyhow 1.0.104, thiserror, async-trait 0.1.92, futures 0.3.34.

**`crates/client` — lib `strk20-client` + bin `strk20-sync`.** FeedTransport trait + impls, FeedStore (verified mirror in sync.db), engine adapter, cursor persistence, MPT proof verifier (module shared by re-export from a `crates/mpt` submodule or a `mpt.rs` in `feed`; see §5.6 — it lives in `crates/feed` as feature `mpt` so both binaries share one implementation without the client linking indexerd). Deps: strk20-feed(+compress,+mpt), discovery-core, starknet-crypto (fork), rusqlite bundled, reqwest, tokio, clap, zeroize (via discovery-core's SecretFelt), tracing.

**`crates/e2e-tests`** — acceptance harness: fixture RPC server, recording reverse proxy, oracle runner, spawns the REAL binaries. Deps: strk20-feed, discovery-core (MockBackend oracle), tokio, axum, hyper, reqwest, tempfile 3.27.0, trybuild (for the compile-fail suite, §10.1).

**`vendor/fixtures/`** — vendored verbatim from upstream at rev 74841caf with `PROVENANCE.md` (tag, rev, per-file sha256; Apache-2.0 notice): `devnet-state.json` (48 slots, alice `0x34ba56f9…`/key `0xa11ce`, bob `0x2939f2dc…`/key `0xb0b`, pool `0x66292db2…`, block 46), `cairo-reference-data.json` (crypto vectors), `devnet-dump.json.gz` + metadata (reference 11-HTTP-test fixtures), plus a verbatim copy of upstream's `#[cfg(test)]`-private `src/test_fixtures.rs` loader (all referenced types are pub — copy compiles).

Roadmap members (NOT in branch, seams pre-cut): `crates/client-wasm`, `ts/packages/discovery-provider` (§12.1).

---

## 4. Data model

### 4.1 SQLite — server `strk20.db` (client `sync.db` reuses the `storage_log` + `events` subset + its own `meta`)

Felts stored as 32-byte big-endian BLOBs. `PRAGMA journal_mode=WAL; PRAGMA synchronous=NORMAL; PRAGMA foreign_keys=ON;`

```sql
CREATE TABLE meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
) WITHOUT ROWID;
-- rows: schema_version, chain_id, pool_address, genesis_block (8978970),
--       epoch_size (10000), head_number, head_hash, l1_accepted_number,
--       decode_state ('ok'|'degraded'), degraded_since_block (nullable)

CREATE TABLE blocks (               -- pool-active blocks + head/l1 checkpoints
  number      INTEGER PRIMARY KEY,
  hash        BLOB NOT NULL,
  parent_hash BLOB NOT NULL,
  timestamp   INTEGER NOT NULL,
  status      INTEGER NOT NULL      -- 0 = ACCEPTED_ON_L2, 1 = ACCEPTED_ON_L1
);
CREATE UNIQUE INDEX blocks_hash ON blocks(hash);

CREATE TABLE storage_log (          -- append-only raw pool diffs; layout-agnostic
  slot  BLOB NOT NULL,
  block INTEGER NOT NULL REFERENCES blocks(number) ON DELETE CASCADE,
  value BLOB NOT NULL,
  PRIMARY KEY (slot, block)
) WITHOUT ROWID;
CREATE INDEX storage_log_block ON storage_log(block);
-- read_slot(s, at) = value at MAX(block) <= at, else Felt::ZERO (Cairo map semantics).
-- write_block comes free — natively replaces the fork-only INCLUDE_LAST_UPDATE_BLOCK RPC extension.
-- NO storage_latest cache table (P3's cache-coherence hazard, rejected by Judge 3): the
-- (slot, block DESC) primary key makes the as-of read a single index seek.

CREATE TABLE events (
  block       INTEGER NOT NULL REFERENCES blocks(number) ON DELETE CASCADE,
  event_index INTEGER NOT NULL,     -- within-block order from getEvents (fork EmittedEvent requires it)
  tx_index    INTEGER NOT NULL,     -- position in getBlockWithTxHashes.transactions
  tx_hash     BLOB NOT NULL,
  key0        BLOB NOT NULL,        -- denormalized selector
  key1        BLOB,                 -- denormalized (nullifier watch, Q10)
  keys        BLOB NOT NULL,        -- concatenated 32-byte felts
  data        BLOB NOT NULL,        -- concatenated 32-byte felts
  PRIMARY KEY (block, event_index)
) WITHOUT ROWID;
CREATE INDEX ev_key0 ON events(key0, block);
CREATE INDEX ev_key1 ON events(key1) WHERE key1 IS NOT NULL;

CREATE TABLE class_history (
  block      INTEGER PRIMARY KEY,   -- block of replaced_classes / deployment
  class_hash BLOB NOT NULL,
  decoder    TEXT                   -- 'v1' | 'v2' | NULL = unknown → degraded from this block
);

CREATE TABLE epochs (
  idx           INTEGER PRIMARY KEY,
  from_block    INTEGER NOT NULL,
  to_block      INTEGER NOT NULL,
  content_hash  BLOB NOT NULL,      -- sha256 of UNCOMPRESSED canonical NDJSON payload
  zst_sha256    BLOB NOT NULL,
  file_size     INTEGER NOT NULL,
  prev_hash     BLOB,               -- content_hash of previous pool epoch (NULL for first)
  anchor_block        INTEGER,      -- all anchor fields NULLABLE and OUTSIDE content addressing
  anchor_block_hash   BLOB,
  anchor_storage_root BLOB,
  anchor_class_hash   BLOB,
  cut_at        INTEGER NOT NULL
);

CREATE TABLE ingest_cursor (
  id                  INTEGER PRIMARY KEY CHECK (id = 1),
  scan_frontier       INTEGER NOT NULL,   -- last fully-ingested block number
  events_continuation TEXT                -- vestigial: LIVE-8 forbids presenting a token, so this is always NULL
);
```

Client `sync.db` extras: `meta` rows `feed_cursor` (`{"epoch":N,"block":n,"hash":"0x…"}`), `discovery_cursor_<address>` (reference-schema JSON, §7.4 — contains SecretFelt-derived channel keys → DB file chmod 0600, documented key-adjacent), `nullifier_watch` derived table.

Rollback = `DELETE FROM blocks WHERE number > :ancestor` (cascades to storage_log/events) in one transaction; floor = last cut epoch's `to_block` (epochs are L1-final, never crossed).

### 4.2 Feed directory (the canonical product)

```
feed/
  genesis.json                      # immutable, written once
  manifest.json                     # atomic tmp+rename on every change
  epochs/00000897.strk20e.zst       # immutable, content-addressed
  epochs/00000897.anchor.json       # sidecar, NOT content-addressed, may be absent
  head.ndjson                       # unfinalized tail, regenerated wholesale
  snapshots/latest.sqlite.zst       # optional convenience; epochs are canon
```

`genesis.json` (frozen): `{"format":"strk20-feed","v":1,"chain_id":"SN_MAIN","pool":"0x040337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a","genesis_block":8978970,"epoch_size":10000}`

Epoch alignment is ABSOLUTE: epoch `e` covers block range `[e*10000, (e+1)*10000)`; the first pool epoch is 897 (⌊8978970/10000⌋). Epoch index = block/10000 by inspection; alignment is independent of deployment config. Tests use small `epoch_size` via a test chain config; mainnet value is frozen in `genesis.json` forever (format change ⇒ `v:2` namespace + dual-serve migration).

### 4.3 Epoch wire format v1 (byte-precise, frozen)

File `epochs/{idx:08}.strk20e.zst` = zstd level 19 over the canonical NDJSON payload. **Content identity = sha256 over the UNCOMPRESSED payload bytes** (zstd output is version-unstable; `zst_sha256` is a transport checksum only, never identity).

Canonical JSON rules (apply to every line): fixed field order exactly as written below; no whitespace; all felts as lowercase `0x`-prefixed minimal hex (no leading zeros; zero = `0x0`); every line terminated `\n` including the last.

```
line 1 (header):
{"t":"hdr","v":1,"kind":"strk20-epoch","chain_id":"SN_MAIN","pool":"0x…","epoch":897,"from":8970000,"to":8979999,"prev":"<64-hex sha256 of previous pool epoch's payload, or null for the first>"}

one line per POOL-ACTIVE block, ascending block number:
{"t":"blk","b":8978970,"h":"0x…","p":"0x…","ts":1720000000,"d":[["0x<slot>","0x<value>"],…],"e":[[<tx_index>,<event_index>,"0x<tx_hash>",["0x<key0>",…],["0x<data0>",…]],…],"rc":"0x<new_class_hash>"}
  - "d": storage diffs sorted ascending by slot (byte order of the 32-byte BE felt)
  - "e": events sorted ascending by event_index (emission order)
  - "rc": present ONLY on blocks where replaced_classes/deployment changed the pool class

last line (footer):
{"t":"end","blocks":<n_blk_lines>,"diffs":<total d entries>,"events":<total e entries>,"class":"0x<pool class_hash as of `to`>"}
```

**No anchor field exists anywhere in the file** (resolution R7). Everything in the payload is a deterministic function of chain data → byte-identical on every mirror → an omitted or altered block is a visible hash fork via `prev`-chain + `content_hash`.

Epochs are cut only when the entire range is ≤ l1_accepted ⇒ immutable by construction.

### 4.4 Manifest, anchor sidecar, head, snapshot

`manifest.json` (atomic tmp+rename):
```json
{"v":1,"chain_id":"SN_MAIN","pool":"0x…","genesis_block":8978970,"epoch_size":10000,
 "head":{"number":14056430,"hash":"0x…","l1_accepted":14049912,"class":"0x67dddd…","decode_state":"ok"},
 "latest_epoch":1405,
 "epochs":[{"e":897,"from":8970000,"to":8979999,"hash":"<64-hex>","zst":"<64-hex>","bytes":12345,
            "anchor":{"block":8980100,"block_hash":"0x…","storage_root":"0x…","class":"0x…"}}, …],
 "snapshot":{"block":14049912,"sha256":"<64-hex>","bytes":123456}}
```
`epochs[].anchor` is `null` when the proof window was missed — a visible metadata gap, never an identity fork. `snapshot` optional/nullable.

`epochs/{idx:08}.anchor.json` sidecar = the full stored `starknet_getStorageProof` response for the anchor block (auditor input), absent when unavailable.

`head.ndjson` (regenerated wholesale on every head change or reorg; ≤ ~100 KB today):
```
{"t":"hdr","v":1,"kind":"strk20-head","tail_from":8980000,"head":14056430,"head_hash":"0x…","l1_accepted":14049912}
<blk lines, same grammar as epochs, plus one extra final field "fin":"l2"|"l1">
{"t":"end",…}
```
Clients refetch it entirely (ETag = sha256 of its bytes); a reorged tail is simply a replaced file. No wire rollback protocol (R3).

`snapshots/latest.sqlite.zst`: content-addressed convenience export; `strk20 snapshot import` verifies it against the epoch chain before use.

---

## 5. Ingest pipeline

One sequential tokio task; deterministic; resumable from `ingest_cursor`. One code path: BACKFILL is FOLLOW with `target = l1_accepted` (then either exit for `strk20 backfill` or continue for `strk20 run`).

### 5.1 States

`INIT → BACKFILL → FOLLOW`, interrupts `REORG`, `UPGRADE`; side jobs `EPOCH_CUT`, `PROMOTE_L1`, `VERIFY_ROOT`.

**INIT**: open DB, create/verify schema, verify meta (chain_id via `starknet_chainId`, pool address, schema_version); `getClassHashAt(latest, pool)` cross-checked against `class_history` + decoder map.

### 5.2 Cycle (BACKFILL and FOLLOW identical)

1. **Finality poll**: `getBlockWithTxHashes("l1_accepted")` and `("latest")`; `PROMOTE_L1`: flip `blocks.status → 1` for numbers ≤ l1_accepted; update meta.
2. **Canonicity check** on stored head: refetch stored head number, compare hash. Mismatch → REORG (§5.4).
3. **Events-first scan** (rewritten for LIVE-8; see `docs/research/live/live-run-findings.md`): `getEvents` is asked over **subdivided block windows**, never with a `continuation_token`. A token is node-local state and the primary endpoint is an aggregator, so presenting one back reaches a different backend that resumes elsewhere and drops the events in between with no error (measured: two paginated scans of one range disagreed by 19 blocks; a full mainnet backfill lost 139 blocks and 489 events). The scan therefore halves a window whenever the answer carries a token and takes the union of single-page answers — one response carries no cross-request state, so it is sound under any routing. A window that still carries a token at single-block granularity is a hard error naming the block, never a truncation. Yields the pool-active block set (~0.23% of blocks) AND per-block event order = `event_index` assignment. `ImplementationReplaced` is an event, so upgrade blocks are always caught by this scan.
4. **Per active block** (bounded concurrency 8 for fetches; writes strictly sequential): `getStateUpdate({block_number})` → filter `state_diff.storage_diffs[pool]`, `replaced_classes`, `deployed_contracts`; `getBlockWithTxHashes({block_number})` → hash/parent/timestamp/status + `tx_hash → tx_index` from transaction position. The block's events come from the scan's own answer — re-asking per block was both a wasted call per active block and the same paging unsoundness in miniature; only the §5.6 rescan path fetches them, in one page. One SQLite transaction per block: insert block, storage_log rows, events rows, class_history if applicable; advance `scan_frontier`.
5. **EPOCH_CUT** (§5.5) whenever the next uncut epoch range is fully ≤ l1_accepted.
6. **Tail regen**: on any head change, regenerate `head.ndjson` wholesale, atomic tmp+rename.
7. FOLLOW poll cadence: latest every 2 s; l1_accepted every 60 s.

### 5.3 RPC source hierarchy & retry/UA rules

Primary `https://rpc.starknet.lava.build` (spec 0.8.1, archive-deep; **403s the default reqwest/python UA — always send a real `User-Agent: strk20-indexer/<version>`**). Fallback `https://starknet.publicnode.com` (0.10.2, throttles — treat 429 with exponential backoff, cap 60 s). Failover: N consecutive failures (default 5) → switch provider; both down → hold with backoff, `/health` 503. All five methods (`getEvents`, `getStateUpdate`, `getBlockWithTxHashes`, `getStorageProof`, `getClassHashAt`, plus `chainId` at INIT) are standard JSON-RPC via plain reqwest + our own serde structs — no starknet-providers dependency. `getStorageProof` on lava answers for **any** historical block, back to genesis, but only on the subset of pooled backends running archive tries — so error 42 names the backend that answered, not the block, and the response to one is a **bounded retry against the same endpoint** (`PROOF_RETRIES`), never a failover (LIVE-6: publicnode implements no proofs at any height). The earlier "~25–55k block window" recorded here, and the "~1024-block window" of consumer-path §11, were both bisections over a nondeterministic predicate and are retracted (`docs/research/live/proof-window.md`). Because the pool is anonymous and load-balanced, every accepted proof is **bound to the chain** before its `storage_root` is believed: `global_roots.block_hash` must equal `getBlockWithTxHashes(block).block_hash`, and a disagreement is a hard error, never a retry and never a capability gap. Alternative backfill source: `strk20 mirror pull <feed-url>` imports another instance's verified feed (full chain + hash verification) then switches to RPC at feed head (U5).

Determinism guarantee: because the DB is keyed by block and epoch serialization reads sorted DB rows, epoch bytes are independent of provider, fetch order, and restart points — asserted offline (§10.3 leg j) and against reality nightly (§10.4).

### 5.4 Reorg handling

On head-hash mismatch: walk back through stored active blocks (refetch hash per stored number, newest first) to the fork ancestor; floor = last cut epoch's `to_block` (never crossed — Grinta-scale ~4000+-block en-masse ACCEPTED_ON_L2 revocation is just a deeper walk, still above the floor since epochs are L1-final). Then one transaction: `DELETE FROM blocks WHERE number > ancestor` (cascades), rewind `scan_frontier`; regenerate `head.ndjson`. Compat mode reproduces reference behavior: a request whose `last_known_block` (number,hash) is non-canonical → HTTP 409 `BLOCK_REORGED`.

### 5.5 Epoch cutting

When l1_accepted ≥ epoch `e`'s `to`: serialize canonical NDJSON from sorted DB queries (pure function of rows) → sha256 (content hash) → zstd-19 → write `epochs/{e:08}.strk20e.zst` tmp+rename → run VERIFY_ROOT (§5.6; on mismatch: DO NOT publish, alarm, rescan) → best-effort fetch `getStorageProof(block_id≈to, pool, [])` for the anchor; store sidecar + manifest anchor entry (or `null`) → append manifest entry with `prev` chain → rewrite `manifest.json` atomically.

### 5.6 Completeness: VERIFY_ROOT (mandatory, load-bearing)

The events-first scan assumes every pool storage write lands in a block with ≥1 pool event (true for both deployed classes: channel/subchannel writes are event-silent but ride note-creating transactions). This assumption is RECONCILED, not trusted: at every epoch cut, recompute the pool's contract-storage MPT root from the full mirrored slot set (pedersen binary trie, ~200 lines over the fork `starknet-crypto`; cheap at current N) and compare against `getStorageProof`'s `contract_leaves_data.storage_root` for the anchor block. Mismatch ⇒ a silent write to a slot the mirror never learned of ⇒ alarm, halt EPOCH_CUT (never publish a divergent epoch), slow-path recovery: full-range per-block `getStateUpdate` rescan of the affected epoch (~4 KB median per block), ingest the missing writes, log loudly, re-cut.

Random-slot spot sampling (16 recent + 16 random mirrored slots vs `getStorageAt`) runs every 30 min in FOLLOW as a cheap RPC-drift liveness check only — it is BLIND to unknown-slot omissions and is never presented as a completeness check (R8).

The MPT module lives in `crates/feed` behind feature `mpt` and is shared verbatim with the client-side U6 verifier (§7.7) — one implementation, two consumers.

### 5.7 Upgrade handling

Decoder map is explicit config (defaults shipped): `{ "0x30b8c540cf04d8ef0f4db2a9098d9cc0e35e83af1cb3325f5a4f40144b4b30b": "v1", "0x67dddd89d80fedadc06b6f160798f94800a4a70164e5a24301cd0d6076b554d": "v2" }` (v1 hash = the pre-upgrade class from `class_history`, verified on-chain — see docs/research/q1-version-pin.md in git history, removed 2026-09-02; the 7 discovery events are byte-identical across both, so one event decoder covers all history — the map exists for the NEXT upgrade). On `replaced_classes` at block b: insert `class_history`; known hash → continue. Unknown hash → **degraded mode from b**: raw ingest, feed cutting, raw endpoints all CONTINUE untouched (storage_log/events are layout-agnostic — mirrors never fork); `meta.decode_state = 'degraded'`; compat answers `SERVICE_UNAVAILABLE` for any `block_ref ≥ b`; `/v1/stats` freezes typed event decoding at b; `/health` reports degraded. Recovery: human adds the class to the decoder map in config → restart → `decode_state = 'ok'`. `upgrade_delay = 0` on the pool means surprise upgrades are the expected path — this is how the one real mainnet upgrade at 11,632,886 played out. Degraded mode has a dedicated acceptance leg (§10.3 leg i).

---

## 6. Public API

Global: JSON errors `{"error":{"code":"<SCREAMING_SNAKE>","message":"…","details":{…}?}}`. HTTP 409 is reserved EXCLUSIVELY for `BLOCK_REORGED` (SDK contract). Versioning: feed format versioned in-band (`"v":1` + `genesis.json`); ops/raw routes under `/v1/`; breaking feed change ⇒ `/feed2/` dual-serve.

### 6.1 Feed (always on, keyless, static files via tower-http ServeDir, strong ETags, CDN-safe)

| Method/Path | Response | Caching |
|---|---|---|
| `GET /feed/genesis.json` | genesis doc (§4.2) | `public, max-age=31536000, immutable` |
| `GET /feed/manifest.json` | manifest (§4.4) — the poll target | `public, max-age=30` |
| `GET /feed/epochs/{idx:08}.strk20e.zst` | epoch file, `Content-Type: application/zstd`, `X-Content-Sha256-Raw: <content_hash>` | immutable, cache-forever |
| `GET /feed/epochs/{idx:08}.anchor.json` | full stored getStorageProof response; 404 if absent | immutable |
| `GET /feed/head.ndjson` | tail (§4.4); ETag = sha256 of bytes — U3's poll target | `no-cache` + ETag/304 |
| `GET /feed/snapshots/latest.sqlite.zst` | optional snapshot | immutable (content-addressed name via redirect) |

No feed route takes any parameter derived from a user. This is the privacy mechanism.

### 6.2 Ops (always on, keyless)

`GET /health` → `{"status":"OK"|"DEGRADED"|"UNHEALTHY","head":{"number","hash","timestamp"},"l1_accepted","lag_secs","latest_epoch","class_hash","decode_state"}`; HTTP 503 when UNHEALTHY.

`GET /v1/stats` → honest set only (Q18 policy): `{"deposits":{"<token>":{"count","amount"}},"withdrawals":{…},"tvl":{…},"note_count","spend_count","open_note_deposits","external_calls":{"<target>":n},"registrations","upgrades":[{"block":11632886,"class":"0x67dddd…"}],"health":{…}}`. Excluded by policy: deposit↔withdrawal joins, per-tx timelines, per-token enc-note splits, nullifier linkage.

`GET /metrics` → Prometheus text (ingest lag, RPC failures, epoch count, verify-root status, decode_state).

### 6.3 Raw (targeted, keyless-but-leaky; OFF by default, `--enable-raw`)

Every response carries `X-Strk20-Privacy: targeted-mode-leaks-queried-slots`. Docs state plainly: leakage = direct RPC; the incoming-path slots are keyed by public address and reveal who you are syncing for. Exists for U6 tooling and remote compat backends.

`POST /v1/raw/read_slots` — body `{"block":"head"|<number>,"slots":["0x…", …]}` (≤ 1000) → `{"block":<n>,"block_hash":"0x…","values":[{"slot","value","write_block":<n>|null}]}`; absent slot → `"0x0"`, `write_block: null`.

`GET /v1/raw/events?from=<n>&to=<n>&key0=0x…&key1=0x…&limit=1000&cursor=<blk>-<idx>` → `{"events":[{"block","tx_index","event_index","tx_hash","keys":[…],"data":[…]}],"cursor":"<blk>-<idx>"|null}` — per-position filter semantics identical to upstream `RawEventAccess`/MockEventBackend.

**Prefix-bucket (spec frozen, implementation ROADMAP — resolution R4):** `GET /v1/raw/slots/prefix?bits=<k>&prefix=<hex>&block=head` → NDJSON `{"slot","value","write_block"}` for all slots whose top k bits match; Pedersen-uniform buckets = near-free PIR halfway; `bits=0` degenerates to the full dump (perfect privacy). Same flag + header regime as above.

### 6.4 Compat (U8; OFF by default, `--enable-compat`; loud startup warning; key-visible, labeled)

Medium reuse (R2): copied reference `api/types.rs` wire types + our handlers over the UNMODIFIED engine via the SQLite trait bridge (`DbBackend: RawStorageAccess + RawEventAccess + StorageSnapshot + StorageBackend` + service ChainState; blanket `impl<T: RawStorageAccess> IViews for T` does the rest). Exact reference wire, proven by replaying upstream's 11 HTTP tests (§10.2). Reference limits reproduced (max_channels 256 etc.); viewing-key-vs-registered-pubkey validation reproduced.

Routes: `GET /health` (reference shape `{status, chain_head:{block_number,block_hash,timestamp}, lag_secs}`); `POST /v1/sync/incoming_state` `{contract_address, viewing_key, recipient_address, last_known_block?, block_ref?, cursor}` → `{block_ref, channels, subchannels, notes, cursor}`; `POST /v1/sync/outgoing_state` (+`sender_address`, `recipients?`); `POST /v1/sync/preflight_check` → `{block_ref, sender_registered, channel_exists, subchannel_exists}`; `POST /v1/history` → `{block_ref, transactions, cursor}`. Non-canonical `last_known_block` → 409 `BLOCK_REORGED`. Degraded mode → `SERVICE_UNAVAILABLE` past the degradation boundary.

Mandatory hardening (Judges 1/2/3 unanimous): every compat response carries `X-Strk20-Mode: compat-keyed`; request/response bodies are NEVER logged (hard-coded, no config to enable — bodies carry raw viewing keys); cursors are NEVER logged or persisted server-side (`DiscoveryCursor` embeds SecretFelt-derived `channel_key` material — key-adjacent); any pubkey cache is memory-only. Note `block_number` served natively from `storage_log.write_block`.

No proof proxying anywhere: U6 clients hit their OWN RPC's `starknet_getStorageProof` — the indexer stays out of the trust path.

---

## 7. Keyless client (`strk20-client` lib + `strk20-sync` bin)

### 7.1 Layering

`FeedTransport → FeedStore (verify + mirror) → engine adapter → unmodified discovery-core → results + spent-state + cursor persistence`.

### 7.2 FeedTransport — the type-system privacy boundary

```rust
pub trait FeedTransport {
    async fn fetch_genesis(&self) -> Result<Genesis>;
    async fn fetch_manifest(&self) -> Result<Manifest>;
    async fn fetch_epoch(&self, idx: u64) -> Result<Vec<u8>>;   // compressed bytes
    async fn fetch_anchor(&self, idx: u64) -> Result<Option<Vec<u8>>>;
    async fn fetch_head(&self, etag: Option<&str>) -> Result<Option<(Vec<u8>, String)>>; // None = 304
}
```
NO method accepts an address, key, slot, or any user-derived value — the keyless property is unrepresentable, locked by a trybuild `compile_fail` suite (§10.1) together with an assertion that `SecretFelt: !Serialize` (upstream guarantees: zeroize-on-drop, Debug=`[REDACTED]`, no Serde). Impls: `HttpTransport` (reqwest; headline), `DirTransport` (local mirror dir / air-gap / tests). Separate, clearly-named leaky transports exist outside this trait for targeted mode: `RawApiTransport` (POST `/v1/raw/read_slots`) and `RpcTransport` (plain `starknet_getStorageAt`, `write_block` degraded to 0 — documented).

### 7.3 FeedStore and engine wiring

Sync flow: fetch manifest → verify the FULL epoch hash chain (each payload's sha256 == manifest hash; each `prev` links; block parent-linkage within and across epochs) → decompress + apply new epoch block lines into `sync.db` (`storage_log` + `events`) → fetch `head.ndjson` (ETag) → delete previously-applied tail rows, reapply new tail idempotently. Any hash mismatch is a hard error naming epoch + expected/actual hash (U5 divergence detection is a client-side property; tamper-tested §10.3 leg e).

Engine adapter: `impl RawStorageAccess for FeedStore` (`read_slot` / `read_slots` / `read_slots_with_block` — three map-lookup/SQL methods; `write_block` native from `storage_log`, so the 10-block maturity rule works) → upstream blanket `impl<T: RawStorageAccess> IViews for T` drives the UNMODIFIED `sync_incoming_state` / `sync_outgoing_state` / `preflight_check` with reference-default `CursorLimits`/`IoBudget`; `impl RawEventAccess for FeedStore` (per-position filter semantics == MockEventBackend, differential-tested) drives `history::fetch_transactions`.

Spent-state machine: `unknown → unspent → spent`, driven by precomputed nullifiers matched against `NoteUsed` events (key1 index) and nullifier-slot diffs arriving in tail/epoch lines.

### 7.4 Cursor persistence (interop-mandated)

`DiscoveryCursor` is serialized in the EXACT reference `api/types.rs` JSON cursor schema, stored in `sync.db` meta per address. Consequence: cursors round-trip between compat mode and the keyless client — a wallet migrating Tier 0 → keyless does not resync its users (Judges 1/2/3 all grafted this). Documented sensitive: contains SecretFelt-derived `channel_key` felts; `sync.db` chmod 0600; caller advised to encrypt at rest; NEVER transmitted anywhere (feed mode has no server-side cursor at all).

### 7.5 Reorg + resume semantics (explicit rule — Judge 3's gap-fix)

Feed cursor = `(epoch, block_number, block_hash)`. On head refetch, if the new tail's history contradicts applied tail rows (head_hash changed AND the old tail blocks are absent/different — detected by comparing stored tail block hashes against the new file): (1) delete ALL applied tail rows from the mirror; (2) rewind the persisted `DiscoveryCursor` to the last L1-final checkpoint = end of the newest cut epoch (cursor snapshots are kept per epoch boundary: after each sync, store the cursor alongside the feed position it was computed at); (3) reapply the new tail; (4) re-run the engine from the rewound cursor. Never resync from scratch. This rule is asserted end-to-end in §10.3 leg g. Epoch files are ≤ l1_accepted and never touched by reorgs.

U3 watch mode (`strk20-sync --watch`): poll `head.ndjson` ETag (default 30 s); on change, apply tail, run incremental engine pass + nullifier match; emit JSON lines on stdout for new notes / spends. Server sees an address-blind conditional GET stream identical for every watcher.

### 7.6 wasm posture (seams pre-cut, nothing shipped)

`strk20-feed` core is `&[u8]`-pure (zstd and mpt behind features) — wasm-clean today. `FeedStore` is trait-backed over a storage abstraction with the SQLite impl feature-gated, so an in-memory + IndexedDB impl slots in without touching the engine adapter. `discovery-core` is verified wasm32-clean upstream; `?Send` friction handled via SendWrapper in the roadmap crate. These seams are the contract §12.1 builds on.

### 7.7 U6 verifier

`strk20-sync verify --rpc <url> [--address … | --slot … | --nullifier …]`: for each discovered note/nullifier slot (or explicit slot), fetch `starknet_getStorageProof` from the USER'S OWN RPC, verify the pedersen-MPT walk with the shared `feed::mpt` module against the proof's `storage_root`, compare leaf values against the mirror; nullifier-slot NON-membership proves un-spent-ness. Cross-check `storage_root`/`class_hash` against epoch anchors when present. Server plays no role.

---

## 8. CLI

`strk20` (server binary):
```
strk20 run          --db <path> --feed-dir <path> --rpc-url <url> [--rpc-fallback <url>]
                    --listen <addr:port> [--enable-raw] [--enable-compat]
                    [--epoch-size <n>: test configs only] [--config <toml>]
strk20 backfill     (same flags; ingest to l1_accepted, cut epochs, exit)
strk20 status       (prints /health + ingest cursor + epoch inventory)
strk20 epoch verify [--all | --epoch <e>] [--feed-dir]        # hash chain + content hashes
strk20 verify-root  [--epoch <e> | --latest] --rpc-url <url>  # MPT recompute vs getStorageProof
strk20 snapshot     create | import <file>
strk20 mirror pull  <feed-url> --feed-dir <path>              # verified feed import (U5)
strk20 bench        [b1|b3|b5|b8]                             # Q19 harness
```
All settings also via env (`STRK20_*`); mainnet chain config (pool, genesis, decoder map) built-in as defaults. `strk20 run` with zero flags on a mainnet box must work (U4).

`strk20-sync` (client binary; links no server code):
```
strk20-sync sync    --feed <URL|DIR> --address <0x…> --key-file <path|-- for stdin>
                    [--db <sync.db>] [--json] [--watch] [--full-resync]
strk20-sync verify  --rpc <url> [--address|--slot|--nullifier] [--feed]
```
Key input is file/stdin ONLY (never argv — process lists leak); read → zeroized buffer → `SecretFelt`. Exit code 0 only when `cursor.is_complete()`. `--json` emits the full result struct (used by the acceptance test).

---

## 9. Trust & privacy model summary

Per-mode leakage (full table: `docs/research-answers.md`, Q7/Q9):

| Mode | Server learns | Posture |
|---|---|---|
| Feed (default) | That SOMEONE syncs: GETs for public files, identical across users (test-asserted multiset equality). No key, no address, no slots, no per-client progress params. | Honest privacy mode. |
| Raw (`--enable-raw`) | Queried slots ⇒ incoming path reveals the public address. = direct-RPC leakage. | Labeled: `X-Strk20-Privacy` header + docs. |
| Compat (`--enable-compat`) | The raw viewing key, per request. | Labeled: `X-Strk20-Mode: compat-keyed`, startup warning, hard no-body-logging, self-host framing. |

Enforcement layers: (1) type system — `FeedTransport` cannot express a user-derived query; `SecretFelt: !Serialize` (trybuild-locked); (2) build discipline — client binary links no server code; (3) mechanical test — acceptance wire capture scans every request byte for every key/address/channel-key encoding, with a detector self-test and a post-run server-side scan (§10.3 legs d, f).

Trust story (honest, not maximal): feed completeness is unprovable to consumers; we ship delegated-trust-with-audits — content-addressed hash-chained epochs (omission = visible fork), cross-mirror byte-identity, server-side verify-root at every cut, client-side pedersen-MPT proof verification of the user's own slots against the user's own RPC (incl. unspent non-membership). Trustless fallback = self-host (`strk20 run` + `strk20-sync --feed dir:...`). The server is never in the proof path.

---

## 10. Testing strategy

### 10.1 Unit

- `feed`: canonical-encoding golden BYTE vectors (pinned files: hdr/blk/end lines, felt minimal-hex rules, sort orders), round-trip encode/decode, hash-chain + manifest verification, tamper detection; **golden vector asserting the epoch file contains no anchor field**.
- `feed::mpt`: pedersen-trie root recomputation against fixture proofs from a recorded `getStorageProof` response.
- `indexerd` store: as-of-block reads + zero-default + write_block — property/differential-tested against upstream `MockBackend` (pub, non-test) loaded with the same rows; reorg rewind never crosses the epoch floor; getEvents paging + `event_index`/`tx_index` assignment; provider failover.
- `client`: event per-position filter semantics differential vs copied MockEventBackend oracle; cursor JSON round-trip against reference schema fixtures; feed-cursor/reorg rewind state machine.
- **trybuild compile-fail suite** (in e2e-tests): (i) `SecretFelt` does not implement `Serialize` (a fn requiring `S: Serialize` fails to accept it); (ii) no `FeedTransport` method can be called with an address/key/slot argument (signature lock via a doctest-style negative).

### 10.2 Conformance vs upstream fixtures

- Trait-bridge proof: engine-over-`DbBackend`(SQLite loaded from `devnet-state.json`) ≡ engine-over-`MockBackend`(same JSON) — full struct equality for incoming/outgoing/preflight, both keys.
- `cairo-reference-data.json` crypto vectors evaluated through the client path (FeedStore-backed engine) — all 26 expected outputs match.
- Compat wire: upstream's 11 HTTP tests (devnet-dump.json.gz fixtures) replayed against our `--enable-compat` mount — byte-level wire match, 409 exactly on the reorg case (this is the proof for resolution R2).
- SDK wire-format fixtures replayed against the compat endpoint (recorded from `indexer-discovery.test.ts` shapes).

### 10.3 THE acceptance e2e (`crates/e2e-tests/tests/acceptance.rs` — CI gate, fully offline, real binaries, real HTTP)

Topology: `[strk20-sync / strk20-client] → HTTP → [recording reverse proxy (byte-capture of every request line, headers, body)] → HTTP → [real spawned strk20 binary] → HTTP → [fixture RPC server (in-process axum JSON-RPC)]`. The fixture RPC also captures every request it receives.

**Seed**: vendored `devnet-state.json` (48 slots; alice addr `0x34ba56f9…`, key `0xa11ce`; bob addr `0x2939f2dc…`, key `0xb0b`). The fixture RPC synthesizes a deterministic chain, blocks 1–46: computed hashes with correct parent links; the 48 write-once slot writes partitioned deterministically (committed partition = ground truth for `write_block`) across blocks **{10, 20, 30}** (resolution R6 — multiple partitions, all ≥ 16 blocks clear of head 46, safely outside the 10-block maturity window); synthesized `ViewingKeySet`/`EncNoteCreated` events on each active block so the production events-first path runs unmodified; `l1_accepted = 40`, `latest = 46`; serves `chainId`/`getEvents` (chunk_size forced to 2 to exercise paging + continuation tokens)/`getStateUpdate`/`getBlockWithTxHashes` (incl. `"l1_accepted"`/`"latest"` tags)/`getClassHashAt`/`getStorageProof` (consistent MPT roots computed with the shared mpt module). Test chain config: `epoch_size = 16` ⇒ epoch windows [0,16) and [16,32) are both fully ≤ 40 = l1_accepted → epochs 0–1 cut; blocks 32–46 in `head.ndjson`.

**Dual independent oracle** (Judge graft, P2): expected values computed BOTH ways — (O1) unmodified `discovery-core` engine over upstream's own `MockBackend` loaded with the same 48 slots, run in-test; (O2) a small hand-pinned golden JSON (checked into the repo, produced once by human inspection of O1 and frozen). A bug shared by our path and the O1 harness cannot self-confirm.

**Procedure and assertions**:

a. **Pipeline liveness** — *folded into b's preamble, not a separate leg*: spawn `strk20 run --rpc-url http://fixture --feed-dir tmp --listen 127.0.0.1:0`; poll `/health` until OK and `latest_epoch = 1` (proves RPC→SQLite→cut→serve end-to-end). The exact-epoch wait is load-bearing and stays — b compares against the published feed, so both epochs must be cut before it runs — but it asserts nothing b does not already require, so it no longer prints a verdict of its own. The lettering below is unchanged; other sections cite it.
b. **Keyless discovery — full equality**: run real `strk20-sync sync --feed http://proxy/feed --json` for alice (incoming + outgoing), bob, and an unused address. Assert alice/bob output == O1 field-for-field after canonical sort (every note's token/amount/note_index/note_id/nullifier; channels; subchannels; `cursor.is_complete() == true`) AND == O2 golden pins. Unused address → empty + complete. **Per-note `block_number` == its committed partition block** ({10,20,30}) — the diff-derived `write_block` capability plain RPC cannot serve.
c. **Key-sensitivity control**: sync with wrong key `0xdead` → zero channels, zero notes (results provably depend on the key).
d. **Mechanical no-key assertion** (the headline negative): over the FULL proxy capture of legs b/c: (i) every request is a GET with empty body; URL multiset ⊆ {`/feed/genesis.json`, `/feed/manifest.json`, `/feed/epochs/…`, `/feed/head.ndjson`} with no query strings; (ii) concatenated captured bytes (URLs+headers+bodies, both proxy AND fixture-RPC captures) contain NO encoding of either viewing key or address: minimal hex, 64-char padded hex, hex without `0x`, uppercase hex, decimal ASCII, raw 32-byte BE, raw 32-byte LE, base64 of the BE bytes — and no oracle-derived `channel_key` felt (computed in-test from O1) in any of those encodings (catches key-EQUIVALENT leaks); (iii) **address-blindness**: alice's and bob's request-URL multisets are IDENTICAL; (iv) **detector self-test** (moved here from f(ii)/h): the SAME scanner, pointed at the compat request body leg h will POST — a body that does carry bob's key — MUST find it. The negative in (ii) is worthless if the scanner is blind, so the self-test sits beside the claim it qualifies instead of four legs later behind a server restart.
e. **Tamper / U5**: flip one byte in the served epoch-0 file → client fails with the named hash-mismatch error identifying epoch + expected/actual hash; restore; `strk20 epoch verify --all` passes.
f. **O(delta) resume + detector self-test + server scan**: (i) fresh client synced only through a truncated phase (fixture at block 30, l1=30, epoch 0 only), cursors persisted; fixture extends to 46; resync → output == full ground truth AND proxy capture proves epoch 0 was NOT refetched (request multiset delta = {manifest, epoch 1, head} only). (ii) **Detector self-test**: *moved into leg d as d(iv)* — it needs a compat request body, not a compat server, and belongs next to the negative it makes non-vacuous. (iii) **Server-side scan**: after keyless legs, scan the strk20 binary's DB file, entire feed dir, and captured stdout/stderr logs with the same scanner — zero key/address/channel-key matches.
g. **Reorg with cursor** (mandatory, Judges 1+3): fixture forks blocks 44–46 → 44'–47' (one alice note moved, one added). Indexer detects, walks back to 43, regenerates head. Client's next `--watch`/sync pass detects tail replacement, rewinds mirror tail + `DiscoveryCursor` to the last L1 checkpoint per §7.5, resyncs → output == re-seeded post-fork O1 ground truth; epoch files byte-untouched (hashes unchanged); persisted `DiscoveryCursor` after the run is complete and consistent (a further no-op resync returns identical results with no engine progress).
h. **Compat smoke**: restart `strk20` with `--enable-compat`; one raw `POST /v1/sync/incoming_state` with bob's real key (reference body) → success, response carries `X-Strk20-Mode: compat-keyed`, note set non-empty. That is all this leg claims, because a live server is the only thing that can claim it. The wire contract has cheaper carriers and is asserted there instead: notes == the upstream engine is `conformance.rs::engine_over_sqlite_equals_engine_over_mock`, the cursor's serde identity is `conformance.rs::cursor_reference_schema_round_trip`, and a non-canonical `last_known_block` → HTTP 409 `BLOCK_REORGED` (plus the unknown-hash case, which must NOT 409) is `compat::tests::reorged_last_known_block_409s`. The key POSTed here is the real one on purpose: f(iii) scans the server's DB, feed and logs for it, and a fake key would make that scan vacuous. The process stays up for leg i's 503.
i. **Upgrade/degraded leg** (Judge 3 graft — the failure mode that actually happened on mainnet): fixture emits `replaced_classes` with an UNKNOWN class hash at block 45' → `/health` reports degraded; feed continues (new tail serves the `rc` line; raw ingest uncut); compat answers `SERVICE_UNAVAILABLE` for latest; restart with the class added to the decoder map → `decode_state: ok`, full function resumes.
j. **Mirror determinism**: second `strk20 backfill` into fresh DB + feed dir against the same fixture → epoch files byte-identical (sha256 equality), manifests equal modulo anchor timing; `strk20 epoch verify --all` cross-passes.
k. **Spent-state**: fixture appends block 48 writing `nullifiers[n] = 1` + `NoteUsed{n}` for one O1 ground-truth nullifier → exactly that note flips to spent in the next sync; all others unchanged.

Entire suite: no network, one `cargo test -p e2e-tests` run, target < 2 min excluding binary build.

### 10.4 Nightly live smoke (`#[ignore]`, not CI-gating)

Backfill the first ~200 active mainnet pool blocks from lava (real UA), assert a PINNED epoch content-hash prefix (determinism against reality, not just the mock); anchor fetch + `getClassHashAt` cross-check; verify-root over the partial range's known slots skipped (needs full set) — root check replaced by value spot-check here, labeled as such.

### 10.5 Bench harness

`strk20 bench`: B1 cold-start bytes/time (feed download + apply + engine), B3 targeted-leak count in feed mode (must equal 0 — measured by proxy, doubles as a regression net), B5 incremental resume cost, B8 full-backfill wall time vs RPC (per Q19).

---

## 11. Implementation order (dependency-ordered; no time estimates)

1. **Workspace + pins**: root Cargo.toml with all §3 pins; CI deny for starknet-types-core ≥ 1.0; vendor `fixtures/` + PROVENANCE.md; smoke-compile a bin that names discovery-core + fork types together (proves type identity).
2. **`strk20-feed`**: canonical codec + golden byte vectors; hash chain + manifest; head grammar; tamper tests. (Everything downstream consumes this.)
3. **`feed::mpt`** (feature): pedersen trie root + proof-walk verify; fixture tests from a recorded getStorageProof.
4. **`indexerd` store**: DDL, as-of reads (differential vs MockBackend), reorg rewind + floor, meta/cursors.
5. **Fixture RPC server** (e2e-tests crate, built early — it is the dev harness for 6–8): deterministic chain synthesis from devnet-state.json, 5 RPC methods, partition {10,20,30}.
6. **Ingest pipeline**: JSON-RPC client (UA, failover, backoff), events-first scan + per-block ingest, PROMOTE_L1, reorg walkback, upgrade/degraded switch, cursors. Tested against the fixture RPC.
7. **Epoch cutter + verify-root + manifest/anchor sidecar + head regen**; determinism test (two backfills byte-identical).
8. **HTTP server**: ServeDir feed + headers/ETags, /health, /v1/stats, /metrics; `strk20 run|backfill|status|epoch verify|verify-root|snapshot|mirror pull` CLI.
9. **`strk20-client`**: FeedTransport (+trybuild locks), FeedStore verify/apply, engine adapter (conformance: engine-over-FeedStore ≡ engine-over-MockBackend), spent-state, cursor persistence in reference schema, reorg rewind rule; `strk20-sync sync` bin + `--watch`.
10. **`strk20-sync verify`** (U6): proof fetch from user RPC + shared mpt walk + non-membership.
11. **Raw endpoints** behind `--enable-raw` + privacy header.
12. **Compat mode**: copied wire types, handlers over DbBackend bridge, hardening (headers, no-log), 11-test upstream replay + SDK wire fixtures.
13. **Acceptance e2e**: recording proxy, dual oracle, legs a–k of §10.3.
14. **Bench harness + nightly live smoke**; README/ops docs; final pass: `strk20 run` zero-flag mainnet check.

Acceptance criterion for the branch = §10.3 green: a test client using the public API exactly as a real wallet would, whose discovered notes match expectations exactly, with the mechanical no-key proof.

---

## 12. Roadmap (explicitly out of this branch)

1. **TOP: `crates/client-wasm` + npm `@strk20/discovery-provider`** (Judges 2+3: keyless privacy only matters if it is the easiest npm integration). Design adopted verbatim from P3: wasm-bindgen over `strk20-client` core (SendWrapper for `?Send`; key in as Uint8Array with an honest zeroization-limit statement; IndexedDB-backed FeedStore impl via the §7.6 seam); TS class `LocalDiscoveryProvider implements DiscoveryProviderInterface` — `discoverNotes`/`discoverChannels`/`discoverRequirement` + `fetchHistory`, reusing the SDK's `notesCursorToApiCursor`/`apiCursorToNotesCursor`/`buildSubchannelCursors`/`convertIncomingNotes` semantics so `NotesCursor`/`ChannelCursor` round-trip identically to `IndexerDiscoveryProvider`; constructor `{feedUrl, storage?}`; drops into `createPrivateTransfers({discoveryProvider})` with zero SDK changes; fixes `ContractDiscoveryProvider`'s three gaps (importability, interface, maturity-blindness). §7.4 cursor interop means Tier-0 users migrate without resync.
2. **Global SSE tail** for U3 latency — one global stream, never per-user subscriptions (durable-fingerprint hazard is a policy line, not a tuning knob).
3. **Prefix-bucket endpoint** per the frozen §6.3 spec (~50 lines) — the documented near-free-PIR halfway; Q9 trigger (~8e5 records) is far off.
4. **Postgres `Store` impl** behind the existing trait — only if hosted compat QPS ever warrants it.
5. **Snapshot-start + pruning UX** — load-bearing when sustained ~100x activity pushes full history toward ~600 MB; format hooks (`snapshots/`, manifest field) already present.
6. **OHTTP / IP-privacy relay** for compat mode — v1 stance: BYO proxy, documented.
7. **Chunked/range tail** if `head.ndjson` wholesale refetch becomes wasteful at extreme activity.
8. Hosted-operator features (API keys, quotas, multi-pool `/pools/{addr}/…`) — deliberately excluded from the flagship posture.
