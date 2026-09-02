# Consumer path — spec addendum (A1–A6)

Status: FINAL for implementation. Extends [architecture.md](architecture.md)
(base spec v1); every amendment quotes the base section it replaces. Honest
deltas of the shipped build are in
[implementation-notes.md](implementation-notes.md) and are designed against,
not around.

Synthesis of the three council proposals
(p1-verification, p2-simplicity, p3-dx — `docs/research/council/consumer-path/`
in git history, removed 2026-09-02) under three judge
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

> **C1 amended by §0.5 S2 (2026-08-31).** The *principle* — every decision out
> of the TS wrapper — is preserved and strengthened. The *instance*
> `check_manifest` is withdrawn: with `apply_feed` shipped in
> `crates/consumer`, a standalone staleness method is a second arbiter beside
> `apply.rs:197-207`, and two arbiters drift. Staleness is now a field on the
> terminal Step (§3.3); `DiscoverOut` and `export_reference_cursor` are
> unchanged.

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

Added by §0.5 (2026-08-31): the §3.3 push ABI (`apply_snapshot`/`apply_epoch`/
`apply_head`) · a second staleness arbiter beside `apply_feed` · a prefix
denylist over key-derived meta keys · a clock or any time source in the wasm
import allowlist · `MemStore`'s base collapsed to latest-per-slot ·
`refresh_spent`/`prune_missing_notes` as overridable default trait methods ·
a second `MemStore` authored in the wasm crate · verification obligations
in the TypeScript wrapper · a key in any constructor · `DelegatedClient` on the
main entry point · a demo path that submits a transaction or embeds an account
key · an unqualified `IDENTICAL`/`DIFFERENT` verdict rendered across two feed
states · any performance number in the README or a live demo panel that was not
measured.

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

> **§0.4.1 superseded in part by what landed — see §3.10 (2026-08-31).** The
> shipped trait is row-level, not `NoteSet`-valued; the apply half is one
> `apply_feed` rather than three pushed entry points; `sync_once`,
> `refresh_spent`, `prune_missing_notes` and the pass loops are free functions
> generic over `ConsumerStore`; and `MemStore` already exists in this crate.
> §3.10 lists the deltas that remain owed, including two blocking corrections.
> The one sentence above that is unchanged and load-bearing is the last: **one
> report schema, one golden oracle.**

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


## 0.5 Second-pass conflict resolutions (ts-api council, 2026-08-31)

§0.1–0.3 resolved the first council (consumer-path). A second council revised
§A3/§A4 against the shipped `crates/consumer` and against the verified
positioning fact that our customer holds a viewing key; its three judges — a
wallet engineer who has to ship it, a privacy auditor, and a five-year
maintainer — disagreed on every one of the three questions. Each disagreement is
recorded here with the ruling and the reason, because a synthesis whose
trade-offs are invisible is one refactor away from being undone. Rulings are
argued from merit; a judge majority is evidence, never the argument.

**S1 — which ABI wins for feed apply. Three judges, three winners.** Wallet
engineer: batched `plan()`/`apply_staged()`. Auditor: `need()`/`provide()`/
`advance()`, because `need()` takes no arguments and is therefore the strongest
structural statement of blindness. Maintainer: the `sync_begin`/`sync_supply`
trampoline, because the module emits the request set by *running* the real
`apply_feed` rather than by predicting it. **Ruling: the trampoline (§3.3), plus
an advisory `prefetch` hint on the Step the module already emits.** The
maintainer's objection is the one that compounds over five years — `plan()` and
`need()` are a *second* derivation of the fetch list that must stay in step with
what `apply_feed` actually asks for, and the drift is silent (over-prediction
wastes bandwidth, under-prediction adds round trips, neither fails a test). The
wallet engineer's objection to the trampoline is real and fatal if unanswered —
one outstanding request means 515 sequential round trips on mainnet — and the
maintainer's own graft answers it without a second authority: demote the
predicted list from an ABI method to a hint field, keep the module asking for and
verifying each artifact individually. The auditor's blindness argument survives
intact and is strengthened: the request sequence is now the output of a function
whose inputs contain no key at all, which §3.9's proptest asserts directly.

**S2 — `check_manifest`.** Auditor and maintainer: delete it (two staleness
arbiters drift). Wallet engineer: silent, having adopted an ABI that folds it in.
**Ruling: deleted**, staleness becomes a field on the terminal Step. This
withdraws the *instance* named in §0.2 C1 while preserving and strengthening its
*principle*; the amendment is written into §3.3 so the withdrawal cannot be read
as drift.

**S3 — key retention in the module.** Wallet engineer: must not ship, hold as a
v2 option. Auditor: ship only as the backing for `staticAccount`, with the
isolation claim corrected. Maintainer: ship as an opt-in `client.unlock(account)`
requiring a worker. **Ruling: deferred to v2 with a named trigger (§3.3).** The
deciding fact is the auditor's own correction — `WebAssembly.Memory.buffer` is a
plain `ArrayBuffer` readable by any same-origin script in the same worker, so
wasm linear memory is not a security domain relative to the JS heap beside it.
The only real benefit left is that `zeroize` executes, and it accrues to the one
caller that already holds the bytes for its process lifetime. The costs — a
key-holding `Engine`, a larger key-accepting entry set, a lifecycle in the ABI's
first version — are paid by everyone. If it ships later, it ships in the
maintainer's shape, with the auditor's memory-dump leg as a blocker.

**S4 — slicing discovery by wall clock.** All three judges rejected the clock
import. **Recorded as unanimous:** `discover_step(handle, max_ops)`, calibrated
in TypeScript (§3.3, §3.9). Reopening a purity gate by redefining one of its four
nouns is the move the gate exists to prevent, and an ops budget makes the slicing
deterministic, which is what lets leg **δ** assert that sliced and unsliced runs
agree.

**S5 — `MemStore`'s base as latest-per-slot, with a new error band.** Auditor and
maintainer: breaks `check_reachability` and `verify_anchors`, both of which fold
roots at anchors far below `last_epoch_to`. **Ruling: rejected** (§3.2); the full
write log stays and `BOUND_UNSUPPORTED` is not added. The proposal would have
traded the only grounding a cold-started client obtains for ~3 % of the smallest
memory term.

**S6 — `refresh_spent` / `prune_missing_notes` as default trait methods.**
Wallet engineer and maintainer: must not ship — they are already free generic
functions, which cannot be overridden, and the rule they encode (a spent note's
slot is not cleared) is the one live-run §7 proved subtle. Auditor: had asked for
them on the trait. **Ruling: free functions stay** (§3.10 item 6).

**S7 — the owner scope.** The auditor's design has `notes`/`upsert_note`/… return
`SCOPE_CLOSED` unless an owner scope is open. **Ruling: not adopted in that
form** (§3.2). It would break the conformance leg, which runs the shipped
`sync_once` over `MemStore` natively with no facade to open a scope, and it would
make the two stores behave differently on precisely the path the leg exists to
compare. The two properties it was buying are obtained instead by the closed
export allowlist (nothing key-derived can be serialised) and by
`discover_finish`/`discover_abort` clearing and zeroizing the session. All three
judges' actual requirement — **allowlist, never a prefix denylist** — is adopted
in full, together with the cursor/generation trait methods that make the mistake
unrepresentable rather than merely refused.

**S8 — ring 6 in the browser.** Wallet engineer and auditor: graft the proposed
`anchor_request()` / `verify_anchor()` pair, because a browser must be able to
reach `anchored`. Maintainer: that pair re-implements candidate selection, the
`MAX_GROUNDING_CANDIDATES` truncation, the MATCH/MISMATCH/UNAVAILABLE
classification and the reset-on-mismatch discipline in TypeScript — the same
mistake §3.3 made for the apply half — and `ProofSource` can simply be parked
like the transport. **Ruling: the maintainer's mechanism, delivering the other
two judges' goal** (§3.3.1's `Step::Rpc`). Choosing the trampoline in S1 is what
makes this free.

**S9 — the tripwire's failure mode.** The trampoline's "pending with no armed
request" was specified as a `panic!`. The wallet engineer noted that
`mem.rs`'s `lock()` uses `.expect("mem store poisoned")`, so a panic while a
guard is live kills the Engine for the session. **Ruling: it is an `Err`, and
`lock()` recovers from poisoning** (§3.2). R-J's tripwire for the *engine*
driver is unchanged.

**S10 — npm winner.** Wallet engineer and maintainer: the integrator surface
(`Account`, key-free `sync()`, `signal`, progress, multi-account,
`anchorPolicy`). Auditor: the privacy surface (`net.ts` chokepoint, closed
unions, delegated subpath). Their graft lists agree almost completely.
**Ruling: the integrator's shapes with the auditor's mechanisms and the
performance paper's browser realities**, which is what all three judges' own
adoption lists describe (§4.2, §4.10, §4.11).

**S11 — the `persist` default.** Auditor and maintainer: `'both'`. Wallet
engineer: `'both'` on the epochs lane only; the snapshot lane's default belongs
to a pre-registered gate that has not run. **Ruling: default `'both'`, justified
solely by the L2 measurement, with the snapshot lane's default explicitly owned
by §4.6's L1 FILL-IN and expected to become `'raw'` if it measures ≤ 500 ms.**
Nobody may flip that arm by argument.

**S12 — demo winner. Again three judges, three winners.** Wallet engineer: the
cold/warm columns and the honesty rules. Auditor: the network panel. Maintainer:
the cards/stages/log shell and "discovery makes zero requests". **Ruling: all
three, in the arrangement each judge's own adoption list describes** — the
shell resolves the brief-versus-orchestrator tension explicitly (cards never
scroll away, the log scrolls, the mutating last line is in the log), the columns
are the top card, the panel is the side card. Specified in
[demo-app.md](demo-app.md), with the individual demo conflicts (S13–S17)
resolved there and summarised in its §11.

**S13 — the pending line's deadline.** Wallet engineer: cancel plus an automatic
`warn` commit carrying **no** latency claim. Auditor and maintainer: cancel, no
timeout, because "a timeout would produce a number that looks like a measurement
and is not one". **Ruling: the wallet engineer's synthesis** — an eternal spinner
is every demo's failure mode, and the objection is to the *number*, not to the
commit (demo-app.md §5.4).

**S14 — the identity comparison's verdict.** Wallet engineer: URL-sequence
equality is the strict claim and a byte difference from a head cut must not
render as a failure. Auditor: pin the manifest hash for the comparison and never
render a verdict across two feed states. Maintainer: both runs must start from a
deleted database. **Ruling: all three, composed** (demo-app.md §7). Each catches
a different way the comparison can lie.

**S15 — rendering the page's own CSP as evidence.** Auditor: the header is the
claim and the list is the evidence. Maintainer: a page displaying its own meta
tag proves nothing to a sceptic and is decoration in the one panel that must be
all evidence. **Ruling: ship the CSP, render it, label it as the page's declared
policy rather than as evidence**, and say in the panel that the viewer can
confirm it independently. The evidence is the request list, the identity
comparison and the scanner (demo-app.md §6.1).

**S16 — the dev mode that submits real transactions.** All three judges:
must not ship. A `VITE_`-prefixed variable is inlined into the client bundle by
design, so one misconfigured CI job publishes a funded account private key from
the page whose entire claim is that we never touch keys — and it contradicts the
roadmap's deliberate no-write-path decision. **Recorded as unanimous and cut**
(demo-app.md §9).

**S17 — a "paste your viewing key" control in the published build.** Raised by
the maintainer alone and uncontradicted: a page under our name that asks you to
paste a wallet secret teaches the behaviour that gets our users phished later.
**Ruling: adopted.** The hosted build offers a generated demo key and the REPLAY
identity; paste is behind a local-build flag (demo-app.md §4).

Two corrections bind regardless of any of the above, are cheap, and land with
step 0a rather than with the wasm crate: the epoch path's missing `entry.zst`
check (a live R-I violation in shipped Rust), and `apply_feed`'s
trust-on-first-use adoption of the feed's own genesis. Both are in §3.10.

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

Second pass, 2026-08-31. §A3 was written against the WASM spike; the consumer
state machine now exists as shipped code (`crates/consumer`), and everything
below is designed against **that tree**, not against the sketch in §0.4.1.
Where the two disagree, the tree wins and the disagreement is named. The
changelog is §3.11; the conflicts resolved between the three judges are §0.5.

### 3.1 Crates

```
crates/consumer     strk20-consumer  — Block B core: apply/verify, the discovery
                                       passes, the registry, the report, AND
                                       `mem::MemStore` (the in-memory store)
crates/wasm         strk20-engine    — cdylib: the wasm-bindgen facade, the two
                                       parked hosts (ParkingTransport,
                                       ParkingProofSource), the AEAD seal, the
                                       state blob codec
```

**`MemStore` is NOT authored in the wasm crate.** It already exists at
`crates/consumer/src/mem.rs` (446 lines, `Arc<Mutex<Inner>>`, `type View =
MemView` with no lifetime) and it is the second implementation the conformance
leg runs `sync_once` over. A browser-only copy would make that leg cover a store
the browser does not use, which is precisely the equality claim this design
rests on. §3.2's amendments are therefore edits **to that file**, and the
conformance leg keeps running over the same type the browser folds into.

