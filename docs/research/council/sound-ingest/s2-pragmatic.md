# Sound ingest — S2, the pragmatic seat

Status: council proposal, 2026-09-01. Everything numeric below was measured
during this session against mainnet (`rpc.starknet.lava.build`) and against the
real mirror at `data/mainnet/strk20.db` (28,678 blocks, 120,216 events,
139,565 slot writes, frontier 14,157,582). Where I re-measured a claim that was
handed to me, I say so and give the retry count. Nothing here rests on an
estimate I did not take myself, except where explicitly marked *inherited*.

The two upstream sections this document was asked to build on
(MECHANISM/PREVALENCE, INDEX COSTS) came back failed. So §1 and §2 establish
them from scratch, and the design in §3–§8 follows from those numbers rather
than from the framing.

---

## 0. The one-sentence version

The events index is genuinely unsound and cannot be fixed by making it sounder;
but it is missing **four blocks in the whole of mainnet history**, a second
almost-free index catches all four, and a single storage proof proves the
result — so the answer is not a 4.9-hour state scan, it is *two cheap redundant
indices plus a cryptographic closure loop that finds and repairs whatever they
missed*.

For sync-from-scratch that is a 3.7×–12× win, and I will admit up front that a
4.9-hour flat scan would also have been acceptable. For the case that actually
hurts — a deep mirror that mismatches at an epoch cut — it is the difference
between fifteen seconds and *never*: the recovery path in the tree today spent
two hours rescanning and could not converge, because it only ever looks at
recent blocks and every hole we found is 2.4 million blocks down (§5.0).

---

## 1. Mechanism and prevalence (the failed section, re-established)

### 1.1 The defect is real — re-measured, not inherited

The premise handed to me was a single-attempt claim against an aggregating
endpoint, which is precisely the failure mode that produced three defects in
this project already (LIVE-1, LIVE-8, proof-window §3). So I retried it.

Block **11,721,848**, mainnet:

| probe | attempts | result |
|---|---|---|
| `getStateUpdate`, pool in `storage_diffs` | 3/3 | **7 storage entries**, byte-identical each time |
| `getEvents` filtered `address=pool` | 6/6 | **0 events**, no continuation token |
| `getEvents` unfiltered | 3/3 | 47 events, 16 distinct addresses, pool absent |
| `getBlockWithReceipts` — events in receipts | 1 | 47 events total, **0 from the pool**, all 10 txs `SUCCEEDED` |

The receipts probe is the one that closes it. `getEvents` could have been lying
(it is an index); the receipts are the block's own record, and they agree.
**The chain really did write seven pool slots in a block that emitted no pool
event.** This is not an RPC artefact and no amount of retrying, re-paging or
endpoint-hardening will recover it.

### 1.2 What wrote them

`starknet_traceBlockTransactions` at that block resolves it exactly. Transaction
index 3 (`0x1aab8598626ee6…`) makes a direct `CALL` into the pool at selector
`0x246333a752c1ac637ff1591c5c885e27d56060d241a29aad8475072da0777db`, with
`events: []` on that invocation. Matching the selector against
`docs/research/data/live-pool-abi.json`:

> `sn_keccak("apply_actions") = 0x246333a752c1ac637ff1591c5c885e27d56060d241a29aad8475072da0777db`

It is **`apply_actions`** — the pool's main entrypoint. Not an admin function,
not an upgrade, not a component event we forgot to map. An ordinary user
operation, through the front door, emitting nothing.

The written slots say what kind of operation. Sorted, they form runs of
*consecutive* addresses — the Cairo idiom for a multi-felt struct stored at
`base+0, base+1, …`:

| block | pool writes | consecutive-slot runs | singleton slots valued `0x1` |
|---|---|---|---|
| 11,721,848 | 7 | 3, 2 | 2 |
| 11,721,893 | 3 | 2 | 1 |
| 14,100,846 | 10 | 3, 2, 2 | 3 |
| 14,101,246 | 10 | 3, 2, 2 | 3 |

Against `discovery-core`'s `privacy_pool::storage_slots`, a 3-run is
`EncChannelInfo` (`recipient_channels`), a 2-run is `EncSubchannelInfo`
(`subchannel_tokens`) or `EncOutgoingChannelInfo` (`outgoing_channels`), and the
singleton booleans are `channel_exists` / `subchannel_exists`. Every block above
is one or more **channel establishments**. That matches what this project
already wrote down and then forgot to act on, in
[verify-classifiability.md](../../verify-classifiability.md):

