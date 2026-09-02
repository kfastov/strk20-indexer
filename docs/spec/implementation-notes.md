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
   (git history: docs/research/verify-discovery-trace.md §3, removed
   2026-09-02), and it is covered by
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

## Snapshots + SSE repair pass (A1/A2), post-review

A test-vacuity review and an adversarial review of the A1/A2 work found one
vacuous convergence assertion, two ways `"anchored"` could be reported for a
mirror nothing had checked, and several rungs of the §1.5 ladder that no test
could falsify. What changed, and what is deliberately left undone:

- **A refused snapshot's rows do not survive the refusal.** `apply_snapshot`
  commits the slot set long before §11.3 reachability can run — the epochs above
  the basis and the head tail have to land first, because reachability validates
  them too. Anything ending the process inside that window (a rejection the
  operator re-runs past, Ctrl-C, an OOM kill) left a populated mirror, and a
  populated mirror is never empty again: the next sync skipped the snapshot
  branch and therefore the grounding, leaving the client permanently on a slot
  set it had explicitly refused. A `snapshot_pending_grounding` meta row is now
  written in the same transaction as the slot rows and cleared only by a
  grounding that passed; any failure of the snapshot path resets the mirror
  regardless of cold-start mode; and a ring-6 mismatch does the same.
- **Ring 6 grounds the MIRROR, never a re-downloaded anchor.** It used to fetch
  `anchors.ndjson` a second time and compare that record against the chain,
  while reachability had compared the mirror against a record from its own
  fetch. Those compose into "the mirror is the chain's" only if both fetches
  returned the same record, which nothing enforced — a hostile feed can answer
  two byte-identical GETs differently, and an honest one breaks it by appending
  an anchor in between. Ring 6 now recomputes the storage root from the client's
  own folded slot set and compares that with the user's RPC; the log is used
  only to CHOOSE a recent block, which is sound for any block at or above the
  basis because pool slots are write-once. Leg S7's two-faced server pins it.
- **Ring 6 is three-valued, like `verify-root` (§11.4/§11.5).** A MISMATCH fails
  the sync; a provider that does not implement `starknet_getStorageProof`, or
  whose window has moved past every block we can ask about, yields
  `Unavailable`, a loud WARN naming the endpoint, and the unchanged
  `server-asserted` grade. "Configured means mandatory" binds the one outcome
  that is evidence about the data. The old two-valued form turned LIVE-6 into a
  hard sync failure against a capability-poor endpoint.
- **Reachability tries every anchor at or above the basis, newest first.**
  Anchors are captured at head, so they sit on reorgable blocks, and the client
  fetches `head.ndjson` and `anchors.ndjson` in separate requests: a server
  reorg between them made the newest anchor disagree with a tail folded from the
  pre-reorg file, which was reported as tampering and, under `auto`, paid for
  with the full history replay §11 says snapshots exist to avoid. Reaching any
  anchor at or above the basis attests the snapshot, so a lower anchor is a
  sound fallback while a forged slot set still fails all of them.
- **Decompression is capped at 256 MiB (`DECOMPRESS_LIMIT`), per §1.5 ring 1.**
  The manifest that names a file's sha256 is written by the same server as the
  file, so a passing transport hash says nothing about how far the frame
  expands. Uncapped, a ~100 KB `.zst` allocates until the process dies — a tab
  crash on the browser target A1 exists to serve.
- **`history_floor` is enforced, not merely reported.** `ClientView::get_events`
  returns `HISTORY_UNAVAILABLE {"floor"}` for any range reaching below it, and
  `FeedStore::view` refuses a bound below the basis with
  `BOUND_BELOW_SNAPSHOT {bound, basis}`. Previously the floor was written to
  meta and surfaced as `history_from` and read by no enforcement anywhere: a
  pre-basis range returned the above-floor events with a success status, which
  is indistinguishable from "nothing happened down there" — the masked
  incompleteness R-L exists to forbid.