**Status of the exploratory crate in the tree.** `crates/wasm` already contains a
prototype facade (`stage_manifest` / `stage_epoch` / `stage_snapshot` / … /
`apply(cold_start)` / `apply_head` / `check_manifest` / `discover` /
`export_state` / `load`) over a `StagedTransport`. Two of its decisions are kept
and are cited below as prior art: `decompress` looks the inflated payload up by
`sha256(compressed)` and enforces the cap on Block B's side of the seam (§3.4),
and the facade module carries the crate's single `unsafe_code` exemption (§3.9).
The rest is **superseded by §3.3**: staging is driven by TypeScript deciding what
to stage, which is the second-planner shape §0.5 S1 rules against, and
`check_manifest` is a second staleness arbiter (§0.5 S2). The prototype's
fixture, its golden reports and its example generator are kept and become the
Node harness of step 3.

`crates/wasm` deps, with the features pinned rather than implied
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
network/storage features, **and no time source of any kind** (§3.9). `default-features
= false` is load-bearing on every RustCrypto line, not tidiness, and leg **s**
asserts the *feature-resolved* dependency graph (`cargo tree -e features
--target wasm32-unknown-unknown`), not merely the set of crate names — a
name-only walk cannot see a feature-flag regression, which is exactly how this
one would have shipped. The workspace already records the measured effect of
`discovery-core`'s `default-features = false` (142 → 118 crates on wasm32); that
number moves from a comment into the diffed CI tree.

### 3.2 The store, and the two drivers

> **§3.2 quoted, replaced.** §3.2 declared a `MemStore` with `base`/`tail`
> compartments and `pub struct MemView<'a> { store: &'a MemStore, bound: u64 }`.
> That does not compile against the shipped trait: `ConsumerStore::View` has **no
> lifetime parameter** and the trait requires `Send + Sync`, so the view owns an
> `Arc` handle and the interior mutability is `std::sync::Mutex` — never
> `RefCell`, which is not `Sync` and whose `unsafe` workaround is what §3.9's
> `deny(unsafe_code)` posture exists to forbid. The tail is not a compartment
> either: it is expressed through `replace_range(Range::Above { floor })`, which
> is how the shipped store already models it.

The store is `crates/consumer/src/mem.rs` as it stands, with three amendments.

**(a) The event arena.** `Inner.events` is today `BTreeMap<(u64,u64), EventRec>`
with a `Vec<Felt>` for `keys` and another for `data` — two allocations per
event, 118,960 times on the mainnet epochs lane. Replaced by a flat arena:

```rust
struct EventHdr { block: u64, index: u32, kind: u8, _pad: [u8;3],
                  tx:   u32,            // index into `tx_hashes`
                  keys: (u32, u16),     // (offset into `felts`, count)
                  data: (u32, u16) }
struct Events { hdr: Vec<EventHdr>,     // ascending by (block, index)
                felts: Vec<Felt>, tx_hashes: Vec<Felt> }
```

`RawEventAccess::get_events` binary-searches `hdr` on `block` and materialises
only the events it returns; nothing is allocated per event at fold time. Sizing
on the epochs lane, **[est]** from the measured volumes (118,960 events,
139,131 writes over 134,879 slots, 28,383 pool-active blocks):

| component | naive layout | arena layout |
|---|---|---|
| slots | ~12.6 MB | ~12.6 MB |
| blocks | ~2.3 MB | ~2.3 MB |
| events | **45–60 MB** | **33.4 MB** |
| live data | 60–75 MB | **~48 MB** |
| realistic peak linear memory | 90–110 MB | **70–85 MB** |

On the snapshot lane the events term collapses to whatever the epochs above the
basis carry, and the whole store is ~15 MB **[est]**. That 5× difference is what
decides whether a mobile tab survives, and it is why `close()` must terminate
the worker (§4.11): wasm linear memory never shrinks, and dropping an instance
does not return it to the OS.

**Rejected: collapsing the write log to latest-per-slot.** It would break
`check_reachability` (`apply.rs:563`) and `verify_anchors` (`anchors.rs:45`),
both of which call `full_slot_set_as_of(a.block)` at **every published anchor
between the basis and head** — and `anchors.ndjson` is head-captured and
append-only, so most of its entries sit far below `last_epoch_to` by the time a
client folds them. Under the collapse those calls error on the ordinary path,
and the only grounding a cold-started client can obtain stops working. The
saving would have been ~3 % of the smallest term (139,131 writes over 134,879
distinct slots ⇒ ~0.4 MB). The full write log stays, and the proposed
`BOUND_UNSUPPORTED` error is **not** added: `BOUND_BELOW_SNAPSHOT`
(`apply.rs:41`) already covers the one case that is genuinely unanswerable.

**(b) Cursors and generations get their own trait methods.** Today the
discovery half persists key-derived state through `meta_set`:
`sync.rs:102-109` composes `format!("cur_{kind}_{a}")` / `ckpt_` / `ckpt_at_`,
`sync.rs:404` composes `gen_{owner}`, and `save_cursor` writes a serialized
`DiscoveryCursor` — **which carries channel keys** — into the same `BTreeMap`
as `head_etag`. An `export()` that iterated `meta` would ship channel keys and
an owner address into a plaintext IndexedDB blob. `ConsumerStore` therefore
gains six methods, and `sync.rs` uses them instead of `meta_*`:

```rust
#[derive(Clone, Copy, PartialEq, Eq)] pub enum CursorKind { Incoming, Outgoing }
#[derive(Clone, Copy, PartialEq, Eq)] pub enum CursorSlot { Live, Checkpoint }

fn cursor_get(&self, k: CursorKind, s: CursorSlot, owner: &Felt) -> Result<Option<String>>;
fn cursor_put(&self, k: CursorKind, s: CursorSlot, owner: &Felt, json: Option<&str>) -> Result<()>;
fn checkpoint_at(&self, k: CursorKind, owner: &Felt) -> Result<Option<u64>>;
fn set_checkpoint_at(&self, k: CursorKind, owner: &Felt, block: u64) -> Result<()>;
fn owner_generation(&self, owner: &Felt) -> Result<u64>;
fn set_owner_generation(&self, owner: &Felt, gen: u64) -> Result<()>;
```

`FeedStore` keeps writing all of it into its existing `meta` table (no behaviour
change, no migration, `full_resync` becomes six `cursor_put(None)` calls). The
gain is that the type system now separates key-independent mirror metadata from
key-derived per-owner state, and `MemStore`'s exporter **cannot name** the
key-derived side.

**(c) A closed allowlist for exportable meta, as the compensating control.**
Even with (b), `meta_set` stays an open `&str` API. `MemStore::meta_set`
therefore rejects with `SCOPE_VIOLATION` any key outside the closed set

```
{ pool, chain_id, epoch_size, genesis_block, last_epoch_applied, last_epoch_hash,
  last_epoch_to, head_etag, head_number, head_hash, l1_accepted, history_floor,
  snapshot_basis, snapshot_pending_grounding, tail_generation }
```

and `export()` **serialises from that list by name, never by iteration**. A
prefix denylist (`cur_`, `ckpt_`, `gen_`) is forbidden: those strings are
`format!`-composed in another module, so the filter is one rename away from
drifting open, silently. After (b) this control should never fire; it exists so
that a future refactor which reintroduces key-derived meta fails loudly instead
of leaking.

**Deliberately not adopted: making the note-registry methods require an open
owner scope.** The proposal was `notes`/`upsert_note`/`set_note_spent`/
`delete_note`/`delete_owner_notes` returning `SCOPE_CLOSED` unless a scope is
open. It would break the conformance leg, which runs the shipped `sync_once`
over `MemStore` natively with no facade to open one, and it would make the two
stores behave differently on the exact path the leg exists to compare. Instead:
the registry lives in `Inner.notes` as it does today; the two properties that
mattered are obtained elsewhere — nothing key-derived is ever exported (by
allowlist, above), and nothing key-derived outlives a discovery session
(`discover_finish` and `discover_abort` both clear the registry and zeroize the
session, §3.3).

**Execution model — two drivers, two rules.**

*The engine may never pend.* Over `MemView` every `discovery-core` future is
`Ready` by construction, so R-J's tripwire is unchanged:

```rust
fn drive<F: Future>(f: F) -> F::Output {
    f.now_or_never().expect("engine future pended over an in-memory view")
}
```

One repair is owed to it: `MemStore::lock()` is `self.inner.lock().expect("mem
store poisoned")`, so a panic anywhere while a guard is live would kill the
Engine for the rest of the session. `lock()` recovers instead
(`unwrap_or_else(PoisonError::into_inner)`), because the tripwire is a
programming-error signal and must not also be a denial of service.

*The feed may pend, and only at the transport.* `apply_feed` is `async` solely
because `FeedTransport` is. The browser keeps the function and parks the host:

```rust
/// The browser's FeedTransport. It performs no IO. Each method records the
/// request it wants and returns a future that is Pending until the wrapper
/// supplies a response, then Ready.
struct ParkingTransport {
    pending:  Mutex<Option<FeedRequest>>,
    supplied: Mutex<Option<FeedResponse>>,
    log:      Mutex<Vec<LoggedRequest>>,   // canonical, key-independent (§3.3)
}
```

The run is a boxed future over **owned** handles (`MemStore` is `Clone` over an
`Arc`; the transport is an `Arc`), so it is `'static` and needs no self-reference
and no `unsafe`. `Engine::pump` polls it once against a no-op waker:

```rust
fn pump(&mut self) -> Result<Step, EngineError> {
    match self.run.as_mut().poll(&mut Context::from_waker(&noop_waker())) {
        Poll::Ready(out)  => Ok(Step::Done(out?)),
        Poll::Pending     => match self.transport.take_pending() {
            Some(req) => Ok(Step::Fetch(req)),
            None      => match self.proofs.take_pending() {
                Some(rpc) => Ok(Step::Rpc(rpc)),
                // Nothing but a parked host may pend. This is a programming
                // error, and it is an Err rather than a panic because a panic
                // here can fire while a store guard is live.
                None => Err(EngineError::internal("PARK_WITHOUT_REQUEST")),
            },
        },
    }
}
```

This is what makes the design's central claim structural rather than tested:
**the sequence of requests is the output of a function whose inputs are
(profile, persisted mirror, server responses)** — no key, no address, no owner
— so two identities produce identical logs by type, which a Rust proptest
asserts directly (§3.9) and the wire capture then corroborates.

### 3.3 Exported ABI (exact)

> **§3.3 quoted, replaced.** §3.3 exported `check_manifest`,
> `apply_snapshot(payload, manifest_snapshot_json, anchor_json)`,
> `apply_epoch(payload, manifest_entry_json)` and `apply_head(payload)` as
> independent entry points, leaving TypeScript to decide which artifacts to
> fetch, in what order, when to cold-start, when to fall back and when to reset.
> **All four are deleted.** The ordering *is* the trust logic, and it now exists
> once, in `crates/consumer/src/apply.rs`: the snapshot ladder, the
> `SnapshotRejected → reset_mirror → Epochs` fallback (`apply.rs:86-106`), the
> manifest-divergence check (`:197-207`), the masked-reorg contradiction test
> (`:236`), the `tail_from > last_epoch_to + 1` mid-sync bail (`:273`), and the
> grounding order (`:330`). Re-expressing that in the layer with the weakest
> tests is the single largest correctness risk in the package, and the `auto`
> fallback is not even expressible in the push ABI, because a rejected snapshot
> changes what must be fetched after the driver has already decided.

**Amendment to §0.2 C1.** C1 ruled "P3's shapes, P1's mechanics", and named
`check_manifest` arbitrating staleness in Rust as an instance of the principle
"keep every decision out of the TS wrapper". The **principle is preserved and
strengthened** here; the **instance is withdrawn**. With `apply_feed` shipped,
`check_manifest` would be a *second* staleness arbiter beside `apply.rs:197-207`,
and two arbiters drift. The three discriminants survive as a field on the
terminal Step (`staleness: "ok" | "behind" | "diverged"`), §3.7's rule that
staleness is a return value and never a throw is unchanged, and leg **q** still
asserts the three cases on the three constructed manifests.

`wasm-bindgen`, `--target web` (plus a `nodejs` build for tests). Every fallible
method throws a `JsError` whose message is one canonical JSON object (§3.7). All
inputs are bytes or JSON strings; all outputs are JSON strings, `Uint8Array`, or
`DiscoverOut`.

