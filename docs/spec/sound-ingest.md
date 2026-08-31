# Sound ingest — spec

Status: FINAL for implementation. Replaces the ingest-soundness reasoning in
[architecture.md](architecture.md) §5.6 (the "rescan recent epochs" recovery
path) and closes LIVE-8's open end. Deltas of the shipped build live in
[implementation-notes.md](implementation-notes.md).

Council input: one proposal survived phase Propose —
[s2-pragmatic](../research/council/sound-ingest/s2-pragmatic.md). Both upstream
measurement phases (MECHANISM/PREVALENCE, INDEX COSTS) failed, and S2
re-established them itself. **An unopposed proposal resting on
self-supplied measurements is exactly the shape of the failure this exercise
exists to prevent**, so instead of grafting from losers, this document
re-derives every claim it could reach without the network and marks the rest by
provenance. Nine claims were checkable locally; eight held exactly, one was
wrong in S2's own disfavour, and three unstated risks surfaced. Details in §2.

Provenance tags used throughout:

| tag | meaning |
|---|---|
| **[V]** | re-verified in this session against `data/mainnet/strk20.db`, `data/mainnet/catchup3.log`, or the source tree |
| **[M]** | measured by S2 against mainnet; not independently re-run here |
| **[X]** | extrapolation from a measured sample — the multiplier is named |
| **[P]** | projection of an unbuilt change — **not** a measurement |

---

## 1. The defect, stated once

Ingest uses `getEvents(address = pool)` as the **index** of which blocks to
visit, then mirrors storage from `getStateUpdate` at each visited block. The
mirror is a storage plane; events are only the index.

