# Consumer-path addendum — Proposal P2 (operational simplicity)

Status: council proposal, 2026-08-30. Extends
[../../../spec/architecture.md](../../../spec/architecture.md) (base spec);
where it amends the base spec, the section is quoted and the replacement text
given (§9 digest collects every amendment). The roadmap decisions in
[../../../roadmap.md](../../../roadmap.md) are taken as given: two blocks with
the `FeedTransport` seam; WASM as a pure synchronous computer; keyless +
delegated dual API; no write path; deferred items stay deferred. No deadline
shaped anything below.

Design stance of this proposal, applied everywhere:

1. **One wire discipline.** Every new artifact (snapshot, state blob) uses the
   epoch file's existing rules: canonical NDJSON, fixed field order, minimal
   lowercase hex, `\n`-terminated lines, sha256 over uncompressed bytes, zstd
   as transport-only compression. No second serialization framework enters the
   codebase.
2. **Notifications, not data channels.** SSE carries pokes; data always flows
   through the one existing verified fetch path. A second data path would be a
   second verification path, and in five years someone would trust the wrong
   one.
3. **Trust logic lives in Rust, once.** Hash-chain, MPT, snapshot-root and
   compatibility checks run inside `strk20-feed`/Block B code shared by the
   native client and the WASM module. TypeScript stores and moves bytes; it
   never decides whether bytes are valid.
4. **Stateless where a key is involved.** Any server surface that sees a
   viewing key handles it per-request and persists nothing derived from it.
   The only durable key-derived artifacts live on the key owner's device,
   sealed under the key itself.
5. **Smallest state machines.** The browser client keeps the discussion-note
   result (§7 of the 2026-08-30 note): it persists only epoch-derived state
   and therefore contains **no reorg logic at all**.

---

## 0. Preliminary restructuring (prerequisite, pure refactor)

Two mechanical moves that every area below builds on. Both keep the suite
green commit-by-commit and change no behavior.

**0.1 `crates/consumer` — Block B core, wasm-clean.** Extract from
`crates/client` the engine-orchestration logic that must run identically in
native and browser hosts: `reopen_cursor` (the pagination-cursor re-open rule,
implementation-notes delta 1), the `run_incoming`/`run_outgoing` pass loops,
note-registry semantics (`register_notes`, nullifier computation, spent-state
refresh), and the checkpoint/live cursor split from `sync.rs`. The crate is
parameterized over one trait:

```rust
/// The store Block B folds into and the engine reads from. Sync methods —
/// the async engine traits are adapted per host (spawn_blocking natively,
/// immediately-ready futures in wasm).
pub trait ConsumerStore {
    fn read_slot_as_of(&self, slot: &Felt, bound: u64) -> Result<(Felt, u64)>;
    fn events_in_range(&self, from: u64, to: u64) -> Result<Vec<StoredEvent>>;
    fn block_hash(&self, number: u64) -> Result<Option<Felt>>;
    fn apply_block_line(&mut self, line: &BlockLine, finality: Finality) -> Result<()>;
    fn supersede_range(&mut self, from: u64, to: u64) -> Result<()>;
    fn meta_get(&self, key: &str) -> Result<Option<String>>;
    fn meta_set(&mut self, key: &str, value: &str) -> Result<()>;
}
```

`crates/client` keeps: SQLite `FeedStore` (an impl of `ConsumerStore`),
transports, CLI, MPT verify subcommand. `crates/client-wasm` (new, §3) adds
the in-memory impl. Deps of `crates/consumer`: `strk20-feed`,
`discovery-core`, `serde_json`, nothing host-specific — `cargo build -p
strk20-consumer --target wasm32-unknown-unknown` is a CI gate from day one.
Existing conformance tests (engine-over-FeedStore ≡ engine-over-MockBackend)
move with the code and pin the refactor.

**0.2 `crates/wire` — reference wire types, vendored once.** Move
`crates/indexerd/src/compat/wire.rs` + `block_id_serde.rs` into a new leaf
crate `strk20-compat-wire` (pure serde structs, provenance notes preserved,
Apache-2.0 notice moves with them). `indexerd` re-exports for source
compatibility. This is what lets `strk20-sync serve` (§5) speak the reference
wire without the client crate ever depending on `strk20-indexerd` — the base
spec's dependency-direction invariant (§3: "nothing depends on
strk20-indexerd") stays intact and enforceable.

---

## 1. A1 — Snapshots + storage-root anchor

### 1.1 What a snapshot is — and the events question, answered head-on

**A snapshot carries slots only** — the full pool slot set as of one block,
each slot with its value and its last write block. It carries **no events**.

Why this works: discovery is slot-driven. The pool stores encrypted notes in
contract storage (the 48-slot `devnet-state.json` fixture drives full
discovery through `MockBackend`, which has no events at all); the engine's
incoming/outgoing sync probes slots and trial-decrypts slot values.
Note-creation-block metadata comes from `read_slots_with_block`
(`StorageResult.last_update_block`) — served by the snapshot's per-slot write
block, so the 10-block maturity rule and per-note `block_number` are exact.
Spent-state is nullifier-slot state — also slots. What events feed is
exactly one engine surface: `RawEventAccess` → `history::fetch_transactions`
(tx-level history), plus our own `NoteUsed` key1 convenience index.

So the honest capability boundary is: **a snapshot-started client has
complete discovery, balances, spent-state and note metadata; transaction
history is available only from the snapshot block forward.** This is made a
first-class, visible property, not a silent gap:

- the store records `events_floor = snapshot_block + 1` in meta;
- `SyncReport` (and the npm report, §4) gains `"history_from": <block>` —
  `0` for epoch-replayed mirrors;
- any history API (`/v1/history` on serve, `history()` in npm/wasm) refuses
  ranges below the floor with error `HISTORY_UNAVAILABLE` naming the floor;
  it never silently returns a truncated answer;
- the escape hatch is the feed itself: a consumer that wants full history
  replays epochs (the canon is untouched); snapshot start is an optimization,
  never a replacement. `strk20-sync serve` therefore defaults to epoch replay
  (§5.4).

The alternative — snapshots that also carry all historical events — was
rejected on merit: it reproduces most of the feed's bulk (event data
dominates raw feed volume), erases the O(1)-cold-start win it exists for, and
adds a second full-history artifact whose completeness cannot be
root-verified (the MPT commits storage, not events). One snapshot format,
slots only, with the gap stated loudly.

The acceptance proof that discovery needs no pre-floor events is leg **l**
(§8): cold-start-from-snapshot output must equal full-replay output
field-for-field. If any future engine version reads events during discovery,
that leg goes red before a release does.

### 1.2 Snapshot wire format v1 (byte-precise, frozen)

