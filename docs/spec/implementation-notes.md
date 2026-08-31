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

5. **verify-root runs once per cut batch** at `min(frontier, rpc_head)`, not
   per historical epoch: `starknet_getStorageProof` covers only a recent
   window, so deep-backfill epochs cannot be root-checked individually — but
   the cumulative mirror check at the batch head subsumes them (a missing
   historical write corrupts the current root too, because pool slots are
   write-once). **Corrected 2026-08-31:** the window was measured at ~1024
   blocks, not the ~25–55k recorded here, while `l1_accepted` lags head by
   ~5000 — so the original `min(l1_accepted, frontier)` target was outside
   the window by construction and the check had never once run against
   mainnet. See §"Live-run fixes" below.

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

## Live-run fixes (2026-08-31)

Six defects measured against real networks
([../research/live/live-run-findings.md](../research/live/live-run-findings.md))
plus the Sepolia chain profile. Behaviour that changed:

- **LIVE-1** `RpcClient::call` classifies JSON-RPC errors instead of treating
  them all as fatal. A pruned-history answer (`-32603 … has been pruned`) is a
  *provider capability* answer — lava routes the same request to an archive or
  a pruned backend nondeterministically — and is retried in place with backoff,
  bounded by `CAPABILITY_RETRIES`, with the provider's own message preserved
  when the bound is hit. Semantic errors (block not found, invalid params) stay
  fatal on the first answer.
- **LIVE-2** the scan phase emits a periodic `scan progress` INFO line
  (`cursor`, `scan_to`, `blocks_ingested`, `events`, `endpoint`), cadence
  `--progress-secs` (default 15, `0` = every page). Tracing now writes to
  stderr with ANSI only on a terminal, so results on stdout stay parseable.
- **LIVE-3** HTTP 429 backs off in place on its own budget and never increments
  the consecutive-failure counter, so throttling can no longer flip a deep
  backfill onto an endpoint that cannot serve the range.
- **LIVE-4** verify-root probes `min(frontier, rpc_head)` — inside the live
  proof window — and distinguishes three outcomes: `Verified`, `Unavailable`
  (a provider gap; never latches `verify_root_failed`, never DEGRADED, exit 0),
  and a `VERIFY-ROOT MISMATCH` error. Going above the frontier would be unsound:
  the chain root there covers writes we have not ingested.
- **LIVE-5** the feed publishes `feed/anchors.ndjson`, an append-only,
  canonically encoded log of `(block, block_hash, storage_root, class)` captured
  whenever a block was provable. `strk20-sync verify-anchors` folds the local
  mirror to each anchor block and recomputes the root. The per-epoch anchor
  sidecar is kept and still written when capturable. Trust meaning is documented
  in `crates/client/src/anchors.rs`: the log is not content-addressed, so what
  the check establishes is agreement between the folded mirror and the root the
  publisher read from a chain proof.
- **LIVE-6** storage proofs are treated as a per-endpoint capability learned at
  runtime: `get_storage_proof` asks endpoints in capability order and never
  moves the active endpoint on a proof refusal, so a failover to a
  proofs-less provider cannot turn every root check into a failure.
- **Fix pack B** `--network mainnet|sepolia` selects a whole verified profile
  (pool, genesis block, chain id, decoder map, default RPC endpoints); every
  explicit flag still overrides it field by field. `strk20-sync sync --network`
  refuses a feed whose stamped chain id is not the expected one, before a single
  epoch is applied.

One latent defect surfaced while fixing the above: the §5.6 recovery rescan
re-ingests blocks *below* the frontier, and `insert_block_data` let that pull
the ingest cursor backwards — a backfill that hit a mismatch then looped
forever (rescan rewinds, next cycle re-advances, repeat). The cursor now only
advances there; `rollback_above` remains the only thing that moves it down.

## Repair pass on the live-run fixes (2026-08-31)

Two independent reviews of the fix pack (test-vacuity and adversarial) found
one critical regression, two false-alarm generators, and three tests that did
not pin what they claimed. The corrections, and the design decisions worth
recording rather than rediscovering:

- **verify-root has ONE candidate block, not a search.** The target is
  `min(frontier, rpc_head)`: pinned from below because a root above the frontier
  covers writes we have not ingested, and from above by what the chain has. The
  original fix wrapped that in a four-probe loop, but the target is recomputed
  identically every time, so the loop could only ever return the same answer at
  a cost of eight RPC calls and 1.4 s. It is now a single attempt. The
  consequence is deliberate and permanent: while `head - frontier` exceeds the
  storage-proof window — the whole deep-backfill phase — verify-root can only
  answer UNAVAILABLE, and the check becomes live once the mirror catches up.
  `t12_a_frontier_far_below_head_is_unavailable_not_a_failure` pins it.
- **The §5.6 recovery rescan follows the verification block.** It used to stop
  at `min(l1_accepted, frontier)`; since LIVE-4 the check happens ~5000 blocks
  higher, so a divergence above `l1_accepted` fell outside the rescan,
  reproduced on every retry, and stopped epoch publication permanently with
  `/health` latched DEGRADED. The rescan now runs to the frontier.
- **Anchors are reorg-scoped.** Head-side anchors sit far above `l1_accepted`
  and are therefore reorgable; `rollback_above` now drops them with the rest of
  the orphaned tail and the log is republished, or every client would report a
  permanent false divergence the publisher manufactured.
- **An anchor without a chain block hash is not published.** Substituting
  `Felt::ZERO` for a missing `global_roots.block_hash` produced the same false
  alarm from the other direction. The block header is used as a fallback; when
  neither is available the anchor is dropped.
- **Capture cadence is per ingest cycle, not per cut batch.** Tying it to a cut
  meant one anchor per `epoch_size` (10 000) blocks on mainnet, which is not
  "opportunistic". The probe still lives in the cutter, not in `ingest.rs` — it
  depends on the mirror being complete to the frontier and on the root
  comparison — and is skipped while the frontier has not moved.
- **The tail is published before the cut.** `/health` reports the new head as
  soon as the ingest cycle commits it, so a consumer that polls `/health` and
  then fetches `head.ndjson` must not receive a tail from before that block; the
  cut path (verify-root, anchor probe, a §5.6 rescan) can take seconds. The head
  is regenerated again after any cut, when the epoch floor moves.
- **"Could not check" is never "nothing was wrong".** `fetch_anchors` now
  distinguishes 404 (the feed publishes none) from every other transport
  outcome, and `verify-anchors` exits non-zero unless it actually checked at
  least one anchor, reporting a `status` that says which case it was.
- **Chain id is enforced, not merely stamped.** The client pins it on first sync
  and compares it on every later one exactly as it does the pool, genesis and
  manifest must agree, and each epoch payload is bound to the chain and pool the
  feed declares (`verify_epoch_binding`). `--network` stays an additional
  external assertion, not the only check.
- **Storage proofs use a short transport budget and never strand on one
  endpoint.** An unreachable or throttled endpoint is not an answer about the
  proof, so `get_storage_proof` moves to the next candidate instead of
  returning; only semantic answers short-circuit. Sustained HTTP 429 escalates
  once to the next endpoint after the in-place budget is spent — still not
  "429 counts toward failover", but no longer fatal to an unattended backfill.
- **`anchors.ndjson` canonicality is a write-side property.** The bytes are a
  pure function of the anchor SET; the set itself is operator-specific because
  captures are opportunistic, so two honest mirrors legitimately differ.
  `parse_anchors` validates structure but deliberately accepts non-canonical
  spellings — a reader cannot detect a non-canonical publisher and must not be
  written as if it could.
