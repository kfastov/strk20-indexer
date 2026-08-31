# TypeScript consumer path — A2 (privacy & verifiability lens)

Council proposal, 2026-08-31. Revises [`docs/spec/consumer-path.md`](../../../spec/consumer-path.md)
**§A3** (wasm ABI) and **§A4** (npm package), and specifies the demo application.

Given, and not re-argued: the two-block architecture and the `FeedTransport`
seam ([roadmap](../../../roadmap.md)); WASM as a pure synchronous computer;
keyless + delegated dual API; no write path; the persistence trade-off of
[design notes §7](../../../notes/2026-08-30-consumer-path-discussion.md);
epochs immutable; discovery-core consumed unmodified; **§12** (historical
storage proofs *are* obtainable, so §A1's anchor ladder stands and §11 is
retracted).

**Lens.** I optimise for one thing: that the privacy and verifiability story is
*provably* intact through the whole TypeScript layer and *visible* to a viewer
of the demo. Two operational consequences run through every section below:

- **Prefer a structural proof to an asserted one.** Where a property can be made
  a theorem about a key-blind pure function in Rust, it must not be left as an
  empirical scan over a wire capture. The scan then becomes a *second*,
  independent check of the same property rather than the only one.
- **A number nobody can check is a liability.** Every claim the demo renders
  must be either measured in that session or labelled with its provenance, and
  the demo must ship the mechanism a sceptic uses to disbelieve it.

---

## 0. Three facts that move the design since §A3/§A4 were written

### 0.1 §A3/§A4 were designed against the spike; the state machine now exists

The WASM spike wrapped `sync_incoming_state` over `MockBackend`. §A3 was
written against that shape and therefore assumed the browser module would own
three independent apply entry points (`apply_snapshot`, `apply_epoch`,
`apply_head`) with TypeScript orchestrating them.

That is no longer the real shape. `crates/consumer/src/apply.rs` holds the
orchestration as **one function** — `apply_feed(store, transport, cold_start)`
— and the orchestration is where most of the trust logic lives: the
manifest-divergence check against the locally applied epoch hash
(`apply.rs:197-207`), the masked-reorg contradiction test before each epoch
range replacement (`apply.rs:236-256`), the mid-sync race bail
(`apply.rs:273-279`), the `snapshot_pending_grounding` flag and the
`reset_mirror`-on-rejection discipline (`apply.rs:79-105`), and the §11.3
reachability walk that must run *after* the epochs above the basis and the head
tail have landed (`apply.rs:330-341`).

§A3's decomposed ABI would have that orchestration re-implemented in
TypeScript. §3.3 already states the principle that forbids this — *"the TS
wrapper — the least testable layer — holds no discovery-adjacent logic at all"*
— and then violates it for the apply half. §1 replaces the decomposed ABI with
a trampoline that keeps `apply_feed` **byte-for-byte the same function** in both
hosts.

The second reality delta is the trait surface. §0.4.1 specified a `NoteSet`
value type with `notes_get`/`notes_put`, explicitly so that *"nothing
key-derived ever enters `MemStore` beyond the lifetime of the call"*. The
trait that actually landed (`crates/consumer/src/store.rs:95-169`) is
row-level: `notes`, `upsert_note`, `set_note_spent`, `delete_note`,
`delete_owner_notes`, and the state machine additionally drives
`refresh_spent` / `prune_missing_notes` and persists **discovery cursors
through `meta_set`** (`crates/client/src/sync.rs:80-118, 226-235, 320-379`).
Cursor JSON contains channel keys. Under §A3's export rule that material is one
careless serializer away from the exported state blob. §1.4 makes the
separation structural instead of prefix-based.

### 0.2 Our customer is a wallet, not a Wallet-API dapp

Verified today from the official Wallet API documentation (orchestrator's
verification, taken as given): *"No viewing keys in your app. The wallet holds
the user's viewing key"*, and *"The wallet discovers notes, builds the proof"*.

A dapp on the Wallet API never possesses a viewing key, therefore can never
call us, therefore is not a customer. **The npm package's customer is a wallet
or a key-holding application** — the same population the
`strk20-privacy-sdk` serves, and the same population that today either runs
`IndexerDiscoveryProvider` against somebody's indexer or re-walks channels
itself.

Three design consequences, each acted on below:

1. The SDK adapter (`strk20-discovery/sdk`, `LocalDiscoveryProvider`) is the
   **primary** integration surface, not a secondary convenience (§2.8). A wallet
   already has a `DiscoveryProvider` seam; we should land in it.
2. The API must make **key custody explicit and revocable**, because our
   caller is by definition holding a key for a long time. §2.4's `unlock()` /
   `Unlocked` handle replaces §4.2's `subscribe(k, cb)`, which forced a
   long-lived client to retain a key with no name, no lifecycle and no status
   bit.
3. **The demo may not "connect a wallet" and then expect to discover notes.**
   It cannot: a Wallet-API connection yields no key. §3 builds the demo around
   the honest positioning instead — we are the *read half* of every write.

### 0.3 The verification ladder the browser must run is the full one

§12 reinstates the basis-block anchor sidecar and the batch root check, so the
browser's ladder is rings 1–5 offline plus ring 6 against the user's own RPC,
identical to `apply.rs`'s. Two browser-specific notes:

- ring 5's honest grade (§1.5's *"buys nothing against the server itself"*)
  must reach the UI as a word, not a boolean. `verified: 'anchored' |
  'server-asserted' | 'replayed'` is already the spec's answer; the npm client
  surfaces it in `status()` **and** the demo renders it as a chip whose text is
  the grade, never a green tick.
- ring 6 in a browser is a fetch to a **user-supplied** RPC URL. It is
  address-blind (public pool, public block) but it is a *second origin*, so it
  is opt-in, it is listed in the demo's network panel like any other request,
  and the wrapper's fetch chokepoint (§2.6) tags it `origin: 'anchor-rpc'` so a
  viewer can see exactly which bytes came from where.

### 0.4 The invariants, restated as things that can fail

| # | Property | Where it is proven |
|---|---|---|
| **P-blind** | The multiset **and order** of feed requests a keyless client emits is a function of feed progress only — identical for every key and address. | **Structural (new):** the request sequence is emitted by `apply_feed` running over a store and transport that have never seen a key (§1.2), so it is the output of a key-blind function; a Rust proptest over two keys asserts identical logs. **Empirical:** leg **u**'s proxy capture, unchanged. **Visible:** demo network panel, two-identity toggle (§3.7). |
| **P-keyless** | No encoding of the viewing key, the address, or any key-derived felt appears in any request, any URL, any header, any log line, any event payload, or any persisted artifact other than the sealed AEAD blob. | Type system (§2.3 closed event/debug unions, no `string` key type); `capture-scan` over the proxy capture and the IDB dump (leg **u**); **new:** the scanner also runs over `Engine.export()` after a `discover()` with a planted key, and over the module's own request log. |
| **P-pure** | The wasm module opens no network handle, no storage handle, no timer, and no randomness source. | §3.9 import-section audit against the checked-in allowlist, unchanged. Strengthened by §1.2: the module cannot fetch *even in principle* because it has no transport — it can only ask. |
| **P-scoped** | Key-derived state lives in `MemStore` only for the duration of one `discover()` call, and cannot reach `export()`. | **Structural (new):** closed allowlist of exportable meta keys; owner-scoped state is a separate field the exporter cannot name (§1.4). |
| **P-immutable** | Nothing folded from epochs can be invalidated by a reorg; the tail is never persisted. | §3.5 grammar bounds (`no line references a block > last_epoch_to`), leg **r** byte-identity across a fork. Unchanged and reaffirmed. |

---

## 1. §A3 revised — the wasm ABI against the real `ConsumerStore`

### 1.1 Crates (§3.1 amended)

```
crates/consumer     strk20-consumer   Block B core (exists; 0a in flight)
crates/client-wasm  strk20-engine     cdylib: wasm-bindgen facade + MemStore + ParkingTransport
```

§3.1's dependency list stands verbatim, including the load-bearing
`default-features = false` on every RustCrypto line and the exclusion of
`compress`. One addition: `futures-core` / `futures-util` at
`default-features = false` for the manual poll driver of §1.2 (no executor, no
`futures-executor`, no `wasm-bindgen-futures` — the last would import a
JS-side task queue and is forbidden by the import allowlist).

### 1.2 The change that matters: a synchronous fetch trampoline, not a decomposed ABI

> **§3.3 quoted, replaced.** §3.3 exports `apply_snapshot(payload,
> manifest_snapshot_json, anchor_json)`, `apply_epoch(payload,
> manifest_entry_json)` and `apply_head(payload)` as three independent methods,
> leaving TypeScript to decide *which* artifacts to fetch, in what order, when
> to cold-start, when to fall back, and when to reset. **Replaced by
> `sync_begin` / `sync_supply` below.**
>
> Reason: the ordering *is* the trust logic (§0.1). Under §3.3 the browser and
> the native client would run two different implementations of it, one of them
> in the layer with the weakest tests, and P-blind would degrade from a property
> of our state machine to a property of whatever the wrapper happened to do.

**Mechanism.** `apply_feed` is `async` only because `FeedTransport` is. In the
browser we keep the function and park the transport:

```rust
/// The browser's FeedTransport. It performs no IO. Each method records the
/// request it wants in `pending` and returns a future that yields Pending until
/// the wrapper supplies a response, then Ready.
pub struct ParkingTransport {
    pending:  RefCell<Option<FeedRequest>>,   // at most one outstanding
    supplied: RefCell<Option<FeedResponse>>,
    log:      RefCell<Vec<LoggedRequest>>,    // canonical, key-independent
}
```

`Engine` drives it with a manual poll over a no-op waker:

```rust
enum Step {
    Fetch(FeedRequest),                 // the wrapper must satisfy this and re-enter
    Done(ApplyOutcome),
}

fn pump(&mut self) -> Result<Step> {
    let waker = noop_waker();
    match self.fut.as_mut().poll(&mut Context::from_waker(&waker)) {
        Poll::Ready(out) => Ok(Step::Done(out?)),
        Poll::Pending => match self.transport.pending.borrow_mut().take() {
            Some(req) => Ok(Step::Fetch(req)),
            // Nothing but the transport may ever pend. A Pending with no armed
            // request means a future we do not control suspended — a
            // programming error, not a runtime path.
            None => panic!("PARK_WITHOUT_REQUEST"),
        },
    }
}
```