> Channel/subchannel counts not derivable (no events, secret slots)

So the mechanism is structural and permanent: **channel establishment is a pool
operation family that writes storage and emits no event, by design.** An
events-based index can never see it, and the hole class will keep being
produced as long as channels are used.

**One thing I did not prove, and the design must not assume.** A singleton slot
holding `0x1` is shape-indistinguishable from `notes(note_id)` or
`nullifiers(x)` — both are `LegacyMap<felt, bool>` — without deriving the
preimage, which I cannot do without the channel key or note id. In all four
blocks the singleton count equals the number of struct-runs that would each
carry an existence marker, which is consistent with "all singletons are channel
markers", but consistent is not proven. **Treat an unrepaired eventless block as
capable of hiding a note or a nullifier.** That is the conservative reading and
it is the one the design is built for. (It also means the premise's alarm about
SPEND markers is not refuted — only unconfirmed.)

### 1.3 Prevalence — measured two independent ways

**(a) Uniform sample, ground truth from state.** Ten windows of 20,000 blocks
each, evenly spread across 8,978,970 → 14,157,582 — **200,000 blocks, 3.86% of
history** — every one fetched with `getStateUpdate` and checked for pool storage
diffs, with per-block retry until answered (0 unresolved).

| window | write-active blocks | in mirror | holes |
|---|---|---|---|
| 8,978,970–8,998,969 | 5 | 5 | 0 |
| 9,552,149–9,572,148 | 3 | 3 | 0 |
| 10,125,328–10,145,327 | 1 | 1 | 0 |
| 10,698,507–10,718,506 | 54 | 54 | 0 |
| 11,271,686–11,291,685 | 934 | 934 | 0 |
| 11,844,865–11,864,864 | 100 | 100 | 0 |
| 12,418,044–12,438,043 | 17 | 17 | 0 |
| 12,991,223–13,011,222 | 0 | 0 | 0 |
| 13,564,402–13,584,401 | 13 | 13 | 0 |
| 14,137,581–14,157,580 | 112 | 112 | 0 |
| **total** | **1,239** | **1,239** | **0** |

Zero holes in 1,239 write-active blocks. The rule-of-three 95% upper bound is
3/1,239 = 0.24% of active blocks → **≤ 69 holes across all 28,678 active
blocks**. Taken per-block instead (0 in 200,000) the bound is 3/200,000 →
**≤ 78 blocks history-wide**. The two agree.

**(b) Exhaustive, via a second index (see §2.3).** Not a sample —
**every block of mainnet history**: exactly **5** blocks carry a pool fee
payment and are absent from the mirror, of which **4** have pool storage writes
(the table in §1.2) and 1 (9,477,649) has none — a harmless false positive.

**(c) Cryptographic confirmation that (b) is not missing anything below the
first hole.** Two runs of the shipped tool:

```
$ strk20 verify-root --block 11721847
verify-root OK at block 11721847: storage_root 0x47a36a326502ef91d03f8af4866cc6c7692be56be08f756cbed4203d606c5cc

$ strk20 verify-root --block 11721848
VERIFY-ROOT MISMATCH at block 11721848:
 local 0x47a36a326502ef91d03f8af4866cc6c7692be56be08f756cbed4203d606c5cc
!= chain 0x2af26769b061cdb180d8201fd75c7ef842372729140746f73bb08f69fb7f9cf
```

Because the note/nullifier plane is write-once, `OK at 11,721,847` attests the
correct value of **every pool slot as of that block**: the mirror is provably
complete over 8,978,970 → 11,721,847, i.e. **2,742,878 blocks, 53% of history,
zero holes of any kind** —
including hole classes nobody has thought of. And the first divergence falls on
**exactly** the first candidate the cheap index predicted. The heuristic and the
cryptography agree to the block, over half of history. That agreement is the
empirical basis for the whole design.

**So: H = 4 known real holes in all of mainnet history, with a 95% ceiling
around 70.** Not thousands. This single number is what makes a repair-based
design beat a scan-based one, and it is why the rest of this document argues
what it argues.

---

## 2. Index costs (the other failed section, re-established)