- **`--cold-start snapshot` refuses instead of degrading.** A feed with
  `manifest.snapshot == null` used to fall through to a full epoch replay
  silently, reported as `verified: "replayed"` — the run the operator explicitly
  asked not to do. It is now `SNAPSHOT_UNAVAILABLE`; `auto` still falls back.
- **`anchors.ndjson` is bounded and revalidated.** Capture is roughly once per
  ingested block, the whole file is re-encoded on every capture, and every
  grounded client downloaded all of it on every sync. Retention keeps the newest
  `ANCHOR_KEEP` records, the route serves a sha256 ETag with a 304 path exactly
  as `head.ndjson` does, and `parse_anchors` caps its input.
- **SSE reconnect jitter is drawn fresh, not from the pid.** `process::id()` is
  constant for the process, so every reconnect landed at the same sub-second
  offset — ~9 bits of stable, server-observable identity surviving reconnects,
  IP changes and OHTTP, which is exactly the linkability §2.6's residual
  paragraph assumed nothing would introduce.
- **The SSE client buffers bytes, not lossy text.** Chunk boundaries fall
  anywhere; decoding each TCP chunk on its own turned a multi-byte character
  split across two of them into U+FFFD plus a stray continuation byte. Every
  payload emitted today is ASCII, but the framing layer must not depend on it.
- **`entry.e` arithmetic is checked.** The epoch index comes from the fetched
  manifest; unchecked, a large value wrapped in release and could be made to
  equal `header.block`, letting a snapshot claim an arbitrary epoch index that
  was then written into `last_epoch_applied`.

### `header.class` is informational under §11 — recorded, not silently dropped

§1.5 ring 5 introduced the `header.class` check specifically because nothing
read the field, and pinned it to the anchor sidecar's
`contract_leaves_data[0].class_hash`. §11.1 deleted the sidecar (a proof at a
basis block is unobtainable), and with it the only value the check could compare
against: a snapshot-started client never fetches the basis epoch, whose footer
carries the class, and the anchors log records the class at a HEAD block, which
may legitimately differ from the class at the basis after an upgrade. So
`header.class` is written and read by no ring. It stays in the format because it
is inside the content hash and is useful to an auditor, and **spec leg m(vi) is
not implementable as written** — it needs the sidecar §11 removed. An operator
running an archive node (§11.6) can restore both.

### Coverage debt recorded rather than papered over

- Leg **l(v)**'s history API (`complete`, `complete_from`,
  `registration_available`, the positive above-floor comparison) belongs to A5
  `serve`, which this branch does not build. The access-layer half —
  `HISTORY_UNAVAILABLE` below the floor — is implemented and unit-tested here.
- Leg **n**'s other halves (`strk20-sync snapshot audit`, the mirror-pull
  regeneration compare, `strk20 epoch verify --all` extended to snapshot hashes,
  a `--snapshot-keep` flag) are not implemented; `SNAPSHOT_KEEP` is a constant.
  Determinism itself is pinned by S1 across two backfills.
- Leg **o(iii)**'s poke-driven client across the leg-g reorg is not exercised;
  E1 pins the poke path and leg g pins the reorg, but not together.
- S1's "two independent operators" is two runs of the same binary against the
  same fixture RPC — real byte-determinism, not operator independence — and the
  harness's independent encoder borrows the product's `felt_hex`, with the hex
  spelling pinned separately by `feed/tests/snapshot.rs::golden_snapshot_bytes`.

## LIVE-8 + §12 correction pass (2026-08-31)

Two defects, both measured against live mainnet, both with one root cause:
`rpc.starknet.lava.build` is an **aggregator**, so successive calls to the same
URL reach different backend nodes with different capabilities and different
local state. Code that assumed "the endpoint" is one node was unsound.

### A — the scan no longer presents a continuation token (LIVE-8)

