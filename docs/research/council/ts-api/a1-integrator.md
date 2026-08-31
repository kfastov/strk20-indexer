# A1 — the integrator's design: wasm ABI, npm API, demo

Author role: the engineer who has to drop this into a real wallet next quarter.
Written 2026-08-31 against `proto/keyless-indexer` at `b6d2faf`, the in-flight
`crates/consumer` refactor (step 0a), `docs/spec/consumer-path.md` §A3/§A4 and
§12, `docs/roadmap.md`, `docs/notes/2026-08-30-consumer-path-discussion.md`, and
the measured numbers in `docs/research/live/live-run-findings.md`.

This document revises §A3 and §A4. Everywhere it departs from them it quotes the
existing text and gives the replacement with a reason. Everything not quoted and
replaced stands as written.

---

## 0. The three facts that reshape the design

**0.1 §A3/§A4 were designed against a spike; `crates/consumer` now exists.**
The real seam is `ConsumerStore` in `crates/consumer/src/store.rs` — 17 methods
plus an associated `View` type — and the real fold is
`strk20_consumer::apply::apply_feed`, an `async fn` that *pulls* through
`FeedTransport`. §A3's ABI assumed TypeScript would push artifacts into the
module one at a time (`apply_epoch(payload, manifest_entry)`), which requires
re-implementing, in the wasm facade, the ordering, divergence, masked-reorg and
snapshot-fallback logic `apply_feed` already owns. That is two implementations of
the trust pipeline, and the second one is the one nobody fuzzes. §1 below
replaces the push ABI with a **need/provide/advance loop over the real
`apply_feed`**, so the browser runs the same bytes of Rust as the CLI.

A note on the brief's "11 methods": the list handed to me (`apply_feed`, `view`,
`meta_get`, `meta_set`, `notes`, `upsert_note`, `refresh_spent`,
`prune_missing_notes`, `delete_owner_notes`, `reset_mirror`, `tail_generation`)
is not the `ConsumerStore` trait — it is the set of `FeedStore` methods
`crates/client/src/sync.rs` calls today. Three of those eleven (`apply_feed`,
`refresh_spent`, `prune_missing_notes`) are **not on the trait** and must not be:
they are algorithms, not storage. §1.2 says where they go, and why getting this
wrong would fork the one piece of semantics the live run proved subtle.

**0.2 Our customer is a wallet, not a dapp.** The Wallet API documentation is
explicit — the wallet holds the viewing key, the wallet discovers notes, the
wallet builds the proof. A dapp on that route never sees a viewing key and
therefore has nothing to hand us. **The npm package's customer is the wallet
itself, or a key-holding backend/CLI/agent.** Consequences carried through this
document:

- the primary integration surface is not `getNotes` — it is
  `strk20-discovery/sdk`'s `DiscoveryProvider`, the socket the Starknet Privacy
  SDK already has for exactly this;