### 2.1 The naive state scan — the real number, not the inherited estimate

The inherited estimate was "hours to days". It is neither: it is **4.9 hours**,
because `getStateUpdate` accepts JSON-RPC **batching**, which nothing in this
project currently uses.

Measured, 200,000 real blocks fetched end to end:

| knob | measurement |
|---|---|
| batch size 100, 1 connection | 61 blocks/s |
| batch 100, 4 / 8 / 12 / 16 workers | 209 / 315 / **295–434** / 295 blocks/s |
| batch 100, 32 workers | rejected (`HTTP 500`, "failed relay, insufficient results") |
| batch size 500 | rejected outright (batch too large) |
| sustained over 200,000 blocks, 12 workers | **294.6 blocks/s**, 679 s |
| payload | **9.1 KB/block** including retry waste (1,819 MB / 200,000) |
| id-level retries (pruned backends) | 16,209 on 200,000 = **8.1%**, concentrated in deep windows |
| HTTP requests | 2,162 for 200,000 blocks |

Extrapolated to the full 5,178,613-block range:

> **4.9 hours wall clock, ~47 GB downloaded, ~56,000 batched HTTP requests.**

That is a real, affordable, one-time cost. It is not the disqualifier the
"hours to days" estimate implied — the honest objection to the flat scan is
elsewhere, and it is §3.3.

`getStateUpdate` also returns `block_hash`, `old_root` and `new_root` alongside
the diff. The current ingest issues a separate `getBlockWithTxHashes` per active
block for the hash; **that call is redundant and can be deleted** — 28,678 calls
saved on a mainnet backfill, for free.

### 2.2 The index scan's cost is a latency tail, not a volume

I could not get a strict single-page pool-event scan of the full range to finish
in under ~25 minutes, and the reason is not window width or event volume. The
**identical** `getEvents` request (pool, blocks 13,900,000–13,999,999,
`chunk_size` 1000), issued ten times in a row:

```
0.52  0.55  0.57  0.57  1.07  1.12  2.65  10.88  17.40  90.0 (timeout)
```

Same query, same URL, same 551 KB answer where it answered at all. Median 0.8 s,
p90 17 s, one hard timeout in ten. A wide window is not intrinsically slow — a
100,000-block window came back in 0.69 s while a 10,000-block window took 55.7 s
in the same minute. It is entirely which backend the aggregator picked.

**Consequence for cost:** a scan of ~500 sequential calls has a near-certain
handful of 15–90 s draws, and that tail — not the work — is what makes the
shipped backfill take 72 minutes. The fix is not a better window predictor; it
is a **fail-fast client**: cancel at ~3 s and re-issue rather than waiting out a
slow backend. Nothing in `rpc.rs` does this today. Projected effect is large
(median-bound scan ≈ 10–15 min) but I did not build it, so treat that figure as
a projection, not a measurement.

This is the same root cause as LIVE-1, LIVE-8 and proof-window §3, showing up in
a fourth guise: **against an aggregating endpoint, latency is a property of the
routing, not of the query.**

### 2.3 A second index nobody looked for: the fee payment

The eventless block emits nothing *from the pool*. But `apply_actions` charges a
fee, and the fee moves as an ordinary ERC-20 transfer from the caller to the
pool's fee collector — on the token contract, with the collector in the **keys**,
which means `getEvents` can filter for it. At block 11,721,848 the pool-calling
transaction emits exactly that:

```
STRK Transfer  from 0x441c3cd2ae71…  to 0x0d79041634625e…(fee_collector)  4.0 STRK
```

and the keyed filter finds it, 3/3 attempts:

```
address = STRK, keys = [[Transfer], [], [fee_collector]]  →  1 event
```

Fee-collector and fee-amount history come from the mirror's own event table
(`FeeCollectorSet` at 9,079,297 → `0x0391b954…`, at 9,477,439 →
`0x0d790416…`; `FeeAmountSet` 4 STRK at 9,079,357, 6 STRK at 12,806,094), so the
index is self-maintaining from data we already ingest.

**Measured over the entire mainnet range**, strict single-page windows only
(LIVE-8 discipline — a continuation token shrinks the window and the answer is
discarded):

> **28,612 distinct blocks, 30,377 fee transfers, 453 seconds.**

Compare against the mirror's 28,678 event-derived blocks:

| set | size | interpretation |
|---|---|---|
| fee-indexed but not in mirror | **5** | 4 real eventless holes + 1 false positive |
| in mirror but not fee-indexed (post-fee era) | **12** | exactly the zero-fee admin blocks — `FeeAmountSet`, `FeeCollectorSet`, the 11,632,886 upgrade, role grants |
| union | 28,683 | covered every one of the 1,239 sampled write-active blocks (event index alone: 1,234/1,239) |

The two indices fail in **disjoint** ways: the event index misses fee-paying
user operations that emit nothing; the fee index misses zero-fee admin
operations that emit events. A block escapes both only if it emits no pool
event *and* pays no fee. None exists in the 3.86% sample, and none exists in the
53% of history that `verify-root` has now proven complete.

Cost of the second index: **7.5 minutes, once**, and one extra `getEvents` per
poll cycle thereafter.

### 2.4 Storage proofs, as an instrument

| measurement | value |
|---|---|
| `getStorageProof` (pool `contracts_proof` only) response | **6.8 KB** |
| attempts to first success, at depths 9.5M / 11.26M / 11.72M / 12.5M / 13.8M / 14.17M | 3, 3, 1, 2, 4, 2 — **mean 2.5** |
| wall clock to a bound proof, serial with retry | 0.4–3.0 s |
| batched 50-in-one-request | works; **20% of batch attempts land on an archive backend** (all-or-nothing per batch), 0.78 s/attempt |
| local MPT root recompute over 139,565 writes | **5.2 s CPU** |
| `strk20 verify-root --block N`, as shipped | **22.8 s wall** (5.2 s CPU + ~17 s of proof retries) |

Two structural notes. Proof batching is all-or-nothing because the whole batch
routes to one backend — retry the batch, do not split it. And the two spellings
of the error-42 message observed in one session ("The node…" and "the node…")
are independent evidence of multiple distinct backend implementations behind the
one URL, which is the root cause this project keeps rediscovering.

### 2.5 Why a root ladder is not a cheaper *index*

Tempting idea: the pool's storage root changes if and only if the pool is
written, so galloping/bisecting on roots enumerates active blocks without
downloading diffs. The arithmetic kills it at mainnet density. Finding `a`
change points in a range of `N` costs ≈ `2a·log₂(N/a)` proofs; with
a = 28,678 and N = 5.18M that is ~430,000 proof successes ≈ 1.1M requests at
2.5 attempts, ~7 GB — and it depends entirely on flaky archive routing.

The break-even against a flat scan, in bytes, is at a gap of ≈ 10 blocks
(`2·log₂(g)·6.8 KB·2.5` vs `g·9.1 KB`). The mirror's **median** gap between
active blocks is 10 and the mean is 180 — so root search is at best marginal as
a bulk index and decisively better only when the thing being searched for is
*sparse*. Which is exactly the repair case, where the density of unknown active
blocks is 4 in 5.18M. **Roots are a scalpel, not a plough.**

---

## 3. The design principle these numbers force

### 3.1 Index soundness is the wrong thing to buy

The instinct after LIVE-8 is to make the index sound. But look at what
"sound" buys against what it costs:

- A flat `getStateUpdate` scan is sound *by construction* — relative to a
  trusted RPC. Against an aggregating endpoint it is 5.18 million independent
  opportunities to be handed a wrong answer, with **no check at all**. That is
  the same trust assumption that produced LIVE-8; a flat scan does not remove
  it, it multiplies it by 5.18 million and removes the pagination that made the
  loss visible.
- The union index is heuristic, but every block it produces is then *closed out*
  by a Pedersen MPT root that the chain itself commits to. A root match at
  block B pins every slot and every value as of B — regardless of how the blocks
  underneath it were found, or whether the index that found them was sound.

So the trustworthy design is not "sound index, no check". It is **"cheap
redundant index, cryptographic check"**. The check is the only artefact in the
system that is self-verifying; the index only has to be good enough that the
check rarely fires.

Measured, on this mirror, the union index is good enough that the check fired
**four times in five million blocks**.

### 3.2 What makes completeness provable

