# strk20-engine

Synchronous WASM bindings for local STRK20 discovery. The host supplies public
feed bytes, decompression and an independently selected checkpoint. The engine
verifies candidate state before exposing discovery results. It performs no HTTP
or filesystem I/O. Run it in a Worker to keep verification off the UI thread.

For application integration, use the [TypeScript SDK](../../ts/strk20-discovery/README.md).
This document covers direct module use. The verification guarantees and cache
trust boundary are defined in [Consumer path](../../docs/spec/consumer-path.md).

## Build and check

From the repository root, with the toolchain in
[`rust-toolchain.toml`](../../rust-toolchain.toml), wasm-pack, Node 24+ and Brotli:

```sh
rustup target add wasm32-unknown-unknown
./crates/wasm/build.sh
```

The script generates fixture data and native SDK goldens, builds the web-target
module into `crates/wasm/pkg`, reports compressed module/glue sizes, audits imports
and runs the actual WASM smoke test against the native results. The SDK's host and
JavaScript zstd decoder add to the module/glue size; those two files alone are not
the installed SDK's total cost.

For just the module build, run in `crates/wasm`:

```sh
wasm-pack build --release --target web --out-dir pkg --out-name strk20_engine
```

Generated bindings are `pkg/strk20_engine.js`, `pkg/strk20_engine.d.ts` and
`pkg/strk20_engine_bg.wasm`. The npm SDK build copies these existing assets; it
does not compile Rust. Rebuild them after changing the Rust consumer or engine.

## Call order

1. Initialize the generated module, then construct `new Engine(genesisJson)`.
   Pin the intended feed identity in the host; do not derive the trusted identity
   solely from an arbitrary feed response.
2. Stage a manifest and checkpoint. `stage_checkpoint` verifies the contract
   proof against the independently acquired header before expensive folding.
3. Download the required snapshot and/or epochs plus the current head. Decompress
   zstd in the host with a size limit. Stage uncompressed epoch bytes; for a
   snapshot, stage both compressed bytes and its uncompressed payload.
4. Call `apply("auto")`, `apply("snapshot")` or `apply("epochs")`. A successful call
   folds and verifies a candidate and makes it the current state.
5. Call `discover`, `channels` or `requirement` with fresh viewing-key buffers.
6. Persist `export_state()` as private trusted cache data and call `free()` when
   the engine is no longer needed.

On an update, stage the new manifest, missing epochs, replacement head and a new
checkpoint, then apply again. An explicit read at the already verified checkpoint
can reuse that state. A lower checkpoint cannot precede the snapshot basis.
The `auto` mode uses an advertised snapshot on an empty store, otherwise epochs;
invalid advertised data is an error. It is not a fallback from failed verification.

This Node example uses only generated test fixtures and can be saved as an `.mjs`
file in `crates/wasm` after running `build.sh`:

```js
import { readFileSync } from 'node:fs';
import init, { Engine } from './pkg/strk20_engine.js';

const bytes = (name) => readFileSync(new URL(`./fixture/${name}`, import.meta.url));
const text = (name) => bytes(name).toString();
await init({
  module_or_path: readFileSync(new URL('./pkg/strk20_engine_bg.wasm', import.meta.url)),
});

const engine = new Engine(text('genesis.json'));
try {
  engine.stage_manifest(text('manifest.json'));
  engine.stage_checkpoint(text('checkpoint.json'), text('proof.json'));
  engine.stage_snapshot(0n, bytes('snapshots/0.zst'), bytes('snapshots/0.ndjson'));
  engine.stage_head(bytes('head.ndjson'), 'fixture-head');
  const applied = JSON.parse(engine.apply('snapshot'));
  console.log(applied.verifiedAt);

  const owner = JSON.parse(text('owners.json'))[0]; // public test identity
  const key = Uint8Array.from(Buffer.from(owner.key, 'hex'));
  const notes = JSON.parse(engine.discover(owner.owner, key));
  console.log(notes.notes.length);
} finally {
  engine.free();
}
```

A live host replaces fixture reads with public downloads and independent header
selection. Copying the feed's claimed root into a checkpoint would remove that
trust boundary. [`worker.ts`](../../ts/strk20-discovery/src/worker.ts) is the
maintained host implementation.

## Exported API

Methods are defined in [`src/lib.rs`](src/lib.rs); generated `.d.ts` bindings are
the JavaScript signature reference. Rust `u64` parameters use JavaScript `bigint`.
JSON-returning methods return strings, which the host parses.

