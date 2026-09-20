# strk20-discovery

Discover STRK20 notes locally, without giving the indexer your viewing key.
The client downloads public pool data, verifies the complete storage root against
an independently fetched Starknet checkpoint, and runs discovery in a Worker.
All wallets use the same public feed URLs. The feed can still see IP addresses
and request timing.

## Install

```sh
npm install strk20-discovery
```

Browser apps need a bundler with module Worker support, such as Vite. Node apps
need Node 24+. Worker/WASM assets and the unmodified official Privacy SDK are
included; installation requires no Rust toolchain or GitHub Packages login.

## Browser

```ts
import { LocalDiscoveryProvider } from 'strk20-discovery';

const discovery = new LocalDiscoveryProvider({
  network: 'mainnet',
  feedUrl: 'https://strk20.nullref.cc/mainnet/feed',
});
const mine = discovery.forAccount({ address, viewingKey });

await discovery.ready;                    // verified startup or local cache restore
const cached = await mine.restore();       // local discovery at the saved checkpoint
const { notes } = await mine.discoverNotes(); // catch up and verify current state
await discovery.subscribe();              // receive subsequent updates over SSE

// When the wallet closes:
await discovery.close();
```

The provider implements the official SDK's `discoverNotes`, `discoverChannels`
and `discoverRequirement`. Results contain real SDK witnesses and cursors.
The Worker maintains discovery progress; supplied cursors do not replace it.

Use the bundled SDK to keep classes and types consistent:

```ts
import { createPrivateTransfers } from 'strk20-discovery/privacy-sdk';

const transfers = createPrivateTransfers({
  account,
  viewingKeyProvider,
  poolContractAddress,
  provingProvider,
  discoveryProvider: discovery.atBlock(provingBlock),
});
```

`atBlock(number)` binds every discovery method to the same proving block.
The [demo](https://strk20.nullref.cc/demo/) and its
[source](https://github.com/kfastov/strk20-indexer/tree/main/ts/demo) show the
complete deposit, private transfer and withdrawal flow.

## Node

```ts
import { NodeDiscoveryProvider } from 'strk20-discovery/node';

const discovery = new NodeDiscoveryProvider({
  network: 'mainnet',
  feedUrl: 'https://strk20.nullref.cc/mainnet/feed',
  cacheDirectory: '/your/private/discovery-cache',
});
try {
  await discovery.ready;
  const mine = discovery.forAccount({ address, viewingKey });
  const current = await mine.discoverNotes();
} finally {
  await discovery.close();
}
```

Node uses the same runtime in `worker_threads` and writes cache files with mode
0600. The cache directory contains private discovery results.

## Configuration and subscriptions

`feedUrl` is required. `network` defaults to `mainnet` and also accepts `sepolia`
or a custom `ChainProfile`. Optional `rpcUrl` selects the trusted header RPC;
`proofRpcUrl` supplies direct proofs. The default `proofSource: "feed"` uses
SSE/HTTP proof delivery and falls back to `proofRpcUrl` on HTTP 410 for historical
blocks outside the feed window. Set `proofSource: "rpc"` to acquire proofs directly.
A current feed proof error is surfaced, not silently bypassed.

Provide `onEvent` when constructing either provider to receive startup progress,
advertised heads, verified engine state, timing spans, public request metadata and
background errors:

```ts
const discovery = new LocalDiscoveryProvider({
  network: 'sepolia',
  feedUrl: 'https://strk20.nullref.cc/feed',
  onEvent(event) {
    if (event.event === 'state') console.log('Verified at', event.value.verifiedAt);
    if (event.event === 'error') console.error(event.value);
  },
});
await discovery.ready;
await discovery.subscribe();
// Later, before using a transaction's resulting notes:
await discovery.waitForBlock(transactionBlock);
const current = await discovery.forAccount({ address, viewingKey }).discoverNotes();
// On teardown, including errors:
await discovery.close();
```

The application supplies `transactionBlock`, wallet `address` and SDK `viewingKey`.
`waitForBlock` waits for advertised feed coverage, not state verification; the
subsequent discovery performs that check. `subscribe()` starts background updates
and returns without waiting for a first verified live event. Use `onEvent` for
background errors and call account discovery to refresh private results.
Transport failures reconnect; a chain mismatch or publisher verification-failure
status stops the stream. `close()` stops it, flushes pending cache writes and
terminates the Worker. There is no separate provider unsubscribe method.

`atBlock(B)` and the SDK `blockIdentifier` parameter accept block numbers (also
`{ block_number: B }` for `blockIdentifier`); an omitted identifier or `latest`
selects current feed state. Hash/tag bounds other than `latest` are unsupported.
A bound below a snapshot's basis requires a different bootstrap/history source.

## Initialization and persistence

`ready` downloads, verifies and saves state on a cold start, and rejects on
failure. A valid trusted cache restores without network requests. `restore()` runs
local discovery at that cached checkpoint without catching up; `discoverNotes()`
catches up and verifies. Keep keys in the application's wallet storage, separate
from disposable discovery state.

The advanced `discovery.client` API exposes `sync(block?)`, `clearCache()` and
`ready` engine information. Clearing it discards discovery state, not application
wallet keys. `workerFactory` supports a custom browser host; Node uses its own
worker-thread factory and requires `cacheDirectory`. The source contracts are
[`ClientOptions`](https://github.com/kfastov/strk20-indexer/blob/main/ts/strk20-discovery/src/client.ts)
and [`WorkerEvent`](https://github.com/kfastov/strk20-indexer/blob/main/ts/strk20-discovery/src/types.ts).

Changed state saves in the background; unchanged reads do not save again.
Snapshot/epoch downloads permit 30 seconds without progress and five minutes in
total; RPC/metadata requests have a 30-second deadline.

State verification covers a selected checkpoint, not every historical transition
or Ethereum finality. The configured header RPC and local cache are trust roots;
cache checksums detect corruption, not malicious replacement. Conservative note
maturity can delay spending. The complete guarantees and failure behavior are in
[Consumer path](https://github.com/kfastov/strk20-indexer/blob/main/docs/spec/consumer-path.md).
For source builds, see the [repository instructions](https://github.com/kfastov/strk20-indexer#build-from-source).