That index is unsound, and not because of paging. `apply_actions` — the pool's
ordinary user entrypoint — can write pool storage and emit **no pool event at
all**. Block 11,721,848 does exactly this: 7 pool storage writes, 0 pool
events, confirmed on three independent surfaces (`getStateUpdate`,
`getEvents`, and the block's own `getBlockWithReceipts`), with
`traceBlockTransactions` naming the entrypoint **[M]**. The written slots have
the shape of channel establishment (`recipient_channels`,
`subchannel_tokens`, plus `*_exists` boolean singletons), which
[verify-classifiability.md](../research/verify-classifiability.md) had already
predicted has no events, and which nobody acted on.

**This hole class is structural and permanent.** No amount of paging
discipline, endpoint hardening or retry budget recovers it, and no
events-based audit can see it: `audit-coverage` compares our event counts to
the chain's event counts, so a block with writes and no events is invisible to
it *by construction*. The shipped audit reports the mirror complete
(28,678 blocks / 120,216 events — matching `data/mainnet/audit.json`'s
`chain_blocks`/`chain_events` exactly **[V]**) while `verify-root` mismatches.
Both statements are true.

A singleton slot holding `0x1` is shape-indistinguishable from
`notes(note_id)` or `nullifiers(x)` without deriving the preimage, which S2
could not do **[M]**. So the conservative reading stands and this spec is built
on it: **an unrepaired eventless block may hide a note or a nullifier**, and a
missed nullifier shows a spent note as unspent to a real user.

---

## 2. Verdict on the proposal's claims

### 2.1 Upheld, re-verified locally

| claim | S2 | verified |
|---|---|---|
| mirror size | 28,678 blocks / 120,216 events / 139,565 writes | exact **[V]** |
| distinct slots / write-once rate | 135,313 / 96.9% first writes | 135,313 → 96.95% **[V]** |
| active-block gap distribution | median 10, p90 181, p99 3,314, mean 180 | exact **[V]** |
| binary search depth | ⌈log₂ 28,678⌉ = 15 | 15 **[V]** |
| root-ladder-as-index is not viable | ~430k proof successes at a = 28,678, N = 5.18M | arithmetic checks **[V]** |
| current recovery does not converge | 4 rounds, 0 progress, cutting halted | exact, `catchup3.log` **[V]** |
| the recovery bound is structural, not a tuning error | — | confirmed in source, see §2.3 **[V]** |
| `verify-root` is already three-valued | MATCH/MISMATCH/UNAVAILABLE required | already shipped, `cutter.rs:302` **[V]** |
| ingest issues a redundant `getBlockWithTxHashes` per active block | 28,678 calls deletable | confirmed, `ingest.rs:338` **[V]** |

The LIVE-8 single-page subdivision is also already shipped (`ingest.rs:198`,
`scan_active_blocks`) **[V]** — this spec inherits it and does not re-argue it.

### 2.2 Corrected

1. **The non-convergent rescan is worse than S2 says.** S2's §5.0 table gives
   round 1 as ~28 min. The log timestamps give **50.7 min**; the four rounds
   total **2.46 hours**, not two **[V]**. The correction runs against S2's own
   argument, which is a point in its favour.
2. **Serial rescan throughput, derived from the same log: 4.9–5.9 blocks/s**
   **[V]** — against S2's measured 295 blocks/s batched **[M]**. That is a
   ~50× gap and it is the strongest available support for batching
   `getStateUpdate`; it is an independent local corroboration of a number S2
   measured remotely.
3. **"Sub-second either way" for the gap scan is wrong.** At 295 blocks/s the
   p99 gap (3,314 blocks) takes **11 s** and the largest real gap in the mirror
   (**102,687 blocks** **[V]**, unmentioned by S2) takes **~6 min**. Bound the
   gap scan explicitly (§4.3) rather than assuming it is free.
4. **Root-ladder break-even is at a gap of ≈14, not 10** (34·log₂g KB vs
   9.1g KB) **[V]**. S2's conclusion — roots are a scalpel, not a plough — is
   unaffected and adopted.

### 2.3 The code-level root cause S2 diagnosed but did not locate

`verify_root` reports the mismatch at the **probe** block:

> `cutter.rs:269` — `"VERIFY-ROOT MISMATCH at block {block}: … Recover with a full-range rescan of recent epochs."`

and `rescan_lower_bound` (`cutter.rs:1112`) then **parses that block number out
of the message** and widens only to the epoch containing it, floored at
`last_epoch.to + 1` (`main.rs:380`) **[V]**. The probe block is by construction
near the frontier. So the recovery range is derived from *where we looked*,
never from *where the divergence is*. The message's word "recent" is not
advice; it is the bug, and it is load-bearing in code. That is why every round
in `catchup3.log` rescanned 14.14M+ while the divergence sits at 11.72M.

### 2.4 Downgraded — cost claims that are not measurements

- **Phase 1 (pool-event index) at "10–15 min" is [P], not [M].** S2 says so
  itself, and adds that its own strict single-page pool scan *did not finish
  inside 25 minutes*. The only measured full-range number for the pool index is
  the 72 min of the shipped backfill **[M, inherited]**, and that run used the
  *unsound* paging. **The cost of a full-range single-page pool-event scan is
  unmeasured.** It is, however, bounded from below by a real analogue: the fee
  index — same endpoint, same single-page discipline, same full range, a
  *denser* event stream — completed in **453 s** **[M]**. Plan for 8–25 min,
  hold the 72 min as the number to beat, and treat any figure under 10 min as
  unproven until a run produces it.
- **The flat state scan at "4.9 h, 47 GB" is [X], ×25.9** from a 200,000-block
  sample (3.86% of history) **[V]** on the arithmetic. Retry waste (8.1%) was
  concentrated in deep windows, so linear extrapolation is roughly fair but not
  guaranteed. Quote it as "≈5 hours, tens of GB", not as a measurement.
- **"~21 s per hole"** assumes recommendation 4 (cached local roots, concurrent
  proofs) is already done. Today a probe costs 22.8 s **[M]**, so a 15-probe
  search costs **~6 min** — still 25× better than one fruitless rescan round,
  but state the two numbers separately and never quote 21 s for the current
  build.

### 2.5 Prevalence — accepted as a cost model, not as a correctness claim

H = 4 known holes across all mainnet history, 95% ceiling ≈ 70, from a
1,239-write-active-block uniform sample with zero holes plus an exhaustive
fee-index diff **[M]**. Accepted for what it is used for — choosing repair over
scanning — and for nothing else. §7.7 restates why.

---

## 3. The principle

Do not buy index soundness. Buy a **cheap redundant index closed out by a
cryptographic check**.

A flat `getStateUpdate` scan is sound only *relative to a trusted RPC*, and the
RPC is an anonymous load-balanced pool that has now produced four distinct
defects from the same root cause (LIVE-1, LIVE-8, proof-window §3, and the
latency tail in S2 §2.2). A flat scan does not remove that trust assumption; it
takes 5.18 million independent draws on it and removes the pagination whose
inconsistency is what made the loss visible in the first place.

The union index is heuristic, but every block it yields is closed out by a
Pedersen MPT root the chain itself commits to:

> **`verify-root` MATCH at block B proves the mirror holds the correct value of
> every pool slot as of B.** Because the note/nullifier plane is write-once
> (96.95% of writes are first writes; the exceptions are mutable admin slots
> **[V]**), it further proves the mirror is missing no note and no nullifier
> written at or below B — regardless of how the blocks below B were found, or
> whether the index that found them was sound.

The index only has to be good enough that the check rarely fires. On this
mirror it fired four times in five million blocks **[M]**.

---

## 4. Chosen design

### 4.1 Design (a) — fresh sync

Four phases. Phases 1–3 are the fast heuristic; phase 4 is what makes it
correct. **Nothing is published before phase 4 returns MATCH.**

**Phase 1 — pool-event index.** `getEvents(address = pool)` over the full
range, strict single-page windows: a response carrying a continuation token is
discarded, the window halves, the range is retried (LIVE-8; already shipped at
`ingest.rs:198`). Runs first because it also yields `FeeCollectorSet` /
`FeeAmountSet`, which phase 2 needs.

**Phase 2 — fee index.** `apply_actions` charges a fee that moves as an
ordinary ERC-20 `Transfer` to the pool's fee collector, on the token contract,
with the collector in the event **keys** — so it is filterable. For each
fee-collector era learned in phase 1:

```
getEvents(address = fee_token, keys = [[Transfer], [], [collector]])
```

same single-page discipline. Eras on mainnet, from our own event table:
`FeeCollectorSet` @9,079,297 → `0x0391b954…`, @9,477,439 → `0x0d790416…`;
`FeeAmountSet` 4 STRK @9,079,357, 6 STRK @12,806,094 **[M]**. The index is
therefore self-maintaining from data already ingested.

Candidate set = phase 1 ∪ phase 2. The two indices fail in **disjoint** ways —
the event index misses fee-paying user operations that emit nothing; the fee
index misses zero-fee admin operations that emit events (measured: 5 blocks in
the first set, 12 in the second, over the whole of mainnet) **[M]**.

**Phase 3 — batched candidate fetch.** `getStateUpdate` for every candidate,
**batch 100, 12 workers**, per-id retry on error. Batch 500 is rejected by the
endpoint and 32 workers draws HTTP 500 **[M]** — those are the ceilings, do not
raise them speculatively. Take `block_hash` from the same response; **delete
the per-block `getBlockWithTxHashes`** (`ingest.rs:338`) — 28,678 calls saved.

**Phase 4 — the closure loop.** §4.2.

**Expected mainnet cost:**

| phase | shipped today | after the §8 work |
|---|---|---|
| 1. pool-event index | ~72 min **[M, unsound paging]** | 8–25 min **[P]**, lower bound anchored by the 453 s fee-index run |
| 2. fee index | 7.5 min **[M]** | ~5 min **[P]** |
| 3. candidate fetch | minutes (57,356 serial calls) **[V, structural]** | ~2 min **[M]** |
| 4. closure, H = 4 | n/a (loop does not exist) | ~6 min today / ~1.5 min cached **[M/P]** |
| **total** | **~80 min, ~1 GB** | **~20–35 min, ~1 GB** |

Against the flat sound scan: **≈5 h, tens of GB [X], and no cryptographic check
at the end.**

**The honest comparison is narrow, and the spec says so:** ≈5 hours for a
from-scratch scan is affordable. If sync-from-scratch were the only question,
"just scan the state plane" would be a defensible answer. It is not the only
question — §4.2 is — and the flat scan does nothing for a mirror that is
already five million blocks deep.

**Fallback, retained:** `--full-state-scan` flat-scans the range with batched
`getStateUpdate`. It costs ≈5 h and it always works, relative to the RPC. It is
the recovery path, never the default.

### 4.2 Design (b) — hole repair in an existing mirror

Entered from the cheapest front door, ordered by cost.

**Step 0 — one bound proof.** `verify-root` at `min(frontier, head)`.
**MATCH → the mirror is provably complete, stop.** The whole audit, in one
call, for 6.8 KB **[M]**. This is why the design is affordable: the common case
is free.

**Step 1 — fee-index diff, ~7.5 min.** On MISMATCH, recompute the fee index
over the full range and diff against the mirror's block set. On this mirror
that yields 5 candidates, 4 real **[M]**. Repair them, re-run step 0.

**Step 2 — the closure loop, the part that needs no heuristic.**

```
local_roots ← root after every known-active block          (§8.4, one pass)
loop:
    p ← probe(frontier)
    MATCH        → stop; mirror provably complete ≤ frontier
    UNAVAILABLE  → back off and retry; NEVER report OK or MISMATCH
    MISMATCH     → i ← binary search over the ACTIVE-BLOCK INDEX for the
                       first mismatching b[i]                (15 probes)
                   gap ← (b[i-1], b[i]]   # b[0] case: [genesis, b[0]]
                   flat-scan gap with batched getStateUpdate
                   ingest every block with pool writes
                   refresh local_roots
```

`probe(B)` = `getStorageProof(B, [], [pool], [])` — **empty arrays, never
`null`** (LIVE-7) — retried until a backend answers (mean 2.5 attempts **[M]**;
lava's pool is part-archive, proof-window §1), then
`global_roots.block_hash` compared against
`getBlockWithTxHashes(B).block_hash` before the root is believed. That bind is
mandatory and already implemented (`cutter.rs:bound_proof`) **[V]**.

**Why search the index and not the block number.** The mirror's local root is a
step function, constant on `[bᵢ, bᵢ₊₁-1]`. Searching 28,678 active blocks costs
15 probes instead of 22 over 5.18M block numbers, and — more importantly — it
lands on an *interval between two known-active blocks*, so clustered holes come
out together. Median interval 10 blocks, p90 181, p99 3,314, max 102,687 **[V]**.

**Monotonicity, and its one exception.** Binary search requires the predicate
"local root == chain root at bᵢ" to be monotone in i. It is, for the
note/nullifier plane, because a missed write to a write-once slot is never
learned later. It is **not** guaranteed for the 3.05% of writes that overwrite
a mutable admin slot: a missed write there can be masked by a later overwrite
the mirror *does* capture, healing the root while the block stays absent. See
§7.10 — this is a real residual risk and it is new to this document.

**Step 3 — republish, local, no RPC.** A repair below the epoch floor rewrites
published history: `recut-epochs --from-block B` re-cuts that epoch and every
epoch above it. **Concretely, for the known first hole at 11,721,848 that is
epoch 1172 and 243 of the mirror's 518 epochs — 47% of the published feed
changes content hash** **[V]**. S2 mentions this only in passing; it is the
largest operational consequence of the whole repair and consumers must be told.
It is a feed event, not an indexer detail.

**Cost, on this mirror:** 15 probes ≈ 53 RPC calls ≈ 255 KB, plus a
median-10-block gap scan, plus one root refresh. **~6 min today, ~15 s once
§8.4 lands** — against **2.46 hours of provably non-convergent rescanning**
**[V]**, which is what the shipped build does instead.

### 4.3 Bounds the implementation must enforce

- Gap scan is capped: a gap wider than 20,000 blocks is scanned in batched
  chunks with progress logging, not as one silent unit (max real gap is
  102,687 blocks ≈ 6 min **[V]**).
- Closure loop iteration cap (default 32). Exceeding it is not "keep trying" —
  it means the index has decayed (§7.3) and the operator is told to run
  `--full-state-scan`.
- Every `getEvents` in phases 1–2 fails fast at ~3 s and re-issues rather than
  waiting out a slow backend: the same request measured
  0.52 / 0.55 / 0.57 / 0.57 / 1.07 / 1.12 / 2.65 / 10.88 / 17.40 / timeout-at-90 s
  **[M]**. Latency is a property of the routing, not the query.

---

## 5. What runs continuously, and what on demand

| cadence | what | cost |
|---|---|---|
| every poll cycle | pool-event index + fee index on the new tail; batched `getStateUpdate` for candidates | one extra `getEvents` per cycle |
| **every epoch cut** | `verify-root` at `min(frontier, head)`; append `(block, block_hash, storage_root, class)` to `anchors.ndjson` | **1 bound proof, 6.8 KB** |
| on MISMATCH | closure loop §4.2 | ~6 min per hole today, ~15 s after §8.4 |
| on demand / weekly | full-history fee-index diff | 7.5 min |
| last resort | `--full-state-scan` | ≈5 h, tens of GB |
| **never** | blind rescan of a recent window on MISMATCH | 23–51 min per round, does not converge |

The per-epoch proof is the load-bearing habit and it is nearly free. It keeps
"last proven-complete block" marching behind the frontier, so any future hole
is bounded to one epoch rather than five million: the 15-probe search collapses
to ~8 and localisation becomes instant.

**This habit is not in effect today: 1 of 518 epochs carries an anchor** **[V]**.
LIVE-4 is the cautionary tale — the check existed, was correct, and had never
once run. A completeness check that runs once is forensics; the same check run
every epoch is an invariant.

---

## 6. Failure modes

| mode | detection | response |
|---|---|---|
| eventless pool write (the motivating defect) | fee index, or the closure loop | repair block, recut epochs |
| write that is both eventless and fee-less | closure loop only | binary search + gap scan; no heuristic involved |
| continuation-token loss (LIVE-8) | single-page discipline prevents it; `verify-root` catches residue | subdivide window |
| proof endpoint refuses (error 42) | `is_proof_unavailable` | **UNAVAILABLE** — retry, back off; never MISMATCH, never OK |
| proof for the wrong block | `global_roots.block_hash` vs header, twice | `PROOF_NOT_BOUND`, halt loudly, do not touch `verify_root_failed` |
| slow backend (p90 17 s, timeouts) | 3 s deadline | cancel and re-issue on the same URL |
| failover onto a proof-less provider (LIVE-6) | per-endpoint capability tracking | never route a proof request there |
| repair below the epoch floor | `last_epoch.to` comparison | `recut-epochs`, then `epoch-verify`, then `verify-root`; announce the chain-hash change |
| prolonged UNAVAILABLE | `blocks_since_last_match` (§8.3) | **new**: degrade health after N epochs unverified — today this state is silent |

---

## 7. What this deliberately does not protect against

Stated plainly, because the previous design's residual risk was unstated and
turned out to be real.

**7.1 It does not make the index sound, and does not claim to.** The union
index is two heuristics. Their union was complete over the 53% of history that
`verify-root OK at 11,721,847` proves, and over a 3.86% uniform sample of the
rest **[M]**. Evidence, not a theorem. The design is safe because a heuristic
miss is *detected*, not because it cannot happen.

**7.2 A root match proves storage completeness, not event completeness.** A
block ingested via the *fee* index whose events we then failed to page passes
`verify-root` while the `events` table is short — breaking typed stats,
`EncNoteCreated` payloads and the explorer without breaking discovery. Both
audits must keep running; neither subsumes the other.

**7.3 The fee index depends on contract policy we do not control.** It is blind
before block 9,079,357 (fee unset — already cleared cryptographically by the OK
at 11,721,847). It would go blind again on a zero-fee path, a fee in a
different token, or a fee-collector rotation we miss — and the collector is
itself learned from a pool *event*, so a missed event propagates into a missed
fee index. There is no third heuristic worth adding.

**7.4 It does not protect against a dishonest endpoint.** Binding every proof
to `getBlockWithTxHashes` defeats a load-balanced pool serving a proof for the
*wrong* block — the failure we can demonstrate. It does not defeat an endpoint
serving an internally consistent *fake* chain, because we never check a block
hash against L1. Closing that needs a second independent provider or an L1
checkpoint. We have neither.

**7.5 A MATCH near head is not final.** Proofs answer at any depth with retry,
but the fast path verifies near head and a near-head block can reorg. Finality
stays the epoch floor's job; a MATCH at an unfinalised block is recorded as
such.

**7.6 UNAVAILABLE is not MISMATCH.** Three-valued or nothing.

**7.7 The prevalence bound is not a correctness claim.** H = 4, ceiling ≈ 70.
Those numbers justify the *cost model* — why repair beats scanning. They are
not why the mirror is correct. The mirror is correct because a root matched.

**7.8 The eventless writes are probably channel data, unproven.** A singleton
`0x1` could be a note or a nullifier. Nothing in this design depends on which,
and nothing downstream should.

**7.9 It covers one contract.** The write-once argument, the root check and the
fee index are all specific to the pool. A helper or anonymizer contract gets
none of it for free.

**7.10 A root match at the frontier does not prove per-block attribution
(new).** The root commits to the slot→value map *as of B*, not to which block
wrote each slot. For write-once slots the two coincide. For the 3.05% of writes
that overwrite a mutable admin slot they do not: a missed write at block h that
is later overwritten by a value we did capture leaves the frontier root
matching while block h is still absent from the mirror and its epoch bytes are
still wrong. Consequences: (i) the binary-search predicate is not strictly
monotone, so the search finds *a* divergence, not provably the *first*; (ii) a
consumer folding the feed to an intermediate epoch can read a wrong value for a
mutable admin slot. Neither affects notes or nullifiers, which is the plane
users depend on. Mitigation, not elimination: keep running `audit-coverage`,
which catches the missing block on the event plane whenever the block also
emitted events — and admin writes generally do.

**7.11 Unverified is not verified (new).** UNAVAILABLE does not block epoch
publication in the shipped build (`cutter.rs:779` gates only on
`verify_root_failed`) **[V]**. That is the right call — a capability gap must
not report as corruption — but it means a provider that never serves a proof
lets the feed publish indefinitely with no completeness evidence and no alarm.
§8.3 adds the alarm. Until then, "we have not been told we are wrong" is being
served as if it were "we have been proven right".

---

## 8. Changes to the shipped commands (described, not implemented)

Ordered by what fixes a system that today cannot converge, then by speed.

**8.1 `audit-root --repair` — new; replaces the recent-window rescan.**
The closure loop of §4.2 as a first-class command, and as the recovery path
`cut_epochs_with_recovery` calls. Concretely: delete the dependence on
`rescan_lower_bound` parsing a block number out of an error message
(`cutter.rs:1112`, `main.rs:380`), and change the MISMATCH text at
`cutter.rs:269` to stop saying "recent epochs" — it is advice that is wrong in
every case observed so far. The mismatch message should name the probe block
*and* say the divergence may be arbitrarily far below it. This is the only item
that fixes a system which today cannot converge at all.

**8.2 `audit-coverage` — keep, and stop letting it imply completeness.**
It compares event counts to event counts and is blind to the entire hole class
by construction. Changes: (i) add the fee index as a second source, so its
block set is the union and its output names *storage-plane* candidates
separately from event-plane discrepancies; (ii) rewrite the "audit-coverage OK"
line so it cannot be read as "the mirror is complete" — it means "our event
counts equal the chain's, which does not cover blocks that wrote storage
without emitting events; run `verify-root` for that"; (iii) reuse
`reingest_blocks` unchanged as the repair primitive — it is correct, only its
input set was wrong.

**8.3 `verify-root` — turn from forensics into an invariant.**
(i) Run it at **every** epoch cut and record the anchor (1 of 518 epochs
carries one today **[V]**); (ii) persist `last_match_block` and
`blocks_since_last_match`, and surface DEGRADED once the mirror has published
more than N epochs (default 6) with no MATCH — distinguishing "unverified" from
"verified OK" (§7.11); (iii) keep the three-valued outcome exactly as shipped.

**8.4 `verify-root` as a probe — the performance change the loop needs.**
Today each call runs `full_slot_set_as_of` plus a full MPT rebuild
(`cutter.rs:253`) **[V]**, 22.8 s wall **[M]**, which makes a 15-probe search
cost ~6 min. Replace with: one pass that inserts all 139,565 writes in block
order and snapshots the root after each active block, giving all 28,678 roots
(≈918 KB, or a `block_roots(block, root)` table) in one build; invalidate from
the lowest repaired block upward. Then a probe is a table lookup plus one bound
proof. Fetch the 15 proofs concurrently. 22.8 s → ~1 s.

**8.5 `recut-epochs` — keep, and make the blast radius visible.**
The mechanism is right and needs no RPC. Add: print how many epochs will be
rewritten before doing it (243 of 518 for the known first hole **[V]**), and
emit a machine-readable republish record consumers can key on, since 47% of the
feed's content hashes change. `epoch-verify` after, then `verify-root`.

**8.6 `backfill` — batch and delete.** Batched `getStateUpdate` (batch 100,
12 workers; 500 and 32 are the measured ceilings) and drop the redundant
per-block `getBlockWithTxHashes` (`ingest.rs:338`). Add the ~3 s fail-fast
deadline on `getEvents`. Add `--full-state-scan` as the flat fallback.

**8.7 Test-suite gap.** The fixture RPC is a single honest process, so it
cannot express any of these failures. It needs fault modes for: a
plausible-but-wrong page for a foreign continuation token; error 42 on a
fraction of proof calls; a proof whose `global_roots.block_hash` belongs to
another block; and a block that writes pool storage while emitting no pool
event. The last one is the whole subject of this document and no test in the
tree can currently represent it.

**8.8 The outstanding falsifiable prediction.** Repair 11,721,848; 11,721,893;
14,100,846; 14,101,246, recut epochs from 11,721,848, and `verify-root` at the
frontier goes MATCH. If it does not, a fifth hole exists that the fee index
cannot see, and §7.1 is the section to re-read. S2 deliberately did not run
this against the live mirror; it stays the cheapest test of the whole design.

---

## 9. Residual risk, stated as bluntly as possible

The previous design's residual risk was **unstated**: "the events index is
complete enough" was an assumption nobody wrote down, no audit could test, and
the chain violated 4 times.

This design's residual risk, stated:

> **The union index is still a heuristic and can still miss a block. The
> mirror's correctness does not rest on it — it rests on a Pedersen root that
> the chain commits to, checked every epoch. So the real residual risk is not
> "the index missed something"; it is "the check did not run".**

That risk is concrete, not hypothetical, and it has already materialised twice
in this project. LIVE-4: `verify-root` existed, was correct, and had never once
executed against mainnet. Today: 1 of 518 epochs carries an anchor, and an
UNAVAILABLE streak publishes unverified epochs silently **[V]**. Both are the
same failure — a correctness argument whose load-bearing member is not running.

The three narrower risks worth naming beside it:

1. **§7.10** — a root match attests the slot→value map, not per-block
   attribution; for mutable admin slots a missed block can hide behind a later
   overwrite, so the search finds *a* divergence rather than provably the
   first. Notes and nullifiers are unaffected.
2. **§7.4** — nothing here defeats an internally consistent dishonest endpoint.
   All four defects this project has found came from one anonymous
   load-balanced pool, and the mitigation for the fifth is a second independent
   provider or an L1 checkpoint, neither of which is ingest design.
3. **§7.2** — storage completeness and event completeness are different
   properties. `verify-root` MATCH plus `audit-coverage` OK is the pair; either
   alone is a half-answer, and shipping either alone as "the mirror is
   complete" is how this started.
