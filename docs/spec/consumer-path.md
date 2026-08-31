# Consumer path — spec addendum (A1–A6)

Status: FINAL for implementation. Extends [architecture.md](architecture.md)
(base spec v1); every amendment quotes the base section it replaces. Honest
deltas of the shipped build are in
[implementation-notes.md](implementation-notes.md) and are designed against,
not around.

Synthesis of the three council proposals
([p1-verification](../research/council/consumer-path/p1-verification.md),
[p2-simplicity](../research/council/consumer-path/p2-simplicity.md),
[p3-dx](../research/council/consumer-path/p3-dx.md)) under three judge
verdicts (auditor / maintainer / integrator). Backbone: **P2's data plane**
(slots-only snapshots at every cut with an honest history floor; a poke-only
notification stream; stateless-where-keyed serve; checkpoint-only per-key
persistence) + **P3's consumer surface** (the wasm ABI that decides staleness
and deltas in Rust; the npm client's browser realism) + **P1's hard edges**
(mandatory snapshot anchors, verify-before-decompress, objdump/lockfile purity
locks, the stamping matrix, mechanically non-vacuous acceptance legs).

Roadmap decisions are GIVEN and not re-litigated: two blocks with the
`FeedTransport` seam; WASM as a pure synchronous computer; keyless + delegated
dual API; no write path in our binaries; deferred items stay deferred.

Invariants that bound every design below, restated because every new surface
inherits them:

- **P-blind** — the multiset of requests a keyless consumer emits is a function
  of feed progress only; identical for every key and every address.
- **P-keyless** — no encoding of the viewing key, the address, or any
  key-derived felt appears in any request, any server artifact, any log, or any
  unencrypted client artifact other than `sync.db` (0600).
- Epochs are immutable (cut only ≤ `l1_accepted`); only the tail rewrites.
- Upstream `discovery-core` is consumed **unmodified** (manifest-only fork
  delta, CI-audited).
- Canonical bytes + the sha256 hash chain are the sole identity of data.
- Every new surface gets an acceptance-test leg, written **red first**.

No deadline shaped any choice here; every ruling in §0 is argued from merit.

---

## 0. Conflict resolutions

Each ruling names the disagreement, the decision, and the reason. Where a
judge's graft is rejected, that is said plainly.

### 0.1 Unanimous, recorded so nothing drifts back

**R-A. Snapshots carry slots only** — `(slot, value, write_block)` triples, no
events, no block index. All three judges. Discovery is `RawStorageAccess`-only
(upstream `MockBackend` has no events and drives full discovery); spent-state
reads nullifier slots; per-note `block_number` and the 10-block maturity rule
come from the per-slot write block. P1's events-in-snapshot is rejected: it
grows linearly with history (destroying the property the snapshot exists to
create) and adds a bulk section the MPT anchor cannot verify. P1's deciding
argument — that the alternative is note-correlated lazy epoch backfill — is a
strawman: nothing proposes that, and §1.6 forbids it by policy and by the
absence of any API for it.

**R-B. The SQLite snapshot and `strk20 snapshot import` are deleted from the
spec.** Nondeterministic page bytes have no place on a content-addressed trust
path, and the browser cannot read them.

**R-C. SSE is a notification plane.** One global parameterless stream carrying
no bytes any client trusts; data always flows through the existing verified
fetch path. P3's replay ring / `boot_id` / `Last-Event-ID` resume and its
`change`/`appended` payload fields are vetoed by all three judges: server state
and a second data path bought for latency the `etag` field already covers.

**R-D. `DelegatedClient` speaks the reference compat wire.** P3's bespoke
`/v1/delegated/*` dialect is vetoed by all three: it forks the ecosystem, loses
compat and stock-SDK interop, abandons the §7.4 cursor-interop mandate on that
surface, and doubles the conformance surface.

**R-E. No SSE capability token in a query string, ever.** Proxy and
access-log exposure of a live capability to a user's note stream. If keyed push
ever ships (§5.7), it is `Authorization`-header + fetch-based SSE.

**R-F. Keyed surfaces fail shut.** A non-loopback `--listen` is REFUSED at
startup unless BOTH `--allow-remote` and `--auth-token-file` are given. P2's
loud-warning posture is rejected.

**R-G. `verify:'background'` is not shipped.** Optional integrity on the only
integrity check a snapshot has; integrators would select it for speed and never
handle the failure event. Latency is solved by the worker, not by making
verification optional.

**R-H. Mirrors pull epochs.** P3's mirror-pull snapshot fast path is vetoed: it
produces a mirror that cannot serve pre-basis epochs or `/v1/raw/events`,
breaking U5 byte-identity and making a legitimate mirror indistinguishable from
an omission fork. Mirrors regenerate snapshots locally and hash-compare.

**R-I. Verify the manifest's `zst` sha256 BEFORE decompression**, with bounded
decompressed output, in Rust and in TypeScript. Amends base §7.3.

**R-J. No `futures::executor::block_on` in wasm.** A future that returns
`Pending` parks a thread that does not exist on wasm32. The driver is
`now_or_never()` with a panic on `Pending` — a named programming-error
tripwire.

**R-K. Address-derived storage keys are out.** `sha256(address)` as an
IndexedDB row key is dictionary-confirmable: anyone with a suspected address
confirms "this browser synced for X". Row ids are HKDF-derived from the viewing
key.

**R-L. Below the history floor, error — never clamp.** P3's silent clamp of
`get_events` to the floor is masked incompleteness: the engine would conclude
"no withdrawals" from an empty range. The access layer errors; the API layer
maps to `HISTORY_UNAVAILABLE` naming the floor. P3's additive `history_from`
response field is kept.

### 0.2 Judge disagreements, resolved

**C1 — A3/A4 winner: P1 (auditor) vs P3 (maintainer, integrator).**
Ruling: **P3's shapes, P1's mechanics.** The ABI is P3's (`check_manifest`
arbitrating staleness in Rust, `DiscoverOut` carrying added/spent deltas,
`export_reference_cursor`), because it keeps every decision out of the TS
wrapper — the least testable layer. P1's mechanical locks are mandatory on top
(lockfile-walk purity deny, `wasm-objdump` import-section audit, scanner over
thrown error strings and exported blobs, single-`[patch]` fork). These are
orthogonal: P3 wins the interface, P1 wins the proof.

**C2 — wasm entropy: P1's `entropy32` parameter vs P2/P3's `getrandom` js
feature.** Maintainer vetoed the parameter (misuse surface); auditor and
integrator vetoed the dependency (it breaks the empty-import purity gate).
Ruling: **entropy passed in, 2–1**, on two merits that survive review finding 1
and finding 12 — neither of which is the original "absolutely empty import
section" argument, which was wrong:

- **Dependency-graph merit.** `getrandom` in the module means a JS-calling
  import whose reachability nobody re-audits after every dependency bump; the
  import allowlist (§3.9) stays a short, frozen, wasm-bindgen-only file instead
  of growing a randomness entry that later cover for others.