`ingest.rs::scan_active_blocks` used to page through one big `getEvents` range
with a `continuation_token`. A token is **node-local state**: handed to a
different backend it does not error — it resumes from somewhere else and the
events in between are dropped silently. Measured on the same range and
endpoint, `chunk_size=1000` found 2,628 distinct blocks in 13 pages while
`chunk_size=200` found 2,608 in 62; a full mainnet backfill lost 139 blocks and
489 events (chain 120,135 events in 28,655 blocks, mirror 119,646 in 28,532),
which is what made `verify-root` report a genuine root mismatch.

The scan now **subdivides the block range until every window is answered in a
single page with no continuation token**, and takes the union. A single
response carries no cross-request state, so it is sound under any routing —
which also removed the "restart the scan when our own failover fired" branch,
since an endpoint change mid-scan is now harmless.

- Window sizing is predictive (`next_window_len`: aim at three quarters of a
  page, using the page capacity the endpoint has actually demonstrated), with
  halving whenever an answer carries a token. An overshoot costs exactly one
  call.
- A window that still carries a token at **single-block granularity** is a hard
  error naming the block. Keeping the first page would be this very defect,
  silently; following the token is what the whole change forbids.
- `ingest_block` no longer re-fetches a block's events: the scan already has
  them. That removed one `getEvents` per active block — the dominant cost — and
  with it a per-block paging loop that was unsound for the same reason. Only
  the §5.6 rescan path fetches, in one page, with the same irreducible-window
  error.

**Cost, measured** against the fixture RPC on a mainnet-shaped sparse chain
(5,200,000 blocks, clustered bursts, 25,797 pool-active blocks carrying 116,095
events — the real backfill saw 28,655 / 120,135; `chunk_size` 1000):

| | `getEvents` calls |
|---|---|
| subdivision scan (new) | **156** |
| paged scan, old | 117 pages |
| + one page per active block, old | 25,797 |
| old total | **25,914** |

So the scan itself costs 1.33x the old page count — 39 extra calls, the
halvings and the growth probes — and the run as a whole costs **166x less**,
because dropping the per-block re-fetch removes the term that dominated. A
200,000-block run at the same density: 20 calls versus 12 + 2,511.

### B — basis-block anchors restored (consumer-path §12)

`docs/research/live/proof-window.md` retracts the "~1024-block proof window":
that was a bisection over a nondeterministic predicate. Deep proofs answer for
any block, back to genesis, from the endpoint we already use — measured 2
successes in 4 attempts at 5.15M blocks behind head, with every proof's
`global_roots.block_hash` matching the real header.

- **B1** `get_storage_proof` retries error 42 against the **same** endpoint
  (`PROOF_RETRIES`) before moving on, and never fails over on a proof refusal
  (LIVE-6: publicnode implements no proofs at any height, so a failover
  guarantees a false alarm). Only after every endpoint has spent its budget is
  the answer `UNAVAILABLE`.
- **B2** `Cutter::bound_proof` is the only door to a proof: the response's
  `global_roots.block_hash` must equal `getBlockWithTxHashes(block).block_hash`
  before any `storage_root` is believed. The proof pool is anonymous and
  load-balanced, so without this, retry-until-success is indistinguishable from
  accepting whichever answer we liked. A disagreement — or a proof with no
  block hash to bind — is a hard error (`PROOF NOT BOUND TO BLOCK`), never a
  retry and never `UNAVAILABLE`; filing it under LIVE-6 would hide a lie behind
  a capability gap.
- **B3** `verify-root` keeps its three-valued `MATCH` / `MISMATCH` /
  `UNAVAILABLE` outcome and its capability awareness — those survive the
  retraction — and now actually reaches a verdict at a block of our choosing.