This preserves every §3.2 property and adds three:

- **`apply_feed` is literally the same function in both hosts.** No second
  implementation of epoch ordering, cold-start selection, the `auto` fallback,
  the mid-sync bail or the grounding order can exist, because there is only one.
- **P-blind becomes structural.** The request sequence is the output of a
  function whose inputs are (profile, persisted mirror, server responses) — no
  key, no address, no owner. Two identities produce identical logs *by type*,
  which a Rust test asserts directly (`leg q'` below). The wire capture in leg
  **u** stops being the only evidence and becomes corroboration.
- **The wrapper cannot invent a request.** It may only satisfy the one the
  module asked for, and the module names the exact path. §2.8.1's closed
  eight-pattern allowlist moves from "a thing TypeScript is asserted to obey"
  to "a thing TypeScript is *told*, by a Rust enum with eight variants".

§3.2's `drive()` tripwire is **kept, unchanged, for the engine**: over
`MemView` no discovery-core future may ever pend, and `now_or_never().expect(…)`
stays the assertion. Two drivers, two rules: the engine may never pend; the feed
may pend only at the transport.

### 1.3 Exported ABI (exact, replaces §3.3)

`wasm-bindgen`, `--target web`, plus a `nodejs` build for tests. Every fallible
method throws a `JsError` whose message is one canonical JSON object (§3.7,
amended in §1.7). All inputs are bytes or JSON strings; all outputs are JSON
strings, `Uint8Array`, or `DiscoverOut`.

```rust
#[wasm_bindgen]
pub struct Engine { /* MemStore + Option<SyncRun> + Option<KeySlot> */ }

#[wasm_bindgen]
impl Engine {
    // ---------------------------------------------------------------- setup

    /// `profile_json` is the §6.1 ChainProfile the caller expects — the
    /// built-in, or a custom one. Identity is pinned HERE, before any byte is
    /// requested; genesis.json is then checked against it inside apply_feed
    /// (§1.6 delta 3), not by the wrapper.
    #[wasm_bindgen(constructor)]
    pub fn new(profile_json: &str) -> Result<Engine, JsError>;

    /// Restore from a persisted state blob (§3.5). Verifies trailer hash,
    /// stamp and structural bounds against `profile_json`. Never partially
    /// loads: STATE_CORRUPT / STATE_VERSION / STATE_FOREIGN.
    pub fn load(profile_json: &str, blob: &[u8]) -> Result<Engine, JsError>;

    /// {"chain_id","pool","genesis_block","epoch_size","last_epoch",
    ///  "last_epoch_hash","last_epoch_to","history_floor","snapshot_basis",
    ///  "head","l1_accepted","slots","blocks","events","verified",
    ///  "engine_version","state_dirty"}
    /// `state_dirty` replaces §3.3's per-apply `state_changed` flag: with one
    /// sync call there is one answer, and the wrapper's export rule (§2.7)
    /// reads it once at the end.
    pub fn info(&self) -> String;

    // ------------------------------------------------------------ feed sync

    /// Start one sync. `cold_start` ∈ {"auto","snapshot","epochs"} (§4.2's one
    /// vocabulary). Returns the first Step. Throws SYNC_IN_PROGRESS if a run
    /// is already open.
    pub fn sync_begin(&mut self, cold_start: &str) -> Result<String, JsError>;

    /// Satisfy the outstanding request and get the next Step.
    /// `meta_json` is the response envelope (§1.3.1); `compressed` is the bytes
    /// exactly as served (or the raw bytes for uncompressed artifacts);
    /// `payload` is the inflated bytes for zstd artifacts, else None.
    /// The module hashes BOTH itself — TypeScript performs no verification.
    pub fn sync_supply(&mut self, meta_json: &str,
                       compressed: Option<Vec<u8>>,
                       payload: Option<Vec<u8>>) -> Result<String, JsError>;

    /// Abandon an open run. The mirror is left exactly as the last completed
    /// store write left it (every write is already atomic per §0.4.1's
    /// "writes that must not tear are single calls"), so an abort is never a
    /// torn state — it is simply an older state.
    pub fn sync_abort(&mut self);

    /// Canonical NDJSON of every request this Engine has emitted since
    /// construction, and its sha256. Key-independent BY CONSTRUCTION (§1.2);
    /// this is the artifact the demo hashes and compares across identities,
    /// and the artifact leg q' proptests.
    pub fn request_log(&self) -> String;
    pub fn request_log_sha256(&self) -> String;

    // ------------------------------------------------------ persisted state

    /// Epoch-derived state ONLY (§3.5, amended §1.5). Call when info().state_dirty.
    pub fn export(&self) -> Vec<u8>;

    // -------------------------------------------------- key-accepting entries
    // The closed set is exactly these four. Asserted by a test over the
    // wasm-bindgen-generated .d.ts (§1.8), not by reading the source.

    /// One full pass for one owner over the current mirror: checkpoint pass at
    /// last_epoch_to, live pass at head, spent refresh, note-registry diff.
    /// `key` is zeroized in place before return. `entropy32` MUST be 32 fresh
    /// bytes from crypto.getRandomValues on EVERY call (§3.6, unchanged).
    pub fn discover(&mut self, owner_hex: &str, key: &mut [u8],
                    sealed: Option<Vec<u8>>, entropy32: &[u8])
        -> Result<DiscoverOut, JsError>;

    /// Paged tx history per §1.1's paging contract. A walk that crosses
    /// history_floor TERMINATES the page set; an explicit
    /// from_block < history_floor throws HISTORY_UNAVAILABLE.
    pub fn history(&mut self, owner_hex: &str, key: &mut [u8],
                   sealed: Option<Vec<u8>>, from_block: Option<u64>, limit: u32)
        -> Result<String, JsError>;

    /// Reference-schema DiscoveryCursor JSON (base §7.4) extracted from a
    /// sealed blob — migration to compat/SDK without resync.
    pub fn export_reference_cursor(&self, key: &mut [u8], sealed: &[u8])
        -> Result<String, JsError>;

    /// Retain a key inside wasm linear memory under an opaque handle, so a
    /// long-lived subscriber does not have to keep one in the JS heap where
    /// zeroize means nothing (§2.4). `key` is zeroized in place; the retained
    /// copy lives in a SecretFelt and is zeroized by key_forget or by Drop.
    pub fn key_retain(&mut self, owner_hex: &str, key: &mut [u8])
        -> Result<u32, JsError>;
    pub fn key_forget(&mut self, handle: u32);
    pub fn key_forget_all(&mut self);

    /// discover()/history() over a retained handle. NOT key-accepting: they
    /// take a u32.
    pub fn discover_retained(&mut self, handle: u32, sealed: Option<Vec<u8>>,
                             entropy32: &[u8]) -> Result<DiscoverOut, JsError>;
    pub fn history_retained(&mut self, handle: u32, sealed: Option<Vec<u8>>,
                            from_block: Option<u64>, limit: u32)
        -> Result<String, JsError>;
}

#[wasm_bindgen(getter_with_clone)]
pub struct DiscoverOut {
    pub report_json: String,   // strk20-consumer SyncReport — field-identical to
                               // `strk20-sync sync --json` (one golden oracle)
    pub sealed: Vec<u8>,       // checkpoint-only sealed blob; hand back next time
    pub added_json: String,    // notes not present in the supplied sealed blob
    pub spent_json: String,    // nullifiers that flipped to spent this pass
}
```

**Why `key_retain` exists, given §3.3's "the key is zeroized before return".**
It does not weaken that rule; it names the case the rule could not cover. A
subscription that re-discovers on every feed poke needs the key at every poke.
Under §4.2's `subscribe(k, cb)` the key would sit in a JS `Uint8Array` for the
subscription's lifetime — unzeroizable, visible in a heap snapshot, and
retained by a layer that never announced it was retaining. `key_retain` moves
that retention into wasm memory where `zeroize` is real, gives it a name, an
explicit release, and a status bit the UI can render (§2.4). The honest limit
is unchanged and printed verbatim in the README: **the guarantee is
non-transmission, not host memory hygiene.**

The module has **no clock** (P-pure: no time import), so it cannot enforce an
auto-lock deadline itself. Auto-lock is a TypeScript timer calling
`key_forget`, and the README says so rather than implying a wasm-side
guarantee.

#### 1.3.1 Step and response envelopes (byte-precise)

Steps (returned by `sync_begin` / `sync_supply`), canonical JSON, one object:

```json
{"step":"fetch","seq":3,"artifact":"epoch","path":"/feed/epochs/00000412.strk20e.zst",
 "optional":false,"compressed":true,"decompress_cap":67108864,
 "conditional":null,"reason":"epoch 412 > last_epoch_applied 411"}

{"step":"fetch","seq":9,"artifact":"head","path":"/feed/head.ndjson",
 "optional":false,"compressed":false,"decompress_cap":null,
 "conditional":{"if_none_match":"\"<64-hex>\""},"reason":"tail refresh"}

{"step":"done","outcome":{"epochs_applied":2,"tail_rewound":false,
 "tail_changed":true,"head":14151989,"l1_accepted":14146900,
 "last_epoch_to":14149999,"snapshot_basis":14059999,"snapshot_rejected":false,
 "history_floor":14060000,"verified":"server-asserted","state_dirty":true}}
```

`artifact` is a **closed enum of eight variants** — `genesis`, `manifest`,
`epoch`, `epoch_anchor`, `snapshot`, `snapshot_anchor`, `anchors`, `head` —
mapping 1:1 onto §2.8.1's closed URL allowlist. `path` is emitted by the
module; the wrapper prefixes the configured feed base and appends **nothing**.
There is no variant carrying a query string, so a query string is
unrepresentable rather than forbidden.

Response envelope supplied back:

```json
{"seq":3,"status":200,"not_modified":false,"absent":false,"etag":null}
```

