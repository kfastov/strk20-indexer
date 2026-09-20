# Consumer path

This is the state and trust contract of the browser/Node provider. See the
[SDK README](../../ts/strk20-discovery/README.md) for installation, application
examples and lifecycle methods, and the [WASM README](../../crates/wasm/README.md)
for direct engine calls. Component ownership and the public feed wire format are
in [Architecture](architecture.md).

## State authentication

Select a trusted checkpoint `(chain_id, pool, block_number, block_hash, state_root)`
independently of the feed. The default TypeScript host obtains an accepted header
from `rpcUrl`, checks its chain ID, and acquires a proof for the same block B.
Proofs normally come from feed SSE or `/feed/proofs/{block}`. HTTP 410 outside
that endpoint's retention window falls back to `proofRpcUrl`; `proofSource: "rpc"`
uses the configured proof RPC directly. Other feed proof failures are surfaced,
not silently treated as permission to skip verification.

[`verify_checkpoint`](../../crates/feed/src/checkpoint.rs) checks:

1. The proof's block hash equals the selected checkpoint hash.
2. Its contracts/classes roots produce the checkpoint's global state commitment.
3. The pool's complete Patricia path reaches the contract leaf.
4. The class hash, storage root and nonce produce that leaf.

Then [`verify_state`](../../crates/consumer/src/anchors.rs) checks that the complete
locally reconstructed pool storage root equals the proved root at B. The snapshot
basis S must satisfy S ≤ B ≤ feed head H. The host checks the small proof before
cold folding. The complete storage trie is hashed at the checkpoint; cached nodes
allow subsequent checks to update affected paths. Discovery reads at the verified
checkpoint, even if the folded feed extends beyond it.

This authenticates the pool's state at B, including absent slots. It does not
prove intermediate events, exact write times, every historical transition, or
freshness beyond B. A self-consistent hash chain cannot establish those claims.
Publisher anchors and proof sidecars do not select the browser's trust root.
The default accepted Starknet RPC header is itself trusted: no Ethereum finality
verification or L1-finalized checkpoint mode is implemented by this host.

## Applying updates

[`apply_feed`](../../crates/consumer/src/apply.rs) validates feed identity, snapshot
and epoch content, ordered hash links and head consistency. A newly finalized
epoch replaces prior tail rows in its range. A changed tail rewinds the affected
rows and discovery progress; the tail generation is persisted with the change.
A changed already-applied epoch hash is a divergence error, requiring an explicit
choice of feed or a new local bootstrap rather than silently accepting rewritten
history.

The WASM facade folds into a candidate copy. Only successful checkpoint and
complete-state verification promotes it. Once checkpoint staging or application
fails, discovery on that engine is blocked until successful application; the
previously verified state remains exportable. Network failures before staging
leave the last verified state available for cached reads. Neither case verifies
fresh state. Malformed updates and root mismatches are not converted into replay
success.

On a cold start, `auto` uses an advertised snapshot, or epochs when none is
advertised. `snapshot` requires one; `epochs` requests replay. An invalid snapshot
fails rather than silently falling back. Snapshots omit earlier events, so
pre-basis reads fail and complete transaction history cannot be reconstructed
from a snapshot alone. Replaying epochs supplies the publisher's earlier records;
it still does not cryptographically authenticate every historical transition.

## Cache restoration

The `S20FOLD2` container preserves the applied manifest, folded storage/events,
cached trie nodes, notes and discovery cursors. Loading it restores state directly,
without replaying all epochs or recomputing all trie hashes. The codec checks size,
format, checksum and feed identity. Browser storage is IndexedDB; Node uses local
cache files. A valid verified cache lets `ready` resolve without network access.
Cold startup downloads, verifies and saves state before reporting readiness.

The local cache is explicitly trusted and contains private discovery results and
witnesses. SHA-256 detects accidental corruption, not malicious replacement.
Encryption/authentication and a cache key-management mechanism are not implemented.
Wallet custody belongs to the application; the demo keeps signing and viewing
keys in a separate database from its disposable discovery cache.

Cache revisions avoid saving unchanged reads. Changed state saves in the
background and `close()` flushes it. Serialization and verification still execute
synchronously in one Worker; they can delay other Worker work. Warm restoration,
network catch-up and cold verification are different workloads, with no guaranteed
latency bound or general speed advantage over a remote discovery service.

## Discovery and spending

[`sdk.rs`](../../crates/consumer/src/sdk.rs) adapts the upstream engine to the
provider. Notes carry SDK spend witnesses; channels expose token indices and
next-note nonces. Cursors are bound to the viewing key and rewound when the tail
changes or a lower checkpoint is selected. The Worker owns this progress; a
caller-supplied SDK cursor does not replace it. `atBlock(B)` pins all builder
reads, including requirement checks, to one proving block.

SDK `created` is a conservative verified-by block (`knownByBlock`) for that note
value. Publisher write timestamps remain separate, unverified metadata
(`reportedWriteBlock`). Cold-discovered notes may need extra maturity blocks
before spending. An account's cached restore runs local discovery on saved state;
it neither catches up nor establishes current spendability.

## Live updates

SSE and HTTP feed bytes use the same Rust parser and checkpoint verifier. The
host assembles every head delta against its stated base before coalescing sync
jobs. A missing base reconnects; gaps or oversized payloads require HTTP catch-up.
The server's current-state reconnect burst is not historical event replay.
Proof events remain untrusted inputs until matched to an independent header.
A normal covered update need not fetch its data again through HTTP.

The provider reconnects after transport failures. Chain mismatch or a publisher
verification-failure status terminates the subscription with an error. A `head`
event and `waitForBlock(B)` mean the feed advertises B; only a verified `state`
event or successful discovery establishes usable state. Subscriptions update
public state; applications call account discovery to refresh private results.
The native CLI also has an SSE/polling path; it is not the browser Worker protocol.

## Native client boundary

`strk20-sync sync` folds a feed into SQLite and discovers locally. Independent
checkpoint verification is opt-in through `--verify-anchor <rpc>`; when configured,
it must succeed. `verify-anchors` only compares local state with publisher anchors.
The separate `verify --rpc` command checks discovered note/nullifier slots and
does not establish completeness of every pool slot. Do not apply the default
browser verification guarantee to an unconfigured native sync. The command
source is [`crates/client/src/main.rs`](../../crates/client/src/main.rs).

## Validation

The [CI workflow](../../.github/workflows/ci.yml) runs Rust build/test/lint,
[invariant checks](../ops/invariants.md), actual compiled WASM/TypeScript tests,
and an isolated install/build of the release archive. Relevant coverage includes
[checkpoint rejection cases](../../crates/feed/tests/checkpoint.rs),
[native integration tests](../../crates/e2e-tests/tests),
[WASM cache and candidate isolation](../../crates/wasm/test/smoke.mjs), and
[Worker/SDK tests](../../ts/strk20-discovery/test). These tests establish their
specified behavior, not independent chain finality or performance on every host.
