# Consumer path

This replaces the earlier mock/delegated/step-budget design. Historical plans
remain in Git; the implemented public API is documented in
[`ts/strk20-discovery/README.md`](../../ts/strk20-discovery/README.md).

## Ownership

- `strk20-feed`: canonical codecs, hash chain, complete storage trie and checkpoint proof verification.
- `strk20-consumer`: folded state, upstream discovery, cursors, note registry and SDK data.
- `strk20-engine`: synchronous WASM staging, candidate verification and folded-cache persistence.
- TypeScript Worker: public network requests, decompression, serialized commands, SSE and IndexedDB.
- `LocalDiscoveryProvider`: actual official SDK interface and account-bound convenience methods.
- Demo: wallet custody, transaction orchestration, user operations and timing display.

## State authentication

Select an independent trusted checkpoint `(network, pool, B, blockHash, stateRoot)`.
The default host obtains an accepted block header from a configured Starknet RPC.
The browser and Node hosts fetch the header and contract storage proof concurrently
for the same block number. They then verify:

1. Proof block hash equals the selected checkpoint hash.
2. Global state commitment matches the contracts and classes roots.
3. The pool address's complete Patricia path reaches its contract leaf.
4. Class hash, storage root and nonce hash to that leaf.
5. The complete locally reconstructed pool storage root at B equals the proved root.

Snapshot basis S must satisfy S ≤ B ≤ feed head H. Check the small proof before
cold folding. Hash the complete pool state once at B, then cache Patricia nodes
and update affected paths on subsequent state changes. Neither publisher
sidecars nor repeated checks of roots chosen by the publisher establish the
trusted checkpoint. Sidecars remain server diagnostics and are not downloaded
by the browser discovery path.

This authenticates state at B, including absent storage slots. It does not prove
intermediate events, exact write times or freshness beyond B. A historical
completeness claim would require authenticated transition/event coverage from
another authority. The default RPC checkpoint is not an Ethereum L1-finalized
checkpoint. Such a mode must pin the finalized Ethereum block used for all core
contract reads, verify the resulting Starknet checkpoint, and label its delay.

## Atomic application and local cache

The WASM facade folds into a candidate copy. Only successful complete-state
verification promotes it. A malformed update, invalid proof or root mismatch
blocks fresh discovery while the previous verified cache remains exportable.
Format errors are not silently converted into full replay success.

Cache version `S20FOLD2` holds manifest identity, folded storage and events,
cached trie nodes, notes and real discovery cursors. Load restores these directly;
it does not replay epochs or recompute every Pedersen hash. The same-origin cache
is explicitly trusted. SHA-256 detects accidental corruption, not malicious
replacement. AEAD is the next persistence task after the main implementation.

The restart target is ≤2 s for engine restoration and cached discovery results.
Network catch-up and the first uncached visit are measured separately.
Wallet signing and viewing keys are kept outside the disposable cache database.

## Discovery and spending

The official `DiscoveryProviderInterface` is implemented without placeholder
cursors or witnesses. Notes carry SDK `Witness(channelKey, index, salt)` values;
channels contain actual token indices and next-note nonces. An account-bound
facade stores `address` and `viewingKey` once. The Worker owns incremental state.
All three discovery methods can be pinned to one proving block via `atBlock(B)`.

For SDK maturity checks, `created` is a conservative verified-by block for that
note value. Publisher write timestamps remain separate, explicitly unverified
metadata. A cold discovery may therefore have to wait additional blocks before
a spend. Cursor caches are bound to the viewing key and rewound on tail changes
or when selecting a lower checkpoint.

## Subscription

SSE starts with a canonical head payload and the latest immutable epoch payload.
Subsequent `head` events carry `delta: {base_etag, header, append, end}` when the
tail's record prefix is unchanged: the header/trailer retain their newlines and
`append` contains only new complete NDJSON records. `payload` is null in a delta.
The SDK assembles every delta before coalescing verification jobs. The resulting
bytes pass through the same Rust parser and checkpoint verification as HTTP.
An unknown base causes reconnect; reconnect, epoch rollover or a changed record
prefix sends a full current head. Oversized artifacts use HTTP catch-up. Deltas
are computed against the last state sent on that connection, so coalesced server
publications need no replay journal or client-supplied cursor. Legacy clients
can use their existing HTTP fallback for null payloads. Event IDs hash the actual
event content. Proofs arrive separately over SSE; independent header RPC checks
remain necessary. Normal updates need no follow-up data GET.

## Verification

- Real mainnet contract proof fixture with negative path/leaf/global-root cases.
- Cached-trie mutation tests against the independent complete-root implementation.
- WASM/native SDK equality for snapshot and epoch starts, cache-only restore,
  key zeroization and failed-candidate isolation.
- Native acceptance, SQLite/memory conformance, snapshot and SSE integration tests.
- Worker tests use actual compiled WASM with fixture HTTP and IndexedDB.
- Test child logging is explicit; servers bind port zero and report the port
  already reserved by the kernel, avoiding races between parallel tests.
