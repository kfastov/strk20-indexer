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
- Production block notifications trigger ingestion through WebSocket; publication
  wakes SSE directly, and bounded discovery waits for a head event. Live state
  updates are ingested once, including silent writes. Unchanged Patricia branches
  reuse hashes; the complete slot set is still compared before verification.
- Client checkpoint header and proof requests run concurrently at the same height.
  Proof hashes and state commitments remain bound to the trusted header. Waiting
  reads run before the cache save queued by an SSE update.
- Browser wallet flow: create/backup/import, public funding, deploy, shield,
  local discovery, private transfer and withdraw. Heavy state work is in a Worker.
- The hosted Sepolia flow completed with real STRK, proofs and transaction
  signatures on 2026-09-06. Our discovered note was spent, the replacement was
  withdrawn, and official/local note and witness results agreed. Receipts and
  timing limits are recorded in [demo evidence](spec/demo-app.md).
- Optional same-block dual observation, explicit viewing-key disclosure consent,
  failures and cache conditions recorded. One transaction serves both observers.
- Removed the mock engine, duplicate wrappers, synthetic replay bundle and stale
  generated reference metrics. Test servers reserve ephemeral ports themselves;
  child logging no longer depends on the parent environment.

## Resume here

1. Continue the performance work without claiming a stable speed win. The event
   path is deployed; Sepolia matched-block WS-to-SSE median fell from 2,493 ms
   (4 samples) to 243 ms (17 samples). This excludes checkpoint verification and
   discovery. Browser restart and comparison measurements, including regressions,
   are recorded in [demo evidence](spec/demo-app.md). The <=2 s navigation-to-data
   target is not yet reliably met. Remaining costs include WASM restoration,
   client verification/queueing and upstream RPC availability for announced blocks.
2. Verify the migrated lifecycle scripts with the user's funded flow. They now
   use our Node Worker host, private file cache and a common proving block. The
   Node integration test verifies real fixture state and restores discovered
   notes and cursors with the HTTP server offline; no transaction is submitted.
3. Record a clean end-to-end video on the final version. The successful Sepolia
   run included fixes and reloads and is acceptance evidence, not a finished
   demo video. Mainnet funded acceptance remains separate work.
4. Verify receipt recovery in the funded run. The signer now persists the exact
   SDK-computed hash before signing, so a lost send response can be resumed after
   reload. Tests mock cryptographic signing and network submission, and verify
   that resuming does not submit again. Legacy pending entries without a hash
   still require manual investigation; no automatic resubmission is attempted.
5. Deployment completed on 2026-09-06: both indexers and `/demo/` now run the
   updated implementation. Both services are healthy and advancing; full SSE
   payloads were observed through nginx. The deployment and rollback details
   are in [hosting](ops/hosting.md). The subsequent Sepolia browser acceptance
   run verified spending; the demo now includes the fixes found by that run.
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
