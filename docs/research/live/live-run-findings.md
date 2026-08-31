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

## Session 3: the client against the real mainnet feed — open question #1 answered

`strk20 run` served the completed 515-epoch feed; `strk20-sync` folded it with a
throwaway key (no notes expected — the point is the fold, not the discovery).

| measurement | value |
|---|---|
| cold start over HTTP (fetch 16 MB + verify hash chain + fold 515 epochs + discovery walk) | **5.97 s**, peak RSS 31 MB |
| cold start from a local dir (no HTTP at all) | **6.18 s** |
| warm re-sync (mirror already folded) | **0.03 s** |
| client mirror on disk | 60 MB SQLite |
| pool storage: writes / distinct slots | 139,131 / 134,879 (write-once confirmed: 96.9% of writes are first writes) |

The two cold numbers being equal says the cost is **entirely the fold** (2.2 s user
+ 2.0 s sys), not the network. This settles design-notes §9 open question 1 in the
direction the note called "mandatory": at 6 s native — and WASM will be slower, not
faster — **the browser client cannot re-fold history on every page load**, so the
persisted folded mirror is required, not an optimization. It also raises the value
of roadmap item 1 (snapshots) from "nice cold-start win" to "the mechanism that
makes a browser client viable at all": a snapshot ships 134,879 current slot values
instead of replaying 139,131 writes across 515 epochs, and the 3.7× expansion from
feed (16 MB) to client mirror (60 MB) is what IndexedDB would otherwise have to hold.

Explorer stats over the full real history (`/v1/stats`), for the record:
**31,077 notes** (the anonymity set), 2,628 registrations, 25,666 spends, 16,199
deposits across 31 tokens, 40,204 withdrawals across 34 tokens.

## Session 4: mirror caught up to head — and LIVE-7

The mirror was brought to 14,151,973 with the head at 14,151,989 (16 blocks
behind), i.e. inside the proof window for the first time, so the root check
could finally be attempted for real. It failed for a *new* reason:

```
$ strk20 verify-root --block 14151973
Error: rpc error from starknet_getStorageProof:
{"code":-32602,"data":{"reason":"expected array for \"class_hashes\""},"message":"Invalid params"}
```

**LIVE-7 (defect, high): `get_storage_proof` sends `null` for the optional
array parameters.** `crates/indexerd/src/rpc.rs:277` builds positional params
`[block_id, null, [contract], null]`. Some backends accept that; others demand
real arrays. Confirmed by direct probe at the same block and endpoint:

| params | result |
|---|---|
| `[{block_number}, null, [pool], null]` | `-32602 expected array for "class_hashes"` |
| `[{block_number}, [], [pool], []]` | **OK** |

This is the third defect in a row caused by the same root cause as LIVE-1/6:
we treated RPC endpoints as one uniform, forgiving implementation. They differ
in retention, in which methods exist, and in strictness about optional params.

Ground truth captured for the regression test —
`fixtures/proof_mainnet_14151973.json`, the full live response at block
14,151,973: pool `storage_root = 0x25e47f354ce696498d59e80ab4eb07483d4e737647a7b4832959a170ae8db09`,
`block_hash = 0x46a19ce7fed109f163453d914dc174f394e4e29270dded25d1d84f78c6b8aaa`,
class `0x67dddd89…76b554d`. Once LIVE-4/LIVE-7 are fixed, our mirror must
recompute exactly that root from its own 134,879 slots — the strongest
end-to-end statement the project can make about a real mirror of real history.

## Session 5: end-to-end on Sepolia, with a note we created ourselves

We minted our own note on the Sepolia pool (see
[sepolia-shield-run.md](sepolia-shield-run.md)): tx
`0x701e056354f9e0e17e86b7d63d4403cb46e239e7061806e9f5e02ff47d65f49`, block
**14,339,115**, 3 STRK, registration + deposit + note creation in one
`apply_actions`. Then the whole read path was run against it.

**Sepolia backfill:** completed in a single process run (no pruned-range
aborts — publicnode Sepolia serves the full range), 19,030 events across 4,455
pool-active blocks, 606 epochs, genesis 8,271,125 → head 14,340,535. Our
transaction's block carries exactly the three expected events (`ViewingKeySet`,
`Deposit`, `EncNoteCreated`).

**Keyless discovery found our note, and only our note:**

