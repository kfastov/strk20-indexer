# Bill of materials — strk20-indexer (verified 2026-08-29)

Everything below was verified hands-on this session unless marked otherwise. Throwaway verification crate: `scratchpad/invcheck` (`cargo run` prints `ok: inc_complete=true out_complete=true sender_registered=false`).

## 1. discovery-core as a dependency — VERIFIED COMPILING + RUNNING

Exact stanza that works (resolved, compiled, and the engine executed against `MockBackend`):

```toml
[dependencies]
discovery-core = { git = "https://github.com/starkware-libs/starknet-privacy.git", tag = "CONTRACT_V2_DEPLOYED_MAINNET_2026-07-08" }
# fork crates — needed ONLY to name types that appear in discovery-core's public API
starknet-core = { package = "starknet-rust-core", git = "https://github.com/software-mansion/starknet-rust.git", rev = "7caedfe" }  # BlockId, StorageResult, EmittedEvent
starknet-types-core = { version = "0.2", features = ["curve", "serde"] }   # Felt — MUST stay 0.2.x (locked 0.2.4); crates.io now has 1.0.0 which is a DIFFERENT Felt type
```

- Lock resolution: `discovery-core 0.1.0 @ 74841caf0466d122117945e28ed983e2864c8fc1` (= the tag), fork crates all `0.19.0-rc.0 @ 7caedfef85a4d748f8e9e5a159c87c31b6fe9d71`.
- Wall time: first `cargo check` (incl. both git fetches + full dep graph) **34.4 s**; `cargo run` after check **26.9 s** (codegen); warm rebuilds ~0.2 s.
- Fallbacks (not needed, tag resolved): `rev = "74841ca"` or `tag = "PRIVACY-0.14.3-RC.3"` (contract + engine source byte-identical per research).
- discovery-core's own deps (from its Cargo.toml at the tag): thiserror 1, async-trait 0.1, num-traits 0.2, tracing 0.1, serde 1 (derive), url 2, futures 0.3, zeroize 1, plus the three fork crates and starknet-types-core 0.2. It also pulls `starknet-providers` (fork) — comes along transitively, we don't need to name it.

### Public API surface (module paths as importable; all confirmed pub at the tag)

`lib.rs` re-exports nothing; everything is via `pub mod`: `discovery`, `events_backend`, `history`, `io_budget`, `privacy_pool`, `storage_backend`, `sync`.

**Traits** (`discovery_core::storage_backend`, `discovery_core::events_backend`):

```rust
#[async_trait] pub trait RawStorageAccess: Send + Sync {
    async fn read_slot(&self, slot: Felt) -> Result<Felt, StorageError>;
    async fn read_slots(&self, slots: Vec<Felt>) -> Result<Vec<Felt>, StorageError>;
    async fn read_slots_with_block(&self, slots: Vec<Felt>) -> Result<Vec<StorageResult>, StorageError>;
}
#[async_trait] pub trait StorageBackend: Send + Sync {
    type Snapshot: StorageSnapshot;
    async fn snapshot(&self, contract_address: Felt, block_id: Option<BlockId>) -> Result<Self::Snapshot, StorageError>;
}
#[async_trait] pub trait StorageSnapshot: IViews {
    fn contract_address(&self) -> Felt;
    fn block_id(&self) -> BlockId;
}
#[async_trait] pub trait RawEventAccess: Send + Sync {   // events_backend
    async fn get_events(&self, keys: &[Vec<Felt>], from_block: BlockId, to_block: BlockId) -> Result<Vec<EmittedEvent>, StorageError>;
    fn block_id(&self) -> BlockId;
    fn block_number(&self) -> u64;
}
```

**Key fact:** the sync engine is generic over `S: IViews` (`discovery_core::privacy_pool::views::IViews`, 15 methods), and there is a **blanket impl `impl<T: RawStorageAccess> IViews for T`** (views.rs). So our DB snapshot only implements `RawStorageAccess` (3 methods) and gets the whole engine. `MockBackend` (pub, non-test: `new/empty/insert/insert_with_block`) is a ready-made in-memory `RawStorageAccess` — usable in our tests directly.

**Engine fns** (exact signatures):

