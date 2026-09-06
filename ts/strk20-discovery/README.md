# strk20-discovery

STRK20 discovery over public pool state. The Worker downloads the snapshot and
incremental diffs, verifies the complete pool storage root at a trusted checkpoint,
and runs upstream discovery locally. Viewing keys and account-specific reads stay
in the browser. The feed still sees IP addresses and request timing.

## Build

This package is not published to npm. From the repository root:

```sh
# Node 24+, Rust and wasm-pack; the SDK is built from its pinned public tag.
./examples/mainnet/setup.sh
./crates/wasm/build.sh
cd ts
npm ci
npm run build --workspace strk20-discovery
```

The package includes its Worker and WASM assets. Its official SDK dependency is
built by `examples/mainnet/setup.sh` into an ignored vendor directory. Build
from the repository, then consume the package
with a bundler that supports module Workers, such as Vite.

## Use

```ts
import { LocalDiscoveryProvider } from 'strk20-discovery';

const discovery = new LocalDiscoveryProvider({
  network: 'mainnet',
  feedUrl: 'https://strk20.nullref.cc/mainnet/feed',
});
const mine = discovery.forAccount({ address, viewingKey });

// Restore previously verified results immediately, without network access.
const cached = await mine.restore();
// Catch up and verify against a newly selected accepted RPC checkpoint.
const { notes, cursor } = await mine.discoverNotes();
await discovery.subscribe();
```

`LocalDiscoveryProvider` implements the official `DiscoveryProviderInterface`:
`discoverNotes`, `discoverChannels` and `discoverRequirement`. Notes contain actual
SDK `Witness` objects, channels contain token and note nonces, and cursors contain
real progress positions. The Worker owns the incremental cursor; caller-provided
cursors do not replace it. The returned note set contains all currently unspent
notes matching the token filter.

Pass the provider to `createPrivateTransfers({ discoveryProvider: discovery, ... })`.
For a proof builder, use `discovery.atBlock(blockNumber)` so every discovery method,
including requirement checks, uses the same proving block. Explicit numbers and
`latest` are supported; unsupported block tags fail rather than select another block.

## Node scripts

```ts
import { NodeDiscoveryProvider } from 'strk20-discovery/node';

const discovery = new NodeDiscoveryProvider({
  network: 'mainnet',
  feedUrl: 'https://strk20.nullref.cc/mainnet/feed',
  cacheDirectory: '/your/private/discovery-cache',
});
try {
  const mine = discovery.forAccount({ address, viewingKey });
  const cached = await mine.restore();
  const current = await mine.discoverNotes();
} finally {
  await discovery.close(); // flushes cache and terminates the worker thread
}
```

The Node host runs the same Worker runtime in `worker_threads`. It atomically
replaces a mode-0600 cache file; the directory is explicitly trusted and contains
private discovery results. No IndexedDB shim or second discovery implementation
is used. The headless lifecycle scripts use this provider for SDK proof inputs.

## Verification and persistence

The default trust root is an independently fetched, accepted Starknet RPC header.
The verifier checks its block hash and state commitment, contract Patricia path,
class hash, nonce and storage root, then compares the root of the complete locally
reconstructed pool state. `rpcUrl` and `proofRpcUrl` can be supplied separately.
A missing or invalid proof prevents discovery from an updated state.

This proves state at one block B. It does not authenticate intermediate history or
publisher-supplied last-write timestamps. SDK note `created` is the conservative
block by which the note value was verified, so a cold discovery may require extra
maturity blocks before spending. No Ethereum L1-finality claim is made.

The demo explicitly trusts its same-origin IndexedDB cache. Its SHA-256 checksum
catches corruption; it does not authenticate a malicious cache. The cache includes
folded storage, cached tree nodes, discovery cursors and witnesses. A failed candidate
cannot replace the last verified state. Clearing discovery cache must not delete
wallet signing or viewing keys. AEAD is deferred until after the main implementation.

SSE carries complete head and epoch payloads. The same decoder handles HTTP catch-up
following a gap or oversized event. Routine stream updates need no GET for their
data; independent checkpoint RPC requests are still necessary. Queues are bounded.
Explicit HTTP catch-up revalidates the mutable manifest instead of waiting for
the browser's cached copy to expire.

## Checks

```sh
npm run typecheck --workspace strk20-discovery
npm test --workspace strk20-discovery
npm run scan:chokepoint --workspace strk20-discovery
```

Tests run the actual compiled WASM against native discovery goldens, exercise both
cold modes, real SDK objects, cache-only restart, verification failures, decompression
limits and public request paths. The WASM smoke also checks key-buffer zeroization.
