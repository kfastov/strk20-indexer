# Q14 + Q15 — Compatible mode & keyless client library

Investigation date: 2026-08-29. Sources: local clones only unless marked (web).
- RC.0 tree (matches originally-deployed contract per upstream README): `/private/tmp/claude-501/-Users-konstantinfastov-Projects-strk20-indexer/b9b259a5-132a-4a96-b7c3-68d3231f50a6/scratchpad/starknet-privacy-rc0` (tag `PRIVACY-0.14.3-RC.0`) — abbreviated below as `$RC0`
- main tree (@ 980da8a, 2026-08-26): `.../scratchpad/starknet-privacy` — abbreviated `$MAIN`
- hackathon repo: `.../scratchpad/strk20-hackathon` — abbreviated `$HACK`

---

## Q14 — What should compatible mode be?

### 14.1 Exact HTTP API surface (VERIFIED)

Router, quoted from `$RC0/crates/discovery-service/src/api/mod.rs:103-112`:

```rust
let api_router = Router::new()
    .route("/health", get(health_handler::<B>))
    .route("/v1/sync/incoming_state", post(incoming_sync_handler::<B>))
    .route("/v1/sync/outgoing_state", post(outgoing_sync_handler::<B>))
    .route("/v1/sync/preflight_check", post(preflight_check_handler::<B>))
    .route("/v1/history", post(history_handler::<B>))
    .with_state(app_state);
```

5 routes total. Optional OHTTP (RFC 9458) envelope layer installed as the router fallback (`mod.rs:120-132`): a `POST /` with `Content-Type: message/ohttp-req` is decapsulated and re-routed through the same router; plaintext requests bypass it. CORS is permissive; body limit `max_request_body_bytes` (default 102,400 bytes); request timeout layer; optional TLS via rustls.

Request/response types (`$RC0/crates/discovery-service/src/api/types.rs`):

- `SyncRequestBase` (`types.rs:151-180`), flattened into both sync requests. JSON fields at top level:
  - `contract_address: Felt`
  - `viewing_key: SecretFelt` (custom serde `secret_felt_serde`) — **the raw private viewing key travels in the request body**
  - `last_known_block: Option<Felt>` — reorg detection; 409 `BLOCK_REORGED` if reorged out
  - `block_ref: Option<BlockId>` (custom `block_id_serde`: hex hash string, number, or tag `"latest" | "pre_confirmed" | "l1_accepted"`)
  - `cursor: DiscoveryCursor` (defaults to empty)
- `IncomingSyncRequest` = `{ recipient_address: Felt } + base` → `IncomingSyncResponse { block_ref, channels: Vec<IncomingChannel>, subchannels: Vec<IncomingSubchannel>, notes: Vec<DecryptedNote>, cursor }` (`types.rs:206-229`)
- `OutgoingSyncRequest` = `{ sender_address: Felt, recipients: Option<HashSet<Felt>> } + base` → `OutgoingSyncResponse { block_ref, channels: Vec<OutgoingChannel>, subchannels: Vec<OutgoingSubchannel>, cursor }` (`types.rs:255-283`)
- `PreflightCheckRequest { contract_address, sender_address, viewing_key, recipient, token }` → `PreflightCheckResponse { block_ref, sender_registered, channel_exists, subchannel_exists }` (`types.rs:289-319`)
- `HistoryRequest { contract_address, user_address, max_transactions: u32, last_known_block?, block_ref?, cursor: HistoryCursor }` → `HistoryResponse { block_ref, transactions: Vec<HistoryTransaction>, cursor }` (`types.rs:322-358`)
- Error shape `{"error": {"code", "message", "details?"}}`; codes (`types.rs:361-372`): `INVALID_REQUEST, DECRYPTION_FAILED, BLOCK_REORGED, SERVICE_UNAVAILABLE, CONTRACT_NOT_FOUND, RPC_UNAVAILABLE, STORAGE_ERROR, INTERNAL_ERROR, OHTTP_DECAPSULATION_FAILED, OHTTP_INVALID_FORMAT`
- `GET /health` → `{status: "OK"|"UNHEALTHY", chain_head?: {block_number, block_hash, timestamp}, lag_secs}`. NOTE drift: at RC.0 a laggy-but-headed service returns HTTP 200 with `"UNHEALTHY"`; at main HEAD it returns 503 (diff in handlers.rs). SDK's `isHealthy()` only checks `body.status === "OK"`, so either behavior works with the SDK.