```rust
// discovery_core::sync::incoming_state
pub async fn sync_incoming_state<S: IViews>(pool: &S, recipient: Felt, viewing_key: &SecretFelt,
    cursor: DiscoveryCursor, cursor_limits: CursorLimits, budget: &IoBudget)
    -> Result<SyncIncomingStateResult, DiscoveryError>;
pub struct SyncIncomingStateResult { pub channels: Vec<IncomingChannel>, pub subchannels: Vec<IncomingSubchannel>, pub notes: Vec<DecryptedNote>, pub cursor: DiscoveryCursor }

// discovery_core::sync::outgoing_state
pub async fn sync_outgoing_state<S: IViews>(pool: &S, sender_addr: Felt, viewing_key: &SecretFelt,
    cursor: DiscoveryCursor, cursor_limits: CursorLimits, budget: &IoBudget, recipients: Option<&HashSet<Felt>>)
    -> Result<SyncOutgoingStateResult, DiscoveryError>;
pub struct SyncOutgoingStateResult { pub channels: Vec<OutgoingChannel>, pub subchannels: Vec<OutgoingSubchannel>, pub cursor: DiscoveryCursor }

// discovery_core::sync::preflight_check
pub async fn preflight_check<S: IViews>(pool: &S, sender_addr: Felt, decryption_key: &SecretFelt,
    recipient: Felt, token: Felt) -> Result<PreflightCheckResult, DiscoveryError>;
pub struct PreflightCheckResult { pub sender_registered: bool, pub channel_exists: bool, pub subchannel_exists: bool }
```

**Key types:**

- `discovery_core::discovery::{DiscoveryCursor, ChannelCursor, SubchannelCursor, CursorLimits}` (re-exported from `discovery::cursor`). All Serialize+Deserialize. `DiscoveryCursor { channel_discovery_complete: bool, total_n_channels: Option<u64>, last_channel_index: Option<u64>, channels: HashMap<Felt, ChannelCursor> }`, `is_complete()`, `all_channels_processed()`. `CursorLimits { max_channels: usize (256), max_subchannels: usize (64), max_note_log_index: u32 (30) }` with `Default`. NOTE: `ChannelCursor.channel_key: SecretFelt` — serialized cursors contain secret-derived material; treat persisted cursors as sensitive.
- `discovery_core::io_budget::IoBudget` — `new(limit)`, `remaining()`, `consume`, `reclaim`, `try_consume`, `consume_up_to`; clone-shared atomic. Cost constants pub in `discovery_core::discovery`: `COST_NUM_CHANNELS=1, COST_CHANNEL_INFO=3, COST_SUBCHANNEL_INFO=2, COST_NOTE=2, COST_OUTGOING_CHANNEL_INFO=3, COST_NOTE_PROBING=1, COST_PUBLIC_KEY=1, COST_BLOCK_EVENTS_QUERY=10, EVENTS_COST_CHUNK_SIZE=1024, COST_EVENTS_CHUNK=10`; `pub fn min_server_budget(max_note_log_index: u32) -> usize`.
- `discovery_core::privacy_pool::types::SecretFelt` — `new(Felt)`, `Deref<Target=Felt>`, Zeroize-on-drop, `Debug` prints `[REDACTED]`, deliberately no Copy/Serde; serde helpers in `privacy_pool::types::secret_felt_serde`.
- `discovery_core::storage_backend::StorageError` (enum: CastToU64Error, ContractNotFound, Backend(Box<dyn Error>), SlotCountMismatch) — our backends map DB errors into `StorageError::Backend`.
- `discovery_core::discovery::DiscoveryError`; history module: `discovery_core::history::{notes, transactions, types}`.
- Crypto/slot helpers all pub: `privacy_pool::hashes::{compute_channel_key, compute_channel_marker, compute_subchannel_marker, ...}`, `privacy_pool::storage_slots::*`, `privacy_pool::decryption`, `privacy_pool::events`, `privacy_pool::felt_hex`.
- `EmittedEvent` is the FORK's type (re-exported at `discovery_core::events_backend::EmittedEvent`): fields `from_address, keys, data, block_hash: Option, block_number: Option, transaction_hash, event_index: u64, transaction_index: u64`. Standard RPC getEvents does NOT return event_index/transaction_index — when materializing `EmittedEvent` from our DB we must store/assign them ourselves at ingest time (order within block).

## 2. Fixtures

Paths at the tag (`git show CONTRACT_V2_DEPLOYED_MAINNET_2026-07-08:<path>`):
- `crates/discovery-core/tests/fixtures/devnet-state.json` — engine-level fixture: `{_comment, constants: {contract_address, alice_address, alice_viewing_key: 0xa11ce, bob_address, bob_viewing_key: 0xb0b, admin_address, eth_token: 0x49d3...4dc7 (ETH), strk_token: 0x4718...938d (STRK)}, block: 46, slots: {48 × hex-felt slot → hex-felt value}}`. contract_address 0x66292db2e7d6fe7d76386b4198a41ad42a108a1895fe09eada749bed7633f76, alice 0x34ba56f92265f0868c57d3fe72ecab144fc96f97954bbbc4252cef8e8a979ba, bob 0x2939f2dc3f80cc7d620e8a86f2e69c1e187b7ff44b74056647368b5c49dc370, admin 0x25a6c9f0c15ef30c139065096b4b8e563e6b86191fd600a4f0616df8f22fb77.
- `crates/discovery-core/tests/fixtures/cairo-reference-data.json` — crypto vectors: `{_comment, _ttl_days, inputs (22 camelCase fields: sender/recipient/keys/token/index/salt/...), proofFacts (12), slots (14 slot addresses), outputs (26 expected values: channelKey, channelMarker, subchannelId/Marker, noteId, nullifier, enc* masks, decNoteAmount, ...)}`.
- `crates/discovery-service/tests/fixtures/devnet-dump.json.gz` + `devnet-dump.metadata.json` — HTTP-level fixture used by the 11 tests in `crates/discovery-service/tests/test_api.rs`.

