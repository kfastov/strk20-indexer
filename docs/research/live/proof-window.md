# Storage-proof availability on Starknet RPC — measured, 2026-08-31

Why this document exists: the project's central integrity mechanism
(`verify-root`, and now the snapshot anchor in
[consumer-path.md](../../spec/consumer-path.md) §1.3/§1.4) assumes
`starknet_getStorageProof` can be asked about a block of our choosing. Running
the real binary against mainnet showed it cannot. These are the measurements
that must constrain the design.

## The window is ~1024 blocks, not ~25-55k

Bisection against `https://rpc.starknet.lava.build`, mainnet pool
`0x0403…812a`, head 14,151,406 at the time of the run:

| block | result |
|---|---|
| head−0, −1, −2, −5, −10, −50, −200 | OK |
| head−600, −800, −900, −950, −962, −968 | OK |
| head−975, −1000, −5000 | error 42 |

Boundary: **OK at head−968, error 42 at head−975** — a sliding window of
~1024 blocks (pathfinder's default trie retention). The
implementation-notes.md §5 figure of "~25–55k blocks on lava" is wrong and is
corrected here.

## No public provider serves deep proofs

Same request at block 14,000,000 and 14,140,000:

| endpoint | verdict |
|---|---|
| `rpc.starknet.lava.build` | error 42 at both depths; OK only inside the ~1024-block window |
| `starknet-rpc.publicnode.com` | error 42 at **every** height including head — the method is not implemented at all |
| `starknet.drpc.org` | `-32601 method is not available` |
| `starknet-mainnet.public.blastapi.io` | service discontinued (redirects to Alchemy) |
| `free-rpc.nethermind.io/mainnet-juno` | no response to any method from here |

Sepolia is the same story from the other direction: proofs answer **only at the
exact current head**, and a block becomes unprovable as soon as the next one
lands (~1.7 s), per [sepolia-volume.md](sepolia-volume.md).

## What this forces

1. **Any block-of-our-choosing proof design is dead.** That includes
   verifying at `min(l1_accepted, frontier)` (l1 lags head by ~5,000 blocks on
   mainnet — measured 14,128,517 vs 14,123,420) and the per-epoch or
   per-snapshot anchor at an epoch's end block (thousands of blocks old at cut
   time). Live proof: 0 of 515 epochs in a completed mainnet backfill carry an
   anchor, and `verify-root` returns error 42 every time.
2. **Proof capture must be head-driven and opportunistic.** The only block that
   is reliably provable is the one that just landed. So the indexer must
   capture (block, block_hash, storage_root) *as it follows the head*, into an
   append-only log, and verify its mirror at that moment.
3. **This is not a weakening.** Pool slots are write-once (measured: 134,879
   distinct slots across 139,131 writes, 96.9% first-writes), so a root match at
   block B attests every write below B. A head-side check is therefore at least
   as strong for mirror-completeness as an l1-side one; finality is a separate
   concern, already handled by cutting epochs only below `l1_accepted`.
4. **Endpoint capability is not uniform and must be tracked.** publicnode does
   not implement proofs at any height, so failing over to it turns every check
   into a false alarm. A capability gap must never be reported as a mirror
   mismatch.
5. **The stronger check remains available to whoever wants it**: an operator
   running their own archive node with tries retained can verify at any block.
   That is an opt-in configuration, not something a public-provider deployment
   can assume.