Spec `$RC0/crates/discovery-service/specs/06-api-design.md` documents all of the above verbatim (quoted extensively in section 14.1 sources), including the cursor JSON schema, completion semantics (`cursor.is_complete()` = `channel_discovery_complete` && every channel `subchannel_discovery_complete` && every subchannel `note_discovery_complete`), the 10-block note-maturity rule driven by note `block_number`, and validation limits (defaults, `config.rs:165-177`): `max_channels 256` (CursorLimits default), `max_outgoing_recipients 64`, `max_history_subchannels 256`, `max_history_transactions 100`, `server_budget 10_000`, `max_request_body_bytes 102_400`.

Server-side behaviors a compatible implementation must reproduce (all VERIFIED in handlers/validators):

1. **Viewing-key validation** (`validators.rs:196-248`): derive pubkey via `starknet_crypto::get_public_key(viewing_key)` (Stark-curve scalar mult), compare with on-chain registered key (storage slot read). Skip check when registered key is zero. Mismatch → 400 `INVALID_REQUEST`. Results cached (`PublicKeyCache`, moka, capacity 10k).
2. **block_ref resolution** (`validate_block_ref`): `None` → current head *hash*; `last_known_block` canonicity check → 409 `BLOCK_REORGED`.
3. **Note filtering**: incoming sync returns only unspent notes (nullifier derived and checked against storage).
4. **Note `block_number`** = the storage slot's `last_update_block` (write-once slots ⇒ creation block). Reference impl gets it from a **non-standard RPC extension**: `starknet_getStorageAt` with `response_flags: [IncludeLastUpdateBlock]` (`rpc_backend.rs:196-211`), a software-mansion starknet-rust fork feature (`StorageResponseFlag`, `GetStorageAtResult::ValueWithMetadata`). Public RPC providers generally do NOT support this — our own DB-backed indexer sidesteps it entirely (we know each slot's write block from indexed state diffs/events).
5. **I/O budget**: per-request `IoBudget::new(server_budget)` caps storage/event reads; partial results returned with an incomplete cursor.

### 14.2 Trait boundaries — can we run unmodified discovery-core over a local DB? (VERIFIED: yes)

`$RC0/crates/discovery-core/src/storage_backend.rs`:

```rust
#[async_trait]
pub trait RawStorageAccess: Send + Sync {
    async fn read_slot(&self, slot: Felt) -> Result<Felt, StorageError>;
    async fn read_slots(&self, slots: Vec<Felt>) -> Result<Vec<Felt>, StorageError>;
    async fn read_slots_with_block(&self, slots: Vec<Felt>) -> Result<Vec<StorageResult>, StorageError>;
}

#[async_trait]
pub trait StorageBackend: Send + Sync {
    type Snapshot: StorageSnapshot;
    async fn snapshot(&self, contract_address: Felt, block_id: Option<BlockId>)
        -> Result<Self::Snapshot, StorageError>;
}

#[async_trait]
pub trait StorageSnapshot: IViews {
    fn contract_address(&self) -> Felt;
    fn block_id(&self) -> BlockId;
}
```

`$RC0/crates/discovery-core/src/events_backend.rs:14-39`:

```rust
#[async_trait]
pub trait RawEventAccess: Send + Sync {
    async fn get_events(&self, keys: &[Vec<Felt>], from_block: BlockId, to_block: BlockId)
        -> Result<Vec<EmittedEvent>, StorageError>;
    fn block_id(&self) -> BlockId;
    fn block_number(&self) -> u64;
}
```

Key structural facts:

- **Blanket impls do all the heavy lifting**: `impl<T: RawStorageAccess> IViews for T` (`privacy_pool/views.rs:121`) provides all 15 contract-view methods (channel/subchannel/note/nullifier/pubkey reads with slot derivation via `storage_slots.rs`); `impl<T: RawEventAccess> IEvents for T` (`privacy_pool/events.rs:222`) provides typed event access (Deposit/Withdrawal/EncNoteCreated/OpenNoteDeposited/ViewingKeySet). We implement ONLY the two raw traits (3 async fns + 3 fns) and the engine's whole view/decrypt/scan logic comes for free.
- **Object safety**: `RawStorageAccess`, `RawEventAccess`, `IViews`, `IEvents`, `StorageSnapshot` use `#[async_trait]` (boxed futures) with no generic methods → object-safe, `dyn`-usable. `StorageBackend` has an associated type `Snapshot` → not object-safe, but it is only ever used generically (`ApiServer<B: StorageBackend + ChainState + Clone ...>`, `mod.rs:70-73`), so this doesn't matter.
- **All async** via `async_trait`; bounds `Send + Sync`; snapshot additionally needs `Clone + Send + Sync + 'static` for the API server.
- The engine entrypoints are plain generic async fns — no runtime dependency:
  - `sync_incoming_state<S: IViews>(pool, recipient, viewing_key, cursor, cursor_limits, budget)` (`sync/incoming_state.rs:69`)
  - `sync_outgoing_state<S: IViews>(pool, sender_addr, viewing_key, cursor, cursor_limits, budget, recipients)` (`sync/outgoing_state.rs:76`)
  - `preflight_check<S: IViews>(...)` (`sync/preflight_check.rs:34`)
  - `fetch_transactions<B: IViews + IEvents>(backend, user_address, &mut cursor, max_transactions, budget)` (`history/transactions.rs:31`)
- Concurrency inside the engine uses `futures::stream::FuturesUnordered` (runtime-agnostic); **no tokio outside `#[cfg(test)]`** (verified by grep — only `#[tokio::test]` hits).
- One extra trait lives in the *service* crate, not core: `ChainState { get_head, set_head, is_canonical }` (`discovery-service/src/chain_state.rs:26-39`) — needed if we reuse the service's `ApiServer`/handlers; trivially implemented over our DB head row.

**What a Postgres/SQLite impl needs** (compatible-mode `DbBackend` + `DbSnapshot`):