Loader `crates/discovery-core/src/test_fixtures.rs` is `#[cfg(test)] mod test_fixtures` in lib.rs → **private to the crate; we must copy it**. What to copy (small): structs `DevnetFixture {constants, slots: HashMap<Felt,Felt>}`, `DevnetConstants` (viewing keys deserialized via `secret_felt_serde`), `CairoRefFixture {inputs, outputs, slots}` (camelCase rename), and loaders `load_devnet_fixture()` / `load_cairo_ref_fixture()` which are just `include_str!` + `serde_json::from_str`. All types they reference are pub, so a verbatim copy into our test tree compiles. Note the fixture ignores the top-level `block` field (not in the struct — serde default tolerates it). Copy the two JSON files into our repo (Apache-2.0, attribute).

Acceptance-test recipe this enables: load devnet-state.json into our SQLite storage mirror → run our snapshot impl → `sync_incoming_state(alice/bob)` → results must equal running the same engine over `MockBackend::new(fixture.slots)` — plus HTTP-level equality against the reference wire fixtures for U8.

## 3. Crates for our own code (crates.io versions verified via sparse index 2026-08-29)

| Crate | Version | Role / note |
|---|---|---|
| axum | 0.8.9 | HTTP server. **Pick axum**: the reference discovery-service is axum 0.8 itself, so U8 (mounting the reference ApiServer/router on our backend) already forces axum 0.8 into the tree; one HTTP stack, tower ecosystem, no contest. |
| tower-http | 0.7.0 (reference pins 0.6; either works) | cors/timeout layers |
| rusqlite | 0.40.2, `features=["bundled"]` | Embedded SQLite, statically linked → U4 one-binary, deterministic. Sync API — wrap in `tokio::task::spawn_blocking` (or a dedicated thread + channel). Chosen over sqlx: sqlx's async adds nothing for a local file DB, its compile-time query checking wants a live DB at build, heavier dep tree. Postgres-optional later = our own trait, not sqlx. |
| zstd | 0.13.3 | epoch bundle compression |
| serde / serde_json | 1.0.229 / 1.0.151 | wire + bundle format |
| tokio | 1.53.1 `features=["rt-multi-thread","macros","signal","time","sync"]` | runtime (engine is async) |
| reqwest | 0.13.4 `features=["json"]` | RPC ingest client (reference service also uses reqwest 0.13) |
| tracing / tracing-subscriber | 0.1.44 / 0.3.23 | discovery-core is instrumented with tracing already |
| clap | 4.6.6 `features=["derive"]` | CLI |
| thiserror | 2.0.20 (lib errors; discovery-core uses 1.x — both coexist fine) | |
| anyhow | 1.0.104 | binary-side error context |
| async-trait | 0.1.92 | to impl the core's traits |
| futures | 0.3.34 | stream utils |
| tempfile | 3.27.0 | tests |

**Fork-provider question, settled:** our 5 ingest methods (getEvents, getStateUpdate, getBlockWithTxHashes, getStorageProof, getClassHashAt) are standard JSON-RPC that we call with plain reqwest + serde_json against endpoints we control — **no starknet-providers needed**; we define our own minimal response structs. The fork's provider is only required for the reference service's non-standard `getStorageAt(response_flags=[INCLUDE_LAST_UPDATE_BLOCK])`, which public RPCs reject anyway and which our diff-derived index replaces. Where a value crosses into discovery-core (Felt, BlockId, StorageResult, EmittedEvent) we construct the fork/types-core types directly — verified compiling in invcheck. Pin `starknet-types-core = "0.2"` and never let 1.0.0 in (different Felt).

## 4. RPC wire shapes (research docs + one live call each on lava, 2026-08-29)