- `absent: true` is the *only* encoding of 404, and only for artifacts the Step
  marked `optional` (`epoch_anchor`, `snapshot_anchor`, `anchors`). A 404 on a
  non-optional artifact is `TRANSPORT` raised by the wrapper, never `absent`
  — the `crates/client/src/transport.rs` `get_optional` discipline (only 404
  means "not published") carried into TypeScript verbatim.
- `not_modified: true` is 304 for the one conditional artifact (`head`).
- Any other non-2xx is `TRANSPORT` from the wrapper and never reaches
  `sync_supply`.
- Supplying a `seq` that is not the outstanding one throws
  `SYNC_PROTOCOL {expected, got}`.

**Decompression, corrected against §3.4.** §3.4 says the module receives
uncompressed payloads and TypeScript is "bound by R-I (verify the `zst` hash
first…)". That places a verification obligation in TypeScript. Replace: the
wrapper supplies **both** buffers, and the module hashes both itself — the
`.zst` sha256 against `manifest.epochs[i].zst` / `manifest.snapshot.zst`, and
the payload sha256 against the content hash, exactly as `apply.rs:439-457`
already does. TypeScript's only remaining obligation is *not to inflate past
the cap the Step named*, which is a resource bound rather than a verification,
and it is backstopped by the payload hash. Cost: one extra buffer per artifact,
transient. Benefit: **zero verification logic in TypeScript**, which is the
whole point of the layer split.

### 1.4 `MemStore` against the real trait, and the scope separation (new)

`MemStore` implements `ConsumerStore` exactly as `crates/consumer/src/store.rs`
declares it — all of `meta_get`, `meta_set`, `is_empty`, `block_hash`,
`block_hashes`, `read_slot_as_of`, `full_slot_set_as_of`, `view`,
`reset_mirror`, `install_snapshot`, `replace_range`, `tail_generation`,
`notes`, `upsert_note`, `set_note_spent`, `delete_note`,
`delete_owner_notes` — plus whatever `refresh_spent` /
`prune_missing_notes` become when 0a lands them (see delta 1 in §1.6).

```rust
pub struct MemStore {
    mirror: RwLock<Mirror>,               // KEY-INDEPENDENT. The only thing export() sees.
    scope:  RwLock<Option<OwnerScope>>,   // KEY-DERIVED. Never exported. Zeroized on drop.
}

struct Mirror {
    identity:  FeedIdentity,
    meta:      BTreeMap<String, String>,  // closed allowlist only — see below
    slots:     BTreeMap<[u8;32], SlotRec>,        // base ∪ tail, tail rows tagged
    blocks:    BTreeMap<u64, BlockMeta>,          // ≥ history_floor
    events:    Vec<EventRec>,                     // ≥ history_floor, (block, index) ascending
    tail_floor: u64,                              // = last_epoch_to; rows above it are tail
    tail_gen:  u64,
}

struct OwnerScope {
    owner: Felt,
    meta:  BTreeMap<String, String>,              // cursors, ckpt_at, gen_
    notes: BTreeMap<(Felt, u64), NoteRow>,
}
impl Drop for OwnerScope { /* zeroize cursor JSON and nullifier bytes */ }
```

**`Send + Sync` without `unsafe`.** The trait requires `Send + Sync`
(`store.rs:95`) and every mutating method takes `&self`, so interior mutability
is mandatory. `std::sync::RwLock` compiles for `wasm32-unknown-unknown` with
std and is uncontended by construction (no threads without atomics; one Engine
per worker), so the cost is the atomic RMW. `RefCell` is rejected: it is not
`Sync`, and making it so requires the `unsafe` that §3.9's
`#![deny(unsafe_code)]` posture exists to forbid outside the wasm-bindgen
facade.

**The scope separation, and why it is a closed allowlist.** Two routing rules,
both hard:

1. `MemStore::meta_set` rejects with `SCOPE_VIOLATION` any key not in the
   **closed allowlist** of key-independent meta keys —
   `{pool, chain_id, epoch_size, genesis_block, last_epoch_applied,
   last_epoch_hash, last_epoch_to, head_etag, head_number, head_hash,
   l1_accepted, history_floor, snapshot_basis, snapshot_pending_grounding}` —
   *unless* an owner scope is open, in which case a key outside the allowlist
   is routed to `scope.meta`. `export()` serialises from the allowlist, by name,
   never by iteration.
2. `notes`, `upsert_note`, `set_note_spent`, `delete_note`,
   `delete_owner_notes` operate on `scope.notes` and require an open scope
   (`SCOPE_CLOSED` otherwise). `discover()` opens the scope from the decrypted
   sealed blob, runs, re-seals, and closes it in a `Drop` guard — including on
   the error path.

A denylist of prefixes (`cur_`, `ckpt_`, `gen_`) would have been the obvious
implementation and is **forbidden**: `sync.rs` composes those keys by
`format!("cur_{kind}_{a}")`, so the prefix set is a convention one refactor away
from drifting, and a drift leaks channel keys into the exported blob. The
allowlist fails closed on any key nobody thought about. This is the same
doctrine §2.8.1 applies to URLs, applied to the second place where a
key-derived string can escape.

**`MemView`** is `{ store: Arc<MemStore>, bound: u64 }` implementing
`RawStorageAccess + RawEventAccess` with futures Ready by construction. Reads
at `bound ≤ last_epoch_to` consult base rows only; reads at `head` consult
tail-then-base. `view()` calls `apply::check_bound_above_basis` rather than
re-deriving the §1.5.2 rule (`apply.rs:41-55` is the shared implementation).

**Reorg posture, reconciled.** §3.2 claims the browser needs *no persisted reorg
logic at all*, which stays true and is proven by leg **r**. It does not mean the
browser needs no reorg logic *in memory*: `apply_feed` still detects the masked
reorg and the tail contradiction, still bumps `tail_generation`, and `discover`
still rewinds live cursors for an owner whose `gen_<addr>` differs
(`sync.rs:320-338`). In the browser those live in the owner scope and last one
session, because the sealed blob is checkpoint-only (§3.6) — so a reorg cannot
reach anything durable, which is the actual claim.

### 1.5 State blob (§3.5 amended)

§3.5's grammar, header stamp, structural rejects and trailer hash stand
verbatim, including the corrected example and the three hard rejects (no line
above `last_epoch_to`, none below `history_floor`, `history_floor ==
snapshot_basis + 1`). Three amendments:

1. **The header gains `verified`** (`"replayed" | "server-asserted" |
   "anchored"`), because the grade is a property of how this mirror was built
   and must survive a reload. A blob whose grade is `anchored` loads as
   `server-asserted` when the session configures no anchor RPC — the grade is
   never *upgraded* by memory of a past session.
2. **Export is by allowlist, not by iteration** (§1.4). §3.5's prose — *"Only
   epoch-derived state is ever exported"* — becomes a property of the serializer's
   shape rather than of its author's care.
3. **`load` additionally rejects a blob whose `chain_id`/`pool`/`genesis_block`/
   `epoch_size` disagree with the *profile* passed to `load`**, not only with a
   genesis document. Under §1.3 the profile is the identity source at
   construction, and genesis.json is checked against it inside `apply_feed`.

### 1.6 Deltas this design asks of the in-flight 0a refactor

Four, in descending order of how much they matter to me.

1. **Give cursors their own trait methods.** Today the discovery half persists
   cursors through `meta_set` (`sync.rs:116-118`). That routes key-derived JSON
   through the same API as `head_etag`. Preferred fix:

   ```rust
   fn cursor_get(&self, kind: CursorKind, owner: &Felt) -> Result<Option<String>>;
   fn cursor_put(&self, kind: CursorKind, owner: &Felt, json: Option<&str>) -> Result<()>;
   fn owner_generation(&self, owner: &Felt) -> Result<u64>;
   fn set_owner_generation(&self, owner: &Felt, gen: u64) -> Result<()>;
   ```

   Then the type system separates key-independent mirror metadata from
   key-derived per-owner state, `FeedStore` keeps writing both into its `meta`
   table (no behaviour change, no migration), and `MemStore`'s exporter
   *cannot name* the key-derived side. If 0a ships without this, §1.4's
   allowlist is the mandatory compensating control and the scanner-over-`export()`
   assertion in leg q' is the mandatory test — but a control that fails closed is
   worth less than a shape that cannot express the mistake.