- **Determinism merit.** Every seal is a pure function of its inputs, so leg
  **q**/**r** can pin sealed bytes exactly and the nonce-safety property is
  *testable* rather than assumed.

**Corrected, and load-bearing:** the original structural claim — that the
authenticated, strictly-incrementing `counter` in `info` makes stale or constant
`entropy32` safe — is **false and is withdrawn** (finding 1). `counter` is
monotone only along a single non-forking chain of blob updates, and §4.3/§4.4
make forking a supported state (two tabs without Web Locks, a restored or
evicted IndexedDB, a crash between `discover` and the IDB write). Two forks read
counter *N*, both derive *N+1*, and under constant entropy both produce the same
nonce over different plaintexts — XChaCha20-Poly1305 keystream reuse over
exactly the key-derived material the seal exists to hide. **Nonce safety rests
on the caller supplying 32 fresh `crypto.getRandomValues` bytes per call**, the
npm wrapper ships that call rather than documenting it, and §3.6 adds a
narrow structural guard (`prev_entropy_h`) that catches the constant-entropy
case without pretending to close the forked-with-stale-entropy case.

**C3 — persisted state blob format: P1/P2 NDJSON vs P3 binary framing.**
Auditor vetoed a second serialization framework; maintainer vetoed a
checksum-less blob; integrator accepted NDJSON "either way keep the integrity
trailer". Ruling: **canonical NDJSON in the one grammar family, with a
mandatory self-hash in the `end` line** (§3.5). Both objections satisfied; no
second framework enters the codebase.

**C4 — sealed per-key blob contents.** P3 seals live cursors + a generation
counter; auditor vetoed (reorg logic re-enters browser persistence). Ruling:
**checkpoint-only.** The durable blob holds only material bound to
`last_epoch_to`; the live pass reruns from the checkpoint in memory on every
slice. This is what makes the notes-§7 property ("the browser client needs no
reorg logic at all") literally true rather than nearly true, and it is what
leg **r** proves by byte-identity across a tail fork.

**C5 — the in-process DB `FeedTransport`: auditor and integrator say build it;
maintainer says the property is already delivered by the compat `DbBackend`
bridge plus colocated `DirTransport`.** Ruling: **build it, 2–1**, and because
the roadmap lists it as a given (item 5) that a spec addendum may implement but
not repeal. The maintainer's technical objection — duplicated schema knowledge
outside `indexerd`, bypassing the verification seam — is answered by design,
not waved away: the epoch encoder lives in `strk20-feed` (shared, not
duplicated); only DDL knowledge is duplicated — plus one leaf-crate copy of the
`full_slot_set_as_of` query, named here as the honest cost — in a crate that
does not depend on `strk20-indexerd`; and **epoch** fetches self-verify
`sha256(payload) == epochs.content_hash` before returning, so epoch
serialization drift dies at the source rather than reaching a client. That last
clause said "every fetch" and was wrong: head and manifest synthesis have no
content hash to check against anywhere in the system, and §5.4 now states that
gap and the feed-dir cross-check that closes it where a feed dir exists. The
maintainer's
substantive point is recorded as the **documented default deployment**:
colocated `serve` uses `DirTransport` on the feed dir; `db:` exists for
feed-dir-less setups (§5.4).

**C6 — `EpochPayload::{Compressed, Raw}` on the compile-locked trait.**
Maintainer vetoed the churn; auditor and integrator require it. Ruling:
**ships.** It is load-bearing, not convenience: base §4.3 states zstd output is
version-unstable, so a DB transport that recompresses cannot promise to match
the manifest's `zst` hash — and under R-I (verify `zst` before decompressing)
that combination is a latent hard error. `Raw` payloads are verified directly
against the content sha256. The compile-fail lock is regenerated in the same
commit; no user-derived parameter becomes expressible.

**C7 — keyed watch/SSE on `serve`.** Integrator ranked P1 first partly for its
header-token watch; auditor and maintainer vetoed keyed registries in v1.
Ruling: **poke + keyed requery in v1**, which the integrator also grafts as the
baseline. The key is then held for request duration instead of connection
lifetime, and the keyless and delegated clients share one update-loop shape.
P1's header-token design is recorded verbatim as the roadmap shape with an
explicit merit trigger (§5.7).

**C8 — snapshot anchor sidecar: a dedicated `snapshots/{e:08}.anchor.json`
(P1) vs reusing the epoch anchor sidecar (P3).** Ruling: **dedicated sidecar,
REQUIRED.** The epoch anchor is best-effort by R7 and may be absent; making a
mandatory guarantee depend on an optional artifact is exactly the silent
trust downgrade the auditor flagged in P2's "ring 3 may be absent". A separate
required file makes the requirement structural.

**C9 — what the snapshot anchor proves offline.** The maintainer grafted "the
DEFAULT client verifies the proof walk offline… binds the recomputed root to a
block hash in the verified chain data, no RPC needed". Ruling: **the offline
walk ships, the claim is corrected.** Under R-A the snapshot carries no block
index, so a cold-started client has no independent knowledge of the basis
block's hash; the offline walk proves the sidecar is a well-formed proof of
*this* storage root, not that the block is canonical. Grounding in the chain
requires ring 3 (the user's own RPC). The ladder in §1.5 states each ring's
grade plainly rather than overclaiming.

**C10 — `fold_epochs`/`fold_step` in `strk20-feed::snapshot`.** Auditor grafts
the module as mandatory; maintainer vetoes it (a permanent two-path
byte-equality obligation) and grafts the cheap audit form; integrator wants
only the audit command. Ruling: **one encoder, no second fold path.**
`strk20-feed::snapshot` ships `encode` / `parse` / `verify_against_manifest` /
`storage_root_of` — the format must live in one wasm-clean place, which all
three want. `fold_step` does not ship: the cutter serializes from DB rows (the
slot-set query `verify-root` already runs), and the auditor's substance is
delivered by `strk20-sync snapshot audit`, which applies epochs into a temp
mirror and byte-compares — the client's apply path *is* the fold.

**C11 — snapshot file naming and manifest shape.** Ruling: epoch-indexed
`snapshots/{e:08}.strk20s.zst` (symmetry with `epochs/` and `manifest.epoch(e)`
lookups — grafted by all three over P2's block-indexed name); **single
`snapshot` object**, not P3's `snapshots[]` array (keep-newest-2 retention
needs no array).

**C12 — snapshot verify-root block.** P2 gates publication on the batch
`verify-root`, which per implementation-note 5 runs at `min(l1, frontier)` —
not at the basis block. Ruling: **both.** No snapshot is published unless (a)
the batch `verify-root` did not fail and (b) a root check at **exactly the
basis block** succeeded. Without (b) the header `storage_root` is
server-declared and a client recompute proves only self-consistency.

**C13 — behavior on a snapshot root mismatch.** P1 wants a named hard error;
P3's leg wants silent fallback to epoch replay. Ruling: **loud error, then
fallback under `--cold-start auto`.** `SNAPSHOT_ROOT_MISMATCH` is raised and
logged at WARN with both roots; `auto` then retries once via full epoch replay
and surfaces `snapshot_rejected: true` in the report and in `status()`. Neither
silent nor a dead end.

**C14 — worker default in npm.** Maintainer vetoed worker-on-by-default
(structured-clone key copies before any measurement); auditor and integrator
want it on. Ruling: **on by default, `worker: false` to opt out**, with the
maintainer's objection answered rather than outvoted: the key crosses to the
worker by **ArrayBuffer transfer**, detaching the caller's buffer, so exactly
one copy exists in flight and the module zeroizes it. The ~40-line worker
recipe still ships as the `/worker` subpath.

**C15 — `persist: 'auto'`.** Integrator grafts it; maintainer vetoes it.
Ruling: **rejected.** A per-device runtime chooser means both lanes alive
forever plus the chooser, and it re-opens per device the question the fold gate
exists to close once. Options are `'raw'` and `'folded'`, with the gate
deciding the default and `'folded'` built only if the gate says so.

**C16 — `DecompressionStream('zstd')` feature-detect.** Auditor grafts it;
maintainer vetoes the dual path for v1. Ruling: **fzstd only in v1.** Two
decompression paths make any divergence appear as browser-specific hash
mismatches — a support trap for a latency win the worker already covers.
Native-stream promotion stays a recorded roadmap item.

**C17 — fold-gate reference device.** P3's "maintainer's laptop" is vetoed
(biases the verdict toward Design R and against the mid-tier phones the ~2 s
band exists for); P2's CI-runner median is vetoed (hardware variance makes the
threshold arbitrary). Ruling: **pre-registered rule, p95 over ≥5 runs on
4×-CPU-throttled headless Chromium**, reference-device numbers recorded
alongside, CI as a trend line with a 3× regression alarm (§4.6).

**C18 — client chain identity.** P2's TOFU-only stance is superseded: a
mainnet wallet pointed at a Sepolia feed URL on first use would pin it silently
and proceed. Clients carry a built-in expected profile (default `mainnet`)
checked on first contact; TOFU remains the mechanism only for explicitly custom
feeds (`--profile` / `network: ChainProfile`).

**C19 — `snapshot_every` profile field.** Dropped with the cadence ruling: one
snapshot per cut batch, no knob.

**C20 — SSE endpoint name and `reorg` flag.** `/feed/live` (2 of 3, and the
name `serve` reuses). P1's `reorg: true` flag is dropped: the client's
contradiction check in `apply_feed` is the mechanism, and an advisory flag on
the notification plane invites clients to make it load-bearing.

**C21 — `SNAPSHOT_EVENT_GAP`.** Falls with R-A (it exists only to patch the
hole events-in-snapshot creates). The auditor's optional post-floor variant is
also dropped: with a history floor it would fire on ordinary head/tail races
while proving nothing anchor-grade. Replaced by leg **m**'s fallback assertion.

### 0.3 Must-not-ship list (consolidated)

Events in the snapshot · `fold_epochs`/`fold_step` dual-path library ·
`snapshot_every` knob · mirror-pull snapshot seed · SQLite snapshot ·
SSE replay ring / `boot_id` / `Last-Event-ID` journal · `appended`/`change`
payload fields on the stream · `block_on` as the wasm driver · checksum-less
state blob · `getrandom` inside the module · live cursors or a generation
counter in the durable sealed blob · address-derived IndexedDB row keys ·
`verify:'background'` · `persist:'auto'` · inline-base64 wasm entry ·
`DecompressionStream` dual path · bespoke `/v1/delegated/*` wire ·
token-in-URL · keyed SSE / watch registries in v1 · warning-instead-of-refusal
on a non-loopback keyed bind · TOFU-only client chain identity · silent
`get_events` clamp to the history floor · maintainer's-laptop fold gate ·
recompress-and-return in the DB transport.

---

## 0.4 Preliminary refactor (prerequisite for everything below)

Two behavior-frozen moves. Both keep the suite green commit by commit; their
test is the existing suite.

**0.4.1 `crates/consumer` — lib `strk20-consumer`, Block B core, wasm-clean.**
Extracted from `crates/client`: the feed apply/verify state machine
(`FeedStore::apply_feed`'s logic), the cursor re-open rule
(implementation-note 1), the `run_incoming`/`run_outgoing` pass loops, note
registration and nullifier computation, spent-state refresh, and the
checkpoint/live split from `sync.rs`. Parameterized over one synchronous trait:

```rust
/// The store Block B folds into. Synchronous: the async engine traits are
/// adapted per host (spawn_blocking natively, ready futures in wasm).
pub trait ConsumerStore {
    fn read_slot_as_of(&self, slot: &Felt, bound: u64) -> Result<(Felt, u64)>;
    fn events_in_range(&self, from: u64, to: u64) -> Result<Vec<StoredEvent>>;
    fn block_meta(&self, number: u64) -> Result<Option<BlockMeta>>;
    fn apply_block_line(&mut self, line: &BlockLine, finality: Finality) -> Result<()>;
    fn apply_slot(&mut self, slot: &Felt, value: &Felt, write_block: u64) -> Result<()>;
    fn supersede_range(&mut self, from: u64, to: u64) -> Result<()>;
    fn is_empty(&self) -> Result<bool>;
    fn meta_get(&self, key: &str) -> Result<Option<String>>;
    fn meta_set(&mut self, key: &str, value: &str) -> Result<()>;

    // --- note registry (review finding 9) ---
    // Without these the trait cannot carry what §0.4.1 says moves into
    // strk20-consumer. `register_notes` / `refresh_spent` are declared generic
    // over a READ-ONLY view (RawStorageAccess + RawEventAccess), which has
    // nowhere to put a registered note; in the shipped code these are
    // `FeedStore` methods writing the `notes_registry` table (`upsert_note`,
    // `notes`, `refresh_spent`, `prune_missing_notes` — the last one is
    // load-bearing reorg cleanup added by the adversarial review), and in wasm
    // the same state lives in the sealed AEAD blob (§3.6 `notes[]`), not in the
    // store at all.
    fn notes_get(&self, owner: &Felt) -> Result<NoteSet>;
    fn notes_put(&mut self, owner: &Felt, notes: &NoteSet) -> Result<()>;
}
```

**The registry is a value type, and the store only persists it.**

```rust
/// Owner-scoped note registry. Ordered by (token, index) so it serializes
/// canonically and diffs deterministically.
pub struct NoteSet(BTreeMap<(Felt, u64), NoteRec>);

impl NoteSet {
    pub fn upsert(&mut self, n: NoteRec);
    pub fn set_spent(&mut self, nullifier: &Felt, spent: bool);
    /// The reorg cleanup that `prune_missing_notes` performs today: drop notes
    /// whose backing slot no longer exists as of `as_of`.
    pub fn prune_missing(&mut self, present: &BTreeSet<Felt>) -> usize;
    /// The pure diff DiscoverOut.added/spent report.
    pub fn diff(&self, prior: &NoteSet) -> (Vec<NoteRec>, Vec<NoteRec>);
}
```

`register_notes` and `refresh_spent` take `&mut NoteSet` alongside the
read-only view and return the diff; **neither touches a store.** The two hosts
then persist it in the two incompatible ways the spec actually requires, and
neither leaks into the other:

- **native** — `FeedStore::notes_get/notes_put` read and write the existing
  `notes_registry` table; reorg cleanup is `NoteSet::prune_missing` driven from
  the same slot query `prune_missing_notes` uses today;
- **wasm** — `MemStore::notes_get` returns the `NoteSet` decoded from the
  decrypted sealed blob supplied to `discover()`, and `notes_put` stages the
  set the module re-seals on return. Nothing key-derived ever enters `MemStore`
  beyond the lifetime of the call.

This is why `DiscoverOut.added_json` / `spent_json` are a **pure diff** against
the supplied sealed blob rather than a store query.

**The trait must be named correct before step 0a**, and 0a's stated test — "the
existing green suite" — **cannot detect a missing abstraction**, which is
precisely how this defect survived to review. 0a therefore gains one test it
does not have today, written red first like everything else: a `NoteSet`
round-trip conformance leg run against both impls (`FeedStore` and `MemStore`),
asserting `notes_put`∘`notes_get` is the identity and that
`register_notes`/`refresh_spent` produce the same `NoteSet` and the same diff
over both.

Also exported: `apply::{apply_snapshot, apply_epoch, apply_head}` (generic over
`ConsumerStore`), `discovery::{reopen_cursor, run_incoming, run_outgoing,
register_notes, refresh_spent, sync_over}` (generic over a view implementing
`RawStorageAccess + RawEventAccess`, plus `&mut NoteSet` where the signature
above requires it), `notes::{NoteSet, NoteRec}`, and `report::SyncReport` —
**one report schema for the native CLI, the wasm module, `serve`, and npm**, so
a single golden oracle file pins all four.

`crates/client` keeps the SQLite `FeedStore` (an impl of `ConsumerStore`),
`ClientView` (unchanged, `spawn_blocking`), transports, CLI, MPT verify.
Deps of `crates/consumer`: `strk20-feed`, `discovery-core`, `serde_json` —
nothing host-specific. `cargo build -p strk20-consumer --target
wasm32-unknown-unknown` is a CI gate from day one. The existing conformance
tests (engine-over-`FeedStore` ≡ engine-over-`MockBackend`) move with the code
and pin the refactor; a third leg (engine-over-`MemStore`) joins them in §3.

**0.4.2 `crates/wire` — lib `strk20-wire`.** Move
`crates/indexerd/src/compat/wire.rs` + `block_id_serde.rs` into a leaf crate
(pure serde structs, provenance notes and the Apache-2.0 notice move with
them); `indexerd` re-exports for source compatibility. This is what lets
`strk20-sync serve` speak the reference wire without the client crate
depending on `strk20-indexerd` — base §3's dependency-direction invariant
("nothing depends on `strk20-indexerd`") stays intact and enforceable.

---

## A1 — Snapshots in the cutter + client-verified storage-root anchor

### 1.1 What a snapshot is, and what it deliberately is not

A snapshot is the folded **slot state** of the pool at one epoch boundary:
every slot with a nonzero value as of that block, carrying its value and its
last write block. It carries no events and no block index (R-A).

Capability boundary, stated as a first-class property rather than a gap:

> A snapshot-started client has complete **discovery, balances, spent-state and
> per-note block metadata**. **Transaction history is a partial page set that
> terminates at the walk's first pre-floor read and never reports complete.**

**This wording is the corrected one (review finding 3).** The previous wording —
"history is available from the snapshot block forward" — was verified against
the pinned engine and is **not deliverable with the engine consumed unmodified**:

- `history::fetch_transactions` walks **backwards** from the view bound.
  `process_next_block` takes each note's block from the note scanner and calls
  `fetch_aggregated_block_events(block_number)` plus
  `fetch_aggregated_withdrawal_events(block_number + 1, cursor.begin_block_number)`.
  Note blocks come from the slot's `last_update_block` — which, for a
  snapshot-started client, is exactly the pre-basis `w` the snapshot carries.
  Under R-L those reads error rather than clamp, so the walk terminates at the
  first pre-floor note block.
- When the scan does complete, the engine **unconditionally** appends the
  synthetic registration transaction: `fetch_registration` reads
  `get_public_key_with_block(user)` (a slot read — served fine from the
  snapshot) and then, whenever `last_update_block != 0`, issues
  `get_viewing_key_set_events(user, reg_block, reg_block)` at the
  **registration** block, which is below the floor for essentially every
  existing user. Non-budget errors there `return Err`, so the whole call fails
  and `cursor.history_complete` can never become true.

So the honest boundary has two parts, and the paging contract that delivers it:

**Paging contract for a snapshot-started client.** `history()` drives the
unmodified engine page by page with a bounded `max_transactions`. Every page
that completes is kept and returned. The first page whose walk raises
`HISTORY_UNAVAILABLE` **terminates** the walk; the terminating error is caught
at the API layer and converted into a page boundary, never propagated as a
failure of the whole request:

```
{ "transactions": [<all completed pages, descending by block>],
  "complete": false,
  "complete_from": <bound>,          // NOT necessarily history_floor — see below
  "registration_available": false }  // when the walk terminated before/at fetch_registration
```

`complete_from` is the lowest block for which an **iteration completed** — i.e.
`cursor.begin_block_number` after the last successful page — and is `≥
history_floor`. It is deliberately **not** reported as `history_floor`: the
terminating iteration fetches block events at its note block *before* the gap
withdrawals in `[note_block + 1, previous_upper]`, so when that note block is
pre-floor, withdrawals in the above-floor part of that gap are never fetched.
Claiming completeness down to the floor would therefore be an overclaim of
exactly the kind R-L exists to prevent. A fully epoch-replayed mirror reports
`complete: true` and `complete_from: 0`.

**This does not repeal R-L.** The access layer still errors and never clamps;
the engine still never concludes "no withdrawals" from a silently emptied range.
The change is at the API layer only, and it makes the incompleteness *louder*,
not quieter: an explicit terminating bound plus `complete: false` plus
`registration_available: false`, instead of a thrown error that discards the
page a caller could legitimately have.

Made loud in five places:

- the store records `history_floor = snapshot.block + 1` (meta row; wasm state
  blob header field). A fully epoch-replayed mirror has `history_floor = 0`;
- `SyncReport` gains `"history_from": <block>` (additive) — surfaced by the
  CLI, wasm, `serve`, compat `/v1/history`, and npm (`completeFrom`);
- `history()` responses carry `complete`, `complete_from` and
  `registration_available` as above, at every API layer;
- any history read below the floor is a hard error at the access layer, mapped
  to `HISTORY_UNAVAILABLE {"floor": <block>}` at every API layer (R-L). A
  caller that asks for an explicit `from_block < history_floor` still gets that
  error — the paging contract covers the walk crossing the floor on its own,
  not a caller reaching below it deliberately;
- the escape hatch is the feed itself: `--cold-start epochs` /
  `coldStart: 'epochs'` replays everything. Snapshot start is an optimization,
  never a replacement — and for complete history it is not an option at all.

### 1.2 Snapshot wire format v1 (byte-precise, frozen)

File `snapshots/{e:08}.strk20s.zst` — zstd level 19 over the canonical NDJSON
payload. **Content identity = sha256 over the UNCOMPRESSED payload** (same rule
as base §4.3; the `zst` hash is a transport checksum only). Canonical JSON
rules identical to base §4.3: fixed field order exactly as written, no
whitespace, all felts lowercase `0x` minimal hex (zero = `0x0`), every line
terminated `\n` including the last.

```
line 1 (header):
{"t":"hdr","v":1,"kind":"strk20-snapshot","chain_id":"SN_MAIN","pool":"0x…","epoch":1405,"block":14059999,"epoch_hash":"<64-hex content hash of epoch 1405's payload>","storage_root":"0x…","class":"0x<pool class as of block>"}

one line per slot with a nonzero value as of `block`, ascending by the 32-byte BE slot:
{"t":"s","k":"0x<slot>","v":"0x<value>","w":<last write block ≤ header.block>}

last line (footer):
{"t":"end","slots":<n slot lines>}
```

Invariants, all test-asserted:

- `header.block == epoch_range(header.epoch).1` — snapshots exist **only** at
  epoch boundaries, hence ≤ `l1_accepted`, hence immutable by construction.
- `header.epoch_hash == manifest.epoch(header.epoch).hash` — the pin that lets
  a snapshot-started client continue the one hash chain. There is no second
  chain: one spine, derived leaves.
- `header.storage_root == feed::mpt::storage_root(slot lines)`. It is inside
  the content-addressed bytes: unlike cut-time anchor metadata (base R7), the
  root is a deterministic function of chain data and therefore cannot fork
  hashes across mirrors.
- Zero-valued slots are never emitted (Cairo map semantics; matches
  `mpt::storage_root`, which the shipped `full_slot_set_as_of` already
  filters).
- The payload is a pure function of DB rows as of `block` → byte-identical
  across operators and re-runs (leg **n**).

### 1.3 Anchor sidecar (required)

> **SUPERSEDED by §11 (measured 2026-08-31).** A proof for `header.block`
> cannot be obtained from any public provider: the `getStorageProof` window is
> ~1024 blocks behind head, while a snapshot's basis block is an epoch boundary
> thousands of blocks old at cut time. Read §11 for the replacement — a
> head-captured anchors log plus a *reachability* check — before implementing
> anything in §1.3–§1.5.

`snapshots/{e:08}.anchor.json` = the full stored `starknet_getStorageProof`
response for `header.block`. **Required**: if it cannot be obtained, no
snapshot is published (§1.4). It is outside content addressing, exactly like
epoch anchors (base R7), but unlike them it is not optional — see C8.

### 1.4 Cutter integration

Amends base **§5.5**. Appended to the cut sequence, after the manifest rewrite
of a successful batch:

1. If the batch's `verify-root` failed, **stop** — no snapshot. (Same posture
   as "never publish a divergent epoch".)
2. Let `e` = the newest cut epoch, `b = epoch_range(e).1`.
3. `full_slot_set_as_of(b)` → `feed::mpt::storage_root` → `local_root`.
4. `getStorageProof(block_id = b, pool, keys = [])`. On any failure: skip the
   snapshot entirely, log at WARN, retry at the next cut batch. On success,
   require `contract_leaves_data[0].storage_root == local_root`; a mismatch is
   the §5.6 alarm path (do not publish, mark `verify_root_failed`).
   This is the **basis-block root check** required by C12 — the batch
   `verify-root` runs at `min(l1_accepted, frontier)`, which is not `b`.
5. `snapshot::encode` → sha256 → zstd-19 → atomic tmp+rename into
   `snapshots/{e:08}.strk20s.zst`; write the sidecar; rewrite `manifest.json`
   with the `snapshot` object.
6. Retention: keep the newest **2** snapshot + sidecar pairs (a client that
   read the previous manifest moments earlier never 404s mid-download); delete
   older ones. Snapshots are derived artifacts — deletable, never in the hash
   chain.

Cadence falls out with no new knob, timer, or state machine: **one snapshot per
cut batch** (~10 000 blocks). The marginal cost is one extra `getStorageProof`
plus one MPT recompute and a serialization pass over a slot set the cutter
already queries.

### 1.5 Client verification ladder (Rust and browser, identical)

Rings, in order. 1–5 are mandatory and offline; 6 is optional, address-blind,
and the only ring that grounds the snapshot in the chain.

1. **Transport**: `sha256(compressed bytes) == manifest.snapshot.zst`
   **before decompression**; decompress with a 256 MiB output cap (R-I).
   Failure: `FEED_HASH_MISMATCH` / `DECOMPRESS_LIMIT`.
2. **Content**: `sha256(payload) == manifest.snapshot.hash`.
3. **Structure + identity**: parse; slot lines strictly ascending; footer count
   matches; every `w ≤ header.block`; `header.block == epoch_range(epoch).1`;
   `header.chain_id`/`pool` equal the expected profile AND the manifest AND
   `genesis.json`. Failure: `FEED_MALFORMED` / `CHAIN_MISMATCH`.
4. **Chain pin**: `manifest.epoch(header.epoch).hash == header.epoch_hash`.
   Failure: `FEED_CHAIN_BROKEN`.
5. **Self-consistency of the slot set against the server's declared root**:
   recompute `feed::mpt::storage_root` over the slot lines and require equality
   with `header.storage_root`, with `manifest.snapshot.storage_root`, and with
   `contract_leaves_data[0].storage_root` parsed from the sidecar; require
   `header.class == contract_leaves_data[0].class_hash ==
   manifest.snapshot.anchor.class` (this is what `header.class` is FOR — review
   finding 14h; before this amendment no ring read it); then walk the sidecar's
   `contracts_proof` from the contract leaf to the declared
   `global_roots.contracts_tree_root` with `feed::mpt::verify_storage_proof`.
   Failure: `SNAPSHOT_ROOT_MISMATCH {computed, header, anchor}`.

   **Grade — renamed and corrected (C9, review finding 2).** The old heading
   "proof-grade for the slot set" is withdrawn. Every value this ring checks
   against — `header.storage_root` (inside the content hash),
   `manifest.snapshot.storage_root`, `contract_leaves_data[0].storage_root`,
   and `global_roots.contracts_tree_root` — is produced by the **same server**,
   and none is bound to a block hash the cold-started client knows. A malicious
   feed can therefore serve a snapshot with an added, missing or altered slot
   and recompute all three roots and the whole sidecar consistently; rings 1–5
   pass. What this ring genuinely buys is: (a) the slot set is exactly the one
   the server declared — no corruption, no truncation, no transport damage, no
   drift between the file, the manifest and the sidecar; (b) the sidecar is a
   well-formed proof of *that* root. It buys **nothing against the server
   itself**. Canonicity comes only from ring 6.

   This is a real difference from the epoch path, and it is not hedged
   elsewhere in this document: on epochs an omitted or altered block is a
   *visible fork* (content-addressed payloads, the `prev` hash chain back to
   the first pool epoch, and cross-mirror byte-identity, base §4.3/§9/U5). On
   the snapshot path none of those three apply to the slot set.
6. **Chain grounding (address-blind) — mandatory whenever an RPC URL is
   available**: fetch `starknet_getStorageProof(block_id = header.block, pool,
   keys = [])` from the **user's own** RPC and compare `storage_root` and
   `block_hash`. The request names only the public pool and a public block —
   identical for every user — so this is keyless-compatible. The server stays
   outside the proof path.

   Amended from "optional, recommended" (review finding 2c): if
   `--verify-anchor <rpc>` / npm `anchorRpcUrl` is configured, ring 6 **runs and
   must pass**; skipping it is not an option and there is no
   `verify:'background'` equivalent for it (R-G's reasoning applies verbatim).
   If no RPC URL is configured the snapshot path still proceeds, and the
   reduced grade is **surfaced, not swallowed** (§1.5.1).

#### 1.5.1 The grade is surfaced, not implied

`SyncReport`, `status()` and `/health` carry

```
"verified": "anchored" | "server-asserted" | "replayed"
```

- `"replayed"` — the mirror was built by epoch replay from genesis; the
  epoch-chain guarantee of base §9 applies unchanged.
- `"anchored"` — snapshot-started **and** ring 6 passed against the user's own
  RPC.
- `"server-asserted"` — snapshot-started with no ring 6. The slot set is
  self-consistent and exactly what the server declared, and nothing more.

This replaces npm `ClientStatus`'s `verified: boolean`, which was ungrounded —
nothing in the spec said what `true` meant. The npm README and the CLI both
print the string, and `"server-asserted"` is printed at WARN on first sync.

#### 1.5.2 Amendment to base §9's trust table (required)

Base **§9**'s table has three rows (Feed / Raw / Compat) and its "Feed
(default)" row describes the epoch-chain guarantee only. Because
`--cold-start auto` makes the snapshot path the **default** for a fresh
consumer, the row integrators read would otherwise advertise a guarantee the
default posture does not deliver. Base §9 gains a fourth row:

| Mode | Server learns | Posture |
|---|---|---|
| Feed, snapshot cold start (`--cold-start auto`/`snapshot`, no anchor RPC) | Same as Feed: public GETs only, identical across users. | **Reduced integrity grade.** The slot set at the basis block is attested only by roots the same server produces (§1.5 ring 5); omission or alteration of a slot is NOT a visible fork the way an omitted block is on the epoch path. Reported as `verified: "server-asserted"`. Restore the full grade with an anchor RPC (ring 6, → `"anchored"`) or with `--cold-start epochs` (→ `"replayed"`). |

Base §9's closing "Trust story" paragraph gains one sentence: *"Cross-mirror
byte-identity backs epochs unconditionally; it backs snapshots only for the
epochs inside the retention window that both mirrors currently hold (§1.4 step
6)."*

**Retention stays at 2 — the review's stronger sub-claim is rejected.** The
review argued that keep-newest-2 makes cross-mirror snapshot comparison
"unavailable in practice". It does not: `epoch_size` is fixed by the profile,
so any two mirrors caught up to the same tip hold snapshots for the **same**
newest two epochs, and that is when a cross-check is meaningful. The claim is
true only for a lagging mirror, which is a comparison nobody can make anyway.
What the review is right about is that the precondition was never stated: leg
**n** must now pin it (§8).

Two guards make the impossible loudly impossible:

- `manifest.snapshot` present with `anchor == null` ⇒ `SNAPSHOT_ANCHOR_MISSING`
  (refuse the snapshot path; fall to epoch replay).
- any view bound `< snapshot.block` ⇒ `BOUND_BELOW_SNAPSHOT {bound, basis}`.
  Pre-basis history does not exist locally and must never be answered with
  zeros. (Engine bounds are always `last_epoch_to` or `head`, both ≥ basis; the
  rule exists so a future refactor cannot introduce a silent zero-read.)

### 1.6 Privacy doctrine for history below the floor

Full history is obtained **only** by full epoch replay — all-or-nothing and
key-independent. Fetching the specific epochs that contain a user's note blocks
would make the request pattern a function of the user's notes and break
P-blind. This is forbidden by policy **and** by the absence of any API for it:
no client surface accepts a block or epoch selector derived from discovery
output, and no transport method accepts a user-derived parameter (§7.2 lock).

### 1.7 Client cold start

Amends base **§7.3**. `apply_feed` gains one branch, taken only when the mirror
is empty, `manifest.snapshot != null`, and cold-start mode allows it:

```
if store.is_empty() and manifest.snapshot is present and mode != epochs
       and transport.snapshot_capable():
    payload = transport.fetch_snapshot(manifest.snapshot.e)?     # verified per §1.5 rings 1–2
    anchor  = transport.fetch_snapshot_anchor(manifest.snapshot.e)?
    # either returning None ⇒ SNAPSHOT_ANCHOR_MISSING ⇒ fall to epoch replay
    run rings 3,4,5, and ring 6 when an anchor RPC is configured (then MANDATORY)
    one transaction:
        for each slot line: apply_slot(k, v, w)     # storage_log(slot, block=w, value)
        meta last_epoch_applied = header.epoch
        meta last_epoch_hash    = header.epoch_hash
        meta last_epoch_to      = header.block
        meta history_floor      = header.block + 1
        meta snapshot_basis     = header.block
        meta verified           = "anchored" if ring 6 ran else "server-asserted"
# the existing path then runs unchanged: epochs > last_epoch_applied verify
# against prev_hash = epoch_hash and apply; the head tail applies on top.
```

No new tables: snapshot rows land in `storage_log` with their real write
blocks, so the shipped as-of query (`ORDER BY block DESC LIMIT 1`) serves them
and `read_slots_with_block` returns exact `last_update_block`. Nothing reads a
`blocks` row below the floor — `ClientView::get_events` joins `blocks` only for
events, and events exist only ≥ floor. Consequence recorded: cross-epoch block
parent-linkage verification is not possible across the snapshot seam and
resumes at `basis + 1`.

CLI: `strk20-sync sync --cold-start auto|snapshot|epochs` (default `auto` = the
branch above, with the C13 fallback). A non-empty mirror never touches
snapshots.

Cold start is O(1) in history length and identical for every user: genesis +
manifest + snapshot + anchor + (epochs > basis, normally 0–1) + head.

### 1.8 Manifest amendment

Base **§4.4** currently reads:

> `"snapshot":{"block":14049912,"sha256":"<64-hex>","bytes":123456}}`

and base **§4.2**:

> `snapshots/latest.sqlite.zst       # optional convenience; epochs are canon`

Replaced by (the field was never emitted by the built system — see
implementation-notes "Not in this branch" — so this is a schema definition, not
a migration):

```json
"snapshot": {
  "e": 1405,
  "block": 14059999,
  "epoch_hash": "<64-hex>",
  "file": "snapshots/00001405.strk20s.zst",
  "hash": "<64-hex sha256 of the uncompressed payload>",
  "zst": "<64-hex sha256 of the .zst file>",
  "bytes": 301234,
  "slots": 48123,
  "storage_root": "0x…",
  "anchor": {"block":14059999,"block_hash":"0x…","storage_root":"0x…","class":"0x…"}
}
```

`snapshot` is `null` until the first anchored snapshot exists; when present,
`anchor` is REQUIRED (non-nullable). Feed dir tree line becomes:

```
snapshots/{e:08}.strk20s.zst      # slots-only folded state at epoch e's end block
snapshots/{e:08}.anchor.json      # full getStorageProof at that block; REQUIRED
```

### 1.9 Format module, mirrors, and audit

`strk20-feed::snapshot` (wasm-clean, no IO, no `compress` feature required):

```rust
pub struct Snapshot { pub header: SnapshotHeader, pub slots: Vec<SnapSlot> }
pub fn encode(s: &Snapshot) -> Vec<u8>;
pub fn parse(payload: &[u8]) -> Result<Snapshot, FeedError>;
pub fn storage_root_of(s: &Snapshot) -> Felt;              // == mpt::storage_root
pub fn verify_against_manifest(payload: &[u8], entry: &ManifestSnapshot,
                               expect: &FeedIdentity) -> Result<Snapshot, FeedError>;
```

No `fold_step` (C10). The cutter serializes from DB rows; the auditor
serializes from an epoch-replayed mirror; the audit leg byte-compares the two.

Mirror-pull is unchanged for servers: `strk20 mirror pull` ingests **epochs**
(a server needs events to cut future epochs and to serve `/v1/raw/events`; it
can never bootstrap from a slots-only snapshot — stated in ops docs). After its
first own cut batch a pulled mirror emits its own snapshot, byte-identical to
the origin's, and `strk20 epoch verify --all` extends to compare snapshot
hashes: divergence is the same loud fork signal as epoch divergence.

CLI additions (amends base §8): `strk20 snapshot create [--epoch <e>]`
(deterministic regen from the DB), `strk20 snapshot verify`,
`strk20-sync snapshot audit --feed <URL|DIR>` (replay all epochs into a temp
mirror, re-serialize, byte-compare against the served file, re-check the
anchor), `--snapshot-keep <n>` (default 2) on `strk20 run|backfill`,
`strk20-sync verify --snapshot`. `strk20 snapshot import` is removed.

### 1.10 Acceptance criteria

Legs **l**, **m**, **n** (§8): cold-start equality vs full replay **minus the
four keys that must differ**, including per-note `block_number` and a note spent
**before** the basis; the MPT check asserted *executed*; the **positive**
history assertion (an above-floor range equals the replay client's and
terminates with `complete_from`, rather than only the negative below-floor
error); six named tamper cases including the consistently-recomputed malicious
snapshot that **nothing catches without ring 6**; verify-before-decompress
asserted by a poisoned decompressor rather than by an error code; no snapshot
after a verify-root failure; determinism across independent backfills at
arranged tip parity; `history_from` and `verified` surfaced.

---

## A2 — SSE on the indexer

### 2.1 Shape

One endpoint on the server binary, always on, no flag:

```
GET /feed/live      →  text/event-stream
```

It notifies; it never carries chain data. On any event the client fetches the
same files it would have polled — `head.ndjson` (conditional), `manifest.json`,
epoch and snapshot files — through the one existing verified path. A lost,
duplicated, reordered or buffered event costs latency only; the polling
fallback bounds it. This keeps base R3's substance: one global stream, never
per-user, no wire rollback protocol — a reorg is just another `head` event
followed by the wholesale tail refetch clients already perform.

Any query string is rejected **400 `INVALID_QUERY`** (stronger than ignoring:
the address-blindness leg gets a server-enforced guarantee). Query-appending
`EventSource` polyfills are documented as unsupported.

### 2.2 Framing (exact)

On connect, in order: a 2 KB `:` padding comment (defeats buffering
middleboxes), `retry: 15000`, `hello`, then the current `head`, the current
`epoch` and `snapshot` (if any), and `status`. Every event is
**state-carrying and idempotent** — never a delta.

```
:<2048 spaces>

retry: 15000

event: hello
id: 1
data: {"v":1,"chain_id":"SN_MAIN","pool":"0x…","module":"strk20/<version>"}

event: head
id: 2
data: {"head":14056431,"head_hash":"0x…","l1_accepted":14049912,"tail_from":14050000,"etag":"\"<64-hex sha256 of head.ndjson>\""}

event: epoch
id: 3
data: {"e":1406,"from":14060000,"to":14069999,"hash":"<64-hex>","zst":"<64-hex>","bytes":12345}

event: snapshot
id: 4
data: {"e":1406,"block":14069999,"hash":"<64-hex>"}

event: status
id: 5
data: {"decode_state":"ok"|"degraded","verify_root_failed":false}

: ka
```

- `hello` carries chain identity so a proxy pointed at the wrong network dies
  before any refetch or state mutation (feeds A6's stamping matrix).
- `head` fires on any change of `head.ndjson` bytes (new block, reorg, L1
  promotion). The `etag` field lets a client skip a conditional GET it has
  already applied; it cannot corrupt anything, because the fetch verifies.
- **Epoch index key is `"e"` on both events that name an epoch** (review finding
  14d): `event: epoch` previously used `"epoch":1406` while `event: snapshot`
  and the manifest both use `"e"`. The manifest is the identity source the
  client cross-references, so `"e"` wins in all three places.
- `epoch` / `snapshot` fire only **after** the manifest rewrite that lists
  them, so a poked client always finds what it fetches for (§2.4).
- `status` fires on `decode_state` / `verify_root_failed` transitions.
- `: ka` keepalive comment every 15 s of silence.
- No `reorg` flag, no `change`/`appended` fields (R-C, C20).

### 2.3 Resume: the empty program

`id:` fields are set so `EventSource` reconnects send `Last-Event-ID`, and the
server **deliberately ignores** it. Because every event carries full current
state and connect always replays current state, a client that was away simply
receives the present; missed intermediate epochs are found in the manifest on
the next fetch, which the connect-time events trigger. There is no replay
buffer, no per-client cursor, no server-side **protocol** state — which is
itself a privacy property: *at the protocol layer* the server cannot be made to
remember a client, because the protocol gives it nothing to remember. **This
paragraph is normative and reproduced in the endpoint docs so nobody later
"fixes" it into a journal.** `id:` exists for client-side dedup and
debuggability only. The claim is scoped to the protocol deliberately; the
transport-layer residual is stated in §2.6 rather than left implied by an
absolute (review finding 18).

### 2.4 Emitter

One global watcher task hashes `head.ndjson` and reads `manifest.json` on a 1 s
interval, publishing a `FeedState` into a `tokio::sync::watch` channel; each
connection is a subscriber that formats events. Watching the **published
files** — rather than plumbing channels out of the ingest loop — makes ordering
correct by construction: the emitter can only announce artifacts that are
already renamed into place and fetchable, which eliminates the
announce-before-rename race class permanently. Response headers:
`Content-Type: text/event-stream`, `Cache-Control: no-cache`,
`X-Accel-Buffering: no`. No connection cap in v1 (self-host posture);
`/metrics` gains `strk20_sse_connections`.

### 2.5 Client behavior

- Reconnect with exponential backoff, 1 s → 60 s, jittered.
- Watchdog: no event or keepalive for 45 s → close and reconnect.
- While disconnected, poll `head.ndjson` with ETag at the configured cadence
  (default 30 s). On any doubt, poll — both paths converge on identical bytes.
- **404/405 on `/feed/live` permanently degrades that session to polling with
  no error surfaced.** A plain static-file mirror has no stream and is a
  fully supported deployment.
- The Rust `--watch` mode **stays polling-only in v1**: one fewer HTTP client
  mode to maintain, on hosts where a 30 s poll is fine. Merit trigger recorded:
  if latency-sensitive Rust consumers appear, consumption is ~60 lines and
  lands in a **separate** trait, never in `FeedTransport`, so the
  compile-locked privacy seam stays byte-stable:

```rust
#[async_trait]
pub trait FeedEvents {                 // impl for HttpTransport only, when it ships
    async fn subscribe(&self) -> Result<BoxStream<'static, FeedNotice>>;
}
pub enum FeedNotice { Hello{..}, Head{..}, Epoch{..}, Snapshot{..}, Status{..} }
```

### 2.6 Privacy invariant

The request is parameterless (400 on any query string), and the emitted bytes
are identical for every subscriber modulo connect ordering and `id` numbering.
The only client-varying header, `Last-Event-ID`, encodes public feed position
and is ignored. Base **§6.1**'s doctrine sentence — "No feed route takes any
parameter derived from a user" — explicitly covers `/feed/live`. Asserted in
leg **o**.

**Transport-level residual, stated rather than asserted away (review finding
18).** The payload analysis above is complete and correct, but SSE changes the
*transport* posture and base R3 treats durable per-client identity as a policy
line, so the line is drawn here instead of left standing behind §2.3's
absolute:

- A long-lived connection **is** session-durable state at the transport layer.
  Its lifetime is observable to the server and to any intermediary, where the
  polling baseline presents a sequence of independent requests.
- §2.7's h2 advice makes this concrete: coalescing the client's file fetches
  onto the stream's connection binds a session's whole fetch sequence to one
  connection identity. Under polling those fetches are independently
  attributable only by IP.
- **Where the line falls:** acceptable for v1. Both residuals are already
  implied by the client's IP for a non-relayed client, so the marginal
  linkability over polling is small; and SSE arguably *improves* the timing
  dimension, since every subscriber is poked simultaneously whereas pollers have
  random phase and therefore a per-client cadence fingerprint. OHTTP remains
  the deferred answer (§9) for clients that must break the IP link at all, and
  it is the same answer for this residual — no separate mechanism is owed.
- Polling remains the reference semantics and a fully supported deployment
  (§2.5), so a client that declines the transport residual has a first-class
  path: `live: false`.

### 2.7 Operator notes (ops docs)

Route `/feed/live` uncached straight to origin; identity `Content-Encoding`;
proxy idle timeout > 60 s (or rely on the 15 s keepalives). h2 multiplexes the
stream beside file fetches, so no separate host is needed — **an operator
convenience, not a client requirement**: per §2.6 it links the session's fetch
sequence to the stream's connection identity, and a client that wants those
independent may use a separate connection at the cost of one more handshake.

### 2.8 Spec amendments

Base **§6.1** table gains:

| Method/Path | Response | Caching |
|---|---|---|
| `GET /feed/live` | SSE notification stream (§A2) | `no-cache`, never cached, no buffering |
| `GET /feed/snapshots/{e:08}.strk20s.zst` | snapshot file, `X-Content-Sha256-Raw` | immutable |
| `GET /feed/snapshots/{e:08}.anchor.json` | snapshot anchor sidecar | immutable |

Base **R3** and **§12.2** are delivered within their guardrails: one global
stream, never per-user, polling remains the reference semantics.

#### 2.8.1 Base §2 invariant 3 and base leg d(i) — quote-and-replace (required)

The one mechanical privacy assertion in the whole suite is an **allowlist**, and
A1 and A2 both widen the fetch set past it. §8 claims "legs a–k exist and stay
green"; without this amendment that claim is arithmetically false the moment
either ships, and the first person to hit the red test will "fix" it by
loosening the allowlist to a prefix match — which is exactly how the property
erodes (review finding 4).

Base **§2 invariant 3** currently reads:

> 3. A feed-mode client emits only GETs for {manifest, missing epochs, head} — a
>    function of download progress only. The keyless property is enforced by the
>    type system (§7.2) and by mechanical wire capture in the acceptance test
>    (§10.3).

Replaced by:

> 3. A feed-mode client emits only GETs, and every URL is drawn from the
>    **closed** set
>    `{ /feed/genesis.json, /feed/manifest.json, /feed/epochs/{idx:08}.strk20e.zst,
>    /feed/epochs/{idx:08}.anchor.json, /feed/snapshots/{e:08}.strk20s.zst,
>    /feed/snapshots/{e:08}.anchor.json, /feed/head.ndjson, /feed/live }`
>    with **no query string on any of them** — a function of feed progress only.
>    The keyless property is enforced by the type system (§7.2), by the
>    server (`/feed/live` answers 400 `INVALID_QUERY` to any query string,
>    §2.1), and by mechanical wire capture in the acceptance test (§10.3).

Base **§10.3 leg d(i)** currently reads:

> (i) every request is a GET with empty body; URL multiset ⊆ {`/feed/genesis.json`,
> `/feed/manifest.json`, `/feed/epochs/…`, `/feed/head.ndjson`} with no query strings;

Replaced by:

> (i) every request is a GET with empty body; URL multiset ⊆ the closed set of
> §2 invariant 3, matched **whole-path against the eight literal patterns
> above** — never by prefix, and never by a `startsWith('/feed/')` test; no
> query strings.

Two consequences, recorded so nothing drifts:

- Leg **d**'s own capture gains `snapshots/*.strk20s.zst` and
  `snapshots/*.anchor.json` as soon as A1's cutter publishes a snapshot into
  the fixture feed, because leg d's client is a fresh mirror under the
  `--cold-start auto` default. It does **not** gain `/feed/live`: the Rust
  `--watch` mode stays polling-only in v1 (§2.5), so `/feed/live` appears only
  in leg **u**'s npm capture.
- The allowlist is **closed and whole-path**. Any future artifact needs an
  amendment here, in the same quote-and-replace form, before it can be fetched.

---

## A3 — WASM package of Block B

### 3.1 Crates

```
crates/consumer     strk20-consumer  (§0.4.1) — wasm-clean Block B core
crates/client-wasm  strk20-engine    — cdylib, wasm-bindgen facade + MemStore
```

`crates/client-wasm` deps, **with the features pinned rather than implied**
(review finding 11a — as previously written the crate tripped the very
`getrandom` gate §3.9 specifies, because `chacha20poly1305`'s default feature
set pulls `getrandom` in through `aead`):

```toml
strk20-consumer      = { path = "../consumer" }
strk20-feed          = { path = "../feed", default-features = false, features = ["mpt"] }
                     # NOT "compress" — zstd-sys has no wasm backend (given)
discovery-core       = { workspace = true, default-features = false }   # §3.8
serde_json           = { version = "1", default-features = false, features = ["alloc"] }
chacha20poly1305     = { version = "0.10", default-features = false, features = ["alloc"] }
hkdf                 = { version = "0.12", default-features = false }
sha2                 = { version = "0.10", default-features = false }
wasm-bindgen         = "0.2"
```

No tokio, no reqwest, no rusqlite, no `getrandom`, no `web-sys`
network/storage features. **`default-features = false` is load-bearing on every
RustCrypto line, not tidiness**, and leg **s** asserts the *feature-resolved*
dependency graph (`cargo tree -e features --target wasm32-unknown-unknown`),
not merely the set of crate names — a name-only walk cannot see a feature-flag
regression, which is exactly how this one would have shipped.

### 3.2 The in-memory view

```rust
pub struct MemStore {
    identity:  FeedIdentity,                       // chain_id, pool, genesis_block, epoch_size
    base:      BTreeMap<[u8;32], SlotRec>,         // folded from snapshot + epochs
    base_blocks: BTreeMap<u64, BlockMeta>,         // ≥ history_floor only
    base_events: Vec<EventRec>,                    // ≥ history_floor, (block, event_index) ascending
    tail:      BTreeMap<[u8;32], SlotRec>,         // folded from head.ndjson; replaced wholesale
    tail_blocks: BTreeMap<u64, BlockMeta>,
    tail_events: Vec<EventRec>,
    chain:     ChainCursor,   // last_epoch, last_epoch_hash, last_epoch_to,
                              // history_floor, head, l1_accepted, snapshot_basis
}
struct SlotRec { value: Felt, write_block: u64 }

pub struct MemView<'a> { store: &'a MemStore, bound: u64 }
// impl RawStorageAccess + RawEventAccess with futures that are Ready by construction
```

Reads at `bound ≤ last_epoch_to` consult `base` only; reads at `head` consult
tail-then-base. `MemStore` holds only plain data, so `Send` is satisfied without
`SendWrapper` (which stays available as the base §7.6 escape hatch).

**Execution model.** `discovery-core`'s entry points are `async`, but over
`MemView` no future ever suspends. They are driven by

```rust
fn drive<F: Future>(f: F) -> F::Output {
    f.now_or_never().expect("engine future pended over an in-memory view")
}
```

— a panicking programming-error tripwire, never a runtime path, and never
`block_on` (R-J). Any future dependency that could actually pend trips it on
the first test run rather than hanging a browser tab.

`apply_head` replaces the tail wholesale and detects contradiction exactly as
the shipped `FeedStore` does (including the mid-sync
`tail_from > last_epoch_to + 1` bail). Because nothing tail-derived is ever
exported (§3.5) and per-key durable state is checkpoint-only (§3.6), the
browser needs **no persisted reorg logic at all** — proven mechanically by
leg **r**.

### 3.3 Exported ABI (exact)

`wasm-bindgen`, `--target web` (plus a `nodejs` build for tests). Every
fallible method throws a `JsError` whose message is one canonical JSON object
(§3.7). All inputs are bytes or JSON strings; all outputs are JSON strings or
`Uint8Array`.

```rust
#[wasm_bindgen]
pub struct Engine { /* MemStore */ }