Write-once slots — mostly. This mirror holds **139,565 writes over 135,313
distinct slots, 96.9% of writes being first writes**, matching the earlier
measurement. The exceptions are the handful of mutable admin slots
(`fee_amount`, `fee_collector`, `auditor_public_key`, …), not note or nullifier
slots. Because the note/nullifier plane is genuinely write-once:

> **`verify-root` MATCH at block B is a proof that the mirror holds the correct
> value of every pool slot as of B** — and, because the note/nullifier plane is
> write-once, that it is missing no note and no nullifier written at or below B.

Not evidence — a proof, modulo the chain-binding caveat in §6.4. It cannot be
obtained from any events-based audit, and `audit-coverage` reporting "complete"
while `verify-root` mismatches is not a contradiction: they check different
planes (§6.2).

The 3.1% of writes that overwrite a mutable admin slot do not weaken this. The
root is a commitment to the slot→**value** map as of B, so a mirror that missed
an overwrite has the wrong value and the root still catches it. Write-once is
what makes a match at B attest everything *below* B; it is not what makes the
match meaningful at B.

### 3.3 What makes a hole *detectable and locatable*, cheaply

The same root, used as a search predicate. The mirror's local root is a step
function: constant on `[bᵢ, bᵢ₊₁-1]` for consecutive known-active blocks. Since
a divergence is permanent once it starts, the predicate "local root == chain
root at bᵢ" is monotone in i, so:

> Binary search over the **index** of active blocks — not over block numbers —
> finds the first divergence in **⌈log₂ 28,678⌉ = 15** probes, and the answer is
> an *interval between two known-active blocks*, which is a median of 10 blocks
> wide (p90 181, p99 3,314).

Then flat-scan that interval. Median 10 `getStateUpdate` calls, p99 3,314 —
sub-second either way at 295 blocks/s.

Searching the index rather than the block number is what makes this cheap: 15
probes instead of 22, and it lands directly on a small scannable gap instead of
a single block, so clustered holes come out together.

---

## 4. Design (a): sync from scratch, no holes, as fast as possible

Four phases. Phases 1–3 are the fast heuristic; phase 4 is what makes it
correct, and nothing is published before phase 4 says MATCH.

### Phase 1 — pool-event index
`getEvents(address = pool)` over the full range, strict single-page windows: a
response carrying a continuation token is **discarded**, the window is halved,
and the range is retried. Never trust a continuation token across an
aggregating endpoint (LIVE-8). This phase also yields `FeeCollectorSet` /
`FeeAmountSet`, which phase 2 needs — so it runs first.

### Phase 2 — fee index
For each fee-collector era learned in phase 1, `getEvents(address = fee_token,
keys = [[Transfer], [], [collector]])`, same single-page discipline.
**Measured: 453 s, 28,612 blocks.**
Candidate set = phase 1 ∪ phase 2.

### Phase 3 — batched block fetch
`getStateUpdate` for every candidate block, **batch 100, 12 workers**, per-id
retry on error (8.1% of ids need one at depth). Take `block_hash` from the same
response; do not issue `getBlockWithTxHashes`.
**28,683 candidates at 295 blocks/s ≈ 97 s of RPC.** The shipped ingest issues
these one block at a time, two serial calls each (57,356 calls) — batching and
dropping the redundant hash call is worth minutes here, though not the bulk of
the backfill's runtime, which is phase 1 (§2.2).

### Phase 4 — closure loop (this is the part that makes it sound)

```
build MPT once; record local_root at every candidate block   (5.2 s)
loop:
    p ← probe(frontier)                     # 1 bound proof
    if p == MATCH:  break                   # mirror provably complete ≤ frontier
    if p == UNAVAILABLE: back off, retry; NEVER report OK or MISMATCH
    i ← binary search over active-block index for first mismatch   (15 probes)
    gap ← (b[i-1], b[i]]                    # median 10 blocks, p99 3,314
    flat-scan gap with batched getStateUpdate; ingest every block with pool writes
    recompute local roots                   (5.2 s, or incremental)
```

`probe(B)` = `getStorageProof(B, [], [pool], [])`, retried until a backend
answers (mean 2.5 attempts), then **`global_roots.block_hash` compared against
`getBlockWithTxHashes(B).block_hash`** before the root is believed. That bind is
mandatory: the endpoint is an anonymous load-balanced pool.

**Cost per iteration:** 15 probes (~15 s) + gap scan (<1 s) + recompute (5.2 s)
≈ **21 s per hole**.