File `snapshots/{block:010}.strk20s.zst` = zstd-19 over canonical NDJSON.
Content identity = sha256 over the **uncompressed** payload (same rule as
epochs, §4.3 of the base spec; `zst` hash is transport checksum only).
Canonical JSON rules identical to epochs: fixed field order, no whitespace,
minimal lowercase hex, every line `\n`-terminated.

```
line 1 (header):
{"t":"hdr","v":1,"kind":"strk20-snapshot","chain_id":"SN_MAIN","pool":"0x…","block":14059999,"epoch":1405,"epoch_hash":"<64-hex content hash of epoch 1405>","storage_root":"0x…","class":"0x…"}

one line per slot, ascending by the 32-byte BE slot:
{"t":"s","k":"0x<slot>","v":"0x<value>","w":<last write block ≤ header.block>}

last line (footer):
{"t":"end","slots":<n slot lines>}
```

Invariants, all test-asserted:

- `header.block` is the `to` of epoch `header.epoch` — snapshots are cut
  **only at epoch boundaries** (boundary alignment; keeps snapshots on the
  same L1-final, immutable footing as epochs and lets them share the epoch's
  anchor).
- `header.epoch_hash` = the manifest hash of that epoch — this is the binding
  that lets a snapshot-started client continue the hash chain (§1.5).
- `header.storage_root` = the pedersen-MPT root of exactly the slot lines,
  computed with the shared `feed::mpt` module.
- The payload is a pure function of DB rows as of `block` → byte-identical
  across mirrors and re-runs (extends the determinism guarantee, leg **m**).
- Zero-valued slots are never emitted (Cairo map semantics: absent = zero).

### 1.3 Manifest amendment

Base spec §4.4 currently reads:

> `"snapshot":{"block":14049912,"sha256":"<64-hex>","bytes":123456}}` …
> `snapshots/latest.sqlite.zst`: content-addressed convenience export;
> `strk20 snapshot import` verifies it against the epoch chain before use.

Replaced by (the field has never been emitted by the built system —
implementation-notes "Not in this branch" — so this is a schema definition,
not a migration):

```json
"snapshot": {"block":14059999, "epoch":1405, "epoch_hash":"<64-hex>",
             "file":"0014059999.strk20s.zst", "sha256":"<64-hex>",
             "zst":"<64-hex>", "bytes":123456, "slots":48123,
             "storage_root":"0x…"}
```

`snapshot` stays `null` until the first cut. The SQLite snapshot
(`latest.sqlite.zst`) and the `strk20 snapshot create|import` subcommands are
**deleted from the spec**: one snapshot format, portable to every consumer
(SQLite is unreadable to the browser and TS; `mirror pull` already covers
server bootstrap). Old clients deserialize the new manifest fine (serde
ignores unknown fields and `Option` covers absence).

### 1.4 Cutter behavior

At the end of every successful `cut_ready_epochs` batch (i.e. after the last
epoch of the batch and its manifest rewrite):

1. `full_slot_set_as_of(to)` — the same query verify-root already runs, so
   the marginal cost of snapshotting is serialization + one MPT recompute;
2. compute `storage_root` locally; serialize; sha256; zstd; atomic
   tmp+rename into `snapshots/`;
3. update `manifest.json` (`snapshot` field) atomically;
4. retention: keep the newest **2** snapshot files (the previous one survives
   one cut so clients holding the prior manifest never 404 mid-download);
   delete older ones. Snapshots are derived artifacts — deletable, never part
   of the hash chain, never pulled by `mirror pull` (a mirror regenerates its
   own from its DB; determinism makes them byte-identical anyway, leg m).

Cadence falls out: one snapshot per epoch cut (~10 000 blocks). No timer, no
new state machine — the cutter's existing trigger is the trigger.

If verify-root failed for the batch, **no snapshot is written** (same rule as
"never publish a divergent epoch"): a snapshot is a claim about the full slot
set and must not outrun the completeness check.

### 1.5 Client cold start (Rust and browser — same algorithm)

`FeedStore::apply_feed` (and the wasm module, §3) gains one branch, taken
only when the local mirror is empty:

```
if mirror is empty AND manifest.snapshot != null:
    fetch snapshots/{file}; decompress; sha256 == manifest.snapshot.sha256 or fail
    parse; header.chain_id/pool == pinned or fail (CHAIN_MISMATCH / POOL_MISMATCH)
    recompute mpt root over slot lines == header.storage_root
        == manifest.snapshot.storage_root or fail (SNAPSHOT_ROOT_MISMATCH)
    manifest.epoch(header.epoch).hash == header.epoch_hash or fail (CHAIN_BROKEN)
    insert each slot line into storage_log as (slot, w, v)   # one transaction
    meta: last_epoch_applied = header.epoch
          last_epoch_hash    = header.epoch_hash
          last_epoch_to      = header.block
          events_floor       = header.block + 1
# then the normal path runs unchanged: epochs > last_epoch_applied verify
# against prev_hash = epoch_hash and apply; head tail applies on top.
```

No new tables: snapshot rows land in `storage_log` with their real write
blocks, so the existing as-of query serves them. The one new rule: **the
snapshot block is the client's minimum view bound** — enforced trivially,
because the only bounds the client ever uses are `last_epoch_to` (≥ snapshot
block by construction) and `head`.

A non-empty mirror never touches snapshots (no mixing; an existing client
keeps its epoch path). `strk20-sync sync --cold-start auto|snapshot|epochs`
(default `auto` = the branch above) makes the choice explicit and testable;
`epochs` forces full replay for history-complete mirrors.

O(1) cold start, concretely: manifest + snapshot + (epochs cut since the
snapshot, normally 0–1) + head — a bounded number of requests and bytes
independent of chain history length, for both the Rust client and the
browser.

### 1.6 Verification story (what the anchor buys)

The snapshot inherits the feed's delegated-trust-with-audits posture, with
the same three rings:

1. **Integrity** (always, offline): sha256 vs manifest; MPT root recompute
   vs the header's declared root. Any bit flip in transit or on a mirror is a
   named hard error (leg l tamper case).
2. **Server-side completeness** (always): the snapshot is only cut after
   verify-root passed for its batch (§1.4) — the declared root has been
   checked against `getStorageProof` on the server side.
3. **Client-side anchoring** (paranoid path, U6): the snapshot block is an
   epoch `to`, so when the epoch's anchor sidecar exists,
   `snapshots root == anchor.storage_root` links the snapshot to a full
   `getStorageProof` response whose global roots a client can check against
   its **own** RPC — `strk20-sync verify` gains `--snapshot`, which does
   exactly that walk with the already-shared `feed::mpt` module. When the
   anchor is absent (proof window missed), ring 3 is unavailable and the
   client says so; rings 1–2 still hold. The server remains outside the proof
   path.

### 1.7 Mirror-pull interplay

Unchanged for servers: `strk20 mirror pull` ingests **epochs** (a server
needs events to cut future epochs and serve `/v1/raw/events` — it can never
bootstrap from a slots-only snapshot; stated in ops docs). After a pulled
mirror's first own cut batch, it emits its own snapshot, byte-identical to
the origin's (leg m).