```rust
#[wasm_bindgen]
pub struct Engine { /* MemStore + Option<SyncRun> + sessions */ }

#[wasm_bindgen]
impl Engine {
    // ---------------------------------------------------------------- setup

    /// `profile_json` is the §6.1 ChainProfile the caller expects — a built-in
    /// or a custom one. Identity is pinned HERE, before a byte is requested;
    /// genesis.json is then checked against it INSIDE apply_feed (§3.10), not
    /// by the wrapper. This closes the trust-on-first-use hole at
    /// `apply.rs:119-130`, where an empty mirror adopts whatever the feed says.
    #[wasm_bindgen(constructor)]
    pub fn new(profile_json: &str) -> Result<Engine, JsError>;

    /// {"chain_id","pool","genesis_block","epoch_size","last_epoch",
    ///  "last_epoch_hash","last_epoch_to","history_floor","snapshot_basis",
    ///  "snapshot_pending_grounding","head","l1_accepted","slots","blocks",
    ///  "events","verified","engine_version","state_dirty"}
    pub fn info(&self) -> String;

    // ------------------------------------------------------------ feed sync

    /// Start one sync. `cold_start` ∈ {"auto","snapshot","epochs"} (§4.2's one
    /// vocabulary). Returns the first Step. SYNC_IN_PROGRESS if a run is open.
    pub fn sync_begin(&mut self, cold_start: &str) -> Result<String, JsError>;

    /// Satisfy the outstanding request and get the next Step. `meta_json` is
    /// the response envelope (§3.3.1); `compressed` is the bytes exactly as
    /// served; `payload` is the inflated bytes for zstd artifacts, else None.
    /// The module hashes BOTH (§3.4) — TypeScript performs no verification.
    pub fn sync_supply(&mut self, meta_json: &str,
                       compressed: Option<Vec<u8>>,
                       payload: Option<Vec<u8>>) -> Result<String, JsError>;

    /// Satisfy an outstanding Step::Rpc (§1.5 ring 6) with the `result` object
    /// of the user's own endpoint, or with an `unavailable` envelope.
    pub fn sync_supply_rpc(&mut self, meta_json: &str, result_json: Option<String>)
        -> Result<String, JsError>;

    /// Abandon an open run. Every store write is already atomic
    /// (`install_snapshot` / `replace_range` carry their metadata), so an abort
    /// is never a torn state — only an older one.
    pub fn sync_abort(&mut self);

    /// Canonical NDJSON of every request this Engine has emitted since
    /// construction, and its sha256. Key-independent BY CONSTRUCTION (§3.2).
    /// This is the artifact the demo compares across identities and the one the
    /// §3.9 proptest pins.
    pub fn request_log(&self) -> String;
    pub fn request_log_sha256(&self) -> String;

    // ------------------------------------------------------ persisted state

    /// Serialize epoch-derived state (§3.5) into a staging buffer; returns its
    /// length. Call only when info().state_dirty.
    pub fn export_begin(&mut self) -> Result<u32, JsError>;
    /// Copy frame `i` (≤ 4 MiB) out. Frames are contiguous and in order.
    pub fn export_chunk(&self, i: u32) -> Result<Vec<u8>, JsError>;
    pub fn export_end(&mut self);

    /// Restore. Frames are pushed in order; `finish` verifies the trailer hash,
    /// the stamp against `profile_json`, and every structural bound — and NEVER
    /// partially loads.
    pub fn load_begin(profile_json: &str) -> Result<Loader, JsError>;

    // -------------------------------------------------- key-accepting entries
    // The closed set is exactly {discover_begin, history, export_reference_cursor},
    // named in a checked-in allowlist file diffed in CI (§3.9) rather than by a
    // magic count in prose.

    /// Open a discovery session for one owner. `key` is copied into a
    /// `SecretFelt` and the caller's staging buffer is zeroized before return.
    /// `entropy32` MUST be 32 fresh bytes from crypto.getRandomValues on EVERY
    /// call (§3.6); it is held by the session and consumed by `finish`.
    pub fn discover_begin(&mut self, owner_hex: &str, key: &mut [u8],
                          sealed: Option<Vec<u8>>, entropy32: &[u8])
        -> Result<u32, JsError>;

    /// Run engine passes until `max_ops` IO-budget units are consumed or the
    /// current phase completes. Returns
    /// {"done":bool,"phase":"ckpt_in"|"ckpt_out"|"live_in"|"live_out"|"spent"|"done",
    ///  "ops":N,"ops_total":N,"channels":N,"notes":N}
    /// NOTE: ops, never milliseconds. The module has no clock (§3.9); TypeScript
    /// owns the clock, calibrates ops-per-millisecond across calls, and picks
    /// its own slice. This also makes the slicing deterministic, which is what
    /// lets leg δ assert that sliced and unsliced runs agree.
    pub fn discover_step(&mut self, handle: u32, max_ops: u32)
        -> Result<String, JsError>;

    /// Persist cursors, refresh spent state, seal, produce the report, clear the
    /// registry and zeroize the session. Legal only after a step said done.
    pub fn discover_finish(&mut self, handle: u32) -> Result<DiscoverOut, JsError>;

    /// Abandon a session: clears the registry, zeroizes key and entropy.
    /// Idempotent. A session that never finishes never consumes its entropy, so
    /// a torn discovery cannot burn a nonce.
    pub fn discover_abort(&mut self, handle: u32);

    /// Paged tx history per §1.1's paging contract. A walk that crosses
    /// history_floor TERMINATES the page set; an explicit
    /// from_block < history_floor throws HISTORY_UNAVAILABLE. One-shot: it is
    /// already bounded by `limit`.
    pub fn history(&mut self, owner_hex: &str, key: &mut [u8],
                   sealed: Option<Vec<u8>>, from_block: Option<u64>, limit: u32)
        -> Result<String, JsError>;

    /// Reference-schema DiscoveryCursor JSON (base §7.4) extracted from a sealed
    /// blob — Tier-0 migration to compat/SDK without resync.
    pub fn export_reference_cursor(&self, key: &mut [u8], sealed: &[u8])
        -> Result<String, JsError>;
}

#[wasm_bindgen(getter_with_clone)]
pub struct DiscoverOut {
    pub report_json: String,   // strk20_consumer::sync::SyncReport, field-identical
                               // to `strk20-sync sync --json` (one golden oracle)
    pub sealed: Vec<u8>,       // checkpoint-only sealed blob; hand back next time
    pub added_json: String,    // notes not present in the supplied sealed blob
    pub spent_json: String,    // nullifiers that flipped to spent this pass
    pub stats_json: String,    // {"slots_read":N,"events_scanned":N,"passes_in":N,
                               //  "passes_out":N,"ops":N,"cursor_reset":false}
                               // counts only; scanner-asserted key-clean like
                               // every other string this module emits
}
```

**Deliberately absent: any method returning a duration.** The module has no
clock. Timing is TypeScript's, measured around the call, which is also the only
place it can honestly include `fetch` and zstd.

**Deliberately absent in v1: `key_retain` / `key_forget` / retained-handle
discovery.** It was proposed to move long-lived key residency out of the JS heap
"into wasm memory where `zeroize` is real". Half of that is true and half is
not: `zeroize` genuinely runs, but `WebAssembly.Memory.prototype.buffer` is a
plain `ArrayBuffer` readable by any same-origin script in the same worker, so
wasm linear memory is not a security domain relative to the JS heap beside it.
The cost is paid by everyone — the module becomes a key-holding object and the
key-accepting entry set grows — and the benefit accrues only to
`staticAccount` (§4.2), whose caller already holds the bytes for the process
lifetime. **Deferred with a trigger:** a wallet with a biometric- or
hardware-gated keystore reporting that per-pass `viewingKey()` is a prompt
storm. If it ships it ships in the reviewed shape — `client.unlock(account)`
requiring `worker: true`, `status().unlocked`, an explicit `lock()`, a
TypeScript auto-lock timer (never a wasm-side one, which would need a clock),
and a blocking acceptance leg: after `key_forget_all`, a linear-memory dump
contains the key in none of the 13 encodings.

#### 3.3.1 Step and response envelopes (byte-precise)

Steps, canonical JSON, one object per call:

```json
{"step":"fetch","seq":3,"artifact":"epoch","path":"/feed/epochs/00000412.strk20e.zst",
 "optional":false,"compressed":true,"decompress_cap":67108864,
 "conditional":null,"reason":"epoch 412 > last_epoch_applied 411",
 "prefetch":[{"artifact":"epoch","path":"/feed/epochs/00000413.strk20e.zst",
              "compressed":true,"decompress_cap":67108864}, …]}

{"step":"fetch","seq":9,"artifact":"head","path":"/feed/head.ndjson",
 "optional":false,"compressed":false,"decompress_cap":null,
 "conditional":{"if_none_match":"\"<64-hex>\""},"reason":"tail refresh",
 "prefetch":[]}

{"step":"rpc","seq":11,"endpoint":"anchor","method":"starknet_getStorageProof",
 "params":[{"block_number":14151973},[],["0x…pool…"],[]],
 "also":[{"method":"starknet_getBlockWithTxHashes","params":[{"block_number":14151973}]}],
 "reason":"ring 6 candidate 1 of 4"}

{"step":"done","staleness":"behind","verified":"server-asserted","state_dirty":true,
 "outcome":{"epochs_applied":2,"tail_rewound":false,"tail_changed":true,
  "head":14151989,"l1_accepted":14146900,"last_epoch_to":14149999,
  "snapshot_basis":14059999,"snapshot_rejected":false,"history_floor":14060000}}
```

`artifact` is a **closed enum of eight variants** — `genesis`, `manifest`,
`epoch`, `epoch_anchor`, `snapshot`, `snapshot_anchor`, `anchors`, `head` —
mapping 1:1 onto §2.8.1's closed URL allowlist. `path` is emitted by the module;
the wrapper prefixes the configured feed base and appends **nothing**. No
variant carries a query string, so a query string is unrepresentable rather than
forbidden.

**`prefetch` is advisory and is the answer to serial round trips.** The
trampoline satisfies one request at a time, and on the epochs lane that is 515
strictly sequential RTTs on mainnet (609 on Sepolia) — tens of seconds of pure
latency on top of the fold. `prefetch` is a hint the module emits *from the same
verified manifest it is already walking*, so there is no second planner and no
second authority: the module still asks for each artifact individually and still
verifies each one. The wrapper may fetch the hinted paths in parallel into its
own buffer; `prefetchConcurrency` (default 6, §4.2) is a knob rather than a
contract. **Nothing is ever applied because it was hinted**: a staged artifact
the module never asks for is simply dropped, so a hostile hint is a wasted GET.

One honesty note that must be printed where the identical-stream claim is made:
`request_log()` is ordered by the module's own asks and is byte-identical across
identities; the **wire** order may interleave within a prefetch window, so the
wire claim is an identical *multiset* unless `prefetchConcurrency: 1`. Leg **u**
and §2.8.1's allowlist are multiset assertions and are unaffected; the demo's
A/B comparison uses `request_log_sha256`, which is the stronger and simpler
claim (§demo-app.md §7).

Response envelope supplied back:

```json
{"seq":3,"status":200,"not_modified":false,"absent":false,"etag":null}
```

- `absent: true` is the *only* encoding of 404, and only for artifacts the Step
  marked `optional` (`epoch_anchor`, `snapshot_anchor`, `anchors`). A 404 on a
  non-optional artifact is `TRANSPORT`, raised by the wrapper, never `absent` —
  `crates/client/src/transport.rs`'s `get_optional` discipline carried into
  TypeScript verbatim.
- `not_modified: true` is 304, for the one conditional artifact (`head`).
- Any other non-2xx is `TRANSPORT` from the wrapper and never reaches
  `sync_supply`.
- A `seq` that is not the outstanding one throws `SYNC_PROTOCOL {expected, got}`.

**Ring 6 rides the same trampoline.** `ProofSource` (`anchors.rs:32`) is an
async trait taking `(pool, block)`, so it parks exactly like the transport and
`Step::Rpc` is the result. The candidate selection, the
`MAX_GROUNDING_CANDIDATES` truncation, the MATCH/MISMATCH/UNAVAILABLE
classification, the `global_roots.block_hash` binding required by §12 point 3,
and the `reset_mirror`-on-mismatch discipline (`sync.rs:341-382`) therefore all
stay in Rust, shared with the native client. The alternative on the table —
exporting `anchor_request(block)` and `verify_anchor(block, proof, header)` —
would have re-implemented that ladder in the wrapper, which is the mistake §3.3
made for the apply half. Two details are normative and come from the live run:
the params array is `[]`, never `null` (LIVE-7), and `UNAVAILABLE` is a **value,
never a throw** (LIVE-6: a capability gap must never read as corruption). This
is what lets a browser reach `verified: "anchored"` at all — which matters
because the browser's user is the one who already has an RPC URL in their wallet.

**Resumability is a guarantee, not an accident.** `apply_feed_once` skips
`entry.e <= last_epoch_applied` (`apply.rs:209`) and each epoch's
`replace_range` carries its own metadata, so a run abandoned at epoch 200
resumes at 201 from persisted meta with no new state. Leg **γ′** asserts it: kill
the fetch at epoch 200, re-open, resume, and demand a byte-identical final export
blob. Without that leg every flaky mobile network costs a full 16 MB refetch.

### 3.4 Decompression, and where verification lives

> **§3.4 quoted, amended.** §3.4 said the module receives uncompressed payloads
> and *"TypeScript decompresses and is bound by R-I (verify the `zst` hash
> first, cap the output)"*. That places a verification obligation in the least
> testable layer. **Replaced:** the wrapper supplies **both** buffers and the
> module hashes both — the `.zst` sha256 against `manifest.epochs[i].zst` /
> `manifest.snapshot.zst`, and the payload sha256 against the content hash,
> exactly as `apply.rs:439-457` already does on the snapshot path. TypeScript's
> only remaining obligation is **not inflating past the cap the Step named**,
> which is a resource bound backstopped by the payload hash, not a verification.

Mechanically, `ParkingTransport::decompress(bytes, cap, artifact)` does not
decompress: it looks up `sha256(bytes)` among the pairs supplied through
`sync_supply` and returns the paired inflated buffer, raising
`DECOMPRESS_UNSTAGED {artifact, zst_sha256}` when the pair is absent. That
preserves `apply.rs`'s existing ring order byte for byte (ring 1 hashes the
compressed bytes *before* `decompress` is called; ring 2 hashes the payload
after) and it structurally forbids the wrapper from inflating bytes it did not
stage. zstd is still not compiled in (given), and content identity is still over
uncompressed bytes everywhere, so nothing is lost.

**Precondition on the native side, blocking (R-I).** `apply.rs:214-224` fetches
an epoch and calls `transport.decompress` having checked only the payload hash,
while the snapshot path checks `entry.zst` first (`:439-446`). `EpochEntry.zst`
exists (`crates/feed/src/manifest.rs:41`). That is a live R-I violation in
shipped Rust and an asymmetry between the two hosts on the shared code path.
Three lines, landed with step 0a, before the wasm crate exists.

### 3.5 State blob (`export` / `load`) — format v2