### Total, mainnet

| phase | today | with the §8 fixes |
|---|---|---|
| 1. pool-event index | **~72 min** (inherited: the shipped full backfill, 4,302 s, dominated by the §2.2 latency tail; my own strict single-page probes did not finish inside 25 min either) | ~10–15 min *(projected — fail-fast retry, not built)* |
| 2. fee index | **7.5 min measured** | ~5 min |
| 3. candidate fetch | minutes (57,356 serial calls) | **~2 min measured** (batched) |
| 4. closure, H = 4 | — (does not exist) | **~1.5 min** |
| 4. closure, H = 70 (95% ceiling) | — | ~25 min |
| **total** | **~80 min, ~1 GB** | **~20–25 min, ~1 GB** |

Against the flat sound scan: **4.9 h, 47 GB, and no cryptographic check at the
end.** Even today, before any of the §8 work, the hybrid is ~3.7× faster and
~45× lighter — and unlike the flat scan it finishes with a proof. With the
fail-fast fix it is ~12× faster.

The honest comparison is narrower than it first looks, and I want to be explicit
about that: **4.9 hours for a from-scratch flat scan is affordable.** If the
only question were sync-from-scratch, "just scan the state plane" would be a
defensible answer. It is not the only question — §5 is — and the flat scan does
nothing for §5, because a mirror that is already 5 million blocks deep cannot
afford 4.9 hours and 47 GB every time an epoch cut mismatches.

**Fallback, explicitly retained.** If phase 4 does not converge — the loop keeps
finding holes, or the candidate index has decayed (§6.3) — abandon the index and
flat-scan the range. It costs 4.9 h and it always works. Keep it behind
`--full-state-scan` and never make it the default; it is the recovery path, not
the design.

---

## 5. Design (b): find and repair holes in an existing mirror

### 5.0 What we do today does not converge — and the log proves it

This is the strongest argument in the document and it costs nothing to verify:
`data/mainnet/catchup3.log`, from the run that produced the current mirror.

On `verify-root` MISMATCH the indexer blindly rescans a *fixed recent window*
anchored at the current epoch floor:

| round | rescanned range | wall clock | recovered | outcome |
|---|---|---|---|---|
| 1 | 14,140,000 → 14,154,790 | ~28 min | 92 blocks | still MISMATCH |
| 2 | 14,140,000 → 14,156,581 | ~48 min | 98 blocks | still MISMATCH |
| 3 | 14,150,000 → 14,158,228 | ~23 min | 28 blocks | still MISMATCH, `epoch cutting halted` |
| 4 | 14,150,000 → 14,159,049 | ~26 min | 28 blocks | still MISMATCH, `epoch cutting halted` |

Two hours of rescanning, 246 blocks "recovered", zero progress, and the run
ended with epoch cutting halted — the feed stopped publishing.

It could not have worked. The actual first hole is at **11,721,848**, which is
**2.43 million blocks below the bottom of every window it rescanned.** The
recovery strategy is not merely slow; it is **non-convergent by construction**
whenever the divergence sits below the rescan window, and every hole we have
found does. The error message even says so out loud — "Recover with a full-range
rescan of recent epochs" — and *recent* is the bug.

The closure loop replaces those two fruitless hours with **15 storage proofs and
about fifteen seconds**, and lands on 11,721,848 regardless of how deep it is,
because binary search over the active-block index does not care where the hole
lives. That is the entire case for this design, and it is already sitting in the
project's own logs.

### 5.1 The loop

Same closure loop as §4, entered from a cheaper front door, ordered by cost.

**Step 0 — free.** `verify-root` at the frontier (inside the proof window,
≤ frontier). One bound proof. **MATCH → the mirror is complete, stop.** This is
the whole audit in one call and it is the reason the design is affordable: the
common case costs 6.8 KB.

**Step 1 — 7.5 min.** On MISMATCH, recompute the fee index and diff against the
mirror's block set. **On this mirror that produced 5 candidates, 4 of them
real, with no false negatives below the first hole** (proven by the OK at
11,721,847). Repair those blocks first; they are almost certainly the answer.
Re-run step 0.

**Step 2 — 21 s per remaining hole.** Still MISMATCH → binary search over the
active-block index (15 probes), scan the resulting gap, ingest, recompute,
repeat. This step needs no heuristic at all and will find a hole of *any*
class, including ones this document has not imagined.