#[wasm_bindgen]
impl Engine {
    /// genesis_json = the fetched /feed/genesis.json bytes. Pins identity.
    #[wasm_bindgen(constructor)]
    pub fn new(genesis_json: &str) -> Result<Engine, JsError>;

    /// Restore from a persisted state blob (§3.5). Verifies trailer + stamp
    /// against `genesis_json`. Never partially loads.
    pub fn load(blob: &[u8], genesis_json: &str) -> Result<Engine, JsError>;

    /// {"chain_id","pool","last_epoch","last_epoch_hash","last_epoch_to",
    ///  "history_floor","snapshot_basis","head","l1_accepted","slots","events",
    ///  "engine_version"}
    pub fn info(&self) -> String;

    /// Arbitrate staleness against a freshly fetched manifest — ALL of it, in
    /// Rust. "ok" | "behind" | "diverged".
    pub fn check_manifest(&self, manifest_json: &str) -> Result<String, JsError>;

    /// UNCOMPRESSED snapshot payload + its manifest "snapshot" object + the
    /// anchor sidecar JSON. Runs §1.5 rings 2–5 inside the module. Empty state
    /// only (SNAPSHOT_NOT_EMPTY otherwise).
    /// {"applied_epoch":1405,"basis":14059999,"slots":48123,
    ///  "history_floor":14060000,"verified":"anchored"|"server-asserted",
    ///  "state_changed":true}
    /// `state_changed` is the field §4.3's export rule reads; it was missing
    /// from this method alone (review finding 14c).
    pub fn apply_snapshot(&mut self, payload: &[u8], manifest_snapshot_json: &str,
                          anchor_json: &str) -> Result<String, JsError>;

    /// UNCOMPRESSED epoch payload + its manifest "epochs[i]" object. Verifies
    /// content sha256, header identity + range, prev-linkage. Must be
    /// last_epoch + 1 (FEED_EPOCH_GAP otherwise).
    pub fn apply_epoch(&mut self, payload: &[u8], manifest_entry_json: &str)
        -> Result<String, JsError>;   // {"applied":e,"state_changed":true}

    /// head.ndjson bytes. {"head","l1_accepted","tail_rewound"}
    pub fn apply_head(&mut self, payload: &[u8]) -> Result<String, JsError>;

    /// Epoch-derived state ONLY; the tail is never exported (§3.5).
    /// Call only when an apply reported state_changed.
    pub fn export(&self) -> Vec<u8>;

    /// THE ONLY key-accepting entries are this and export_reference_cursor.
    /// One full pass for one owner: checkpoint pass at last_epoch_to, live
    /// pass at head, spent refresh — the two-pass structure of sync_once.
    /// `key` is zeroized in place before return. `entropy32` MUST be 32 fresh
    /// bytes from crypto.getRandomValues on EVERY call (§3.6).
    pub fn discover(&mut self, owner_hex: &str, key: &mut [u8],
                    sealed: Option<Vec<u8>>, entropy32: &[u8])
        -> Result<DiscoverOut, JsError>;