> **§3.5 quoted, replaced.** §3.5 specified canonical NDJSON with hex felts. On
> the epochs lane that is ~15 MB of slot lines plus ~24 MB of event lines ≈
> **40 MB [est]**, which costs ~250k hex parses, a 40 MB buffer on the wasm
> heap, a 40 MB copy into JS and a structured clone into IndexedDB — seconds of
> main-thread work spent to avoid seconds of fold work. Replaced by a framed
> hybrid, `strk20-state v2`.

```
line 1   : {"t":"hdr","v":2,"kind":"strk20-state","chain_id":"SN_MAIN","pool":"0x…",
            "genesis_block":8978970,"epoch_size":10000,"engine":"<crate semver>",
            "last_epoch":1406,"last_epoch_hash":"<64-hex>","last_epoch_to":14069999,
            "history_floor":14060000,"snapshot_basis":14059999,
            "snapshot_pending_grounding":false,"verified":"server-asserted",
            "body":{"enc":"bin1","len":N}}\n
body     : N bytes, little-endian framed, in this order:
             u32 n_slots,  then n_slots × { [u8;32] slot, [u8;32] value, u64 w }
             u32 n_blocks, then n_blocks × { u64 number, [u8;32] hash, [u8;32] parent, u64 ts }
             u32 n_events, then the §3.2 arena: hdr table, tx_hashes, felts
last line: {"t":"end","slots":N,"blocks":P,"events":M,"sha256":"<64-hex over all preceding bytes>"}\n
```

`jq` still reads the stamp (`head -1`) and the trailer (`tail -1`), which is
what §3.5's debuggability argument was actually about; nobody was going to `jq`
134,879 slot lines. Size **[est]** ~40 MB → ~17 MB, and the body is
`copy_from_slice` into the arena rather than parsed.

**Every §3.5 structural check survives, unchanged in strength and in two cases
stronger** — as array-bounds checks they cannot be defeated by a malformed line:

- no slot's `w`, block's `number` or event's `block` exceeds `last_epoch_to` —
  the parser-level form of "the tail is never exported", which is what leg **r**
  depends on;
- no block or event lies below `history_floor`;
- `snapshot_basis` is either absent (replayed mirror, `history_floor == 0`) or
  satisfies `history_floor == snapshot_basis + 1`;
- the trailer self-hash, and `load`'s three rejection codes, unchanged:
  `STATE_CORRUPT` / `STATE_VERSION` / `STATE_FOREIGN`.

The degenerate case §3.5 called out stays correct and stays worth stating: a
client that has applied **only** a snapshot has `history_floor = last_epoch_to +
1`, so it exports zero blocks and zero events. That is not a bug.

Three header amendments, each earning its place:

1. **`verified`** — the integrity grade is a property of how this mirror was
   built and must survive a reload. It is never *upgraded* by memory: a blob
   stamped `anchored` loads as `server-asserted` when the session configures no
   anchor RPC.
2. **`snapshot_pending_grounding`** — `apply.rs:79-85` discards a mirror
   carrying an ungrounded snapshot. A blob that dropped the flag would let a
   browser reload onto a slot set it never grounded and never re-enter the
   snapshot branch. Carrying it means the existing mechanism fires unchanged.
3. **The stamp is checked against the `ChainProfile`**, not only against a
   genesis document, because under §3.3 the profile is the identity source at
   construction.

`load` rejects and never partially applies. **Export is by allowlist, by name,
never by iteration** (§3.2c), so "only epoch-derived state is ever exported"
becomes a property of the serializer's shape rather than of its author's care.
Per-key material is never in this blob.

### 3.6 Sealed per-key state (checkpoint-only)

The construction is **unchanged**: `S20SEAL1` ‖ nonce(24) ‖
XChaCha20-Poly1305, HKDF key and nonce derivation with their domain separation,
the AAD, mandatory fresh `entropy32`, `ENTROPY_INVALID` on any length but 32,
and the corrected nonce doctrine in full — **nonce safety comes from the
caller's entropy, not from the counter**, with the `prev_entropy_h` guard stated
at exactly its real strength (it closes the constant-entropy case completely,
including across a fork; it does not close two forks caching two different stale
values, which is what fresh `getRandomValues` is for). Leg **q**'s nonce-safety
sub-leg is unchanged, including the test-only build with the guard disabled that
proves the guard is what prevents the collision.

The plaintext is amended in two places:

```
{"v":1,"counter":<u64>,"prev_entropy_h":"<64-hex>",
 "ckpt_at":<block ≤ last_epoch_to>,
 "ckpt_epoch":<epoch index containing ckpt_at>,
 "ckpt_epoch_hash":"<64-hex payload sha256 of that epoch>",
 "in_ckpt":<reference DiscoveryCursor JSON>,"out_ckpt":<reference DiscoveryCursor JSON>,
 "notes":[{note fields…},…]}          # ONLY notes with block <= ckpt_at
```

1. **`notes[]` is checkpoint-bounded.** §3.6 declared the blob checkpoint-only —
   "no live cursors, no generation counter, nothing bound to the tail" — and
   then sealed a `notes[]` with a `block` field and no bound. A note discovered
   by the live pass sits above `last_epoch_to` and can be reorged away; sealing
   it is exactly the durable tail state the browser design exists not to have.
   Rule: **seal only notes with `block <= ckpt_at`; return live notes to the
   caller and never seal them.** The live pass rediscovers them from the
   refetched tail next session, at the cost of a walk over ≤ one epoch. This is
   what makes "no persisted reorg logic at all" true rather than nearly true.
2. **`ckpt_epoch` + `ckpt_epoch_hash` make the seal invalidatable.** A cursor is
   a position in a history, and a diverged feed changes the history. On open the
   module compares `ckpt_epoch_hash` against the verified manifest's entry for
   `ckpt_epoch`; a mismatch is treated exactly like an AEAD failure — **no
   cursor, fresh discovery, `stats.cursor_reset = true`** — never an exception.
   Without it the seal is a cache with no invalidation rule.

`counter` keeps its demoted meaning (a rollback/authenticity signal inside the
AEAD and nothing more), and no text in this spec may re-attach nonce safety to
it. An AEAD failure is still "no cursor": fresh discovery with
`cursor_reset: true` surfaced — the correct behaviour for a different user on
the same origin. Cursors still use the exact reference JSON schema (base §7.4),
so sealed cursors round-trip with compat and `serve` wire cursors, and
`export_reference_cursor` still delivers Tier-0 migration without resync.

**In-memory reorg logic is unaffected and is not a contradiction.** `apply_feed`
still detects the masked reorg and the tail contradiction, still bumps
`tail_generation`, and `sync_once` still rewinds an owner whose generation
differs. In the browser all of that lives for one session, because the only
durable per-key artifact is checkpoint-only. The claim is about persistence, and
leg **r** is what proves it.

### 3.7 Error model

Every throw is `{"code":"<SCREAMING_SNAKE>","message":"…","details":{…},"retryable":bool}`.
Closed set, shared verbatim by `strk20-feed`, the wasm module, npm and `serve`.
§3.7's table stands — including the deletion of `STATE_STALE` and the standing
prohibition on reintroducing a thrown staleness error — with these changes:

| change | code | details | retryable | raised by |
|---|---|---|---|---|
| **added** | `SYNC_PROTOCOL` | `{expected_seq, got_seq}` | no | `sync_supply` for a response that is not outstanding, or re-entry into a closed run |
| **added** | `SYNC_IN_PROGRESS` | — | no | `sync_begin` while a run is open |
| **added** | `DECOMPRESS_UNSTAGED` | `{artifact, zst_sha256}` | no | the parked transport: the wrapper inflated bytes it did not stage (§3.4) |
| **added** | `SCOPE_VIOLATION` | `{key}` | no | `MemStore::meta_set` of a key outside §3.2c's allowlist — an internal invariant; escaping it is a bug |
| **added** | `SESSION_INVALID` | `{handle}` | no | `discover_step`/`finish` on an unknown, finished or aborted handle |
| **added** | `SESSION_INCOMPLETE` | `{phase}` | no | `discover_finish` before a step said `done` |
| **added** | `SNAPSHOT_UNREACHABLE` | `{basis, head, tried}` | no | §11.3 reachability — already raised by `apply.rs:569-631` and missing from the table |
| **added** | `SNAPSHOT_UNAVAILABLE` | — | no | `coldStart:'snapshot'` against a feed publishing none (`apply.rs:173-178`) — likewise missing |
| **added** | `ANCHOR_UNBOUND` | `{block, proof_block_hash, header_hash}` | no | ring 6, §12 point 3 |
| **added** | `KEY_UNAVAILABLE` | `{reason}` | **yes** | npm only — `Account.viewingKey()` rejected (locked wallet) |
| **added** | `ABORTED` | — | no | npm only — `AbortSignal` |
| **clarified** | `FEED_ADVANCED_MIDSYNC` stays, raised by `apply_feed` itself (`apply.rs:273-279`) | `{tail_from, floor}` | yes | — |
| **clarified** | `DECOMPRESS_LIMIT` is raised by TypeScript (its one resource obligation) and by Rust for uncompressed-artifact bounds | `{artifact, cap}` | no | — |
| **removed** | `BOUND_UNSUPPORTED` was proposed and is **not** added | — | — | `BOUND_BELOW_SNAPSHOT` already covers the only unanswerable case (§3.2) |

Ring 6's `UNAVAILABLE`, `plan`-style staleness, and `need_more`-style
continuation are all **control flow, never throws**. Every `message` and every
`details` value is asserted key-clean by the leg **q** scanner.

### 3.8 Consuming the fork until the upstream PR lands

Unchanged. Given: `starknet-providers` is declared but unused in
`discovery-core` at rev `74841ca`; feature-gating it is a two-line
`Cargo.toml` change (roadmap item 7).

1. Fork `starkware-libs/starknet-privacy` under our org; branch
   `strk20/providers-gate-74841ca` = the pinned rev **plus exactly one commit**
   touching only `discovery-core/Cargo.toml` (`optional = true`,
   `[features] default = ["providers"]`,
   `providers = ["dep:starknet-providers"]`).
2. **One** workspace-wide
   `[patch."https://github.com/starkware-libs/starknet-privacy.git"]` entry
   pinning the fork by rev, with the feature **default-on** so native builds are
   behaviorally identical to today; `crates/wasm` and `crates/consumer`
   set `default-features = false`. A split pin is **forbidden**: two sources for
   one git dependency in one workspace yields two `discovery-core`/`Felt` type
   identities.
3. The diff is vendored at `patches/discovery-core-providers-gate.patch`. CI job
   `fork-delta-check` asserts, on every run: the fork rev equals the upstream rev
   plus that patch, **and** `git diff <upstream>..<fork> --
   crates/discovery-core/src` is EMPTY — Cargo metadata only, zero source lines.
4. The upstream PR is filed at step 0b, in parallel with the refactor. On merge,
   the `[patch]` section and `patches/` file are deleted in one commit and the CI
   job inverts into a tripwire that fails if the `[patch]` section ever returns.

### 3.9 Purity and size gates (CI, run with the suite)

§3.9 stands entire — the feature-resolved dependency walk with its checked-in
diffed tree, the corrected `deny`-not-`forbid` posture with exactly one
documented `#[allow]` scope on the facade module, the checked-in
`import-allowlist.txt` diffed as a **file** rather than matched as a name
pattern, the honest restatement of what the audit proves (**the module cannot
open a network handle, a storage handle, a timer or a randomness source of its
own** — the import section is not empty, and `__wbindgen_*` calls into JS
carrying arbitrary strings are how every ABI method returns its JSON), the one
denominator for wire cost, and the size FILL-IN. Five amendments:

- **The import allowlist must contain no time source, and that is now named as a
  gate.** A wall-clock parameter for `discover_step` was proposed and is
  rejected: §3.9's audit states its property in terms that include timers, and
  reopening a purity gate by redefining one of its four nouns is exactly the
  move the gate exists to prevent. It would also make the module's behaviour
  device-dependent, which sits badly with the determinism leg **δ** asserts.
  Slicing is by op budget (§3.3); TypeScript already owns the clock and every
  other timing, and its calibration logic simply moves one layer up.
- **The key-accepting entry lock becomes a named allowlist file.** CI parses the
  wasm-bindgen-generated `.d.ts` and asserts that the set of exported methods
  taking a `Uint8Array` named `key` is exactly `{discover_begin, history,
  export_reference_cursor}` — a list, diffed as a file, for the same reason
  §3.9 stopped pattern-matching import names.
- **New gate: the request-emitter purity proptest.** In `strk20-consumer`,
  native, over `MemStore` and a scripted transport: for any two distinct
  (address, key) pairs and any feed fixture, `request_log()` after driving a sync
  to completion is **byte-identical**, and stays so when `discover_*` is
  interleaved between syncs. This is P-blind as a theorem about a key-blind
  function; the wire capture in leg **u** becomes its independent empirical
  check rather than its only evidence.
- **New gate: the Pedersen delta, measured at step 3 before any npm code.**
  `feed::mpt` pulls Pedersen, and Pedersen implementations ship large
  precomputed point tables. If `discovery-core`'s slot derivation already links
  the same tables the marginal cost is ~0; if not, the tables alone can approach
  the whole provisional 300 KB budget. Procedure: build with default features,
  record `gzip(wasm)`; build with `mpt` removed (ring 5 stubbed), record; the
  delta is the true cost of client-side root verification. **The split-module
  fallback is not designed until that number exists** — if the tables are shared
  it buys nothing and costs a second artifact to version. If the split ships, the
  denominator becomes the sum of what a cold snapshot-lane session downloads,
  including the second module, so the split cannot make the number look smaller.

  > **FILL-IN (Pedersen delta, pending step 3):** gzip(wasm) with `mpt` = ___ KB;
  > without = ___ KB; delta = ___ KB. **Split module: yes / no.** Date: ___.