**Step 3 — local, no RPC.** Any repair below the epoch floor rewrites published
history: `strk20 recut-epochs --from-block B` re-cuts that epoch and every epoch
above it. Consumers see a chain-hash change and must be told; that is a feed
event, not an indexer detail.

**Implementation debt this exposes.** `verify-root --block N` costs 22.8 s today
(5.2 s recomputing the whole MPT + ~17 s of serial proof retries). As a *probe*
inside a 15-step search that is 6 minutes per hole instead of 15 seconds. Two
changes fix it: precompute local roots for all active blocks in one pass and
cache them, and fetch proofs concurrently. Neither is deep.

---

## 6. What this design does NOT protect against

Stated plainly, because every one of these has already bitten this project once
in a form nobody predicted.

**6.1 It does not make the index sound, and does not claim to.** The union
index is two heuristics. Their union was complete over the 53% of history that
is cryptographically proven and over a 3.86% uniform sample of the rest. That is
evidence, not a theorem. The design is safe because a heuristic miss is
*detected*, not because it cannot happen.

**6.2 A root match proves storage completeness, not event completeness.** The
two audits are complementary and neither subsumes the other:

| audit | proves | blind to |
|---|---|---|
| `verify-root` | every pool storage write ≤ B is mirrored, with correct values | missing/short **events** for blocks whose storage we did have |
| `audit-coverage` | our event counts equal the chain's, per block | any block with writes and no events — by construction |

A block ingested via the *fee* index whose events we then failed to page would
pass `verify-root` while the `events` table is short — breaking typed stats,
`EncNoteCreated` payloads and the explorer without breaking discovery. Both
audits must keep running. The premise's observation that "an events-based audit
can never find this class of hole" is exactly right and the converse is equally
true.

**6.3 The fee index depends on contract policy we do not control.** It is blind
before block 9,079,357 (fee amount unset — a ~100k-block window, already cleared
cryptographically by the OK at 11,721,847). It would go blind again on a
zero-fee path, a fee denominated in a different token, or a fee-collector
rotation we miss — and the collector is itself learned from a pool *event*, so
a missed event propagates into a missed fee index. There is no third heuristic
worth adding. The mitigation is that `verify-root` notices within one epoch,
which is an argument for running it continuously rather than for trusting the
index more.

**6.4 It does not protect against a dishonest endpoint.** Every proof is bound
to the chain by checking `global_roots.block_hash` against
`getBlockWithTxHashes` — that defeats a load-balanced pool serving a proof for
the *wrong* block, which is the failure we can actually demonstrate. It does not
defeat an endpoint serving an internally consistent *fake* chain, because we
never check a block hash against L1. Closing that needs a second independent
provider or an L1 checkpoint. We have neither today, and no amount of ingest
design substitutes for it.

**6.5 A MATCH inside the proof window is not a permanent attestation.** Proofs
answer at any depth on lava with retry (proof-window §1), so verification at a
finalised block is available — but the fast path verifies near head, and a
near-head block can reorg. Finality remains the epoch floor's job. A MATCH at an
unfinalised block should be recorded as such in `anchors.ndjson`, not treated as
final.

**6.6 UNAVAILABLE is not MISMATCH.** A capability gap (error 42 that never
retries through, a failover onto publicnode which does not implement proofs at
all) must report UNAVAILABLE and leave completeness *unknown*. Reporting it as a
mismatch is LIVE-6; reporting it as OK would be worse. Three-valued or nothing.

**6.7 The prevalence bound is not a correctness claim.** H = 4 measured, ≤ 70 at
95%. Those numbers justify the *cost model* — they are why repair beats scanning.
They are not why the mirror is correct. The mirror is correct because a root
matched.

**6.8 The eventless writes are probably channel data, but I did not prove it.**
§1.2. A singleton `0x1` could be a note or a nullifier. Nothing in this design
depends on which it is, and nothing downstream should either.

**6.9 It covers one contract.** All of it — the write-once argument, the root
check, the fee index — is specific to the pool. A helper or anonymizer contract
whose storage we ever need gets none of this for free.

---

## 7. What runs when