- **B4** a snapshot's **basis-block anchor is the primary grounding again**:
  `snapshots/{e:08}.anchor.json` carries the stored proof, `manifest.snapshot.
  anchor` carries `{block, block_hash, storage_root, class}`, and
  `manifest.snapshot.grounding` says which grounding was used
  (`"basis-anchor"` or `"reachability"`) rather than leaving a client to infer
  it from a missing field. The §11.3 reachability walk is **kept**, demoted to
  the fallback for a basis whose proof could not be obtained: it also validates
  the intervening epochs and it is the only check that catches an internally
  consistent forged slot set (S4(ii)). The client refuses a manifest that
  claims an anchor it cannot show, or whose sidecar does not agree with the
  snapshot's own slot set — `SNAPSHOT_ROOT_MISMATCH`.

The basis proof is attempted over a **bounded number of cycles per basis
epoch** (`BASIS_PROBE_ATTEMPTS`, tracked by `snapshot_basis_probe_epoch` +
`snapshot_basis_probe_attempts`), and the counter is written only by an attempt
that actually happened and actually failed. Both halves matter: a refusal is
per-call routing luck rather than a property of the block, so one unlucky group
of retries must not cost a snapshot its primary grounding for good; and a
counter written *before* the call would make the mismatch path below into the
silent skip it exists to prevent. The budget is bounded because an endpoint
that implements no proofs at any height must not be asked once per poll for the
life of the process.

**Method note, unchanged and still binding:** against an aggregating endpoint a
single failed request proves nothing. Three defects in this project now share
that root cause (LIVE-1 pruned history, LIVE-8 continuation tokens, the
retracted proof-window measurement).


## Repair pass on the LIVE-8 / §12 change set (2026-08-31, same day)

An adversarial review of the change set above found ten issues; nine are fixed
here, one is rejected with reasoning. Every fix carries a leg that fails when
the fix is reverted (verified by mutation, not by inspection).

**The live data-integrity hole (F1 + F2), one defect in two places.** A basis
proof that was OBTAINED and DISAGREED with the snapshot's slot set was detected
once and then lost: the per-epoch probe marker was committed before the proof
call, so the `bail!` left the marker behind, and the very next call — which
`cut_epochs_with_recovery` makes inside the same function — skipped the proof,
found the §11.3 anchor gate met, and published the slot set the chain had just
contradicted, with `grounding: "reachability"` and health still OK. The comment
above that `bail!` asserted the opposite of what the code did. Fixed on both
sides: the probe counter is written only after a definitive answer and never on
the mismatch path, and a basis mismatch now **latches** `verify_root_failed`,
which is what stops the fallback grounding from publishing while the divergence
stands. Pinned by `publication_gate.rs::a_basis_proof_that_contradicts_the_
slot_set_latches_instead_of_falling_back`, whose primary assertion is the
published file, not the latch.

F2 is the same hole's other end: the §5.6 recovery rescan started at
`last_epoch.to + 1`, and a basis mismatch is reported AT `last_epoch.to`, so the
rescan covered a range that provably could not contain the divergence and
reported "recovered 0 blocks". `cutter::rescan_lower_bound` now widens the range
to the start of the epoch containing the block the mismatch names, and a
mismatch that survives the rescan says so and names `--full-resync` instead of
repeating one line.

**F4 — reorg versus lie.** `bound_proof` compares a proof's
`global_roots.block_hash` with a header hash from a second, independently
routed call, at a block deliberately chosen near head. At that depth two hashes
for one block number are ordinary reorg behaviour, and reporting them as
`PROOF NOT BOUND TO BLOCK` puts routine chain noise on the one channel that
must stay quiet to be believed. A disagreement is now re-tested once — proof
and header both re-fetched — and only a disagreement that survives is the hard
error. A missing `block_hash` is still fatal on the first answer: no re-read
supplies a field the proof does not have.