- **New gate: the Pedersen MPT at *runtime*, which nobody had costed.**
  `check_reachability` ends a snapshot cold start with
  `strk20_feed::mpt::storage_root` over `full_slot_set_as_of` — a Pedersen MPT
  over ~135,000 mainnet slots, inside wasm, on a phone, on the path whose entire
  selling point is a fast cold start. Measure it at step 3 alongside the size
  delta. If it is seconds, grounding must move off the critical path (the sync
  completes at `server-asserted` and upgrades asynchronously), and that is an
  **ABI-shaped decision**, so it has to be known before step 4 rather than
  discovered during it.

  > **FILL-IN (MPT root runtime, pending step 3):** `mpt::storage_root` over the
  > mainnet slot set, in wasm, desktop / 4× throttle = ___ / ___ ms.
  > **Grounding stays on the critical path: yes / no.** Date: ___.

### 3.10 Deltas required of `crates/consumer` (0a completion)

Everything §A3 asks of the shared crate, in one place, so it lands with step 0a
rather than being discovered by the browser. All of it is small, all of it is in
the shared path, and the first two are **blocking and independent of the wasm
crate**.

1. **Check `entry.zst` before decompressing an epoch** (`apply.rs:214-224`), as
   the snapshot path already does. Three lines. A live R-I violation today.
2. **The exportable-meta allowlist and the cursor/generation trait methods**
   (§3.2b, §3.2c), plus `MemStore::lock` recovering from poisoning.
3. **`apply_feed` takes `expect: &ChainProfile`** and compares genesis against
   it, not only against stored meta (`apply.rs:119-143`). This closes the
   trust-on-first-use hole — an empty mirror today adopts whatever chain id and
   pool the feed declares — and it is what §6.2's stamping matrix already
   requires. Both hosts get the check; neither wrapper re-implements it.
4. **`ApplyOutcome` gains `verified: &'static str` and `state_dirty: bool`.**
   `verified` is computed in `sync_once` today; the browser needs it from the
   apply half because `sync()` is key-free (§4.2).
5. **The event arena** in `mem.rs` (§3.2a).
6. **`refresh_spent` and `prune_missing_notes` stay free generic functions.** A
   proposal to make them default trait methods "so `FeedStore` may override them
   for batched SQL" is rejected: a free function cannot be overridden and a
   default method can, and the one rule the live run showed is subtle — *a spent
   note's storage slot is not cleared, so spentness lives only in the nullifier
   slot* (live-run §7) — is exactly the rule that must not have two
   implementations.
7. **`ProofSource` stays the ring-6 seam** and is parked in the browser exactly
   like the transport (§3.3.1). No ring-6 decision moves into a wrapper.

Note for the record: the shipped `ConsumerStore` is **not** §0.4.1's nine-method
sketch and not the eleven-method surface the second-pass brief described. It is
seventeen methods today (`meta_get`, `meta_set`, `is_empty`, `block_hash`,
`block_hashes`, `read_slot_as_of`, `full_slot_set_as_of`, `view`,
`reset_mirror`, `install_snapshot`, `replace_range`, `tail_generation`, `notes`,
`upsert_note`, `set_note_spent`, `delete_note`, `delete_owner_notes`), plus the
six of §3.2b, with `apply_feed`, `sync_once`, `refresh_spent`,
`prune_missing_notes`, `run_incoming`, `run_outgoing`, `register_notes`,
`reopen_cursor` and `full_resync` as free functions generic over it. §0.4.1's
`NoteSet` value type was not built and is not being built: the row-level trait
is the better fit, and the `added`/`spent` pure diff it justified now happens in
`discover_finish` against the decrypted prior seal.

### 3.11 Changelog — second pass (2026-08-31)

What changed against the council's first pass, and why.