| cadence | what | cost |
|---|---|---|
| every poll cycle | pool-event index + fee index on the new tail; batched `getStateUpdate` for candidates | one extra `getEvents` per cycle |
| every epoch cut | `verify-root` at a block inside the proof window and ≤ frontier; append `(block, block_hash, storage_root, class)` to `anchors.ndjson` | **1 bound proof, 6.8 KB** |
| on MISMATCH | closure loop §4 phase 4 | ~21 s per hole |
| on demand / weekly | full-history fee-index diff (§5.1 step 1) | 7.5 min |
| last resort only | full-range flat state scan | 4.9 h, 47 GB |
| **never** | blind rescan of a recent window on MISMATCH | 25–48 min per round, does not converge (§5.0) |

The per-epoch proof is the load-bearing habit, and it is nearly free. It keeps
the "last proven-complete block" marching along behind the frontier, which means
any future hole is bounded to the blocks since the last MATCH — one epoch, not
five million. The 15-probe search collapses to ~8, and localisation becomes
instant. **A completeness check that runs once is forensics; the same check run
every epoch is an invariant.** LIVE-4 is the whole cautionary tale: the check
existed, was correct, and had never once run.

---

## 8. Recommendations, in the order I would do them

1. **Replace the recent-window rescan with the closure loop** (§4 phase 4), as
   `strk20 audit-root --repair`, searching the active-block *index*. This is the
   only item that fixes a system which today cannot converge at all (§5.0).
   Everything else is speed.
2. **Add the fee index.** 453 s of measurement says it catches every known
   eventless block including the one that motivated this council, and it would
   have found all four without a single proof. Cheapest correctness-per-line in
   the project.
3. **Verify at every epoch cut and record the anchor.** Turns the existing
   `verify-root` from forensics into an invariant, at 6.8 KB per epoch, and
   bounds every future search to one epoch.
4. **Make `verify-root` usable as a probe** — cache local roots for all active
   blocks in one pass, fetch proofs concurrently. 22.8 s → ~1 s turns a 6-minute
   search into a 15-second one.
5. **Fail fast on slow RPC.** Cancel a `getEvents` at ~3 s and re-issue rather
   than waiting out a 90 s backend (§2.2). This is the single biggest lever on
   backfill wall clock and it is a timeout constant.
6. **Batch `getStateUpdate` and drop the redundant `getBlockWithTxHashes`.**
   295 blocks/s measured, and 28,678 calls deleted outright since the state
   update already carries `block_hash`.
7. **Keep the flat scan** behind `--full-state-scan` as the recovery path.
   4.9 h, always works, never the default.
8. **Repair the four known holes** (11,721,848; 11,721,893; 14,100,846;
   14,101,246), re-cut epochs from 11,721,848, and confirm `verify-root` at the
   frontier goes MATCH. That is the outstanding prediction this document makes,
   and it is falsifiable in one command. If it does not go MATCH, a fifth hole
   exists that the fee index cannot see, and §6.1 is the section to re-read.

---

## Appendix — reproduction

Every measurement above came from ad-hoc probes in the session scratchpad
against `https://rpc.starknet.lava.build` and `data/mainnet/strk20.db`, on
2026-09-01, mainnet head ≈ 14,168,800. The load-bearing ones:

- eventless block, receipts + trace: `getBlockWithReceipts` /
  `traceBlockTransactions` at 11,721,848; selector matched against
  `docs/research/data/live-pool-abi.json`.
- prevalence sample: 10 × 20,000-block `getStateUpdate` sweep, batch 100,
  12 workers, per-id retry, 679 s.
- fee index: `getEvents(STRK, [[Transfer],[],[collector]])`, single-page
  windows, full range, 453 s.
- closure evidence: `strk20 verify-root --block 11721847` (OK) and
  `--block 11721848` (MISMATCH).
- `getEvents` latency tail: the same request issued ten times via `curl`
  (`%{time_total}`): 0.52 / 0.55 / 0.57 / 0.57 / 1.07 / 1.12 / 2.65 / 10.88 /
  17.40 / timeout at 90.
- non-convergent recovery: `data/mainnet/catchup3.log`, four `rescanning range`
  rounds, all anchored at 14,140,000+.

One thing I deliberately did not do: repair the four holes. That writes to the
user's live mainnet mirror, and no measurement in this document required it.
Recommendation 8 is left as a stated, falsifiable prediction instead.
