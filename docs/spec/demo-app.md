# Real-chain demo

Current implementation: `ts/demo`. Mainnet and Sepolia use public feeds and
RPCs; the default build contains no synthetic replay or mock discovery engine.

## User flow

One primary button advances through wallet creation, funding detection,
deployment, shield, local discovery, private transfer, discovery and withdrawal.
The activity log shows user operations and their durations. Expand an operation
to inspect the proof, verification and discovery work beneath it. WASM runs only
in a Worker, so proof verification cannot freeze the action button or spinner.

The wallet is generated locally. Signing and viewing keys persist in a separate
IndexedDB database from disposable feed state. Each network retains its own
wallet. Export/import preserves both public and shielded funds' recovery material.
The page warns that keys live in this browser and recommends small amounts.
Clearing discovery cache must never erase wallet material.

Funding is detected with public `balance_of` RPC; it does not exercise private
note discovery. The shield → spend transition uses our `LocalDiscoveryProvider`,
including real witnesses, channels and requirement checks. All builder discovery
reads are pinned to the proving block. The proving service remains an external
confidentiality dependency and receives the proving inputs.

## Measurement

The same operation spans populate the screen and exported timing JSON.
Cached initialization, network catch-up, cold verification, note discovery,
proof generation and transaction confirmation are distinct measurements.

Optional comparison submits a transaction only once. Our provider and the
official discovery service then observe the same block concurrently. Enabling
comparison explicitly permits sending this demo wallet's viewing key to that
service. Errors and attempts are recorded. Results must agree on unspent notes,
amounts and spend witnesses before treating a run as a speed comparison.
Conservative local `created` bounds differ from upstream write timestamps and
are not presented as equivalent historical proofs. Cache conditions are also
reported: our provider may restore saved state; the official provider starts
without a supplied cursor. This is a comparison of these configured paths,
not a claim of identical cold-start workloads.

## Current verification evidence

On 2026-09-06, the built page was checked in isolated headless Chrome. Creation
of an unfunded Sepolia wallet and balance lookup succeeded without console errors.
On full public mainnet state at block 14,420,590, cold verification plus discovery
for an empty test identity took 37.85 s; three fresh Worker restores took
298.6, 295.9 and 292.0 ms. The complete state was verified in each cold run; this
is not a tiny fixture measurement. A funded wallet may add note-discovery work.
The maximum observed interval of a 50 ms main-thread heartbeat was 82.2 ms.
These are local-machine measurements, not guarantees for all devices or networks.

### Funded Sepolia acceptance run, 2026-09-06