```json
{"token":"0x4718f5a0…c938d","index":0,
 "note_id":"0xce526b286fed962b9e3942771c5e519c69b8677dc24136ae380ba523a067ff",
 "nullifier":"0x6f3769425be9f731773213fb6917264bfda572b2eeda180513d5cf5cbb71662",
 "amount":"3000000000000000000","block_number":14339115,"spent":false}
```

in **1.19 s**. The `note_id` and amount match what the SDK reported at mint
time, independently derived — the indexer never saw the SDK's output, only the
chain.

**The no-key claim, proven on live traffic.** A recording proxy
(`data/sepolia/wireproxy.py`) sat between client and server for two syncs:
wallet A = our real address + viewing key (finds the note), wallet B = an
unrelated address + key (finds nothing).

| | result |
|---|---|
| viewing key in wallet-A traffic | **not found** in any of 13 encodings (minimal hex, padded, decimal, upper/lower, 0x-prefixed, raw BE/LE bytes) |
| address in wallet-A traffic | **not found** in any of 13 encodings |
| request streams A vs B | **byte-identical**: 609 requests, 64,509 bytes each |
| detector self-test | the same scanner **does** find the key when planted in a synthetic body |

The requests are exactly what the design promises: `genesis.json`,
`manifest.json`, and a sequence of `epochs/{n}.strk20e.zst` — public static
files, in the same order, for both wallets.

## Session 6: an unannounced contract upgrade, live, mid-run

While the Sepolia server was running, the pool was **upgraded on chain** at
block **14,339,893** — 778 blocks after our own note landed, and hours after
the morning's research recorded `0x56ab118a…` as current. New class:
`0x7e2bbd7ccc1e68b2695caef70aeb2a3be6cd017b5d5159278ba08f2d8de33f`, a sixth
Sepolia class nobody told us about.

The system behaved exactly as acceptance leg (i) simulates synthetically:

- `class_history` recorded the new class at its block automatically;
- typed decoding switched to `decode_state=degraded`, `/health` → `DEGRADED`,
  with a WARN naming the unknown class;
- **raw ingest and the feed continued uninterrupted** — the epochs kept cutting
  and the keyless discovery above ran against this very feed and still found
  our note, because discovery reads pool storage, not decoded event types.

A synthetic test asserting this is worth something; the same thing happening
unannounced, in production, during the run, is worth more. Recovery is
`--allow-class 0x7e2bbd7c…` once the new ABI is diffed.

## Session 7: spend + post-upgrade note, through our own pipeline

A second transaction we made (`0x3d253f8a…`, block **14,340,785**, a private
self-transfer) spent the first note and created a new one — under the *new*
class, 892 blocks after the upgrade. Details in
[sepolia-shield-run.md](sepolia-shield-run.md) Run 2.

With `--allow-class 0x7e2bbd7c…` the server returned to `status: OK`,
`decode_state: ok` — the documented recovery path, exercised against a real
upgrade rather than a synthetic one. Then one incremental client sync:

```
note 0xce526b286fed962b9e3942771c5e519c69b8677dc24136ae380ba523a067ff  3.0 STRK  block 14339115  spent=True
note 0x3aa1d44c8920593d29297e509a26445e2bc2a6389fa5e8d59fc2e5944553ecd  3.0 STRK  block 14340785  spent=False
balances: 3.0 STRK
```

Three properties confirmed in one shot, all on data we created:

1. **Spent-state flips on the right note.** The nullifier our client predicted
   (`0x06f3769425be9f731773213fb6917264bfda572b2eeda180513d5cf5cbb71662`)
   appeared verbatim in the on-chain `NoteUsed` event's `keys[1]` — an
   independent confirmation of the nullifier formula, from the contract itself.
2. **Discovery works across the upgrade.** The new note was written by the new
   class and is found by the same unmodified engine over the same slot
   derivation. The ABI diff said the events we consume are identical; this says
   the *storage layout* is too, which no ABI could have told us.
3. **The balance counts only the unspent note** — 3.0 STRK, not 6.0.

**Indexer semantics worth pinning in a test:** a spent note's storage slot is
**not cleared** — `get_note` still returns its packed value after the spend.
Spentness lives only in `nullifiers` / `NoteUsed`. Anything inferring "unspent"
from "slot is populated" would be wrong. Our engine gets this right because it
reads nullifiers, but nothing in the suite currently pins it against a *live*
spend.

## Session 8: the client's own-RPC proof check works live — and why

`strk20-sync verify` against a public Sepolia RPC, on the two notes above:

```
all_ok: true
pool_class_hash: 0x7e2bbd7c…de33f
storage_root:    0x2c7cdae493453a87660db1d2914fd17867eb81798d10613ba6887906c41aaa1
note 0xce526b28…  note: proven   spent_state: spent-proven
note 0x3aa1d44c…  note: proven   spent_state: unspent-proven
```

Both notes proven against Starknet state roots, both spent-states proven, with
the indexer entirely out of the trust path.

This is worth stating because it looks like it contradicts LIVE-4, and does
not — it sharpens it. **The client never needs a historical proof.** Pool slots
are write-once, so a note created at block 14,339,115 still occupies its slot at
head, and its nullifier slot is present-or-absent at head; checking *current*
state answers both questions. Only the server's `verify-root` asked about an
old block, and that is exactly the query the ~1024-block window forbids.

The same observation is what makes the §11 amendment sound rather than a
climbdown: write-once means a root match at a recent block attests everything
below it, so head-side verification is not a weaker substitute for l1-side
verification — it is the same statement, obtained from a query providers will
actually answer.

## Session 9: verify-root works — and immediately finds real data loss

With LIVE-4 (window-aware block choice) and LIVE-7 (empty-array params) fixed,
`verify-root` ran against mainnet for the first time. It did not print OK:

```
VERIFY-ROOT MISMATCH at block 14154790:
local 0x58a49c27f1509a3542fc9e53c4245d10e807f2e6b4cc6577bfc3468e254d273
 != chain 0x31eeb9485ae623c4b65d3cf05f201e0aacf0c34de3349c73fbbced07ff5174c
```

Reproducible, stable across runs, and it survived the automatic rescan. So the
mirror really was wrong — the check earned its keep on its first working run.

**Narrowing it down** (each step ruling out a hypothesis):

| test | result |
|---|---|
| zero-valued slots leaking into the trie | 5 exist, all correctly filtered by `full_slot_set_as_of` — not the cause |
| per-block write completeness, 150 sampled ingested blocks | 150/150 match the chain's `storage_entries` count exactly |
| slot values, 120 sampled slots via `getStorageAt` at the verified block | 120/120 identical |
| **block coverage: full re-count of pool events from the chain** | **chain 120,135 events in 28,655 blocks; ours 119,646 in 28,532 — 139 blocks and 489 events missing** |

Spot-checked, the loss is total for the affected blocks: block **11,263,135**
has 6 pool events and 4 pool storage writes on chain, and our mirror has **no
row for it in any table**. Same for 11,279,543. The missing blocks come in
contiguous clusters (11,263,874–11,263,880; 11,265,889–11,265,893;
11,279,543–11,279,756; …).

### LIVE-8 (defect, CRITICAL): continuation-token paging is unsound across an aggregating endpoint

The scan loop is internally correct — it accumulates all pages for a range into
a `BTreeMap` and restarts if *our* failover fires. The flaw is the assumption
underneath it: that a continuation token means the same thing on the next
request. **It does not.** `rpc.starknet.lava.build` is an aggregator; successive
calls to the same URL reach different backend nodes (already visible in this
project as nondeterministic pruned-history errors and a `specVersion` that
answers 0.8.1 or 0.10.x depending on the call). A continuation token is
node-local state. Handed to a different node, it does not error — it resumes
from somewhere else, and the events in between are silently dropped.

Demonstrated directly, same range `11,260,000–11,285,000`, same endpoint:

| method | pages | distinct blocks found |
|---|---|---|
| paged, `chunk_size=1000` | 13 | **2,628** |
| paged, `chunk_size=200` | 62 | **2,608** — 19 blocks fewer |

Two paginated scans of an identical range disagree, with no error raised in
either. More pages means more chances to be routed elsewhere, so loss scales
with page count — which is exactly why a 5.2M-block backfill with 150+ pages,
restarted 27 times, lost 139 blocks.

**Why nothing caught it earlier:** the fixture RPC in the test suite is a single
honest process, so its tokens are always valid — the synthetic harness cannot
express this failure at all. The one mechanism that *could* catch it is
`verify-root`, and LIVE-4 had prevented it from ever running.

**Fix direction:** stop trusting cross-request pagination. Subdivide the scan
range until every window is answered in a **single page with no continuation
token**, and take the union. A single response carries no cross-request state,
so it is sound under any routing. Cost is bounded because pool-active blocks are
sparse. `verify-root` stays the backstop that proves the result, and the fixture
RPC needs a fault mode that returns a *plausible but wrong* page for a foreign
token, so the suite can express this class of failure at all.

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
