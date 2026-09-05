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

No funded transaction was signed or submitted during these checks. A recorded
shield → transfer → withdrawal run remains necessary before claiming a verified
end-to-end live transaction demonstration.

## Deferred

AEAD after the main implementation, Ethereum-finalized checkpoint selection,
WebSocket comparison, PIR research, custom recoverable accounts and automatic
recovery authorizations. None is represented by a stub mode in this demo.
