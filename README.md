# strk20-indexer

**Discover private STRK20 notes without sending your viewing key to an indexer.**

`strk20-discovery` is a TypeScript SDK for wallets on Starknet. It downloads public
pool state, checks it against an independently selected blockchain checkpoint,
and discovers notes locally. The same Rust engine runs in a browser Worker or a
Node worker thread. A self-hostable Rust indexer publishes the shared feed.

[Try the live demo](https://strk20.nullref.cc/demo/) ·
[Watch the demo video](https://youtu.be/ngoYvunrOsI) ·
[SDK reference](ts/strk20-discovery/README.md) ·
[Demo workflow](docs/spec/demo-app.md) ·
[Self-hosting](docs/ops/hosting.md)

## Use the SDK

```sh
npm install strk20-discovery
```

```ts
import { LocalDiscoveryProvider } from 'strk20-discovery';

const discovery = new LocalDiscoveryProvider({
  network: 'mainnet',
  feedUrl: 'https://strk20.nullref.cc/mainnet/feed',
});
const mine = discovery.forAccount({ address, viewingKey });

await discovery.ready;
const cached = await mine.restore(); // discover from the verified local state
await discovery.subscribe();        // keep state current through SSE diffs
const { notes } = await mine.discoverNotes();

// On teardown:
await discovery.close();
```

`address` and `viewingKey` belong to the wallet integrating the SDK. They stay on
the client during local discovery. A Vite-compatible module Worker bundler is
required in the browser; Node 24+ uses `NodeDiscoveryProvider` from
`strk20-discovery/node`.

The provider implements the official `DiscoveryProviderInterface`, including
notes with spend witnesses, channels and requirement checks. Use
`discovery.atBlock(B)` to pin all builder reads to the same proving block.
The pinned official Privacy SDK ships with the package and is available through
`strk20-discovery/privacy-sdk`; consumers do not need a GitHub Packages token.

The maintained integration example is **[the demo](ts/demo)**. Its
[transaction flow](ts/demo/src/transactions.ts) passes our discovery results to
the official builder, so the note we find is the note used for the next spend.

## Try a real transaction

Open [the demo](https://strk20.nullref.cc/demo/), choose Sepolia or mainnet and
follow the primary action button:

1. Create a software wallet and export its backup.
2. Fund its displayed address with STRK and deploy the account.
3. Shield STRK, discover the note locally, transfer privately and withdraw.

The page shows the stages and timings of the requested wallet operation;
background synchronization and cache timings stay out of that log. An
optional comparison observes the same block through both discovery providers.
Enabling it explicitly sends this demo wallet's viewing key to the official
service. It does not send a second transaction.

Signing and viewing keys persist in a separate browser database; clearing the
discovery cache does not delete the wallet. Use small amounts and keep the backup.
The hosted proving service receives proving inputs: private local discovery does
not make every part of transaction creation private from that service.

The [demo workflow](docs/spec/demo-app.md) explains each action and receipt
recovery, and retains six historical transaction examples from Mainnet and
Sepolia. Those examples do not validate the current build.

## How it works

See the [architecture and dataflow diagram](docs/spec/architecture.md#data-path-and-trust-boundaries).

Every wallet consumes public pool artifacts rather than requesting its own
slots. The indexer needs no viewing key. Live ingestion includes state writes
that emit no pool event. The client checks the complete reconstructed storage
root, the pool's contract proof and the global state commitment against a header
from its configured RPC. Snapshot and incremental updates use the same verifier.

A folded cache preserves verified state, trie hashes and discovery progress.
Restart restores that state instead of replaying the full history. Fresh updates
still require verification; cold initialization, cache restoration and discovering
a new transaction are different workloads. A general speed advantage over the
official service is not established.

## Trust and current limits

- **State at a checkpoint, not every historical transition.** Verification covers
  the complete pool state at block B, including absent slots. It does not prove
  intermediate events or exact write timestamps. Note maturity uses a conservative
  verified-by block and can require extra waiting after a cold start.
- **The configured RPC is a trust root.** The default mode checks an accepted
  Starknet header; it does not establish Ethereum-finalized state independently.
- **The local cache is trusted.** Its checksum detects corruption, not malicious
  replacement. Cache encryption and authentication are not implemented.
- **Cold verification is still substantial.** WASM runs off the UI thread, but
  the first visit must download and verify state. Warm-start and fresh-discovery
  results must be evaluated separately.
- **Proof availability can delay updates.** The server distinguishes MATCH,
  MISMATCH and UNAVAILABLE. A mismatch aborts the current epoch cut; live-tail
  publication can continue. An unavailable proof permits unverified epoch cuts.
  The SDK must verify new state before using it for discovery.
- **Privacy has boundaries.** Feed hosts see IP addresses and request timing.
  Optional official comparison discloses its viewing key; hosted proving receives
  proving inputs. Raw/compat server modes have different privacy properties and
  are disabled by default in Compose.

## Run the indexer or contribute

```sh
docker compose up -d --build
```

Compose runs isolated mainnet and Sepolia services. For RPC configuration, backups,
health checks and public endpoint allowlists, see [hosting](docs/ops/hosting.md).
Public feeds are available at `/mainnet/feed` and `/feed` on
[the hosted instance](https://strk20.nullref.cc/demo/).

## Build from source

Use Node 24+, the Rust toolchain in `rust-toolchain.toml`, wasm-pack and Brotli:

```sh
./examples/mainnet/setup.sh       # builds the pinned upstream SDK
./crates/wasm/build.sh
npm --prefix ts ci
npm --prefix ts run dev          # Vite demo; no browser auto-open
```

For a production demo build, run `npm --prefix ts run build`. Rust workspace
checks, real-WASM/TypeScript tests and package validation are listed in
[CI](.github/workflows/ci.yml). The setup helper builds dependencies without
creating a wallet or submitting transactions.

- [Consumer architecture and proof contract](docs/spec/consumer-path.md)
- [Server architecture and feed interfaces](docs/spec/architecture.md)
- [Direct WASM API and build checks](crates/wasm/README.md)
- [Invariant checks](docs/ops/invariants.md)
- [Mainnet command-line examples](examples/mainnet/README.md)
- [Packaging-only upstream fork](docs/ops/fork.md)
- [Future work and known issues](https://github.com/kfastov/strk20-indexer/issues)

Rust, real-WASM/TypeScript, SDK compatibility and packaging checks validate the
implementation. The upstream discovery engine uses a feature-gated dependency
fork; CI checks that its source matches upstream. Its status and conditions for
returning to upstream are in the fork guide.

Apache-2.0. Upstream provenance and notices ship with the applicable packages and
[fixtures](fixtures/upstream/PROVENANCE.md).
