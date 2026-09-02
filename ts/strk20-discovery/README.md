# strk20-discovery

Keyless STRK20 note discovery. The viewing key stays in your application, which
fetches a public verified feed any mirror can serve and folds it locally by
running the upstream `strk20-consumer` engine, compiled to WebAssembly, in the
same process. The fetch plan is a pure function of the feed's `manifest.json`,
so clients holding different keys issue the same requests in the same order and
the server learns nothing about who is asking. The caller pins chain identity
before a byte is requested, so a hostile mirror cannot switch the pool.

## Install

Not published to npm. Depend on the directory and build it:

```json
{ "dependencies": { "strk20-discovery": "file:../strk20-indexer/ts/strk20-discovery" } }
```

The WASM module is not bundled either. Build it with `cd crates/wasm &&
wasm-pack build --release --target web --out-dir pkg --out-name strk20_engine`,
copy `pkg/` next to your app, and hand the glue to the factory below. This
package never imports a URL itself; the host does the import and passes it in.

## KeylessClient

The default: the key stays here and so does the computation.

```ts
import { KeylessClient, staticAccount } from 'strk20-discovery';
import { wasmEngineFactory, type WasmGlue } from 'strk20-discovery/engine/wasm';

const client = new KeylessClient({
  feedUrl: 'https://feed.example.org/sepolia',
  network: 'sepolia',
  engine: wasmEngineFactory({
    loadGlue: () => import('./pkg/strk20_engine.js') as unknown as Promise<WasmGlue>,
  }),
});
const { notes, balances } = await client.getNotes(staticAccount('0x1234', viewingKey32));
```

## DelegatedClient

A different trust boundary: the key travels to a server you run, so it sits at a
separate import path. The wire calls are not built yet; the construction-time
refusals are, and a plaintext non-loopback `serverUrl` is rejected.

```ts
import { DelegatedClient } from 'strk20-discovery/delegated';

const delegated = new DelegatedClient({ serverUrl: 'https://sync.internal', network: 'sepolia' });
await delegated.verifyChainIdentity();
```

## Errors

Everything thrown is a `Strk20Error` with a code from a closed union, faults
inside the WASM adapter included.

```ts
import { isStrk20Error } from 'strk20-discovery';

try { await client.sync(); }
catch (e) { if (isStrk20Error(e)) report(e.code, e.retryable); else throw e; }
```

## TypeScript

Floor is 5.6. The published typings avoid the generic `Uint8Array<T>` form
introduced in 5.7, so `skipLibCheck` is not required.

## Not yet

- Not an SDK `DiscoveryProviderInterface`: `discoverNotes`, `discoverChannels`
  and `discoverRequirement` are not implemented. `client.provider()` returns a
  smaller shape of this package's own.
- Notes carry no witness, so they cannot fund a spend. Balances and history only.
- No round-trippable cursor and no history: `export_reference_cursor` throws
  `SESSION_INCOMPLETE` and `history()` throws `HISTORY_UNAVAILABLE`.
- Not published to npm. Licensed under Apache-2.0, like the rest of the repo.
