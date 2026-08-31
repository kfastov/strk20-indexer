# Storage-proof availability on Starknet RPC — measured, 2026-08-31

> **CORRECTED 2026-08-31 (same day).** The first version of this document
> concluded that historical storage proofs are unobtainable and that the
> project must abandon block-of-our-choosing verification. **That conclusion
> was wrong**, and it was wrong because of a measurement error described in
> §3. Deep proofs *are* available, back to genesis, from an endpoint we
> already use. The original design stands. The corrected findings follow; the
> retracted reasoning is kept in §3 because the mistake is instructive.

## 1. What is actually true

`starknet_getStorageProof` on `https://rpc.starknet.lava.build` answers for
**any historical block, back to genesis** — but only on the subset of backends
in its pool that run archive tries, so a single attempt fails often and a
retry succeeds. Measured against the mainnet pool
`0x0403…812a`, retrying until two successes:

| block | blocks behind head | successes / attempts | proof `block_hash` vs real header |
|---|---|---|---|
| 9,000,000 | ~5,150,000 | 2 / 4 | **match** |
| 11,263,135 | ~2,890,000 | 2 / 10 | **match** |
| 12,000,000 | ~2,150,000 | 2 / 4 | **match** |
| 14,000,000 | ~150,000 | 2 / 5 | **match** |
| 14,140,000 | ~15,000 | 2 / 5 | **match** |

Every returned proof carries a `global_roots.block_hash` equal to the block's
real hash from `starknet_getBlockWithTxHashes`, so these are genuine proofs
for the requested block, not a near-head substitute. Success rate is roughly
one in two to one in five attempts; there is no depth at which it stops.

**Consequence: verifying at a block of our choosing — `l1_accepted`, an epoch
boundary, a snapshot basis — is possible.** It needs a bounded retry loop on
error 42, exactly like the pruned-history retry that LIVE-1 already required
for `getEvents`.

## 2. Endpoint capability still differs, and still matters

| endpoint | verdict |
|---|---|
| `rpc.starknet.lava.build` | archive to genesis on ~a fifth to a half of attempts; **retry required** |
| `starknet-rpc.publicnode.com` | error 42 at **every** height — does not implement proofs at all |
| `starknet.drpc.org` | `-32601 method is not available` |
| `starknet-mainnet.public.blastapi.io` | discontinued |
| Alchemy (open demo key) | works from ~13.2M; error 42 below ~13.15M |
| Juno-backed providers | head block only, by design — Juno's source says "We do not support historical storage proofs for now" |

So LIVE-6 stands unchanged: a capability gap must never be reported as a
mirror mismatch, and failover must not move a proof request onto an endpoint
that cannot serve it. What changes is that a *retry on the same endpoint* is
the first thing to try, not a last resort.

Because the pool is anonymous and load-balanced, every accepted proof must be
bound to the chain independently: check `global_roots.block_hash` against
`starknet_getBlockWithTxHashes(block).block_hash` before believing the root.
That check is cheap and was verified to hold at all five depths above.

## 3. The measurement error, kept as a lesson

The retracted conclusion came from a bisection: proofs answered OK at head−968
and error 42 at head−975, so a ~1024-block sliding window was inferred (and
attributed to pathfinder's default trie retention, which is real — the default
really is 20 blocks kept, and only `--storage.state-tries=archive` at database
creation changes it).

The flaw: **a bisection assumes a deterministic predicate.** Lava is an
aggregator; "does this block have a proof" is a property of whichever backend
answered, not of the block. Every "error 42" was read as evidence about depth
when it was evidence about routing. One retry at head−975 would have falsified
the whole conclusion.

This is the *same* root cause as LIVE-1 (a pruned-history error that succeeds
on retry) and LIVE-8 (continuation tokens routed to a foreign backend). The
lesson had already been written down in this project and was still not applied
to the next measurement: **against an aggregating endpoint, never conclude
anything from a single failed request.**

## 4. What the earlier version got right, and keeps

- Pool slots are write-once (134,879 distinct slots across 139,131 writes), so
  a root match at block B attests every write at or below B. Still true, still
  useful — it is why a single verification point is worth so much.
- `verify-root` must be three-valued: `MATCH` / `MISMATCH` / `UNAVAILABLE`.
  Conflating "we could not check" with "the mirror is wrong" is what made a
  capability-poor endpoint look like corruption. Still required.
- An append-only anchors log is cheap and worth keeping as a running audit
  trail. It is no longer a *replacement* for per-epoch anchors — those are
  obtainable — but a complement.
- Self-hosting an archive node remains the strict-SLA option: pathfinder with
  `--storage.state-tries=archive`, chosen at DB creation and immutable after,
  requiring a full genesis resync (the published snapshot is trie-pruned and
  cannot be upgraded to archive) at an inferred ~2 TB.