1. Table `storage(contract, slot, value, write_block)` append-only (or slot→latest for the write-once slots plus history for mutable ones). `read_slot(s)` = value with max `write_block <= snapshot_block`, defaulting to `Felt::ZERO` when absent (Cairo map semantics — MockBackend does exactly this, `storage_backend.rs:118`). `read_slots_with_block` returns `StorageResult { value, last_update_block }` — trivially our `write_block`. This is where a DB backend is *better* than RPC: no fork-only RPC extension needed.
2. Table `events(block_number, tx_hash, event_index, transaction_index, key0..k, keys[], data[])` filtered by per-position key sets (`RawEventAccess::get_events` semantics: event matches if for every non-empty filter set, `event.keys[position] ∈ set`; mirror `MockEventBackend`'s filter, `events_backend.rs:82-99`). Must return `starknet_core::types::EmittedEvent` (fork type — includes `event_index`, `transaction_index`).
3. `snapshot()` resolves `BlockId` (hash/number/tag) → concrete `block_number` against our indexed chain table; `Tag(Latest)` = our head; return `ContractNotFound` if the contract isn't the indexed pool.
4. `ChainState`: head row (number, hash, timestamp) maintained by the ingest loop; `is_canonical(hash)` = hash exists in our canonical blocks table.
5. `StorageError::Backend(Box<dyn Error + Send + Sync>)` wraps DB errors — no error-type friction.

**Verdict (VERIFIED): compatible mode = implement `RawStorageAccess` + `RawEventAccess` + `StorageSnapshot` + `StorageBackend` + `ChainState` over the local DB, then run the unmodified `discovery-core` engine — zero forked logic.** Two reuse depths are possible:
- *Max reuse*: depend on `discovery-service` as a library too (it has `[lib] name = "discovery_service"`, Cargo.toml:6-8) and instantiate its `ApiServer<DbBackend>` — the entire wire format, validators, error mapping, OHTTP layer come for free. Cost: inherits its axum/rustls/tower-ohttp dep tree (tower_ohttp is a git dep on starkware-libs/sequencer).
- *Medium reuse*: own thin axum layer calling `discovery_core::sync::*` + copy of `api/types.rs` serde types (~350 lines, stable). Keeps deps lean; wire format must be conformance-tested (assets in Q15.3).

### 14.3 Is discovery-core consumable as a dependency? (VERIFIED, with caveats)

- `$RC0/crates/discovery-core/Cargo.toml`: `name = "discovery-core", version = "0.1.0", edition = "2021"` — **no `license` field, no `publish` flag; path-only workspace member** (`$RC0/Cargo.toml` workspace members: discovery-core, discovery-service). Repo root `LICENSE` is **Apache-2.0** (verified head of file) → redistribution/forking permitted with attribution.
- **Not on crates.io and cannot be published there as-is**: it depends on git dependencies (`starknet-core/-crypto/-providers = { package = "starknet-rust-*", git = "https://github.com/software-mansion/starknet-rust.git", rev = "7caedfe" }`), and crates.io rejects git deps (VERIFIED Cargo.toml; the crates.io-policy point is standard cargo behavior). Consumption paths: (a) `discovery-core = { git = "https://github.com/starkware-libs/starknet-privacy", tag = "PRIVACY-0.14.3-RC.0" }` — cargo resolves workspace-member crates inside a git repo by package name (INFERRED-mechanical, standard cargo; the repo being a mixed Scarb+Cargo tree doesn't block this since the root Cargo.toml is a proper cargo workspace); (b) vendored fork.
- **Version pinning hazard is low**: `git diff PRIVACY-0.14.3-RC.0..PRIVACY-0.14.3-RC.5 -- crates/discovery-core` is **empty** (VERIFIED). RC.0..main HEAD touches only `privacy_pool/views.rs` (+47/-4: a chunking refactor to `as_chunks` — needs Rust ≥1.88 — plus a new unit test); the engine (`discovery/`, `sync/`, `history/`), slot derivation, and hashes are byte-identical across all RC tags and main. Rustc 1.95.0 locally compiles both.
- Our crate must use the **same starknet-rust fork rev 7caedfe** for type identity (`Felt`, `BlockId`, `EmittedEvent`, `StorageResult` are fork types; `StorageResult`/`StorageResponseFlag`/`AddressFilter` don't exist in mainline starknet-rs).

### 14.4 Service ↔ SDK version match (VERIFIED: wire-compatible)

- SDK at main = published `@starkware-libs/starknet-privacy-sdk@0.14.3-rc.5` (package.json; GitHub Packages registry). Its `IndexerDiscoveryProvider` posts to exactly the service's paths: `/health`, `/v1/sync/incoming_state`, `/v1/sync/outgoing_state`, `/v1/sync/preflight_check`, `/v1/history` (`sdk/src/internal/indexer-discovery.ts:125,173,247,328,375,413`).
- Field-level match verified: SDK's `ApiDiscoveryCursor`/`ApiIncoming*`/`ApiOutgoing*` types (indexer-discovery.ts:31-97) mirror the Rust serde types 1:1, including `total_n_channels`, `total_n_notes`, `precomputed`, `last_note_index: number|null`.
- SDK rc.0→rc.5 client diff is behavioral only (total-only mode now paginates to the sentinel; cursor pruning; `total` surfaced in `discoverChannels` result) — **same wire format** (diff quoted in transcript; no path or field changes).
- Reorg contract: SDK treats HTTP 409 exclusively as `ReorgError` (indexer-discovery.ts:26-27) — our service must reserve 409 for `BLOCK_REORGED`.
- The live pool's class hash differs from the RC.0 README (context: measured `0x67dddd...` vs README `0x52107f...`), so the *contract* may be newer than RC.0 — but since discovery-core (slot layout `storage_slots.rs`, hashes, decryption) is identical RC.0→RC.5→main, there is no known layout drift to worry about within the published tags. Residual risk (INFERRED): a deployed contract class not corresponding to any public tag could have a different storage layout; mitigate by running the conformance fixtures against mainnet data early (e.g. verify a known user's registered pubkey slot).

---

## Q15 — What should the keyless client library look like?

### 15.1 SDK provider interface (VERIFIED)

Drop-in surface, quoted from `$MAIN/sdk/src/interfaces.ts:775-816`:

```ts
export interface DiscoveryProviderInterface {
  discoverNotes(
    address: StarknetAddressBigint,
    viewingKey: ViewingKey,
    params?: { cursor?: NotesCursor; tokens?: StarknetAddressBigint[]; blockIdentifier?: BlockIdentifier }
  ): Promise<{ timestamp: BlockIdentifier; notes: AddressMap<Note[]>; cursor: NotesCursor }>;

  discoverChannels(
    address: StarknetAddressBigint,
    viewingKey: ViewingKey,
    recipients: RecipientsFilter,   // StarknetAddress[] | "all" | "total-only"
    params?: { cursor?: ChannelCursor; blockIdentifier?: BlockIdentifier }
  ): Promise<{ timestamp: BlockIdentifier; channels?: AddressMap<Channel>; total?: number }>;

  discoverRequirement(
    address: StarknetAddressBigint,
    viewingKey: ViewingKey,
    recipient: StarknetAddressBigint,
    token: StarknetAddressBigint
  ): Promise<SetupRequirement>;   // Register=0 | SetupChannel=1 | SetupToken=2 | Ready=3
}
```

`createPrivateTransfers({ discoveryProvider })` accepts **any instance** implementing this interface (`factory.ts:105-109`; config-vs-instance discrimination is duck-typed via `"url" in x && !("discoverNotes" in x)`). `AbstractDiscoveryProvider` (exported from `./internal/index.ts`, and importable in-source) supplies a default `discoverRequirement` built on `discoverChannels` (`abstract-discovery.ts:37-47`). History (`fetchHistory`) is **not** part of the interface — it's an `IndexerDiscoveryProvider`-specific method used by the demo wallet and e2e tests; a fully drop-in provider should mirror its signature for wallet compatibility (`indexer-discovery.ts:388-415`).

**Viewing key flow through IndexerDiscoveryProvider (VERIFIED)**: serialized as a hex string into the JSON request body field `viewing_key` on every sync/preflight call — `viewing_key: toHex(viewingKey)` at `indexer-discovery.ts:162` (incoming), `:239` and `:316` (outgoing), `:378` (preflight). i.e. the hosted-indexer route sends the raw private viewing key over the wire (optionally inside an OHTTP envelope, `ohttp-client.ts`, when `options.ohttp` set). This is exactly the exposure our keyless client removes.

### 15.2 ContractDiscoveryProvider export status at 0.14.3-rc.5 (hackathon claim: PARTIALLY REFUTED)

Hackathon claim (`$HACK/docs/MAINNET-DAY-0.md:37`): not re-exported from the package entry, and no `./internal/*` subpath ⇒ "cannot be deep-imported either... discovery on the SDK route means a hosted indexer". Tracked in starkience/strk20-hackathon#121.

- VERIFIED TRUE for the main entry: `sdk/src/index.ts` exports only `type DiscoveryOptions` from contract-discovery (line 34) and the class `IndexerDiscoveryProvider` (line 35); no `ContractDiscoveryProvider`. VERIFIED TRUE that the `exports` map (package.json:13-33) has no `./internal/*` subpath.
- **VERIFIED FALSE as an absolute**: at tag `PRIVACY-0.14.3-RC.5`, `sdk/src/testing/index.ts:35` exports `ContractDiscoveryProvider` (with `type PoolContractInterface`), and `sdk/src/testing/browser.ts` (the `./browser/testing` bundle entry, "Browser-compatible testing utilities. Excludes Devnet") exports it too. The `exports` map includes `./testing` and `./browser/testing`; `tsc -p tsconfig.build.json` includes all of `src` → `dist/testing` ships (`files: ["dist"]`). So `import { ContractDiscoveryProvider } from "@starkware-libs/starknet-privacy-sdk/testing"` works at rc.5 (source-level VERIFIED; the published npm artifact itself is INFERRED — registry is GitHub Packages, auth-gated).
- Issue #121 (web, fetched 2026-08-29): OPEN, author OoJae, filed 2026-08-19, **no maintainer response**, no workaround noted in-thread. Nobody has flagged the `/testing` path.
- Practical caveats even with the workaround (all VERIFIED in source): `ContractDiscoveryProvider` needs a `PoolContractInterface` implementation — the SDK ships only `MockPoolContract` (testing); a real one must be hand-wired from starknet.js `Contract` + the `./abi` export. Its `discoverNotes` returns `timestamp: 0` and sets every note's `created: 0` (`contract-discovery.ts:223` via `NotesDiscovery.discover`), so **note maturity (10-block rule) is unknowable on the contract path** — notes can be unspendable-in-practice right after transfer. It also issues per-slot `get_*` view calls (bisect + scan), so it's RPC-heavy. These gaps are our indexer's pitch, and worth stating in the hackathon submission + as a comment on #121.

### 15.3 WASM feasibility (VERIFIED empirically)

**`cargo check -p discovery-core --target wasm32-unknown-unknown` compiles cleanly with zero patches at BOTH RC.0 and main HEAD** (rustc 1.95.0 stable; RC.0 `Finished ... in 1m 14s`, HEAD `Finished ... in 29.38s`). One toolchain note: the repo's `rust-toolchain.toml` pins channel 1.91.0 — irrelevant when consuming discovery-core as a git dependency (toolchain files don't propagate to dependents), only matters if building inside their workspace. Dependency notes:

| Dep (Cargo.toml / lock) | Role | wasm32 status |
|---|---|---|
| `starknet-types-core 0.2.4` (crates.io, features curve+serde) | Felt + Stark curve `AffinePoint` (ECDH in `decryption.rs:41-44`) | pure Rust (lambdaworks-math/crypto 0.13) — compiles (VERIFIED, in the check's build graph) |
| `starknet-rust-crypto 0.19.0-rc.0` (fork git rev 7caedfe) | `poseidon_hash_many` (all tags/hashes, `hashes.rs`), `get_public_key` | pure Rust (blake2, crypto-bigint, digest) — compiles (VERIFIED) |
| `starknet-rust-core` (fork) | types (`BlockId`, `EmittedEvent`, `StorageResult`) | compiles (VERIFIED) |
| `starknet-rust-providers` (fork) | **declared but never referenced in discovery-core src** (grep `starknet_providers` → 0 hits; VERIFIED) | compiles anyway on wasm (reqwest 0.13 wasm backend via wasm-bindgen/web-sys — in build graph); removable in a fork for leaner builds |
| `futures 0.3` (`FuturesUnordered` in sync/*) | engine concurrency | runtime-agnostic, wasm-fine (VERIFIED compile) |
| `zeroize 1` | `SecretFelt` zero-on-drop | compiles; see caveat below |
| `async-trait, thiserror, num-traits, tracing, serde, url` | plumbing | all wasm-fine (VERIFIED compile) |
| tokio | **dev-dependency only**; no tokio in library code (VERIFIED grep) | n/a |

No AES/ChaCha and no RNG in discovery-core at all — decryption is ECDH (Stark-curve scalar mult) + poseidon-hash masking + felt subtraction (`decryption.rs`); no `rand`/`getrandom` usage of our own (getrandom appears transitively and compiled fine).

**Zeroization in the browser (reality check, INFERRED from well-known platform behavior)**: `SecretFelt` (`privacy_pool/types.rs:28-60`) zeroizes on drop, excludes `Copy`/`Serialize`, and `Debug`-prints `[REDACTED]` — good hygiene that survives in wasm for the linear memory copy. But it is best-effort in a browser: the JS `bigint`/hex string the viewing key arrives as is GC-managed and cannot be scrubbed; wasm linear memory may be copied by the engine; no `mlock`. Honest posture for the README: zeroization limits exposure windows inside the wasm heap; the JS boundary value is out of our control. Recommend the wasm API accept the key as bytes (`Uint8Array`, caller-clearable) rather than a JS string.

`sync_incoming_state`/`sync_outgoing_state`/`preflight_check` are `<S: IViews>` generic — a wasm build wires an `IViews` impl (via the `RawStorageAccess` blanket) whose `read_slots` calls back into JS (`fetch` to a Starknet RPC's `starknet_getStorageAt`, or to our indexer's raw-slot endpoint). `Send + Sync` bounds on the traits are satisfiable in single-threaded wasm via `wasm-bindgen-futures` + `send_wrapper` or by a small `?Send` fork of the two trait defs (the only foreseeable friction; `async_trait` supports `#[async_trait(?Send)]` — a 4-line patch in a fork, or keep upstream and use `SendWrapper`). This is the one place "unmodified" may bend for the browser build (INFERRED — not yet compiled end-to-end with wasm-bindgen).

### 15.4 Client library shape (recommendation)

**Verdict: wrap the crate, don't reimplement.** discovery-core is Apache-2.0, wasm-clean, engine-stable across every published tag, and gets us protocol-exact decryption for free. Reimplementing poseidon-tag ECDH + cursor semantics in TS would duplicate ~2k lines of subtle logic the SDK itself doesn't expose for reuse (its own crypto in `sdk/src/utils/hashes.ts`/`encryptions.ts` exists but the scan engine in `contract-discovery.ts` is testing-namespace and maturity-broken).

Proposed layers:

1. **Rust core (`strk20-client-core`)**: depends on `discovery-core` (git tag). Implements `RawStorageAccess`/`RawEventAccess` over two pluggable transports: (a) plain Starknet JSON-RPC (`starknet_getStorageAt` batches — works keylessly against any public node, no fork extension needed except `last_update_block`, which we substitute from our indexer or degrade to `0`/event-derived), (b) our indexer's raw endpoints (batched slot reads + `last_update_block` + events, no viewing key ever sent). Exposes `sync_incoming`, `sync_outgoing`, `preflight`, `history` mirroring the engine entrypoints.
2. **WASM package (`strk20-client-wasm`)**: wasm-bindgen wrapper; key in as bytes; JSON-in/JSON-out using the exact serde shapes of `api/types.rs` so cursors are interchangeable with the hosted-indexer wire format.
3. **TS adapter (`strk20-discovery-provider`)**: a class implementing `DiscoveryProviderInterface` (3 methods, §15.1) + `fetchHistory` mirroring `IndexerDiscoveryProvider`, driving the wasm engine locally. The cursor mapping functions to imitate are exported and testable in the SDK: `notesCursorToApiCursor`, `apiCursorToNotesCursor`, `buildSubchannelCursors`, `convertIncomingNotes` (`indexer-discovery.ts:462-679`) — we reuse their semantics so `NotesCursor`/`ChannelCursor` round-trip identically. Plugs into `createPrivateTransfers({ discoveryProvider: new LocalDiscoveryProvider(...) })` with zero SDK changes (VERIFIED factory accepts instances).

Result: the same engine binary serves both modes — server-side (compatible mode, viewing key sent to *your own self-hosted* service) and client-side (keyless mode, viewing key never leaves the browser; the indexer only ever sees address-blind raw slot/event queries).

### 15.5 Conformance-test asset inventory (VERIFIED)

| Asset | Location ($RC0 unless noted) | Contents / reuse |
|---|---|---|
| **Cairo reference vectors** | `crates/discovery-core/tests/fixtures/cairo-reference-data.json` (5,775 B) | Canonical cross-language vectors generated from the Cairo contract (`sdk` script `generate:cairo-refs` runs `scarb test ... generate_reference`): 22 inputs (keys, channel_key, salts, ephemeral secret, shared_x...), 26 outputs (every enc/dec value + all tag-hash outputs + note_id/nullifier/markers), 14 storage-slot addresses. **The** conformance fixture for any reimplementation or wrapper — exercised by `decryption.rs` tests (`test_decrypt_*_with_cairo_vectors`). |
| Same vectors, TS side | `sdk/tests/fixtures/cairo-reference-data.json` | `inputs`/`outputs` **identical** to the Rust copy (VERIFIED by JSON compare) — proves it's the shared cross-language contract; our TS adapter tests can consume it directly. |
| **Devnet state snapshot** | `crates/discovery-core/tests/fixtures/devnet-state.json` (6,581 B) | 48 slot→value pairs + constants (contract, alice/bob addresses + viewing keys, eth/strk tokens). Loaded into `MockBackend` to run the *full engine* (channels→subchannels→notes) end-to-end without a chain. Perfect seed for a SQLite/Postgres backend conformance test: load slots into our DB, assert engine output equals engine-over-MockBackend output. Loader structs: `test_fixtures.rs` (`DevnetFixture`, `load_devnet_fixture`). Caveat: `test_fixtures` is `#[cfg(test)]`-private (`lib.rs:9-10`) — copy the ~200-line loader or use `include_str!` on the JSON ourselves. |
| **Service-level devnet dump** | `crates/discovery-service/tests/fixtures/devnet-dump.json.gz` (260 KB) + `devnet-dump.metadata.json` | Full starknet-devnet dump replayed by `common/devnet.rs`; drives 11 HTTP-level tests in `tests/test_api.rs` (health, incoming/outgoing sync, preflight, history) using the service's own request/response structs. Reusable as black-box conformance suite: point the same tests at our compatible-mode server. |
| **SDK wire-format tests** | `$MAIN/sdk/tests/internal/indexer-discovery.test.ts` (667 lines) | Mocked-`fetch` tests of every endpoint round-trip incl. cursor conversion, pagination, 409 reorg handling, total-only mode. Defines exactly what JSON the shipped SDK accepts/emits — replayable against our server (serve the fixture bodies, or run the SDK against our endpoint). |
| SDK hash/encryption tests | `$MAIN/sdk/tests/utils/hashes.test.ts`, `encryption.test.ts` | TS-side vectors for tags/ECDH — sanity net for the TS adapter boundary. |
| **e2e suites** | `e2e/tests/devnet/{history,pagination-discovery,payment-service-discovery,reorg-recovery,smoke,ohttp,ohttp-relay,...}.test.ts`, `e2e/tests/integration/privacy-starknet-integration.test.ts` | Full-stack SDK↔service flows incl. `fetchHistory` pagination and reorg recovery; the devnet ones are runnable against a local devnet + our indexer for the strongest "drop-in" demonstration. |
| Mock backends in-crate | `MockBackend` (pub, `storage_backend.rs:81`) usable from our tests; `MockEventBackend` + `mock_event` are `#[cfg(test)]`-only (`events_backend.rs:41-131`) — copy ~90 lines if needed. |
| Screening vectors | `fixtures/screening-vectors.json` (1,652 B) | Deposit-screening domain — not needed for discovery conformance. |

Version note: all of the above identical at RC.0/RC.5 for discovery-core (zero diff); sdk tests referenced from $MAIN (= rc.5 package).

---

## Bottom line

- **Q14 verdict — REUSE CORE (and probably the service crate too)**: compatible mode is "implement 4 small async traits (+`ChainState`) over our DB and mount the reference `ApiServer` on top". The HTTP surface is 5 routes; wire format has been frozen across RC.0→RC.5→main; the SDK's shipped client binds to exactly it. The only RPC-fork-specific feature (`IncludeLastUpdateBlock`) becomes trivial with a local DB. License Apache-2.0; consume via cargo git tag (not on crates.io, can't be, due to git deps).
- **Q15 verdict — WRAP, DON'T REIMPLEMENT**: `discovery-core` compiles for wasm32-unknown-unknown untouched (empirically verified). Ship Rust core → wasm-bindgen package → TS class implementing the 3-method `DiscoveryProviderInterface` (+`fetchHistory`), drop-in via `createPrivateTransfers`. The hosted route serializes the raw viewing key into every request body (`viewing_key` field) — our keyless mode is a real, demonstrable privacy win. Bonus finding for the submission: `ContractDiscoveryProvider` *is* reachable at rc.5 via the undocumented `.../testing` subpath (issue #121 is open, unanswered, and unaware of this), but it's testing-namespace, needs a hand-built `PoolContractInterface`, and loses note-creation blocks (`created: 0`) → maturity-blind. Our provider fixes all three.
- **Conformance assets**: cairo-reference-data.json (cross-language crypto vectors, identical Rust/TS copies), devnet-state.json (48-slot engine-level snapshot), service devnet dump + 11 HTTP tests, and the SDK's 667-line wire-format test file — enough to prove byte-compatibility of both our server mode and our client library without writing new vectors.

## Open unknowns

1. wasm `Send` bounds: end-to-end wasm-bindgen build (JS-callback-backed `RawStorageAccess`) not yet attempted; may need `#[async_trait(?Send)]` fork or `SendWrapper` (small, known techniques).
2. Published npm artifact of sdk `/testing` subpath not directly inspected (GitHub Packages auth) — source-level export chain verified instead.
3. Whether the live mainnet pool's (upgraded) class matches RC-tag storage layout — no layout drift across public tags, but the deployed class hash matches no README value; verify one known slot against mainnet before the demo.
4. ~~wasm check at main HEAD~~ — RESOLVED: also compiles cleanly (29.38s, stable 1.95.0).