| § | first pass | now | why |
|---|---|---|---|
| 3.1 | `MemStore` lives in the wasm crate | it already lives in `crates/consumer/src/mem.rs`; the wasm crate is the facade only | a browser-only second store would make the conformance leg cover a store the browser does not use |
| 3.2 | `MemView<'a>` borrowing the store; `base`/`tail` compartments | owned `Arc` view with no lifetime, `Mutex` interior mutability, tail expressed through `Range::Above` | the shipped trait has `type View` with no lifetime and demands `Send + Sync`; the borrowed form does not compile and `RefCell` is not `Sync` |
| 3.3 | `apply_snapshot` / `apply_epoch` / `apply_head` pushed from TypeScript | `sync_begin` / `sync_supply` over the unmodified `apply_feed`, with an advisory `prefetch` hint | the ordering is the trust logic and now exists once; the `auto` fallback is inexpressible in a push ABI |
| 3.3 | `check_manifest` as a standalone arbiter (§0.2 C1's instance) | deleted; `staleness` is a field on the terminal Step | two arbiters drift; C1's principle is preserved and strengthened |
| 3.3 | `discover` as one synchronous full pass | `discover_begin` / `discover_step(max_ops)` / `discover_finish` / `discover_abort` | 1.19 s measured on Sepolia with one note; the engine's `IoBudget` pass loop already made it resumable and we were discarding that |
| 3.3 | — | ring 6 as `Step::Rpc` on the same trampoline | the browser can otherwise never reach `verified: "anchored"` while the CLI can — backwards, since the browser's user has an RPC URL already |
| 3.3 | "exactly two key-accepting entries" | three, named in a diffed allowlist file; `key_retain` deferred with a trigger | wasm linear memory is same-origin readable, so retention's only real win is that `zeroize` runs; that does not pay for a key-holding module in v1 |
| 3.4 | TypeScript verifies the `.zst` hash | the module hashes both buffers; TypeScript owns only the output cap | verification does not belong in the least testable layer; and the native epoch path's missing `entry.zst` check is fixed rather than papered over |
| 3.5 | NDJSON with hex felts, one buffer | v2: JSON header + binary body + JSON trailer, ≤4 MiB frames; header gains `verified` and `snapshot_pending_grounding` | ~40 MB → ~17 MB **[est]** and no parsing; the bounds become array checks, which are stronger than grammar checks |
| 3.6 | `notes[]` unbounded; no seal invalidation | notes bounded by `ckpt_at`; `ckpt_epoch` + `ckpt_epoch_hash` | otherwise the seal carries tail state and a cursor outlives the history it indexes |
| 3.9 | — | no-clock gate named explicitly; request-emitter proptest; Pedersen size **and runtime** gates | a clock import would have reopened an audited allowlist for a convenience; the MPT's runtime cost on a phone was uncosted by everyone |

Two cross-cutting reasons sit behind most of the table:

**The `ConsumerStore` reality.** §A3 was designed against a spike. Block B is
now shipped code with a named seam, a second store, and a conformance leg that
runs the same state machine over both. Every decision above is made against that
tree, and where the first pass assumed a different shape — a borrowed view, a
crate that does not hold `MemStore`, a `NoteSet` value type, an apply path that
TypeScript drives — the tree wins and the assumption is retired in writing.

**The customer holds a key.** Verified from the official Wallet API docs: *"No
viewing keys in your app. The wallet holds the user's viewing key"* and *"The
wallet discovers notes, builds the proof"*. A dapp on the Wallet API therefore
never sees a viewing key and can never call this module. Our consumer is a
**wallet or a key-holding app**, which is why the ABI is shaped around a
short-lived key handed in per pass and zeroized on return, why the key-free
phase (`sync`) is separately drivable, and why key *retention* is a deferred
option rather than a v1 primitive.

---

## A4 — npm package

Second pass, 2026-08-31. Changelog at §4.12; judge conflicts at §0.5; the demo
this package must carry is specified separately in
[demo-app.md](demo-app.md).

### 4.1 Name, layout, and who this is for

Unscoped **`strk20-discovery`** (unanimous; amends base **§12.1**, which named
it `@strk20/discovery-provider`). ESM + `.d.ts`, built with `tsc`, no bundler.

```
strk20-discovery
├── dist/index.js|d.ts        LocalDiscoveryProvider, KeylessClient, Account,
│                             types, errors            (package root)
├── dist/sdk.js|d.ts          alias re-export          (subpath "strk20-discovery/sdk")
├── dist/delegated.js|d.ts    DelegatedClient          (subpath "strk20-discovery/delegated")
├── dist/worker.js|d.ts       ~40-line worker host     (subpath "strk20-discovery/worker")
├── dist/engine_bg.wasm       strk20-engine, lazily instantiated
└── README.md
```

**Is this for you?** The README opens with this table, because the positioning
fact is the first thing an integrator can waste a day on.

| you are | do you hold a viewing key? | use |
|---|---|---|
| a dapp on the **Starknet Wallet API** | **no** — the wallet holds it, discovers your notes and builds the proof | not this package. You do not need it and cannot use it. |
| a **wallet**, or an app with its own keystore | yes | `LocalDiscoveryProvider` (this package, keyless: the key stays in the browser) |
| a **key-holding backend / self-hoster** | yes, server-side | `LocalDiscoveryProvider` in Node, or `DelegatedClient` against your own `strk20-sync serve` |

`LocalDiscoveryProvider` is exported **from the package root** and is the first
identifier in the README, because our actual customer's integration is one field
in `createPrivateTransfers({ discoveryProvider })`. `/sdk` remains as an alias
subpath so nothing breaks. All base §12.1 cursor-conversion semantics carry over
verbatim, so `NotesCursor`/`ChannelCursor` round-trip identically to
`IndexerDiscoveryProvider`.

`node >= 20` and evergreen browsers. The wasm loads via `new
URL('engine_bg.wasm', import.meta.url)` — untouched in Vite, webpack 5 and Next;
a `wasmUrl` option covers exotic setups; no inline-base64 entry. Instantiation
uses `WebAssembly.instantiateStreaming` when the host serves
`application/wasm`, falling back to `instantiate(await res.arrayBuffer())`,
because the package cannot control the host's `Content-Type` and getting it
wrong silently doubles cold start. The wasm is fetched **inside the worker**, so
instantiation never competes with first paint.

Wire cost = wasm + glue + `fzstd`, gzipped — the **one** denominator §3.9 gates.
**No size or performance number appears in the README before §3.9 and §4.6
measure one.**

Supply-chain posture: no install scripts; npm provenance publishing; a `files`
whitelist; the wasm module's sha256 printed in the README and asserted in CI;
`fzstd` the only runtime dependency, pinned exact.

### 4.2 One interface, two clients

> **§4.2 quoted, replaced in one place.** §4.2 declared
> `interface KeyRef { address; viewingKey: Uint8Array }` and
> `subscribe(k: KeyRef, cb)`. `getNotes(k)` with a `Uint8Array` is defensible —
> one call, one copy, zeroized on return. A subscription is not: it forces the
> integrator to hand a long-lived key to a long-lived object, which then holds
> it across an unbounded number of passes, across a locked wallet, across a
> backgrounded tab. For our verified customer — a wallet with a lock screen —
> that is the wrong shape, and it is the shape they will have to work around.

```ts
/** An owner the client can discover for. The client NEVER stores the key: it
 *  calls `viewingKey()` at the start of every pass and zeroizes the bytes it
 *  was given before the pass returns. A locked wallet rejects, and the client
 *  reports `{type:'status', state:'locked'}` rather than failing the session. */
export interface Account {
  readonly address: `0x${string}`;
  /** 32-byte big-endian viewing key. Return a FRESH array each call — the
   *  client zeroizes it. Reject to decline (locked, denied, revoked). */
  viewingKey(): Promise<Uint8Array>;
}

/** For a backend or CLI that legitimately holds the bytes for the process
 *  lifetime. Named so the shape is visible in the integrator's own review. */
export function staticAccount(address: `0x${string}`, key: Uint8Array): Account;
```

`Uint8Array` only, never a hex string: a string would create unzeroizable copies
and make the honest zeroization statement cover nothing. The address is bundled
because upstream discovery is (address, key)-parameterized and hiding that would
be a lie. `staticAccount` holds a JS buffer that cannot be reliably zeroized; the
README says so in the same paragraph that says the module never writes a key
anywhere — **the guarantee is non-transmission, not host memory hygiene.**

```ts
export type Strk20ErrorCode =                     // closed union, never `string`
  | 'FEED_HASH_MISMATCH' | 'FEED_CHAIN_BROKEN' | 'FEED_MALFORMED' | 'FEED_EPOCH_GAP'
  | 'FEED_ADVANCED_MIDSYNC' | 'DECOMPRESS_LIMIT' | 'DECOMPRESS_UNSTAGED'
  | 'SNAPSHOT_ROOT_MISMATCH' | 'SNAPSHOT_ANCHOR_MISSING' | 'SNAPSHOT_NOT_EMPTY'
  | 'SNAPSHOT_UNREACHABLE' | 'SNAPSHOT_UNAVAILABLE' | 'ANCHOR_UNBOUND'
  | 'BOUND_BELOW_SNAPSHOT' | 'CHAIN_MISMATCH'
  | 'STATE_CORRUPT' | 'STATE_VERSION' | 'STATE_FOREIGN' | 'SEALED_STATE_MISMATCH'
  | 'KEY_INVALID' | 'KEY_UNAVAILABLE' | 'ENTROPY_INVALID' | 'ENTROPY_REUSED'
  | 'DISCOVERY_INCOMPLETE' | 'HISTORY_UNAVAILABLE'
  | 'SYNC_PROTOCOL' | 'SYNC_IN_PROGRESS' | 'SCOPE_VIOLATION'
  | 'SESSION_INVALID' | 'SESSION_INCOMPLETE'
  | 'TRANSPORT' | 'CONFIG_INVALID' | 'ABORTED' | 'INTERNAL';

export interface Note {
  token: string; index: number; noteId: string; nullifier: string;
  amount: bigint; blockNumber: number; blockTimestamp: number;
  sender: string; spent: boolean;
}

export type Phase = 'idle' | 'open' | 'manifest' | 'snapshot' | 'epochs' | 'head'
                  | 'anchor' | 'persist' | 'discover';

export interface Progress {
  phase: Phase; done: number; total: number;
  bytes: number; requests: number; elapsedMs: number;
}

export interface SyncTiming {
  totalMs: number;
  phases: { open: number; manifest: number; fetch: number; decompress: number;
            apply: number; load: number; export: number; anchor: number;
            discover: number };
  cold: boolean;
  fromCache: 'folded' | 'raw' | 'none';
}

export interface RequestRecord {
  url: string;                       // absolute, exactly as issued, never truncated
  method: 'GET' | 'POST';
  purpose: 'feed' | 'live' | 'anchor-rpc';
  artifact: 'genesis' | 'manifest' | 'epoch' | 'epoch_anchor' | 'snapshot'
          | 'snapshot_anchor' | 'anchors' | 'head' | 'live' | 'rpc';
  status: number;
  bytes: number;                     // response body bytes
  transferBytes: number | null;      // PerformanceResourceTiming.transferSize, or null
  requestBodyBytes: number;          // 0 for every feed request, by construction
  source: 'network' | 'etag-304' | 'idb-cache';
  ms: number; at: number;            // performance.now()
}
export interface NetworkSummary {
  requests: number; bytes: number;
  byArtifact: Record<string, { requests: number; bytes: number }>;
  requestLogSha256: string;          // computed INSIDE the module (§3.3)
}

export interface FeedState {
  head: number; l1Accepted: number; lastEpoch: number; lastEpochTo: number;
  historyFrom: number; snapshotBasis: number | null; snapshotRejected: boolean;
  verified: 'anchored' | 'server-asserted' | 'replayed';
  staleness: 'ok' | 'behind' | 'diverged';
  changed: boolean; cold: boolean;
  timing: SyncTiming; network: NetworkSummary;
}

export interface NotesResult {
  notes: Note[]; balances: Map<string, bigint>;
  added: Note[]; spent: Note[];
  feed: FeedState;
  complete: boolean; historyFrom: number; cursorReset: boolean;
  stats: { slotsRead: number; eventsScanned: number; passesIn: number; passesOut: number };
  elapsedMs: number;                 // discovery only, excludes the feed pass
  raw: unknown;                      // the untouched SyncReport (oracle equality)
}

export type DiscoveryEvent =         // closed union: no member carries a key,
  | { type: 'progress'; progress: Progress }        // a cursor, or a free string
  | { type: 'feed';     feed: FeedState }
  | { type: 'notes';    added: Note[]; spent: Note[];
                        balances: Map<string, bigint>; head: number; elapsedMs: number }
  | { type: 'reorg';    rewoundTo: number }
  | { type: 'status';   state: 'live' | 'polling' | 'degraded' | 'locked' | 'idle' }
  | { type: 'request';  record: RequestRecord }
  | { type: 'error';    error: Strk20Error; recovering: boolean };

export interface Subscription { close(): void; readonly closed: boolean; }

export interface DiscoveryClient {
  /** Bring the local mirror to the feed's head. Takes NO key and emits no
   *  key-derived value: a wallet can keep the mirror warm while locked, and the
   *  expensive part of this system is demonstrably runnable with nothing about
   *  the user in the process. */
  sync(opts?: { signal?: AbortSignal; onProgress?: (p: Progress) => void })
      : Promise<FeedState>;

  getNotes(a: Account, opts?: {
    signal?: AbortSignal;
    onProgress?: (p: Progress) => void;
    refresh?: 'auto' | 'force' | 'none';   // 'none' = discover over the mirror as it is
  }): Promise<NotesResult>;

  watch(a: Account, cb: (ev: DiscoveryEvent) => void): Subscription;

  history(a: Account, opts?: { fromBlock?: number; limit?: number; signal?: AbortSignal })
    : Promise<{ transactions: HistoryTx[]; complete: boolean;
                completeFrom: number; registrationAvailable: boolean }>;

  /** The SDK socket — the primary integration for a wallet. */
  provider(a: Account): DiscoveryProvider;

  status(): ClientStatus;
  network(): { records: readonly RequestRecord[]; summary: NetworkSummary };
  resetCache(opts?: { identities?: boolean }): Promise<void>;
  close(): Promise<void>;            // terminates the worker; see §4.11
}

export interface ClientStatus {
  mode: 'keyless' | 'delegated';
  transport: 'sse' | 'polling';
  persistence: 'indexeddb' | 'memory';
  persisted: boolean;                // navigator.storage.persisted()
  persistMode: 'raw' | 'folded' | 'both';
  blocking: boolean;                 // true when worker:false — work runs on the caller's thread
  leader: boolean;                   // this tab owns the SSE connection (§4.11)
  engineBytes: number;               // wasm linear memory currently held
  head: number; l1Accepted: number; lastEpoch: number; historyFrom: number;
  verified: 'anchored' | 'server-asserted' | 'replayed';
  accounts: number;
  network: { requests: number; bytes: number };
}

export class KeylessClient implements DiscoveryClient {
  constructor(opts: {
    feedUrl: string;
    network?: 'mainnet' | 'sepolia' | ChainProfile;   // default 'mainnet' (§A6, C18)
    coldStart?: 'auto' | 'snapshot' | 'epochs';       // default 'auto' — ONE vocabulary
    persistence?: 'indexeddb' | 'memory' | StorageAdapter;   // default 'indexeddb'
    persist?: 'raw' | 'folded' | 'both';              // default 'both'; see §4.5
    live?: boolean;                                   // default true
    pollIntervalMs?: number;                          // default 30_000
    worker?: boolean;                                 // default true (C14)
    prefetchConcurrency?: number;                     // default 6; 1 = strict wire order
    stepBudgetMs?: number;                            // default 50 (worker) / 16 (main)
    maxArtifactBytes?: number;                        // default 64 * 2**20
    anchorRpcUrl?: string;                            // enables §1.5 ring 6
    anchorPolicy?: 'off' | 'best-effort' | 'require'; // default 'best-effort'
    requestPersistentStorage?: boolean;
    wasmUrl?: string | URL;
    fetch?: typeof fetch;
    onRequest?: (r: RequestRecord) => void;
  });
}
```

`stepBudgetMs` is a **TypeScript** budget: the wrapper calibrates ops per
millisecond across `discover_step` calls and passes `max_ops`. The module has no
clock (§3.9), and saying so in the option's docstring is cheaper than a support
thread later.

**Additions to §4.2, each with its reason:**

- **`sync()`, key-free.** Separates "keep the mirror current" from "tell me my
  notes". A wallet syncs on a schedule while locked; the demo times a cold load
  before a key exists; and the central claim becomes a program you can run with
  no key anywhere in it.
- **`signal`.** A wallet UI closes the account screen mid-cold-start. Without
  cancellation the integrator's only recourse is abandoning a running worker.
  Abort is checked between Steps and between discovery slices; partial
  application is retained (§3.3's resumability guarantee).
- **`onProgress` / `progress` events.** The cold path is seconds of work — 3–5 s
  desktop, 12–20 s on a mid-tier phone **[est]**. §4.2 gave no way to draw a
  progress bar, so every integrator would build a fake one or block their UI.
  Phases are the wrapper's own loop boundaries, so nothing is invented.
- **`refresh: 'none'`.** Without it, N accounts cost N feed passes over 16 MB.
- **`watch` replacing `subscribe`.** Returns a `Subscription` whose `closed` is
  inspectable, which is the shape an integrator stores on a component instance.
- **`Note.blockTimestamp`.** Already in `BlockLine` and in `MemStore`'s
  `BlockRec` (currently `#[allow(dead_code)]`). Free, and every UI needs it.
- **`anchorPolicy`.** §7.1's "configured means mandatory" is wrong in a browser
  given LIVE-6: publicnode implements no storage proofs at any height, so a
  user's own RPC that cannot answer would fail every sync. Three-valued: `off`
  never asks; `best-effort` asks, downgrades `verified` on `UNAVAILABLE`, fails
  on `MISMATCH`; `require` fails on anything but `MATCH`. **`MISMATCH` always
  fails** — that one is evidence about the data, and the shipped code already
  agrees (`sync.rs:341-382`).
- **`network()` / `onRequest`.** A shipped surface, not demo scaffolding: it is
  what makes the no-key claim checkable by the integrator rather than only by
  our suite, it is a real cost meter, and it powers the demo's panel.

**Multi-account, and where the lock lives.** Unstated in §A4 and the first thing
a wallet asks:

- **One client, one mirror, N accounts.** `KeylessClient` owns exactly one
  `Engine` and one IndexedDB database. The feed pass runs once per refresh and is
  shared; discovery is per account, and concurrent `getNotes` calls for different
  accounts coalesce onto one feed pass.
- All engine access is serialized inside the client (the wasm `Engine` is `&mut`
  for both sync and discovery; there is no concurrency to be had). Cross-tab,
  `navigator.locks.request('strk20:<db>')` as in §4.3, with §4.3's scope
  correction intact.
- **One sealed blob per (account, chain, pool)**, keyed by §4.4's `keyId`.
  Accounts never share a cursor.
- `watch()` over N accounts: one SSE subscription, one feed pass per poke, N
  discoveries, N `notes` events. A locked account's `viewingKey()` rejection
  emits `{type:'status', state:'locked'}` for that subscription and skips it; the
  others proceed.

**Must not ship**, recorded so it cannot drift back: any constructor that takes a
key; `getNotes(k)`/`subscribe(k, cb)` with a raw key as the only way to make
progress; `DelegatedClient` on the main entry (§4.8); any event payload or
`details` field typed as an open `string`/`unknown` that a logger or telemetry
pipe could receive; and any performance figure in the README before §3.9 and
§4.6 measure one.

Switching keyless ↔ delegated remains a constructor swap; leg **v** asserts
deep-equal results from both against the same fixture.

Worker (C14): on by default. The key crosses by **ArrayBuffer transfer**,
detaching the caller's buffer, so exactly one copy is in flight and the module
zeroizes it. `worker: false` runs on the main thread and sets `status().blocking
= true`; the README calls it a testing mode, not a deployment mode (§4.11).

### 4.3 Keyless data flow

```
open IDB (or memory fallback)                       → status().persistence
load profile → Engine.new(profile) or Engine.load_begin(profile) + frames
loop:  step = Engine.sync_begin(coldStart) | Engine.sync_supply(...)
         step.fetch → net.request(base + step.path)      # §4.10, one chokepoint
                      (+ prefetch hints, ≤ prefetchConcurrency in flight)
                      fzstd within step.decompress_cap
                      Engine.sync_supply(env, zbytes, payload)   # module hashes both
         step.rpc   → POST the user's own anchorRpcUrl (§1.5 ring 6)
                      Engine.sync_supply_rpc(env, result | unavailable)
         step.done  → FeedState { staleness, verified, changed, cold, timing, network }
if info().state_dirty and persist includes 'folded':
         Engine.export_begin/export_chunk → one IDB transaction, ≤4 MiB frames
per account: sealed = IDB.cursors[keyId]
         key = await account.viewingKey()             # fresh copy, per pass
         h = Engine.discover_begin(addr, key, sealed, entropy32)   # key zeroized here
         while (!done) Engine.discover_step(h, ops)   # ops calibrated from stepBudgetMs
         out = Engine.discover_finish(h)  → store sealed, emit added/spent
watch():  leader-elected EventSource /feed/live (§4.11); on poke, repeat the loop
          on error, poll fallback (§2.5)
```

`genesis.json` is fetched every session as the first Step and byte-compared
against the stored copy before any row lands (§4.4). Nothing about the fetch
plan is decided by TypeScript: the wrapper GETs the paths a key-blind module
named, inflates within the cap the module named, and hands both buffers back.

All sync passes run under `navigator.locks.request('strk20:<db>', …)` so tabs
serialize; without Web Locks, last-writer-wins is safe **for key-independent
state** because every persisted value is self-verifying (blobs carry a self-hash
and a stamp; epochs are re-hashed).

**Scope correction, unchanged and load-bearing.** That safety argument covers
`meta`, `artifacts` and `state`. It does **not** extend to `cursors`: the sealed
blob is an AEAD ciphertext, and two tabs forking from the same prior blob are
exactly the nonce-collision case §3.6 addresses. Forking there is safe only
because every discovery session supplies fresh `crypto.getRandomValues` entropy,
with `ENTROPY_REUSED` as the backstop. Web Locks reduce the frequency of the
fork; they are not what makes it safe, and no implementation may treat them as
the mitigation.

### 4.4 IndexedDB layout

Database name `strk20-discovery:<chain_id>:<pool>` — per-chain-and-pool, so
cross-network confusion is impossible rather than detected and a schema
migration never touches two chains at once. Version 1:

| store | key | value |
|---|---|---|
| `meta` | string | `format_v`, `last_epoch`, `last_epoch_hash`, `snapshot_e`, persist mode, **`genesis` (the raw `/feed/genesis.json` bytes)** |
| `artifacts` | `"snapshot"` \| `"anchor"` \| epoch idx (number) | `{hash: string, zbytes: ArrayBuffer}` — compressed **exactly as served** |
| `state` | `"folded/meta"` \| `"folded/<i>"` | `{frames, len, sha256, stamp, engine_version, profile_hash, written_at, source_manifest_hash}` / `ArrayBuffer` ≤ 4 MiB |
| `cursors` | `keyId` (hex string) | `{sealed: ArrayBuffer, updatedAt: number}` |

`keyId = hex(HKDF-SHA256(ikm = viewingKey, salt = "strk20-idb-keyid-v1", info =
chain_id ‖ pool ‖ owner))` — the **full 32-byte HKDF output rendered as 64
lowercase hex characters, no slice**. Unguessable without the key (R-K).

`artifacts` values stay compressed exactly as served, because Design R's whole
point is that a reload re-runs the same verification ladder over the same bytes
the network would have delivered; storing inflated payloads would put a
TypeScript decompressor between the network and the hash the module checks.

**`genesis` is persisted AND re-fetched.** The stored bytes give `load` a stamp
source that does not depend on the network; the re-fetch and byte-compare each
session is what catches **a feed that changes its own genesis**, which a
stored-only copy would never see. Mismatch ⇒ `CHAIN_MISMATCH` before any row
lands. Leg **u**'s reload delta is `{genesis, manifest, head}` + SSE.

Never stored: `head.ndjson` bytes, the head ETag, anything tail-derived — the
no-persisted-reorg-logic property is enforced by the schema having nowhere to put
a tail. Documented residual metadata: row existence, sizes, timestamps.

Quirks engineering, each with a test:

1. IndexedDB transactions auto-commit at microtask end — never `await fetch`
   inside a transaction; stage bytes first, write in one transaction.
2. `open` can throw synchronously or fire `onblocked` — every path falls back to
   `persistence: 'memory'` and reports it through `status()`.
3. Eviction is normal: an empty store is a cold start, never corruption.
4. Multi-tab: Web Locks when present, safe without it (§4.3).
5. Safari first-write latency: the initial persist happens after `getNotes`
   resolves, never on the critical path.
6. **Safari evicts after 7 days without interaction (ITP)** unless
   `navigator.storage.persist()` was granted. For a wallet this is the difference
   between a 0.03 s-class warm start and a full cold fold on the user's second
   visit a week later. `requestPersistentStorage: true` is the recommended wallet
   setting, `status().persisted` reports the actual grant, and the README states
   that a denied grant on Safari means periodic cold starts. The eviction is
   **undetectable after the fact** — the flag that would record it is evicted too
   — so quirk 3 governs: an empty store is a cold start, never an alarm.
7. **Firefox private browsing gives an in-memory IndexedDB**: it opens
   successfully and loses everything on close. Indistinguishable from eviction
   and handled identically.
8. **Structured clone of a large `ArrayBuffer` is a main-thread copy.** ≤4 MiB
   frames (§3.5), written in one transaction; a partial write is detectable as a
   frame-count mismatch and is treated as a cache miss, never a corruption.
9. **`onblocked` during an upgrade with other tabs open**: the package does not
   force-close other tabs. It falls back to `persistence: 'memory'` for the
   session, reports it, and sends an advisory `BroadcastChannel` release request.

### 4.5 Persistence: both designs, one gate

**Design R — raw artifacts are the persisted truth.** Persist `artifacts` +
`cursors` + `meta`. Every load re-runs the full verification ladder over stored
bytes and refolds. A tampered row fails its hash and is refetched: local storage
is never trusted, only network-equivalent bytes re-verified per load.

**Design M — folded-mirror cache over R.** Additionally persist the §3.5 blob
into `state` after a sync reports `state_dirty` (epoch cadence — never per head
poke; the discussion-§7 hazard). Load: frames → `Engine.load_begin` → sync;
`"ok"`/`"behind"` skips all folding; `"diverged"` or any `STATE_*` deletes the
record and falls through to R, then to the network. Strictly a cache: deleting it
is always correct.

Honest trust statement, printed in the README where M ships: M trusts IndexedDB
integrity between loads. No secret exists to MAC a key-independent blob, so a
same-origin attacker can alter folded values undetected until the next full
refold. The marginal risk over R is precisely *persistence of tampering beyond
the tampering code's presence*. Mitigation is architectural: an opportunistic
`requestIdleCallback` full refold + byte-compare every N loads (default 20),
flagging divergence as `{type:'error', error: STATE_CORRUPT, recovering:true}`.

> **§4.5 amended.** §4.5 said *"if the gate selects R, `export`/`load` stay in
> the ABI, dormant"*. **Design M is built**, and `persist` becomes
> `'raw' | 'folded' | 'both'` with default `'both'`. The reason is measurement,
> not argument: the L2 arm of §4.6's decision rule (`t_cold(L2) > 2000 ms` ⇒ M
> for `coldStart:'epochs'` sessions) is already answered by the native
> **5.97 s** cold fold of full mainnet history, and the browser is slower on
> every term (§3.2 **[est]** 3–5 s desktop, 12–20 s throttled). The measured
> **0.03 s** warm resync is itself a Design-M number — a persisted folded mirror
> — so presenting M as an optimisation over R on the epochs lane is backwards.
> `'raw'` stays available for a caller who wants no folded blob on disk at all;
> `'folded'` for a caller who wants minimum stored bytes.

**What is still open is L1, the snapshot lane, and its default is the bench's to
set, not this document's.** Snapshots do not exist yet (roadmap item 1), so
nothing has measured the snapshot-lane cold path. Until §4.6's L1 arm runs, a
snapshot-lane session inherits the default `'both'`; when it runs and p95
`t_cold(L1)` ≤ 500 ms, the snapshot lane's default becomes `'raw'`, which is the
better trust posture. That flip is a measurement, and no argument in this
document may pre-empt it.

`CONFIG_INVALID` still applies to a mode the shipped build does not implement,
and the published union is still narrowed at publish time: an unimplemented mode
arriving from untyped JavaScript is rejected in the constructor with
`CONFIG_INVALID {option:'persist', got, built}` — never silently downgraded,
because a caller that asked for a cache and got none should learn it at
construction and not from a latency graph. There is no `'auto'` mode (C15).

**Cache invalidation — normative, complete.** §A4 had no invalidation table, and
a cache with no written invalidation rule is how the folded-mirror design earns
the bad reputation §4.5 warns about.

| trigger | `meta` | `artifacts` | `state` | `cursors` |
|---|---|---|---|---|
| `meta.format_v` ≠ ours | delete DB entirely | — | — | — |
| stored `genesis` ≠ fetched genesis | **no writes at all**; throw `CHAIN_MISMATCH` | keep | keep | keep |
| `staleness == "diverged"` | keep identity rows | delete all | delete | keep (each seal is invalidated on open by `ckpt_epoch_hash`, §3.6) |
| `load` → `STATE_CORRUPT`/`STATE_VERSION`/`STATE_FOREIGN`, or `engine_version`/`profile_hash` differ | keep | keep | delete | keep |
| engine major bump | keep | keep | delete | delete iff `seal_v` changed |
| snapshot rejected (`auto` fell back) | keep | delete the snapshot **and** anchor rows together | delete | keep |
| `FEED_HASH_MISMATCH` on a stored artifact | keep | delete that one row, refetch once; a second failure is a hard error naming both hashes | keep | keep |
| **ring 6 `MISMATCH`** (the module called `reset_mirror`) | keep identity rows | **delete all** | **delete** | keep | 
| sealed blob fails AEAD open, or `ckpt_at > last_epoch_to` | — | — | — | treated as **no cursor**: fresh discovery, `cursorReset: true` |
| IDB eviction / empty store / private window | cold start; **never** an error (quirks 3, 6, 7) | | | |
| `artifacts` over `maxArtifactBytes` | prune oldest epochs at or below the folded blob's floor; never the snapshot, never an epoch above it | | | |
| `resetCache()` | keep `meta.genesis` | delete | delete | kept unless `{identities:true}` |
| a sync reports `state_dirty == false` | **no write** — the epoch cadence is ~4.7 h and a head poke must never rewrite the blob | | | |

The ring-6 `MISMATCH` row is the one nobody had, and it is the browser form of a
bug the Rust already guards (`sync.rs:355-358`): the user's own RPC has proven
this mirror is not the chain's, so `reset_mirror()` runs in wasm — and if the
IDB `state` and `artifacts` rows survive it, the next load restores a refuted
mirror from cache, sees a non-empty store, never re-enters the snapshot branch
and never re-grounds. Both rows are dropped in the same transaction.

**A non-obvious rule, stated because getting it wrong costs a full
rediscovery:** re-cold-starting from a *newer* snapshot raises `history_floor`
but does **not** invalidate sealed cursors or the note registry. Pool slots are
write-once (134,879 distinct slots across 139,131 writes, 96.9 % first writes),
so slot state below the new floor is complete and only *events* are missing.
Discovery and spent-state read slots and nullifiers, so they are unaffected;
only `history()` is, and that is exactly what `historyFrom` / `complete` report.

### 4.6 The fold-time measurement gate (pre-registered)

Runs, and its results are published, **before any persistence default is set for
a lane it has measured** — the discussion-§7 mandate, made binding and now
scoped by what is already known.

- Harness `ts/strk20-discovery/bench/fold.bench.ts`, driven by Playwright.
- Inputs, checked in under `bench/fixtures/`: **L1** = the snapshot lane
  (snapshot + epochs-after + head, recorded at a pinned manifest hash); **L2** =
  the full-history lane (all epochs + head); **L3** = a synthetic 10× history
  from `strk20 bench synth-feed --scale 10` (headroom only, never a shipping
  trigger).
- Measurement profile (C17): **headless Chromium at 4× CPU throttle**, ≥5 runs,
  **p95** of `t_cold = decompress + verify + fold + root-verify` (network
  excluded; `t_zstd` recorded separately). One named physical mid-tier device is
  measured alongside and recorded, when available. CI runs the same bench as a
  **trend line only**, failing the build if the L1 median regresses 3× against
  the recorded baseline.
- **L2's arm is already answered** by the native 5.97 s measurement (live-run
  §3) and is recorded as answered rather than re-run for show: M is built and is
  the epochs-lane default. L2 and L3 continue as trend lines.