    /// Paged tx history per §1.1's paging contract. Returns
    /// {"transactions":[…],"complete":bool,"complete_from":<block>,
    ///  "registration_available":bool}. A walk that crosses history_floor
    /// TERMINATES the page set; it does not throw. An explicit
    /// `from_block < history_floor` does throw HISTORY_UNAVAILABLE.
    /// `sealed` is Option for symmetry with `discover` — a first call with no
    /// prior blob is legal and does fresh discovery (review finding 14f).
    pub fn history(&self, owner_hex: &str, key: &mut [u8],
                   sealed: Option<Vec<u8>>,
                   from_block: u64, limit: u32) -> Result<String, JsError>;

    /// Reference-schema DiscoveryCursor JSON (base §7.4 interop) extracted
    /// from a sealed blob — migration to compat/SDK without resync.
    pub fn export_reference_cursor(&self, key: &mut [u8], sealed: &[u8])
        -> Result<String, JsError>;
}

#[wasm_bindgen(getter_with_clone)]
pub struct DiscoverOut {
    pub report_json: String,   // strk20-consumer SyncReport, field-identical to
                               // `strk20-sync sync --json` (+ history_from)
    pub sealed: Vec<u8>,       // checkpoint-only sealed blob; hand back next time
    pub added_json: String,    // notes not present in the supplied sealed blob
    pub spent_json: String,    // nullifiers that flipped to spent this pass
}
```

Why one `discover` rather than the roadmap sketch's per-flow calls: a wallet
wants one call per feed change. Incoming/outgoing, the checkpoint/live split,
cursor re-opening and spent refresh are internal mechanics, and `added`/`spent`
deltas are computed here so the TS wrapper — the least testable layer — holds
no discovery-adjacent logic at all. `check_manifest` is the same principle for
staleness.

Key handling: the 32-byte BE key enters linear memory, is copied into
`SecretFelt` (zeroize-on-drop), and the staging buffer is zeroized before
return. Honest limit, printed verbatim in the npm README: JS cannot reliably
zeroize its own buffers; the guarantee is **non-transmission** — the module
never writes the key anywhere and zeroizes what it owns — not memory hygiene
in the host.

### 3.4 Note on decompression

zstd is not compiled in (given). The module receives **uncompressed** payloads
and hashes them; content identity is over uncompressed bytes everywhere, so
nothing is lost. TypeScript decompresses and is bound by R-I (verify the `zst`
hash first, cap the output).

### 3.5 State blob (`export` / `load`)

Canonical NDJSON in the one grammar family (C3), debuggable with `jq`, with a
mandatory self-hash:

```
{"t":"hdr","v":1,"kind":"strk20-state","chain_id":"SN_MAIN","pool":"0x…","genesis_block":8978970,"epoch_size":10000,"engine":"<crate semver>","last_epoch":1406,"last_epoch_hash":"<64-hex>","last_epoch_to":14069999,"history_floor":14060000,"snapshot_basis":14059999}
{"t":"s","k":"0x…","v":"0x…","w":14031234}                      # slots as of last_epoch_to, ascending by 32-byte BE slot; w ≤ last_epoch_to
{"t":"b","b":14061200,"h":"0x…","p":"0x…","ts":1720000000}      # blocks in [history_floor, last_epoch_to], ascending
{"t":"ev","b":14061200,"i":0,"x":2,"h":"0x<tx>","K":["0x…"],"D":["0x…"]}  # events in [history_floor, last_epoch_to], (b,i) ascending
{"t":"end","slots":N,"blocks":P,"events":M,"sha256":"<64-hex over all preceding bytes>"}
```

**The example was wrong and is corrected (review finding 10).** §1.7 sets
`history_floor = header.block + 1`, so for `snapshot_basis = 14059999` the floor
is **14060000**, not 14050000 (which is epoch 1405's `from`). The example is the
artifact an implementer copies, so it now shows a *coherent* client — one that
applied the snapshot at basis 14059999 and then epoch 1406 — rather than a
combination no run can produce. Note the degenerate case this exposes and which
the old example hid: a client that has applied **only** the snapshot has
`history_floor = last_epoch_to + 1`, so `[history_floor, last_epoch_to]` is
empty and the blob has **zero** `b` and `ev` lines. That is correct, not a bug.

**Bounds are now written into the format, not only into the prose.** §3.5's
"only epoch-derived state is ever exported; the tail lives and dies in memory"
was the sole statement of the upper bound, and leg **r**'s whole assertion (the
blob is byte-identical across a tail fork) depends on it. `load`'s structural
checks therefore gain, as hard rejects (`FEED_MALFORMED`):

- **no line in the blob references a block `> last_epoch_to`** — slots' `w`,
  block lines' `b`, event lines' `b`. This is the parser-level form of "the tail
  is never exported", so leg **r** is pinned by the grammar and not only by a
  byte comparison that happens to pass;
- no `b`/`ev` line references a block `< history_floor`;
- `snapshot_basis` is either absent (replayed mirror, `history_floor == 0`) or
  satisfies `history_floor == snapshot_basis + 1`.

The header is the **compatibility stamp** of discussion §7 made executable:
format version, chain identity, engine major, and the hash of the last applied
epoch. `load` rejects, never partially applies: bad trailer hash →
`STATE_CORRUPT`; `v` ≠ 1 or engine major mismatch → `STATE_VERSION`;
`chain_id`/`pool`/`genesis_block`/`epoch_size` ≠ the passed genesis →
`STATE_FOREIGN`. `check_manifest` then arbitrates content staleness against the
live feed (`ok` / `behind` / `diverged`).

**Only epoch-derived state is ever exported.** The tail lives and dies in
memory, which is what makes the blob un-stale-able by a reorg. Per-key material
is never in this blob.

### 3.6 Sealed per-key state (checkpoint-only)

Per-key artifacts are key-derived and therefore a fingerprint on a shared
machine (discussion §7). The module seals them itself, so the wrapper and
IndexedDB only ever see uniform noise.

```
blob = "S20SEAL1" (8 bytes ASCII) ‖ nonce(24) ‖ XChaCha20-Poly1305 ciphertext

key   = HKDF-SHA256(ikm  = viewing key, 32-byte BE,
                    salt = "strk20-seal-key-v1",
                    info = chain_id ‖ 0x00 ‖ pool_hex ‖ 0x00 ‖ owner_hex)
nonce = HKDF-SHA256(ikm  = entropy32,
                    salt = "strk20-seal-nonce-v1",
                    info = counter_be8)[0..24]
aad   = "S20SEAL1" ‖ chain_id ‖ 0x00 ‖ pool_hex
plaintext (canonical JSON):
{"v":1,"counter":<u64>,"prev_entropy_h":"<64-hex sha256 of the entropy32 that sealed this blob>",
 "ckpt_at":<block ≤ last_epoch_to>,
 "in_ckpt":<reference DiscoveryCursor JSON>,"out_ckpt":<reference DiscoveryCursor JSON>,
 "notes":[{"note_id","owner","sender","token","index","nullifier","amount","block","spent"},…]}
```

**Checkpoint-only (C4):** no live cursors, no generation counter, nothing bound
to the tail. The live pass reruns from the checkpoint every slice, in memory.

**Nonce discipline (C2, corrected by review finding 1).** The derivation is
unchanged in shape — HKDF whitens possibly-biased caller entropy and gives the
nonce its own domain separation — but the *claim* attached to it is replaced:

> **Nonce safety comes from the caller's entropy, not from the counter.**
> `entropy32` MUST be 32 fresh bytes from `crypto.getRandomValues` on every
> `discover()` call. `counter` is a rollback/authenticity signal inside the
> AEAD plaintext and nothing more.

Why the old structural claim does not hold: `counter` is monotone only along a
single non-forking chain of blob updates, and the spec deliberately supports
forking — §4.3's "without Web Locks, last-writer-wins is safe", §4.4 quirk 3's
evicted store, and a tab that crashes after `discover` but before the IDB write.
Two forks read counter *N*, both derive *N+1*, and under identical stale entropy
both produce the same nonce over different plaintexts (different `ckpt_at`,
different `notes[]`). That is XChaCha20-Poly1305 `(key, nonce)` reuse over the
reference cursors, note ids and nullifiers the seal exists to hide.

**Structural guard, stated at exactly its real strength.** The plaintext carries
`prev_entropy_h = sha256(entropy32 used for this blob)`. On re-seal, the module
compares `sha256(entropy32)` against the decrypted `prev_entropy_h` and raises
`ENTROPY_REUSED` on equality, refusing to seal. This closes the
**constant-entropy** case completely, including across a fork (both forks read
the same `prev_entropy_h` and both refuse). It does **not** close the case of two
forks that each cache a *different* stale value and happen to supply the same one
— that requires fresh entropy, which is the contract. The guard is
defence-in-depth against the misuse the maintainer flagged; it is not a
substitute for `getRandomValues`, and no text in this spec may claim otherwise.

`entropy32` that is not exactly 32 bytes is `ENTROPY_INVALID` (never silently
padded or truncated).

Acceptance assertions (leg **q**), written so they can actually fail:

- two `discover()` calls fed the **same** prior sealed blob and the **same**
  `entropy32` (the forked path) — the module raises `ENTROPY_REUSED`; with the
  guard disabled in a test-only build, the two nonces are asserted **equal**, so
  the guard is proven to be the thing preventing the collision rather than an
  accident of the derivation;
- two `discover()` calls fed the same prior blob and **different** `entropy32`
  produce different nonces;
- a purity check that the wrapper never caches, reuses or derives `entropy32` —
  every call site is a fresh `crypto.getRandomValues(new Uint8Array(32))`,
  asserted by a scanner over the wrapper source and by a spy in the Node build.

The old unit test ("nonce inequality across seals under fixed entropy" on the
sequential path) is retained but explicitly labelled **non-sufficient**: it
passes on the sequential path and cannot see the forked path, which is why the
first assertion above exists.

An AEAD failure (wrong key, tampered store, cross-network reuse caught by the
AAD) is treated as **no cursor**: fresh discovery, with
`details.cursor_reset = true` surfaced so the wrapper can log it. Correct
behavior for "different user on the same origin".

Cursors use the exact reference JSON schema (base §7.4), so sealed-state
cursors round-trip with compat and `serve` wire cursors, and
`export_reference_cursor` delivers Tier-0 migration without resync in the
browser.

### 3.7 Error model

Every throw is `{"code":"<SCREAMING_SNAKE>","message":"…","details":{…},"retryable":bool}`.
Closed set, shared verbatim by `strk20-feed` (`FeedError` maps 1:1 onto the
`FEED_*` and `SNAPSHOT_*` codes), the wasm module, npm, and `serve`:

| code | details | retryable | raised by |
|---|---|---|---|
| `FEED_HASH_MISMATCH` | `{artifact, epoch?, expected, actual}` | no | any content check |
| `FEED_CHAIN_BROKEN` | `{epoch, expected_prev, actual_prev}` | no | epoch apply |
| `FEED_MALFORMED` | `{artifact, line?, detail}` | no | any parser |
| `FEED_EPOCH_GAP` | `{expected, got}` | no | epoch apply |
| `FEED_ADVANCED_MIDSYNC` | `{tail_from, floor}` | **yes** | head apply (manifest/head race) |
| `DECOMPRESS_LIMIT` | `{artifact, cap}` | no | TS/Rust decompression |
| `SNAPSHOT_ROOT_MISMATCH` | `{computed, header, anchor}` | no | §1.5 ring 5 |
| `SNAPSHOT_ANCHOR_MISSING` | `{e}` | no | manifest check |
| `SNAPSHOT_NOT_EMPTY` | — | no | apply_snapshot |
| `BOUND_BELOW_SNAPSHOT` | `{bound, basis}` | no | view construction |
| `CHAIN_MISMATCH` | `{field, expected, got}` | no | every identity check |
| `STATE_CORRUPT` / `STATE_VERSION` / `STATE_FOREIGN` | stamp fields | no | `load` |
| `SEALED_STATE_MISMATCH` | `{cursor_reset:true}` | no | sealed open (non-fatal) |
| `KEY_INVALID` | — | no | `discover` |
| `ENTROPY_INVALID` | `{expected:32, got}` | no | `discover` (entropy32 length) |
| `ENTROPY_REUSED` | — | no | `discover` (§3.6 `prev_entropy_h` guard) |
| `DISCOVERY_INCOMPLETE` | `{passes}` | no | pass budget exhausted |
| `HISTORY_UNAVAILABLE` | `{floor}` | no | history below the floor |
| `INTERNAL` | scrubbed | no | panic hook |

**`STATE_STALE` is deleted from the set (review finding 13).** Staleness is a
**return value, never a throw**: `check_manifest` returns `"ok" | "behind" |
"diverged"`, §4.3's flow switches on it, and leg **q** asserts the three
discriminants. A blob that is unusable rather than merely stale already has its
code (`STATE_CORRUPT` / `STATE_VERSION` / `STATE_FOREIGN`, all from `load`).
Nothing in this spec may reintroduce a thrown staleness error: a throw and a
discriminant are different TypeScript control flow and different
`DiscoveryEvent` emission, and the two cannot both be normative.

Codes added by other layers: `TRANSPORT` (npm, retryable), `CONFIG_INVALID`
(npm, a constructor option the shipped build does not implement — see §4.5),
`INVALID_QUERY` (SSE 400), `AUTH_REQUIRED` / `INVALID_TOKEN` / `INVALID_BODY` /
`SERVICE_UNAVAILABLE` / `BLOCK_REORGED` (serve + compat). Nothing else is ever
thrown across a boundary. `FEED_HASH_MISMATCH` names the epoch and both hashes
so the wasm, npm and Rust wordings are one vocabulary. Every `message` and
`details` value is asserted key-clean by the scanner (leg **q**).

### 3.8 Consuming the fork until the upstream PR lands

Given: `starknet-providers` is declared but unused in `discovery-core` at rev
`74841ca`; feature-gating it is a two-line `Cargo.toml` change (roadmap item
7).

1. Fork `starkware-libs/starknet-privacy` under our org; branch
   `strk20/providers-gate-74841ca` = the pinned rev **plus exactly one commit**
   touching only `discovery-core/Cargo.toml` (`optional = true`,
   `[features] default = ["providers"]`,
   `providers = ["dep:starknet-providers"]`).
2. **One** workspace-wide
   `[patch."https://github.com/starkware-libs/starknet-privacy.git"]` entry
   pinning the fork by rev, with the feature **default-on** so native builds
   are behaviorally identical to today; `crates/client-wasm` and
   `crates/consumer` set `default-features = false`. A split pin (wasm crates
   on the fork, native crates on upstream) is **forbidden**: two sources for
   one git dependency in one workspace yields two `discovery-core`/`Felt` type
   identities — precisely the silent trait-bound failure base §3's CI deny
   exists to prevent.
3. The diff is vendored at `patches/discovery-core-providers-gate.patch`. CI
   job `fork-delta-check` asserts, on every run: the fork rev equals the
   upstream rev plus that patch, **and**
   `git diff <upstream>..<fork> -- crates/discovery-core/src` is EMPTY — Cargo
   metadata only, zero source lines. This is the mechanical form of "consumed
   UNMODIFIED".
4. The upstream PR is filed at implementation step 0b, in parallel with the
   refactor, so the fork's clock starts immediately. On merge, the `[patch]`
   section and `patches/` file are deleted in one commit and the CI job inverts
   into a tripwire that fails if the `[patch]` section ever returns.

### 3.9 Purity and size gates (CI, run with the suite)

- **Feature-resolved dependency walk** (not a lockfile name walk — review
  finding 11a): `cargo tree -e features -p strk20-consumer` and
  `-p strk20-engine --target wasm32-unknown-unknown` must not reach `tokio`,
  `reqwest`, `rusqlite`, `getrandom`, or `web-sys` network/storage features.
  The resolved tree is checked in and diffed in CI, so a transitive default
  feature that re-enables `getrandom` is a red build rather than a silent
  reopening of C2.
- **`unsafe` posture, corrected (review finding 11b).**
  `crates/consumer` is pure Rust and keeps `#![forbid(unsafe_code)]`.
  **`crates/client-wasm` carries `#![deny(unsafe_code)]`, not `forbid`**:
  `#[wasm_bindgen]` expansion emits `unsafe` items (the generated `__wbg_*_free`
  externs and ABI shims), and `forbid` — unlike `deny` — cannot be lifted by an
  `allow` inside macro-generated code, so the cdylib as previously specified
  would not compile at all. Exactly one documented `#[allow(unsafe_code)]`
  scope is permitted, on the `#[wasm_bindgen]` facade module; CI asserts the
  count is one and that the crate contains no hand-written `unsafe` block.
- **Import-section audit**: `wasm-objdump -j Import -x` on the release module,
  compared against a checked-in `crates/client-wasm/import-allowlist.txt` of
  permitted `(module, field)` pairs, diffed in CI. The allowlist is frozen by
  that file rather than by a name-pattern judgement, because `__wbg_*` import
  names are derived from the JS API being bound and a pattern match drifts open
  (review finding 12).

  **The property, restated at its real strength.** The old wording — "the
  allowlist is wasm-bindgen glue only… *the module cannot leak what it cannot
  call*", with C2 pricing an ABI parameter on an "absolutely empty" import
  section — overclaimed twice. The section is not empty: `__wbindgen_string_new`
  / `__wbindgen_throw` / `__wbg_*` are calls **into JS carrying arbitrary
  strings**, and they are how every ABI method returns its JSON. What the audit
  actually proves is: **the module cannot open a network handle, a storage
  handle, a timer or a randomness source of its own.** It can only hand bytes to
  the wrapper — and the wrapper is what legs **q** and **u** scan. The entropy
  parameter is justified on the merits recorded in C2 (dependency graph,
  determinism), not on emptiness.
- **Size budget — one denominator (review finding 17).** §4.1 counted module +
  glue + fzstd against the same 300 KB that this section and leg **s** applied
  to the module alone, a ~23 KB disagreement in the number integrators quote.
  The single denominator is **total published wire cost**: gzip of
  `engine_bg.wasm` + the wasm-bindgen glue + `fzstd`, i.e. what a consumer
  actually downloads. Both this section and leg **s** gate that figure and
  nothing else.

  The **budget value is not settable from the spike.** 300 KB was derived from
  the 231 KB spike baseline, which predates codec + mpt + AEAD + `serde_json` +
  the ABI. It is therefore recorded as a FILL-IN, set from a measured number at
  the end of step 4 and defended thereafter:

  > **FILL-IN (size budget, pending step 4):** measured total wire cost = ___ KB
  > gzip (wasm ___ + glue ___ + fzstd ___). **Budget = measured + 20 %**, entered
  > here and in leg **s**. Date: ___.

  Until it is filled in, the provisional gate is 300 KB and a breach is a review
  event, not silent creep.
- **Compile-fail lock**: the module exposes exactly two key-accepting entries
  (`discover`, `export_reference_cursor`) and no method taking a transport-ish
  type — trivially true because no network type exists in the crate; the lock
  is the wasm32 build plus the dependency-graph test above.

---

## A4 — npm package

### 4.1 Name and layout

Unscoped **`strk20-discovery`** (unanimous; amends base **§12.1**, which named
it `@strk20/discovery-provider`). ESM + `.d.ts`, built with `tsc`, no bundler.

```
strk20-discovery
├── dist/index.js|d.ts        KeylessClient, DelegatedClient, types, errors
├── dist/sdk.js|d.ts          LocalDiscoveryProvider  (subpath "strk20-discovery/sdk")
├── dist/worker.js|d.ts       ~40-line worker host    (subpath "strk20-discovery/worker")
├── dist/engine_bg.wasm       strk20-engine, lazily instantiated
└── README.md
```

`node >= 20` and evergreen browsers. The wasm loads via
`new URL('engine_bg.wasm', import.meta.url)` — untouched in Vite, webpack 5 and
Next; a `wasmUrl` option covers exotic setups. No inline-base64 entry (a second
distribution artifact to version and test). Wire cost = wasm + glue + fzstd,
gzipped — the **one** denominator §3.9 gates (review finding 17). The
"~255 KB" arithmetic previously printed here assumed the module stayed at the
231 KB spike baseline even though §3.9 anticipates growth from codec + mpt +
AEAD + `serde_json` + the ABI; it is withdrawn in favour of §3.9's measured
FILL-IN, and no number is quoted to integrators before step 4 measures one.

Supply-chain posture: **no install scripts**; npm provenance publishing;
`files` whitelist; the wasm module's sha256 printed in the README and asserted
in CI; the only runtime dependency is `fzstd` (pinned exact).

The SDK adapter ships **inside** the same package (`/sdk`): one install, both
audiences. All base §12.1 cursor-conversion semantics carry over verbatim, so
`NotesCursor`/`ChannelCursor` round-trip identically to
`IndexerDiscoveryProvider`.

### 4.2 One interface, two clients

