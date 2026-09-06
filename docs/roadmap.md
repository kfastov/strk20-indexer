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

## Submission reconciliation, 2026-09-06

This reconciles the original [21-item plan](pre-submission-corrections.md), the
later accepted design, current source and live evidence. The original plan is a
historical decision record, not a current checklist. Source reviewed: `1955dd9`.
The deadline is September 7, 23:59 UTC (September 8, 02:59 Moscow), according to
the [current rules](https://github.com/starkience/strk20-hackathon#submitting).

The hub's current `projects.json` reports four verified mainnet transactions,
`demo=true`, `mainnet=true`, `video=false`, status `building`. Its indexed source
is still `5152e07`, so it is not evidence that the newest implementation was
reviewed. Local `strk20.json.demo_video` is also empty. No extra submission PR is
required; the repository at the deadline is the entry.

| Original items | Current evidence / remaining acceptance |
|---|---|
| A: mainnet repair and backward epoch recut (1–2) | Repair/recut code and published snapshots exist; full consumer state checks now pass on both networks. This audit did not repeat the old all-history event-count comparison. State-root agreement does not prove intermediate history. |
| B: CORS, Docker, compose, health, cache headers (3–7) | Deployed on both networks; backups, rollback, advancing heads and complete public SSE payloads checked. Final release smoke remains part of acceptance. |
| C: packaging-only fork, single pin, delta CI, upstream PR (8–11) | Implemented. Upstream PR #984 remains open; merging is outside our control and is not a submission dependency. |
| D: consumer/WASM/cache/SSE/SDK (12) | Implemented, including actual SDK interface and account-bound sugar. Release 0.1.0 bundles the unchanged official SDK and WASM. Installation, Node discovery against Sepolia, and a demo build passed in an isolated consumer without a source checkout or GitHub Packages credentials. [0.1.0 is published on npm](https://www.npmjs.com/package/strk20-discovery). |
| D: transaction-history API (13) | Still optional, not implemented as the proposed complete transaction-history surface. Do not imply current-state verification authenticates transaction history. |
| E: README, diagram, pitch (14–16) | README rewritten around the SDK and demo, with explicit trust and performance limits. Still outstanding: pitch claims a final-state root proves every historical write, advertises old 0.03 s measurements and says snapshots are unpublished. Diagram still shows polling and a SQLite snapshot. Rewrite before using them for judging/video. |
| E: Sepolia scripts (17) | The old standalone Sepolia scripts were removed on the user's instruction; the SDK and browser demo are now the maintained integration path. |
| F: main branch and mainnet hashes (18–19) | Commits are pushed; four existing mainnet hashes are verified by the hub. They do not establish a mainnet lifecycle using our newly implemented provider. |
| F: video and final metadata (20–21) | Not done: no video URL, no final metadata/hub acceptance. Empty optional `contracts` is not a missing deployed contract. |

### Later accepted requirements

- **Real state proofs:** implemented. Complete state at accepted RPC checkpoint B;
  no proof of all intermediate writes or Ethereum finality. Cache remains explicitly trusted.
- **Self-contained wallet demo:** real Sepolia shield → local discovery → spend →
  withdraw passed. Mainnet state verification passed, but a complete funded mainnet
  lifecycle with the new provider is not yet established. The previous Sepolia run
  included fixes and reloads; it is not a clean final video run.
- **Restart and transfer size:** earlier funded restarts exceeded two seconds; later
  individual restores were faster. Repeat navigation-to-cached-notes acceptance on
  the final build is still needed. Record actual cold transferred bytes, WASM work
  and catch-up separately; current-state snapshot size and full-history size are
  different quantities. Do not use the former 8/16 MB figures as current cold traffic.
- **Same-transaction benchmark:** implemented. Latest 20-pair VPS series after queue
  priority: local fresh work 203–422 ms, cached 1–3 ms, official 122–153 ms. No stable
  fresh-path speed victory established. Final funded evidence remains separate.
- **Recovery:** both wallet keys persist separately from disposable state, with
  export/import. Precomputed transaction hashes are saved before signing and
  response-loss behavior is tested. A funded receipt-resume acceptance run remains.
- **Simplification/test isolation:** mock engine and duplicate wrappers were removed;
  test ports, child logging and Worker host isolation improved. This is not a completed
  whole-project bloat audit: `ingest.rs` has roughly 1,100 lines before its test module,
  `cutter.rs` roughly 1,188, and CLI/run handling shares a 1,292-line `main.rs`.
  Size is a review trigger, not proof that all these lines are unnecessary.
- **External adoption:** no confirmed third-party integration. Prepare a reproducible
  example; contact a team only with explicit message authorization. Adoption and
  upstream acceptance remain bonus evidence, not gates for submission.

### Mandatory fixes before submission: mainnet acceptance findings

- Provider initialization must perform the first snapshot download and complete
  state verification before reporting readiness. Do not defer a cold sync to the
  first shield/builder call or merely rename that work. Preserve the trusted-cache
  fast restart path. Requested by the user on 2026-09-06 after the 53.77 s shield.
- Separate foreground transaction spans from background SSE/cache spans. The
  current global `Operations.active` attaches concurrent Worker events to whichever
  foreground action is open; nested durations cannot be summed as serial work.
- Instrument signing/submission, balance/fee preflight RPCs and persistence so the
  unaccounted part of transaction latency is visible. The first mainnet transfer
  took 15.53 s; its named foreground stages cover only 13.06 s.
- Investigate the warm discovery fluctuation observed after the mainnet transfer:
  7.02 s total, with verification spans 1.74–1.89 s and cache-save spans
  1.48–2.18 s, versus earlier 0.15–0.21 s verification. Attribute queue delay,
  execution and background work separately before claiming stable warm latency.
- Receipt polling is replaced by transaction-status WebSocket events plus one
  catch-up read when connecting/reconnecting. Receipt/hash/block validation and
  resume without a new send are retained. The next user-submitted mainnet action
  still needs to measure this path on a newly accepted transaction; do not present
  already-confirmed receipt checks as inclusion-latency measurements.

### Ordered remaining work

1. Finish the pitch and single dataflow diagram; README is rewritten. Center the pitch on
   discovery without disclosing the viewing key and independently checked state;
   remove historical-proof and universal-speed claims. Keep measurements in the
   demo evidence rather than multiplying contradictory copies.
2. Rehearse the final deployed flow from entry through funding, shield, discovery,
   transfer and withdrawal. Complete mainnet acceptance with our provider; retain
   hashes and exported timing evidence. Verify reload/receipt recovery and repeated
   warm starts. Use the same run as the basis for the video.
3. Keep the release focused on SDK and demo. The user explicitly moved the bloat
   audit and large-server-module revision after submission. Fix release-blocking
   defects found during acceptance without starting that refactor.
4. Record and publish the three-minute video. Show real transactions, our discovered
   note supplying the next spend, and the privacy distinction. Explain any compressed
   waits; do not turn cached-vs-fresh timings into a universal speed claim.
5. Insert the real video URL into `strk20.json`, retain valid mainnet hashes, push,
   then verify the hub recognizes all requirements. Check public demo/assets,
   clean build and CI once more on the final release.

### Performance work boundary

The deployed transport is **WS header → one feeder HTTP request with block,
receipts and state update → indexer → full-payload SSE → client**, plus independent
client checkpoint RPCs. The data request after the upstream notification remains.
Standard new-head subscriptions contain headers, not complete storage diffs.
A data stream such as Apibara/Starkstream is a candidate, not an implemented or
measured replacement. Do not delay required submission artifacts for that integration.

The role split allows live RPC and proof/archive capabilities to differ. There
has been no isolated end-to-end A/B test changing only that configuration. A
fresh 20-head mainnet comparison found more error-24 refusals on PublicNode than
Lava despite shorter successful-response tails. Neither endpoint is established
as universally fastest/freshest; details are in [demo evidence](spec/demo-app.md).
Automatic latency routing remains unimplemented. Further provider research,
zero-extra-request data streaming and cross-transport experiments are deferred
behind the submission work, unless a reproducible release-blocking failure appears.

## After the main plan

- Bloat audit and revision of the large server modules (explicitly deferred by the
  user on 2026-09-06). Continue removing proven duplication and test pollution there.

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
