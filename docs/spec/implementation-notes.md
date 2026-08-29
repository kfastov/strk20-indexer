# Implementation notes — deltas from the spec

Honest record of where the built system deviates from
[architecture.md](architecture.md), and why. Everything not listed here is
implemented as specified.

## Discoveries made during implementation

1. **Upstream's cursor is a pagination cursor, not a resume cursor.** A
   `DiscoveryCursor` whose completion flags are set short-circuits the engine
   entirely — it will never look for notes created after the block it was
   computed at. The client therefore *re-opens* a persisted cursor before
   every pass: completion flags cleared, cached totals (`total_n_channels`,
   `total_n_notes`) dropped, every progress position kept. The engine then
   re-probes only boundary slots. This is the concrete form of the
   "watch-set grows" caveat from the research
   (docs/research/verify-discovery-trace.md §3), and it is covered by
   acceptance legs f/g/k. Cursors still round-trip byte-compatibly with the
   reference schema (conformance test).

2. **The upstream devnet fixture's only bob note is spent** — upstream's own
   step-test asserts the engine filters it. The acceptance chain therefore
   mints two fresh unspent notes for bob at block 31 (valid ciphertexts built
   with the engine's own crypto), refining resolution R6's partition to
   {10, 20, 30} + 31.

## Deliberate deviations

3. **Compat `/health` is served by the ops route** (spec §6.4 listed a
   reference-shaped compat health). Axum forbids two handlers on one path;
   the SDK's `isHealthy()` reads only `body.status`, which the ops shape
   provides. The reference `HealthResponse` type is still vendored in
   `compat/wire.rs`.

4. **R2's compat proof is not (yet) a byte-replay of upstream's 11 HTTP
   tests.** What stands instead: verbatim-vendored wire types with provenance,
   the unmodified engine behind them (trait-bridge equivalence proven in
   conformance), acceptance leg h (compat notes == oracle, 409 semantics,
   cursor interop), and the reference serde schema round-trip. Porting the
   upstream `devnet-dump.json.gz` harness is the top testing roadmap item.

5. **verify-root runs once per cut batch** at `min(l1_accepted, frontier)`,
   not per historical epoch: `starknet_getStorageProof` covers only a recent
   window (~25–55k blocks on lava), so deep-backfill epochs cannot be
   root-checked individually — but the cumulative mirror check at the batch
   head subsumes them (a missing historical write corrupts the current root
   too, because pool slots are write-once). Per-epoch anchors remain
   best-effort sidecars, absent outside the proof window, as R7 specifies.

6. **trybuild → `compile_fail` doctests** for the two privacy locks
   (`SecretFelt: !Serialize`, `FeedTransport` signature). Same guarantee,
   no extra dev-dependency.

## Not in this branch (spec'd as roadmap, restated here)

- `strk20 snapshot create|import` and the manifest `snapshot` field (dir and
  schema exist; `mirror-pull` covers the bootstrap use case today).
- `strk20 bench` harness (§10.5) and the nightly live smoke (§10.4).
- Prefix-bucket endpoint (wire frozen in §6.3; ~50 lines when wanted).
- wasm client package, SSE tail, Postgres backend — §12 roadmap unchanged.

## Post-implementation adversarial review (2026-08-30)

A three-lens adversarial review (correctness / privacy / client-semantics)
with per-finding verification confirmed 22 defects the green test suite did
not catch — full record in
[../research/review/adversarial-review.md](../research/review/adversarial-review.md).
All 22 are fixed; highlights that changed observable behavior:

- per-block event ingestion is paginated (a block's events can span getEvents
  pages) and guarded by fork-consistency checks on every fetched artifact;
- reorg rollbacks tombstone forgotten hashes instead of deleting them, so
  compat's 409 gate distinguishes known-reorged from merely-unknown hashes
  (a rebuilt indexer no longer 409s every existing client);
- a verify-root mismatch now triggers the spec §5.6 per-block rescan and
  surfaces in /health as DEGRADED instead of silently halting the feed;
- the client supersedes epoch ranges on apply (masked-reorg poison removed)
  and rewinds cursors per-owner via a crash-safe persisted tail generation;
- sync.db is chmod 0600 BEFORE SQLite creates -wal/-shm; compat/raw reject
  malformed bodies without echoing them and label every response;
- `strk20 mirror-pull` ingests verified epochs into the DB (real bootstrap);
  `strk20-sync --full-resync` exists; `--watch` emits each note once and
  survives transient transport errors; `verify` fails on unprovable slots.
