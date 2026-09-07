# strk20-discovery

STRK20 discovery over public pool state. The Worker downloads the snapshot and
incremental diffs, verifies the complete pool storage root at a trusted checkpoint,
and runs upstream discovery locally. Viewing keys and account-specific reads stay
in the browser. The feed still sees IP addresses and request timing.

## Install

```sh
npm install strk20-discovery
```

Use a bundler with module Worker support, such as Vite, for browser applications.
Node applications require Node 24+. The release includes compiled Worker/WASM
assets and the unmodified official Privacy SDK 0.14.3-rc.5; no Rust toolchain,
GitHub Packages token or local vendor checkout is required to install it.

The official SDK is available through `strk20-discovery/privacy-sdk`, so the
builder and discovery provider share the same SDK classes and types:

```ts
import { LocalDiscoveryProvider } from 'strk20-discovery';
import { createPrivateTransfers } from 'strk20-discovery/privacy-sdk';
```

The [demo source](https://github.com/kfastov/strk20-indexer/tree/main/ts/demo)
is the maintained complete example, including signing, proving and recovery.

## Use

```ts
import { LocalDiscoveryProvider } from 'strk20-discovery';

const discovery = new LocalDiscoveryProvider({
  network: 'mainnet',
  feedUrl: 'https://strk20.nullref.cc/mainnet/feed',
});
const mine = discovery.forAccount({ address, viewingKey });

// Startup verifies and saves pool state on the first visit.
// With a verified cache, startup restores locally without network access.
await discovery.ready;
const cached = await mine.restore();
// Catch up and verify against a newly selected accepted RPC checkpoint.
const { notes, cursor } = await mine.discoverNotes();
await discovery.subscribe();
```

This startup contract applies from 0.1.1. Version 0.1.0 defers cold verification
until the first discovery call and defaults to the discontinued Lava mainnet
proof endpoint. Upgrade to 0.1.1, or explicitly set `proofRpcUrl` to
`https://api.cartridge.gg/x/starknet/mainnet` when using 0.1.0.
Proofs remain bound to the accepted header from the configured `rpcUrl`.
`ready` resolves only after a verified state is available. Cold startup includes
the initial download, complete state verification and cache save; missing or invalid
proofs reject it. A valid trusted cache restores without network access. The
`onEvent` callback receives `startup` events for engine loading, cache restoration,
feed metadata, checkpoint acquisition, data loading, verification, saving and readiness.
Data loading reports completed/total files. These are work stages, not elapsed-time
percentages; verification remains one synchronous WASM operation in the Worker.

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
is used. The demo is the maintained integration example; standalone lifecycle examples
are not the primary supported entry point.

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
the browser's cached copy to expire. Bounded reads reuse already staged artifacts
but still verify their independent checkpoint. `await discovery.waitForBlock(B)`
waits for an advertised SSE head at or above B (120-second timeout, cancelled on
close); it is an availability hint, not a verification result. Use a subsequent
`discoverNotes(..., { blockIdentifier: B })` to obtain verified notes. A foreground
read waiting for B takes priority over background checkpoint selection, so the
same update does not trigger proofs for two different blocks.

## Checks

```sh
npm run typecheck --workspace strk20-discovery
npm test --workspace strk20-discovery
npm run scan:chokepoint --workspace strk20-discovery
```

Tests run the actual compiled WASM against native discovery goldens, exercise both
cold modes, real SDK objects, cache-only restart, verification failures, decompression
limits and public request paths. The WASM smoke also checks key-buffer zeroization.

## Build and package from source

From the repository root:

```sh
./examples/mainnet/setup.sh
./crates/wasm/build.sh
npm --prefix ts ci
npm --prefix ts run pack:release --workspace strk20-discovery
```

The archive is written to `ts/strk20-discovery/release/`. The packaging script
replaces the development-only SDK path with the exact bundled version and keeps
public transitive dependencies as normal npm dependencies. Publish this archive,
not the development workspace directory. Upstream attribution is in `UPSTREAM.txt`
and the bundled package's license. The repository's Apache-2.0 license also ships.

Maintainer releases use `.github/workflows/publish-sdk.yml` on `main`. Configure
the package's npm trusted publisher for GitHub user `kfastov`, repository
`strk20-indexer`, workflow `publish-sdk.yml`, with direct `npm publish` allowed.
After committing and pushing a new package version and its lockfile, run:

```sh
gh workflow run publish-sdk.yml --ref main
```

The GitHub-hosted runner builds WASM and the pinned official SDK, runs tests and
the isolated consumer check, then publishes the tested archive with OIDC and
provenance. No npm token or local npm login is needed for this workflow. A
previously published version cannot be overwritten; inspect the run and registry
before retrying an uncertain publication.