**F5 — a continuation token does not mean the page was full.** `chunk_size` is
a maximum in the JSON-RPC spec and a provider may stop early on an internal
budget. The scan treated every token as evidence about event density, which
(a) clamped its page estimate monotonically for the rest of a multi-hour scan
and (b) aborted a backfill at a one-block window with "raise --chunk-size" —
advice that cannot work when the page was not full, and which fires on a block
with no pool events at all. Now: the estimate shrinks only on a page that came
back full, a short page carrying a token is re-requested once (a fresh
single-page request carries no cross-request state, so asking again is sound),
and the irreducible-window error reports both what was asked for and what came
back, with different guidance for the two causes.

**F6 — retry budget sized against the measurement.** `PROOF_RETRIES` was 8
against a worst observed success rate of ~0.2 per attempt, i.e. `0.8^8` ≈ 17%:
roughly one obtainable deep proof in six answered `UNAVAILABLE`. It is 16 now
(2.8%), and no caller depends on one group of attempts any more (see the basis
probe budget above). Also: `proofs_served` is incremented only after the
response parses, so an endpoint answering unparseable JSON no longer promotes
itself to the head of `proof_order`.

**F7 — a grounding claim with nothing behind it.** `manifest.snapshot.grounding`
was published and never enforced: a manifest claiming `"basis-anchor"` with
`anchor: null` was accepted, silently downgrading every consumer to the fallback
while both the manifest and the client's log said otherwise. The two fields come
from one `Option` server-side, so a disagreement is `FEED_MALFORMED`. (Base leg
m(iv) names this case `SNAPSHOT_ANCHOR_MISSING` for a design in which the anchor
was unconditional; under §12 B4 `anchor: null` is legitimate whenever grounding
says `"reachability"`, so only the contradiction is an error.)

**F9 / F10 — structural, not behavioural.** `RpcClient::get_events` no longer
takes a `continuation` parameter and `ingest_cursor` no longer has a
continuation column: "never present a token" is enforced by the type and the
schema rather than by two call sites and one integration counter. And the scan
is segmented (`SCAN_SEGMENT`), so a failure costs one segment instead of the
whole backfill — pinned by `t21`, where an irreducible window in the third
segment leaves the first segment's blocks mirrored and the frontier
checkpointed.

**F3 — REJECTED in part, and the false claim removed.** The review is right
that the code claimed more than it delivered: `cutter.rs` said the sidecar is
walked by "§1.5 ring 5, so the roots are not merely asserted", and the client
reads only `contracts_proof.contract_leaves_data[0].storage_root` — a scalar —
never `contracts_proof.nodes`. That comment is gone, and both the cutter and
`check_basis_anchor` now state exactly what the sidecar buys.

The proposed remedy — walk the contracts proof from the leaf to
`global_roots.contracts_tree_root` — is **rejected**: it would not change the
adversary model it is offered against. A keyless client has no independent
source for the block's state root, so `global_roots` is a publisher claim
however it is reached; a publisher forging the slot set can forge a
self-consistent trie over it just as easily, and the walk would buy a stronger-
sounding log line for no additional security. Note the base spec never claimed
otherwise — §1.5 defines ring 5 as self-consistency against the server's
declared root and leg m(ii-b) pins that nothing below ring 6 catches a
consistently recomputed tamper. This is therefore a code-comment defect, not a
missing check, and it is fixed by making the comments true. **Spec delta:**
§1.3's line about "the `contracts_proof` node set that §1.5 ring 5 walks" (in
the `db:`-transport discussion) overstates the shipped client for the same
reason; the conclusion it supports — `db:` cannot serve a snapshot cold start —
is unaffected.

**F8 — retracted reasoning deleted from the code.** `verify_root_in_window`
still carried the "~1024-block proof window" premise and told the reader that
"retrying the same block within one batch cannot change that", which is now
precisely backwards: retrying is the mechanism. Renamed
`verify_root_at_target`, with the UNAVAILABLE meaning restated as "every
endpoint spent its retry budget refusing" rather than "the block is too old".
