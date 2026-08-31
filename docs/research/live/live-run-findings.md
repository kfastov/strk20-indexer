# Live-run findings — real-network testing log

Started 2026-08-30. Every defect and observation from running the actual
binaries against real networks (mainnet, Sepolia). These feed the
implementation fix list and the next adversarial review.

## Session 1: `strk20 backfill` against mainnet (lava primary, publicnode fallback)

Command: `strk20 backfill --db data/mainnet/strk20.db --feed-dir data/mainnet/feed`
(defaults; release build at c57f5a5+).

**What worked:**

- Init checks, chain-id verification, meta bootstrap: clean.
- Events-first scan progressed genesis (8,978,970) → ~9,693,358 in ~10 min
  (~1.2K blocks/min-of-history per second through lava getEvents paging),
  ingesting 250 pool-active blocks / 1,143 events / 1,648 slot writes.
- Crash-resume: `ingest_cursor` persisted; a restarted backfill continues from
  the cursor, no rescan of covered range. Verified by restarting after the
  failure below.

**LIVE-1 (defect, high): one pruned-history JSON-RPC error kills the whole
backfill.** At scan cursor ~9,693,358 lava returned:

```
Error: rpc error from starknet_getEvents:
{"code":-32603,"data":"block 9693374 has been pruned; oldest retained block is 13108361","message":"Internal error"}
```

`RpcClient::call` deliberately treats JSON-RPC-level errors as non-retryable
(they are "not transport failures") — correct for semantic errors, wrong for
this class. Evidence it was lava itself, not the publicnode fallback: zero
`warn` lines in the log (tracing was on at `info`), and failover requires 5
consecutive logged failures — so the active endpoint never left primary.
**Lava is an aggregator: the same URL nondeterministically routes to archive
or pruned backends** (retention observed: exactly last 1,000,000 blocks on the
pruned one). A deep query can fail on one attempt and succeed on the next.
Fix direction: classify "pruned/Internal error -32603" as a *provider
capability* error → retry (same endpoint is fine for aggregators), bounded,
with backoff; never let it abort ingest. Note the related hazard: if failover
*had* happened, publicnode is also non-archive, so a deep backfill would die
there too — capability-aware endpoint selection matters for deep ranges.

**LIVE-2 (defect, medium): the scan phase logs no progress.** A multi-hour
backfill is silent between start and the final summary/error. Ops needs a
periodic `info` line (scan cursor, active blocks found, events ingested,
current endpoint).

**LIVE-3 (observation): `429` handling couples rate-limit pressure to
failover.** `TOO_MANY_REQUESTS` increments the same consecutive-failure
counter that triggers failover; a burst of throttling can flip a deep backfill
onto a fallback that cannot serve the range at all (publicnode: pruned).
Backoff-in-place is likely the right response to 429 for deep ranges; failover
should be reserved for hard transport failures. Revisit with LIVE-1 fix.

**Workaround in use meanwhile:** restart loop with `--rpc-fallback` pointed at
lava as well (each pruned hit costs one process restart + 5 s; the cursor
resumes; lava usually routes the retried page to an archive backend).

## Session 2: full mainnet backfill completed (2026-08-31)

With the LIVE-1 workaround (restart loop), the backfill reached head:

```
backfill complete elapsed_secs=4302 head=14128517
```

28 process runs (27 pruned-block aborts), 71.7 min of ingest work. Result:
**118,960 events, 28,383 pool-active blocks, 515 epochs, feed 16 MB, DB 66 MB**
for the full pool history 8,978,970 → 14,128,517. `epoch-verify` over all 515
epochs: hash chain OK. `class_history` recorded exactly the two known classes at
the two known blocks. Volume prediction from research (~19 MB raw feed) held.

**The user's own mainnet transaction is in the mirror.** Block 14,093,171,
tx_index 1, four events — selectors decoded: `ViewingKeySet`, `Deposit`,
`EncNoteCreated`, `Withdrawal` — matching the design-notes account of
registration + deposit + note creation in one transaction, plus the 6 STRK fee
leaving as a Withdrawal. First end-to-end confirmation on real data that the
ingest path captures a real user's real note creation.

**LIVE-4 (defect, CRITICAL): `verify-root` can never succeed in production.**

```
$ strk20 verify-root --db data/mainnet/strk20.db --feed-dir data/mainnet/feed
Error: rpc error from starknet_getStorageProof:
{"code":42,"message":"the node doesn't support storage proofs for blocks that are too far in the past"}
```

Measured proof window (bisection against lava mainnet, head 14,151,406):
OK at head−968, error 42 at head−975 — i.e. a **~1024-block sliding window**
(pathfinder's default trie retention), not the "~25–55k blocks" recorded in
implementation-notes.md §5. Meanwhile `l1_accepted` lags head by ~5,000 blocks
(14,128,517 vs 14,123,420 in this run). The cutter verifies at
`min(l1_accepted, frontier)`, which is *by construction* outside the window.
**Consequence: the mirror-completeness check that the whole trust story rests on
has never once run against mainnet, and would fail on every cut.** Same story on
Sepolia (research: proofs only at the exact head there).

Corollary **LIVE-5**: per-epoch anchors are not merely "best-effort absent
outside the proof window" — they are absent *always*. 0 of 515 epochs carry an
anchor, because an epoch's end block is by definition thousands of blocks old at
cut time. The client-side anchor check is vacuous in production today.

Corollary **LIVE-6 (defect, high): failover is not capability-aware.**
publicnode returns error 42 for `getStorageProof` at *every* height including
head — it does not implement the method at all. So a failover from lava to
publicnode (which LIVE-1/LIVE-3 make likely) silently converts every root check
into a failure, tripping `verify_root_failed` and DEGRADED health for a reason
that has nothing to do with mirror correctness.

Fix direction (feeds spec addendum A1): stop verifying at `l1_accepted`. Verify
at a block chosen *inside the live proof window* and ≤ our frontier, with error
42 handled as "move closer to head", not "mirror is wrong". Because pool slots
are write-once, a root match at block B subsumes every write below B, so a
head-side check is strictly stronger than an l1-side one for completeness
purposes; finality is a separate concern already handled by the epoch floor.
Replace per-epoch anchors with an append-only `anchors.ndjson` of
(block, block_hash, storage_root, class) captured opportunistically at head —
that turns an always-empty field into a real client-verifiable audit trail. And
track per-endpoint capability (proofs / archive depth) instead of treating
endpoints as interchangeable.

## Network facts confirmed live (2026-08-30)

- Mainnet l1_accepted at time of run: 14,108,361; the user's own
  registration+shield tx (block 14,093,171) is inside backfill range.
- publicnode mainnet endpoint: non-archive (this class of provider prunes to
  ~last 1M blocks) — usable for tail-following, not for deep backfill.
- Sepolia pool `0x0254a6b2997ef52e9f830ce1f543f6b29768295e8d17e2267d672c552cfe0d91`:
  version 2.0, fee 2 STRK, screener key set (screening enforced).
  First pool event at block 8,271,125. Class history (via
  ImplementationReplaced): deploy class `0x715b22ab…659d2af` →
  @10,829,820 `0x30b8c540…4b4b30b` (= mainnet v1) →
  @11,111,946 `0x1a78d2da…46033d18` →
  @11,612,079 `0x67dddd89…76b554d` (= mainnet v2) →
  @12,932,675 `0x56ab118a…45623b2` (current; not yet on mainnet — Sepolia
  runs ahead, making it the natural rehearsal stage for our upgrade/degraded
  path).
- publicnode sepolia rejects block tag `pending` ("unknown block tag") —
  starkli needs `--block latest`.
