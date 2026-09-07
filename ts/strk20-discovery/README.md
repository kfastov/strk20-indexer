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
  onEvent(event) {
    if (event.event === 'state') {
      // The new pool state has passed verification. Discover this wallet's
      // notes locally, without another network request.
      void publishNotes().catch(reportError);
    }
    if (event.event === 'error') reportError(new Error(event.value));
  },
});
const mine = discovery.forAccount({ address, viewingKey });

async function publishNotes() {
  const result = await mine.restore();
  if (result) renderNotes(result.notes);
}

await discovery.ready;
await publishNotes();        // initial result, including a warm cache restore
await discovery.subscribe(); // keep verifying updates and emitting state events

// Call this when the wallet/component closes, not immediately after subscribe.
const stop = () => discovery.close();
```

`renderNotes` and `reportError` are your application's callbacks. `onEvent` is
the update channel in both browser and Node providers. `state` means verification
finished; `head` only announces feed availability and must not update a balance.
`mine.restore()` runs discovery against the latest verified local state, including
newly received records. It does not merely return an old saved note list.

Each result contains the wallet's **full current unspent note set**, with real
SDK witnesses. Replace the displayed set on each callback, or compare note IDs
to identify additions and removals. Updates can coalesce; this is a state
subscription, not a transaction-by-transaction history. Handle errors from both
the event stream and the asynchronous local read, as shown above.

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

## Sync and verification

- **First startup:** download the snapshot and subsequent changes, verify pool
  state, then save it. `ready` rejects if verification fails. `onEvent` reports
  startup stages and completed/total downloads.
- **Warm startup:** restore a trusted local cache without network access.
  Discovery then catches up from that state.
- **Live updates:** SSE sends a starting tail, then a new header and only appended
  records. Proofs also arrive as events. Reconnects, epoch rollovers and changed
  record prefixes replace the tail; a missing delta base triggers reconnect.
- **Trust:** WASM verifies the reconstructed storage root and contract proof
  against an accepted header from independent `rpcUrl`. Feed proofs add no trust
  in the indexer. Bootstrap and missed proofs use the feed's proof endpoint;
  historical blocks outside its retention window use `proofRpcUrl`.
- **Persistence:** changed state saves in the background; unchanged reads do not
  save again. `close()` flushes pending changes. Verification and serialization
  still execute in the single Worker.

Verification establishes state at a checkpoint, not the authenticity of earlier
write timestamps or Ethereum finality. Cold-discovered notes may need additional
maturity blocks before spending. The cache is trusted local storage: its checksum
detects corruption, not malicious replacement.

Snapshot/epoch downloads permit 30 seconds without progress and five minutes in
total; RPC/metadata requests have a 30-second deadline. For API details,
verification limits and source-build instructions, see the
[consumer specification](https://github.com/kfastov/strk20-indexer/blob/main/docs/spec/consumer-path.md)
and [repository](https://github.com/kfastov/strk20-indexer).