- **L1 remains the open question**, and it is the one that decides a default:

> **FILL-IN (fold gate L1, pending snapshots + step 5):** `t_zstd` L1 = ___ ms;
> p95 `t_cold` L1 = ___ ms; throttled profile = ___; reference device = ___.
> **Decision for the snapshot lane: `persist` default `raw` / `both`.**
> Date: ___.

Full numbers and device profiles go to `docs/research/fold-gate-results.md`.

### 4.7 zstd in TypeScript

**`fzstd`** — pure-JS, decompress-only, ~8 KB gzip, MIT, no native or wasm
dependency. We never compress client-side, so decompress-only is the whole
requirement. Exact-version pinned; one path only (C16);
`DecompressionStream('zstd')` promotion is a recorded roadmap item.

**What TypeScript owes here is now exactly one thing: the cap.** Per §3.4 the
module hashes both the compressed and the inflated buffer, so a decoder bug or a
substituted payload becomes a loud hash mismatch inside Rust. The wrapper's sole
obligation is to inflate no further than `step.decompress_cap` (64 MiB for
epochs, 256 MiB for snapshots) and to raise `DECOMPRESS_LIMIT {artifact, cap}`
when it would — a resource bound, not a verification. The previous formulation
("verify the `.zst` hash first, then inflate") is withdrawn: it put an
accept/reject verdict in the least testable layer, and R-I is better served by
the module checking both hashes in the order `apply.rs` already checks them.

### 4.8 DelegatedClient

Exported from **`strk20-discovery/delegated`**, not from the package root. In
delegated mode the viewing key leaves the browser; that is a legitimate
self-host posture and a materially different trust boundary, and it should not be
one autocomplete away from `KeylessClient`.

It speaks the **reference compat wire** (`POST /v1/sync/incoming_state`,
`/v1/sync/outgoing_state`, `/v1/sync/preflight_check`, `POST /v1/history`; types
from `crates/wire`) to either `strk20-sync serve` (§A5) or `strk20
--enable-compat`, and by construction to any stock reference deployment. Cursors
round-trip in the reference schema, exercising base §7.4 interop from
TypeScript. It adopts §4.2's `Account` and event shapes, so the constructor swap
in leg **v** still works. The README states the trust boundary in base §9's
words: **the viewing key travels to a server you run.**

`subscribe()` uses **fetch-based SSE with an `Authorization: Bearer` header**,
not `EventSource` (review finding 8): `/feed/live` on `serve` is inside the auth
perimeter (§5.5) and native `EventSource` cannot send headers. No capability
token, no `POST /v1/watch`, no registry, nothing key-derived on the wire; R-E is
honoured since the token rides a header and never a URL. Native `EventSource`
remains the transport for the **keyless** `/feed/live` on the indexer, which
takes no auth and no parameters.

**Chain identity on construction.** The client reads `/health` and verifies
chain identity **before sending any key**. Base §6.2's ops `/health` body gains
`chain_id` and `pool` (additive). When the fields are **absent**,
`DelegatedClient` **refuses to construct**, throwing `CHAIN_MISMATCH
{field:'chain_id', expected:<profile>, got:null}` unless the caller passes
`assertUncheckedNetwork`. There is no "verify if present" mode.

**Insecure transport is refused.** A `serverUrl` that is neither loopback nor
`https:` throws `CONFIG_INVALID {option:'serverUrl', reason:'plaintext
non-loopback'}` unless `allowInsecureServer` is set. A viewing key travelling in
clear over a LAN is not a trade-off anyone makes deliberately.

