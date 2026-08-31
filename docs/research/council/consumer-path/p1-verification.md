# Consumer-path addendum — P1 (verification-rigor & privacy lens)

Status: council proposal, 2026-08-30. Extends
[docs/spec/architecture.md](../../../spec/architecture.md) (v1) per
[docs/roadmap.md](../../../roadmap.md). The roadmap's decisions are taken as
given: two blocks with the `FeedTransport` seam; WASM as a pure synchronous
computer; keyless + delegated dual API; no write path; deferred items stay
deferred. Where this addendum amends the base spec, the amended section is
quoted and the replacement text given (§7 of this document consolidates them).
No deadline shaped any choice below; everything is argued from merit.

Design stance of this proposal: **every byte a client trusts must be
checkable, and every new surface must preserve the address-blind and keyless
invariants provably** — by type system where possible, by mechanical test
always. Each area below therefore ends with (i) an explicit trust ladder for
its bytes and (ii) named acceptance legs (§9), written before implementation.

---

## 0. Trust inventory — every byte a consumer will trust after this addendum

| Artifact | Delivery | Integrity check (client-side) | Grounding |
|---|---|---|---|
| `genesis.json` | GET | equality vs built-in chain profile (§A6) | profile pinned in binary / npm package |
| `manifest.json` | GET | binds the epoch chain; divergence check vs locally applied hashes | epoch hash chain + anchors |
| epoch payload | GET | `zst` sha256 → decompress → content sha256 vs manifest → `prev` chain → structural → identity fields (chain_id, pool — new, §A6) | hash chain rooted in genesis + profile; server verify-root at cut; per-epoch anchors |
| `head.ndjson` | GET / SSE-prompted GET | structural + parent linkage + contradiction detection; replaced wholesale | ephemeral by design; superseded by epochs at cut |
| **snapshot** (new, §A1) | GET | content sha256 + basis pin into the epoch chain + **client-side MPT root vs anchor** (slot set: proof-grade) + per-note event cross-check + refold audit (events, write-blocks: audit-grade) | anchor re-checkable against ANY RPC with an **address-blind** request |
| **SSE events** (new, §A2) | stream | none needed — **advisory only**; no trusted state ever rides the stream | n/a by construction |
| **wasm blobs** (new, §A3) | local (IndexedDB) | stamp + self-hash (corruption); Mode R refold (tampering) | IndexedDB trust boundary stated honestly (§A4.5) |
| delegated wire (§A5) | HTTPS to *your own* box | reference-wire conformance; keyed by definition, labeled | self-host framing, loopback default |

Two invariants restated as testable properties, used throughout:

- **P-blind**: the multiset of requests a keyless client emits is a function of
  feed progress only — identical for every key and address. (Extended to the
  SSE stream, the snapshot fetch, and the anchor check.)
- **P-keyless**: no encoding of the viewing key, the address, or any
  key-derived felt (channel keys, cursor material) appears in any request,
  any server-side artifact, any log, or any unencrypted client-side artifact
  other than `sync.db` (0600) — and never at all in wasm mirror blobs, which
  are key-independent by construction.

---

## A1. Snapshots in the cutter + client-verified storage-root anchor

### A1.1 What a snapshot carries — settled by reading the engine

Two facts, verified in the pinned upstream source
(`crates/discovery-core/src`, rev `74841ca`):

1. **Discovery uses only storage slots.** `sync_incoming_state` /
   `sync_outgoing_state` / `preflight_check` never call `RawEventAccess`.
   Note content is decrypted from the **packed storage value**
   (`discovery/notes.rs: decrypt_note(channel_key, token, index, note_id,
   packed, block_number)`), and the note-creation block is the slot's
   `last_update_block` (`discovery/notes.rs:55`, delivered by
   `read_slots_with_block`).
2. **Events feed only history.** `history::fetch_transactions` calls
   `get_block_events(note_block)`, `get_withdrawal_events(addr, …)`,
   `get_viewing_key_set_events(addr, …)` (`privacy_pool/events.rs`,
   `history/transactions.rs`).

Consequences:

- A slots-only snapshot with plain `(slot, value)` pairs is **wrong**: it
  loses `write_block`, which breaks the note `block_number` metadata and the
  10-block maturity rule. Snapshot slot lines are **triples**
  `(slot, value, write_block)`.
