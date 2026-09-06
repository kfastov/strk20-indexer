# Implementation status and next work

Updated 2026-09-06. Work resumed on the user's instruction after the SDK and
demo stages were committed and pushed.
The current contracts are [consumer path](spec/consumer-path.md),
[SDK API](../ts/strk20-discovery/README.md), and [demo](spec/demo-app.md).

## Completed in this implementation

- Complete pool-state verification at an independently selected accepted
  Starknet RPC checkpoint; contract Patricia proof and global state commitment.
- Cached Patricia updates and atomic candidate application. Failed verification
  cannot replace the previous verified cache.
- Folded WASM cache: restore state and discovery cursors without replaying epochs.
  Local storage is explicitly trusted; SHA-256 is corruption detection only.
- Actual official `DiscoveryProviderInterface`, spend witnesses and account-bound
  convenience API. The official builder consumes our discovered note in a
  fixture test; the proof/signature submission path is not executed in that test.
- Full head/epoch SSE payloads, bounded queues and HTTP catch-up. The actual WASM
  Worker test verifies an epoch advance with no follow-up artifact GET.
- Browser wallet flow: create/backup/import, public funding, deploy, shield,
  local discovery, private transfer and withdraw. Heavy state work is in a Worker.
- Optional same-block dual observation, explicit viewing-key disclosure consent,
  failures and cache conditions recorded. One transaction serves both observers.
- Removed the mock engine, duplicate wrappers, synthetic replay bundle and stale
  generated reference metrics. Test servers reserve ephemeral ports themselves;
  child logging no longer depends on the parent environment.

## Resume here

1. Run the funded demo flow with the user and record the video. No funded
   transaction was signed or submitted during implementation checks. The current
   evidence covers unfunded wallet creation, real public checkpoint verification,
   cache restoration and SDK builder consumption of a real fixture note.
2. Verify the migrated lifecycle scripts with the user's funded flow. They now
   use our Node Worker host, private file cache and a common proving block. The
   Node integration test verifies real fixture state and restores discovered
   notes and cursors with the HTTP server offline; no transaction is submitted.
3. Measure incremental catch-up and discovery on a funded account. Full-mainnet
   empty-account measurements include a 33-block catch-up in 1.625 s, with
   176 ms spent verifying state; cold 40.31 s and fresh Worker restores
   308–342 ms. The repeat-start target is met for that measurement; this is not
   a cold-start or all-device guarantee.
4. Verify receipt recovery in the funded run. The signer now persists the exact
   SDK-computed hash before signing, so a lost send response can be resumed after
   reload. Tests mock cryptographic signing and network submission, and verify
   that resuming does not submit again. Legacy pending entries without a hash
   still require manual investigation; no automatic resubmission is attempted.
5. Deploy and verify the hosted page after the user supplies the server/deploy
   route. Checkpoint acquisition and bounded decompression now have separate
   modules; the Worker owns sequencing, state and persistence. Git push is not
   a production deployment.
6. Finish the hackathon video and submission metadata against the current rules.
   External wallet adoption or upstream acceptance is not implied by a demo.

## After the main plan

- AEAD for local state with an explicit key and attacker model. A replaceable key
  stored beside the cache does not authenticate the cache against that attacker.
- Ethereum-finalized checkpoint selection. Current mode trusts an accepted
  Starknet RPC header; it does not prove L1 finality or intermediate history.
- WebSocket measurements against full-payload SSE under identical conditions.
- PIR/private cold-loading research: identify which state can be skipped while
  preserving complete discovery and verifiable state. No unmeasured speed claim.

Custom recoverable accounts and automatic recovery allowances remain out of
scope. Demo keys are backed up together so both public and shielded funds remain
recoverable. The hosted prover receives proving inputs; local discovery does not
make that write-path service private.