```ts
export interface KeyRef {
  address: `0x${string}`;
  viewingKey: Uint8Array;          // 32-byte BE. Uint8Array ONLY — see note.
}

export interface Note {
  token: string; index: number; noteId: string; nullifier: string;
  amount: bigint; blockNumber: number; sender: string; spent: boolean;
}

export interface NotesResult {
  notes: Note[]; balances: Map<string, bigint>;
  head: number; l1Accepted: number; complete: boolean;
  historyFrom: number; snapshotRejected: boolean;
  raw: unknown;                    // the untouched SyncReport JSON (oracle equality)
}

export type DiscoveryEvent =
  | { type: 'notes';  added: Note[]; spent: Note[]; head: number }
  | { type: 'reorg';  rewoundTo: number }                     // epoch floor
  | { type: 'status'; state: 'live' | 'polling' | 'degraded' }
  | { type: 'error';  error: Strk20Error; recovering: boolean };

export interface DiscoveryClient {
  getNotes(k: KeyRef): Promise<NotesResult>;
  subscribe(k: KeyRef, cb: (ev: DiscoveryEvent) => void): () => void;   // unsubscribe
  history(k: KeyRef, opts?: {fromBlock?: number; limit?: number}):
      Promise<{ transactions: HistoryTx[];
                complete: boolean;              // §1.1 paging contract
                completeFrom: number;           // walk's last completed bound, ≥ historyFrom
                registrationAvailable: boolean }>;
  status(): ClientStatus;   // {mode:'keyless'|'delegated', transport:'sse'|'polling',
                            //  head, l1Accepted, lastEpoch, historyFrom,
                            //  verified: 'anchored'|'server-asserted'|'replayed',  // §1.5.1
                            //  persistence:'indexeddb'|'memory'}
  close(): Promise<void>;
}

export class KeylessClient implements DiscoveryClient {
  constructor(opts: {
    feedUrl: string;
    network?: 'mainnet' | 'sepolia' | ChainProfile;   // default 'mainnet' (§A6, C18)
    coldStart?: 'auto' | 'snapshot' | 'epochs';       // default 'auto' — ONE vocabulary
    persistence?: 'indexeddb' | 'memory' | StorageAdapter;  // default 'indexeddb'
    persist?: 'raw' | 'folded';                       // narrowed at publish; see §4.5
    live?: boolean;                                   // default true
    pollIntervalMs?: number;                          // default 30_000
    worker?: boolean;                                 // default true (C14)
    anchorRpcUrl?: string;                            // enables §1.5 ring 6
    requestPersistentStorage?: boolean;
    wasmUrl?: string | URL;
    fetch?: typeof fetch;
  });
}

export class DelegatedClient implements DiscoveryClient {
  constructor(opts: { serverUrl: string; authToken?: string;
                      network?: 'mainnet' | 'sepolia' | ChainProfile;
                      assertUncheckedNetwork?: boolean;   // see §4.8
                      pollIntervalMs?: number; fetch?: typeof fetch });
}

export class Strk20Error extends Error {
  code: string;                        // the §3.7 closed set
  details?: Record<string, unknown>;
  retryable: boolean;
}
```

`viewingKey` is `Uint8Array` only. Accepting a hex string would create
unzeroizable copies and make the honest-zeroization statement cover nothing;
the type refuses the footgun rather than documenting it. `KeyRef` bundles the
address because upstream discovery is (address, key)-parameterized — hiding
that would be a lie.

**One cold-start vocabulary across both languages (review finding 14a).** The
three surfaces named the same three modes three ways — `strk20-sync sync
--cold-start auto|snapshot|epochs` (§1.7), `strk20-sync serve --cold-start
epochs|snapshot` (§5.5), and npm `'auto'|'snapshot'|'genesis'` (above), where
`epochs` and `genesis` were the same mode. The vocabulary is now
**`auto` | `snapshot` | `epochs`** everywhere, including §1.1's escape-hatch
sentence; `serve` continues to accept only the two modes that make sense for it
(`epochs` default, `snapshot` allowed, no `auto`), using those names. §6.1's
"one profile source" doctrine exists to stop exactly this kind of Rust/TS drift,
and a vocabulary is as much a constant as a chain id: leg **t** asserts the
accepted mode strings are identical in the two halves.

Switching keyless ↔ delegated is a constructor swap; leg **v** asserts
deep-equal results from both against the same fixture.

Worker (C14): on by default. The key crosses to the worker by **ArrayBuffer
transfer**, detaching the caller's buffer, so exactly one copy is in flight and
the module zeroizes it; `worker: false` runs on the main thread. The `/worker`
subpath ships the recipe as code — advice without code never gets followed.

### 4.3 Keyless data flow

```
open IDB (or memory fallback)
fetch genesis → byte-compare vs meta.genesis (CHAIN_MISMATCH on disagreement, §4.4)
             → Engine.new(genesis) or Engine.load(stateBlob, genesis)
fetch manifest → Engine.check_manifest        # RETURNS a discriminant; never throws
   "ok"       → nothing to apply
   "behind"   → fetch+apply epochs last_epoch+1..
   "diverged" → drop persisted state, cold start
cold start   → snapshot .zst → verify zst hash → fzstd → apply_snapshot(+anchor)
fetch head (ETag) → apply_head
per identity → sealed = IDB.cursors[keyId] → Engine.discover(addr, key, sealed, entropy32)
             → store new sealed → emit added/spent deltas
subscribe()  → EventSource /feed/live → on head/epoch/snapshot repeat the slice
             → on error, poll fallback (§2.5)
export()     → ONLY when an apply reported state_changed (epoch cadence ~4.7 h),
               never on head events
```

All sync passes run under `navigator.locks.request('strk20:<db>', …)` so tabs
serialize; without Web Locks, last-writer-wins is safe **for key-independent
state** because every persisted value is self-verifying (blobs carry a self-hash
and a stamp; epochs are re-hashed) — stated and tested.

**Scope correction (review finding 1).** That safety argument covers `meta`,
`artifacts` and `state`. It does **not** extend to `cursors`: the sealed blob is
an AEAD ciphertext, and two tabs that fork from the same prior blob are exactly
the nonce-collision case §3.6 addresses. Forking there is safe only because
every `discover()` call supplies fresh `crypto.getRandomValues` entropy, with
`ENTROPY_REUSED` as the backstop against a caller that does not. Web Locks
reduce the frequency of the fork; they are not what makes it safe, and no
implementation may treat them as the mitigation.

### 4.4 IndexedDB layout

Database name `strk20-discovery:<chain_id>:<pool>` — **per-chain-and-pool
database**, so cross-network confusion is impossible rather than detected and
a schema migration never touches two chains at once. Version 1:

| store | key | value |
|---|---|---|
| `meta` | string | `format_v`, `last_epoch`, `last_epoch_hash`, `snapshot_e`, persist mode, **`genesis` (the raw `/feed/genesis.json` bytes)** |
| `artifacts` | `"snapshot"` \| `"anchor"` \| epoch idx (number) | `{hash: string, zbytes: ArrayBuffer}` — compressed **exactly as served** |
| `state` | `"folded"` | `ArrayBuffer` — `Engine.export()` blob (Design M only) |
| `cursors` | `keyId` (hex string) | `{sealed: ArrayBuffer, updatedAt: number}` |

`keyId = hex(HKDF-SHA256(ikm = viewingKey, salt = "strk20-idb-keyid-v1",
info = chain_id ‖ pool ‖ owner))` — the **full 32-byte HKDF output rendered as
64 lowercase hex characters, no slice**. The previous `[0..32]` was ambiguous
between 32 hex characters (128 bits) and 32 bytes of a 32-byte output (the whole
thing), and the two readings give different row keys in the Rust and TS halves
(review finding 14b). Unguessable without the key (R-K).

**`genesis` is persisted AND re-fetched (review finding 15).** Both
`Engine.new(genesis_json)` and `Engine.load(blob, genesis_json)` require the
genesis document and `load` compares the blob's stamp against it, but the `meta`
store held none of it, so leg **u**'s asserted reload delta of `{manifest, head}`
was unachievable — the client must fetch `/feed/genesis.json` on every reload,
and reconstructing it from the listed `meta` rows is impossible. Both halves of
the fix ship, because they buy different things:

- the raw bytes are **stored**, so `load` has a stamp source that does not
  depend on the network being reachable;
- the document is **re-fetched every session** and byte-compared against the
  stored copy, mismatch ⇒ `CHAIN_MISMATCH` before any row lands. This is the
  stronger property and the reason to pay for the request: it catches **a feed
  that changes its own genesis**, which a stored-only copy would never see and
  which §6.3's first-contact profile check alone does not cover on later
  sessions.

Leg **u**'s reload delta is therefore `{genesis, manifest, head}` + SSE, and
that is what the leg now asserts (§8).

Never stored: `head.ndjson` bytes, the head ETag, anything tail-derived — the
no-persisted-reorg-logic property is enforced by the schema having nowhere to
put a tail. Documented residual metadata: row existence, sizes, timestamps.

Quirks engineering, each with a test:

1. IndexedDB transactions auto-commit at microtask end — never `await fetch`
   inside a transaction; stage bytes first, write in one transaction.
2. `open` can throw synchronously or fire `onblocked` (private windows,
   eviction, another tab mid-upgrade) — every path falls back to
   `persistence: 'memory'` and reports it through `status()`.
3. Eviction is normal: an empty store is a cold start, never corruption.
4. Multi-tab: Web Locks when present, safe without it (see §4.3).
5. Safari first-write latency: the initial persist happens after `getNotes`
   resolves, never on the critical path.

### 4.5 Persistence: both designs, one gate

**Design R — raw artifacts are the persisted truth (the default lane).**
Persist `artifacts` + `cursors` + `meta`. Every load re-runs the full
verification ladder over stored bytes and refolds. A tampered or corrupted row
fails its hash and is refetched: local storage is never trusted, only
network-equivalent bytes re-verified per load. Zero cache coherence, zero reorg
logic, one source of truth.

**Design M — folded-mirror cache over R.** Additionally persist
`Engine.export()` into `state` after an apply reports `state_changed` (epoch
cadence — never per head poke; discussion §7's explicit hazard). Load:
`Engine.load` → `check_manifest`; `"ok"`/`"behind"` skips all folding;
`"diverged"` or any `STATE_*` error deletes the record and falls through to R,
then to the network. Strictly a cache: deleting it is always correct.

Honest trust statement, printed in the README where M ships: M trusts IndexedDB
integrity between loads. No secret exists to MAC a key-independent blob, so a
same-origin attacker can alter folded values undetected until the next full
refold. The marginal risk over R is precisely *persistence of tampering beyond
the tampering code's presence*. Mitigation is architectural: an opportunistic
idle-callback full refold + byte-compare every N loads, flagging divergence.

If the gate selects R, `export`/`load` **stay in the ABI, dormant** — they cost
nothing and M is then turned on later by measurement, not by argument. There is
no `'auto'` mode (C15).

**What `persist?: 'raw' | 'folded'` means when M is not built (review finding
14g).** §4.2 published a two-member union while this section says M "is not
built" under an R verdict — a documented option that may throw. Settled two
ways, both required:

- the published union is **narrowed at publish time**. Step 5 (the gate) runs
  before step 7 (npm) by §7's order, so under an R verdict the shipped `.d.ts`
  declares `persist?: 'raw'`, and asking for `'folded'` is a compile error for
  the integrator rather than a runtime surprise;
- at runtime, an unimplemented mode arriving from untyped JavaScript is
  rejected in the constructor with `CONFIG_INVALID
  {option:'persist', got:'folded', built:['raw']}` — never silently downgraded
  to `'raw'`, because a caller that asked for a cache and got none should learn
  it at construction and not from a latency graph.

### 4.6 The fold-time measurement gate (pre-registered)

Runs, and its results are published, **before any `KeylessClient` persistence
code is written** — the discussion-§7 mandate, made binding.

- Harness `ts/strk20-discovery/bench/fold.bench.ts`, driven by Playwright.
- Inputs, checked in under `bench/fixtures/`: **L1** = the default snapshot
  lane (snapshot + epochs-after + head, recorded from the live feed at a pinned
  manifest hash); **L2** = the full-history lane (all epochs + head); **L3** =
  a synthetic 10× history from `strk20 bench synth-feed --scale 10` (headroom
  only, never a shipping trigger).
- Measurement profile (C17): **headless Chromium at 4× CPU throttle**, ≥5 runs,
  **p95** of `t_cold = decompress + verify + fold + root-verify` (network
  excluded; `t_zstd` recorded separately). One named physical mid-tier device is
  measured alongside and recorded, when available. CI runs the same bench as a
  **trend line only**.
- Decision rule, fixed now:
  - p95 `t_cold(L1)` ≤ 500 ms → **ship Design R alone**; M is not built.
  - p95 `t_cold(L1)` > 500 ms → **Design M is the default** for the snapshot
    lane.
  - Independently, p95 `t_cold(L2)` > 2000 ms → M is enabled for
    `coldStart:'epochs'` sessions regardless of L1's verdict; **L2 alone never
    makes M the default for snapshot-lane users**.
  - CI alarm: fail the build if the L1 median regresses 3× against the recorded
    baseline, so the verdict stays revisable by measurement rather than
    argument.
- Full numbers, device profiles and the decision go to
  `docs/research/fold-gate-results.md`, and the verdict is recorded here:

> **FILL-IN (fold gate, pending step 5):** `t_zstd` L1/L2 = ___ / ___ ms;
> p95 `t_cold` L1/L2/L3 = ___ / ___ / ___ ms; throttled profile = ___;
> reference device = ___. **Decision: R / M / M-for-fullHistory-only.**
> Date: ___.

### 4.7 zstd in TypeScript

**`fzstd`** — pure-JS, decompress-only, ~8 KB gzip, MIT, no native or wasm
dependency. We never compress client-side, so decompress-only is the whole
requirement. Output is **always** sha256-verified against the manifest, so a
decoder bug becomes a loud hash mismatch and never smuggled bytes; and per R-I
the `zst` hash is checked **before** fzstd sees the bytes, with a 64 MiB cap
for epochs and 256 MiB for snapshots. Exact-version pinned. One path only
(C16); `DecompressionStream('zstd')` promotion is a recorded roadmap item.

### 4.8 DelegatedClient

Speaks the **reference compat wire** (`POST /v1/sync/incoming_state`,
`/v1/sync/outgoing_state`, `/v1/sync/preflight_check`, `POST /v1/history`;
types from `crates/wire`) to either `strk20-sync serve` (§A5) or
`strk20 --enable-compat`, and by construction to any stock reference
deployment. Cursors round-trip through requests and responses in the reference
schema, exercising the base §7.4 interop guarantee from TypeScript. The README
states the trust boundary in the same words as base §9's compat row: the viewing
key travels to a server the user runs.

**`subscribe()` uses fetch-based SSE, not `EventSource` (review finding 8).**
`/feed/live` on `serve` is **inside the auth perimeter** (§5.5), and §5.7 already
says in as many words that native `EventSource` cannot send headers. The
previous text specified `EventSource` against an authenticated route, which has
only two outcomes and both are bad: `subscribe()` silently degrades to polling
on precisely the remote deployments R-F exists for, or `/feed/live` is carved
out of the perimeter and becomes the one unauthenticated route on a keyed binary
— a long-lived stream advertising the mirror's head/epoch/apply cadence to
anyone who can reach the port. So:

- `DelegatedClient.subscribe()` consumes `/feed/live` over **fetch + `ReadableStream`
  with an `Authorization: Bearer` header**, then keyed re-query on each poke.
  This is §5.7's transport shipped in v1 **for the notification plane only**:
  no capability token, no `POST /v1/watch`, no registry, nothing key-derived on
  the wire. C7's veto is not reopened — it vetoed keyed registries, and there is
  none here — and R-E is honoured, since the token rides a header and never a
  URL.
- Native `EventSource` remains the transport for the **keyless** `/feed/live`
  on the indexer (§2.1), which takes no auth and no parameters.
- Against a `serve` with no token file configured (loopback-only, the R-F
  default), the same fetch-based path runs without the header.

**Chain identity on construction, made checkable (review finding 7).** The
client reads `/health` and verifies chain identity **before sending any key**.
That check was unimplementable against two of the three named targets: the ops
`/health` body carries `{status, head, l1_accepted, lag_secs, latest_epoch,
class_hash, decode_state, verify_root_failed}` and **no `chain_id`, no `pool`**;
implementation-note 3 records that compat mode deliberately reuses that one
route (axum forbids two handlers on one path), so `strk20 --enable-compat`
cannot serve a chain-stamped health either. Left as written it would have been
implemented as "check if present" — i.e. skipped exactly where it matters, on a
stock reference deployment pointed at the wrong network. Fixed at the source:

- base **§6.2**'s ops `/health` body gains `chain_id` and `pool` (§6.2 below —
  additive, breaks no existing consumer, and it is the same one-line change the
  stamping matrix already mandates for `serve`);
- **client behaviour when the fields are ABSENT is specified, not left to the
  implementer**: `DelegatedClient` **refuses to construct**, throwing
  `CHAIN_MISMATCH {field:'chain_id', expected:<profile>, got:null}`, unless the
  caller passes an explicit `network` assertion acknowledging that the server
  cannot be checked. There is no "verify if present" mode.

```ts
export class DelegatedClient implements DiscoveryClient {
  constructor(opts: { serverUrl: string; authToken?: string;
                      network?: 'mainnet' | 'sepolia' | ChainProfile;
                      // Required to proceed against a server whose /health
                      // carries no chain_id/pool (pre-amendment binaries).
                      assertUncheckedNetwork?: boolean;
                      pollIntervalMs?: number; fetch?: typeof fetch });
}
```

### 4.9 TS e2e against the real server binary (test-first)

`crates/e2e-tests` promotes its in-process harnesses to `[[bin]]` targets so a
non-Rust process can spawn them: `fixture-rpc`, `recording-proxy`, and
`capture-scan`.

```
vitest globalSetup:
  1. cargo build (or $STRK20_PREBUILT)
  2. spawn fixture-rpc                    → :A
  3. spawn strk20 run --rpc-url :A --feed-dir tmp --listen :B
  4. spawn recording-proxy :C → :B        (capture: proxy-capture.bin)
  5. FEED_URL = http://127.0.0.1:C/feed
teardown: kill children; run capture-scan over proxy-capture.bin + idb-dump.json
```

- Node polyfills: `fake-indexeddb`, an EventSource polyfill; one Playwright
  Chromium smoke exercises real IndexedDB / EventSource / Worker against the
  same stack.
- **The scanner is not reimplemented in TypeScript.** `capture-scan` is the
  leg-d Rust scanner promoted to a bin and reused verbatim over the TS proxy
  capture and an IndexedDB dump. One scanner implementation for every capture
  surface, no port that can silently weaken. Its self-test leg is retained: the
  scanner MUST find the key in a delegated capture.
- Golden truth: the TS suite reads the **same** checked-in O2 golden JSON the
  Rust acceptance test pins — one file, byte-one, never duplicated.

CI order: `cargo build` → `cargo test -p e2e-tests` → `pnpm e2e`. No network.

---

## A5 — `strk20-sync serve`

### 5.1 Shape: a stateless keyed read head over a verified mirror

`serve` is Block B running server-side for self-hosters. It maintains a
verified mirror from any feed (HTTP, local dir, or the colocated indexer DB)
and serves the **reference compat wire** over the unmodified engine. It is
deliberately **stateless with respect to keys**: the key arrives per request,
drives one engine pass, and is dropped; cursors ride requests and responses and
are **never persisted server-side**; there is no subscription registry and no
per-key background work. Its own `serve.db` contains only the public mirror —
nothing key-derived, ever (still chmod 0600, uniformly with `sync.db`).

It links no `strk20-indexerd` code: wire types come from `crates/wire` (§0.4.2),
so base R5's dependency direction is untouched and the flagship server binary
still never sees a key by default.

### 5.2 Endpoints

```
GET  /health                    # ops shape (base §6.2, amended §6.2 below) + history_from,
                                #   feed_source, verified (§1.5.1).  UNAUTHENTICATED.
POST /v1/sync/incoming_state    # reference wire, crates/wire types
POST /v1/sync/outgoing_state
POST /v1/sync/preflight_check
POST /v1/history                # §1.1 paging contract; carries history_from
GET  /feed/live                 # the §2.2 poke stream, re-emitted after each successful local apply
```

**Auth perimeter, decided and written down (review finding 8).** §5.5 said
"`Authorization: Bearer` checked when a token file is configured" without
scoping which routes, which left `/feed/live` undecided and `DelegatedClient`'s
`subscribe()` unimplementable either way. The line:

| route | perimeter | why |
|---|---|---|
| `/v1/*` | **inside** | keyed |
| `/feed/live` | **inside** | a long-lived stream on a keyed binary; consumed with fetch-based SSE + header, never `EventSource` (§4.8) |
| `/health` | **outside** | it is how a client checks chain identity *before* sending a key (§4.8), and it carries nothing key-derived; the same body a load balancer probes |

Every keyed response carries `X-Strk20-Mode: delegated-keyed`. HTTP 409 stays
reserved exclusively for `BLOCK_REORGED`, driven by the mirror's
tail-replacement rewind exactly as compat's is.

### 5.3 Why no keyed push in v1

`serve` re-emits `/feed/live` after each local apply; `DelegatedClient` then
re-queries with the key. Latency is the same as a keyed stream, but the key is
held for **request duration** rather than connection lifetime, there is no
watch registry and no per-key scheduler, and the keyless and delegated clients
end up with the **same update-loop shape**. The base spec's never-per-user-push
policy is honored structurally instead of carefully.

### 5.4 Feed sources, including the in-process DB transport

Amends base **§7.2**. The trait becomes:

```rust
pub enum EpochPayload { Compressed(Vec<u8>), Raw(Vec<u8>) }

#[async_trait]
pub trait FeedTransport: Send + Sync {
    async fn fetch_genesis(&self) -> Result<Genesis>;
    async fn fetch_manifest(&self) -> Result<Manifest>;
    async fn fetch_epoch(&self, idx: u64) -> Result<EpochPayload>;
    async fn fetch_anchor(&self, idx: u64) -> Result<Option<Vec<u8>>>;
    async fn fetch_snapshot(&self, e: u64) -> Result<Option<EpochPayload>>;
    async fn fetch_snapshot_anchor(&self, e: u64) -> Result<Option<Vec<u8>>>;
    async fn fetch_head(&self, etag: Option<&str>) -> Result<Option<(Vec<u8>, String)>>;

    /// Declared once at construction; the cold-start branch (§1.7) consults it
    /// instead of discovering the answer through a failed fetch.
    fn snapshot_capable(&self) -> bool { true }
}
```

**`fetch_snapshot_anchor` returns `Option`, and `db:` is not a snapshot source
(review finding 6a).** As previously written the method was non-optional while
`DbTransport` has no source for it, so `--feed db: --cold-start snapshot` was
structurally impossible against a trait that declared it mandatory. Verified in
the shipped code: the full `starknet_getStorageProof` response is written only
to a sidecar **file** (`cutter.rs` writes `epochs/{idx:08}.anchor.json`); the
`epochs` table stores only the four scalar summary columns `anchor_block /
anchor_block_hash / anchor_storage_root / anchor_class_hash`, and there is no
snapshots table and no proof blob anywhere in `strk20.db`. A `db:` feed
therefore cannot produce the `contracts_proof` node set that §1.5 ring 5 walks.

Between the two available fixes, **adding a proof-blob column to the cutter's
schema is rejected** and dropping snapshot capability from `db:` is taken:
a base-spec DDL amendment to store a large, non-content-addressed,
best-effort-by-R7 blob in the indexer's hot table buys one deployment shape
(feed-dir-less **and** snapshot-cold-started) that `serve` does not even default
to — §5.5 defaults `serve` to `--cold-start epochs`. So:

- `DbTransport::snapshot_capable() == false`; `fetch_snapshot` and
  `fetch_snapshot_anchor` return `Ok(None)`;
- `strk20-sync serve --feed db:… --cold-start snapshot` is a **startup
  refusal** naming the reason, not a runtime surprise on the first empty
  mirror;
- a `None` snapshot anchor where the manifest declares a snapshot is
  `SNAPSHOT_ANCHOR_MISSING` and falls to epoch replay — the §1.5 guard,
  unchanged.

Still no method accepts an address, key, slot, or any user-derived value —
`e`/`idx` are manifest-derived, exactly like `fetch_epoch`'s index today. The
`compile_fail` doctest lock is regenerated for the new signatures in the same
commit.

Verification rule per variant (C6): `Compressed(b)` → `sha256(b) ==
manifest.zst` **first**, then bounded decompress, then content sha256;
`Raw(p)` → `sha256(p) == manifest.hash` directly. This is why the variant is
load-bearing: base §4.3 declares zstd output version-unstable, so a transport
that rebuilds and recompresses cannot promise the manifest's `zst` hash, and
under R-I that combination would be a latent hard error.

Impls:

- `HttpTransport`, `DirTransport` — unchanged, return `Compressed`.
- **`DbTransport`** (new leaf crate `crates/dbfeed`, lib `strk20-dbfeed`; deps
  `strk20-feed`, `rusqlite` — **not** `strk20-indexerd`). Opens the indexer's
  `strk20.db` read-only (`?mode=ro`, `PRAGMA query_only`) and synthesizes the
  transport surface from rows, with one non-negotiable safety property:

```rust
// fetch_epoch(idx): rows → BlockLine* → strk20_feed::codec::encode_epoch →
//     assert sha256(payload) == epochs.content_hash   ← SELF-CHECKED: synthesis
//     drift dies here, at the source                  ← then return Raw(payload)
// fetch_manifest: epochs table + meta rows → Manifest   ← NOT self-checked (see below)
// fetch_head:     rows above the epoch floor + meta → codec::encode_head
//                                                       ← NOT self-checked (see below)
// fetch_snapshot / fetch_snapshot_anchor: Ok(None) — snapshot_capable() == false
// A schema_version mismatch in meta is a startup refusal, not a best-effort read.
```

  The encoder is `strk20-feed`'s (shared, not duplicated); only DDL knowledge is
  duplicated — plus, honestly named per C5, a leaf-crate copy of the one slot-set
  query that lives at `crates/indexerd/src/db.rs::full_slot_set_as_of`. The
  downstream client ladder is completely unchanged — in-process bytes get **no
  shortcut** through verification.

  **The self-check covers epochs, and only epochs (review finding 6b).** C5's
  answer to the maintainer — "every fetch self-verifies `sha256(payload) ==
  epochs.content_hash`… serialization drift dies at the source rather than
  reaching a client" — is true for `fetch_epoch` and false for the rest.
  `head.ndjson` is **not content-addressed anywhere**: no hash for it exists in
  the manifest, in the DB, or in the client, which verifies nothing about it
  beyond the `tail_from > last_epoch_to + 1` sanity bail. The manifest likewise
  carries no hash of itself. So a head- or manifest-encoder drift in
  `strk20-dbfeed` reaches the client unnoticed. Two consequences, both binding:

  - **When a feed dir is present** (the documented default deployment,
    `--feed-dir` alongside `db:`), `DbTransport::fetch_head` and
    `fetch_manifest` byte-compare their synthesized output against the on-disk
    `head.ndjson` / `manifest.json` and refuse on divergence. This is the real
    cross-check and it is cheap, because the file is right there.
  - **Otherwise, head and manifest synthesis is UNCHECKED**, and this spec says
    so rather than letting C5's sentence imply otherwise. `db:` without a feed
    dir logs the gap at startup, and `/health` reports
    `feed_source: "db:unchecked-head"`. A `db:`-only `serve` is a convenience
    for feed-dir-less operators, not a verification-equivalent path.

**Documented default deployment:** a colocated `serve` uses
`--feed /var/lib/strk20/feed` (`DirTransport`) — in-process file reads, no HTTP
hop, full hash-chain verification, nothing duplicated. `db:` exists for
feed-dir-less setups and for operators who want the mirror without the feed
directory. Both are supported; the docs lead with the former.

### 5.5 CLI and security posture

```
strk20-sync serve --feed <URL|DIR|db:PATH> [--listen 127.0.0.1:7020]
                  [--db serve.db] [--network <name>|--profile <path>]
                  [--cold-start epochs|snapshot]     # default: epochs
                  [--poll <secs, default 5>]         # feed re-apply cadence
                  [--auth-token-file <path>] [--allow-remote]
                  [--cors-origin <origin>]...