The deployed browser demo completed registration/shield, private self-transfer
and withdrawal with our provider supplying the builder's discovery data. The
account was generated in the page, received 105 test STRK (5 through the agent
faucet and 100 from the user's faucet claim), and was deployed in 7.50 s.

| Action | Accepted block | Browser operation | Receipt |
|---|---:|---:|---|
| Shield 0.01 STRK | 14630720 | 16.87 s | [SUCCEEDED](https://sepolia.voyager.online/tx/0x760b6fc6e6cefa318a53071823466163b4ba48a2aac513ccbc3a7fc5cf0e88d) |
| Spend by self-transfer | 14630873 | 17.41 s | [SUCCEEDED](https://sepolia.voyager.online/tx/0x48557def3325c9fc153b0e53e22fc624a51a65148d0f904b10a0d0099ed5386) |
| Withdraw 0.01 STRK | 14631004 | 15.93 s | [SUCCEEDED](https://sepolia.voyager.online/tx/0x28d8c41ea12d5ce715e557c69945f87a0e0260f4a0210bb0d6d20e2fd845ce4) |

Both discovery providers agreed on notes and spend witnesses after transfer
(block 14630981) and withdrawal (14631073). Final displayed balances were
91.84274 public test STRK and zero private STRK. The wallet survived reloads
between actions. This is a live Sepolia result, not a mainnet transaction run.

The run exposed and fixed three defects: zero-public-key channel placeholders
broke registration; background SSE skipped the explicit observation step; and
hex-string versus bigint note IDs caused a false benchmark mismatch. The first
shield attempt stopped before proof submission. The false comparison is not
counted as a valid measurement.

Performance remains unfinished. At block 14630981, official discovery took
0.46 s (one attempt), local discovery 7.03 s (six attempts). After withdrawal:
0.43 s versus 4.37 s (two local attempts). The local path was waiting/retrying
for the requested state; equal results do not imply a speed advantage. Repeated
funded-wallet restores measured 1.98, 2.27 and 2.68 s; the last reload used the
same deployed assets. In the 2.27 s run, restoring WASM state took 1.94 s.
The <=2 s repeat-start target is therefore not yet reliably met by the hosted
demo. The earlier empty-account numbers below do not establish that target.

### Event-driven performance follow-up, 2026-09-06

Backend `4bbef8a` removes production head polling, the one-second SSE file poll,
repeated live-block ingestion and unchanged-branch rehashing. The entire slot
set is still checked; silent state writes and rollback remain covered. The SDK
waits on feed events for a requested bound, reuses staged public artifacts,
prioritizes that checkpoint, fetches header/proof concurrently, and queues the
post-SSE cache save behind already waiting reads (`66847ea`). No proof is skipped.

A separate probe connected to PublicNode new-head WebSocket and the public SSE
endpoint concurrently and matched first arrival of the same block number:

| Deployment / network | Matched samples | Median | Min–max |
|---|---:|---:|---:|
| Before / Sepolia | 4 | 2,493 ms | 2,117–2,860 ms |
| Event wake only / Sepolia | 15 | 621 ms | 438–2,248 ms |
| Final backend / Sepolia | 17 | 243 ms | 221–806 ms |
| Final backend / mainnet | 17 | 664 ms | 128–2,864 ms |

These are short samples of matching block numbers, not transaction-to-discovery
latency or an all-block percentile. Coalesced heads without an exact match are
excluded. RPC sometimes returns `Block not found` for a just-announced block;
the indexer catches up on a subsequent notification. HTTP `latest` also lagged
WebSocket by one or two blocks, so the indexer now requests the announced height.
After warming, server root calculation fell from the first mainnet calculation's
12,559 ms to 337–393 ms; Sepolia roots took 45–74 ms. Full consumer checkpoint
verification succeeded on both deployed feeds, including parallel proof retrieval
at Sepolia 14634430 and mainnet 14446387.

Browser measurements below use the same funded wallet after its withdrawal:
current note/witness sets are empty and match, so this is read-path performance,
not a new nonempty spend acceptance run. Both observers request the same block;
the local cache and reference's fresh cursor differ as described above.

| Backend / demo | Block | Local | Official | Local attempts |
|---|---:|---:|---:|---:|
| Before | 14632459 | 5.71 s | 0.83 s | 5 |
| Event wake only | 14632698 | 0.54 s | 0.81 s | 1 |
| Event wait client | 14632909 | 0.89 s | 0.42 s | 2 |
| Event wait client | 14633155 | 4.58 s | 0.41 s | 2 |
| Final backend / 639d856 | 14634289 | 1.90 s | 0.91 s | 1 |
| Final backend / 639d856 | 14634312 | 2.06 s | 0.49 s | 1 |
| Final backend / 639d856 | 14634325 | 1.24 s | 0.43 s | 1 |
| Final backend / cb631d2 | 14634520 | 2.28 s | 0.41 s | 2 |
| Final backend / 66847ea | 14634611 | 0.55 s | 0.44 s | 1 |
| Final backend / 66847ea | 14634625 | 1.75 s | 0.42 s | 2 |
| Final backend / 66847ea | 14634653 | 2.40 s | 0.40 s | 2 |
| Final backend / 66847ea | 14634664 | 0.36 s | 0.40 s | 1 |

Earlier labels saying “Page to restored result” measured initialization only,
not navigation. The corrected metric includes document/scripts/Worker loading.
After restarting browser control, three `639d856` loads measured 5.39, 2.37 and
4.79 s navigation-to-restored-data (initialization 3.28, 1.19 and 3.14 s).
The cache was 4,522,005 bytes; WASM restore took 1.42, 0.56 and 1.63 s. First loads
of the next demo builds measured 2.20 s (`cb631d2`) and 4.55 s (`66847ea`), including
changed script assets. Reloading the same `66847ea` assets then took 2.49 s
(initialization 1.11 s, cache read 0.05 s, WASM restore 0.57 s, discovery 0.16 s).
Initialization alone had also measured 0.09–0.17 s before
these client changes: those values cannot be attributed to this optimization.
The <=2 s navigation target and a stable advantage over the official path remain
unproven. A single faster run does not establish either claim.

### Discovery tail-latency diagnosis (2026-09-06)

A 14-observation Node Worker trace used the deployed Sepolia feed, the actual
SDK/WASM, public RPC, and an independent empty identity. It did not spend or
reuse the funded browser wallet. Each observation fixed a block number before
reading. Seven already-verified reads took 0–6 ms; these reused the exact
checkpoint and are **not** fresh-verification benchmarks. Two fresh ready-feed
reads took 240 and 429 ms. Five observations waited for the feed and took
1,148–4,651 ms, including 538–3,837 ms inside the event-driven `waitForBlock`.
No fixed retry backoff was used in those five cases. Across all 11 checkpoint
acquisitions in the trace, network retrieval took 190–759 ms and local pool
verification 28–49 ms. Across 14 reads, empty-note discovery took 0.05–4.41 ms.

The 4,271 ms observation at block 14637928 decomposed as follows:

| Critical-path part | Time |
|---|---:|
| Initial requests discover that the feed is behind | 279 ms |
| Wait for a covering SSE publication | 3,329 ms |
| Fetch the requested checkpoint header and proof concurrently | 622 ms |
| Verify pool state | 30 ms |
| Discover notes | <1 ms |

The remaining roughly 10 ms covers scheduling and folding. A subsequent
40.5 ms cache save was outside the observation. The foreground retry queued
behind background verification of the **same requested block** and reused its
result; there was no second proof acquisition on that retry.

During the feed wait, server logs show `getBlockWithTxHashes` error 24 twice,
then ingestion through block 14637930. The old event loop discarded the failed
target and requested the newest notification each time. HTTP availability
lagging WebSocket announcements could therefore prevent progress across
several notifications even though earlier blocks were already available.
A separate event-triggered 16-head probe confirmed this: Cartridge served
13/16 newly announced blocks and PublicNode 12/16; both served the preceding
announced block in all 15 applicable checks. Two new heads were absent from
both providers. Merely switching providers cannot remove that availability gap.
The preceding server-log sample also contained two `getStateUpdate` error-24
failures, versus 157 header failures across both networks. The header fallback
does not remove state-update/proof availability failures or ordinary RPC RTT.

This also explains the earlier 243 ms matched-block SSE median: coalesced
publications such as 14637930 covering a wait for 14637928 were excluded from
that sample. It is not a percentile of all consumer waits.

The ingestion correction tries the exact announced height first. Only when
that header returns `Block not found` does it request HTTP `latest` once and
process the available range in the same event cycle. No timer or repeated
availability polling is added. A stale answer cannot move the stored frontier
backward without a detected reorg; wrong numbered responses are still rejected.
The regression test failed on the old implementation and checks progress
across three consecutive unavailable announcements plus stale-head rejection.

Backend `e553c2c` was activated after CI passed at `429f07d` (a test-only
follow-up fixes a health/log snapshot race in the recovery acceptance test).
The next 14-observation run used the same Worker harness and cache: six
already-verified reads took 0–7 ms, three ready-feed reads 200–492 ms, and five
feed-waiting reads 1,347–2,840 ms (713–2,410 ms waiting). These short, sequential
samples do not establish a production percentile or isolate changing RPC RTT.

The remaining 2,840 ms case at 14638767 is also traced. The producer received
error 24, processed available 14638766 and finished that cycle at 10:55:29.472
UTC. The client requested 14638767 at 10:55:29.997. Publication covering it
arrived at 10:55:32.599, followed by a 199 ms checkpoint fetch and 30 ms local
verification. The fallback prevents starvation of available earlier blocks,
but a prematurely announced target still waits for another wakeup: the configured
new-head subscription gives no separate signal when HTTP data becomes ready. This
remaining delay must not be described as solved by the fallback.

A further passive-WebSocket trace confirms the wakeup gap independently.
PublicNode announced 14638815 at 10:56:48.890 UTC; the producer's HTTP request
failed and its fallback cycle finished at 10:56:49.110 with head 14638814.
The next WS announcements arrived at 10:56:52.598 and 10:56:52.629 (14638816
and 14638817): a 3,708 ms gap followed by a 31 ms burst. SSE covering the
requested 14638815 arrived at 10:56:52.849, 251 ms after the next notification.
The waiting read took 2,859 ms: 305 ms before the bound error, 2,309 ms waiting,
205 ms fetching the checkpoint and 35 ms verifying it. The producer was idle
between events; neither a one-second SSE poll nor WASM computation caused this
tail. Faster future recovery needs a data-ready event source or a bounded
alternate RPC attempt on failure; adding another WebSocket transport alone
does not supply that missing readiness signal.

Demo `8d24318` exports observation start timestamps and per-retry reasons,
failed-attempt durations and feed/backoff wait durations. These are also
expandable inside the comparison result. Before the ingestion correction, the
funded browser wallet's empty-note read at 14638198 took 1.33 s locally versus
0.93 s officially: the failed attempt took 0.32 s and feed waiting 0.46 s.
A separate 7.04 s one-attempt browser result overlapped local Rust compilation
and browser-control timeouts. Its expanded activity includes 2.01 s local
verification, 2.64 s discovery and 2.09 s persistence spans, as well as 2.51 s
checkpoint retrieval. Activity includes background work, so these spans cannot
be summed into the observation's duration. Local scheduling/load is a confounder;
its precise contribution is not established and this result is not used to
estimate deployment latency. Native Chrome UI access subsequently recovered
control. Post-deployment reads at 14638945 and 14638969 rounded to 0.00 s local
versus 1.15 and 0.44 s official, with matching notes and witnesses: the requested
state was already verified by the subscription. These are cache-hit examples,
not evidence of faster fresh checkpoint verification.

### Source latency and coherent block ingestion (2026-09-06)

Paired observations on the production VPS remove the home network from the
comparison. Both the actual Node/WASM provider and official SDK queried the
same fixed block and independent empty identity. Before feeder ingestion,
20 observations included local delays of 1,061 and 1,676 ms while the warmed
official endpoint took 116–125 ms. Checkpoint prefetch alone still left waits
of 1,165, 1,595 and 2,602 ms.

Backend `778849f` acquires the sequencer feeder's complete accepted block,
receipts and state diff in one request, bound to the subscription header.
It includes silent storage writes; events alone are insufficient. Samples
returned 20/20 valid bundles on each network (Sepolia median 138 ms, mainnet
126 ms). Comparison against eight real pool-active Sepolia blocks matched all
27 storage writes and 25 events, including transaction hashes and event indices.
This input transport does not replace the client's independent checkpoint proof.

In the subsequent 20 paired observations, local results ranged from 2 to
1,910 ms; the official warmed results were 124–130 ms. The 1,910 ms observation
at 14640800 waited for a late source notification: the official result finished
at 11:51:56.453 UTC, the feed arrived around 11:51:58.038. Its prepared proof
had already completed. Local verification took 85 ms and an intervening cache
save 97 ms. Separately, 24 observed notification-to-SSE times were mostly
126–160 ms, with a 575 ms maximum. This latter metric excludes time before
the notification and cannot stand in for transaction discovery latency.
Two parallel PublicNode subscriptions delivered the same 28 heads within
9 ms of each other; duplicating that connection would not remove source lag.

A public `head.ndjson` read now also requests an ingestion cycle against the
latest complete feeder block. Requests coalesce once per published version;
there is no timer, wallet-specific parameter or extra client request. This
provides a second wakeup when a subscriber's target precedes a delayed source
notification. It is not a guarantee that upstream has published the target yet.

Chrome extension verification after `778849f` restored the funded browser
wallet and discovery in 0.26 s. A read at 14640897 showed local 0.00 s versus
official 0.39 s, with matching notes and witnesses; this was another cached
checkpoint, not a fresh-path speed claim. Browser tests use tab-scoped extension
control and screenshots without activating the user's browser window.

### Demand wakeup and remaining proof availability (2026-09-06)

After `7e444be`, 20 further same-block VPS pairs produced local times of
2–1,767 ms. The three feed-unavailable observations completed in 330, 335 and
462 ms. The two slowest results (1,767 and 1,552 ms) instead exhausted fresh
`getStorageProof` error-24 retries and incurred the benchmark's 1,000 ms
transport backoff. The official API also returned HTTP 503 in those two cases,
completing in 1,253 and 1,254 ms on its second attempt. Its other observations
were 123–155 ms. These results do not establish that local fresh verification
is always faster. Two of 26 separate subscription observations found the block
already published before the PublicNode notification arrived.

A 20-head proof probe queried Cartridge concurrently by both block number and
hash: both forms succeeded on the same 14 heads and both refused the same six
with error 24. Changing identifier form does not solve this observed lag.
Other public Sepolia proof endpoints were unusable in this check: Lava v0.8/v0.9
returned no available providers, dRPC did not expose the method, OnFinality's
Sepolia host was unreachable, and Blast returned 403 announcing retirement.
The public OnFinality mainnet WebSocket demanded an API key even for the first
subscription. None is configured as a speculative fallback.

`9e42967` acquires server proof and binding header concurrently. Observed
Sepolia proof/binding time fell from about 195 ms to 97–99 ms while reorg and
persistent-mismatch tests retained their distinct outcomes. Mainnet verification
through the actual Node/WASM consumer succeeded at 14453734. That VPS restored
24,114,460 bytes of saved state in 1,232 ms and caught up in a further roughly
1.3 s; the saved state became available before catch-up. The preceding cold
mainnet verification at 14453291 took 114.2 s. These CPU/hardware-dependent
measurements are distinct from the browser's 0.26 s Sepolia restoration.

Server root reuse is invalidated by transactional SQLite storage-write triggers,
including direct updates, deletes, cascaded rollback and writes from another
connection. A read snapshot also checks for stored writes between requested
heights: historical traversal must not reuse a newer root merely because the
DB revision did not change. Empty blocks can reuse the root; each block's proof
is still acquired and bound separately. No feed format or client trust boundary
changes.

### Subscription recovery and foreground scheduling (2026-09-06)

A live startup reproduced an open WebSocket that never acknowledged its
subscription: ingestion was only awakened again by a reader. The former
60-second inactivity wait allowed the published head to fall behind. An
absolute two-second ACK deadline and ten-second head deadline now bound these
states, independent of ping traffic. On deployment the ACK failure recurred
and recovered through reconnect within seconds. This does not remove delays
before the upstream source publishes its notification.

A failed requested checkpoint no longer pins future SSE verification to an
unavailable old block. This matters with limited-history proof services: in
one concurrent Cartridge probe, distances 0–16 behind its own head returned
proofs while distances 32–512 returned error 42. This is one observed window,
not a universal retention guarantee. No working alternative public Sepolia
proof service was established by the probes above.

After backend `1dbd6be`, two 20-pair VPS series completed without a local
one-second transport backoff. Local ranges were 1–396 and 2–643 ms; these
include cached reads. The second series exposed queued background work ahead
of foreground reads: its 593 ms case included three verification spans and a
114 ms save; another 643 ms case included a 345 ms proof wait and two
verification spans. Its 628 ms case combined waiting for feed publication,
an initial proof refusal and recovery. The official API returned HTTP 503 in
that last case and completed in 1,252 ms after the harness's one-second retry.

The worker now keeps foreground and background queues. State mutations remain
serialized; waiting client requests precede the next coalesced SSE update or
cache write. An active operation still completes before another starts. A
real-WASM regression pauses the current verification, queues an SSE head and
then a foreground read: the old order requested proofs for 98,99,97, whereas
the corrected order serves 98,97 first. The test owns its Worker host rather
than replacing global handlers.

A subsequent 20-pair VPS series with this scheduling change produced local
1–422 ms (fresh work 203–422 ms, cached 1–3 ms), versus official 122–153 ms.
Two local cases waited for an SSE update and completed in 365 and 385 ms.
These short public-network samples do not establish a latency ceiling or
that fresh independent verification beats the official API. Benchmarks use
the same requested block and actual SDK/WASM on the VPS with an empty test
identity; cached local state and uncached official responses are reported
separately. They are not funded-transaction discovery measurements.

Mainnet restored 24,116,066 bytes in 1,255 ms, then independently verified
14455072 with 213 ms checkpoint wait and 1,066 ms apply time. Sepolia restored
4,536,620 bytes in 172 ms and independently verified 14643219. Startup restore,
fresh catch-up and cold full verification remain different measurements.

An experiment sharing immutable trie structures was discarded: measured
steady verification medians of roughly 97 versus 92 ms did not establish a
worthwhile end-to-end improvement. The deployed WASM uses the original
representation, rebuilt from committed Rust source.

### What the RPC-role measurements establish

There is no isolated full-discovery A/B test changing only the mainnet RPC role
configuration. Earlier consumer timings also changed ingestion, trie reuse,
checkpoint concurrency and scheduling, and cannot attribute the entire gain to
provider separation.

A prior 20-request mainnet `l1_accepted` header probe had Lava 19 successes,
median 86 ms, maximum 1,609 ms and one three-second timeout; PublicNode had 20,
median 100 ms and maximum 317 ms. This favored the observed tail, not median
latency, and did not measure availability of newly announced blocks.

A follow-up probe on 2026-09-06 observed 20 PublicNode WS head notifications on
the VPS. For each exact announced height, both endpoints received one header
and one state-update request concurrently, with no retry and a three-second
request timeout. Success required the returned block hash to match the notification.

| Method | Provider | Matching answers / 20 | Median successful response | Maximum successful response |
|---|---|---:|---:|---:|
| Block header | Lava | 18 | 149.9 ms | 1,483.9 ms |
| Block header | PublicNode | 14 | 116.1 ms | 337.2 ms |
| State update | Lava | 17 | 159.4 ms | 999.5 ms |
| State update | PublicNode | 15 | 120.7 ms | 312.9 ms |

All unsuccessful answers were RPC error 24. These successful-response medians
exclude different sets of blocks. On the 13 common successful header pairs,
PublicNode won only six and its paired median was 29.2 ms slower. On the 13
common successful state-update pairs, it won eight with a 68.6 ms paired median
advantage. This is not evidence that PublicNode is universally the fastest live
source. Sampling is triggered by PublicNode itself, not by an independent chain
arrival clock; no retry-to-ready time or complete-discovery time was measured.

Provider roles remain useful for capability separation: archive/proof requests
need not move the ordinary live endpoint. Current primary block data comes from
the coherent feeder, so the table does not directly measure that ingest path.
There is no automatic fastest-provider router. A full-data stream could remove
the feeder request after a header notification, but it has not been integrated.
Routine indexer-to-client SSE already carries the data itself; independent
checkpoint header/proof requests still remain.

### Earlier state-only measurements

The 2026-09-06 follow-up measurement verified block 14,440,930 and restored it
in 341.5, 312.4 and 308.4 ms. Catch-up to 14,440,963 took 1,624.7 ms, including
245.1 ms for the queued cache save, 961.5 ms for checkpoint acquisition and
176.1 ms for verification. The HTTP response bodies totalled 204,417 bytes;
this is decoded body size, not compressed wire traffic. Two subsequent polls
returned the same checkpoint and are not counted as further chain advances.
Cold discovery took 40.31 s; the maximum 50 ms main-thread interval was 51.7 ms.
With manifest revalidation enabled, another full-state run took 40.54 s cold,
306–313 ms on restore, and 1.331 s to advance from 14,441,166 to 14,441,197
(177.1 ms verification). The following two polls again returned that same block.
Separate direct HTTP samples also observed an unchanged published head across
an 11-second interval; this observation does not establish the cause of the delay.

Transaction intent and the SDK-computed transaction hash are saved before the
signer can enable broadcast. If the RPC response is lost, the normal resume
button looks up that saved hash. A test retains the actual SDK transaction
construction, replaces cryptographic signing and RPC submission, simulates a
lost response, reloads the wallet and confirms that resume does not send again.
Legacy pending records without a hash remain a manual recovery case.

## Deferred

AEAD after the main implementation, Ethereum-finalized checkpoint selection,
WebSocket comparison, PIR research, custom recoverable accounts and automatic
recovery authorizations. None is represented by a stub mode in this demo.