- `starknet_getEvents` — params `[{ "filter"-less object: from_block:{block_number}|tag, to_block, address, keys?: [[felt,...],...], chunk_size, continuation_token? }]` (single positional object). Result: `{ events: [{ from_address, keys: [felt], data: [felt], block_hash, block_number, transaction_hash }], continuation_token?: "8978970-3" }` (token format `<block>-<offset>`; absent on last page; partial chunks < chunk_size occur on sparse ranges — keep paging until token absent). LIVE-VERIFIED.
- `starknet_getStateUpdate` — params `[{block_number: N}]`. Result: `{ block_hash, new_root, old_root, state_diff: { storage_diffs: [{address, storage_entries: [{key, value}]}], nonces, deployed_contracts: [{address, class_hash}], replaced_classes: [{contract_address, class_hash}], declared_classes, deprecated_declared_classes } }`. Pool deploy appears in `deployed_contracts` @8,978,970; the one upgrade in `replaced_classes` @11,632,886. VERIFIED (research, live samples in docs/research/data/sample-pool-diffs.json).
- `starknet_getBlockWithTxHashes` — params `[{block_number}|"latest"|"l1_accepted"|"pre_confirmed"]`. Result fields (live): `block_hash, block_number, parent_hash, new_root, timestamp, status, sequencer_address, starknet_version, l1_gas_price, l2_gas_price, l1_data_gas_price, l1_da_mode, transactions: [tx_hash]`. `status`: `ACCEPTED_ON_L2` | `ACCEPTED_ON_L1`. `"l1_accepted"` tag works on lava's 0.8.1 path (live: block 14,056,429 → ACCEPTED_ON_L1); `pre_confirmed` only on /rpc/v0_9 (no hash, status null). LIVE-VERIFIED.
- `starknet_getStorageProof` — positional `[block_id, classes?, [contract_addresses], contract_storage_keys?]` (publicnode-verified form `["latest", null, [pool], null]`; per-key form adds `[{contract_address, storage_keys:[...]}]`). Result: `{ classes_proof, contracts_proof: { nodes, contract_leaves_data: [{nonce, class_hash, storage_root}] }, contracts_storage_proofs: [[nodes]], global_roots: {contracts_tree_root, classes_tree_root, block_hash} }`. Compare against header `new_root` via `state_root = h_Pos("STARKNET_STATE_V0", contracts_tree_root, classes_tree_root)`. Availability window on lava ≈ last 25k–55k blocks → verify promptly. Bonus: every proof carries the pool's current `class_hash`. VERIFIED (research, raw proof archived).
- `starknet_getClassHashAt` — params `[block_id, contract_address]` → felt class hash (live @11,632,886: `0x67dddd89d80fedadc06b6f160798f94800a4a70164e5a24301cd0d6076b554d`). LIVE-VERIFIED.
- Endpoints: primary `https://rpc.starknet.lava.build` (0.8.1, archive-deep, set a real User-Agent), secondary `https://starknet.publicnode.com` (0.10.2, <5 rps).

## 5. SDK adapter target — `sdk/src/interfaces.ts` (working tree = main; wire frozen RC.0→RC.5)

```ts
export interface DiscoveryProviderInterface {
  discoverNotes(
    address: StarknetAddressBigint, viewingKey: ViewingKey,
    params?: { cursor?: NotesCursor; tokens?: StarknetAddressBigint[]; blockIdentifier?: BlockIdentifier; }
  ): Promise<{ timestamp: BlockIdentifier; notes: AddressMap<Note[]>; cursor: NotesCursor; }>;

  discoverChannels(
    address: StarknetAddressBigint, viewingKey: ViewingKey, recipients: RecipientsFilter,
    params?: { cursor?: ChannelCursor; blockIdentifier?: BlockIdentifier }
  ): Promise<{ timestamp: BlockIdentifier; channels?: AddressMap<Channel>; total?: number }>;

  discoverRequirement(
    address: StarknetAddressBigint, viewingKey: ViewingKey,
    recipient: StarknetAddressBigint, token: StarknetAddressBigint
  ): Promise<SetupRequirement>;
}
// ViewingKey = BigNumberish (types.ts:36); StarknetAddressBigint = bigint (types.ts:68)
```

Every method takes the raw `viewingKey` — the SDK's built-in `IndexerDiscoveryProvider` ships it to the server (U8 compat mode, key-visible, must be labeled); our keyless client lib implements this same interface locally over the wasm'd discovery-core + our bulk feed (U1/U2).

## 6. Reference service facts relevant to reuse

- Routes (crates/discovery-service/src/api/mod.rs:104-112): `GET /health`, `POST /v1/sync/incoming_state`, `POST /v1/sync/outgoing_state`, `POST /v1/sync/preflight_check`, `POST /v1/history` (+ optional OHTTP fallback layer). 5 routes total incl. health.
- Its Cargo.toml: axum 0.8, reqwest 0.13, tower 0.5, tower-http 0.6, moka 0.12 cache, clap 4, same fork rev 7caedfe. Mounting its `ApiServer` over our `StorageBackend` impl is dependency-compatible with our stack by construction.