```

- **Fail-shut (R-F):** binds loopback by default; a non-loopback `--listen` is
  **refused at startup** unless BOTH `--allow-remote` and `--auth-token-file`
  are given. Keyed surfaces fail closed.
- `Authorization: Bearer` checked with a constant-time compare when a token
  file is configured, **on every route inside the §5.2 perimeter — `/v1/*` and
  `/feed/live`, not `/health`**; 401 `AUTH_REQUIRED` / `INVALID_TOKEN`, no body
  echo.
- Compat hardening inherited **verbatim and hard-coded** (base §6.4): request
  and response bodies are never logged (no config can enable it — bodies carry
  raw viewing keys); cursors are never logged or persisted; malformed bodies
  are rejected without echo (400 `INVALID_BODY`); any pubkey cache is
  memory-only.
- CORS closed unless `--cors-origin` is given; TLS terminates at the operator's
  reverse proxy (one fewer certificate state machine in a keyed binary).
- Default `--cold-start epochs`: a `serve` instance is a server, and complete
  `/v1/history` is worth one replay. The rationale is now **stronger** than
  when it was written (review finding 3): the snapshot variant does not merely
  "lose pre-floor history" — under the unmodified engine its `/v1/history`
  returns a partial page set that terminates at the first pre-floor note block
  and **never reports complete**, and the synthetic registration transaction is
  never available at all (§1.1). `snapshot` remains allowed and serves the §1.1
  paging contract verbatim, with `complete: false` and
  `registration_available: false` in every response; operators are told plainly
  in the ops docs that a `serve` started this way cannot answer a
  complete-history question for any pre-existing user.

### 5.6 Relation to server compat mode

| | `strk20 --enable-compat` | `strk20-sync serve` |
|---|---|---|
| ingest | full RPC ingest (Block A) | none — folds any feed |
| trust root | its own RPC + verify-root | the feed hash chain (+ optional own-RPC anchor check) |
| state seen | `strk20.db` (authoritative) | verified mirror copy |
| wire | reference compat wire | **identical** wire, same `crates/wire` types |
| when | operator hosting the full stack | self-host against any public mirror |

One dialect everywhere means `DelegatedClient`, the stock SDK
`IndexerDiscoveryProvider` and any Tier-0 integration work against both with no
code-path fork. The conformance assertions of base leg **h** are run against
**both mounts** — one conformance suite, two servers (leg **w**).

### 5.7 Recorded roadmap shape (not v1)

If real self-hosters demand keyed push, the shape is fixed now so it cannot be
improvised later: `POST /v1/watch` (key → `SecretFelt`, RAM only) returns a
capability token; `GET /v1/subscribe` carries it in an **`Authorization`
header** over fetch-based SSE (native `EventSource` cannot send headers, and a
token must never ride a URL — R-E); the token dies with the process. Merit
trigger: a self-hoster for whom poke + requery is measurably insufficient.

---

## A6 — Chain profiles

### 6.1 One profile source, consumed by Rust and TypeScript

New top-level `profiles/` directory of JSON files — embedded with
`include_str!` in Rust and **imported by the npm package**, with a test in each
language asserting the built-ins equal the files. This kills the five-year
drift hazard of a second copy of the constants in TS.

`ChainProfile` lives in **`strk20-feed`** (pure data; clients and the wasm
module need it without touching `indexerd`):

```rust
pub struct ChainProfile {
    pub name: String,                        // "mainnet" | "sepolia" | custom
    pub chain_id: String,                    // "SN_MAIN" | "SN_SEPOLIA"
    pub pool: Felt,
    pub genesis_block: u64,
    pub epoch_size: u64,                     // 10_000 for both built-ins
    pub decoder_map: BTreeMap<Felt, String>, // class hash → decoder version
    pub rpc: RpcHints,                       // primary/fallback; ignored by clients
}
impl ChainProfile {
    pub fn builtin(name: &str) -> Option<Self>;
    pub fn from_json(s: &str) -> Result<Self, FeedError>;
}
pub struct FeedIdentity { pub chain_id: String, pub pool: Felt,
                          pub genesis_block: u64, pub epoch_size: u64 }
```

```json
// profiles/mainnet.json
{"name":"mainnet","chain_id":"SN_MAIN",
 "pool":"0x040337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a",
 "genesis_block":8978970,"epoch_size":10000,
 "decoder_map":{"0x30b8c540cf04d8ef0f4db2a9098d9cc0e35e83af1cb3325f5a4f40144b4b30b":"v1",
                "0x67dddd89d80fedadc06b6f160798f94800a4a70164e5a24301cd0d6076b554d":"v2"},
 "rpc":{"primary":"https://rpc.starknet.lava.build",
        "fallback":"https://starknet.publicnode.com"}}
```

```json
// profiles/sepolia.json — MECHANISM SHIPS NOW; the values are pure data.
{"name":"sepolia","chain_id":"SN_SEPOLIA",
 "pool":"0x0254a6b2997ef52e9f830ce1f543f6b29768295e8d17e2267d672c552cfe0d91",
 "genesis_block":null,
 "epoch_size":10000,
 "decoder_map":{},
 "rpc":{"primary":null,"fallback":null}}
```

> **TODO(research) — verified Sepolia class table and constants.** A parallel
> research task fills exactly these fields; no code changes when it lands.
>
> | field | value | status |
> |---|---|---|
> | `pool` | `0x0254a6b2997ef52e9f830ce1f543f6b29768295e8d17e2267d672c552cfe0d91` | given (roadmap item 6) |
> | `chain_id` | `SN_SEPOLIA` | given |
> | `epoch_size` | `10000` | fixed — deliberately identical to mainnet; no merit in divergence |
> | `genesis_block` (pool deployment block) | ___ | **TODO(research)** |
> | `decoder_map` (class hash → `"v1"`/`"v2"`/…) | ___ | **TODO(research)** |
> | `rpc.primary` / `rpc.fallback` | ___ | **TODO(research)** |

The loader **refuses a profile containing nulls** unless
`--allow-incomplete-profile` (dev-only): a half-filled Sepolia can never
silently run. An unknown on-chain class still degrades exactly as on mainnet
(base §5.7) — an incomplete table degrades loudly, it never corrupts.

Selection and precedence: `--network <name>` (both binaries; npm `network`) →
`--profile <path.json>` for custom chains → `--config <toml>` → explicit flags.
**Precedence: flags > config file > profile.** One process serves exactly one
network and one feed dir; two networks are two processes. Default data paths
become network-scoped (`…/strk20/<name>/strk20.db`, feed dir likewise) so two
networks cannot share state by accident. A profile change under an existing
feed dir or DB is a hard INIT error, never a migration.

`strk20 profile verify --network <n> --rpc <url>` — locates the deployment
block by bisection on `getClassHashAt` (contract-not-found below, class above),
checks `getClassHashAt(latest) ∈ decoder_map`, and prints the JSON/TOML lines.
The fill-in above is therefore **verified, not transcribed**, and the same
command is the base §5.7 degraded-mode recovery tool for the next mainnet class
upgrade: run it, paste the line, restart.

### 6.2 Stamping matrix — who checks what, where

Failure is uniformly `CHAIN_MISMATCH {field, expected, got}`, always **before
any state mutation**.

| Artifact | Carries | Checked by | Status |
|---|---|---|---|
| RPC | `starknet_chainId` | server INIT vs profile | exists (base §5.1) |
| `genesis.json` | chain_id, pool, genesis_block, epoch_size | client vs expected profile AND vs stored meta | **amend** — `FeedStore::apply_feed` today pins and compares `pool` only (`store.rs`); it now pins and compares **all four**, every sync |
| epoch header | chain_id, pool | `verify_epoch_against_manifest` | **amend `strk20-feed`** — new parameter `expect: &FeedIdentity`; epoch-header identity is **unchecked today** |
| manifest | chain_id, pool, genesis_block, epoch_size | client cross-check vs `genesis.json` every sync | **amend** |
| `head.ndjson` hdr | — | — | **amend grammar (additive)** — see below |
| snapshot header | chain_id, pool | §1.5 ring 3 | new (A1) |
| SSE `hello` | chain_id, pool | consumer before any refetch | new (A2) |
| wasm state blob | stamp in `hdr` | `load` → `STATE_FOREIGN` | new (A3) |
| sealed cursor blob | AAD = `chain_id ‖ pool` | AEAD failure on cross-network reuse | new (A3.6) |
| IndexedDB | database **name** `…:<chain_id>:<pool>` | structural | new (A4.4) |
| ops `/health` (**both** `strk20` and `strk20-sync serve`) | chain_id, pool | `DelegatedClient` on construction; refuses when absent unless `assertUncheckedNetwork` (§4.8) | **amend base §6.2** — see below |
| `sync.db` meta | four identity rows | every sync | amended above |

Base **§6.2**'s ops `/health` body currently carries
`{status, head{number,hash,timestamp}, l1_accepted, lag_secs, latest_epoch,
class_hash, decode_state, verify_root_failed}` and **no chain identity at all**.
It gains two fields:

> `"chain_id":"SN_MAIN","pool":"0x…"`

Additive; no existing consumer breaks. This is required, not cosmetic (review
finding 7): implementation-note 3 records that compat mode deliberately reuses
this one route (axum forbids two handlers on one path), so without the
amendment `strk20 --enable-compat` and every stock reference deployment are
un-checkable, and §4.8's "verify chain identity before sending any key" would be
implemented as "check if present" — skipped exactly where a mainnet wallet meets
a Sepolia server. The matrix row above is one change for all three targets.

Base **§4.4** `head.ndjson` header currently reads:

> `{"t":"hdr","v":1,"kind":"strk20-head","tail_from":8980000,"head":14056430,"head_hash":"0x…","l1_accepted":14049912}`

becomes:

> `{"t":"hdr","v":1,"kind":"strk20-head","chain_id":"SN_MAIN","pool":"0x…","tail_from":8980000,"head":14056430,"head_hash":"0x…","l1_accepted":14049912}`

Additive: decoders ignore unknown fields, `v` stays 1, and the ETag changes
once at deployment.

### 6.3 Client-side identity posture (C18)

Clients carry a **built-in expected profile** (default `mainnet`) and validate
it against `genesis.json` on **first contact**, before any row lands. TOFU
pinning remains the mechanism only for explicitly custom feeds (`--profile`,
`network: ChainProfile`), where there is no built-in to compare against; there,
first contact pins and any later disagreement is a hard error. A mainnet wallet
pointed at a Sepolia feed URL fails naming both ids instead of silently pinning
the wrong network.

---

## 7. Implementation order

Binding discipline, carried from the base spec and reaffirmed by all three
judges: **every item's first commit is its RED acceptance leg plus its unit
vectors.** No time estimates; order and edges only.

```
0a. Refactor: crates/consumer (ConsumerStore + NoteSet + apply/discovery/report)
    + crates/wire (reference wire types moved out of indexerd)
    — behavior-frozen; the existing green suite is its test, PLUS the one new
      test 0a is owed: the NoteSet round-trip/diff conformance leg over both
      impls (§0.4.1) — the existing suite cannot detect a missing abstraction,
      which is how the registry gap survived to review;
      CI gate from day one: cargo build -p strk20-consumer --target wasm32
        │
0b. Fork branch + single [patch] + fork-delta-check CI + FILE THE UPSTREAM PR
    (parallel with 0a; the sooner filed, the sooner the fork dies)
        │
        ▼
1.  A6 profiles + identity stamping + verify-zst-before-decompress + caps
    (leg t red → green)         [every later fixture then runs under a named profile]
        │
        ├──────────────┐
        ▼              ▼
2.  A1 snapshots    3.  A2 SSE on indexerd
    feed::snapshot      (legs o, p)          [independent of 2]
    → cutter
    → Rust cold start
    (legs l, m, n)
        │              │
        ▼              │
4.  A3 wasm crate + ABI over the extracted core        [needs 0a, 1, 2]
    (legs q, r, s)     │
        │              │
        ▼              │
5.  Fold gate: harness, run, verdict published to
    docs/research/fold-gate-results.md and to §4.6     [needs 4; BEFORE any
        │              │                                KeylessClient persistence code]
        │              ▼
        │          6.  A5 serve + DbTransport (legs w, x)   [needs 0a, 1, 3;
        │              │                                     parallel with 4–5]
        ▼              ▼
7.  A4 npm strk20-discovery + TS e2e (legs u, v, y)    [needs 2, 3, 4, 5, 6]

8.  Sepolia fill-in via `strk20 profile verify` + nightly smoke   [anytime after 1]
```

Edges in words. The extraction (0a) is first because every other area sits on
it, it is the only step whose test already exists, and doing it now means
snapshot cold start and the watch logic are written **once** in the extracted
crate instead of written in `crates/client` and moved later. Profiles (1) come
next because their stamps flow through every format defined afterwards.
Snapshots (2) precede wasm (4) because `apply_snapshot` consumes the format.
SSE (3) and serve (6) are independent islands. The gate (5) sits between wasm
and npm because the discussion note requires the measurement before the
persistence layer exists. The npm package (7) is last because it integrates
everything, and its e2e is the branch's new headline gate.

---

## 8. New acceptance-test legs

Continuing base **§10.3**'s lettering. **a–k stay green, but not unamended**
(review finding 4): leg **d(i)**'s URL allowlist and base §2 invariant 3 are
replaced by §2.8.1 in the same commit that publishes the first fixture snapshot,
because leg d's own client is a fresh mirror under the `--cold-start auto`
default and legitimately fetches `snapshots/*`. No leg is "kept green" by
loosening an allowlist to a prefix match. Same harness:
fully offline, real binaries, recording proxy, fixture RPC, dual oracle (O1 =
unmodified engine over `MockBackend`; O2 = the checked-in golden JSON — **one
file, shared byte-for-byte by the Rust, wasm and TypeScript suites, never
duplicated**). "Scanner" = leg d's byte-scanner including channel-key
encodings.

**l. Snapshot cold start (A1).** Fixture cuts epochs 0–1 and publishes an
anchored snapshot at epoch 1's `to`. *Fixture requirement: the seed contains a
note spent **before** the snapshot basis*, so cold-start spent-state is proven
to come from nullifier slots rather than from pre-floor `NoteUsed` events.
Assertions:

(i) **Report equality, with the exemption named (review finding 5).** A fresh
`strk20-sync --cold-start snapshot` produces output == full-replay output == O1
== O2 **field-for-field except the four keys that MUST differ**, which are
asserted separately and individually:

| key | snapshot run | replay run |
|---|---|---|
| `history_from` | `basis + 1` | `0` |
| `snapshot_basis` | `basis` | absent |
| `snapshot_rejected` | `false` | `false` |
| `verified` | `"server-asserted"` (or `"anchored"` with ring 6) | `"replayed"` |

The old leg required both "field-for-field" equality **and**
`history_from == basis + 1` over one `SyncReport` schema (§0.4.1: "one report
schema" for all four hosts), which cannot both hold. The equality is now a
**structural diff over the report minus exactly that key set**, computed by
deleting the keys and comparing what remains — so a field added to `SyncReport`
later lands in the compared set by default and cannot silently fall out of the
comparison.

(ii) Per-note `block_number` == its committed partition block, and the
pre-basis note's `spent == true`.

(iii) The proxy capture shows **no epoch ≤ basis fetched**; URL multiset =
`{genesis, manifest, snapshot, snapshot anchor, epochs > basis, head}`, matched
whole-path against §2.8.1's closed allowlist.

(iv) The **MPT root check is asserted EXECUTED** (an instrumented counter, so a
short-circuited ladder fails the leg rather than passing on a right answer).

(v) **History actually works above the floor — the positive case the old leg
never had (review finding 3).** The old leg asserted only the negative ("a
history call below the floor fails `HISTORY_UNAVAILABLE`"), which is why the
engine's backwards walk and its unconditional registration fetch went unnoticed.
Now, all four:

- a history call over a range **fully above the floor** returns the **same
  transactions as the full-replay client for that range** (registration tx
  excluded from the comparison) and **terminates rather than throwing**, with
  `complete == false` and `complete_from` naming the walk's last completed
  bound (`≥ history_floor`);
- `registration_available == false`, and the absence of the synthetic
  registration transaction is asserted **explicitly**, not inferred from a
  count;
- `complete` is asserted **never** `true` on the snapshot-started client for
  the fixture's pre-existing user, and `true` on the replay client;
- a caller-supplied `from_block < history_floor` still fails
  `HISTORY_UNAVAILABLE` naming the floor (R-L, unchanged).

**m. Snapshot tamper + publication negatives (A1).**

(i) Flip one byte in the served `.zst` → `FEED_HASH_MISMATCH` raised **before
decompression**, and the ordering is asserted the way leg **l** asserts the MPT
check: **the decompressor is a poisoned stub that panics if invoked, plus a
call counter asserted zero** (review finding 16a). Observing the error code
alone proved nothing about ordering — a byte-flip in a zstd frame commonly fails
decompression too, so an implementation that decompressed first and hashed
second passed this leg. R-I exists precisely to keep an attacker-controlled
decompressor from running before the bytes are authenticated, so the leg must
test the ordering and not the outcome.

(ii) Alter one slot value and fix up the content sha256 → `SNAPSHOT_ROOT_MISMATCH`
naming computed vs header vs anchor; under `--cold-start auto` the client then
logs the warning, falls back to full epoch replay, still equals O1, and reports
`snapshot_rejected: true` (C13).

(ii-b) **The malicious-server case, which (ii) does not cover (review finding
2d).** Serve a snapshot with one slot **added, one removed and one altered**,
with `header.storage_root`, `manifest.snapshot.storage_root` **and** the whole
`contracts_proof` sidecar recomputed consistently — i.e. what a server that
wants to lie would actually produce, as opposed to (ii)'s corruption. Assert
what genuinely catches it:

- with ring 6 configured → `SNAPSHOT_ROOT_MISMATCH` against the user's own RPC,
  and `verified` never reaches `"anchored"`;
- **with no ring 6 → nothing catches it. Rings 1–5 pass.** The leg asserts that
  outcome positively — the client accepts the tampered slot set and reports
  `verified: "server-asserted"` — so the reduced grade of §1.5/§1.5.2 is pinned
  by a test rather than by a paragraph, and any future claim that ring 5 is
  proof-grade against the server turns this leg red.

(iii) Point `epoch_hash` at a wrong value → `FEED_CHAIN_BROKEN`.
(iv) Manifest snapshot with `anchor: null` → `SNAPSHOT_ANCHOR_MISSING`.
(v) **Negative:** after a forced verify-root failure no snapshot file and no
manifest snapshot entry are produced.
(vi) A snapshot whose `header.class` disagrees with the sidecar's
`contract_leaves_data[0].class_hash` → `SNAPSHOT_ROOT_MISMATCH` (§1.5 ring 5;
before that amendment `header.class` was checked by nothing).

**n. Snapshot determinism + audit (A1, extends leg j).** Two independent
backfills emit byte-identical snapshot files (sha256 equality) alongside
byte-identical epochs; **with both mirrors driven to the same tip first**, a
`mirror pull`ed instance's own regenerated snapshot equals the origin's — the
parity precondition is now arranged by the fixture and asserted, because
keep-newest-2 retention (§1.4 step 6) means a lagging mirror and the origin need
not hold a snapshot for the same epoch, and the old leg left that to coincidence
(review finding 2); `strk20-sync snapshot audit` passes (epoch-replayed
re-serialization == served bytes); `strk20 epoch verify --all` covers snapshot
hashes.

**o. SSE conformance (A2).** Two concurrent watchers on `/feed/live`:
(i) both receive `hello` (carrying chain_id + pool) then current-state
`head`/`epoch`/`snapshot`/`status`, and identical subsequent sequences;
(ii) their captured request bytes are multiset-identical and pass the scanner,
which is also run over the full SSE response capture; (iii) a poke-driven
client reaches the same final mirror state and report as a polling client over
the same timeline, **including across the leg-g reorg** (a reorg is just a head
poke — no special wire exists); (iv) reconnect with a stale `Last-Event-ID`
receives current state (ignored-header semantics asserted); (v) `retry:`, the
2 KB padding comment and keepalive comments are present in the raw capture;
(vi) **any** query string on `/feed/live` → 400 `INVALID_QUERY`.

**p. SSE degrade and restore (A2).** The proxy kills the stream mid-test, then
404s it: the client degrades to polling (status event `polling`), still
converges to the post-extension ground truth, and restores `live` when the
route returns.

**q. WASM conformance (A3).** The module (Node build of the same crate) is fed
the fixture feed's raw bytes on **both** paths (snapshot and full-epoch);
`DiscoverOut.report_json` deep-equals the native `strk20-sync sync --json`
output and the O2 pins. Sealed round-trip: a second `discover` with the returned
blob does no rediscovery (identical report, `ckpt_at` advancing only with the
feed). Wrong-key sealed blob → `SEALED_STATE_MISMATCH` with
`cursor_reset: true`, fresh discovery, same final notes. `check_manifest`
**returns** `ok`/`behind`/`diverged` on the three constructed manifests — the
return-value form is the asserted one and no staleness throw exists to provoke
(review finding 13). `export_reference_cursor` output round-trips into the
compat wire (interop). Every error-table code is provoked at least once,
including `ENTROPY_INVALID` (a 31-byte buffer) and `ENTROPY_REUSED`. **The
scanner runs over every thrown error string and over the exported state blob** —
mechanical proof of key-independence.

**Nonce-safety sub-leg (review finding 1), which the old suite could not fail.**
Feed two `discover()` calls the **same** prior sealed blob and the **same**
`entropy32` — the forked-tab path:

- the shipping build raises `ENTROPY_REUSED` and seals nothing;
- a test-only build with the `prev_entropy_h` guard disabled produces **equal
  nonces**, asserted equal. This is the leg that proves the guard is what
  prevents the collision, and that the withdrawn counter argument never did.

Then: same prior blob, **different** entropy → different nonces. And a purity
check that no wrapper call site caches, reuses or derives `entropy32` — a spy
over `crypto.getRandomValues` in the Node build asserts one fresh 32-byte draw
per `discover()`.

**r. WASM reorg byte-identity (A3).** Replay the leg-g fork through the module:
`apply_head` reports `tail_rewound`; the next `discover` equals the post-fork
O1; and the exported state blob is **byte-identical before and after the fork**
— the mechanical proof that the tail is never exported and that browser
persistence needs no reorg logic.

**s. WASM purity + size (A3.9, CI gates run with the suite).**
**Feature-resolved** dependency walk (`cargo tree -e features`, not a crate-name
walk): `consumer` and `client-wasm` reach no
`tokio`/`reqwest`/`rusqlite`/`getrandom` — asserted with a **red-first negative**
that removes `default-features = false` from `chacha20poly1305` and confirms the
gate fires, since a name-only walk cannot see the default-feature path through
`aead` that would otherwise have shipped `getrandom` (review finding 11a).
`wasm-objdump` import section matches
`crates/client-wasm/import-allowlist.txt` exactly, diffed as a file rather than
matched as a name pattern (review finding 12). `crates/consumer` compiles under
`#![forbid(unsafe_code)]` and `crates/client-wasm` under
`#![deny(unsafe_code)]` with exactly one documented `#[allow]` scope and zero
hand-written `unsafe` blocks — the leg **builds the cdylib**, which is what
would have caught `forbid` being impossible over `#[wasm_bindgen]` expansion
(review finding 11b). **Total published wire cost** (wasm + glue + fzstd, gzip)
≤ the §3.9 budget — one denominator, the same one §4.1 quotes.
`fork-delta-check` asserts the `discovery-core/src` diff is EMPTY.

**t. Chain profiles (A6).** The **whole fixture suite runs under a named
synthetic profile** (`SN_TEST`), proving no mainnet hardcoding survives. Then:
(i) a mirror synced under one profile pointed at a feed with another `chain_id`
or `pool` → `CHAIN_MISMATCH` naming both, **before any DB write — asserted by a
fixed logical digest over `meta`, `blocks`, `storage_log`, `events` and
`notes_registry`, taken before and after**. Byte-equality of `sync.db` is
**not** the oracle (review finding 16b): the store opens WAL and `FeedStore::open`
creates and chmods the file before the identity check runs, and a checkpoint on
close can rewrite the main database file whether or not a row changed — so
byte-equality would be both flaky and, worse, silently vacuous. The digest is
what the leg actually means. (ii) an epoch file whose header
`chain_id` disagrees with its manifest is rejected; (iii) the wasm `Engine`
pinned to chain A throws `CHAIN_MISMATCH` on chain B's epoch and
`STATE_FOREIGN` on chain B's blob; (iv) two chains open distinct IndexedDB
databases; (v) an incomplete profile is refused without the dev flag; (vi) the
stamps are asserted **present** in genesis, manifest, epoch hdr, head hdr,
snapshot hdr, SSE `hello` and **ops `/health` on both `strk20` and
`strk20-sync serve`** (§6.2 amendment); (vii) `DelegatedClient` against a
`/health` with the identity fields stripped **refuses to construct** unless
`assertUncheckedNetwork` is set (review finding 7 — the "check if present"
failure mode is asserted absent); (viii) the accepted `--cold-start` /
`coldStart` mode strings are **identical strings** in the Rust and TypeScript
halves, read from one shared fixture (review finding 14a).

**u. npm keyless e2e (A4).** `KeylessClient` in Node and in Chromium through
the recording proxy against the real spawned `strk20`: `getNotes().raw`
deep-equals the O2 golden; requests are GET-only with URL multiset ⊆ **§2.8.1's
closed eight-pattern allowlist** (whole-path match, `/feed/live` included, no
query strings); alice's and bob's request multisets are identical;
`capture-scan` (the Rust bin) finds no key/address/channel-key encoding in the
TS proxy capture or in the IndexedDB dump, **and its self-test confirms it DOES
find the key in a delegated capture**; a reload converges with a request delta
of **`{genesis, manifest, head}`** + SSE only — genesis is in the delta by
design, since §4.4 re-fetches it every session and byte-compares against the
stored copy to catch a feed that changes its own genesis, and the old
`{manifest, head}` was unachievable because the schema stored nothing to
reconstruct it from (review finding 15); the reload leg additionally asserts
that a **mutated** served `genesis.json` fails `CHAIN_MISMATCH` before any row
lands; the five IndexedDB quirks each have their
assertion (blocked open → memory fallback reported by `status()`; emptied store
→ cold start, not error; no fetch inside a transaction; multi-tab under and
without Web Locks; first write off the critical path).

**v. npm delegated e2e (A4/A5).** `DelegatedClient` against
`strk20-sync serve` deep-equals the `KeylessClient` result and the O2 golden;
cursors round-trip request ↔ response in the reference schema; a forked
`last_known_block` yields HTTP 409 `BLOCK_REORGED`; `subscribe` delivers the
leg-k spend as a `notes` event with the note in `spent`.

**w. serve (A5).** `strk20-sync serve --feed <dir>` colocated with a live
cutter dir: **base leg h's compat assertions are re-run verbatim against this
second mount** (one conformance suite, two mounts); every response carries
`X-Strk20-Mode: delegated-keyed`; `/feed/live` pokes after the fixture extends
the chain; a non-loopback `--listen` without both `--allow-remote` and
`--auth-token-file` is **refused at startup**; a missing or wrong bearer yields
401 with no body echo; history below `history_floor` on a snapshot-started
serve → `HISTORY_UNAVAILABLE`; the scanner over serve's stdout, stderr and
on-disk artifacts finds nothing (the key is legitimately in RAM, never at
rest).

**x. DbTransport (A5.4).** Reworded to assert what is actually checkable
(review finding 6c — the old leg claimed synthesized manifest/head/snapshot
"verify through the **unchanged** client ladder", a verification the ladder does
not perform for head or manifest, and a snapshot the transport cannot produce at
all):

(i) For every cut epoch, the `Raw` payload returned by `DbTransport::fetch_epoch`
is **byte-identical** to the on-disk epoch payload, and a deliberately corrupted
DB row makes the transport fail its own `sha256(payload) == epochs.content_hash`
check rather than serving divergent bytes.
(ii) With a feed dir present, synthesized `head.ndjson` and `manifest.json` are
byte-compared against the on-disk files and a seeded encoder drift is **refused
by the transport**.
(iii) **Named gap, asserted as a gap:** with no feed dir, a seeded head-encoder
drift reaches the client **undetected** — the client verifies no hash for
`head.ndjson` anywhere — and `/health` reports
`feed_source: "db:unchecked-head"`. Asserting the gap positively stops a future
reader from inferring a check that does not exist.
(iv) `DbTransport::snapshot_capable() == false`; `fetch_snapshot` and
`fetch_snapshot_anchor` return `None`; `serve --feed db:… --cold-start snapshot`
is refused **at startup** naming the reason.
(v) A `schema_version` mismatch is a startup refusal.

**y. Fold gate as a watched number (A4.6).** The bench publishes
`t_zstd`/`t_cold` for L1/L2/L3 as CI build artifacts with the 3× regression
alarm; the recorded verdict in §4.6 is asserted non-empty before the npm
persistence code is allowed to reference a mode.

**z. Compile-fail locks (extends base §10.1).** Regenerated for the new
`FeedTransport` signatures including `EpochPayload`, `fetch_snapshot` and
`fetch_snapshot_anchor` (no user-derived parameter is expressible);
`SecretFelt: !Serialize` unchanged; the wasm crate exposes exactly two
key-accepting entries and no transport type.

---

## 9. Non-goals, restated

Unchanged from base §1 and reaffirmed here because new surfaces invite them
back:

- **No write path in our binaries.** Signing, key custody and prover operation
  stay out. We are the read half of every write: the SDK cannot build a spend
  without knowing your notes, and post-submit confirmation (nullifier landed,
  no reorg) arrives through `subscribe`.
- **No per-user push on any keyless surface, ever.** A policy line, not a
  tuning knob. `/feed/live` is global, parameterless, and identical for every
  subscriber.
- **No second data path.** SSE notifies; bytes always come through the
  content-addressed, hash-verified fetch path.
- **No note-correlated fetching.** History below the floor is obtained by full
  epoch replay or not at all (§1.6).
- **No keyed watch registries, no server-side cursors, no key at rest** on any
  surface we ship.
- **OHTTP** — deferred; pointless in keyless mode where every client fetches
  identical bytes. Trigger unchanged: delegated mode gains non-self-hosted
  users.
- **Prefix-bucket endpoint, then PIR** — wire frozen in base §6.3, unimplemented.
  Trigger unchanged: snapshot beyond ~50 MB (~8×10⁵ records).
- **Postgres store, hosted-operator features (API keys, quotas, multi-pool
  routes), explorer UI, transaction CLI, horizontal ingest scaling** — still
  out. Mirroring static files is the scale-out.
- **`DecompressionStream('zstd')`, native-stream promotion** — roadmap;
  one decompression path in v1.
- **Worker-only or worker-less API forks, `persist:'auto'`, two trust states in
  the UI** — rejected in §0.2.
- **TLS in the keyed binary** — terminate at the operator's reverse proxy.

Everything else in the base spec — wire format v1, hash chain, ingest pipeline,
reorg floors, compat hardening, and legs a–k — stands unmodified except where
this addendum amends it in quote-and-replace form, and every surface above
inherits its invariants. The base-spec amendments this addendum now carries are
collected in §10.1.

---

## 10. Red-team resolution log

An adversarial review of this document returned 18 findings, verified against
the base spec, the implementation notes, the shipped crates, and the pinned
`discovery-core` checkout at rev `74841ca`. **All 18 are accepted and fixed in
place.** Three carried sub-claims or sub-options that are rejected on merit;
those rejections are recorded inside their rows and nowhere else, so a later
reader cannot mistake a partial dissent for a wholesale one.

No GIVEN roadmap decision was weakened: two blocks with the `FeedTransport`
seam, WASM as a pure synchronous computer, the keyless + delegated dual API, no
write path in our binaries, and every deferred item's deferral all stand
unchanged. Where a fix touched a council ruling (C2, C5, C9), the ruling's
**verdict** survives and only its **reasoning** was corrected — which is the
point of recording reasoning separately from verdicts.

| # | Finding | Disposition | Where |
|---|---|---|---|
| 1 | CRITICAL — sealed-blob nonce derivation admits `(key, nonce)` reuse; C2's structural answer is false | **FIXED.** The counter argument is withdrawn outright: it holds only along a non-forking chain, and §4.3/§4.4 make forking supported. Nonce safety is restated as resting on fresh `crypto.getRandomValues` entropy. Added the `prev_entropy_h` guard, which closes the constant-entropy case **including across a fork** and is stated at exactly that strength, not as a general fix. Added `ENTROPY_REUSED`/`ENTROPY_INVALID`, the forked-path acceptance assertion the old unit test could not fail, and a wrapper purity spy. C2's verdict (entropy passed in) survives on two restated merits. | C2, §3.6, §3.7, §4.3, leg **q** |
| 2 | MAJOR — snapshot cold start is a real trust downgrade, is the default, and base §9's trust table is not amended | **FIXED, with one sub-claim rejected.** Base §9 gains a snapshot row (§1.5.2); ring 5 is renamed from "proof-grade" to self-consistency against the server's declared root, with the malicious-server case spelled out; ring 6 becomes mandatory whenever an RPC URL is configured; `verified: 'anchored'\|'server-asserted'\|'replayed'` replaces the ungrounded `verified: boolean` and is surfaced in `SyncReport`, `status()` and `/health`; leg **m** gains case (ii-b) asserting that **nothing** catches a consistently-recomputed tamper without ring 6. **Rejected:** making `--cold-start epochs` the default (it makes cold start O(history) for every browser consumer, defeating A1) and the claim that keep-newest-2 makes cross-mirror snapshot comparison unavailable (two mirrors at the same tip hold the same newest two; the review is right only that the parity precondition was unstated, which leg **n** now pins). | §1.5, §1.5.1, §1.5.2, §4.2, legs **m**, **n** |
| 3 | MAJOR — A1's "history from the snapshot block forward" is not deliverable with the unmodified engine | **FIXED.** Confirmed in the pinned engine: `fetch_transactions` walks backwards from note blocks sourced from `last_update_block` (pre-basis for a snapshot client), and `fetch_registration` unconditionally reads `ViewingKeySet` events at the registration block. The boundary is restated as "a partial page set that terminates at the walk's first pre-floor read and never reports complete", with a paging contract returning `complete`, `complete_from` and `registration_available`. `complete_from` is the walk's last completed bound, **not** `history_floor` — the terminating iteration's above-floor withdrawal gap is genuinely unfetched, and the review's suggested `completeFrom = floor` would have been a fresh overclaim. R-L is untouched: the access layer still errors, the API layer converts the terminating error into an explicit boundary. Leg **l** gains the positive assertion it never had. | §1.1, §3.3, §4.2, §5.5, leg **l** |
| 4 | MAJOR — the new fetch set contradicts base §2 invariant 3 and base leg d | **FIXED.** Quote-and-replace amendments to base §2 invariant 3 and base leg d(i), with a closed eight-pattern allowlist matched **whole-path** and "no query strings". §8's preamble now says plainly that a–k stay green *by amendment*, not by loosening — the prefix-match erosion the review predicted is named and forbidden. Recorded: leg d gains the snapshot paths (its client is a fresh `--cold-start auto` mirror) but not `/feed/live`, since Rust `--watch` stays polling-only in v1. | §2.8.1, §8 preamble, legs **l**, **u** |
| 5 | MAJOR — leg l asserts two mutually exclusive things | **FIXED, and widened.** The exemption is named as a table, and the review's list was incomplete: `snapshot_basis`, `snapshot_rejected` and `verified` must differ too, not only `history_from`. The equality is now a structural diff over the report **minus exactly that key set**, so a later-added `SyncReport` field lands in the compared set by default. | leg **l** |
| 6 | MAJOR — `DbTransport` cannot serve the snapshot anchor; its head/manifest have no self-check | **FIXED.** Confirmed in code: the proof response is written only to a sidecar file, the `epochs` table holds four scalar summary columns, no snapshots table, no proof blob. `fetch_snapshot`/`fetch_snapshot_anchor` return `Option`, `snapshot_capable()` joins the trait, `db:` + `--cold-start snapshot` is a startup refusal. **Rejected:** adding a proof-blob column to the cutter's schema — a base-spec DDL amendment storing a large best-effort blob in the indexer's hot table, to buy one deployment shape `serve` does not default to. C5's "every fetch self-verifies" is corrected to "epoch fetches"; head/manifest gain a feed-dir byte-comparison where a feed dir exists and are declared **UNCHECKED** otherwise, with `feed_source: "db:unchecked-head"` on `/health`. Leg **x** rewritten to assert the gap positively. | C5, §5.4, leg **x** |
| 7 | MAJOR — `DelegatedClient`'s chain-identity check cannot work against `--enable-compat` | **FIXED at the source.** Confirmed: the ops `/health` body carries no `chain_id` and no `pool`, and implementation-note 3 records that compat reuses the one route. Base §6.2 is amended to add both fields (additive). Client behaviour when they are **absent** is specified rather than left as "verify if present": refuse to construct with `CHAIN_MISMATCH`, unless the caller passes `assertUncheckedNetwork`. Leg **t** asserts the refusal. | §4.8, §6.2, leg **t** |
| 8 | MAJOR — `subscribe()` uses `EventSource` against a token-authenticated `serve` | **FIXED by deciding, not hedging.** `/feed/live` on `serve` is **inside** the auth perimeter; `/health` is outside (it is how a client checks identity before sending a key); a perimeter table now exists so no route is undecided. `DelegatedClient.subscribe()` uses fetch-based SSE with an `Authorization` header — §5.7's transport shipped for the notification plane only, with no capability token, no `POST /v1/watch` and no registry, so C7's veto of keyed registries is not reopened and R-E is honoured. Native `EventSource` remains the transport for the keyless `/feed/live`. | §4.8, §5.2, §5.5 |
| 9 | MAJOR — `ConsumerStore` cannot express the note registry `§0.4.1` says moves into `strk20-consumer` | **FIXED.** Confirmed: `notes_registry` with `upsert_note`/`notes`/`refresh_spent`/`prune_missing_notes` lives in `crates/client/src/store.rs`, while §0.4.1's `register_notes`/`refresh_spent` were declared generic over a read-only view. The registry becomes a value type `NoteSet` that those functions take and return — which is also what makes `DiscoverOut.added/spent` a pure diff — plus `notes_get`/`notes_put` on the trait so the two incompatible persistence models (SQLite table natively, sealed AEAD blob in wasm) each get what they need. The review's key point is adopted verbatim: 0a's stated test (the existing suite) **cannot detect a missing abstraction**, so 0a gains a `NoteSet` conformance leg over both impls. | §0.4.1, §7 step 0a |
| 10 | MAJOR — the frozen state-blob example contradicts §1.7's floor rule and lacks the upper bound leg r depends on | **FIXED, and the example rebuilt rather than patched.** `history_floor` for basis 14059999 is 14060000; correcting that number alone would have left the example's `b`/`ev` lines below the floor, so the example now shows a client that applied the snapshot *and* epoch 1406. The degenerate case it had been hiding is called out: a snapshot-only client has an empty `[history_floor, last_epoch_to]` and therefore zero block/event lines. Bounds are written into the format lines and into `load`'s structural checks, so "no line references a block > `last_epoch_to`" is enforced by the parser and leg **r** is pinned by the grammar rather than by a byte comparison that happens to pass. | §3.5 |
| 11 | MAJOR — the wasm crate fails two of its own §3.9 gates on day one | **FIXED, both halves.** (a) Every RustCrypto dependency is pinned `default-features = false` with an explicit feature list, and the gate becomes a **feature-resolved** `cargo tree -e features` walk with a checked-in diffed tree — a crate-name walk cannot see the default-feature path that would have shipped `getrandom` and quietly voided C2. Leg **s** gets a red-first negative that removes the pin and confirms the gate fires. (b) `crates/client-wasm` moves to `#![deny(unsafe_code)]` with one documented `#[allow]` scope, since `forbid` cannot be lifted inside `#[wasm_bindgen]`-generated code and the cdylib would not have compiled; `#![forbid]` stays on the pure-Rust `crates/consumer`. Leg **s** builds the cdylib. | §3.1, §3.9, leg **s** |
| 12 | MINOR — the import-section audit proves less than C2 spends an ABI parameter on, and its allowlist is unspecified | **FIXED.** The allowlist becomes a checked-in `import-allowlist.txt` of `(module, field)` pairs diffed in CI, not a name-pattern judgement that drifts open. The property is restated honestly: the section is **not** empty — `__wbindgen_string_new`/`__wbindgen_throw`/`__wbg_*` are calls into JS carrying arbitrary strings, and are how every ABI method returns its JSON — so what the audit proves is that the module cannot open a network, storage, timer or randomness handle of its own. The entropy parameter is re-justified on dependency-graph and determinism merits (C2), not on emptiness. | C2, §3.9, leg **s** |
| 13 | MINOR — `check_manifest`: returns "diverged" or throws `STATE_STALE`? | **FIXED.** The return-value form wins (it is what the ABI signature and §4.3's flow use) and the `STATE_STALE` row is deleted from the closed set, with a standing prohibition on reintroducing a thrown staleness error — a throw and a discriminant are different control flow and different `DiscoveryEvent` emission, and both cannot be normative. Genuinely unusable blobs keep their `STATE_CORRUPT`/`STATE_VERSION`/`STATE_FOREIGN` codes on `load`. | §3.7, §4.3, leg **q** |
| 14 | MINOR — cluster of eight underspecified formats | **FIXED, all eight.** (a) one cold-start vocabulary `auto\|snapshot\|epochs` in Rust and TS, asserted identical from one fixture in leg **t**; (b) `keyId` is the full 32-byte HKDF output as 64 lowercase hex, no slice; (c) `apply_snapshot` gets its return schema including the `state_changed` field §4.3 reads; (d) `"e"` is the epoch key on both SSE events and the manifest; (e) `ENTROPY_INVALID` added; (f) `history()` takes `sealed: Option`; (g) `persist` is narrowed at publish time and rejects an unbuilt mode with `CONFIG_INVALID` rather than silently downgrading; (h) `header.class` is now checked by ring 5 against the sidecar's `class_hash` and the manifest — the field previously existed for nothing. | §1.5, §2.2, §3.3, §3.7, §4.2, §4.4, §4.5, leg **t** |
| 15 | MINOR — leg u's reload delta omits `genesis.json`, which the schema never stores | **FIXED, taking the stronger of the two options.** The review offered "store it" **or** "correct the leg", noting the second is stronger — but the two are mutually exclusive and the review's own fix text mixed them. Settled: genesis is **both** stored in `meta` (so `load` has an offline stamp source) **and** re-fetched every session and byte-compared (so a feed that changes its own genesis is caught, which a stored-only copy never sees). The delta is therefore `{genesis, manifest, head}`, and leg **u** asserts it plus a mutated-genesis negative. | §4.4, §4.3, leg **u** |
| 16 | MINOR — two legs do not pin the property they claim | **FIXED, both.** (a) Leg m(i)'s verify-before-decompress now uses a poisoned decompressor that panics if invoked plus a zero-call assertion, matching leg l's instrumented-counter discipline; an error code alone could not order the check against decompression, and a zstd byte-flip fails decompression anyway. (b) Leg t(i)'s no-write oracle moves from `sync.db` byte-equality — unreliable under WAL, where open creates and chmods the file and a checkpoint on close can rewrite it regardless of row changes — to a fixed logical digest over `meta`/`blocks`/`storage_log`/`events`/`notes_registry`. | legs **m**, **t** |
| 17 | MINOR — the 300 KB gate is measured against two different things | **FIXED.** One denominator: **total published wire cost** (wasm + glue + fzstd, gzip), since that is what a consumer pays. §4.1's "~255 KB" arithmetic is withdrawn — it assumed the module stayed at the 231 KB spike baseline that §3.9 itself says will grow. The budget value becomes a FILL-IN set from a measured post-step-4 number (measured + 20 %), with 300 KB as the provisional gate until then, rather than a number back-derived from a spike. | §3.9, §4.1, leg **s** |
| 18 | MINOR — SSE's linkability delta is asserted away, and §2.7 recommends the linking | **FIXED.** §2.3's absolute is scoped to the protocol layer explicitly. §2.6 gains the transport residual stated plainly — connection lifetime is observable, h2 coalescing binds the session's fetch stream to it — **and states which way base R3's policy line falls**: acceptable for v1, because both residuals are already implied by a non-relayed client's IP and SSE improves the timing dimension (simultaneous pokes versus per-client polling phase), with OHTTP the same deferred answer for both and `live: false` a first-class opt-out. §2.7's h2 advice is labelled an operator convenience, not a client requirement. | §2.3, §2.6, §2.7 |

### 10.1 Base-spec amendments carried by this addendum (index)

Every one is written in quote-and-replace form at the section named:

| Base section | Amendment | Here |
|---|---|---|
| §2 invariant 3 | closed eight-pattern URL allowlist, whole-path, no query strings | §2.8.1 |
| §4.2 / §4.4 | `snapshot` manifest object; `snapshots/` tree lines; `head.ndjson` hdr gains `chain_id`/`pool` | §1.8, §6.2 |
| §5.5 | snapshot cut step, basis-block root check, retention | §1.4 |
| §6.1 | `/feed/live`, `/feed/snapshots/*` routes | §2.8 |
| §6.2 | ops `/health` gains `chain_id` and `pool` (**both** binaries) | §6.2 |
| §7.2 | `FeedTransport` gains `EpochPayload`, snapshot methods (`Option`), `snapshot_capable` | §5.4 |
| §7.3 | verify `zst` before decompression with bounded output; snapshot cold-start branch | R-I, §1.7 |
| §9 | trust table gains a snapshot-cold-start row; cross-mirror sentence qualified | §1.5.2 |
| §10.3 leg d(i) | allowlist replaced to match §2 invariant 3 | §2.8.1 |
| §12.1 | npm package renamed `strk20-discovery` | §4.1 |

---

## 11. Measured-reality amendment — storage proofs (2026-08-31)

> **RETRACTED THE SAME DAY — read §12 instead.** Everything below rests on the
> claim that historical storage proofs cannot be obtained. That claim came from
> a bisection against an aggregating endpoint and is FALSE: deep proofs are
> available back to genesis with a retry. See
> [../research/live/proof-window.md](../research/live/proof-window.md) §1 and
> §3. The text is kept only because §12 refers to it.

Everything above was designed against a premise that running the real binaries
against mainnet falsified. This section supersedes the clauses it names. The
measurements are in [../research/live/proof-window.md](../research/live/proof-window.md)
and [../research/live/live-run-findings.md](../research/live/live-run-findings.md).

### 11.1 The premise that failed

`starknet_getStorageProof` cannot be asked about a block of our choosing.
Bisected against lava mainnet: **OK at head−968, error 42 at head−975** — a
~1024-block sliding window, not the "~25–55k blocks" recorded in
implementation-note 5. No public provider does better: publicnode does not
implement the method at any height, drpc returns `-32601`, blast is
discontinued. On Sepolia proofs answer only at the exact head.

Three consequences, all observed rather than reasoned:

1. `verify-root` at `min(l1_accepted, frontier)` **can never succeed** —
   `l1_accepted` lags head by ~5,000 blocks on mainnet (measured 14,128,517 vs
   14,123,420). A completed full backfill returned error 42.
2. Per-epoch anchors are absent **always**, not merely "outside the window":
   **0 of 515 epochs** in a completed mainnet backfill carry one.
3. Therefore §1.3's required snapshot anchor and §1.4 step 4's basis-block root
   check are unimplementable as written, and a spec that keeps them publishes
   **no snapshots at all** in production.

### 11.2 Replacement: a head-captured anchors log

The only reliably provable block is the one that just landed, so proof capture
becomes head-driven and append-only.

`feed/anchors.ndjson` — canonical NDJSON, one record per captured anchor,
strictly ascending by `block`, append-only, outside content addressing (it is
an attestation *about* the feed, not part of its hash chain):

```
{"block":N,"block_hash":"0x…","storage_root":"0x…","class":"0x…"}
```

Captured by the ingest loop when a block is inside the live window and the
mirror's recomputed root for that block equals the proof's
`contracts_proof.contract_leaves_data[0].storage_root`. A capture failure is
never a mirror alarm (§11.4).

### 11.3 Replacement: reachability, not point-proofs

A client cannot get a proof at a snapshot's basis block `b` either — it has the
same providers we do. So the snapshot's grounding becomes a **reachability
check**, which needs no new RPC capability and is strictly stronger than a
point-proof at `b` because it validates the intervening epochs too:

> Fold `snapshot(b)`, then apply epochs `b+1 … A` from the feed, where `A` is
> the newest block in `anchors.ndjson` that the client has folded to.
> Recompute the storage root and compare with that anchor's `storage_root`.
> A match attests the snapshot **and** every epoch between `b` and `A`.

Ring 6 of the §1.5 ladder (the user's own RPC) is unchanged and remains the
only ring that grounds an anchor in the chain itself: an anchor record is a
server assertion until the client checks its `block_hash`/`storage_root`
against an RPC it trusts — which any client can do for a *recent* anchor,
because recent is exactly what the window serves. This is the honest statement
of the trust grade, and it replaces §1.5's claim that a stored proof sidecar
grounds the basis block.

Snapshot publication gate (**replaces §1.4 step 4**): publish when the mirror's
root matched the chain at the most recent anchor capture, i.e. `anchors.ndjson`
has a record at some `A ≥ b` with no verified mismatch since. Because pool
slots are write-once (measured: 134,879 distinct slots across 139,131 writes,
96.9 % first-writes), a match at `A` attests every write at or below `A`,
`b` included.

### 11.4 `verify-root` semantics (replaces implementation-note 5)

Three outcomes, not two:

| outcome | meaning | health |
|---|---|---|
| `MATCH` | recomputed root equals the chain's at a block inside the window | OK |
| `MISMATCH` | they differ — the mirror is wrong | **DEGRADED**, stop publishing |
| `UNAVAILABLE` | no block was provable (window, or the endpoint lacks the method) | OK, logged |

Conflating the third with the second is what would have made a
capability-poor endpoint look like mirror corruption. The verification block is
chosen inside the live window and ≤ our frontier, walking toward head on error
42, bounded.

### 11.5 Endpoint capability is part of the model

Endpoints are not interchangeable; the code must stop assuming they are. Three
live defects share this single root cause: LIVE-1 (a pruned-history error from
one aggregator backend aborted a whole backfill, though a retry succeeds),
LIVE-6 (failover to a proof-less endpoint turns every check into a false
alarm), LIVE-7 (`null` for `class_hashes`/`contracts_storage_keys` is rejected
by some backends; `[]` is accepted by all — `crates/indexerd/src/rpc.rs:277`).
Track per-endpoint capability (implements proofs; observed retention depth;
param strictness) and select an endpoint that can serve the request kind. A
capability gap is never reported as a data defect.

### 11.6 What an operator with an archive node gets

An operator running their own node with tries retained can verify at any block,
publish per-epoch anchors, and satisfy the original §1.3/§1.4 text. That path
stays supported as an opt-in configuration and is the strictest mode we offer.
It is not something a public-provider deployment may assume, which is exactly
the mistake this amendment corrects.


---

## 12. Correction to §11 — historical proofs ARE obtainable (2026-08-31)

§11 told this spec to abandon per-epoch and per-snapshot anchors because
`getStorageProof` appeared to answer only within ~1024 blocks of head. That
measurement was a bisection over a *nondeterministic* predicate: lava routes
each call to a different backend and only some run archive tries, so error 42
reports which backend answered, not how old the block is. Retrying, proofs
come back for any block — verified at 5.15M blocks behind head, with each
proof's `global_roots.block_hash` matching the real block header.

**Therefore §A1 as the council wrote it stands.** Specifically:

1. **§1.3's required anchor sidecar is reinstated.** A snapshot's basis block
   can be proved. Obtain it with a bounded retry loop on error 42 against a
   proof-capable endpoint.
2. **§1.4 step 4's basis-block root check is reinstated**, and so is the batch
   `verify-root` at `min(l1_accepted, frontier)` — l1 lagging head by ~5,000
   blocks is not an obstacle.
3. **Every accepted proof must be bound to the chain**: compare
   `global_roots.block_hash` with `starknet_getBlockWithTxHashes(block)`
   before trusting `contract_leaves_data[].storage_root`. The proof pool is
   anonymous and load-balanced; this check is what makes a retry-until-success
   loop safe rather than a way to accept whichever answer we liked.
4. **Retry, don't fail over.** Error 42 means "this backend cannot", not "this
   block cannot". Retry the same endpoint a bounded number of times before
   concluding `UNAVAILABLE`, and never move the active endpoint on account of
   a proof refusal (LIVE-6 unchanged: publicnode implements no proofs at any
   height, so failing over to it guarantees a false alarm).

**What §11 contributed that survives:**

- `verify-root` stays three-valued — `MATCH` / `MISMATCH` / `UNAVAILABLE` —
  because a capability gap must never read as mirror corruption.
- `feed/anchors.ndjson` stays, demoted from replacement to complement: a cheap
  running audit trail that also covers epochs cut before proofs were sought.
- The reachability check of §11.3 stays as a **fallback** for any snapshot
  whose basis-block anchor could not be obtained, and as an extra check that
  validates the intervening epochs. It is no longer the primary grounding.
- Write-once slot semantics (measured: 134,879 distinct slots across 139,131
  writes) remain the reason one verification point attests everything below it.

**Method note, binding on future work in this repo:** against an aggregating
endpoint, a single failed request proves nothing. This is the third defect in
this project with that root cause (LIVE-1 pruned history, LIVE-8 continuation
tokens, and this retracted measurement). Any conclusion of the form "the node
cannot do X" must be established by a retried, multi-attempt probe.