```ts
export class DelegatedClient implements DiscoveryClient {
  constructor(opts: { serverUrl: string; authToken?: string;
                      network?: 'mainnet' | 'sepolia' | ChainProfile;
                      assertUncheckedNetwork?: boolean;
                      allowInsecureServer?: boolean;
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
  Chromium smoke exercises real IndexedDB / EventSource / Worker against the same
  stack.
- **The scanner is not reimplemented in TypeScript.** `capture-scan` is the
  leg-d Rust scanner promoted to a bin and reused verbatim over the TS proxy
  capture, an IndexedDB dump, the emitted `RequestRecord` stream and the demo's
  exported run log. One scanner implementation for every capture surface. Its
  self-test leg is retained: the scanner MUST find the key in a delegated
  capture, and the 13-encoding list lives in **one shared fixture** consumed by
  both the Rust scanner and the in-page scanner of demo-app.md §6, so the two
  cannot drift.
- **The chokepoint scan** of §4.10 runs beside it.
- **Resumability leg (γ′):** kill the transport at epoch 200 of the L2 fixture,
  re-open, resume, and assert a byte-identical final export blob and identical
  `SyncReport`. Without it every flaky mobile network costs a full refetch and
  nothing would catch a regression.
- Golden truth: the TS suite reads the **same** checked-in O2 golden JSON the
  Rust acceptance test pins — one file, byte-one, never duplicated.

CI order: `cargo build` → `cargo test -p e2e-tests` → `pnpm e2e`. No network.

### 4.10 The fetch chokepoint (new, and load-bearing)

Every byte this package fetches goes through one module, `src/net.ts`, exporting
one function.

```ts
// src/net.ts — the ONLY place this package touches the network.
export async function request(spec: FetchSpec): Promise<FetchOutcome>;
```

Obligations:

1. Emit a `RequestRecord` for every call, before and after, onto the event bus —
   which is what `network()` accumulates and `onRequest` forwards. **Requests
   issued inside the worker are forwarded to the hook over `postMessage`**;
   otherwise the hook lies by omission exactly where the audit matters.
2. Build the URL as `base + step.path` with **no interpolation of any
   caller-supplied string** beyond the base. `step.path` comes from the module's
   closed artifact enum, so a query string is unrepresentable.
3. Set no request header beyond `Accept`, `If-None-Match` (head only) and, in
   delegated mode, `Authorization`. `credentials: 'omit'`, no cookies, no custom
   UA.
4. Reject at runtime any URL not matching §2.8.1's eight whole-path patterns,
   plus `/feed/live`, plus (when configured) the anchor-RPC origin. Whole-path
   match, never a prefix, never `startsWith('/feed/')`.

**Mechanical enforcement:** a build-time scan asserting that no file under
`src/` other than `net.ts` contains the identifiers `fetch`,
`XMLHttpRequest`, `EventSource`, `sendBeacon`, or a dynamic `import()` of a URL,
run in CI beside leg **u**. TypeScript has no type-system move that expresses
"this module does no IO"; a scan over one filename is the checkable substitute,
and it is what makes `onRequest` honest rather than best-effort.

The anchor RPC is the one `POST` and the one non-feed origin. Its body carries a
public pool address and a public block number and is identical for every user
(§3.3.1); it is recorded with `purpose: 'anchor-rpc'` so it is visually separable
everywhere it is displayed.

### 4.11 Worker, SSE leadership, and memory

- **Everything expensive runs in the worker**: wasm instantiation, `fzstd`,
  `sync_supply`, every `discover_step`, every `export_chunk`. The main thread
  does `fetch`, IndexedDB and rendering.
- **`close()` terminates the worker.** That is the only way to return the
  ~70–85 MB **[est]** of wasm linear memory the epochs lane holds: linear memory
  never shrinks, and dropping an instance does not return it to the OS. A
  main-thread engine holds it for the life of the page, which is a killed tab on
  mobile. `status().engineBytes` reports it so an integrator can see the cost,
  and `worker: false` is documented as a testing mode with `status().blocking =
  true`.
- **SSE is leader-elected over Web Locks.** HTTP/1.1 caps concurrent connections
  at 6 per origin and an `EventSource` holds one for its lifetime: four tabs on
  the same feed origin leave two connections for every epoch fetch, and six
  deadlock. One tab takes
  `navigator.locks.request('strk20:sse:<db>', {mode:'exclusive'})`, opens the
  stream, and fans pokes out over a `BroadcastChannel`; followers run their own
  verified fetch on a poke — identical semantics, one connection.
  `status().leader` reports which tab holds it. Where Web Locks are unavailable,
  every tab opens its own stream and the package logs one warning; the poll
  cadence bounds the damage. **Nothing about blindness changes**: the leader's
  request is byte-identical to any other client's, and `/feed/live` is
  parameterless.
- **Operators should serve the feed over HTTP/2**, where the cap is per-stream
  and the issue evaporates. This belongs in the ops docs beside §2.7, not in
  client code.

### 4.12 Changelog — second pass (2026-08-31)

| § | first pass | now | why |
|---|---|---|---|
| 4.1 | `/sdk` a subpath afterthought; `KeylessClient` the headline | `LocalDiscoveryProvider` exported from the root and named first; an "Is this for you?" table opens the README | verified from the Wallet API docs: a Wallet-API dapp never receives a viewing key, so it can never call us. Our customer is a wallet or a key-holding app, and its integration is one field in `createPrivateTransfers` |
| 4.2 | `KeyRef { viewingKey: Uint8Array }`, `subscribe(k, cb)` | `Account { address, viewingKey() }`, `watch(a, cb)`, `staticAccount` as the named escape hatch | a real keystore authorizes a *use*; it does not hand out bytes for an object's lifetime. This also makes the locked wallet a first-class status instead of an integrator workaround |
| 4.2 | no key-free phase | `sync()` takes no key | a wallet keeps its mirror warm while locked, and the central claim becomes a runnable program |
| 4.2 | no cancel, no progress, no multi-account, `anchorRpcUrl` mandatory | `signal`, `onProgress`, one client / one mirror / N accounts with coalesced passes, `anchorPolicy` three-valued | a 3–20 s cold path with no cancel and no progress is unshippable; N accounts × N feed passes over 16 MB is the first thing a wallet notices; LIVE-6 says a capability gap must never fail a sync |
| 4.2 | — | `network()` / `onRequest` (worker-forwarded), closed `DiscoveryEvent` and `Strk20ErrorCode` unions | the no-key claim must be checkable by the integrator; a closed union means a logger attached to our bus *cannot* receive key material |
| 4.3 | TypeScript decides what to fetch | TypeScript fetches what a key-blind module names | the URL author has no key on that code path |
| 4.4 | five quirks | nine: + Safari ITP eviction, `persisted()` surfaced, Firefox private-mode IDB, `onblocked` without force-closing tabs; `state` re-shaped into ≤4 MiB frames | ITP turns a returning wallet user's 0.03 s warm start into a full cold fold and is undetectable after the fact |
| 4.5 | R is the lane, M is dormant if the gate says R | **M is built**; `persist: 'raw'\|'folded'\|'both'` default `'both'`; full invalidation table incl. the ring-6 `MISMATCH` row | the L2 arm is answered by the 5.97 s native measurement; the 0.03 s warm figure is itself a Design-M number. A cache with no written invalidation rule is worse than no cache |
| 4.6 | gate open on both lanes | L2 recorded as answered; the FILL-IN scoped to L1, which alone decides a default | measured, not argued — and the snapshot lane genuinely has not been measured |
| 4.7 | TypeScript verifies the `.zst` hash | TypeScript owns only the output cap | §3.4 |
| 4.8 | on the main entry | own subpath + insecure-transport gate | the key leaves the browser there |
| — | no equivalent | §4.10 chokepoint + CI identifier scan; §4.11 worker/leader/memory | TypeScript cannot express "does no IO"; and six-connection starvation, ITP and 80 MB of linear memory are what a browser actually does to you |

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

**0a has largely landed, and what remains of it is §3.10.** `crates/consumer`
now holds `ConsumerStore`, `MemStore`, `apply_feed`, `sync_once` and the pass
loops as generics; `crates/client/src/sync.rs` is a re-export shim. §0.4.1's
`NoteSet` value type was not built and is not being built (§3.10). The seven
remaining deltas — including the two blocking corrections, the missing
`entry.zst` check and `apply_feed`'s trust-on-first-use genesis adoption — land
in step 0a, before the wasm crate exists. Step 3 additionally owns the two
Pedersen gates of §3.9 (bundle delta and MPT runtime), because a bad answer
there is ABI-shaped and must be known before step 4, not during it.

Edges in words. The extraction (0a) is first because every other area sits on
it, it is the only step whose test already exists, and doing it now means
snapshot cold start and the watch logic are written **once** in the extracted
crate instead of written in `crates/client` and moved later. Profiles (1) come
next because their stamps flow through every format defined afterwards.
Snapshots (2) precede wasm (4) because the snapshot branch of `apply_feed`
consumes the format.
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
`cursor_reset: true`, fresh discovery, same final notes. The terminal Step's
`staleness` field **returns** `ok`/`behind`/`diverged` on the three constructed
manifests — the return-value form is the asserted one and no staleness throw
exists to provoke (review finding 13; the standalone `check_manifest` it used to
be asserted on is deleted by §0.5 S2, and the assertion moves rather than
lapsing). `export_reference_cursor` output round-trips into the
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
the terminal Step's outcome reports `tail_rewound`; the next discovery session
equals the post-fork O1; the sealed blob carries no note above `ckpt_at`
(§3.6); and the exported state blob is **byte-identical before and after the
fork**
— the mechanical proof that the tail is never exported and that browser
persistence needs no reorg logic.

**s. WASM purity + size (A3.9, CI gates run with the suite).**
**Feature-resolved** dependency walk (`cargo tree -e features`, not a crate-name
walk): `consumer` and `wasm` reach no
`tokio`/`reqwest`/`rusqlite`/`getrandom` — asserted with a **red-first negative**
that removes `default-features = false` from `chacha20poly1305` and confirms the
gate fires, since a name-only walk cannot see the default-feature path through
`aead` that would otherwise have shipped `getrandom` (review finding 11a).
`wasm-objdump` import section matches
`crates/wasm/import-allowlist.txt` exactly, diffed as a file rather than
matched as a name pattern (review finding 12). `crates/consumer` compiles under
`#![forbid(unsafe_code)]` and `crates/wasm` under
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
`SecretFelt: !Serialize` unchanged; the wasm crate exposes **exactly the
key-accepting entries named in `crates/wasm/key-entries.txt`** — a diffed
file rather than a count in prose (§3.9) — and no transport type.

### 8.1 Legs added by the second pass (§0.5)

Greek letters, so the a–z sequence stays stable. All runnable, all red first.

**α. Store conformance over the new trait surface.** `MemStore` and `FeedStore`
give identical `refresh_spent` / `prune_missing_notes` results over the same
fixture, including the live-observed case *a spent note's slot is not cleared*
(live-run §7), and identical `cursor_get`/`cursor_put`/`owner_generation`
round-trips (§3.2b).

**β. Request-emitter purity (proptest, native).** For any two distinct
(address, key) pairs and any feed fixture, `request_log()` after driving a sync
to completion is **byte-identical**, and remains so when discovery sessions are
interleaved between syncs (§3.9). P-blind as a theorem; leg **u**'s wire capture
becomes its independent empirical check.

**γ. Prefetch equivalence.** `prefetchConcurrency` 1, 6 and 64 yield identical
`request_log()`, byte-identical export blobs and identical `SyncReport`s
(§3.3.1).

**γ′. Resumability.** Kill the transport at epoch 200 of the L2 fixture,
re-open, resume; the final export blob is byte-identical to an uninterrupted
run's and the report matches (§3.3, §4.9).

**δ. Discovery slicing equivalence.** A session stepped at small `max_ops`
produces the same `DiscoverOut` as one stepped with an unbounded budget, and no
step overruns its budget by more than one pass (§3.3).

**ε. State blob v2.** Round-trip; the §3.5 bounds asserted as array checks; the
header's `verified` never upgraded on load; a blob carrying
`snapshot_pending_grounding` causes the next sync to discard the mirror. Leg
**r** is unchanged and still asserts byte-identity across a tail fork.

**ζ. Memory budget.** Peak wasm linear memory on the L2 fixture stays under a
recorded budget; the §3.2 arena layout is asserted by that budget rather than by
inspection.

**η. Leader election.** N simulated tabs open exactly one `EventSource`; killing
the leader promotes another within one lock timeout (§4.11).

**θ. Chokepoint scan.** No file under `ts/strk20-discovery/src/` other than
`net.ts` names `fetch`, `XMLHttpRequest`, `EventSource`, `sendBeacon`, or a
dynamic `import()` of a URL; and a URL outside §2.8.1's whole-path allowlist is
rejected at runtime (§4.10).

The demo's own legs (d1–d7, and the clock leg that asserts the demo measures
what it claims) live in [demo-app.md](demo-app.md) §10 and run in CI against its
REPLAY lane.

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
| 11 | MAJOR — the wasm crate fails two of its own §3.9 gates on day one | **FIXED, both halves.** (a) Every RustCrypto dependency is pinned `default-features = false` with an explicit feature list, and the gate becomes a **feature-resolved** `cargo tree -e features` walk with a checked-in diffed tree — a crate-name walk cannot see the default-feature path that would have shipped `getrandom` and quietly voided C2. Leg **s** gets a red-first negative that removes the pin and confirms the gate fires. (b) the wasm crate moves to `#![deny(unsafe_code)]` with one documented `#[allow]` scope, since `forbid` cannot be lifted inside `#[wasm_bindgen]`-generated code and the cdylib would not have compiled; `#![forbid]` stays on the pure-Rust `crates/consumer`. Leg **s** builds the cdylib. | §3.1, §3.9, leg **s** |
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