- the API must assume the key is *locked most of the time* and lives somewhere
  the client does not own (§2.2's `Account.viewingKey()`);
- the demo cannot "connect a wallet and discover notes" — that combination does
  not exist. The demo holds its own key and says so in a banner (§3.3).

**0.3 The measured numbers.** Only these, and no others, appear in any UI, doc
or README produced by this design:

| fact | value | source |
|---|---|---|
| cold fold, full mainnet history, native | 5.97 s (515 epochs, 16 MB feed, 60 MB mirror, 31 MB peak RSS) | live-run §3 |
| warm re-sync, native | 0.03 s | live-run §3 |
| mainnet volume | 118,960 events / 28,383 pool-active blocks | live-run §2 |
| anonymity set | 31,077 notes | live-run §3 |
| our own Sepolia note, discovered keylessly | 1.19 s | live-run §5 |
| two identities, identical request streams | 609 requests / 64,509 bytes (Sepolia); 518 requests / 16 MB (mainnet, pre-snapshot) | live-run §5, §2 |

Every one of them is a **native** measurement. No browser number exists yet. The
package ships no performance claim until §2.9's gate produces one, and the demo
measures live rather than quoting.

---

## 1. Revised §A3 — the wasm ABI

### 1.1 Crate shape (amends §3.1)

§3.1 stands as written for the crate list and the pinned features. Two additions
it could not have known:

```
crates/consumer     strk20-consumer  — Block B core (EXISTS, step 0a in flight)
crates/client-wasm  strk20-engine    — cdylib, wasm-bindgen facade + MemStore
                                       + StagedTransport
```

`crates/client-wasm` gains no dependency beyond §3.1's list. `StagedTransport`
is ~80 lines of `HashMap<ArtifactKey, Vec<u8>>` and needs nothing.

`MemStore` must use `std::sync::Mutex` for interior mutability, not `RefCell`:
`ConsumerStore` methods take `&self` even for writes (SQLite hides a `Mutex`
already), and the trait requires `Send + Sync`. `std::sync::Mutex` compiles and
works on `wasm32-unknown-unknown`; `RefCell` does not satisfy `Sync`. This is a
compile error waiting at the start of step 3 and is cheaper to know now.

### 1.2 What must move into `strk20-consumer` before the ABI can exist

This is the load-bearing finding for the in-flight refactor. Three things
`crates/client/src/sync.rs` calls are **inherent `FeedStore` methods implemented
in SQL** and are absent from `ConsumerStore`:

| today | where it must live | why not the trait |
|---|---|---|
| `FeedStore::apply_feed` | already moved — `strk20_consumer::apply::apply_feed` | it is the algorithm |
| `FeedStore::refresh_spent(owner, block)` (`store.rs:960`) | `strk20_consumer::notes::refresh_spent<S: ConsumerStore>` | it is `notes()` + `read_slot_as_of(nullifiers(n.nullifier))` + `set_note_spent`, all trait primitives |
| `FeedStore::prune_missing_notes(owner, as_of)` (`store.rs:900`) | `strk20_consumer::notes::prune_missing_notes<S: ConsumerStore>` | same: `notes()` + `read_slot_as_of(notes(n.note_id))` + `delete_note` |

Likewise `sync_once`, `run_incoming`, `run_outgoing`, `reopen_cursor`,
`register_notes` and `full_resync` must become generic over `ConsumerStore`
(`strk20_consumer::sync`), with `SyncReport` moving with them per §0.4.1.

Why this is not a tidiness point. Live-run §7 recorded the semantics these two
functions encode: *a spent note's storage slot is not cleared — `get_note` still
returns its packed value after the spend; spentness lives only in `nullifiers` /
`NoteUsed`.* If the browser host re-implements spent-state, that is a second
place for the rule to be got wrong, in the layer with the worst test coverage,
and the failure mode is a wallet showing spent money as spendable. The
conformance leg §0.4.1 already mandates (`FeedStore` vs `MemStore` produce the
same `NoteSet` and the same diff) only proves anything if both hosts run the
*same* `refresh_spent`.

**§0.4.1's `NoteSet` value type is not what got built.** The spec proposed
`notes_get`/`notes_put` over an owner-scoped `NoteSet`; the shipped trait has
row-level `notes` / `upsert_note` / `set_note_spent` / `delete_note` /
`delete_owner_notes`. Row-level is the better fit for SQLite and is fine for
`MemStore` too. Keep it, and delete §0.4.1's `NoteSet` paragraph; the diff it
existed to produce (`DiscoverOut.added`/`spent`) is computed in the facade from
the pre-image the sealed blob supplied (§1.6), which is where it always belonged.

### 1.3 The execution model, restated (amends §3.2)

§3.2 says:

> **Execution model.** `discovery-core`'s entry points are `async`, but over
> `MemView` no future ever suspends. They are driven by `fn drive<F: Future>(f: F)
> -> F::Output { f.now_or_never().expect(...) }`

That stands, and now covers more than the engine: `apply_feed` is `async`
because `FeedTransport` is. In the browser, **every artifact `apply_feed` asks
for is already in memory**, so the transport futures are `Ready` by construction
and `drive()` carries the whole fold. The tripwire panic ("engine future pended
over an in-memory view") gains a second message for the transport case
("transport future pended over a staged cassette") — either is a programming
error, never a runtime path. No `wasm-bindgen-futures`, no async JS, invariant
intact.

§3.2's `MemStore` struct is **replaced**. It was written before the trait
existed and models `base`/`tail` compartments the trait does not have (the trait
expresses tail replacement through `replace_range(Range::Above{floor}, …)`).
Replacement:

```rust
pub struct MemStore { inner: Mutex<Inner> }

struct Inner {
    // ---- feed compartment: key-independent, and the ONLY thing export() sees
    meta:   BTreeMap<String, String>,      // pool, chain_id, epoch_size, genesis_block,
                                           // last_epoch_applied, last_epoch_hash,
                                           // last_epoch_to, head_number, head_hash,
                                           // head_etag, l1_accepted, history_floor,
                                           // snapshot_basis, snapshot_pending_grounding
    slots:  BTreeMap<(Felt /*slot*/, u64 /*block*/), Felt>,   // the storage_log
    blocks: BTreeMap<u64, BlockRec>,       // hash, parent, timestamp, l1_final
    events: BTreeMap<(u64, u32), EventRec>,
    tail_generation: u64,

    // ---- scratch compartment: key-DERIVED, alive only inside discover()/history()
    scratch_meta:  BTreeMap<String, String>,   // cur_*, ckpt_*, ckpt_at_*, gen_*
    scratch_notes: BTreeMap<Felt /*note_id*/, NoteRow>,
    scratch_live:  bool,
}
```

`meta_get`/`meta_set` route to `scratch_meta` for any key matching
`cur_|ckpt_|ckpt_at_|gen_`, and to `meta` otherwise; the note-registry methods
always route to `scratch_notes`. `discover()` opens the scratch compartment from
the sealed blob, runs, re-seals, and **zeroizes and clears scratch before
returning**. `export()` serializes the feed compartment only, and asserts
`scratch_meta.is_empty() && scratch_notes.is_empty()` — so §3.5's "only
epoch-derived state is ever exported" and §3.6's "nothing key-derived ever enters
`MemStore` beyond the lifetime of the call" are enforced by construction rather
than by review. Leg **r** (the blob is byte-identical across a tail fork) keeps
its grammar-level guard from §3.5 unchanged.

### 1.4 The fetch loop — `need` / `provide` / `advance` (replaces §3.3's push ABI)

§3.3 says:

> ```rust
> /// UNCOMPRESSED snapshot payload + its manifest "snapshot" object + the anchor
> /// sidecar JSON. Runs §1.5 rings 2–5 inside the module. […]
> pub fn apply_snapshot(&mut self, payload: &[u8], manifest_snapshot_json: &str,
>                       anchor_json: &str) -> Result<String, JsError>;
> /// UNCOMPRESSED epoch payload + its manifest "epochs[i]" object. […] Must be
> /// last_epoch + 1 (FEED_EPOCH_GAP otherwise).
> pub fn apply_epoch(&mut self, payload: &[u8], manifest_entry_json: &str)
>     -> Result<String, JsError>;
> /// head.ndjson bytes. {"head","l1_accepted","tail_rewound"}
> pub fn apply_head(&mut self, payload: &[u8]) -> Result<String, JsError>;
> ```

**Replaced by:**

```rust
/// What the engine wants fetched next. Emitted BY THE MODULE, from a manifest
/// the module verified. TypeScript never parses a feed artifact.
/// [{"kind":"epoch","idx":1406,"path":"epochs/1406.strk20e.zst",
///   "zst_sha256":"<64hex>","cap":67108864,"compressed":true,"optional":false}, …]
pub fn need(&self) -> String;

/// Stage one fetched artifact. `bytes` is the UNCOMPRESSED payload (TypeScript
/// ran fzstd; §3.4 unchanged). The engine verifies the payload sha256 against
/// the manifest before staging; a mismatch throws FEED_HASH_MISMATCH and stages
/// nothing.
pub fn provide(&mut self, item_json: &str, bytes: &[u8]) -> Result<(), JsError>;

/// Run `strk20_consumer::apply::apply_feed` over the staged cassette until it
/// completes or asks for an artifact that is not staged.
///   {"status":"done","outcome":{…ApplyOutcome…},"state_changed":true,"stats":{…}}
///   {"status":"need","need":[…],"progress":{"epochs_applied":12,"epochs_total":515}}
/// A "need" return is NOT an error: everything applied before the miss is
/// committed, and the next call resumes from persisted meta.
pub fn advance(&mut self) -> Result<String, JsError>;

/// sha256 helper so TypeScript can honour R-I (check the .zst hash BEFORE
/// inflating, cap the output) without shipping a second hash implementation.
#[wasm_bindgen]
pub fn sha256_hex(bytes: &[u8]) -> String;
```

**Why.** Five reasons, in the order I would defend them in review.

1. *One implementation of the trust pipeline.* `apply_feed` already does epoch
   ordering, the manifest-divergence check ("feed diverged: epoch N hash X !=
   locally applied Y"), the masked-reorg supersede via `block_hashes` +
   `replace_range(…, bump_generation)`, the `tail_from > last_epoch_to + 1`
   mid-sync bail, the snapshot ladder, and the `SnapshotRejected` → `auto`
   fallback with `reset_mirror`. §3.3's `apply_epoch` would need every one of
   those re-expressed as a state machine driven from TypeScript.
2. *The `auto` fallback is not expressible in the push ABI.* When a snapshot
   fails to verify, `apply_feed` resets the mirror and replays from epoch 0 —
   which changes what must be fetched. A TypeScript driver that decided the
   artifact list up front cannot do that without duplicating the decision.
3. *TypeScript stops parsing feed artifacts entirely.* The URL list, the byte
   caps and the expected hashes all come out of `need()`. The wrapper's whole
   job becomes "GET the paths a keyless module named, inflate, hand back". This
   is what makes the privacy claim structural rather than tested: the module
   that authors the URLs has no key on that code path, and `need()` takes no
   arguments at all.
4. *Memory stays bounded.* `provide` → `advance` → cassette cleared. Under
   §3.3's ABI, the natural TypeScript implementation fetches epochs in parallel
   and holds them; here the wrapper's batch size is a knob (`fetchConcurrency`,
   default 6) and the module rejects overflow with `CASSETTE_FULL {staged, cap}`
   (default cap 96 MB, `opts.cassetteCap`).
5. *Resumability is free.* A dropped connection mid-cold-start leaves a
   consistent mirror at whatever epoch it reached; the next `advance()` needs
   the next epoch. This is the same property the native client already has
   (`last_epoch_applied` in meta) rather than a new one.

**The advisory-plan rule.** `need()` may over- or under-predict (it derives the
epoch list from the verified manifest and `last_epoch_applied`). Nothing is ever
applied *because it was planned*: `provide` verifies the payload hash, and
`apply_feed` verifies the chain linkage and the chain/pool binding when it
consumes it. An artifact staged but never asked for is dropped by the cassette
clear. A hostile `need()` list is therefore a wasted GET and nothing more.

**Loop, in full, as TypeScript will write it:**

```ts
let r = JSON.parse(engine.advance());          // first call needs the manifest
while (r.status === 'need') {
  await Promise.all(r.need.map(async (item) => {
    const z = await get(item.path);            // one parameterless GET
    if (item.compressed) {
      if (sha256_hex(z) !== item.zst_sha256) throw feedHashMismatch(item);
      const raw = fzstd.decompress(z, item.cap);   // R-I: hash first, cap output
      engine.provide(JSON.stringify(item), raw);
    } else {
      engine.provide(JSON.stringify(item), z);
    }
  }));
  r = JSON.parse(engine.advance());
}
```

### 1.5 The rest of the exported ABI

```rust
#[wasm_bindgen]
impl Engine {
    /// genesis.json BYTES (not a &str — it is byte-compared against the stored
    /// copy per §4.4 and a string round-trip is a chance to normalise it).
    #[wasm_bindgen(constructor)]
    pub fn new(genesis_json: &[u8], opts_json: &str) -> Result<Engine, JsError>;

    /// Restore from a persisted state blob (§3.5). Verifies trailer + stamp
    /// against `genesis_json`. Never partially loads.
    pub fn load(blob: &[u8], genesis_json: &[u8], opts_json: &str) -> Result<Engine, JsError>;

    /// {"chain_id","pool","genesis_block","epoch_size","last_epoch",
    ///  "last_epoch_hash","last_epoch_to","history_floor","snapshot_basis",
    ///  "head","l1_accepted","verified","slots","blocks","events",
    ///  "engine_version"}
    pub fn info(&self) -> String;

    /// Staleness arbitration against a freshly fetched manifest, ALL of it in
    /// Rust. "ok" | "behind" | "diverged". Never throws for staleness (§3.7).
    /// Kept as a standalone method — the warm path answers "is there anything
    /// to do?" in one call, before any cassette exists, and that is the
    /// measurement §3.2 of the demo needs.
    pub fn check_manifest(&self, manifest_json: &[u8]) -> Result<String, JsError>;

    /// Epoch-derived state ONLY (§3.5). Call only when advance() reported
    /// state_changed. Asserts the scratch compartment is empty.
    pub fn export(&self) -> Vec<u8>;

    /// THE ONLY key-accepting entries: this, `history`, `export_reference_cursor`.
    /// One full pass for one owner — checkpoint pass at last_epoch_to, live pass
    /// at head, spent refresh — i.e. `strk20_consumer::sync::sync_once` over
    /// MemStore. `key` is zeroized in place before return. `entropy32` MUST be
    /// 32 fresh bytes from crypto.getRandomValues on EVERY call (§3.6).
    pub fn discover(&mut self, owner_hex: &str, key: &mut [u8],
                    sealed: Option<Vec<u8>>, entropy32: &[u8])
        -> Result<DiscoverOut, JsError>;

    /// Paged tx history per §1.1's paging contract, unchanged from §3.3.
    pub fn history(&self, owner_hex: &str, key: &mut [u8], sealed: Option<Vec<u8>>,
                   from_block: u64, limit: u32) -> Result<String, JsError>;

    /// Reference-schema DiscoveryCursor JSON extracted from a sealed blob —
    /// Tier-0 migration to compat/SDK without resync. Unchanged from §3.3.
    pub fn export_reference_cursor(&self, key: &mut [u8], sealed: &[u8])
        -> Result<String, JsError>;

    // ------------------------------------------------- §1.5 ring 6, in the browser

    /// What to ask the USER'S OWN RPC for, to ground this mirror in the chain.
    /// {"block":14151973,"method":"starknet_getStorageProof",
    ///  "params":[{"block_number":14151973},[],["0x…pool…"],[]],
    ///  "also":[{"method":"starknet_getBlockWithTxHashes","params":[{"block_number":…}]}]}
    /// Address-blind by construction: a public pool and a public block. The
    /// params are `[]`, never `null` — LIVE-7.
    pub fn anchor_request(&self, block: u64) -> Result<String, JsError>;

    /// Fold this mirror's full slot set at `block` into a Pedersen MPT root and
    /// compare it with the proof, after binding the proof to the chain by
    /// `global_roots.block_hash` == the header's hash (§12 point 3).
    /// {"outcome":"MATCH"|"MISMATCH"|"UNAVAILABLE","block":…,"local":"0x…",
    ///  "chain":"0x…","verified":"anchored"}
    /// UNAVAILABLE on JSON-RPC error 42 — a statement about the endpoint, never
    /// about the mirror (§11.4, retained by §12).
    pub fn verify_anchor(&mut self, block: u64, proof_json: &str,
                         header_json: &str) -> Result<String, JsError>;
}

#[wasm_bindgen(getter_with_clone)]
pub struct DiscoverOut {
    pub report_json: String,   // strk20_consumer::report::SyncReport, field-identical
                               // to `strk20-sync sync --json`
    pub sealed: Vec<u8>,       // checkpoint-only sealed blob; hand back next time
    pub added_json: String,    // notes absent from the supplied sealed blob
    pub spent_json: String,    // nullifiers that flipped to spent this pass
    pub stats_json: String,    // {"slots_read":…,"events_scanned":…,"passes_in":…,
                               //  "passes_out":…,"cursor_reset":false}
}
```

**Two additions to §3.3, both earned.**

*`anchor_request` / `verify_anchor`.* §3.3 has no ring-6 path, so a browser
client can only ever reach `verified: "server-asserted"` — while the native CLI
reaches `"anchored"`. That is backwards: the browser is the host with a user who
has an RPC URL in their wallet already. The spike confirmed `strk20-feed` builds
for wasm32 with `mpt`, and live-run §8 confirmed the client-side proof check
works against a public Sepolia RPC with the indexer out of the trust path. Cost:
two exported methods and one extra fetch, to the user's own endpoint, carrying a
public pool address and a public block number. This is also the demo's strongest
five-second claim (§3.6).

*`stats_json`.* Every honest UI needs to say what the machine did, and every
integrator debugging a slow first sync needs it. It is derived from counters the
passes already keep; it contains no key-derived value (counts only) and is
asserted key-clean by the leg-**q** scanner like every other string.

**Deliberately NOT added: timing.** No method returns a duration. `Date.now()`
inside wasm requires a JS import the §3.9 import-allowlist audit exists to keep
out. Timing is TypeScript's, measured around the call — which is also the only
place it can honestly include zstd and fetch.

### 1.6 Sealed per-key state (amends §3.6)

The construction (`S20SEAL1` ‖ nonce(24) ‖ XChaCha20-Poly1305, HKDF key/nonce
derivation, AAD, `entropy32` mandatory, the `prev_entropy_h` constant-entropy
guard stated at exactly its real strength) is **unchanged**. §3.6's plaintext is
amended in three places.

§3.6 says:

> ```
> plaintext (canonical JSON):
> {"v":1,"counter":<u64>,"prev_entropy_h":"<64-hex …>",
>  "ckpt_at":<block ≤ last_epoch_to>,
>  "in_ckpt":<reference DiscoveryCursor JSON>,"out_ckpt":<reference DiscoveryCursor JSON>,
>  "notes":[{"note_id","owner","sender","token","index","nullifier","amount","block","spent"},…]}
> ```

**Replaced by:**

```
{"v":1,"counter":<u64>,"prev_entropy_h":"<64-hex>",
 "ckpt_at":<block ≤ last_epoch_to>,
 "ckpt_epoch":<epoch index containing ckpt_at>,
 "ckpt_epoch_hash":"<64-hex payload sha256 of that epoch>",
 "in_ckpt":<reference DiscoveryCursor JSON>,"out_ckpt":<reference DiscoveryCursor JSON>,
 "notes":[{note fields…},…]}          # ONLY notes with block <= ckpt_at
```

1. **`notes[]` is checkpoint-only, like everything else in the blob.** §3.6
   declared the blob checkpoint-only ("no live cursors, no generation counter,
   nothing bound to the tail") and then sealed a `notes[]` with a `block` field
   and no bound. A note discovered by the live pass sits above `last_epoch_to`
   and can be reorged away; sealing it is exactly the persisted-reorg state the
   whole browser design exists to not have. Rule: **seal notes with
   `block <= ckpt_at`; return live notes to the caller but never seal them.**
   The live pass rediscovers them from the refetched tail on the next session,
   which costs a walk over ≤ one epoch of blocks.

   This is what lets §3.6's "no persisted reorg logic at all" be true rather than
   nearly true, and it deletes the browser's need for `tail_generation`,
   `gen_<owner>` and `prune_missing_notes` entirely. `MemStore` still implements
   `tail_generation()` (the trait requires it, and `apply_feed` bumps it) — it is
   simply never persisted and never compared, because a fresh `MemStore` starts
   at generation 0 with no owner generation to disagree with it.

2. **`ckpt_epoch` + `ckpt_epoch_hash` make the seal invalidatable.** A cursor is
   a position in a history. If the feed diverged (`check_manifest` →
   `"diverged"`), the epoch containing `ckpt_at` may now hash differently, and
   resuming from the old cursor resumes over a history that no longer exists.
   On open, the module compares `ckpt_epoch_hash` against the verified manifest's
   entry for `ckpt_epoch`; a mismatch is treated exactly like an AEAD failure —
   **no cursor, fresh discovery, `stats.cursor_reset = true`** — never an
   exception. Without this the seal is a cache with no invalidation rule, which
   is the one kind of cache that is worse than none.

3. **`counter` keeps its §3.6 meaning** (a rollback/authenticity signal inside
   the AEAD, and nothing more). No text anywhere may re-attach nonce safety to
   it.

### 1.7 Error model (amends §3.7)

§3.7's table stands, including the deletion of `STATE_STALE`. Additions:

| code | details | retryable | raised by |
|---|---|---|---|
| `CASSETTE_FULL` | `{staged, cap}` | no | `provide` |
| `CASSETTE_UNEXPECTED` | `{kind, idx}` | no | `provide` (an item `need()` never asked for and the manifest does not list) |
| `ANCHOR_UNBOUND` | `{block, proof_block_hash, header_hash}` | no | `verify_anchor`, §12 point 3 |
| `KEY_UNAVAILABLE` | `{reason}` | yes | npm only — `Account.viewingKey()` rejected (locked wallet) |
| `ABORTED` | — | no | npm only — `AbortSignal` |

`SNAPSHOT_ROOT_MISMATCH` gains `UNAVAILABLE` as a non-error sibling: it is a
`verify_anchor` *return value*, never a throw, for the same reason
`check_manifest` returns a discriminant — LIVE-6 says a capability gap must
never read as corruption, and a throw and a discriminant are different control
flow in TypeScript.

### 1.8 §3.8 and §3.9 stand

The fork/patch discipline (§3.8) and the purity/size gates (§3.9) are unchanged,
including the FILL-IN for the size budget and the honest restatement of what the
import audit proves. Two notes:

- the feature-resolved dependency walk now has a real target to run against:
  `crates/consumer/Cargo.toml` already sets
  `discovery-core = { default-features = false }`, and the workspace comment at
  `Cargo.toml:57` records the measured effect (142 → 118 crates on wasm32).
  Wire that number into the CI diff rather than leaving it in a comment.
- `#![deny(unsafe_code)]` on `client-wasm` with exactly one documented
  `#[allow]` on the facade module, per §3.9's correction. `StagedTransport` adds
  no `unsafe`.

---

## 2. Revised §A4 — the npm package

### 2.1 Positioning, and what it changes (amends §4.1)

§4.1 says:

> The SDK adapter ships **inside** the same package (`/sdk`): one install, both
> audiences.

"Both audiences" was keyless-browser-app and SDK-user. Given §0.2 the audiences
are: **(a) a wallet or key-holding app driving the Starknet Privacy SDK, and (b)
a key-holding backend/CLI/agent that wants notes without an SDK.** A Wallet-API
dapp is not an audience at all and must be told so in the first paragraph of the
README, because the alternative is an integrator spending a day discovering it:

> **Who this is for.** `strk20-discovery` needs a viewing key. If your app talks
> to the user's wallet through the Starknet Wallet API, you never receive one —
> the wallet holds the key, discovers the notes and builds the proof, and you do
> not need this package. This is for the wallet itself, and for anything else
> that holds a key: a key-holding backend, a CLI, an agent, a self-custody app.

Consequence for layout: `strk20-discovery/sdk` is promoted from adapter to
**primary surface**, documented first, with `KeylessClient` presented as the
lower layer underneath it. Everything else in §4.1 stands — unscoped name, ESM +
`.d.ts`, no bundler, no install scripts, provenance publishing, `files`
whitelist, wasm sha256 in the README and asserted in CI, `fzstd` the single
pinned runtime dependency, and no size number quoted before §3.9 measures one.

### 2.2 The key never lives in our object (replaces §4.2's `KeyRef`)

§4.2 says:

> ```ts
> export interface KeyRef { address: `0x${string}`; viewingKey: Uint8Array; }
> …
> subscribe(k: KeyRef, cb: (ev: DiscoveryEvent) => void): () => void;
> ```

`getNotes(k)` with a `Uint8Array` is defensible — one call, one copy, zeroized
on return. `subscribe(k, cb)` is not: it forces the integrator to hand a
long-lived key to a long-lived object, and our object then holds it across an
unbounded number of passes, across a locked wallet, across a backgrounded tab.
For our actual customer — a wallet with a lock screen — that is the wrong shape
and it is the shape they will have to work around.

**Replaced by:**

```ts
/** An owner the client can discover for. The client NEVER stores the key: it
 *  calls `viewingKey()` at the start of every pass and zeroizes the bytes it
 *  was given before the pass returns. A locked wallet rejects, and the client
 *  reports `{type:'status', state:'locked'}` rather than failing the session. */
export interface Account {
  readonly address: `0x${string}`;
  /** 32-byte big-endian viewing key. Return a FRESH array each call — the
   *  client zeroizes it. Reject to decline (locked, user denied, revoked). */
  viewingKey(): Promise<Uint8Array>;
}

/** For a backend/CLI that legitimately holds the bytes for the process
 *  lifetime. Named so the shape is visible in review. */
export function staticAccount(address: `0x${string}`, key: Uint8Array): Account;
```

One shape everywhere: `getNotes(account)`, `watch(account, cb)`,
`history(account, …)`, `provider(account)`. Reasons, in order:

1. it makes "the client does not retain your key" a **type-level** statement
   rather than a README sentence;
2. the locked-wallet case becomes a first-class status instead of an integrator
   workaround;
3. a wallet with N accounts registers N `Account`s over one client and one
   mirror, which is the multi-account story §4.2 never told (§2.4);
4. `staticAccount` is a grep target in a wallet's own review.

§4.2's justification for `Uint8Array`-only and for bundling the address stands
verbatim and applies to the returned array.

### 2.3 The interface

```ts
export type Phase =
  | 'idle' | 'manifest' | 'snapshot' | 'epochs' | 'head'
  | 'anchor' | 'persist' | 'discover';

export interface Progress {
  phase: Phase;
  done: number; total: number;        // epochs applied / total, or 0/0
  bytes: number; requests: number;    // cumulative, this operation
  elapsedMs: number;
}

export interface Note {
  token: string; index: number; noteId: string; nullifier: string;
  amount: bigint; blockNumber: number; blockTimestamp: number;   // + (new)
  sender: string; spent: boolean;
}

export interface FeedState {
  head: number; l1Accepted: number; lastEpoch: number; lastEpochTo: number;
  historyFrom: number; snapshotBasis: number | null; snapshotRejected: boolean;
  verified: 'anchored' | 'server-asserted' | 'replayed';
  changed: boolean;                   // any epoch/snapshot/tail applied
  cold: boolean;                      // this call built the mirror from nothing
  elapsedMs: number; computeMs: number; bytes: number; requests: number;
}

export interface NotesResult {
  notes: Note[]; balances: Map<string, bigint>;
  added: Note[]; spent: Note[];
  feed: FeedState;
  complete: boolean; historyFrom: number; cursorReset: boolean;
  elapsedMs: number;                  // discovery only, excludes the feed pass
  raw: unknown;                       // the untouched SyncReport (oracle equality)
}

export type DiscoveryEvent =
  | { type: 'progress'; progress: Progress }
  | { type: 'feed';     feed: FeedState }
  | { type: 'notes';    added: Note[]; spent: Note[];
                        balances: Map<string, bigint>; head: number; elapsedMs: number }
  | { type: 'reorg';    rewoundTo: number }
  | { type: 'status';   state: 'live' | 'polling' | 'degraded' | 'locked' | 'idle' }
  | { type: 'error';    error: Strk20Error; recovering: boolean };

export interface Subscription { close(): void; readonly closed: boolean; }

export interface DiscoveryClient {
  /** Bring the local mirror to the feed's head. Takes NO key and emits no
   *  key-derived value: a wallet can keep the mirror warm while locked. */
  sync(opts?: { signal?: AbortSignal; onProgress?: (p: Progress) => void })
      : Promise<FeedState>;

  getNotes(a: Account, opts?: {
    signal?: AbortSignal;
    onProgress?: (p: Progress) => void;
    refresh?: 'auto' | 'force' | 'none';   // 'none' = discover over the mirror as it is
  }): Promise<NotesResult>;

  watch(a: Account, cb: (ev: DiscoveryEvent) => void): Subscription;

  history(a: Account, opts?: { fromBlock?: number; limit?: number; signal?: AbortSignal })
    : Promise<{ transactions: HistoryTx[]; complete: boolean;
                completeFrom: number; registrationAvailable: boolean }>;

  /** The SDK socket (§2.7). This is the primary integration for a wallet. */
  provider(a: Account): DiscoveryProvider;

  status(): ClientStatus;
  close(): Promise<void>;
}

export interface ClientStatus {
  mode: 'keyless' | 'delegated';
  transport: 'sse' | 'polling';
  persistence: 'indexeddb' | 'memory';
  persistMode: 'raw' | 'folded';
  head: number; l1Accepted: number; lastEpoch: number; historyFrom: number;
  verified: 'anchored' | 'server-asserted' | 'replayed';
  accounts: number;                  // how many are being watched
  network: { requests: number; bytes: number };   // since construction
}
```

Constructor:

```ts
export class KeylessClient implements DiscoveryClient {
  constructor(opts: {
    feedUrl: string;
    network?: 'mainnet' | 'sepolia' | ChainProfile;   // default 'mainnet' (§A6, C18)
    coldStart?: 'auto' | 'snapshot' | 'epochs';       // default 'auto' (one vocabulary)
    persistence?: 'indexeddb' | 'memory' | StorageAdapter;   // default 'indexeddb'
    persist?: 'raw' | 'folded';                       // narrowed at publish (§4.5)
    live?: boolean;                                   // default true
    pollIntervalMs?: number;                          // default 30_000
    worker?: boolean;                                 // default true (C14)
    fetchConcurrency?: number;                        // default 6
    cassetteCap?: number;                             // default 96 * 2**20
    anchorRpcUrl?: string;                            // enables ring 6 (§1.5 above)
    anchorPolicy?: 'off' | 'best-effort' | 'require'; // default 'best-effort'
    requestPersistentStorage?: boolean;
    wasmUrl?: string | URL;
    fetch?: typeof fetch;
    onRequest?: (r: RequestRecord) => void;           // §2.5
  });
}
```

**Additions to §4.2, each with its reason:**

- **`sync()`** — separates "keep the mirror current" from "tell me my notes".
  A wallet syncs on a schedule while locked; the demo times a cold load before a
  key exists. It also makes the central privacy claim demonstrable: the
  expensive part of this system runs with no key in the process.
- **`signal`** — a wallet UI closes the account screen mid-cold-start. Without
  cancellation the integrator's only recourse is to abandon a running worker.
  Abort is checked between `advance()` calls and between discovery passes;
  partial application is retained (see resumability, §1.4).
- **`onProgress` / `progress` events** — the cold path is seconds of work. §4.2
  gave no way to draw a progress bar, so every integrator would either build a
  fake one or block their UI. Phases are exactly the wrapper's own loop
  boundaries, so nothing is invented to report them.
- **`refresh: 'none'`** — a wallet that just synced wants notes for account #7
  without another manifest round trip. Without it, N accounts cost N feed
  passes.
- **`watch` replacing `subscribe`** — returns a `Subscription` object rather
  than a bare unsubscribe closure, so `closed` is inspectable and the shape
  matches what an integrator stores on a component instance.
- **`Note.blockTimestamp`** — already in `BlockLine` (`codec.rs:37`) and needed
  by any UI that shows "3 minutes ago" without a second RPC. Also the honest
  denominator for the demo's second latency clock (§3.5).
- **`anchorPolicy`** — §4.2 had `anchorRpcUrl` with §7.1's "configured means
  mandatory" semantics, which LIVE-6 makes wrong for a browser: a user's RPC
  that does not implement `getStorageProof` would fail every sync. Three-valued:
  `off` never asks; `best-effort` asks and downgrades `verified` on
  `UNAVAILABLE`, fails on `MISMATCH`; `require` fails on anything but `MATCH`.
  `MISMATCH` always fails — that is evidence about the data.

### 2.4 Multi-account, and where the lock lives

Unstated in §A4 and the first thing a wallet asks. Specified:

- **One client, one mirror, N accounts.** `KeylessClient` owns exactly one
  `Engine` and one IndexedDB database. The feed pass is done once per refresh
  and shared; discovery is per account.
- `getNotes(a)` with `refresh: 'auto'` (default) runs a feed pass at most once
  per `pollIntervalMs`, then discovers. Concurrent `getNotes` calls for
  different accounts coalesce onto one feed pass.
- **Serialization.** All engine access is serialized inside the client (the
  wasm `Engine` is `&mut` for both `advance` and `discover`; there is no
  concurrency to be had). Cross-tab, `navigator.locks.request('strk20:<db>')`
  as in §4.3, with §4.3's scope correction intact: last-writer-wins is safe for
  key-independent rows because they are self-verifying, and the `cursors` store
  is safe only because every `discover()` supplies fresh
  `crypto.getRandomValues` entropy.
- **One sealed blob per (account, chain, pool)**, keyed by §4.4's `keyId`.
  Accounts never share a cursor.
- `watch()` on N accounts: one SSE subscription, one feed pass per poke, N
  discoveries, N `notes` events. A locked account's `viewingKey()` rejection
  emits `{type:'status', state:'locked'}` for that subscription and skips it;
  the others proceed.

### 2.5 The network hook (new)

```ts
export interface RequestRecord {
  url: string;                 // absolute, exactly as issued
  method: 'GET' | 'POST';
  purpose: 'feed' | 'live' | 'anchor-rpc';
  status: number;
  bytes: number;               // response body length
  transferBytes: number | null;// PerformanceResourceTiming.transferSize, or null
  fromCache: boolean;
  ms: number;
  requestBodyBytes: number;    // 0 for every feed request, by construction
}
```

Every request the client makes passes through `onRequest`, including requests
made inside the worker (forwarded over `postMessage` — otherwise the hook lies
by omission exactly where the audit matters). Three uses, in order of
importance:

1. it is what makes the no-key claim **checkable by the integrator**, not just
   by our test suite: point it at your logger and read the URLs;
2. metrics and budgets in a real app;
3. the demo's network panel (§3.6).

Test obligation: the leg-**d** `capture-scan` scanner, promoted to a bin per
§4.9, runs over the emitted `RequestRecord` stream as well as over the proxy
capture — the key and the address must not appear in any of the 13 encodings,
and the scanner's self-test (it *does* find a planted key) is retained.

### 2.6 Data flow (replaces §4.3's fetch plan)

§4.3 says:

> ```
> fetch manifest → Engine.check_manifest        # RETURNS a discriminant; never throws
>    "ok"       → nothing to apply
>    "behind"   → fetch+apply epochs last_epoch+1..
>    "diverged" → drop persisted state, cold start
> cold start   → snapshot .zst → verify zst hash → fzstd → apply_snapshot(+anchor)
> ```

**Replaced by:**

```
open IDB (or memory fallback)                                        [persistence]
fetch genesis.json → byte-compare vs meta.genesis                    [CHAIN_MISMATCH]
  → Engine.load(state, genesis)  or  Engine.new(genesis)
fetch manifest → Engine.check_manifest(bytes)
  "ok"        → done; no cassette, no fold                           [the warm path]
  "behind"    → run the need/provide/advance loop (§1.4)
  "diverged"  → delete `state` + `artifacts`; Engine.new(genesis); run the loop
loop           r = advance(); while r.status==='need':
                 fetch each r.need path (parameterless GET)
                 sha256_hex(z) === item.zst_sha256   ELSE FEED_HASH_MISMATCH
                 fzstd.decompress(z, item.cap)                       [R-I]
                 engine.provide(item, raw)
                 r = advance()
anchor         if anchorPolicy !== 'off':
                 engine.anchor_request(block) → POST to anchorRpcUrl (user's own)
                 engine.verify_anchor(block, proof, header)          [MATCH|MISMATCH|UNAVAILABLE]
persist        if r.state_changed: IDB.state = engine.export()       [Design M only]
               artifacts written/pruned per §2.8
discover       per account: key = await a.viewingKey()
                            sealed = IDB.cursors[keyId]
                            entropy = crypto.getRandomValues(new Uint8Array(32))
                            out = engine.discover(addr, key, sealed, entropy)
                            IDB.cursors[keyId] = out.sealed; zeroize(key)
subscribe      EventSource /feed/live (keyless; no auth, no params)
                 on head/epoch/snapshot → repeat from `fetch manifest`
                 on error → poll fallback (§2.5), status 'polling'
```

The differences that matter: TypeScript never names an epoch, never reads the
manifest, and never decides that a snapshot should be applied. It fetches paths
a keyless module printed.

### 2.7 The SDK provider (amends §4.1's `/sdk`)

```ts
// strk20-discovery/sdk
export function localDiscoveryProvider(
  client: DiscoveryClient, account: Account
): DiscoveryProvider;
```

Base §12.1's cursor-conversion semantics carry over verbatim; `NotesCursor` /
`ChannelCursor` round-trip identically to `IndexerDiscoveryProvider`, which is
what `Engine.export_reference_cursor` exists for. Two obligations this design
adds:

- the provider must be constructible **without a key** and acquire it per call
  through `Account.viewingKey()`, for the same lock-screen reason as §2.2;
- migration in both directions is a documented, tested path: an SDK user with an
  existing `DiscoveryCursor` can seed us (`import_reference_cursor`, the mirror
  of `export_reference_cursor`), and a user of ours can leave with
  `export_reference_cursor` and no resync. An integrator who cannot leave will
  not arrive.

### 2.8 Persistence and cache invalidation (amends §4.4/§4.5)

§4.4's schema stands (per-chain-and-pool database name, four stores, full
64-hex `keyId`, genesis stored **and** re-fetched and byte-compared, nothing
tail-derived ever stored, the five quirks each with a test). §4.5's Design R /
Design M framing and the §4.6 gate stand. What was missing is the invalidation
table — a cache with no written invalidation rule is how the folded-mirror
design earns the bad reputation §4.5 warns about. Normative:

| trigger | `meta` | `artifacts` | `state` | `cursors` |
|---|---|---|---|---|
| `meta.format_v` ≠ ours | delete DB entirely | — | — | — |
| stored genesis ≠ fetched genesis | **no writes at all**; throw `CHAIN_MISMATCH` | keep | keep | keep |
| `check_manifest` → `"diverged"` | keep identity rows | delete all | delete | keep (invalidated per-seal by `ckpt_epoch_hash`, §1.6) |
| `Engine.load` → `STATE_CORRUPT`/`STATE_VERSION`/`STATE_FOREIGN` | keep | keep | delete | keep |
| engine major version bump | keep | keep | delete | delete if `seal_v` changed, else keep |
| snapshot rejected (`auto` fell back) | keep | delete the snapshot rows | delete | keep |
| IDB eviction / empty store | cold start; **never** an error (§4.4 quirk 3) | | | |
| `artifacts` over budget | prune oldest epochs at or below `state.last_epoch`; never the snapshot, never an epoch above `state`'s floor | | | |

`maxArtifactBytes` default 64 MB (mainnet's whole compressed feed is 16 MB
today, so the default is not a constraint yet; snapshots change the shape and it
becomes one). Deleting `state` is always correct and always safe — it is a
cache, and §4.5's honest trust statement about IndexedDB integrity between loads
is printed in the README where M ships, unchanged.

One correction of emphasis for §4.6. The gate is pre-registered and stays
pre-registered; but live-run §3 already measured the thing it was going to
decide, natively: 5.97 s cold, 0.03 s warm, and *"WASM will be slower, not
faster"*. The gate's remaining real question is not "R or M" but **"how much
does the snapshot lane (L1) reduce the cold path once snapshots exist"** — which
is unmeasured because snapshots are roadmap item 1. So: build M-capable code
paths, run the gate on the snapshot lane, and let it set the default. Do not
publish a number before it runs.

### 2.9 Errors, and what an integrator must handle

`Strk20Error` per §4.2, carrying §3.7's closed code set plus §1.7's additions.
The README documents exactly four handling classes, because a longer list gets
ignored:

1. **retryable** (`error.retryable === true`): `TRANSPORT`,
   `FEED_ADVANCED_MIDSYNC`, `KEY_UNAVAILABLE`. Back off and repeat the call.
2. **configuration** (`CONFIG_INVALID`, `CHAIN_MISMATCH`): your options or your
   feed URL are wrong; no retry will help.
3. **integrity** (`FEED_HASH_MISMATCH`, `FEED_CHAIN_BROKEN`,
   `SNAPSHOT_ROOT_MISMATCH`, `ANCHOR_UNBOUND`): the feed disagrees with itself
   or with the chain. Surface it; do not silently refetch in a loop.
4. **local state** (`STATE_*`): handled internally by the invalidation table;
   they reach you only if you supplied a `StorageAdapter`.

`verified: 'server-asserted'` is not an error and is not hidden: `status()`
carries it, and the README says what it means in the same words as the CLI's
warning at `sync.rs:305`.

### 2.10 `DelegatedClient` (§4.8) stands

Unchanged: the reference compat wire, fetch-based SSE with an `Authorization`
header rather than `EventSource` (review finding 8), the `/health` chain-identity
check before any key is sent, and `assertUncheckedNetwork` as the only way past
a server that cannot be checked. It adopts §2.2's `Account` and §2.3's event
shapes so the constructor swap in leg **v** still works, and its README line
stays as blunt as base §9's compat row: **the viewing key travels to a server you
run.**

---

## 3. The demo

### 3.1 What it must prove, in order

1. Cold is seconds, warm is instant — **side by side**, not sequentially in a
   log where the contrast is lost.
2. The network sees no key and no address — **visibly**, by listing every URL,
   and by showing that two different identities produce the same list.
3. A note is discovered, and how long that took.
4. The request count and the byte count, because the drop from ~518 requests to
   a handful once snapshots land is the product.

Everything else is decoration and is cut before any of these.

### 3.2 Shape

`ts/demo`, a single-page app, no framework, no bundler beyond `tsc` + `esbuild`,
importing `strk20-discovery` from the workspace. Two lanes, one state machine:

- **LIVE lane** — a real feed (`strk20 run` over the Sepolia mirror by default;
  a mainnet feed URL is accepted). Real fetches, real folds, real chain events.
- **REPLAY lane** — a recorded cassette of one pinned manifest (the same
  `bench/fixtures/` artifacts §4.6 uses), served from a local static directory.
  Deterministic, offline, used for the cold/warm comparison and the identity
  toggle so the numbers are stable when someone is watching.

The lane is a labelled toggle, and every log line and card carries its lane. A
replayed run never prints an unqualified timing.

Layout, top to bottom: **cards** (persistent, never scroll away) then **stages**
(buttons) then **the log** (scrolls). The brief's mutating last line is the log;
the orchestrator's side-by-side requirements are the cards. They do not compete.

### 3.3 Stages — approve-precedes-swap

Each stage is a row of controls, dimmed until its precondition holds, with the
precondition named in the dimmed state rather than left to guessing.

**Stage 1 — Feed.** `[ Cold load ]  [ Warm load ]  [ Cold A/B ]`
No key exists yet, and the stage says so: *"no key needed for this — the
expensive part of this system runs with nothing about you in the process."*
`Cold load` deletes the IndexedDB database, constructs a client and calls
`sync()`. `Warm load` constructs a client over what cold left and calls `sync()`.
`Cold A/B` is §3.6's identity comparison.

**Stage 2 — Identity.** Precondition: stage 1 completed once.
`[ Generate demo key ]  [ Paste viewing key ]  [ Identity B ]`
Above the buttons, a banner that is not dismissible:

> This page holds a viewing key, because that is what our customer does — a
> wallet, or a key-holding app. A dapp on the Starknet Wallet API never receives
> a viewing key: the wallet holds it, discovers the notes and builds the proof.
> Such a dapp cannot use this library and does not need it.

A generated key discovers nothing (it is nobody's key) and the demo says so —
useful precisely because it proves the request stream is the same as a real
key's. `Paste viewing key` accepts a 64-hex string, converts once to
`Uint8Array`, keeps it in a closure behind an `Account.viewingKey()` that returns
a fresh copy per call, and never renders it. The UI shows `keyId` (the §4.4 HKDF
id) and the address, plus a fixed line: **viewing key: held in this tab — 0
bytes of it have crossed the network.** That line is computed, not written: it is
`onRequest` bytes matched against the key in the 13 encodings, i.e. the
`capture-scan` predicate running live in the page.

**Stage 3 — Discovery.** Precondition: an identity exists.
`[ Check now ]` and a toggle `Subscription: ON | OFF`.
ON subscribes to `/feed/live` and runs discovery on every poke. OFF leaves
`Check now` as the only trigger. Either way the log records how long it took, and
which trigger caused it (`sse` / `poll` / `manual`).

**Stage 4 — Act.** Precondition: discovery has run once.
`[ Deposit ]  [ Send ]  [ Withdraw ]`

We have no write path — deliberately, per roadmap §"Deferred" and design-notes
§4 — and the demo must not pretend otherwise. Each button opens a hand-off
sheet: what to do in your wallet or with the SDK (the pool address, the amount
field, the exact `starkli`/SDK snippet for the Sepolia pool
`0x0254a6b2997ef52e9f830ce1f543f6b29768295e8d17e2267d672c552cfe0d91`), a
`[ I've submitted it ]` button, and an optional tx-hash field. Pressing it arms a
watcher and pushes the pending log line. The sheet's header states the honest
position in one sentence:

> We are the read half of every write. You sign it; we tell you the moment your
> note exists, and later the moment it is spent.

For **Send** and **Withdraw** the watcher is armed on the *nullifier* of a
selected note, so the pending line resolves on the spend rather than on a new
note — which exercises exactly the property live-run §7 confirmed on real data
(the nullifier the client predicted appeared verbatim in the chain's `NoteUsed`
event).

In the REPLAY lane the buttons are disabled with the reason shown
("replay has no chain to write to"), never silently faked.

### 3.4 The log

One array of records, rendered as fixed-height monospace rows:

```ts
type LogLine = {
  seq: number; t: number;                       // ms since page load
  lane: 'live' | 'replay';
  text: string;                                 // mutates while pending
  status: 'pending' | 'ok' | 'warn' | 'fail';
  elapsedMs?: number;                           // set when it commits
  detail?: string;                              // one line, revealed on click
};
```

Rules, enforced in the reducer rather than by discipline:

- **at most one `pending` line, and it is always the last.** A new operation
  while one is pending is refused by the stage gating, not queued.
- a pending line mutates in place (`"waiting for the note…"` →
  `"waiting for the note… (block 14,339,102, 3 blocks ago)"`) and commits with
  its elapsed time when it resolves;
- committed lines never mutate;
- `warn` for anything degraded but working (SSE dropped to polling; `verified`
  came back `server-asserted`; persistence fell back to memory); `fail` for a
  thrown `Strk20Error`, with `error.code` in `detail`.

Rendering is `text` on the left, dot leaders, elapsed right-aligned:

```
[t+0.4s]  cold load — 1 manifest, 1 snapshot, 3 epochs, 1 head ......... 2.13 s
[t+2.6s]  warm load — manifest said "ok", nothing to fold .............. 0.04 s
[t+9.1s]  waiting for the note…                                          ⠋
[t+21.7s] note 0xce526b28… 3.0 STRK found ............................. 12.6 s
```

### 3.5 What is measured, and how it is obtained honestly

| shown | obtained | excludes / caveat printed in the UI |
|---|---|---|
| **cold total** | `performance.now()` around `client.sync()` after `indexedDB.deleteDatabase` | includes fetch; the lane label says whether the bytes came from the network or a local replay dir |
| **cold compute** | sum of the wrapper's own spans around `advance()` + `provide()` + zstd | excludes all network — the number that is comparable to the native 5.97 s |
| **cold zstd** | span around `fzstd.decompress` | reported separately because it is a TS cost, not a wasm one |
| **warm total** | `performance.now()` around a `sync()` on a fresh client over the persisted DB | the honest comparator; see the reload variant below |
| **warm (page reload)** | a flag written to IDB before `location.reload()`; on boot, measure from `performance.timeOrigin` to `sync()` resolving | includes wasm instantiation and IDB open — the number a user actually feels |
| **time-to-discover (ours)** | poke or click → the `notes` event containing a note whose `noteId` was not previously known | our latency, and only ours |
| **time-to-discover (end to end)** | `Date.now() - note.blockTimestamp * 1000` | labelled *"includes block production and indexer lag — not our latency"*, never quoted as a product number |
| **requests / bytes** | count and `bytes` summed from `onRequest`; `transferBytes` shown alongside when `PerformanceResourceTiming` supplies it | `transferSize` is 0 on cache hits and null cross-origin without `Timing-Allow-Origin`; the demo server sets that header and the UI prints `n/a` when it is missing rather than showing a wrong 0 |
| **fold work** | `DiscoverOut.stats_json` (slots read, events scanned, passes) and `advance()`'s stats | not a timing; it is what the machine did |

Rules that keep the numbers honest and are worth stating because they are easy
to get wrong:

- **cold and warm are measured sequentially, displayed simultaneously.** Two
  folds racing on one core would make both numbers lies. The card holds the last
  cold result and the last warm result side by side, each stamped with its lane,
  its timestamp and its feed URL; it never averages across lanes or across feeds.
- **no median, no p95, in the demo.** Single runs, honestly labelled. The
  statistical profile belongs to §4.6's bench under a throttled headless
  Chromium, and the demo links to its results rather than imitating them.
- **nothing measured is quoted from this document.** Every number on screen was
  produced by the run in front of you. The native figures from live-run appear
  in exactly one place — a small "measured natively, 2026-08-31" footnote under
  the cold card, for scale — and never inside the live readout.

### 3.6 The network panel, and the identity toggle

A persistent card, fed by `onRequest`, with three regions.

**(a) Every URL.** A scrolling list: method, path, bytes, ms, purpose. Feed
requests are all `GET` with a zero-length body and no query string — the panel
shows a `?`-free column and a `body: 0 B` column so that is visible rather than
asserted. The user's own RPC (`purpose: 'anchor-rpc'`) is the only `POST`, is
visually separated, and is annotated: *"your RPC, not the feed. Body: a public
pool address and a public block number — identical for every user."*

**(b) The live key scan.** The `capture-scan` predicate, running in the page over
every URL and every request body: the key and the address, in the 13 encodings
(minimal hex, padded, decimal, upper/lower, `0x`-prefixed, raw BE/LE bytes).
Displayed as `key: 0 hits / 518 requests`. A **self-test button** plants the key
in a synthetic request record and shows the scanner catching it — because a
detector that has never fired proves nothing, and live-run §5 ran exactly this
self-test.

**(c) Identity comparison.** Two claims, both checkable in five seconds:

1. **Discovery makes zero requests.** After a warm sync, `Check now` under
   identity A and then under identity B each add **0** rows to the panel. This
   is stronger than "identical" and it is the true statement: the key-consuming
   code path has no network access at all — the wasm import section (§3.9's
   audit) does not contain one.
2. **Cold streams are identical.** `Cold A/B` runs two full cold loads in two
   separate databases, under identity A then identity B, records
   `(method, path, bytes)` for each, and shows both counts, both byte totals, and
   the sha256 of each stream — plus a diff view when they disagree. Both runs
   start from a deleted database, because the request stream is a function of
   *(feed state, local mirror state)* and comparing a cold run against a warm one
   would be comparing two different questions. The panel says that out loud:

   > A client's requests depend on what the feed has published and on what this
   > browser has already stored — never on who you are. Both runs below start
   > from an empty store, which is why they are comparable.

Under the two counts, one line of context, marked as a native measurement:
*"measured natively on 2026-08-31: 609 requests / 64,509 bytes on Sepolia, and
518 requests / 16 MB on mainnet — before snapshots."* The panel's live counter
sits next to it, so when snapshots land the drop is visible on the same card
without editing a word.

### 3.7 The cards

**Cold vs Warm.** Two columns, same rows: total, compute, zstd, requests, bytes,
epochs applied, slots folded. A ratio badge between them (`×N faster`) computed
from the two totals actually on screen. Empty columns say `not run yet` rather
than `0`.

**Trust.** `verified` grade as a three-state badge (`replayed` / `anchored` /
`server-asserted`), head, l1_accepted, last epoch, history floor, snapshot basis.
`server-asserted` renders with the CLI's own words: *"the snapshot's slot set is
attested only by an anchor the feed itself published — set your own RPC for
`anchored`."* Setting `anchorRpcUrl` in the UI flips it live, and that flip is
the demo's best single argument.

**Notes.** Balance per token, and the note list with `spent` struck through.
Nothing else.

### 3.8 The state machine

```
              ┌──────────────────────────────────────────┐
boot ─────────► feedIdle                                  │
                 │ Cold load        │ Warm load           │
                 ▼                  ▼                     │
              feedCold ──────────► feedReady ◄────────── feedWarm
                 (sync, progress)     │                    (sync)
                                      │ identity chosen
                                      ▼
                                  identityReady
                                      │ Check now / poke
                                      ▼
                                  discovering ──► ready ──┐
                                      ▲                   │
                                      └───────────────────┘
                                            (watch)
```

Orthogonal region, independent of the above:

```
op: idle ──[Deposit|Send|Withdraw armed]──► pending{kind, armedAt, watch}
     ▲                                          │
     └──────[note appeared | nullifier spent | timeout 10 min | cancel]
```

Exactly one `pending` op at a time; the stage buttons are disabled while one is
armed, which is what makes the log's one-pending-line rule enforceable rather
than aspirational. A timeout commits the line as `warn` with the elapsed time and
the reason — never leaves a spinner running forever, which is the failure mode
every demo has.

Subscription state (`live` / `polling` / `degraded`) is a third orthogonal region
driven by `{type:'status'}` events; it renders as a dot next to the toggle and
writes one `warn` log line on each transition.

### 3.9 Demo acceptance (it is a test target, not a slide)

Playwright, in CI, against the REPLAY lane so it is deterministic and offline:

- **d1** cold then warm: both cards populated; warm total < cold total; the warm
  run's `onRequest` list contains exactly `genesis.json`, `manifest.json` and
  `head.ndjson` (§4.4's reload delta, plus SSE).
- **d2** identity A vs identity B cold: request stream hashes equal; the
  assertion is on the hash, and the failure message prints the diff.
- **d3** discovery under both identities after a warm sync: `onRequest` gains
  zero rows.
- **d4** the live key scan reports 0 hits across the whole session, and the
  self-test button makes it report 1.
- **d5** exactly one pending log line ever exists; a committed line never
  mutates (asserted over a recorded reducer trace).
- **d6** a forced `Strk20Error` (a corrupted epoch in a fault-injecting replay
  dir) commits a `fail` line carrying `FEED_HASH_MISMATCH`, and the app is still
  usable afterwards.
- **d7** the timeout path: an armed op with no matching note commits `warn` at
  the deadline.

---

## 4. Departures index

| § | what it said | what this replaces it with | why |
|---|---|---|---|
| §0.4.1 | `NoteSet` value type with `notes_get`/`notes_put` | the shipped row-level trait methods; `NoteSet` paragraph deleted | the refactor built row-level and it is the better fit; the diff moves to the facade |
| §0.4.1 | trait carries the note registry only | plus: `refresh_spent`, `prune_missing_notes`, `sync_once` become generic free functions in `strk20-consumer` | otherwise the browser re-implements spent-state semantics — the one rule live-run §7 showed is subtle |
| §3.1 | crate list | unchanged, plus `StagedTransport` and `std::sync::Mutex` in `MemStore` | `RefCell` is not `Sync`; the trait requires `Send + Sync` |
| §3.2 | `MemStore` with `base`/`tail` compartments | `MemStore` implementing the real `ConsumerStore`, split feed/scratch | the trait expresses the tail through `replace_range(Range::Above)` |
| §3.3 | `apply_snapshot` / `apply_epoch` / `apply_head` pushed from TS | `need()` / `provide()` / `advance()` over the real `apply_feed` | one implementation of the trust pipeline; `auto` fallback is inexpressible in the push ABI; TS stops parsing artifacts |
| §3.3 | — | `anchor_request` / `verify_anchor` | otherwise the browser can never reach `verified: 'anchored'` while the CLI can |
| §3.3 | `DiscoverOut` (4 fields) | plus `stats_json` | an honest UI must say what the machine did |
| §3.6 | seal `notes[]` unbounded | seal only notes with `block <= ckpt_at` | otherwise the seal carries tail state and "no persisted reorg logic" is false |
| §3.6 | no seal invalidation | `ckpt_epoch` + `ckpt_epoch_hash`, mismatch = no cursor | a cursor is a position in a history; a diverged feed changes the history |
| §3.7 | code set | plus `CASSETTE_FULL`, `CASSETTE_UNEXPECTED`, `ANCHOR_UNBOUND`, `KEY_UNAVAILABLE`, `ABORTED` | the new surfaces need codes; `UNAVAILABLE` stays a return value |
| §4.1 | "/sdk: one install, both audiences" | `/sdk` is the primary surface; README opens with "if you are a Wallet-API dapp, you do not need this" | a Wallet-API dapp never holds a viewing key |
| §4.2 | `KeyRef { viewingKey: Uint8Array }`, `subscribe(k, cb)` | `Account { address, viewingKey(): Promise<Uint8Array> }`, `watch(a, cb)` | the client must not retain a key across passes; locked wallets are the normal case |
| §4.2 | no `sync()`, no `signal`, no progress, no `refresh` | all four added | a seconds-long cold path with no progress, no cancel and no key-free mode is unusable in a wallet |
| §4.2 | `anchorRpcUrl` with mandatory semantics | `anchorPolicy: 'off'|'best-effort'|'require'` | LIVE-6: a capability gap must never fail a sync |
| §4.2 | — | `onRequest` hook, forwarded from the worker | makes the no-key claim checkable by the integrator, and powers the demo panel |
| §4.2 | `Note` fields | plus `blockTimestamp` | already in `BlockLine`; every UI needs it and it is the honest end-to-end denominator |
| §4.3 | TS decides what to fetch | TS fetches what `need()` names | the URL author has no key input |
| §4.4/§4.5 | schema + two designs | plus the invalidation table (§2.8) | a cache with no written invalidation rule is worse than no cache |
| §4.6 | gate decides R vs M | gate stands; its real question is the snapshot lane's cold cost | live-run §3 already measured 5.97 s natively and said WASM will be slower |
| §A4 | multi-account unspecified | one client, one mirror, N accounts, one seal each, coalesced feed passes | first question a wallet asks |

## 5. Implementation order

1. **0a completion** — land `refresh_spent`, `prune_missing_notes`, `sync_once`,
   `run_incoming`/`run_outgoing`, `reopen_cursor`, `register_notes`,
   `SyncReport` into `strk20-consumer` as generics over `ConsumerStore`.
   `crates/client` keeps `FeedStore`, `ClientView`, transports, CLI, MPT verify.
   Test: the existing suite, plus the §0.4.1 conformance leg run over
   `FeedStore` and a stub store.
2. **`MemStore` + `StagedTransport`** in `crates/client-wasm`, with the third
   conformance leg (engine-over-`MemStore` ≡ engine-over-`FeedStore` ≡
   engine-over-`MockBackend`) red first.
3. **The ABI** (§1.4, §1.5), `nodejs` build, driven from a Node harness over the
   checked-in fixture feed before any browser code exists.
4. **npm core** — the need/provide/advance loop, IndexedDB, worker, `onRequest`,
   `sync()`/`getNotes()`/`watch()`.
5. **§4.6 bench** on the snapshot lane; set `persist` default and the §3.9 size
   FILL-IN from measurements.
6. **`/sdk` provider** and the cursor migration both ways.
7. **The demo**, REPLAY lane first (it is the CI target), LIVE lane second.

`DelegatedClient` (§4.8) and `strk20-sync serve` (§A5) proceed in parallel and
are unaffected by anything above except the `Account` shape.