2. **`refresh_spent` and `prune_missing_notes` on the trait** (as the
   orchestrator's 11-method surface has them) rather than as `FeedStore`
   inherent methods. Both need a read view *and* the registry, so both must be
   host-implemented; `MemStore`'s versions operate on `scope.notes`. Their
   inputs include nullifiers, which are key-derived — that is unavoidable and
   is exactly why the scope exists.

3. **Move the profile check into `apply_feed`.** §6.2's stamping matrix requires
   the client to check genesis against the *expected profile* as well as against
   stored meta. `apply_feed` today compares genesis against stored meta only
   (`apply.rs:119-143`). Add an `expect: &ChainProfile` parameter and do it
   there, so both hosts get it and neither wrapper re-implements it. This is
   also what lets `Engine::new` take a profile instead of a genesis document.

4. **`ApplyOutcome` gains `verified: &'static str` and `state_dirty: bool`.**
   `verified` is computed in `sync_once` today (`sync.rs:300-304`) from
   `snapshot_basis` and the ring-6 result; the browser needs it from the apply
   half because `sync()` is key-free (§2.2). `state_dirty` replaces §3.3's
   per-apply `state_changed` flags.

### 1.7 Error model (§3.7 amended)

§3.7's table stands, including the deletion of `STATE_STALE` and the rule that
staleness is a return value. Amendments:

| change | code | details | retryable |
|---|---|---|---|
| **removed** | `FEED_ADVANCED_MIDSYNC` stays but is now raised by `apply_feed` itself (`apply.rs:273-279`), not by a wrapper-orchestrated `apply_head` | `{tail_from, floor}` | yes |
| **added** | `SYNC_PROTOCOL` — the wrapper supplied a response for a request that is not outstanding, or re-entered a closed run | `{expected_seq, got_seq}` | no |
| **added** | `SYNC_IN_PROGRESS` — `sync_begin` while a run is open | — | no |
| **added** | `SCOPE_VIOLATION` — a key-derived meta write attempted with no owner scope open (internal invariant; escaping it is a bug, and it is `INTERNAL`-adjacent by design so it can never be caught and ignored) | `{key}` | no |
| **added** | `SNAPSHOT_UNREACHABLE` — §11.3 reachability found no anchor between basis and head, or reproduced none. Already raised by `apply.rs:569-631` and missing from §3.7's table. | `{basis, head, tried}` | no |
| **added** | `SNAPSHOT_UNAVAILABLE` — `coldStart:'snapshot'` against a feed publishing none (`apply.rs:173-178`). Also missing from §3.7. | — | no |
| **clarified** | `DECOMPRESS_LIMIT` is raised by **TypeScript** (the only verification-adjacent obligation it holds) and by Rust for uncompressed-artifact size bounds | `{artifact, cap}` | no |

`check_manifest` is **deleted from the ABI**. §3.3 exported it so TypeScript
could arbitrate `ok`/`behind`/`diverged` before deciding what to apply; under
§1.2 `apply_feed` makes that decision internally (`apply.rs:197-207` is the
`diverged` test; the epoch loop is the `behind` case; zero Steps beyond
`genesis`/`manifest`/`head` is `ok`). Keeping a second staleness arbiter that
nothing calls is how the two drift apart. The three discriminants survive as an
**outcome**: `SyncResult.staleness: 'ok' | 'behind' | 'diverged'`, derived from
`epochs_applied` and whether a `reset_mirror` happened, and leg q' still
asserts the three cases on the three constructed manifests.

### 1.8 Purity, size and the compile-fail locks (§3.9 amended)

§3.9 stands entire — feature-resolved dependency walk, the corrected
`deny`-not-`forbid` posture with exactly one documented `#[allow]` scope, the
checked-in import allowlist diffed as a file, the single wire-cost denominator,
and the FILL-IN discipline for the budget. Amendments:

- **The key-accepting entry lock is now a named allowlist**, because the count
  changed from two to four. CI parses the wasm-bindgen-generated `.d.ts` and
  asserts the set of exported methods taking a `Uint8Array` named `key` is
  exactly `{discover, history, export_reference_cursor, key_retain}` — a list,
  diffed as a file, for the same reason §3.9 stopped pattern-matching import
  names.
- **New gate: the request-emitter purity proptest.** `strk20-consumer`, native,
  over a `MemStore` and a scripted transport: for any two distinct
  (address, key) pairs and any feed fixture, `request_log()` after
  `sync_begin`/`sync_supply` to completion is **byte-identical**, and remains so
  when `discover()` is interleaved between syncs. This is P-blind as a theorem;
  the wire capture stays as its independent empirical check.
- **New gate: the import allowlist must contain no time source.** Explicitly
  named because `key_retain` invites a wasm-side auto-lock, and adding one
  would import a clock. The auto-lock is TypeScript's, permanently.

---

## 2. §A4 revised — the npm package

### 2.1 Name, layout, positioning (§4.1 amended)

Unscoped **`strk20-discovery`** stands. Layout:

```
strk20-discovery
├── dist/index.js|d.ts        KeylessClient, types, errors           (main)
├── dist/sdk.js|d.ts          LocalDiscoveryProvider                  ("strk20-discovery/sdk")
├── dist/worker.js|d.ts       the worker host                         ("strk20-discovery/worker")
├── dist/delegated.js|d.ts    DelegatedClient                         ("strk20-discovery/delegated")
├── dist/engine_bg.wasm       strk20-engine, lazily instantiated
└── README.md
```

Two changes from §4.1:

- **`DelegatedClient` moves to its own subpath.** In delegated mode the viewing
  key leaves the browser. That is a legitimate self-host posture and a
  materially different trust boundary, and it should not be one autocomplete
  away from `KeylessClient` in the same import. The subpath is a speed bump with
  a docstring on it. (§4.8's construction-time chain check and the
  `assertUncheckedNetwork` gate are unchanged and reproduced in §2.9.)
- **The README opens with the positioning fact of §0.2**, in these words: *"If
  your app talks to a user's wallet through the Starknet Wallet API, you do not
  need this package and cannot use it — the wallet holds the viewing key and
  discovers notes itself. This package is for software that holds a viewing key:
  wallets, key-holding backends, and SDK integrations."* Getting this wrong
  costs an integrator a day and costs us the report that our API "doesn't
  work".

Supply-chain posture unchanged: no install scripts, npm provenance, `files`
whitelist, the wasm sha256 in the README and asserted in CI, one pinned runtime
dependency (`fzstd`). Wire cost is §3.9's single denominator with no number
quoted before step 4 measures one.

### 2.2 The shape change: feed sync is key-free and separately callable

> **§4.2 quoted, replaced.** *"`getNotes(k: KeyRef): Promise<NotesResult>;
> subscribe(k: KeyRef, cb: (ev: DiscoveryEvent) => void): () => void;"*
>
> Replaced by the interface below, whose defining property is that **advancing
> the feed takes no key**.

Reason. Under §4.2 the only way to make progress is to hand over a key, so a
long-lived client holds one for its lifetime, and the natural integration is
"construct the client with the key at app start". That is the shape that leaks:
it maximises key residency, it makes the key-free phase invisible, and it
tempts a wallet into keeping an unlocked key alive while the UI is idle. It
also fights the actual wallet flow, where the app boots locked and the user
unlocks later — during which time we could have been syncing.

Splitting the phases costs one extra method and buys four things: the key-free
phase can be warmed before unlock; the two phases are separately observable and
separately measurable (which is what makes §3's demo honest); key residency
becomes a deliberate, named act (§2.4); and the P-blind demonstration is a
program you can run with **no key in the process at all**.

### 2.3 Types (exact `.d.ts`)

```ts
// ---------------------------------------------------------------- primitives
export type Hex = `0x${string}`;
export type ChainName = 'mainnet' | 'sepolia';
export type ColdStartMode = 'auto' | 'snapshot' | 'epochs';
export type Verified = 'anchored' | 'server-asserted' | 'replayed';
export type Persistence = 'indexeddb' | 'memory';
export type TransportState = 'sse' | 'polling' | 'offline';
export type Staleness = 'ok' | 'behind' | 'diverged';

/** 32-byte BE viewing key. Uint8Array ONLY: a hex string would create
 *  unzeroizable copies and make the honest-zeroization statement cover nothing.
 *  The type refuses the footgun rather than documenting it. */
export interface KeyRef { address: Hex; viewingKey: Uint8Array; }

export interface Note {
  token: Hex; index: number; noteId: Hex; nullifier: Hex;
  amount: bigint; blockNumber: number; sender: Hex; spent: boolean;
}

export interface HistoryTx {
  kind: 'deposit' | 'transfer-in' | 'transfer-out' | 'withdrawal' | 'registration';
  blockNumber: number; txHash: Hex; token: Hex; amount: bigint;
  counterparty: Hex | null;
}

// ------------------------------------------------------------------ feed state
export interface FeedState {
  head: number; l1Accepted: number;
  lastEpoch: number; lastEpochTo: number;
  historyFrom: number;               // §1.1 floor; 0 for a replayed mirror
  snapshotBasis: number | null;
  verified: Verified;                // §1.5.1 — a word, never a boolean
}

// ------------------------------------------------------------- measurement
export interface RequestRecord {
  seq: number;
  origin: 'feed' | 'anchor-rpc' | 'delegated';
  method: 'GET' | 'POST';            // POST only ever on origin 'delegated'
  url: string;                       // absolute, exactly as issued
  artifact: 'genesis' | 'manifest' | 'epoch' | 'epoch_anchor' | 'snapshot'
          | 'snapshot_anchor' | 'anchors' | 'head' | 'live' | 'rpc' | 'keyed';
  status: number;                    // 0 = network failure
  bytesOnWire: number;               // measured from the received ArrayBuffer
  bytesInflated: number | null;
  startedAt: number;                 // performance.now()
  durationMs: number;
  source: 'network' | 'indexeddb';   // an IDB hit is recorded, and never counted as a request
}

export interface SyncTiming {
  totalMs: number;
  networkMs: number;        // summed chokepoint durations (wall, may overlap)
  decompressMs: number;     // fzstd
  engineMs: number;         // summed time inside sync_begin/sync_supply
  idbReadMs: number; idbWriteMs: number;
  workerRoundTripMs: number;
}

export interface DiscoverTiming {
  totalMs: number; engineMs: number; sealMs: number;
  workerRoundTripMs: number; passes: number;
}

export interface SessionStats {
  requests: RequestRecord[];         // every request this client issued, in order
  requestCount: number;
  bytesOnWire: number;
  bytesInflated: number;
  /** sha256 of the module's canonical request log (§1.3). Key-independent by
   *  construction; two clients on the same feed at the same manifest MUST
   *  produce the same value. This is the number to compare across identities. */
  requestLogSha256: Hex;
  syncs: SyncTiming[];
  discoveries: DiscoverTiming[];
}

// -------------------------------------------------------------------- results
export interface SyncResult extends FeedState {
  changed: boolean;                  // anything applied at all
  staleness: Staleness;              // §1.7 — replaces check_manifest
  epochsApplied: number;
  coldStarted: boolean;
  snapshotRejected: boolean;
  tailRewound: boolean;
  timing: SyncTiming;
  requests: RequestRecord[];         // just this call's
}

export interface NotesResult extends FeedState {
  notes: Note[];
  balances: Map<Hex, bigint>;
  added: Note[];                     // vs the sealed cursor we were given
  spent: Note[];                     // flipped to spent this pass
  complete: boolean;                 // incoming && outgoing cursors complete
  cursorReset: boolean;              // sealed blob unusable → fresh discovery
  raw: unknown;                      // untouched SyncReport JSON (oracle equality)
  timing: DiscoverTiming;
}

export interface HistoryResult {
  transactions: HistoryTx[];
  complete: boolean;                 // §1.1 paging contract
  completeFrom: number;              // walk's last completed bound, ≥ historyFrom
  registrationAvailable: boolean;
}

export interface ClientStatus {
  mode: 'keyless' | 'delegated';
  transport: TransportState;
  persistence: Persistence;
  feed: FeedState | null;            // null before the first sync
  unlocked: Hex[];                   // addresses with a retained key, right now
  worker: boolean;
  ready: boolean;
}

// --------------------------------------------------------------------- events
export type DiscoveryEvent =
  | { type: 'feed';   state: FeedState; changed: boolean;
      cause: 'sse' | 'poll' | 'manual'; timing: SyncTiming }
  | { type: 'notes';  address: Hex; added: Note[]; spent: Note[];
      state: FeedState; timing: DiscoverTiming }
  | { type: 'reorg';  rewoundTo: number }
  | { type: 'status'; transport: TransportState; persistence: Persistence }
  | { type: 'request'; record: RequestRecord }      // one per chokepoint call
  | { type: 'error';  error: Strk20Error; recovering: boolean };

export type Unsubscribe = () => void;

// -------------------------------------------------------------------- errors
export type Strk20ErrorCode =
  // §3.7, from the module
  | 'FEED_HASH_MISMATCH' | 'FEED_CHAIN_BROKEN' | 'FEED_MALFORMED'
  | 'FEED_EPOCH_GAP' | 'FEED_ADVANCED_MIDSYNC'
  | 'SNAPSHOT_ROOT_MISMATCH' | 'SNAPSHOT_ANCHOR_MISSING' | 'SNAPSHOT_NOT_EMPTY'
  | 'SNAPSHOT_UNREACHABLE' | 'SNAPSHOT_UNAVAILABLE'
  | 'BOUND_BELOW_SNAPSHOT' | 'CHAIN_MISMATCH'
  | 'STATE_CORRUPT' | 'STATE_VERSION' | 'STATE_FOREIGN'
  | 'SEALED_STATE_MISMATCH' | 'KEY_INVALID'
  | 'ENTROPY_INVALID' | 'ENTROPY_REUSED'
  | 'DISCOVERY_INCOMPLETE' | 'HISTORY_UNAVAILABLE'
  | 'SYNC_PROTOCOL' | 'SYNC_IN_PROGRESS' | 'INTERNAL'
  // npm layer
  | 'TRANSPORT' | 'DECOMPRESS_LIMIT' | 'CONFIG_INVALID'
  | 'KEY_LOCKED' | 'CLIENT_CLOSED'
  // delegated / serve
  | 'AUTH_REQUIRED' | 'INVALID_TOKEN' | 'INVALID_BODY'
  | 'SERVICE_UNAVAILABLE' | 'BLOCK_REORGED' | 'INVALID_QUERY';

export class Strk20Error extends Error {
  readonly code: Strk20ErrorCode;
  readonly details?: Record<string, unknown>;
  readonly retryable: boolean;
}
```

**Two type-level locks worth naming.** `DiscoveryEvent` is a closed union with
no member carrying a key, a cursor, or an unstructured `string` payload — so an
event handler, a logger or a telemetry pipe attached to it cannot receive key
material. `Strk20ErrorCode` is a closed union rather than `string`, so a new
code cannot be introduced without touching the union, which is where the
scanner's error-string assertion is anchored.

### 2.4 The client interface

```ts
export interface DiscoveryClient {
  /** Advance the feed. TAKES NO KEY. Idempotent; concurrent calls coalesce. */
  sync(opts?: { coldStart?: ColdStartMode; force?: boolean }): Promise<SyncResult>;

  /** Discover for one identity over the current mirror.
   *  `sync: true` (default) advances the feed first. */
  getNotes(id: KeyRef | Unlocked, opts?: { sync?: boolean }): Promise<NotesResult>;

  history(id: KeyRef | Unlocked,
          opts?: { fromBlock?: number; limit?: number }): Promise<HistoryResult>;

  /** Retain a key inside the worker's wasm memory under a handle. THIS IS THE
   *  ONLY WAY the client holds key material across calls, it is visible in
   *  status().unlocked, and it is revocable. Requires worker: true. */
  unlock(k: KeyRef, opts?: { autoLockMs?: number }): Promise<Unlocked>;

  /** Feed subscription. TAKES NO KEY. With `identity`, the client additionally
   *  runs discovery on each poke and emits `notes` events. */
  subscribe(cb: (ev: DiscoveryEvent) => void,
            opts?: { identity?: Unlocked }): Unsubscribe;

  status(): ClientStatus;
  stats(): SessionStats;
  close(): Promise<void>;            // locks every handle, terminates the worker
}

export interface Unlocked {
  readonly address: Hex;
  readonly locked: boolean;
  lock(): Promise<void>;             // key_forget in the worker
}
```

**`unlock` is the whole key-custody story, and it is deliberately loud.**
Retention has a name, a lifetime, an explicit release, an auto-lock, and a
status bit any UI can render. `subscribe` without `identity` is a fully useful
mode (feed events only), so the *default* subscription retains nothing.
`worker: false` makes `unlock()` throw `CONFIG_INVALID {option:'worker',
reason:'unlock requires a worker'}` — on the main thread there is no isolation
worth the claim, and offering it anyway would be selling a padlock painted on a
door.

Auto-lock is a TypeScript timer (the module has no clock, §1.3). The README says
that in those words.

**`getNotes` accepts `KeyRef | Unlocked`.** With a `KeyRef` the buffer is
transferred to the worker (detaching the caller's), used, zeroized, and never
retained — §4.2's transfer discipline, unchanged. With an `Unlocked` no key
crosses the boundary at all.

### 2.5 Keyless data flow (§4.3 amended)

```
construct   → resolve profile (built-in by name, or the caller's ChainProfile)
            → open IDB (or memory fallback, reported through status())
            → spawn worker, instantiate wasm, Engine.new(profile)
            → if state blob present: Engine.load(profile, blob)   [Design M only]

sync()      → engine.sync_begin(coldStart)
              loop:
                step = fetch                → net.get(step.path)          [chokepoint]
                                            → if compressed: fzstd under step.decompress_cap
                                            → engine.sync_supply(meta, compressed, payload)
                step = done                 → break
            → persist artifacts (Design R) and, if info().state_dirty, export() (Design M)
            → resolve SyncResult

getNotes(id)→ [optionally sync() first]
            → sealed = IDB.cursors[keyId]
            → engine.discover(addr, key, sealed, crypto.getRandomValues(32))
            → IDB.cursors[keyId] = out.sealed
            → resolve NotesResult with added/spent

subscribe() → EventSource(feedUrl + '/feed/live')      [no auth, no params]
            → on head|epoch|snapshot: sync(); if identity: getNotes(identity)
            → on error / 404 / 405: degrade to polling at pollIntervalMs,
              status event, no error surfaced (§2.5)
```

`navigator.locks.request('strk20:<db>', …)` serialises sync passes across tabs.
§4.3's **scope correction** is reproduced verbatim and is not weakened: last-
writer-wins is safe for `meta`/`artifacts`/`state` because every persisted value
is self-verifying; it is **not** safe for `cursors`, where forking tabs are the
nonce-collision case, and what makes *that* safe is fresh
`crypto.getRandomValues` on every `discover()` with `ENTROPY_REUSED` as the
backstop. Web Locks reduce the fork's frequency and are not the mitigation.

### 2.6 The fetch chokepoint (new, and load-bearing)

Every byte the client fetches goes through one module, `src/net.ts`, exporting
one function. Nothing else in the package may reference `fetch`, `XMLHttpRequest`,
`EventSource`, `navigator.sendBeacon` or `import()` of a URL.

```ts
// src/net.ts — the ONLY place this package touches the network.
export async function request(spec: FetchSpec): Promise<FetchOutcome>;
```

Obligations:

1. Emits a `RequestRecord` for every call, before and after, onto the event bus
   — which is what `stats()` accumulates and what the demo renders.
2. Builds the URL as `base + step.path` with **no interpolation of any
   caller-supplied string** beyond the base. `step.path` comes from the module's
   closed artifact enum, so a query string is unrepresentable.
3. Sets no request header beyond `Accept`, `If-None-Match` (head only) and, in
   delegated mode, `Authorization`. No cookies (`credentials: 'omit'`), no
   `Referer` beyond the browser default, no custom UA.
4. Rejects, at runtime, any URL not matching §2.8.1's eight whole-path patterns
   plus `/feed/live` plus (when configured) the anchor-RPC origin. Whole-path
   match, never a prefix, never `startsWith('/feed/')`.

Mechanical enforcement: a build-time scan asserting that no file under `src/`
other than `net.ts` contains any of those identifiers, run in CI beside leg
**u**. This is the TypeScript half of §7.2's compile-locked seam — TypeScript has
no type system move that expresses "this module cannot do IO", so the lock is a
scan, and a scan over one filename is a lock a reviewer can actually check.

### 2.7 Persistence and cache invalidation (§4.4, §4.5 amended)

IndexedDB layout is §4.4's, with the database named
`strk20-discovery:<chain_id>:<pool>`, the corrected `keyId` (**full 32-byte HKDF
output as 64 lowercase hex characters, no slice**), the persisted-*and*-refetched
`genesis`, and the five quirk mitigations. Two schema notes:

- `artifacts` values stay `{hash, zbytes}` — **compressed exactly as served** —
  because Design R's whole point is that a reload re-runs the same verification
  ladder over the same bytes the network would have delivered. Storing inflated
  payloads would put a TypeScript decompressor between the network and the
  hash the module checks.
- `state` gains a sibling row `state_meta = {engine_version, profile_hash,
  written_at, source_manifest_hash}` so a stale-format blob is detectable
  without parsing the blob.

**Cache invalidation rules, complete.** Each row is either self-verifying or
deleted; nothing is repaired in place.

| record | written | invalidated by | action |
|---|---|---|---|
| `meta.genesis` | first sync | re-fetched `/feed/genesis.json` differs bytewise | `CHAIN_MISMATCH` thrown **before any row lands**; nothing deleted; the client refuses to proceed. This is the check that catches a feed changing its own genesis (§4.4). |
| `meta.*` (identity, epoch cursor) | every apply | profile mismatch on construction | whole database is a different name; nothing to invalidate |
| `artifacts.epoch:<n>` | after the module accepted the payload | manifest no longer lists `n`, or lists it with a different `hash`/`zst` | delete row, refetch. Never trusted: every load re-hashes. |
| `artifacts.snapshot` / `artifacts.anchor` | after ring 5 passed | `manifest.snapshot.e` or `.hash` changed | delete both rows together (an anchor without its snapshot attests nothing) |
| `state.folded` | after a sync reported `state_dirty` | `load` throws any `STATE_*`; `engine_version` major differs; `profile_hash` differs; the mirror's `last_epoch_hash` is not the manifest's for that epoch (the `diverged` case) | delete, fall through to Design R, then to the network. Strictly a cache: deleting it is always correct. |
| `cursors.<keyId>` | after every `discover` | AEAD open fails (wrong key, tamper, cross-network AAD) → treated as **no cursor**, fresh discovery, `cursorReset: true` surfaced; or `sealed.ckpt_at > mirror.last_epoch_to` (a cursor from a future the mirror no longer has) → same treatment | never an error the caller must handle; always a slower correct pass |
| everything | — | quota eviction, private window, blocked `open` | a cold start, never corruption; `persistence: 'memory'` reported through `status()` |

**A non-obvious rule, stated because getting it wrong costs a full rediscovery:**
re-cold-starting from a *newer* snapshot raises `history_floor` but does **not**
invalidate sealed cursors or the note registry. Pool slots are write-once
(measured: 134,879 distinct slots across 139,131 writes, 96.9 % first writes),
so slot state below the new floor is complete; only *events* are missing.
Discovery and spent-state read slots and nullifiers, so they are unaffected;
only `history()` is affected, and that is exactly what `historyFrom` /
`complete` report.

**Design R / Design M.** §4.5 stands verbatim, including the honest trust
statement (M trusts IndexedDB integrity between loads; no secret exists to MAC a
key-independent blob; the marginal risk over R is persistence of tampering
beyond the tampering code's presence) and the opportunistic idle-callback refold
audit. §4.6's pre-registered fold-time gate stands, with its FILL-IN unfilled.
One thing the live run does settle, and it should be recorded rather than
pre-empting the gate: **the epochs lane is already known to need M.** 5.97 s
cold fold of full mainnet history natively, and WASM is slower, not faster — so
`coldStart: 'epochs'` cannot re-fold per page load. That is §4.6's *second*
decision rule (`t_cold(L2) > 2000 ms` → M for epochs sessions) reached by a
native measurement rather than the browser bench, and the bench still owns the
L1/snapshot-lane verdict, which is the one that decides the default.

The `persist?: 'raw' | 'folded'` union stays narrowed at publish time with the
`CONFIG_INVALID {option:'persist', got:'folded', built:['raw']}` runtime reject
(§4.5). No `'auto'` mode.

### 2.8 The SDK adapter is the front door

```ts
// "strk20-discovery/sdk"
export function localDiscoveryProvider(
  client: DiscoveryClient
): DiscoveryProvider;                 // the reference SDK's own interface
```

All base §12.1 cursor-conversion semantics carry over verbatim, so
`NotesCursor`/`ChannelCursor` round-trip identically to
`IndexerDiscoveryProvider`, and `Engine.export_reference_cursor` gives a
zero-resync migration path in both directions. Given §0.2, this adapter is the
shortest sentence we can say to our actual customer: *replace one provider,
keep everything else, and the key stops leaving the process.*

### 2.9 `DelegatedClient` (§4.8, carried with one addition)

Unchanged: the reference compat wire (`POST /v1/sync/incoming_state`,
`/v1/sync/outgoing_state`, `/v1/sync/preflight_check`, `POST /v1/history`);
fetch-based SSE with an `Authorization: Bearer` header rather than
`EventSource`, because `/feed/live` on `serve` is inside the auth perimeter;
chain identity verified against `/health`'s amended `chain_id`/`pool` **before
any key is sent**, with a hard refusal to construct (never a "verify if
present" mode) unless `assertUncheckedNetwork` is passed.

Addition, symmetric with `serve`'s own `--allow-remote` gate: a `serverUrl`
that is neither loopback nor `https:` is refused with `CONFIG_INVALID
{option:'serverUrl', reason:'plaintext non-loopback'}` unless
`allowInsecureServer: true`. A viewing key travelling in clear over a LAN is
not a trade-off anyone made deliberately.

### 2.10 API shapes that tempt a leak, and how each is refused

| Temptation | Refusal |
|---|---|
| Pass the key as a hex string | `viewingKey: Uint8Array` only (§4.2, kept) |
| Hold the key for the client's lifetime | No constructor takes a key; retention only via `unlock()`, visible in `status().unlocked`, revocable, auto-locking |
| Ask the server about an address | No transport method takes a user-derived parameter (§7.2 compile lock); in the browser the module *emits* the request set, so the wrapper has nothing to parameterise |
| "Just fetch the epochs containing my notes" | No API accepts an epoch or block selector derived from discovery output (§1.6). Full history is `coldStart:'epochs'`, all-or-nothing, key-independent |
| Log the key while debugging | The client never logs. The only outbound channel is `DiscoveryEvent`, a closed union with no key-bearing member |
| Reach delegated mode by accident | Separate import subpath; explicit chain check; explicit insecure-transport gate |
| Trust a boolean "verified" | There is no boolean. `verified` is one of three words, and `'server-asserted'` is what a default snapshot cold start returns |

---

## 3. The demo application

### 3.1 What the demo can honestly be

The brief asks for deposit / send / withdraw buttons. We have no write path,
no signing, no prover — deliberately ([design notes §4](../../../notes/2026-08-30-consumer-path-discussion.md)).
And per §0.2 a Wallet-API connection yields no viewing key, so a "connect
wallet, then discover" demo is not merely out of scope, it is impossible.

The honest framing is also the strongest one, and it is the project's own:
**we are the read half of every write.** The user performs the write wherever
they already perform it — their wallet, the SDK, a script — and the demo is
what watches for it, keylessly, and reports the moment it lands and how long
that took. That maps exactly onto the brief's mechanic: a pending last line
that mutates in place ("waiting for the note…") and commits with its elapsed
time. The multi-stage approve-then-swap spirit survives too, because the flow
genuinely has stages: sync (no key) → unlock (key, local) → discover → watch.

**Two run modes, and the mode is always on screen.**

- **LIVE** — points at a real feed (Sepolia by default). The write buttons open
  the instructions for performing that action in your own wallet, and the demo
  waits. Every number is measured now.
- **REPLAY** — points at a pinned static feed directory captured from Sepolia,
  containing the two transactions we made: the note at block **14,339,115** and
  the spend at **14,340,785**. The write buttons advance the demo's view of the
  feed to just past the relevant block, so discovery genuinely runs and
  genuinely finds the note. Timings are real; the *event* is recorded. The mode
  chip reads `REPLAY — recorded Sepolia history, discovery is live` and cannot
  be dismissed.

REPLAY exists because a demo that depends on a stranger having 3 STRK and a
privacy-enabled wallet is a demo nobody sees, and because it runs offline from
a static directory — which is itself a claim worth demonstrating (no server,
no API, no account).

### 3.2 Screen

One page, four regions, no routing, no framework requirement beyond what the
package needs.

```
┌──────────────────────────────────────────────┬───────────────────────────────┐
│  A · COLD vs WARM                            │  C · WHAT WENT TO THE NETWORK │
│  ┌───────────────┐  ┌───────────────┐        │  connect-src: <feed origin>   │
│  │ COLD          │  │ WARM          │        │  ─────────────────────────────│
│  │ 0 rows in IDB │  │ IDB restored  │        │  GET /feed/genesis.json  412 B│
│  │ requests  518 │  │ requests    3 │        │  GET /feed/manifest.json 47 kB│
│  │ bytes   16 MB │  │ bytes   61 kB │        │  GET /feed/epochs/000000…     │
│  │ total  ____ms │  │ total  ____ms │        │  …                            │
│  │  net   ____   │  │  net   ____   │        │  ─────────────────────────────│
│  │  zstd  ____   │  │  idb   ____   │        │  518 requests · 16,004,112 B  │
│  │  fold  ____   │  │  fold     0   │        │  log sha256 3f9c…a71          │
│  └───────────────┘  └───────────────┘        │                               │
│                                              │  IDENTITY A  3f9c…a71         │
├──────────────────────────────────────────────┤  IDENTITY B  3f9c…a71         │
│  B · LOG (scrolling; last line mutates)      │  ✔ IDENTICAL                  │
│  ─────────────────────────────────────────   │                               │
│  feed synced to 14,340,535             1.4 s │  scanner: key not found       │
│  unlocked 0x04f2…9ab (key stayed here)       │           in 13 encodings     │
│  discovered 1 note · 3.0 STRK          1.19s │  addr not found (13)          │
│  ▸ waiting for the note…               4.7 s │  [ self-test ] plants the key │
├──────────────────────────────────────────────┴───────────────────────────────┤
│  D ·  [deposit]  [send]  [withdraw]      subscription (●ON) [check now]       │
└──────────────────────────────────────────────────────────────────────────────┘
```

Region A answers the orchestrator's first requirement (cold and warm side by
side, not sequentially). Region C answers the second and third (requests and
bytes; the live URL list, the identity comparison, the scanner). Region B is
the brief's scrolling log with its mutating last line. Region D is the brief's
stage buttons and subscription toggle.

### 3.3 State machine (exact)

States, and the only legal transitions:

```
boot ──ready──▶ cold_choice
                   │
     ┌─────────────┴──────────────┐
     ▼                            ▼
 cold_run                     warm_run            (both write into region A)
     └──────────┬─────────────────┘
                ▼
              idle ◀──────────────────────────┐
                │                             │
     unlock     ▼                             │
            unlocking ──ok──▶ unlocked        │
                │                             │
     getNotes   ▼                             │
            discovering ──notes──▶ unlocked   │
                │                             │
     action     ▼                             │
            awaiting_note ──note found──▶ unlocked
                │  (last log line pulses; SSE or poll pokes drive sync+discover)
                └── timeout(none) ──▶ unlocked (line commits as "no change")
                                              │
     lock / autolock ─────────────────────────┘
     any error ▶ error_shown (a committed log line, red) ▶ previous state
     close ▶ boot
```

Guards worth naming:

- `cold_run` is only enterable after `indexedDB.deleteDatabase(name)` **resolves**
  and a fresh `Engine` has been constructed. If deletion is blocked or storage
  is unavailable, the cold column renders `unavailable — could not clear
  storage` and the run does not start. A cold number that was not measured cold
  is worse than no number.
- `unlocked` renders a padlock bound to `status().unlocked`, polled from the
  client, not from local UI state — so an auto-lock that fires while the tab is
  idle changes the padlock without the UI having to know.
- `awaiting_note` never blocks the log: other lines may commit above it; it
  stays last by construction because it is the pending line.

### 3.4 The log line model

```ts
interface LogLine {
  id: string;
  stage: 'feed' | 'identity' | 'discover' | 'await' | 'network' | 'error';
  text: string;                       // committed text
  pendingText?: string;               // shown while status === 'pending'
  status: 'pending' | 'ok' | 'warn' | 'err';
  startedAt: number;                  // performance.now()
  elapsedMs?: number;                 // set at commit
  metrics?: { label: string; value: string; provenance: Provenance }[];
}
type Provenance = 'measured' | 'recorded' | 'derived';
```

Exactly one line may be `pending` at a time. While pending it renders its live
elapsed time (rAF-driven, 100 ms granularity) and its `pendingText`; on resolve
it swaps to `text`, freezes `elapsedMs`, and the next line becomes the last.
That is the brief's mechanic, and it is also the only animation in the page.

### 3.5 What is logged, exactly

| stage | line (committed form) | metrics attached |
|---|---|---|
| feed cold | `cold start · folded 515 epochs to 14,151,989` | total, net, zstd, fold, requests, bytes, `verified` grade |
| feed warm | `warm start · restored from IndexedDB` | total, idb read, requests (should be 3), bytes |
| feed sync | `feed advanced 14,151,989 → 14,152,004` | total, requests, bytes, `staleness` |
| feed sync (nothing) | `feed unchanged at 14,152,004` | total, requests (1: conditional head), bytes (0 on 304) |
| identity | `unlocked 0x04f2…9ab — key stayed in this tab` | — (never the key, never a truncation of the key) |
| discover | `discovered 1 note · 3.0 STRK` | discover total, engine ms, passes |
| discover (delta) | `+1 note 0xce52…7ff · 3.0 STRK @14,339,115` | time-to-discover (§3.6) |
| discover (spend) | `note 0xce52…7ff is now spent` | time-to-discover |
| await | pending `waiting for the note…` → `note landed` | elapsed since the action click; poke count while waiting |
| network | `518 requests · 16,004,112 bytes · log 3f9c…a71` | — |
| identity compare | `identity B produced the identical request log` | both hashes |
| error | `<code>: <message>` | retryable flag |

Never logged, and asserted by the same `capture-scan` binary run over a dump of
the demo's log state: the viewing key in any encoding, any channel key, any
cursor, any truncated form of the key. The address **is** logged — it is the
user's own public address, displayed in their own browser — but it is asserted
absent from every request record.

### 3.6 Measurements, and how each is obtained honestly

Every number carries a `provenance` chip. `measured` is black; `recorded` is
grey with a date and a source link; `derived` is grey-italic and reveals its
formula on hover. Nothing in region A or B may be `derived`.

| metric | how obtained | honesty notes |
|---|---|---|
| **cold total** | `performance.now()` bracketing the whole cold path, started only after `deleteDatabase` resolves and a fresh `Engine` exists; every fetch issued with `cache: 'no-store'` | The HTTP cache must be bypassed or the number is a warm number wearing a cold label. If `no-store` is unavailable, the column says so. |
| **cold breakdown** | `SyncTiming` from the client: `networkMs` summed over chokepoint durations, `decompressMs` around each `fzstd` call, `engineMs` summed over `sync_supply`, `idbWriteMs` around the transaction | Sub-timings may overlap wall time (fetch and inflate interleave); the panel labels the column *components, not a partition*, and shows their sum separately from `total` rather than pretending they add up. |
| **warm total** | reload with IDB intact, HTTP cache allowed, same bracket | The honest warm story is also a *request* story: the delta is `{genesis, manifest, head}` by design (§4.4 re-fetches genesis every session to catch a feed that changes its own genesis). Three requests, shown as three rows. |
| **cold vs warm together** | both are real runs in the same session: the page runs warm first (if state exists), then offers `run cold now`, which clears storage and re-runs. Both columns persist. | Never render one from a previous session's stored number. If only one has been run, the other column is empty, not estimated. |
| **requests / bytes** | the chokepoint's `RequestRecord`s; `bytesOnWire` is the received `ArrayBuffer.byteLength`, not `Content-Length` | An IDB hit is `source:'indexeddb'` and is **not** counted as a request. The panel shows both counts: `network 3 · cache 512`. |
| **snapshot lane vs epochs lane** | two real runs: `coldStart:'snapshot'` and `coldStart:'epochs'`, each from cleared storage, each with its own request count and bytes | This is the ~518-to-a-handful story, and it must be *run*, not projected. If the feed publishes no snapshot, the panel reads `snapshot lane unavailable on this feed` and shows nothing else. |
| **time-to-discover (t\_seen)** | from the arrival of the poke that carried the note's block (SSE event timestamp, or the poll tick that returned changed feed state) to the note appearing in `added` | This is the number our product controls, and it is the one the log line shows. |
| **time-to-discover (t\_chain)** | block timestamp of the note's block → local wall clock at discovery | Shown separately and labelled *includes indexer lag and network*; it is not our latency and must not be presented as it. |
| **discovery engine time** | `DiscoverTiming.engineMs`, measured in the worker around the wasm call | The module has no clock, so all timings are taken in TS at the call boundary: exact at the boundary, approximate inside. Stated in the panel's footnote. |
| **request-log hash** | `stats().requestLogSha256`, computed **inside the module** over its canonical request log | Computed by the key-blind component, not by the UI. |
| **recorded reference numbers** | the grey column: 5.97 s cold fold / 0.03 s warm resync (native, full mainnet, 515 epochs, 16 MB feed, 60 MB mirror, 31 MB peak RSS); 1.19 s to discover our own Sepolia note keylessly; 609 requests / 64,509 bytes byte-identical across two wallets on Sepolia; 518 requests / 16 MB on mainnet today | Each links to [`live-run-findings.md`](../live/live-run-findings.md) with its session number. The browser's cold number will differ from 5.97 s (that is native, and the epochs lane); the panel says so in one line rather than inviting the comparison silently. |

**One rule above the others:** if a measurement cannot be taken, the slot reads
`unavailable` and why. No placeholder, no last-known value, no projection in a
primary panel.

### 3.7 The network panel — turning an asserted claim into a visible one

The panel's job is that a sceptic with five seconds can see there is no key and
no address in anything we send.

1. **Header: the page's own CSP `connect-src`.** Rendered from
   `document.querySelector('meta[http-equiv=Content-Security-Policy]')`. The
   demo ships a CSP with `connect-src` limited to the feed origin (plus the
   anchor RPC origin when configured), `script-src 'self'`, no third-party
   fonts, no analytics, no `img-src` beyond `'self' data:`. The header is the
   claim; the list below is the evidence.
2. **Live request list.** One row per `RequestRecord`, in order: method, full
   URL, status, bytes, ms, and a source tag (`network` / `indexeddb`). The URL
   is rendered in full, never truncated in the middle — truncation is where a
   query string would hide.
3. **Totals.** `N requests · B bytes · log sha256 <hash>`.
4. **Second-identity toggle.** Runs the *same* cold feed sync a second time
   under identity B (a different address and a different key), from cleared
   storage, and displays the two `requestLogSha256` values one above the other
   with an `IDENTICAL` / `DIFFERENT` verdict. `DIFFERENT` renders red and loud;
   it is a bug report, not a UI state. Under §1.2 this comparison is a check on
   a theorem rather than a hopeful observation — which is why it can be trusted
   to be boring, and why it is worth showing.
5. **The leak scanner, with its self-test.** In-browser, over every URL, every
   request header name and value, and every request body in the session:
   search for the viewing key and the address in the same 13 encodings the Rust
   scanner uses (minimal hex, padded, decimal, upper/lower, `0x`-prefixed, raw
   BE/LE bytes, …). Displays `key: not found in 13 encodings`. A negative result
   from an unproven scanner is worth nothing, so a **self-test** button plants
   the key into a synthetic request record and shows the scanner catching it —
   the same discipline as leg **d**'s self-test, and the reason the negative
   result means something. The encodings list is imported from one shared
   fixture with the Rust scanner so the two cannot drift.
6. **A key-presence indicator.** `key in this tab: yes (worker, wasm memory) ·
   key in any request: no`. The first half comes from `status().unlocked`, the
   second from the scanner. Both are live.

### 3.8 The buttons, honestly

`deposit` / `send` / `withdraw` are **stage triggers, not transactions.** Each:

1. commits a log line naming what the user must do and where (LIVE), or what is
   being replayed (REPLAY);
2. opens the pending line `waiting for the note…` / `waiting for the spend…`;
3. drives the subscription (or the poll) until the corresponding change appears
   in `added` / `spent`;
4. commits with elapsed time and the note or nullifier.

In LIVE mode the button text is `deposit — do it in your wallet, we'll watch`.
It must not look like it will move funds. The demo never asks for a private key,
never builds a transaction, and never talks to a prover; the README and the page
both say so.

`withdraw` deserves one extra line of copy, because it is the case where our
value is clearest: the SDK cannot build a spend without knowing your notes, and
that is what we just supplied, keylessly.

**Subscription toggle.** ON: `EventSource /feed/live`, discovery runs
automatically on each poke (requires an `Unlocked` identity; without one the
toggle still runs feed sync and says so). OFF: the `check now` button runs
`sync()` + `getNotes()` once. Either way the log records how long it took and
how many requests it cost — which makes the SSE-vs-polling difference something
the viewer sees rather than reads.

If `/feed/live` 404s or the connection drops, the client degrades to polling
with a `status` event; the toggle renders `ON (polling — this feed publishes no
stream)`. A static-file mirror is a fully supported deployment (§2.5) and the
demo should demonstrate that rather than look broken on it.

### 3.9 A stage that demonstrates a privacy rule costing something

One extra control, off to the side: `fetch full history`. It switches the
client to `coldStart: 'epochs'`, clears storage, and re-runs — showing the
request count and byte count jump from the snapshot lane's handful to the full
history's hundreds.

The log line reads: `full history requires the whole feed — fetching only the
epochs containing your notes would make the request pattern a function of your
notes`. That is §1.6's rule, and this is the only way to show that we pay for it
rather than merely claiming we would. A demo that only shows the cheap paths
invites the question of what we are hiding; this answers it before it is asked.

### 3.10 Demo build and hosting posture

- No bundler-required build: ESM + `<script type="module">`, the package's own
  `dist/` and `engine_bg.wasm` served beside it.
- No third-party origins at all: fonts are system stacks, there is no analytics,
  no error reporting, no CDN. The CSP in §3.7 is enforceable precisely because
  there is nothing to exempt.
- Runs from `file://`-adjacent static hosting against a static feed directory,
  so REPLAY mode needs no server.
- The demo's own source is the shortest useful integration example, and the
  README points at it as such — for our real customer (§0.2), the SDK adapter
  path in `demo/src/sdk-variant.ts` is the second example, thirty lines long.

---

## 4. Departures from §A3/§A4 — consolidated

| § | Quoted | Replacement | Reason |
|---|---|---|---|
| 3.3 | `apply_snapshot` / `apply_epoch` / `apply_head` as three independent ABI methods, TypeScript orchestrating | `sync_begin` / `sync_supply` trampoline over the unmodified `apply_feed` (§1.2–1.3) | The orchestration *is* the trust logic and now exists in `crates/consumer`; §A3 would fork it into the least-tested layer. Also makes P-blind structural. |
| 3.3 | `check_manifest(manifest_json) -> "ok"\|"behind"\|"diverged"` | Deleted from the ABI; the three discriminants become `SyncResult.staleness`, derived from the one arbiter inside `apply_feed` | Two staleness arbiters drift. §3.7's "staleness is a return value, never a throw" is preserved. |
| 3.3 | *"the module exposes exactly two key-accepting entries"* | Four, named in a checked-in allowlist: `discover`, `history`, `export_reference_cursor`, `key_retain` | `key_retain` moves long-lived key residency from the JS heap into wasm memory where zeroize is real (§1.3). |
| 3.4 | *"TypeScript decompresses and is bound by R-I (verify the `zst` hash first…)"* | TypeScript supplies **both** compressed and inflated buffers; the module hashes both. TypeScript's only obligation is the output cap. | Removes the last verification obligation from the wrapper. |
| 3.2 / 0.4.1 | `MemStore` with `notes_get`/`notes_put` over a `NoteSet` value type | `MemStore` implements the trait that actually landed (row-level notes, cursors through `meta_set`), with a **closed exportable-meta allowlist** and a separate zeroizing `OwnerScope` (§1.4) | The shipped trait routes key-derived cursor JSON through the same API as `head_etag`; a prefix denylist would drift. Plus a requested 0a delta to give cursors their own methods (§1.6). |
| 3.5 | header fields | + `verified`; export by allowlist not iteration; `load` checks the *profile* | The integrity grade must survive a reload and must never be upgraded by memory. |
| 3.3 | `Engine::new(genesis_json)` | `Engine::new(profile_json)`; genesis is fetched and checked **inside** `apply_feed` against the profile | §6.2's stamping matrix requires the profile comparison; putting it in the wrapper duplicates it per host. |
| 4.2 | `getNotes(k)` / `subscribe(k, cb)` as the only ways to make progress | `sync()` takes no key; `subscribe(cb)` takes no key; retention only via `unlock()` returning an `Unlocked` handle (§2.4) | The old shape maximises key residency and hides the key-free phase; it also fights the wallet flow where the app boots locked. |
| 4.2 | `ClientStatus` | + `unlocked: Hex[]`, `ready`, `worker`; `verified` moves into `FeedState` | Key residency must be observable. |
| 4.1 | `DelegatedClient` exported from the main entry | Own subpath `strk20-discovery/delegated`, plus an insecure-transport gate | The key leaves the browser there; it should not be one autocomplete away. |
| 4.3 | data flow driven by TypeScript decisions | data flow driven by module Steps (§2.5) | Follows from §1.2. |
| — | no equivalent | `src/net.ts` single fetch chokepoint with a CI scan (§2.6) | TypeScript cannot express "this module does no IO" in its type system; a one-file scan is the checkable substitute. |
| — | no equivalent | `SessionStats` / `RequestRecord` / `SyncTiming` / `DiscoverTiming` as public API (§2.3) | The demo needs honest numbers; so does every integrator debugging a slow load. Making them public stops the demo from instrumenting around the package. |
| 4.5 | fold gate open on both lanes | L2 (epochs lane) verdict recorded as already reached by the native 5.97 s measurement; L1 verdict still owned by the browser bench | Measured, not argued. |

**Not departed from, and worth saying so:** §3.6's sealed-blob design in full,
including the corrected nonce doctrine (safety comes from the caller's fresh
entropy, not from the counter), the `prev_entropy_h` guard at exactly its real
strength, and `ENTROPY_INVALID` on any length but 32. §3.9's purity gates and
their honest restatement of what the import audit proves. §4.4's `keyId`
derivation and the persisted-and-refetched genesis. §4.5's Design R/M framing
and its honest IndexedDB trust statement. §4.8's `/health` chain check and its
refusal to construct. §1.5.1's three-word grade. §2.8.1's closed allowlist —
which this design strengthens into a Rust enum.

---

## 5. Acceptance legs

Amendments to existing legs:

- **q′ (was q, WASM conformance).** Keeps every existing assertion. Adds:
  (i) the **request-emitter proptest** — two distinct (address, key) pairs over
  the same fixture feed produce byte-identical `request_log()`, with
  `discover()` interleaved; (ii) `sync_supply` with a wrong `seq` raises
  `SYNC_PROTOCOL`; (iii) a `PARK_WITHOUT_REQUEST` negative — a test-only
  transport whose future pends without arming a request must panic, proving the
  tripwire is live; (iv) the scanner runs over `Engine.export()` **after** a
  `discover()` with a planted key, and over `request_log()`; (v) `key_retain` →
  `discover_retained` → `key_forget` leaves no key bytes in a wasm memory dump
  (search the linear memory for the key in all 13 encodings after
  `key_forget_all`); (vi) `meta_set` of an unlisted key with no scope open
  raises `SCOPE_VIOLATION`; (vii) the three `staleness` discriminants on the
  three constructed manifests, now as `SyncResult` values.
- **r (WASM reorg byte-identity).** Unchanged in substance; the fork is now
  replayed through `sync_begin`/`sync_supply`, and the leg additionally asserts
  the two request logs (pre- and post-fork sync) differ only by the head
  refetch, i.e. that a reorg changes no epoch request.
- **s (purity + size).** Adds the key-accepting-entry allowlist diff and the
  "no time source in the import allowlist" assertion.
- **u (npm keyless e2e).** Adds: the `net.ts` chokepoint scan (no `fetch` /
  `EventSource` / `XMLHttpRequest` identifier outside that file); a run that
  completes a full `sync()` **with no key present in the process**, asserted by
  the scanner over the whole capture; `stats().requestLogSha256` equal across
  two identities; `unlock()` → `status().unlocked` non-empty → `lock()` →
  empty, with the IDB dump scanned after each.

New legs:

- **α. Demo honesty.** Playwright over the built demo: (i) with storage
  deletion blocked, the cold column renders `unavailable` and **no number**;
  (ii) every rendered metric carries a provenance attribute, and no element in
  regions A or B carries `derived`; (iii) the recorded-reference column's values
  equal the values parsed from `live-run-findings.md` (one source, no
  transcription); (iv) the scanner self-test, driven from the page, reports a
  find.
- **β. Demo network panel completeness.** Every request the page issues appears
  in the panel: assert `panel.rowCount === capture.requestCount` against the
  proxy capture, so a request that bypassed the chokepoint fails the leg rather
  than merely going unrendered.
- **γ. Demo CSP.** The served page's CSP `connect-src` equals the configured
  feed origin (plus anchor RPC when set); a synthetic request to any other
  origin from page context is blocked, asserted by console error.

---

## 6. Open, and deliberately not decided here

1. **The L1 fold-gate number.** §4.6's browser bench decides Design R vs M for
   the snapshot lane. I have not pre-empted it; §2.7 records only what the
   native 5.97 s already settles (the epochs lane).
2. **Whether `discovery-core`'s engine ever pends over `MemView`.** §3.2's
   `now_or_never` tripwire assumes not, and the spike supports it, but the
   assumption is only proven when the full ABI runs the real engine over the
   real fixture. If it trips, the answer is a second parked driver, not
   `block_on`.
3. **Sealed-blob size at 31,077-note scale.** The anonymity set is 31,077 notes
   total; a heavy identity's `notes[]` is bounded by their own notes, not the
   set, so this is almost certainly a non-issue — but nobody has measured a
   sealed blob for a wallet with thousands of notes, and the IDB write is on the
   post-`getNotes` path (§4.4 quirk 5) for a reason.
4. **Whether `unlock()` should have a wasm-side deadline.** It cannot today
   (no clock, P-pure). The alternative — a caller-supplied monotonic tick — is
   an ABI parameter that buys a guarantee against a caller who is already
   trusted with the key. I lean no, and record it rather than settling it.
5. **REPLAY-mode feed provenance.** The pinned Sepolia capture must be
   reproducible from the public feed at a named manifest hash, or the demo is
   showing bytes nobody can re-derive. That capture, and the command that
   reproduces it, are a prerequisite for shipping the demo, not a detail.