| Method | Input and result |
|---|---|
| `new Engine(genesisJson)` | JSON feed identity; creates empty, unverified state. |
| `Engine.version()` | Engine package version string. |
| `stage_manifest(json)` | Current manifest JSON. |
| `stage_epoch(epoch, payload)` | Epoch index as `bigint` and uncompressed canonical bytes. |
| `stage_snapshot(epoch, compressed, payload)` | Snapshot index, compressed bytes and decompressed bytes. |
| `stage_head(payload, etag)` | Complete head NDJSON and a validator that changes with its bytes. |
| `stage_checkpoint(checkpointJson, proofJson)` | Checkpoint fields `chain_id`, `pool`, `block_number`, `block_hash`, `state_root`; proof accepts the RPC result object or its JSON-RPC envelope. |
| `apply(mode)` | `auto`, `snapshot` or `epochs`; returns JSON with `head`, `verifiedAt`, `tail_rewound`, `epochs_applied`, `snapshot_basis`. |
| `info()` | JSON identity, head/basis, checkpoint and verification status. `verified` is `unverified`, `rpc-verified` or `failed`. |
| `discover(owner, key)` | Owner hex string and mutable 32-byte key; returns JSON notes, incoming cursor and completeness report. |
| `channels(owner, key, recipientsJson)` | Recipients are a JSON array of hex addresses or `null` for all; returns channel data as JSON. |
| `requirement(owner, key, recipient, token)` | Hex addresses/token and key; returns the SDK requirement code as a number. |
| `forget_owner(owner)` | Drops that owner's cursors and note registry; keeps public state. |
| `cache_revision()` | Revision used by the host to avoid unchanged cache writes. |
| `export_state()` | Serialized verified cache as `Uint8Array`. Can export the previous good state after candidate failure. |
| `Engine.load(bytes, genesisJson)` | Restores a trusted local cache directly, with no staging or replay required. |
| `free()` | Releases this engine's WASM allocation. |

Discovery JSON schemas are represented by `DiscoveryResult` and `ChannelResult`
in the [SDK types](../../ts/strk20-discovery/src/types.ts). Conversion to official
SDK `Note`, `Channel`, `Witness` and cursor types belongs to the
[provider adapter](../../ts/strk20-discovery/src/provider.ts).

## Persistence and errors

The [`S20FOLD2` codec](src/blob.rs) checks format, checksum, size and feed identity.
The cache includes private notes, witnesses and discovery cursors. Its checksum
is not authentication: accept it only from trusted local storage. A cache version
or identity error requires a deliberate cold bootstrap; preserve the independent
wallet backup. Loading a valid cache does not verify chain freshness.

Failures throw JavaScript `Error` objects whose `message` is JSON:

```json
{"code":"CHECKPOINT_REQUIRED","message":"...","details":{},"retryable":false}
```

The complete mapping lives in [`err.rs`](src/err.rs). Relevant groups are:

| Codes | Host action |
|---|---|
| `NOT_STAGED`, `CONFIG_INVALID` | Correct staging or call arguments. |
| `CHECKPOINT_REQUIRED`, `CHECKPOINT_FAILED`, `CHECKPOINT_STATE_MISMATCH`, `PROOF_MALFORMED` | Require a valid checkpoint/application before fresh discovery. Preserve the previous good cache; do not downgrade verification. |
| `CHECKPOINT_AHEAD`, `BOUND_BELOW_SNAPSHOT` | Select a covered block, wait for coverage, or explicitly bootstrap the required history. |
| `FEED_HASH_MISMATCH`, `FEED_CHAIN_BROKEN`, `FEED_MALFORMED`, `SNAPSHOT_ROOT_MISMATCH`, `DECOMPRESS_LIMIT` | Reject the artifact; investigate transport/feed integrity. |
| `STATE_CORRUPT`, `STATE_VERSION`, `STATE_FOREIGN` | Reject this cache and choose a cold start for the intended identity. |
| `KEY_INVALID` | Supply a valid fresh viewing-key buffer. |

Only `FEED_ADVANCED_MIDSYNC` is marked `retryable` by the WASM error mapper. The
TypeScript host separately handles a missed epoch with one metadata catch-up.
Do not infer that every error is transient. After checkpoint staging or apply
fails, discovery is blocked until a successful apply, while export retains the
previous verified state.

## Key handling

Key inputs are mutable 32-byte big-endian buffers. The engine zeroizes the input
buffer on success and rejection and uses `SecretFelt` internally. Supply a new
buffer for each call. The host must also clear its own copies; immutable JavaScript
strings and bigints cannot be reliably erased. `free()` is resource cleanup, not
a guarantee that every host copy or cached witness has been erased. Keep keys out
of URLs, logs and public feed transport.