- Events must be included, but **not** because discovery needs them — because
  history does, and the alternative (lazy backfill of old epochs when the user
  opens history) is a **privacy violation**: fetching exactly the epochs
  containing the user's note blocks makes the request pattern a function of
  the user's notes, breaking P-blind. Events therefore ship in the snapshot,
  identical bytes for everyone, up front. This is the deciding argument; size
  is secondary (today's full event log is on the order of the slot set).
- A block index is included so `EmittedEvent.block_hash`, timestamps, and
  class history survive cold start.

Byte-size honesty: for the write-once discovery slots a snapshot saves little
over the epoch sum; the genuine and growing savings come from collapsing
overwritten accumulator slots (pool totals change on every deposit) and from
O(1) round trips + a single verification instead of hundreds. "O(1) cold
start" means constant request count and one fold, not magically fewer bytes.

### A1.2 Snapshot wire format v1 (byte-precise, frozen)

File `snapshots/{basis:08}.strk20s.zst` = zstd-19 over canonical NDJSON.
Content identity = sha256 over the **uncompressed** payload (same rule as
epochs). Canonical JSON rules are exactly §4.3's: fixed field order as
written, no whitespace, minimal lowercase `0x` hex, `\n` after every line
including the last. Section order is fixed: header, `sb*`, `ss*`, `se*`, end.

```
line 1 (header):
{"t":"hdr","v":1,"kind":"strk20-snapshot","chain_id":"SN_MAIN","pool":"0x…","basis_epoch":1405,"basis_hash":"<64-hex content hash of epoch basis_epoch>","block":14059999}

one line per pool-active block ≤ block, ascending:
{"t":"sb","b":8978970,"h":"0x…","p":"0x…","ts":1720000000,"rc":"0x…"}
  - "rc" present ONLY on blocks where the pool class changed (preserves class history)

one line per slot with nonzero value as of `block`, ascending by 32-byte BE slot:
{"t":"ss","s":"0x<slot>","v":"0x<value>","w":8978970}
  - "v" = value as of `block`; "w" = last write block ≤ `block` (write_block)
  - zero-valued slots omitted (Cairo map semantics; matches mpt::storage_root)

one line per pool event ≤ block, ascending by (b, i):
{"t":"se","b":8978970,"i":0,"x":2,"tx":"0x…","k":["0x…",…],"d":["0x…",…]}
  - i = event_index, x = tx_index

last line:
{"t":"end","blocks":<n_sb>,"slots":<n_ss>,"events":<n_se>,"class":"0x<pool class as of block>"}
```

`block` is ALWAYS the end block of `basis_epoch`
(`epoch_range(basis_epoch).1`) — snapshot boundaries are epoch boundaries,
hence ≤ l1_accepted, hence the snapshot is **immutable** (a pure function of
epochs `first_epoch..=basis_epoch`). `basis_hash` pins the snapshot into the
epoch hash chain; there is no separate snapshot chain — one spine, derived
leaves.

### A1.3 The fold function is the format (shared, deterministic)

New module `strk20-feed::snapshot` (wasm-clean, no IO):

```rust
pub struct Snapshot { pub header: SnapshotHeader, pub blocks: Vec<SnapBlock>,
                      pub slots: Vec<SnapSlot>, pub events: Vec<SnapEvent>,
                      pub footer: SnapFooter }

/// Fold verified epochs (ascending, contiguous from first_epoch) into a
/// Snapshot. Pure; deterministic; the ONLY definition of snapshot content.
pub fn fold_epochs<'a>(epochs: impl Iterator<Item = &'a codec::Epoch>) -> Result<Snapshot, FeedError>;
/// Fold one more epoch into an existing snapshot (server incremental path,
/// and the client mirror-cache update path). fold_epochs == iterated fold_step.
pub fn fold_step(base: &mut Snapshot, next: &codec::Epoch) -> Result<(), FeedError>;

pub fn encode_snapshot(s: &Snapshot) -> Vec<u8>;          // canonical bytes
pub fn parse_snapshot(payload: &[u8]) -> Result<Snapshot, FeedError>; // structural validation
pub fn verify_snapshot_against_manifest(payload: &[u8], entry: &ManifestSnapshot,
    expect: &FeedIdentity) -> Result<Snapshot, FeedError>;
```

Server (`cutter`) and every client produce/verify snapshots **through this one
module**, so "snapshot correctness" is definitionally "byte equality with a
local fold of the hash-chained epochs" — anyone can audit
(`strk20-sync snapshot audit`, §A1.8), and mirrors get a free cross-check.

### A1.4 The verification ladder (client-side, in order; all mandatory unless marked)

1. **Identity**: manifest `snapshot.hash`, `chain_id`, `pool` vs profile.
2. **Transport**: sha256 of the compressed file == manifest `snapshot.zst`
   BEFORE decompression (the decompressor never touches bytes the manifest
   did not commit to — see §A4.6 for the same rule applied to epochs).
3. **Content**: sha256 of the uncompressed payload == manifest `snapshot.hash`.
4. **Structure**: `parse_snapshot` (sorted sections, counts vs footer,
   `w ≤ block`, every `se.b`/`ss.w` present in `sb`, header identity fields).
5. **Chain pin**: `manifest.epoch(basis_epoch).hash == header.basis_hash`.
6. **MPT anchor (proof-grade, slots)**: recompute
   `feed::mpt::storage_root(&slots)` and compare with
   `manifest.snapshot.anchor.storage_root` AND with
   `contract_leaves_data[0].storage_root` parsed from the fetched sidecar
   `snapshots/{basis:08}.anchor.json` (the full stored `getStorageProof`
   response). This proves the slot **set and values** exactly — a single
   missing, added, or altered slot changes the root.
7. **Anchor grounding (optional, recommended, address-blind)**: if the client
   is configured with any RPC URL (`--verify-anchor <url>` / TS
   `anchorRpcUrl`), fetch `starknet_getStorageProof(block_id = block, pool,
   keys = [])` from it and compare `storage_root` + `block_hash`. The request
   names only the public pool and a public block — **identical for every
   user** — so this check is keyless-compatible and may default ON whenever a
   URL is configured.
8. **Per-note event cross-check (key-local, at discover time)**: for every
   note the engine discovers (from proof-grade slots), assert an
   `EncNoteCreated`/`OpenNoteDeposited` event exists at the note's
   `write_block` carrying its `note_id`; absence ⇒ error
   `SNAPSHOT_EVENT_GAP{block, note_id}`. Runs locally with the key, no
   request — P-blind preserved. This catches targeted event omission against
   *your* notes precisely because the notes themselves are slot-verified.
9. **Full audit (anyone, offline)**: refold all epochs and compare bytes
   (§A1.3). This — not the anchor — is what covers the two fields the MPT
   cannot see: `w` (write blocks) and the event section. The trust grade is
   stated plainly: **slots+values are proof-grade; write-blocks+events are
   audit-grade** (derivable from the hash-chained epochs by anyone, checked
   byte-for-byte by every mirror, spot-checked per-note by every key holder).

A snapshot **without an anchor is never published** (the cutter skips and
retries next cut): an unanchored snapshot would silently downgrade cold-start
trust, and the anchor window (~25–55k blocks) is never a problem at cut time.
`manifest.snapshot.anchor` is therefore non-nullable — unlike per-epoch
anchors, which remain best-effort (R7 unchanged).

### A1.5 Cutter integration

Appended to §5.5's cut sequence, after the manifest rewrite:

- After each cut batch, if `latest_epoch > snapshot.basis_epoch` (or no
  snapshot exists): `fold_step` the new epochs onto the previous snapshot
  (or fold from DB rows — both paths must produce identical bytes,
  test-asserted), `encode_snapshot` → sha256 → zstd-19 →
  `snapshots/{basis:08}.strk20s.zst` tmp+rename; fetch
  `getStorageProof(block, pool, [])`, on success write
  `snapshots/{basis:08}.anchor.json` + manifest `snapshot` entry; on failure
  skip publication entirely (retry at next cut), log loudly.
- Cross-check before publishing: recompute `mpt::storage_root` over the
  snapshot's slot section and compare with the proof — this is verify-root
  re-run over the snapshot's own bytes; mismatch ⇒ do not publish, alarm
  (same posture as §5.6).
- Retention: keep the newest two snapshots (per-basis filenames are
  immutable + cache-forever; the previous one covers the client that read
  the manifest moments before a new cut); delete older files.
- Cadence = epoch cadence (~4.7 h at 10k blocks). No head-coupled writes.

### A1.6 Manifest schema (amendment)

§4.4 currently: `"snapshot":{"block":…,"sha256":…,"bytes":…}` (optional/
nullable) and the dir entry `snapshots/latest.sqlite.zst`. Replaced by:

```json
"snapshot": {
  "basis_epoch": 1405,
  "block": 14059999,
  "hash":  "<64-hex sha256 of uncompressed canonical payload>",
  "zst":   "<64-hex sha256 of the .zst file>",
  "bytes": 123456,
  "file":  "snapshots/00001405.strk20s.zst",
  "anchor": {"block":14059999,"block_hash":"0x…","storage_root":"0x…","class":"0x…"}
}
```

`snapshot` is `null` until the first anchored snapshot exists; when present,
`anchor` is REQUIRED. The SQLite snapshot (`latest.sqlite.zst`) is dropped
from the spec: a SQLite file has nondeterministic page bytes (not
content-stable), is not wasm-parseable, and would be a second, unverifiable
format on the trust path. `strk20 snapshot import` is superseded by the
verified NDJSON cold start below; `mirror pull` keeps covering
server-to-server bootstrap.

### A1.7 Client cold start (Rust and browser)

`FeedTransport` gains two methods (parameterless in the user dimension —
`basis` comes from the manifest, i.e. server-derived; the compile-lock is
extended to them):

```rust
async fn fetch_snapshot(&self, basis: u64) -> Result<Vec<u8>>;         // compressed
async fn fetch_snapshot_anchor(&self, basis: u64) -> Result<Vec<u8>>;  // sidecar JSON
```

`FeedStore::apply_feed` cold-start branch (empty mirror AND
`manifest.snapshot` present AND `--no-snapshot` not given): run ladder steps
1–7; then in one transaction bulk-load `ss` → `storage_log(slot, w, value)`
(as-of reads at any bound ≥ `block` are exact, `write_block` preserved
exactly), `sb` → `blocks` (finality=1), `se` → `events`; set
`last_epoch_applied = basis_epoch`, `last_epoch_hash = basis_hash`,
`snapshot_basis = basis_epoch`. Then the normal path applies epochs
`> basis_epoch` and the tail — `prev`-chain verification of epoch
`basis+1` against `basis_hash` works unchanged, so the snapshot is pinned
into the same chain the incremental path verifies.

New hard rule: `FeedStore::view(bound)` with `bound < snapshot_basis_block`
fails with `BOUND_BELOW_SNAPSHOT` — pre-basis history does not exist locally
and must never be silently answered with zeros. (Engine bounds are always
head or the epoch checkpoint, both ≥ basis; the rule exists to make the
impossible loudly impossible.)

Request count for cold start: genesis + manifest + snapshot + anchor +
(epochs > basis) + head = O(1) in history length, identical for every user
(P-blind). Browser path is the same ladder inside wasm (§A3), TS doing the
fetching.

### A1.8 Mirror-pull interplay and audit commands

- `strk20 mirror pull` continues to pull **epochs** (the canonical spine).
  It then **regenerates** the snapshot locally from its own verified DB
  (deterministic ⇒ byte-identical to origin's) and compares its hash with the
  origin manifest's `snapshot.hash`: equality is a free cross-mirror check;
  mismatch is an alarm naming both hashes (origin divergence), and the mirror
  publishes its own locally-derived snapshot regardless — a mirror never
  re-serves snapshot bytes it did not derive or refold itself.
- `strk20 snapshot create` (deterministic regen from DB), `strk20 snapshot
  verify` (refold + compare against the published file + anchor recheck).
- `strk20-sync snapshot audit --feed <URL|DIR>`: downloads all epochs,
  refolds via `feed::snapshot`, compares bytes to the served snapshot —
  the audit-grade check packaged for anyone.

Acceptance: legs **l, m, n** (§9).

---

## A2. SSE on the indexer

### A2.1 Design decision: notification-only, one global stream

The stream carries **no bytes the client trusts** — only prompts to run the
existing verified fetch path. Argument from merit:

- A second wire format for trusted data would need its own verification,
  its own tamper legs, and its own reorg semantics — reintroducing exactly
  the protocol class R3 deleted. Inlining "diffs" of the tail contradicts the
  wholesale-tail model; inlining the whole tail duplicates a fetch the client
  performs anyway with a 304 guard.
- Notification-only makes the stream **non-load-bearing**: a proxy that
  drops, reorders, duplicates, or corrupts events can delay convergence but
  never corrupt state (polling fallback bounds the delay). Nothing on the
  trust ladder changes.
- Privacy is trivial to prove: the endpoint takes no parameters, and the
  event bytes are identical for every listener.

This satisfies the roadmap's "new-head diffs + epoch-cut events" as: an event
per head change (the diff is what the client then fetches — the replaced
tail), and an event per epoch cut / snapshot publish.

### A2.2 Endpoint and framing

`GET /feed/stream` → `text/event-stream`. No query parameters, no cookies
read, no per-connection state beyond the socket. Response headers:
`Cache-Control: no-cache`, `X-Accel-Buffering: no`. A comment keepalive
(`: ka\n\n`) every 15 s defeats idle proxy timeouts; `retry: 5000` is sent
once at connect.

```
retry: 5000

id: 184467
event: hello
data: {"v":1,"chain_id":"SN_MAIN","pool":"0x…","head":14056430,"head_hash":"0x…","head_etag":"\"<64hex>\"","l1_accepted":14049912,"latest_epoch":1405,"snapshot_basis":1405,"decode_state":"ok"}

id: 184468
event: head
data: {"head":14056431,"head_hash":"0x…","head_etag":"\"<64hex>\"","l1_accepted":14049912,"reorg":false}

id: 184469
event: epoch
data: {"epoch":1406,"hash":"<64hex>","zst":"<64hex>","bytes":12345}

id: 184470
event: snapshot
data: {"basis_epoch":1406,"hash":"<64hex>"}

id: 184471
event: status
data: {"decode_state":"degraded"}
```

`hello` is sent immediately on every connect. `head` fires on every tail
regeneration; `reorg:true` when the regeneration followed a reorg walkback
(advisory — the client's contradiction check in `apply_feed` remains the
mechanism; global chain fact, identical for all). `epoch`/`snapshot` fire at
cut/publish. `status` fires on `decode_state` transitions. `id` is a
per-process monotonic u64 — explicitly **non-durable and non-load-bearing**.

### A2.3 Resume semantics: reconcile, never replay

Because events are notifications, missed history is worthless: on every
(re)connect the client runs one ordinary poll cycle (manifest + conditional
head GET) prompted by `hello`, then rides the stream. The server ignores
`Last-Event-ID` (it may arrive; it carries a coarse "when did you
disconnect" — session-derived, not key- or address-derived; documented).
There is no replay buffer, no per-client cursor, no server-side state —
which is itself a privacy feature: the server cannot be made to remember a
client because the protocol gives it nothing to remember.

### A2.4 Privacy invariant and its test

P-blind extension: the SSE request is parameterless; the event stream bytes
are identical for all concurrent listeners (modulo connect-time `hello` and
`id` numbering); connection lifetime is the only per-client dimension and is
uncorrelated with keys or addresses. Acceptance leg **o** captures two
concurrent clients' full streams and asserts event-sequence identity, plus
the leg-d byte-scanner over the SSE request/response capture.

### A2.5 Fallback to polling (mandatory client behavior)

Rust `--watch` and TS both: try SSE; on connect failure or 2 drops within
60 s → ETag polling at the configured cadence (default 30 s, ±10 % jitter),
re-attempting SSE every 5 min. Both modes MUST produce identical final state
(same fetch path — asserted in leg o). Static mirrors (plain file copies)
have no `/feed/stream`; 404 ⇒ immediate clean fallback, no error surfaced.

### A2.6 Server implementation sketch

`tokio::sync::broadcast::Sender<FeedEvent>` in `AppState`; emit points:
tail regen (ingest §5.2 step 6), epoch cut, snapshot publish, decode-state
flip. Handler: `axum::response::sse::Sse` over the broadcast receiver; on
`Lagged`, emit a fresh `hello` (client reconciles — correctness unaffected).
Rust client: new optional trait, deliberately OUTSIDE `FeedTransport` so the
compile-lock on the trust seam stays byte-identical:

```rust
#[async_trait]
pub trait FeedEvents {                        // impl for HttpTransport only
    async fn subscribe(&self) -> Result<BoxStream<'static, FeedNotice>>;
}
pub enum FeedNotice { Hello{…}, Head{…}, Epoch{…}, Snapshot{…}, Status{…} }
```

Amendment note: R3/§12.2 promoted from roadmap with its constraint intact —
**one global stream, never per-user subscriptions** (the durable-fingerprint
line is policy, not tuning). Acceptance: legs **o, p**.

---

## A3. WASM package of Block B

### A3.1 Crate split

```
crates/client-core   NEW lib  — engine adapter over an in-memory view, feed
                     apply/verify state machine, cursor reopen logic, spent
                     state, discovery orchestration. NO rusqlite, NO tokio,
                     NO reqwest (denied by a lockfile-walk test, leg v).
crates/client        existing — keeps rusqlite FeedStore/ClientView and the
                     binary; re-implemented over client-core where they
                     overlap (conformance suite must stay green through the
                     extraction — the refactor is behavior-frozen).
crates/client-wasm   NEW cdylib — wasm-bindgen facade over client-core +
                     strk20-feed (features: mpt, NO compress).
ts/packages/strk20-discovery   §A4.
```

### A3.2 The in-memory view and the synchronous execution model

```rust
pub struct MemView {
    slots:  BTreeMap<[u8;32], Vec<(u64, Felt)>>,  // slot → ascending (block, value)
    blocks: BTreeMap<u64, BlockMeta>,              // hash, parent, ts, rc
    events: Vec<EventRow>,                          // sorted (block, event_index)
    bound:  u64,
}
impl RawStorageAccess for MemView { /* async fns with zero awaits → Ready */ }
impl RawEventAccess   for MemView { /* per-position filter == MockEventBackend semantics */ }
```

`MemView` holds only plain data ⇒ `Send` is satisfied without `SendWrapper`.
The engine's futures over it are Ready-only; the module drives them with
`FutureExt::now_or_never().expect("engine future pended on in-memory view")` —
a panicking assertion, not a parked thread. Any future dependency that could
actually pend would trip it in the first test run. This is the concrete form
of "WASM is a pure synchronous computer": no executor, no waker, no `await`
crossing the JS boundary.

### A3.3 Exported ABI (wasm-bindgen; exact JS-facing signatures)

```ts
// One class; all inputs are bytes or JSON strings, all outputs JSON strings
// or Uint8Array — the bindgen surface stays minimal and auditable.
export class KeylessEngine {
  constructor(profileJson: string);
  // profileJson: {"chain_id":"SN_MAIN","pool":"0x…","genesis_block":8978970,"epoch_size":10000}

  static fromBlob(blob: Uint8Array, profileJson: string,
                  expectedLastEpochHashHex: string): KeylessEngine;
  // throws EngineError{code:'STALE_BLOB'|'INCOMPATIBLE'|'CORRUPT_BLOB'}

  info(): string;
  // {"head":…,"l1_accepted":…,"last_epoch":…,"last_epoch_hash":"…",
  //  "snapshot_basis":…,"anchored":bool,"tail_generation":…,
  //  "blocks":…,"slots":…,"events":…}

  loadSnapshot(payload: Uint8Array, manifestSnapshotJson: string): void;
  // payload is UNCOMPRESSED canonical bytes (TS decompresses, §A4.6).
  // Runs ladder steps 1,3,4,5 internally (2 is TS's, pre-decompression).

  verifyAnchor(anchorProofJson: string): void;
  // ladder step 6 (+7 when TS fetched the proof from a user-configured RPC):
  // recomputes mpt::storage_root over the loaded slot set, compares against
  // the proof's contract_leaves_data.storage_root. Sets info().anchored.

  applyEpoch(payload: Uint8Array, expectedHashHex: string): void;
  // UNCOMPRESSED bytes; verifies sha256 == expectedHash (manifest binding),
  // prev == internal last_epoch_hash (chain), identity fields, range
  // contiguity (RANGE_GAP otherwise); supersedes rows in range.

  applyHead(payload: Uint8Array): boolean;   // true = tail replaced (contradiction)
  exportMirror(): Uint8Array;                // §A3.4 mirror blob; call at epoch cadence

  discover(ownerHex: string, viewingKeyBe32: Uint8Array,
           stateBlob: Uint8Array | null, entropy32: Uint8Array):
      { reportJson: string, state: Uint8Array };
  // Runs the full two-pass native sync (checkpoint pass at the epoch floor,
  // live pass at head, cursor reopen, generation-based rewind, note-registry
  // prune, spent refresh, the A1.4-step-8 event cross-check) INSIDE the
  // module — TS holds no discovery logic on the trust path. reportJson is
  // byte-compatible with `strk20-sync sync --json`'s SyncReport.

  history(ownerHex: string, viewingKeyBe32: Uint8Array,
          stateBlob: Uint8Array, historyCursorJson: string | null): string;

  free(): void;  // zeroizes key material and drops the instance
}
```

Key handling: the 32-byte BE key enters wasm linear memory, is copied into
`SecretFelt` (zeroize-on-drop) and the staging buffer is zeroized. Honest
limit, documented verbatim in the npm README: JS cannot reliably zeroize its
own `Uint8Array`, and structured-clone copies to a Worker are outside our
control — the guarantee is "the module never writes the key anywhere and
zeroizes what it owns", not "the key never existed in JS memory" (matches
§12.1's honest-zeroization stance).

### A3.4 Blob formats, versioning, compatibility stamp

**Mirror blob** (`exportMirror`) — key-independent public chain data:

```
offset 0   8 bytes   magic "STRK20MB"
offset 8   u16 LE    blob_version = 1
offset 10  u32 LE    stamp_len
offset 14  stamp     UTF-8 JSON:
  {"blob_v":1,"format_v":1,"chain_id":"SN_MAIN","pool":"0x…",
   "engine_semver":"0.3.1","last_epoch":1405,"last_epoch_hash":"<64hex>",
   "snapshot_basis":1405,"tail_generation":7,"payload_sha256":"<64hex>"}
then       payload   canonical NDJSON in the strk20-snapshot grammar (§A1.2)
                     with kind "strk20-mirror", basis = last applied epoch
```

The payload **reuses the snapshot grammar** — one canonical fold format for
server snapshots and client mirror caches; `fromBlob` re-runs the same
structural validation plus `payload_sha256` (corruption detection; tampering
detection is Mode R's job, §A4.5). The tail is NEVER in the blob (§7 of the
notes: only epoch-derived state is persisted; the tail is refetched each
load — which is why the browser client still needs no reorg logic).
Compatibility: mismatch on `blob_v`/`format_v`/`chain_id`/`pool`/engine
major ⇒ `STALE_BLOB` (TS falls back to cold start; never a silent accept).
Uncompressed; TS compresses with native `CompressionStream('gzip')` before
IndexedDB (§A4), keeping zstd and all IO out of wasm.

**Discovery-state blob** (`discover().state`) — key-derived, therefore
**AEAD-encrypted inside the module**: XChaCha20-Poly1305 (pure-Rust
`chacha20poly1305`, wasm-clean); key = HKDF-SHA256(ikm = viewing-key bytes,
salt = `"strk20-discovery/state/v1"`, info = chain_id ‖ pool ‖ owner);
nonce = first 24 bytes of HKDF(entropy32 ‖ u64 counter) — entropy is passed
IN from `crypto.getRandomValues` so the module stays deterministic-given-
inputs and needs no `getrandom` import. Plaintext = incoming/outgoing
cursors (reference JSON schema — interop preserved), note registry rows,
owner generation. AAD = the stamp fields. On a shared machine the blob is
noise without the key, closing the §7 registry-fingerprint concern by
construction rather than by documentation.

### A3.5 Error model

Every method throws `EngineError { code, message, context }`; `message` and
`context` are asserted key-clean by the scanner (leg q). Codes (closed set):

```
INCOMPATIBLE      profile/stamp identity mismatch (chain_id, pool, versions)
STALE_BLOB        blob is valid but superseded (hash/basis mismatch vs manifest)
CORRUPT_BLOB      magic/self-hash/structure failure
HASH_MISMATCH     content sha256 != expected        (names epoch/snapshot + both hashes)
CHAIN_BROKEN      prev linkage failure               (names epoch + both hashes)
MALFORMED         structural/grammar failure
RANGE_GAP         epoch applied out of order
ANCHOR_MISMATCH   recomputed MPT root != proof root
SNAPSHOT_EVENT_GAP  discovered note has no creation event (ladder step 8)
BOUND_BELOW_SNAPSHOT
BUDGET_EXCEEDED   discovery did not complete within pass budget
INTERNAL          bug (panic hook maps to this; message scrubbed)
```

`FeedError` variants map 1:1 onto the first seven.

### A3.6 Managing the discovery-core patch until the upstream PR lands

- Fork `starknet-privacy` under our org; branch
  `strk20/providers-gate-74841ca` = the pinned rev + exactly ONE commit
  gating `starknet-providers` behind a default-on `providers` feature
  (`optional = true` + `[features] default = ["providers"]`,
  `providers = ["dep:starknet-providers"]`) — the two-line change the spike
  identified, i.e. the upstream PR's exact content (roadmap item 7).
- Workspace consumes it via
  `[patch."https://github.com/starkware-libs/starknet-privacy.git"]`;
  native builds keep default features (identical behavior);
  `crates/client-wasm` sets `default-features = false`.
- **The "consumed UNMODIFIED" invariant stays mechanical**: the patch is
  committed to this repo as `patches/discovery-core-providers-gate.patch`,
  and a CI job asserts (a) fork rev == upstream rev + that patch and
  (b) `git diff upstream..fork -- crates/discovery-core/src` is EMPTY —
  Cargo metadata only, zero source lines. When upstream merges, the patch
  section is deleted and the CI job flips to asserting the fork is retired.

### A3.7 Purity and size gates (CI)

- **Purity lock** (the wasm analog of "links no server code"): a test walks
  the lockfile and fails if `crates/client-core` or `crates/client-wasm`
  reach `tokio`, `reqwest`, `rusqlite`, `web-sys` network/storage features,
  or `getrandom`; plus a `wasm-objdump`-based check that the module's import
  section contains nothing beyond wasm-bindgen glue (no `fetch`, no
  IndexedDB, no timers). The module cannot leak what it cannot call.
- **Size gate**: gzip size of the release module ≤ 320 KB (spike baseline
  231 KB + codec/mpt/AEAD headroom); regression = red CI.

Acceptance: legs **q, v**.

---

## A4. npm package

### A4.1 Naming and layout

Unscoped **`strk20-discovery`** (per the notes: no dependency on someone
else's npm org, no implication of officialdom; §12.1's provisional
`@strk20/discovery-provider` is amended accordingly). Contents: ESM +
`.d.ts`; `strk20-discovery/wasm` asset entry (bundler-friendly) and
`strk20-discovery/inline` (base64, zero-config) — both carrying the same
module, whose sha256 is printed in the README and verified by a postinstall-
free integrity test in CI (no install scripts — supply-chain posture).
Publishing: GitHub Actions with npm provenance; `files` whitelist; no
runtime dependencies except `fzstd` (§A4.6).

### A4.2 One interface (exact)

```ts
export interface KeyRef { address: `0x${string}`; viewingKey: Uint8Array }  // 32-byte BE

export type DiscoveryEvent =
  | { type: 'note';  note: Note }
  | { type: 'spent'; nullifier: `0x${string}`; noteId: `0x${string}` }
  | { type: 'reorg' }                          // rewound + resynced (informational)
  | { type: 'head';  head: number; l1Accepted: number }
  | { type: 'degraded' };

export interface DiscoveryClient {
  getNotes(k: KeyRef): Promise<SyncReport>;    // SyncReport == strk20-sync --json shape
  subscribe(k: KeyRef, on: (e: DiscoveryEvent) => void): () => void;  // returns unsubscribe
  history(k: KeyRef, cursor?: HistoryCursor): Promise<HistoryPage>;
  status(): Promise<FeedStatus>;               // head, l1, lastEpoch, anchored, decodeState
  close(): Promise<void>;
}

export class KeylessClient implements DiscoveryClient {
  constructor(opts: {
    feedUrl: string;
    network?: 'mainnet' | 'sepolia' | ChainProfile;   // default 'mainnet'
    storage?: 'indexeddb' | 'memory';                  // default 'indexeddb'
    persist?: 'auto' | 'raw-epochs' | 'mirror-cache';  // §A4.5; default 'auto'
    anchorRpcUrl?: string;                             // enables ladder step 7
    worker?: boolean;                                  // default true
  });
}

export class DelegatedClient implements DiscoveryClient {
  constructor(opts: { serverUrl: string; authToken?: string });
  // speaks EXACTLY the reference /v1/sync/* + /v1/history wire — one
  // protocol for both `strk20 --enable-compat` and `strk20-sync serve`;
  // subscribe() probes POST /v1/watch (404 ⇒ ETag-style polling fallback).
}

// SDK drop-in (base spec §12.1, adopted): wraps KeylessClient.
export class LocalDiscoveryProvider implements DiscoveryProviderInterface {
  constructor(opts: ConstructorParameters<typeof KeylessClient>[0]);
  // discoverNotes / discoverChannels / discoverRequirement + fetchHistory,
  // reusing the SDK's cursor conversion semantics so NotesCursor/
  // ChannelCursor round-trip identically to IndexerDiscoveryProvider.
}
```

`KeylessClient` runs the wasm module in a dedicated Worker by default
(discovery is CPU-bound; the main thread never blocks); the key is
transferred, used, and zeroized module-side per §A3.3's honest limits.

### A4.3 IndexedDB layout

DB name `strk20-discovery::<chain_id>::<pool>`, version 1:

| store | key | value |
|---|---|---|
| `meta` | string | stamp JSON, `headEtag`, persist decision, schema version |
| `snapshot` | `'base'` | `{basisEpoch, hash, zst: ArrayBuffer, anchorJson}` (compressed as fetched) |
| `epochs` | epoch idx (number) | `{idx, hash, zst: ArrayBuffer}` — Mode R store |
| `mirror` | `'cache'` | `{stamp…, gz: ArrayBuffer}` (mirror blob, native-gzip) — Mode M store |
| `state` | keyId (hex string) | `{ct: ArrayBuffer}` — the AEAD discovery-state blob |

`keyId = hex(HKDF(viewingKey, salt="strk20-discovery/state-id/v1",
info=chain‖pool‖owner))[0..32]` — the row key itself reveals nothing on a
shared machine. Documented residual metadata: row existence, sizes, mtimes.
Everything except `state` is public chain data; `state` is noise without the
key (§A3.4). Every IndexedDB read/write is best-effort: a missing or
unreadable store degrades to network cold start, never to an error.

### A4.5 Persistence: both modes designed; the measurement gate picks

**Mode R — raw epochs (integrity-maximal).** Store compressed snapshot +
compressed epochs exactly as fetched. Every page load: fresh manifest →
re-run the FULL verification ladder over stored bytes (zst hash, content
hash, chain, structure; MPT anchor once per basis) → fold in the Worker →
apply tail. A tampered or corrupted IndexedDB row fails its hash and is
refetched — **local storage is never trusted, only network-equivalent
bytes re-verified per load**. Cost: T_fold per load.

**Mode M — folded mirror cache (latency-maximal).** Additionally store
`exportMirror()` after each newly applied epoch (epoch cadence — never per
head poll, per the §7 note on `export()` cadence). Load: `fromBlob` with
`expectedLastEpochHashHex` from the FRESH manifest (the cache key is the
chain-head hash, exactly the §7 proposal); any mismatch/corruption ⇒ silent
fall through to Mode R stores, then network. Honest trust statement, stated
in the docs verbatim: Mode M trusts IndexedDB integrity between loads; a
same-origin attacker (or any process writing the profile directory) can
alter folded values undetected until the next full refold. The marginal risk
over Mode R is precisely *persistence of tampering beyond the tampering
code's presence*. Mitigation is architectural, not cryptographic (no secret
exists to MAC the key-independent blob with): a background full refold +
byte-compare runs opportunistically (idle callback) every N loads, flagging
divergence.

**The gate (pre-registered, run BEFORE any TS client code is written).**
Harness: `ts/bench/fold-gate/` — Node + Playwright/Chromium driving the real
`client-wasm` module over (a) the real mainnet feed at a pinned manifest
hash, (b) synthetic 2x and 10x histories generated by
`strk20 bench synth-feed --scale N` (epochs replicated with re-chained
hashes). Measured on Chromium with 4x CPU throttle (mid-tier device proxy)
and, if available, one physical mid-range Android phone: p95 of
T_verify_fold (ladder + fold, cold) and T_mirror_load (fromBlob). Decision
rule:

- p95 T_verify_fold(1x) ≤ 500 ms → **Mode R only**; Mode M is not built and
  a whole layer disappears (the notes' ~200 ms branch).
- p95 T_verify_fold(1x) > 2 s → **Mode M default**, Mode R behind
  `persist:'raw-epochs'` (the paranoid option is never removed).
- Between → both built; default `'auto'` measures the device's first real
  fold and persists Mode M iff it exceeded 1 s.

Results published to `docs/research/fold-gate-results.md` (numbers, device
profiles, decision) before `KeylessClient` implementation starts. The 10x
row does not change the shipped default but is recorded as the re-trigger
threshold: if live history growth crosses the measured >2 s point, Mode M
graduates per the rule above.

### A4.6 zstd in TypeScript + verify-before-decompress (amendment)

Decompressor: **`fzstd`** — pure-TS, decompress-only (~8 KB), no second wasm
module, auditable. Merit: we never compress on the client (the mirror cache
uses native `CompressionStream('gzip')`), and a pure-JS decoder is a smaller
supply-chain and memory-safety surface than a wasm zstd build.

Hardening rule, applied to BOTH clients and added to the spec: **verify the
manifest's `zst` sha256 BEFORE decompression** (the decompressor only ever
sees bytes the manifest committed to), and cap decompressed output
(64 MiB epochs / 256 MiB snapshot, streaming). Today's Rust client
decompresses first — §7.3's flow is amended (see §7 below); defense in depth
for the C zstd there, load-bearing for the TS decoder here.

### A4.7 SSE consumption

`subscribe()` opens `EventSource('/feed/stream')`; on `head` → conditional
GET `head.ndjson` → Worker `applyHead` → `discover` pass → emit events
diffed against the previous report (new notes, newly spent). Fallback and
retry cadence exactly as §A2.5; `document.visibilityState === 'hidden'`
pauses polling (SSE stays open; browsers throttle it naturally).

### A4.8 TS e2e against the real server binary

`crates/e2e-tests` gains a `fixture-rpc` **binary target** exposing the
existing in-process fixture RPC (same deterministic chain, partition
{10,20,30}+31), so non-Rust harnesses can spawn it. `pnpm e2e`
(CI job after `cargo build --release`):

1. spawn `fixture-rpc`, spawn real `strk20 run` against it (feed + SSE),
   spawn a TS recording proxy (byte capture, same role as the Rust one);
2. run `KeylessClient` in Node (fake-indexeddb + fetch-SSE polyfill) AND in
   headless Chromium (Playwright, real IndexedDB/EventSource/Worker) through
   the proxy;
3. assert: report deep-equals the vendored O2 golden pins AND deep-equals
   the JSON of a native `strk20-sync sync --json` run against the same feed
   (Rust/TS report-shape lockstep);
4. request capture: GETs only, URL multiset ⊆ feed paths incl. snapshot +
   `/feed/stream`, and the leg-d byte-scanner (ported to TS, with its own
   self-test against a DelegatedClient capture where the scanner MUST find
   the key);
5. persistence: reload the Chromium context → converged report with a
   request-multiset delta of {manifest, head} only (Mode R and, if built,
   Mode M variants).

Acceptance: legs **r** (and **s** for `DelegatedClient`).

---

## A5. `strk20-sync serve` — the delegated self-host surface

### A5.2 Purpose vs compat mode

| | `strk20 --enable-compat` | `strk20-sync serve` |
|---|---|---|
| host binary | indexer (Block A box) | client (Block B only) |
| needs Block A locally | yes (it IS the indexer) | **no** — feeds off any public feed URL/dir/db |
| engine runs over | server `strk20.db` bridge | client's verified `sync.db` mirror |
| wire | reference `/v1/sync/*` + `/v1/history` | **the same wire**, + keyed watch/SSE |
| key exposure | per request, memory only | per request/watch, memory only |

One protocol for `DelegatedClient`; `serve` exists for self-hosters who run
no indexer at all — the mirror it serves from is verified through the full
client ladder (hash chain, snapshot anchor), so a delegated user inherits
the keyless trust story for the DATA even though they surrendered the key to
their own box.

### A5.3 Endpoints

```
strk20-sync serve --feed <URL|DIR|db:PATH> [--listen 127.0.0.1:8420]
                  [--auth-token-file <path>] [--cors-origin <origin>]
                  [--poll <secs>] [--network <name>] [--db <sync.db>]
```

- `GET  /health` → `{"status":…,"chain_head":{"block_number","block_hash","timestamp"},"lag_secs":…,"mode":"delegated"}`
- `POST /v1/sync/incoming_state | outgoing_state | preflight_check`,
  `POST /v1/history` — byte-exact reference wire (same vendored `compat/wire.rs`
  types, same engine, same 409 `BLOCK_REORGED` rule driven by the mirror's
  tail-generation rewind). Cursor interop with compat mode and the keyless
  client is inherited (§7.4 of the base spec).
- `POST /v1/watch` body `{contract_address, viewing_key, recipient_address}`
  → `{"watch_token":"<random 256-bit hex>"}`. Key → `SecretFelt`, RAM only,
  zeroized on unwatch/shutdown; token is a pure capability, dies with the
  process.
- `GET  /v1/subscribe` with `Authorization: Bearer <watch_token>` → SSE
  (fetch-based SSE client-side — native `EventSource` cannot send headers,
  and the key or token must NEVER ride a URL where proxies and logs see it):

```
event: note
data: {"token":"0x…","index":3,"note_id":"0x…","amount":"1000","block_number":14056440,"sender":"0x…"}
event: spent
data: {"nullifier":"0x…","note_id":"0x…","block":14056441}
event: reorg
data: {"rewound_to_epoch_floor":14049999}
event: head
data: {"head":14056442,"l1_accepted":14049930}
```

Per-user push is permitted HERE and only here: the durable-fingerprint
hazard doctrine bans it on the public indexer; on a keyed surface the
operator already holds the key — push adds zero marginal exposure, and the
policy line ("never per-user push on the keyless feed") is unchanged.

### A5.4 The third `FeedTransport` impl: `DbTransport`

`--feed db:/path/strk20.db` opens the INDEXER's database read-only (SQLite
`mode=ro`, WAL concurrent reader) for colocated deployments with no feed dir
and no HTTP hop. It synthesizes the transport surface from rows:

```rust
pub struct DbTransport { conn: Connection /* read-only */ }
impl FeedTransport for DbTransport {
    // fetch_manifest: epochs table + meta rows → Manifest (incl. snapshot entry)
    // fetch_epoch(idx): rows → BlockLine* → codec::encode_epoch → VERIFY
    //   sha256(payload) == epochs.content_hash (stored at cut) — synthesis
    //   drift dies HERE, at the source, before any client sees it — then
    //   compress and return.
    // fetch_head: rows above the epoch floor + meta → codec::encode_head.
    // fetch_snapshot/anchor: feed::snapshot::fold from rows / stored sidecar.
}
```

The self-verification line is the design's safety: the schema knowledge this
duplicates from `indexerd` is checked against the stored content hash on
every single fetch, and leg **t** additionally asserts byte-equality against
the cutter's on-disk files. The client-side verification pipeline downstream
is completely unchanged — in-process bytes get no shortcut through the
ladder.

### A5.5 Security posture

- **Loopback by default; non-loopback `--listen` without
  `--auth-token-file` is a refusal, not a warning** (this surface is keyed).
- Every response: `X-Strk20-Mode: delegated-keyed`.
- Compat mode's hardening is inherited verbatim and hard-coded: request/
  response bodies never logged; cursors never logged or persisted
  server-side; pubkey/watch state memory-only; malformed bodies rejected
  without echo.
- CORS closed by default; `--cors-origin` is an explicit allowlist.
- TLS is out of scope (reverse-proxy guidance in ops docs), matching the
  self-host framing.

Acceptance: legs **s, t**.

---

## A6. Chain profiles

### A6.1 Mechanism: one profile source of truth, consumed by Rust and TS

New top-level `profiles/` directory — JSON, embedded via `include_str!` in
Rust and imported by the TS package; a test asserts the Rust built-ins equal
the JSON files (single source, no drift):

```json
// profiles/mainnet.json
{
  "name": "mainnet",
  "chain_id": "SN_MAIN",
  "pool": "0x040337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a",
  "genesis_block": 8978970,
  "epoch_size": 10000,
  "decoder_map": {
    "0x30b8c540cf04d8ef0f4db2a9098d9cc0e35e83af1cb3325f5a4f40144b4b30b": "v1",
    "0x67dddd89d80fedadc06b6f160798f94800a4a70164e5a24301cd0d6076b554d": "v2"
  },
  "rpc": { "primary": "https://rpc.starknet.lava.build",
           "fallback": "https://starknet.publicnode.com" }
}

// profiles/sepolia.json   — MECHANISM SHIPS NOW, verified values are a fill-in
{
  "name": "sepolia",
  "chain_id": "SN_SEPOLIA",
  "pool": "0x0254a6b2997ef52e9f830ce1f543f6b29768295e8d17e2267d672c552cfe0d91",
  "genesis_block": null,          // FILL-IN: parallel research task
  "epoch_size": 10000,            // deliberately identical to mainnet — no merit in divergence
  "decoder_map": {},              // FILL-IN: verified Sepolia class table (research task)
  "rpc": { "primary": null, "fallback": null }   // FILL-IN
}
```

`ChainConfig` grows into `ChainProfile` (adds `name`, `rpc`); the loader
REFUSES a profile containing nulls unless `--allow-incomplete-profile`
(dev-only escape hatch) — a half-filled Sepolia can never silently run.
Selection: `--network <name>` (both binaries, TS `network` option) picks a
built-in; `--profile <path.json>` loads a custom one; explicit flags
(`--rpc-url` etc.) override fields. Default data paths become
network-scoped (`…/strk20/<name>/strk20.db`, feed dir likewise) so two
networks cannot share state by accident.

### A6.2 Chain-id stamping matrix — who checks what, where

| Artifact | Carries | Checked by | Status |
|---|---|---|---|
| RPC | `starknet_chainId` | server INIT vs profile | exists (§5.1) |
| `genesis.json` | chain_id, pool, epoch_size, genesis_block | client vs profile AND vs stored meta | **amend**: `FeedStore` today checks only `pool` — check all four, every sync |
| epoch header | chain_id, pool | `verify_epoch_against_manifest` | **amend feed crate**: new param `expect: &FeedIdentity{chain_id, pool}`; today header identity is unchecked |
| head header | — | — | **amend grammar (additive)**: hdr gains `"chain_id","pool"` after `"kind"`; decoding ignores unknown fields, so v stays 1; client checks them |
| snapshot header | chain_id, pool | ladder step 1/4 | new (§A1) |
| manifest | chain_id, pool, epoch_size, genesis_block | client vs genesis every sync | **amend**: add the cross-check |
| SSE `hello` | chain_id, pool | watch loop before first refetch | new (§A2) — a proxy pointed at the wrong network dies before any state mutation |
| wasm blobs | stamp: chain_id, pool | `fromBlob` / `constructor` | new (§A3) |
| IndexedDB | DB name scoped `::<chain_id>::<pool>` + stamp | wrapper | new (§A4) |
| `sync.db` | meta rows | every sync | amended with genesis check above |

Failure is uniformly `CHAIN_MISMATCH` (Rust: named `bail!`; wasm:
`INCOMPATIBLE`), always BEFORE any state mutation. The e2e fixture gets
`chain_id: "SN_TEST"` in its test profile so cross-network rejection is
exercised against real wire bytes (leg u).

Per-chain decoder maps ride the profile (mechanism already in place via
`decoder_map`; degraded-mode semantics of §5.7 are unchanged and
per-profile). The verified Sepolia class table drops into
`profiles/sepolia.json` when the research task lands — no code change.

---

## §7. Consolidated base-spec amendments (quote → new text)

1. **§4.2 feed directory** — line `snapshots/latest.sqlite.zst  # optional
   convenience; epochs are canon` becomes:
   ```
   snapshots/{basis:08}.strk20s.zst    # immutable, content-addressed fold (addendum A1.2)
   snapshots/{basis:08}.anchor.json    # full getStorageProof at the basis block; REQUIRED for publication
   ```
2. **§4.4 manifest `snapshot` field** — replaced by the A1.6 schema
   (anchor required when snapshot present).
3. **§4.4 `head.ndjson` hdr** — `{"t":"hdr","v":1,"kind":"strk20-head",…}`
   gains `"chain_id":"…","pool":"0x…"` immediately after `"kind"` (additive;
   parsers ignore unknown fields; ETag changes once).
4. **§6.1 feed table** — three new rows:
   `GET /feed/snapshots/{basis:08}.strk20s.zst` (immutable),
   `GET /feed/snapshots/{basis:08}.anchor.json` (immutable),
   `GET /feed/stream` (SSE; `no-cache`; "No feed route takes any parameter
   derived from a user" now explicitly includes the stream).
5. **§12.2 (roadmap: global SSE)** — promoted into scope per A2, constraint
   preserved verbatim: one global stream, never per-user subscriptions.
6. **§7.2 `FeedTransport`** — trait gains `fetch_snapshot(basis)` /
   `fetch_snapshot_anchor(basis)` (indices are server-derived like
   `fetch_epoch`'s; the compile-fail lock is extended to the new methods);
   impls now: `HttpTransport`, `DirTransport`, `DbTransport` (A5.4).
   Subscription lives in a SEPARATE `FeedEvents` trait — the privacy-locked
   trait stays minimal.
7. **§7.3 sync flow** — "decompress + apply" becomes "verify manifest `zst`
   sha256 over the compressed bytes, THEN decompress (bounded), THEN verify
   content sha256" (A4.6); cold start may begin from a verified snapshot
   (A1.7) with `last_epoch_applied/hash` seeded from its basis.
8. **§5.5 cutter** — snapshot fold/anchor/publish step appended per A1.5.
9. **§8 CLI** — `strk20 snapshot create|verify` (replaces `import`),
   `strk20 bench synth-feed --scale N`; `strk20-sync snapshot audit`,
   `strk20-sync serve …` (A5.3), `--network/--profile` on both binaries,
   `--no-snapshot`, `--verify-anchor <url>` on `strk20-sync sync`.
10. **§3 crate layout** — add `crates/client-core`, `crates/client-wasm`,
    `ts/packages/strk20-discovery`, `profiles/`,
    `patches/discovery-core-providers-gate.patch`; e2e-tests gains bin
    `fixture-rpc`.
11. **§9 trust table** — add rows: snapshot (proof-grade slots /
    audit-grade events, per A1.4), SSE (advisory-only), serve
    (`delegated-keyed`, loopback default), wasm blobs (Mode R/M statement).
12. **§12.1 npm naming** — `@strk20/discovery-provider` → unscoped
    `strk20-discovery`; `LocalDiscoveryProvider` ships inside it (A4.2).

---

## §8. Implementation order (dependency-ordered; tests written first — every
item's first commit is its RED acceptance leg + unit vectors)

```
0. Fork + patch + CI unmodified-guard (A3.6); open the upstream PR.        [S; unblocks 4]
1. Profiles + identity stamping (A6): profiles/, ChainProfile, feed-crate
   identity checks (epoch hdr param, head hdr fields, manifest↔genesis),
   FeedStore meta checks, zst-before-decompress. Leg u RED→GREEN.          [S; everything stamps through it]
2. Snapshots (A1): feed::snapshot fold/encode/parse + golden byte vectors;
   cutter integration + manifest + retention; Rust cold start + audit CLI;
   mirror-pull regen+compare. Legs l, m, n.                                [M; needs 1]
3. SSE (A2): broadcast + /feed/stream; FeedEvents on HttpTransport; --watch
   consumption + fallback. Legs o, p.                                      [M; needs 1; parallel with 2]
4. WASM (A3): client-core extraction (behavior-frozen; conformance suite
   stays green), MemView, client-wasm ABI + blobs + error model; node
   conformance harness; purity + size gates. Legs q, v.                    [M–L; needs 0, 1, 2]
5. Fold-time gate (A4.5): bench harness over the real feed + synth scales;
   publish docs/research/fold-gate-results.md; decide Mode R/M.            [S; needs 2, 4]
6. npm (A4): TS wrapper per gate decision, IndexedDB, fzstd, SSE
   consumption, DelegatedClient, LocalDiscoveryProvider; fixture-rpc bin;
   TS e2e harness + scanner port + self-test. Legs r (+s client half).     [L; needs 3, 4, 5]
7. serve (A5): DbTransport + byte-equality self-check; keyed wire reusing
   compat wire types; watch/subscribe; posture locks. Legs s, t.           [M; needs 1–3; parallel with 4–6]
8. Sepolia fill-in (A6): drop verified values into profiles/sepolia.json
   when the research task lands; sepolia nightly smoke.                    [S; anytime after 1]
```

Critical path: 1 → 2 → 4 → 5 → 6. Items 3 and 7 run parallel to it.

---

## §9. New acceptance-test legs (extending §10.3 a–k; same harness: real
binaries, recording proxy, fixture RPC, dual oracle; scanner = leg d's
byte-scanner incl. channel-key encodings unless stated)

- **l. Snapshot cold start + anchor (A1).** Fixture with ≥2 cut epochs;
  cutter publishes an anchored snapshot. Fresh `strk20-sync` cold-starts
  from it: report == O1 == full-replay client's report; proxy capture shows
  NO epoch ≤ basis fetched (multiset = {genesis, manifest, snapshot, anchor,
  epochs>basis, head}); MPT root check asserted executed (fails leg if
  skipped). Tamper: flip one byte in the served snapshot → named
  `HASH_MISMATCH`; corrupt one `ss` value with fixed-up content hash →
  `ANCHOR_MISMATCH`; `--no-snapshot` replay still green. Per-note
  `block_number` == committed partition blocks ({10,20,30}+31) THROUGH the
  snapshot path (the write-block-preservation proof).
- **m. Snapshot determinism + fold equality (A1).** Cutter snapshot bytes ==
  `feed::snapshot::fold_epochs` over the verified epoch files (byte
  equality) == second backfill's snapshot (extends leg j); `strk20-sync
  snapshot audit` passes; `fold_step` iterated == `fold_epochs` (property).
- **n. Event-gap detection (A1.4 step 8).** Serve a snapshot with alice's
  note-creation event removed but slots intact (passes the MPT check by
  construction): alice's sync raises `SNAPSHOT_EVENT_GAP` naming block +
  note_id; bob's sync is unaffected; discovery equality vs O1 still holds
  for both (documents exactly what the anchor does and does not cover).
- **o. SSE liveness, equality, blindness, fallback (A2).** Two concurrent
  `--watch` clients on `/feed/stream`; fixture extends the chain; both
  converge to extended O1; their event sequences are identical; scanner over
  the full SSE request/response capture finds nothing; proxy kills the
  stream → client falls back to polling and reaches the identical end state
  (SSE-vs-polling state equality).
- **p. SSE reorg (A2).** Leg-g fork replayed under SSE: `head` event with
  `reorg:true`, client rewind per §7.5 unchanged, state == post-fork oracle;
  epoch files byte-untouched.
- **q. WASM conformance (A3).** The module under Node fed the same feed
  bytes as leg b: `discover` report JSON deep-equals the native
  `--json` report and O2 pins; mirror-blob export→`fromBlob` roundtrip
  yields identical reports; stale/foreign/corrupt blobs → `STALE_BLOB` /
  `INCOMPATIBLE` / `CORRUPT_BLOB`; scanner over all thrown error strings and
  the exported mirror blob (key-independence of the blob, mechanically).
- **r. TS e2e keyless (A4.8).** As specified there: Node + Chromium against
  real `strk20` + `fixture-rpc` through a TS recording proxy; golden-pin and
  Rust-report equality; GET-only + scanner (TS port) + scanner self-test
  against a delegated capture; reload → request delta {manifest, head} only.
- **s. Delegated / serve (A5).** `strk20-sync serve --feed db:…` and
  `--feed http…`: `DelegatedClient` notes == O1; wire conformance fixtures
  replayed (same vendored shapes as compat); forked `last_known_block` →
  409 `BLOCK_REORGED`; watch/subscribe delivers `note` then `spent` (block-48
  leg replayed through SSE); non-loopback bind without token refused; every
  response carries `X-Strk20-Mode: delegated-keyed`; scanner over serve's
  stdout/stderr + on-disk artifacts (key legitimately in RAM, never at
  rest).
- **t. DbTransport byte-equality (A5.4).** For every cut epoch:
  decompressed `DbTransport::fetch_epoch` payload == on-disk epoch payload
  byte-for-byte; synthesized manifest/head verify through the unchanged
  client ladder; a deliberately corrupted DB row → the transport itself
  errors on its content-hash self-check (never serves divergent bytes).
- **u. Chain-profile guards (A6).** Fixture profile `SN_TEST` stamps through
  genesis/manifest/epoch-hdr/head-hdr/snapshot/SSE-hello (asserted present);
  a mirror synced under one profile pointed at a feed with another chain_id
  or pool → `CHAIN_MISMATCH` BEFORE any DB write (asserted by pre/post DB
  snapshot equality); same rejection from `fromBlob` and the IndexedDB
  stamp; incomplete sepolia profile refused without the dev flag.
- **v. WASM purity + size (A3.7).** Lockfile-walk: client-core/client-wasm
  reach no tokio/reqwest/rusqlite/getrandom; wasm import section contains no
  network/storage/timer imports; release-module gzip ≤ 320 KB. (CI gates,
  run with the suite.)

Suite discipline unchanged: fully offline, one `cargo test -p e2e-tests` +
one `pnpm e2e`, real binaries, no mocks on the wire path.

---

## Appendix — decisions taken vs alternatives rejected (for the record)

- **Events in the snapshot** vs lazy epoch backfill on history access:
  rejected the latter as a P-blind violation (note-block-correlated fetch
  pattern). A1.1.
- **Anchor-mandatory snapshots** vs best-effort (like epoch anchors): a
  snapshot exists to be trusted at cold start; unanchored publication is a
  silent trust downgrade. A1.4.
- **Notification-only SSE** vs inline diffs: no second trusted wire format;
  non-load-bearing stream. A2.1.
- **`FeedEvents` outside `FeedTransport`**: keeps the compile-locked privacy
  seam byte-stable. A2.6.
- **Snapshot grammar reused as the wasm mirror-blob payload**: one fold
  format, one validator, cross-checked by legs m/q. A3.4.
- **Encrypted discovery-state blob** vs documented-plaintext registry:
  closes the shared-machine fingerprint structurally. A3.4.
- **`fzstd` (pure TS, decompress-only)** vs wasm zstd: smaller supply chain,
  no second wasm module; paired with verify-before-decompress. A4.6.
- **Refusal (not warning) on non-loopback keyed serve without auth**: keyed
  surfaces fail closed. A5.5.
- **SQLite snapshot dropped**: nondeterministic bytes have no place on a
  content-addressed trust path. A1.6.