### 1.8 Spec amendments (beyond §1.3)

- §4.2 tree line `snapshots/latest.sqlite.zst  # optional convenience` →
  `snapshots/{block:010}.strk20s.zst  # slots-only state snapshot (addendum §1)`.
- §6.1 table row for snapshots → serves the new file names (route code
  already generic).
- §8 CLI: drop `strk20 snapshot create|import`; add `--snapshot-keep <n>`
  (default 2) to `strk20 run|backfill`; add `--cold-start` to
  `strk20-sync sync`; `strk20-sync verify` gains `--snapshot`.
- §12.5 (roadmap "Snapshot-start") — delivered by this addendum.

### 1.9 Acceptance criteria (details in §8, leg l/m)

Cold-start equality vs full replay; tamper detection at each of sha256, root,
epoch_hash binding; `history_from` surfaced and enforced; snapshot
determinism across independent backfills; no snapshot after a verify-root
failure.

---

## 2. A2 — SSE on the indexer

### 2.1 Shape: a poke stream, not a data stream

One new endpoint on the server binary:

```
GET /feed/live            (always on; no parameters; text/event-stream)
```

It notifies; it never carries chain data. On any state change the client
reacts by fetching the same files it would have polled — `head.ndjson` via
ETag, `manifest.json`, epoch files — through the one existing verified path.
Rationale (stance #2): inline diff frames would be a second wire format with
a second verification story and a proxy-buffering failure mode that corrupts
rather than merely delays. A lost or buffered poke costs latency only; the
fallback poll (§2.5) bounds it. This also keeps base-spec R3's substance: the
only stream is global, and no wire rollback protocol exists — a reorg is just
another `head` poke followed by the wholesale tail refetch clients already do.

### 2.2 Event framing (exact)

```
retry: 15000

event: head
id: h:<head_number>:<first 16 hex of etag>
data: {"head":14056431,"head_hash":"0x…","l1_accepted":14049912,"tail_from":14050000,"etag":"<64-hex sha256 of head.ndjson>"}

event: epoch
id: e:<idx>
data: {"epoch":1406,"from":14060000,"to":14069999,"hash":"<64-hex>"}

event: status
data: {"decode_state":"ok"|"degraded"}

: keepalive          (comment line, every 15 s of silence)
```

Rules:

- On connect the server always sends the current `head`, the latest `epoch`
  (if any), and `status` — every event is **state-carrying and idempotent**,
  never a delta.
- `head` fires on any change of head.ndjson bytes (new block, reorg, l1
  promotion) — the `etag` field lets a client skip a redundant conditional
  GET it has already applied.
- `epoch` fires only **after** the manifest rewrite that lists the epoch
  (ordering by construction: the emitter watches the published files, §2.6),
  so a poked client always finds the entry it fetches for.
- `status` fires on `decode_state` transitions (degraded-mode visibility,
  base spec §5.7).

### 2.3 Resume and Last-Event-ID

`id:` fields are set (so `EventSource` reconnects send `Last-Event-ID`), and
the server deliberately **ignores** the header: because every event carries
full current state and connect always replays current state, resume logic is
the empty program. A client that was away simply receives the present. Missed
intermediate epochs are discovered from the manifest on the next fetch — which
the connect-time `epoch`/`head` events trigger. This is documented in the
endpoint docs so nobody "fixes" it into a journal later; the ids exist purely
for client-side dedup and debuggability.

### 2.4 Privacy invariant

The request is parameterless and identical for every subscriber; the only
client-varying header, `Last-Event-ID`, encodes public feed position and is
ignored. Extended acceptance assertion (leg **n**): two concurrent watchers'
captured request bytes are multiset-identical, and the SSE capture contains
no key/address encoding under the leg-d byte scanner. The feed-route doctrine
("no feed route takes any parameter derived from a user", base spec §6.1)
now covers `/feed/live` explicitly.

### 2.5 Fallback to polling

Nothing about polling changes; SSE is strictly additive. Client rule
(implemented in the npm wrapper, available to any consumer): on `EventSource`
error, reconnect with exponential backoff (1 s → 60 s cap, jittered); while
disconnected, poll `head.ndjson`/ETag at the existing cadence (30 s default).
On any doubt, poll — the two paths converge on identical bytes. The Rust
`--watch` mode **stays polling-only**: it runs on servers where a 30 s poll
is fine, and one fewer HTTP client mode is one fewer thing to maintain. (If
latency-sensitive Rust consumers appear, the consumption loop is ~60 lines;
merit trigger recorded here.)

### 2.6 Implementation and proxy friendliness

One global watcher task hashes `head.ndjson` + reads `manifest.json` on a 1 s
interval and publishes to a `tokio::sync::watch::Sender<FeedState>`; each
connection is a subscriber that formats events. File-watching (rather than
plumbing channels out of the ingest loop) keeps the emitter decoupled,
crash-consistent, and correct-by-construction on ordering — it can only
announce what is already published and fetchable. Response headers:
`Cache-Control: no-cache`, `X-Accel-Buffering: no`, keepalive comments per
§2.2. No connection cap in v1 (self-host posture); `/metrics` gains
`strk20_sse_connections`.

### 2.7 Spec amendments

- §6.1 table: add row `GET /feed/live | SSE poke stream (addendum §2) |
  no-cache, no buffering`.
- R3 ("no SSE/409 wire protocol in v1") — the v1 branch shipped without it as
  resolved; this addendum delivers the §12.2 roadmap item within R3's
  guardrails: single global stream, never per-user, no rollback protocol,
  polling remains the reference semantics.

---

## 3. A3 — the WASM package of Block B

### 3.1 Crate and module posture

New crate `crates/client-wasm` (cdylib, `wasm-bindgen`), a thin shell over
`crates/consumer` (§0.1) plus an in-memory `ConsumerStore`:

```rust
pub struct MemStore {
    // as-of reads: per slot, ascending (block, value) writes
    slots: BTreeMap<[u8; 32], Vec<(u64, Felt)>>,
    events: BTreeMap<u64, Vec<StoredEvent>>,   // by block
    blocks: BTreeMap<u64, BlockMeta>,          // hash, parent, ts, finality
    meta: BTreeMap<String, String>,
}
```

The module is a pure synchronous computer (roadmap-given): no network, no
storage, no timers, no async JS. The engine's `async fn` traits are driven
with `futures::executor::block_on` over futures that never actually suspend
(every store method is synchronous — the spike proved this exact pattern);
`?Send`/`SendWrapper` handled as in the spike. zstd is **not** compiled in
(given: `zstd-sys` has no wasm path); the module receives uncompressed
payloads and hashes them — content identity is over uncompressed bytes, so
nothing is lost. `strk20-feed` builds with `mpt` (given), so snapshot-root
verification runs inside the module.

### 3.2 Exported ABI (exact)

```rust
#[wasm_bindgen]
pub struct Engine { /* MemStore + pinned identity + chain position */ }

#[wasm_bindgen]
impl Engine {
    /// `genesis_json` = the feed's genesis.json bytes; pins chain_id/pool.
    #[wasm_bindgen(constructor)]
    pub fn new(genesis_json: &[u8]) -> Result<Engine, JsError>;

    /// JSON: {"chain_id","pool","last_epoch","last_epoch_hash","last_epoch_to",
    ///        "head","l1_accepted","events_floor","module_version"}
    pub fn info(&self) -> String;

    /// Restore from a previously exported blob. "loaded" | "stale".
    /// Identity mismatch (chain/pool/format) THROWS; content staleness
    /// (unknown epoch hash — feed moved or blob from another mirror line)
    /// RETURNS "stale": the caller refolds from raw artifacts.
    pub fn load_state(&mut self, blob: &[u8]) -> Result<String, JsError>;

    /// Serialize the folded epoch-derived state (§3.3). Called once per
    /// applied epoch at most — never on head pokes.
    pub fn export_state(&self) -> Vec<u8>;

    /// `manifest_snapshot_json` = the manifest's "snapshot" object bytes.
    /// Verifies sha256, MPT root, chain/pool pin, epoch_hash binding (§1.5).
    pub fn apply_snapshot(&mut self, payload: &[u8],
                          manifest_snapshot_json: &[u8]) -> Result<(), JsError>;

    /// `manifest_entry_json` = the manifest "epochs[i]" object bytes.
    /// Verifies content hash + prev-chain against internal position.
    pub fn apply_epoch(&mut self, payload: &[u8],
                       manifest_entry_json: &[u8]) -> Result<(), JsError>;

    /// Wholesale tail replace above the epoch floor (client §7.5 semantics,
    /// minus persistence). JSON: {"head","head_hash","l1_accepted","tail_rewound"}
    pub fn apply_head(&mut self, payload: &[u8]) -> Result<String, JsError>;

    /// The ONLY key-accepting entry point. Runs checkpoint+live discovery
    /// (incoming+outgoing), spent refresh; returns the report and the new
    /// sealed per-key state (§3.4). `sealed` = None on first run.
    pub fn discover(&self, address_hex: &str, viewing_key: &[u8],
                    sealed: Option<Vec<u8>>) -> Result<DiscoverOut, JsError>;

    /// Tx history ≥ events_floor; HISTORY_UNAVAILABLE below it.
    pub fn history(&self, address_hex: &str, viewing_key: &[u8],
                   sealed: &[u8], from_block: u64, limit: u32)
                   -> Result<String, JsError>;
}

#[wasm_bindgen]
pub struct DiscoverOut {
    #[wasm_bindgen(getter_with_clone)] pub report_json: String, // SyncReport shape
    #[wasm_bindgen(getter_with_clone)] pub sealed: Vec<u8>,
}
```

`report_json` is the **exact serde shape of the Rust `SyncReport`** (plus
`history_from`) — one report schema across native CLI, wasm, and npm, so the
golden oracle pins are shared verbatim.

The browser needs no reorg surface: `apply_head` rebuilds the tail wholesale
in memory; persisted state is epoch-derived only (§4.4), so `tail_rewound` is
informational. No generation counters, no rewind entry points — the state
machine the native client needs for its durable tail does not exist here.

### 3.3 State blob format (`export_state`) — versioned, boring

Canonical NDJSON, same discipline as everything else (debuggable with `zcat`
and `jq`; compresses well if the wrapper chooses to):

```
{"t":"hdr","v":1,"kind":"strk20-state","chain_id":"SN_MAIN","pool":"0x…","last_epoch":1405,"last_epoch_hash":"<64-hex>","last_epoch_to":14059999,"events_floor":14050000,"module":"<crate semver>"}
{"t":"s","k":"0x…","v":"0x…","w":14031234}          # slots, ascending, as-of last_epoch_to
{"t":"ev","b":14051200,"i":0,"x":2,"h":"0x<tx>","K":["0x…"],"D":["0x…"]}   # events ≥ events_floor, ascending (b,i)
{"t":"end","slots":N,"events":M}
```

The header is the **compatibility stamp** (discussion §7): format version,
chain id, pool, last applied epoch + its hash. `load_state` validates the
stamp per §3.2. Only epoch-derived state is ever exported — the tail is
excluded by construction (it lives and dies in memory), which is what makes
the blob un-stale-able by reorgs. Per-key material is **never** in this blob.

### 3.4 Sealed per-key state

The per-key artifacts (checkpoint discovery cursors incoming+outgoing,
`ckpt_at`, and the note registry rows) are key-derived — a fingerprint on a
shared machine (discussion §7 note). The module seals them itself, so the
wrapper and IndexedDB only ever see ciphertext:

```
sealed = "S20K1" || nonce(24 bytes, random) ||
         XChaCha20-Poly1305(
           key   = HKDF-SHA256(ikm = viewing_key 32-byte BE,
                               info = "strk20-sealed-state-v1"),
           aad   = chain_id || pool,
           plain = JSON {"v":1,"ckpt_at":N,"cursor_in":{…},"cursor_out":{…},
                          "notes":[NoteRow…]} )
```

Cursors use the exact reference JSON schema (base spec §7.4) — sealed-state
cursors round-trip with compat/serve wire cursors. New deps:
`chacha20poly1305`, `hkdf` (RustCrypto, pure Rust, wasm-clean),
`getrandom` with the `js` feature. A wrong key fails AEAD open → the module
returns `SEALED_STATE_MISMATCH` and the caller restarts discovery from
scratch for that key (correct behavior for "different user on same origin").

### 3.5 Error model

Every fallible export throws `JsError` whose message is a single JSON object:

```json
{"code":"HASH_MISMATCH","epoch":1406,"expected":"<64-hex>","actual":"<64-hex>","retryable":false}
```

Closed code set (shared constants with the npm package):
`POOL_MISMATCH`, `CHAIN_MISMATCH`, `MALFORMED`, `HASH_MISMATCH`,
`CHAIN_BROKEN`, `SNAPSHOT_ROOT_MISMATCH`, `FEED_ADVANCED_MIDSYNC`
(retryable — the manifest/head race guard, same rule as the native store),
`STATE_STALE`, `SEALED_STATE_MISMATCH`, `DISCOVERY_INCOMPLETE` (pass budget
exhausted), `HISTORY_UNAVAILABLE` (carries `"floor"`). The npm wrapper adds
exactly one code of its own: `TRANSPORT` (network failures, always
retryable). Nothing else is ever thrown across the boundary.

### 3.6 Managing the discovery-core patch until the upstream PR lands

Given: upstream needs a two-line feature gate on `starknet-providers`
(roadmap item 7). Until merged:

1. Fork `starkware-libs/starknet-privacy` under our org; branch
   `feature-gate-providers` = pinned rev `74841caf` + exactly the two-line
   patch; workspace `[patch]` entry pins **our fork by rev** for wasm builds
   only (native builds keep the upstream pin — behavior identity where it
   compiles today).
2. The diff is also vendored at `patches/discovery-core-providers.patch`
   with a CI job that clones upstream at the pinned rev, applies the patch,
   and asserts tree-hash equality with our fork rev — a mechanical proof the
   fork is upstream + these two lines and nothing more, re-run on every CI
   pass so drift is impossible to hide.
3. When the upstream PR merges and a rev containing it is pinned, the
   `[patch]` entry and `patches/` file are deleted in one commit. The CI job
   inverts into a tripwire that fails if the `[patch]` section ever returns.

### 3.7 Size and packaging

`wasm-pack --target web`. CI budget assertion: gzip size ≤ 300 KB (spike
baseline 231 KB + codec + AEAD; fail loudly on regression past the budget
rather than creep). The artifact is consumed only via the npm package (§4) —
not published separately.

---

## 4. A4 — the npm package

### 4.1 Name and surface

Unscoped **`strk20-discovery`** (discussion §6: free, no org dependency, no
false officialness). ESM + `.d.ts`, built with `tsc` only — no bundler; the
wasm loads via `new URL('engine_bg.wasm', import.meta.url)`. Node ≥ 20 and
evergreen browsers.

```ts
export interface Strk20Discovery {
  getNotes(id: Identity): Promise<DiscoveryReport>;
  subscribe(id: Identity, onUpdate: (r: DiscoveryReport) => void): () => void; // returns unsubscribe
  history(id: Identity, opts?: {fromBlock?: number; limit?: number}): Promise<HistoryPage>;
  status(): Promise<ClientStatus>;   // {head, l1Accepted, lastEpoch, historyFrom, mode:"keyless"|"delegated", live:boolean}
  close(): Promise<void>;
}

export type Identity = { address: string; viewingKey: Uint8Array | string };
// DiscoveryReport = the SyncReport JSON shape, camelCased by a generated mapper,
// with the raw JSON also exposed as .raw for oracle-equality tests.

export class KeylessClient implements Strk20Discovery {
  constructor(opts: { feedUrl: string;
                      storage?: "indexeddb" | "memory";   // default indexeddb, memory fallback
                      fullHistory?: boolean;              // default false: snapshot cold start
                      pollIntervalMs?: number });         // fallback poll, default 30_000
}

export class DelegatedClient implements Strk20Discovery {
  constructor(opts: { serverUrl: string; authToken?: string; pollIntervalMs?: number });
}
```

One interface, two constructors — exactly the roadmap's dual API. The keyless
honesty note ships in the README verbatim from base spec §12.1: JS cannot
guarantee zeroization of key copies; the guarantee is *non-transmission*
(and the acceptance capture proves it), not memory hygiene.

### 4.2 KeylessClient data flow

```
open IDB → (Design M only: try Engine.load_state)
        → else: read stored snapshot/epochs → decompress (fzstd) → Engine.apply_*
fetch manifest → fetch & apply missing artifacts (snapshot if cold, epochs > last)
fetch head (ETag) → apply_head
per identity: sealed = IDB.keys[id] → Engine.discover(addr, key, sealed)
            → store new sealed; fire callback if report changed
subscribe(): EventSource /feed/live → on head/epoch event, repeat the
             fetch-apply-discover slice; on SSE error, poll fallback (§2.5)
```

All sync passes run under `navigator.locks.request("strk20:<dbname>", …)` so
multiple tabs serialize (graceful no-op where Web Locks is absent —
concurrent passes are idempotent, just wasteful).

### 4.3 IndexedDB layout

Database name `strk20:<chain_id>:<pool>` (per-chain isolation is structural —
a Sepolia blob can never meet a mainnet engine), version 1:

| store | key | value |
|---|---|---|
| `meta` | string | string (`format_v`, `last_epoch`, `last_epoch_hash`, `head_etag_hint`, `snapshot_block`) |
| `artifacts` | `"snapshot"` \| epoch idx (number) | `{hash: string, zbytes: ArrayBuffer}` — the **compressed bytes exactly as served** |
| `state` | `"folded"` | `ArrayBuffer` (export_state blob) — Design M only |
| `keys` | hex(sha256(chain_id‖pool‖address)) | `{sealed: ArrayBuffer, updatedAt: number}` |

Storing served-verbatim compressed artifacts is the provenance-simple choice:
what is on disk is exactly what the mirror served, and the module re-verifies
the full chain on every load — same-origin tampering with IDB is caught by
the same hash checks that catch a hostile mirror (discussion §7 trade-off,
resolved on the "raw epochs as source of truth" side). The `keys` store's
record key is address-derived and therefore itself a fingerprint; contents
are sealed (§3.4); both facts are documented in the README's shared-machine
section.

Eviction honesty: IDB is best-effort storage (browser may evict under
pressure); every load path must work from empty — which it does, because
empty = cold start (§1.5).

### 4.4 Persistence: both designs, and the gate that picks

**Design R — raw artifacts only (the default lane).** Persist: `artifacts`
(snapshot + epochs after it; with `fullHistory`, all epochs), sealed per-key
blobs, meta. Every page load refolds from raw artifacts. Never persist the
tail; never persist folded state. Consequences: zero reorg logic, zero cache
coherence, one source of truth, and the hash chain is re-verified on every
single load. Per-key blobs store only the **checkpoint** cursor (computed at
`last_epoch_to`); the live pass reruns from the checkpoint on every slice —
mirroring the native ckpt/live split with the live half kept purely in
memory, so a tail reorg can never poison anything durable.

**Design M — folded-mirror cache (the conditional layer).** Everything in R,
plus: after applying a new epoch (and only then — never on head pokes,
discussion §7 cadence warning), `export_state()` → `state` store, keyed by
the stamp inside the blob. Load path: `load_state` fast path; `"stale"` or
identity throw → delete the record, fall back to R's refold, re-export.
Strictly a cache: deleting the record at any time is always correct.

**The measurement gate (defined now, run before any TS beyond the harness is
written).** Harness `ts/strk20-discovery/bench/fold.bench.ts`, headless
Chromium via Playwright on the standard CI runner, five runs each over two
recorded inputs (checked-in fixture set `bench/fixtures/mainnet-feed-<date>/`,
refreshed manually from the live feed):

- **P1 snapshot path** (the default UX): snapshot + epochs-after + head →
  `t_apply` = wall time of all `apply_*` calls (excludes network and fzstd,
  which are measured separately as `t_zstd`).
- **P2 full-history path** (`fullHistory: true`): all epochs + head.

Decision rule: ship Design M **iff** median `t_apply(P1)` > 500 ms or median
`t_apply(P2)` > 2000 ms; otherwise ship Design R alone, keep
`load_state`/`export_state` in the ABI (they cost nothing dormant and M can
be turned on later by measurement, not by argument). Thresholds are the
discussion-note bands (§7: ~200 ms ⇒ layer unnecessary, ~2 s ⇒ mandatory)
made operational. Measured numbers land here: **[FILL-IN after gate run:
t_zstd P1/P2, t_apply P1/P2, verdict]**. Expectation stated for the record:
with A1 snapshots, P1 folds a bounded working set and R should win; P2 is
the only plausible M trigger.

### 4.5 zstd in TypeScript

**`fzstd`** (pure-JS decompress-only, ~8 KB gzipped, MIT, zero native/wasm
deps). We never compress client-side, so a decompress-only dependency is the
whole requirement. Rejected: zstd wasm builds (a second wasm to version and
load), `DecompressionStream` (no zstd support in browsers), changing the feed
compression (the format is frozen). `fzstd` is pinned exact-version and its
output is always sha256-verified against the manifest before use, so a
decompressor bug cannot smuggle bytes past verification — worst case is a
loud hash mismatch.

### 4.6 DelegatedClient

Speaks the **reference compat wire** (`POST /v1/sync/incoming_state` /
`outgoing_state`, `/v1/history` — types from `crates/wire`) to either
`strk20-sync serve` (§5) or `strk20 --enable-compat` — one dialect, two
servers, and by construction also any stock reference deployment.
`subscribe()` = EventSource on the server's `/feed/live` poke (§5.2) +
keyed re-query of `incoming_state`/`outgoing_state` on poke; cursors from the
server round-trip into subsequent requests (reference schema — the §7.4
interop guarantee, now exercised from TS). The viewing key is sent in request
bodies over the user's own transport to their own server; the README states
this trust boundary in the same words as the base spec's compat labeling.

### 4.7 TS e2e against the real server binary (test-first)

The fixture RPC server (already an in-process axum harness in
`crates/e2e-tests`) gains a binary target `fixture-rpc` (`--listen
127.0.0.1:0 --state vendor/fixtures/devnet-state.json`, prints
`PORT=<n>` on stdout; flags to extend the chain / fork the tail mirror the
in-process harness API). The vitest suite (`ts/strk20-discovery/e2e/`):

1. spawns `fixture-rpc`, then the **real `strk20` binary** (path from
   `STRK20_BIN`, default `target/debug/strk20`) with a temp feed dir;
2. runs `KeylessClient` in Node (`fake-indexeddb`, `undici` EventSource
   polyfill) → asserts `report.raw` **equals the same O2 golden JSON** the
   Rust acceptance test pins (one oracle file, loaded from the repo, never
   duplicated);
3. simulates reload (new client instance, same fake-IDB) → asserts the
   request log (recorded by a Node reverse proxy, port-for-port the Rust
   recording proxy's role) shows only `{manifest, head}` + SSE — persistence
   proof;
4. runs `DelegatedClient` against `strk20-sync serve` → same golden;
5. byte-scans every captured keyless request for key/address encodings —
   the leg-d scanner reimplemented once in TS with its own self-test leg
   (it must find the key in the delegated capture).

CI order: cargo build → cargo e2e → npm e2e. No network anywhere.

---

## 5. A5 — `strk20-sync serve`

### 5.1 The shape: a stateless keyed read head over a verified mirror

`strk20-sync serve` is Block B running server-side for self-hosters: it
maintains a verified `sync.db` mirror from any feed (HTTP mirror or local
dir) and serves the **reference compat wire** over the unmodified engine.
It is deliberately **stateless with respect to keys**: the viewing key
arrives per request (reference wire body), drives one engine pass over the
mirror, and is dropped; cursors travel in requests/responses (reference
schema) and are **never persisted server-side**; there is no subscription
registry and no per-key background work. `serve.db` contains only the public
mirror — nothing key-derived, ever (still chmod 0600, uniformly with sync).

Endpoints:

```
GET  /health                      # ops shape (same as base §6.2)
POST /v1/sync/incoming_state      # reference wire (crates/wire types)
POST /v1/sync/outgoing_state
POST /v1/sync/preflight_check
POST /v1/history                  # HISTORY_UNAVAILABLE below events_floor (§1.1)
GET  /feed/live                   # the §2.2 poke stream, fired after each
                                  # successful local apply_feed pass
```

Every keyed response carries `X-Strk20-Mode: delegated-keyed`.

### 5.2 Why no keyed SSE

`subscribe(key)` on the wire was considered and rejected: it would make the
serve process hold viewing keys for connection lifetimes, schedule per-key
engine work, and maintain a subscription registry — three state machines
bought for nothing, because the poke-then-keyed-requery pattern (§4.6)
delivers the same latency with the key held only for request duration. The
delegated and keyless clients end up with the **same** update loop shape
(poke → recompute), which is the property a future maintainer will thank us
for. The durable-fingerprint policy line (base spec: never per-user push) is
also structurally honored rather than carefully avoided.

### 5.3 The "third FeedTransport impl" — resolved as deployment, not code

The roadmap's in-process seam ("server reads its own DB directly") is
delivered without a new transport, and this addendum records why:

- For **Block B inside the server binary**, the in-process path already
  exists and ships: compat mode's `bridge.rs` `DbBackend` runs the unmodified
  engine directly over `strk20.db` with zero HTTP. That *is* "the server
  reads its own DB".
- For **colocated `strk20-sync serve`**, the canonical in-process source is
  the feed directory — the server's canonical product — via the existing
  `DirTransport` (`--feed /var/lib/strk20/feed`): in-process file reads, no
  network, full hash-chain verification retained.
- A literal DB-reading `FeedTransport` is **rejected on merit**: `strk20.db`
  does not contain epoch payloads (only their hashes), so such a transport
  would have to re-run the cutter's serializer inside the client crate —
  importing server code across the base spec's dependency-direction privacy
  boundary — while *bypassing* the verification seam the trait exists to
  protect. It would save one redundant file read and cost the invariant.

Deliverable for this item: the colocation deployment documented in ops docs +
acceptance leg **q** proving `serve --feed <dir>` against a live cutter's
dir. The seam stays exactly as pre-cut.

### 5.4 CLI and security posture

```
strk20-sync serve --feed <URL|DIR> [--listen 127.0.0.1:7020] [--db serve.db]
                  [--cold-start epochs|snapshot]     # default: epochs (full history)
                  [--poll <secs, default 5>]         # feed re-apply cadence
                  [--auth-token-file <path>]         # optional static bearer token
```

- Binds loopback by default; changing `--listen` prints the same loud
  key-visible warning compat mode prints. TLS is explicitly out of scope
  (self-hosters terminate TLS in their reverse proxy — one fewer cert state
  machine in our binary).
- `--auth-token-file` (never argv) enables `Authorization: Bearer` checking
  for LAN exposure; constant-time compare.
- Request/response bodies never logged (hard-coded, same rule as compat);
  keys and cursors exist only in request scope.
- Default `--cold-start epochs`: a serve instance is a server; complete
  `/v1/history` is worth the one-time replay. `snapshot` is allowed and
  answers pre-floor history with `HISTORY_UNAVAILABLE` (§1.1).

### 5.5 Relation to server compat mode

| | `strk20 --enable-compat` | `strk20-sync serve` |
|---|---|---|
| ingest | full RPC ingest (Block A) | none — folds any feed |
| trust root | its own RPC + verify-root | the feed hash chain (+ optional own-RPC verify) |
| state seen | strk20.db (authoritative) | sync.db mirror (verified copy) |
| wire | reference compat wire | **identical** wire (same `crates/wire` types) |
| when to run | operator hosting the full stack | lightweight self-host against any public mirror |

Same dialect everywhere means `DelegatedClient`, the stock SDK
`IndexerDiscoveryProvider`, and any Tier-0 integration work against both
without a code path fork. Conformance: the compat wire tests run against
**both** mounts (leg q reuses the leg-h assertions verbatim).

---

## 6. A6 — chain profiles

### 6.1 Mechanism

`ChainConfig` (crates/indexerd/src/config.rs) generalizes to a profile
registry — same struct, plus identity and RPC defaults, minus nothing:

```rust
pub struct ChainProfile {
    pub name: String,          // "mainnet" | "sepolia" | custom
    pub chain_id: String,      // "SN_MAIN" | "SN_SEPOLIA"
    pub pool: Felt,
    pub genesis_block: u64,
    pub epoch_size: u64,       // 10_000 for both built-ins; frozen per feed in genesis.json
    pub decoder_map: HashMap<Felt, String>,
    pub rpc_primary: String,
    pub rpc_fallback: Option<String>,
}

pub fn profile(name: &str) -> Option<ChainProfile>;   // built-ins
pub fn from_toml(path: &Path) -> Result<ChainProfile>; // custom chains
```

CLI: `strk20 run --network sepolia` (default `mainnet`; `--config <toml>`
overrides/extends a profile; explicit flags override both — precedence:
flags > config file > profile). One process serves exactly **one** chain and
one feed dir; multi-chain = multiple processes (no `/pools/{addr}` routing —
the base spec's §12.8 exclusion stands).

TOML shape (also the fill-in vehicle):

```toml
[chain]
name         = "sepolia"
chain_id     = "SN_SEPOLIA"
pool         = "0x0254a6b2997ef52e9f830ce1f543f6b29768295e8d17e2267d672c552cfe0d91"
genesis_block = 0        # FILL-IN: pool deployment block (parallel research task)
epoch_size   = 10000
rpc_primary  = "https://…"   # FILL-IN: chosen sepolia archive endpoint
rpc_fallback = "https://…"

[chain.decoders]
# FILL-IN: verified sepolia class-hash table from the parallel research task.
# Mechanism note: an on-chain class absent from this map triggers degraded
# mode exactly as on mainnet (base spec §5.7) — an incomplete table degrades
# loudly, it never corrupts.
```

The fill-in slots are deliberately *data*, not design: the mechanism ships
and is acceptance-tested with a synthetic profile (leg r) regardless of when
the verified Sepolia values land.

### 6.2 Chain identity stamped end to end

The chain id and pool already ride the wire (`genesis.json`, `manifest.json`,
every epoch header). This addendum closes the checks so a wrong-chain
artifact can never be applied quietly — six checkpoints, three of them new:

1. Server INIT: `starknet_chainId` == profile (exists).
2. Epoch/head cutting stamps profile `chain_id`/`pool` (exists).
3. **Client pin (extended):** `FeedStore::apply_feed` today pins and compares
   `pool` only; it now pins and compares `chain_id` identically, and
   `verify_epoch_against_manifest` additionally checks the epoch header's
   `chain_id`/`pool` against the manifest's (cheap string compares; error
   `CHAIN_MISMATCH`/`POOL_MISMATCH`).
4. **Snapshot header** carries `chain_id`/`pool`, checked at apply (§1.5).
5. **WASM stamp:** `Engine::new(genesis_json)` pins identity; every
   `apply_*` and `load_state` validates against it (§3.2, §3.3).
6. **Browser storage isolation:** the IDB database *name* embeds
   `chain_id:pool` (§4.3) — cross-chain blob confusion is impossible rather
   than detected.

Consumers need no `--network` flag: the feed declares its chain and the
client pins it on first contact (trust-on-first-use for identity, hard
error on any later disagreement — the same pattern the pool pin uses today).

### 6.3 Per-chain decoder maps and genesis

Decoder maps are per-profile (the mainnet v1/v2 map stays the built-in
default; Sepolia's table is the fill-in). Degraded-mode semantics are
chain-independent and already tested (leg i); leg r adds the cross-chain
rejection case. `genesis.json` remains immutable per feed dir; a profile
change under an existing feed dir/DB is a hard INIT error (`meta.chain_id`
mismatch), never a migration.

---

## 7. Implementation order (dependency-ordered; tests first at every step)

Rule carried from the base spec: each item begins by extending the fixture
harness and writing its acceptance leg(s) red, then implementing to green.
No time estimates — order and edges only.

```
0. crates/consumer extraction + crates/wire move        (refactor; suite green throughout)
   └─ CI gate: consumer builds for wasm32
1. A6 chain profiles + identity checks (leg r red → green)      [small, unblocks all fixtures
                                                                 running under a named profile]
2. A1 snapshots: format in strk20-feed → cutter → client cold start
   (legs l, m red → green; leg j extended)
3. A2 SSE on indexerd (leg n red → green)                        [independent of A1]
4. A3 wasm module over crates/consumer (+ fork patch CI, size budget)
   (leg o red → green)                                           [needs 0; A1 for apply_snapshot]
5. Fold-time gate: bench harness + recorded fixture; run; record verdict in §4.4
                                                                 [needs 4]
6. A5 strk20-sync serve (leg q red → green)                      [needs 0.2; independent of 4–5;
                                                                 parallel to them]
7. A4 npm strk20-discovery + TS e2e (leg p red → green)          [needs 2, 3, 4, 5; DelegatedClient
                                                                 half needs 6]
8. Upstream PR for the starknet-providers gate (roadmap item 7)  [parallel, any time; on merge,
                                                                 delete the [patch] per §3.6]
```

Edges in words: the refactor (0) is first because every area sits on it and
it is the only step with no new tests of its own (the existing suite is its
test). Profiles (1) go early so every later fixture/leg runs under an
explicit profile rather than retrofitting one. Snapshots (2) precede wasm (4)
because `apply_snapshot` consumes the format. SSE (3) and serve (6) are
independent islands. The npm package (7) is last because it is the
integration of everything and its e2e is the branch's new headline gate.

---

## 8. New acceptance-test legs (written before their implementations)

Continuing the base spec's §10.3 lettering (a–k exist and stay green):

**l. Snapshot cold start (A1).** Fixture cuts epochs 0–1 (+ snapshot at
epoch 1's `to`). A fresh client with `--cold-start snapshot` syncs; output ==
full-replay client output == O1/O2 golden, field-for-field, including per-note
`block_number`. `history_from` == snapshot block + 1; a history call below it
fails with `HISTORY_UNAVAILABLE`. Tamper sub-legs: flip a byte in the
snapshot file → `HASH_MISMATCH`; corrupt one slot line (valid JSON, wrong
value, fixed-up sha) → `SNAPSHOT_ROOT_MISMATCH`; point `epoch_hash` at a
wrong value → `CHAIN_BROKEN`. Negative: after a forced verify-root failure,
no snapshot file is produced.

**m. Snapshot determinism (extends leg j).** Two independent backfills emit
byte-identical snapshot files (sha256 equality) alongside byte-identical
epochs; a `mirror pull`ed instance's own snapshot equals the origin's.

**n. SSE (A2).** Two concurrent watchers on `/feed/live`: (i) both receive
the connect-time state events and identical subsequent event sequences;
(ii) their captured request bytes are multiset-identical and pass the leg-d
no-key/no-address scanner; (iii) a poke-driven client (fetch-on-event)
reaches the same final mirror state and report as a polling client over the
same fixture timeline, including across the leg-g reorg (reorg = head poke,
no special event); (iv) reconnect with a stale `Last-Event-ID` receives
current state (ignored-header semantics); (v) keepalive comments and
`retry:` present in the raw capture.

**o. WASM conformance (A3).** The wasm module (Node, `wasm-pack --target
nodejs` build of the same crate) is fed the fixture feed's raw bytes
(manifest-driven: snapshot path AND full-epoch path) and its
`discover` report_json == O1/O2 golden — the same pins as legs b and l, one
oracle file. Sealed-state round-trip: second `discover` with the returned
blob does no rediscovery work (report identical, `ckpt_at` advanced only
with the feed). Wrong-key open → `SEALED_STATE_MISMATCH`. Size budget:
gzip ≤ 300 KB asserted in CI. Fork-purity: the §3.6 patch-equality job.

**p. npm e2e (A4).** Per §4.7: KeylessClient against the real spawned
binaries == golden; reload persistence proof (request delta = {manifest,
head} + SSE only); the TS byte-scanner finds no key/address in keyless
capture and DOES find the key in the delegated capture (self-test);
cross-chain IDB isolation (a second fixture chain with a different chain_id
opens a distinct database and rejects nothing because nothing is shared).

**q. Serve mode (A5).** `strk20-sync serve --feed <dir>` colocated with a
live cutter dir: reference-wire `incoming_state` for alice == O1 in reference
JSON with `X-Strk20-Mode: delegated-keyed`; cursors round-trip
request↔response (reference schema, shared with leg h's assertions run
against this second mount); `/feed/live` pokes after the fixture extends the
chain; serve's stdout/stderr/DB pass the leg-f server-side key scan;
`--auth-token-file` rejects a missing/wrong bearer with 401 and no body echo;
history below `events_floor` (snapshot-started serve) → `HISTORY_UNAVAILABLE`.

**r. Chain profiles (A6).** The whole fixture suite runs under a named
synthetic profile (proving no mainnet hardcoding survives); a second fixture
feed with a different `chain_id` + pool: (i) client with a mainnet-pinned
sync.db rejects it with `CHAIN_MISMATCH` before applying anything; (ii) wasm
`Engine` pinned to chain A throws on chain B's epoch; (iii) an epoch file
whose header chain_id disagrees with its manifest is rejected. Unknown class
hash under the synthetic profile still degrades per leg i (mechanism is
chain-independent).

**s. Fold-time gate record (A4, measurement not assertion).** The bench
harness runs in CI on the recorded mainnet fixture and publishes
`t_zstd`/`t_apply` for P1/P2 as build artifacts with an alarm threshold
(fail if P1 median regresses 3× over the recorded baseline) — the gate's
verdict is taken once per §4.4, but the number stays watched so the verdict
can be revisited by measurement.

Compile-fail locks (extending base §10.1, same doctest mechanism): the wasm
crate exposes exactly one key-accepting entry (`discover`/`history`); a
doctest asserts `Engine` has no method accepting both a network-ish type and
a key (trivially true — no network types exist in the crate; the lock is the
`wasm32` build of `crates/consumer` with `#![forbid(unsafe_code)]` and a
dependency-graph test that neither `consumer` nor `client-wasm` links
`reqwest`, `tokio`, or `rusqlite`).

---

## 9. Spec amendment digest

| Base spec location | Change |
|---|---|
| §4.2 tree + §4.4 `snapshot` field | slots-only NDJSON snapshot format + new manifest schema (this doc §1.2–§1.3); SQLite snapshot deleted |
| §4.4 (new) | `snapshots/{block:010}.strk20s.zst` naming, retention 2 |
| §6.1 table | add `GET /feed/live` (SSE poke, this doc §2.2); snapshot row updated |
| §6.4 / R2 context | wire types relocated to `crates/wire` (vendoring + provenance unchanged); compat wire now also served by `strk20-sync serve` (this doc §5) |
| §7.3 `FeedStore` | snapshot cold-start branch (this doc §1.5); `chain_id` pin joins the pool pin |
| §7.4 | sealed-state blobs (browser) use the same reference cursor schema — interop extended to wasm/npm |
| §8 CLI | drop `strk20 snapshot create\|import`; add `--snapshot-keep`, `--network`, `strk20-sync sync --cold-start`, `strk20-sync verify --snapshot`, `strk20-sync serve` (this doc §5.4) |
| §10.3 | new legs l–s (this doc §8) |
| §12.1 (roadmap TOP) | delivered as §§3–4 of this doc; npm name finalized `strk20-discovery` (unscoped), class names `KeylessClient`/`DelegatedClient` per the roadmap |
| §12.2 (global SSE) | delivered as §2, within R3's guardrails |
| §12.5 (snapshot-start) | delivered as §1 |
| §3 crate list | + `crates/consumer`, `crates/wire`, `crates/client-wasm`, `ts/strk20-discovery` |

Everything else in the base spec — wire format v1, hash chain, ingest
pipeline, reorg floors, compat hardening, the trust table, legs a–k — stands
unmodified, and every new surface above inherits its invariants: feed
requests identical for every user, the viewing key never serializable and
never on the wire in keyless mode, epochs immutable below l1_accepted with
only the tail rewriting, upstream `discovery-core` consumed unmodified, and
canonical bytes + the hash chain as the sole identity of the data.
