# A3 — the browser-performance revision of §A3/§A4, plus the demo

Written 2026-08-31. Role: what the browser actually does — bundle size,
main-thread blocking, IndexedDB quirks, SSE through proxies, the cost of
folding and of re-hashing, cold start on a mid-tier phone.

Revises `docs/spec/consumer-path.md` **§A3** (wasm ABI) and **§A4** (npm
package) against three things that did not exist when they were written:

1. the real `ConsumerStore` trait now in flight in `crates/consumer/src/store.rs`
   (0a), which is **not** the trait §0.4.1 sketched;
2. the verified positioning fact that a Wallet-API dapp never sees a viewing
   key and therefore is not our customer;
3. the live measurements in `docs/research/live/live-run-findings.md`.

Given and not reopened: the two-block architecture, the persistence trade-off,
the storage-proof correction of §12 (§11 retracted), the hard invariants (key
never leaves the browser, byte-identical request streams, WASM is a pure
synchronous computer, `discovery-core` consumed unmodified, epochs immutable).

Estimates are labelled **[est]** and carry their arithmetic. Everything else is
either measured (cited) or a design decision.

---

## 0. Summary of departures

| # | §  | What §A3/§A4 says | Replacement | Why |
|---|----|---|---|---|
| D1 | §3.3 | `apply_snapshot` / `apply_epoch` / `apply_head` / `check_manifest` as four ABI entry points | `plan()` + `apply_staged()` over a **StagedTransport**, running the real `apply_feed` unmodified | The four-entry ABI forks the verification *order* into TypeScript. `crates/consumer/src/apply.rs` now owns that order and is shared with native. One implementation or none. |
| D2 | §3.2 | `MemView<'a> { store: &'a MemStore }` | `MemStore(Arc<Inner>)`, `MemView { inner: Arc<Inner>, bound: u64 }`; `Mutex`, never `RefCell` | The shipped trait has `type View` with **no lifetime parameter** and requires `Send + Sync`; the borrowed view does not compile and `RefCell` is not `Sync`. |
| D3 | §3.2 | `base_events: Vec<EventRec>` with per-event `Vec<Felt>` fields | flat felt arena + fixed-size header table | ~20–25 MB and ~240k allocations saved on the epochs lane **[est, §1.3]**. This is the difference between "tight" and "the tab is killed". |
| D4 | §3.3 | `discover(...)` — one synchronous full pass | `discover_begin` / `discover_step(deadline_ms)` / `discover_finish` | A full pass measured **1.19 s** on Sepolia with one note; it is unsliceable as specified and blocks whatever thread it is on. `IoBudget` already makes the engine resumable — we were throwing that away. |
| D5 | §3.5 | state blob = canonical NDJSON, `export() -> Vec<u8>` | JSON header line + **binary body** + JSON trailer line; `export_begin`/`export_chunk(i)` in ≤4 MB frames | A ~40 MB **[est]** blob hex-encoded and moved as one buffer costs three copies (wasm heap → JS → structured clone) and 250k+ hex parses per load. `jq` still reads line 1 and the last line. |
| D6 | §4.5/§4.6 | "if the gate selects R, `export`/`load` stay dormant" | **Design M is built.** The L2 arm of the gate is already decided by measurement; only L1 (snapshot lane) is open | 5.97 s native cold fold ⇒ p95 `t_cold(L2)` > 2000 ms is not in doubt. The measured 0.03 s warm resync is a **Design-M number** — a persisted folded mirror — not a Design-R number. |
| D7 | §4.1/§4.2 | package headline is `KeylessClient`; `/sdk` is a subpath afterthought | headline is `LocalDiscoveryProvider` (drop-in for `IndexerDiscoveryProvider`); `KeylessClient` is the lower layer | Verified: a Wallet-API dapp never holds a viewing key, so it can never call us. The customer is a wallet or a key-holding backend, and that audience wires `createPrivateTransfers({discoveryProvider})`. |
| D8 | §4.2 | no network observability in the API | `onRequest` / `client.network()` as a **first-class, shipped** surface | The demo's central claim ("look, no key and no address in any URL") must be computed from the client's own record of what it fetched, not from a demo-side wrapper a viewer cannot trust. Also gives integrators a real cost meter. |
| D9 | §4.2 | `worker?: boolean` default true | worker default true, **and** the main-thread mode is documented as blocking with `status().blocking = true`; leader-elected SSE via Web Locks | Six-connection-per-origin HTTP/1.1 cap: N tabs each holding an `EventSource` starve the feed fetches. One tab holds the stream. |
| D10 | §4.4 | IDB quirks 1–5 | + Safari 7-day ITP eviction, `navigator.storage.persisted()` surfaced, ≤4 MB record chunking | A returning Safari user silently gets a cold start; `requestPersistentStorage` is load-bearing there, not cosmetic. |
| D11 | §3.9 | 300 KB provisional total wire budget | unchanged as a number, but **a step-3 measurement gate on Pedersen constants** with a documented split-module fallback | `feed::mpt` pulls Pedersen; if its precomputed tables are not already shared with `discovery-core`'s hashing, they alone can exceed the budget. Measure before designing around it. |

Everything not listed here stands as written, including §3.6 (sealed state and
the `ENTROPY_REUSED` guard), §3.7's error vocabulary, §3.8 (the fork), §4.3's
scope-corrected locking argument, §4.7 (`fzstd`), and §4.9's single-scanner
rule.

---

## 1. The budget, derived from measurements

### 1.1 What was measured (the only numbers quoted anywhere below)

| measurement | value | source |
|---|---|---|
| cold fold, full mainnet history, native | **5.97 s** (2.2 s user + 2.0 s sys), peak RSS **31 MB** | live-run-findings §3 |
| same from a local dir, no HTTP | 6.18 s — the cost is the fold, not the network | ibid. |
| warm re-sync over a folded mirror | **0.03 s** | ibid. |
| feed / client mirror | 16 MB feed → **60 MB** SQLite mirror (3.7×) | ibid. |
| mainnet volume | 118,960 events, 28,383 pool-active blocks, 515 epochs | live-run-findings §2 |
| pool storage | 139,131 writes over **134,879 distinct slots** (96.9 % first writes) | live-run-findings §3 |
| anonymity set | 31,077 notes | ibid. |
| our Sepolia note, discovered keylessly | **1.19 s** | live-run-findings §5 |
| two wallets, request streams | **byte-identical**: 609 requests / 64,509 bytes (Sepolia); 518 requests / 16 MB (mainnet) | live-run-findings §5, §2 |

The request counts are arithmetic, and the demo should show the arithmetic
because it is checkable by eye: Sepolia 1 genesis + 1 manifest + **606 epochs**
+ 1 head = **609**. Mainnet 1 + 1 + **515** + 1 = **518**. Nothing else is
fetched. When snapshots land the same arithmetic reads 1 + 1 + 1 snapshot + 1
anchor + (0–1 epochs) + 1 head ≈ **5**.

### 1.2 Fold cost inside WASM **[est]**

Native's 5.97 s is 2.2 s user + 2.0 s sys. The sys half is SQLite writing 60 MB;
the browser does not pay it — and does not get it back either, because it pays
two costs native did not:

| stage | native | browser **[est]** | basis |
|---|---|---|---|
| zstd inflate 16 MB → ~80 MB | C zstd, inside the 2.2 s user | 1.0–1.6 s | `fzstd` pure JS, ~50–100 MB/s output |
| sha256 over ~80 MB of payloads | part of user time | 0.3–0.5 s | wasm sha2 ~150–300 MB/s |
| NDJSON parse (serde_json, ~80 MB) | same code | 1.0–1.6 s | ~50–100 MB/s in wasm |
| fold: 139k slot writes + 28k blocks + 119k events into memory | SQLite inserts (the sys 2.0 s) | 0.2–0.5 s | ~1.3 M B-tree inserts |
| **desktop total** | 5.97 s | **~3–5 s** | |
| **mid-tier phone (4× CPU throttle, the §4.6 profile)** | — | **~12–20 s** | |

Conclusions that follow and are not negotiable:

- **The epochs lane cannot run on every page load.** Not on desktop, and not at
  all on a phone. A persisted folded mirror is mandatory — which is what the
  live findings already concluded, now with the browser arithmetic attached.
- **Design R alone is not viable on the epochs lane**, because R's "re-verify
  stored bytes and refold on every load" is *exactly the 3–5 s above*. The
  0.03 s warm number is what a **persisted folded mirror** buys. §4.5 presents R
  as the default lane and M as an optimisation; on the epochs lane that is
  backwards. See D6 and §3.7.
- **Snapshots are the mechanism that makes mobile work**, not a cold-start
  nicety: they delete the inflate, the parse and the event fold, leaving
  134,879 slot lines (~15 MB payload **[est]**, ~4 MB compressed **[est]** at
  the measured 3.7× feed→mirror expansion and zstd-19 on hex NDJSON).

### 1.3 Memory, which is the harder constraint on a phone **[est]**

Linear memory for `MemStore` on the **epochs lane** (`history_floor = 0`, every
event held):

| component | naive layout | arena layout (D3) |
|---|---|---|
| slots: 134,879 × (32 B key + 32 B value + 8 B block) in a `BTreeMap` | ~12.6 MB | ~12.6 MB |
| blocks: 28,383 × ~80 B | ~2.3 MB | ~2.3 MB |
| events: 118,960, avg ~8 felts of keys+data | `Vec<Felt>` per field ⇒ 2 allocations/event, ~330 B/event + dlmalloc overhead ≈ **45–60 MB** | one `Vec<Felt>` of 951,680 felts (30.5 MB) + one 24 B header per event (2.9 MB) = **33.4 MB**, zero per-event allocations |
| **live data** | **60–75 MB** | **~48 MB** |
| realistic peak linear memory (fragmentation + a staged batch) | 90–110 MB | **70–85 MB** |

On the **snapshot lane** the events term collapses to whatever the epochs above
the basis carry — effectively zero — and the whole store is **~15 MB**. That is
a 5× difference in the number that decides whether a mobile tab survives.

Two consequences for the ABI:

- the event store is a **flat arena** (D3). Concretely:
  ```rust
  struct EventHdr { block: u64, index: u32, kind: u8, _pad: [u8;3],
                    tx: u32,            // index into `tx_hashes`
                    keys: (u32, u16),   // (offset into `felts`, count)
                    data: (u32, u16) }
  struct Events { hdr: Vec<EventHdr>,   // ascending by (block, index)
                  felts: Vec<Felt>, tx_hashes: Vec<Felt> }
  ```
  `RawEventAccess` answers a range query by binary-searching `hdr` on `block`
  and materialising only the events it returns. Nothing is allocated per event
  at fold time.
- **wasm linear memory never shrinks.** A session that folded the epochs lane
  holds ~80 MB until the module is dropped, and dropping a wasm instance does
  not return memory to the OS — only terminating the worker does. This is why
  `close()` must terminate the worker (§3.3) and why `worker: false` is a
  testing mode, not a deployment mode.

### 1.4 Bundle size — the one unmeasured risk that can bite hard

The 231 KB gzip spike baseline predates `feed::mpt`, AEAD, `serde_json` and the
ABI (§3.9 already says so). The specific hazard nobody has costed: **`feed::mpt`
needs Pedersen**, and Pedersen implementations ship large precomputed point
tables. If `discovery-core`'s own slot derivation already links the same tables,
the marginal cost is ~0; if not, the tables alone can approach the whole
provisional 300 KB budget.

Gate, added to §3.9 and run at **step 3**, before any npm code:

```
1. build engine with default features       → record gzip(wasm)
2. build with `mpt` removed (stub ring 5)   → record gzip(wasm)
3. delta = the true cost of client-side root verification
```

If the delta is small, nothing changes. If it is large, the fallback is a
**split module**, and it is a clean split because the surface is narrow:
`engine_bg.wasm` (fold + discover, loaded always) and `engine_mpt_bg.wasm`
(rings 5–6, loaded lazily on the first snapshot apply or anchor check, i.e.
never on the epochs lane and never in a warm load). The npm package's
`verifySnapshot` path awaits the second module; `status().verified` is
unaffected. **Do not design the split before the measurement** — if the tables
are shared it buys nothing and costs a second artifact to version.

Wire-cost mechanics, unchanged in intent and made concrete:

- `WebAssembly.instantiateStreaming` when the host serves `application/wasm`,
  falling back to `instantiate(await res.arrayBuffer())` — the package cannot
  control the host's `Content-Type`, and getting this wrong silently doubles
  cold start on a large module.
- the wasm is fetched **inside the worker**, so instantiation never competes
  with first paint.
- `fzstd` stays the only runtime dependency, exact-pinned (§4.7 unchanged).

---

## 2. Revised §A3 — the wasm ABI against the real `ConsumerStore`

### 2.1 The seam as actually built, and what that changes

`crates/consumer/src/store.rs` is not §0.4.1's sketch. Differences that matter
to the browser:

- an **associated type** `View: RawStorageAccess + RawEventAccess + Send + Sync`
  with **no lifetime parameter**;
- the whole trait is `Send + Sync` and every write takes `&self` (interior
  mutability), because the native impl is a `Mutex<Connection>`;
- writes that must not tear are **single calls**: `install_snapshot(slots, meta)`
  and `replace_range(range, blocks, meta, bump_generation)`;
- reads the browser must serve: `block_hash`, `block_hashes(Range)`,
  `read_slot_as_of`, `full_slot_set_as_of`, `is_empty`;
- the note registry is **store-resident** (`notes`, `upsert_note`,
  `set_note_spent`, `delete_note`, `delete_owner_notes`) — §0.4.1's `NoteSet`
  value type did not survive contact with the code.

The eleven methods `sync_once` actually drives are `apply_feed`, `view`,
`meta_get`, `meta_set`, `notes`, `upsert_note`, `refresh_spent`,
`prune_missing_notes`, `delete_owner_notes`, `reset_mirror`, `tail_generation`
— but `refresh_spent` and `prune_missing_notes` are today `FeedStore` inherent
methods, not trait methods.

**Recommendation R-A1 (reduces the wasm surface by two methods and a class of
divergence).** Both are expressible over methods the trait already has:

```rust
// crates/consumer/src/store.rs — DEFAULT methods, not required overrides.
fn refresh_spent(&self, owner: &Felt, block: u64) -> Result<Vec<Felt>> {
    let mut flipped = Vec::new();
    for n in self.notes(owner)? {
        let slot = discovery_core::privacy_pool::storage_slots::nullifiers(n.nullifier);
        let (v, _) = self.read_slot_as_of(&slot, block)?;
        let is_spent = v != Felt::ZERO;
        if is_spent != n.spent {
            self.set_note_spent(&n.note_id, is_spent)?;
            if is_spent { flipped.push(n.nullifier); }
        }
    }
    Ok(flipped)
}

fn prune_missing_notes(&self, owner: &Felt, as_of: u64) -> Result<usize> {
    let mut pruned = 0;
    for n in self.notes(owner)? {
        let slot = discovery_core::privacy_pool::storage_slots::notes(n.note_id);
        let (v, _) = self.read_slot_as_of(&slot, as_of)?;
        if v == Felt::ZERO { self.delete_note(&n.note_id)?; pruned += 1; }
    }
    Ok(pruned)
}
```

These are line-for-line what `crates/client/src/store.rs:900` and `:960` do,
with the SQL replaced by the trait's own `read_slot_as_of` (whose SQLite impl is
that same query). `FeedStore` may override them for the batched SQL if a
profile justifies it; the point is that **`MemStore` implements neither**, and
the spent/prune semantics cannot drift between hosts. The live findings pin why
this matters: a spent note's slot is **not cleared** (§7), so "spent" and
"present" are two different slot reads and a browser reimplementation getting
that backwards is a wrong-balance bug nobody would see in a demo.

### 2.2 `MemStore` — replacement text for §3.2

> §3.2 currently reads: `pub struct MemView<'a> { store: &'a MemStore, bound: u64 }`.

That does not compile against the shipped trait (`type View` has no lifetime).
Replacement:

```rust
pub struct MemStore(Arc<Inner>);

struct Inner {
    identity: FeedIdentity,                // chain_id, pool, genesis_block, epoch_size
    meta:  Mutex<BTreeMap<String, String>>,
    base:  Mutex<Base>,                    // folded from snapshot + epochs
    tail:  Mutex<Tail>,                    // folded from head.ndjson, replaced wholesale
    generation: Mutex<u64>,
    notes: Mutex<BTreeMap<Felt, NoteRow>>, // per-call scratch; see §2.6
}

struct Base {                              // everything ≤ last_epoch_to
    slots:  BTreeMap<[u8;32], SlotRec>,    // LATEST value per slot only — see the bound rule
    blocks: BTreeMap<u64, BlockMeta>,      // ≥ history_floor
    events: Events,                        // the D3 arena, ≥ history_floor
}
struct Tail {                              // everything > last_epoch_to
    writes: Vec<(u64, [u8;32], Felt)>,     // FULL write log, ascending by (block, slot)
    blocks: BTreeMap<u64, BlockMeta>,
    events: Events,
}
struct SlotRec { value: Felt, write_block: u64 }

pub struct MemView { inner: Arc<Inner>, bound: u64, history_floor: u64 }
```

- **`Mutex`, never `RefCell`.** The trait is `Send + Sync`; `RefCell` is not
  `Sync` and the crate will not compile with it. `std::sync::Mutex` is available
  and correct on `wasm32-unknown-unknown` (single-threaded, uncontended, ~free).
  **Lock discipline, normative:** no guard is ever held across a call into
  `apply::*` or `discovery::*`. Every method takes what it needs, clones or
  copies, and drops the guard before returning. A re-entrant lock in a
  single-threaded runtime is a deadlock, not a panic, and a deadlocked tab is
  the worst failure mode we can ship.
- **Base holds latest-per-slot; the tail holds a full write log.** This is what
  makes the 134,879-slot base cost 12.6 MB instead of 139,131 writes' worth, and
  it is sound because the base is only ever read at `bound ≥ last_epoch_to`.
  The tail is ≤10k blocks (~16 KB at today's volume, per the discussion notes),
  so keeping every write there is free and gives exact as-of reads at any
  `bound > last_epoch_to`.
- **The bound rule, made explicit and enforced:**

  | bound | served how |
  |---|---|
  | `< snapshot_basis` | `BOUND_BELOW_SNAPSHOT` (§1.5.2, unchanged) |
  | `== last_epoch_to` | base only |
  | `> last_epoch_to` | tail writes ≤ bound, then base |
  | `snapshot_basis ≤ bound < last_epoch_to` | **`BOUND_UNSUPPORTED {bound}`** — new code |

  The last row is new and honest: the base is a folded latest-per-slot map and
  cannot answer a historical as-of query. Nothing in `sync_once` asks for one
  (bounds are `last_epoch_to` or `head`), and `full_slot_set_as_of` — used by
  the ring-6 / §11.3 reachability check — is only ever called at an anchor block
  the client has folded to, i.e. `≥ last_epoch_to`. Making it an error rather
  than a silently-wrong answer is the same discipline as R-L.
- `MemView` is `Clone` (an `Arc` bump) and satisfies `Send + Sync` because every
  field is. `RawStorageAccess`/`RawEventAccess` futures are `Ready` by
  construction.
- **Execution model unchanged from §3.2**: `now_or_never().expect(...)` as a
  panicking programming-error tripwire, never `block_on`.

### 2.3 Applying the feed — replacement for §3.3's four entry points (D1)

> §3.3 currently exports `check_manifest`, `apply_snapshot(payload,
> manifest_snapshot_json, anchor_json)`, `apply_epoch(payload,
> manifest_entry_json)` and `apply_head(payload)`.

Those were written when Block B's apply logic did not exist as shared code. It
does now: `crates/consumer/src/apply.rs::apply_feed` owns the snapshot branch,
the `SnapshotRejected` → `reset_mirror` → epochs fallback, the divergence check
against the manifest's entry for our last applied epoch, the masked-reorg
`block_hashes` contradiction check, the `tail_from > last_epoch_to + 1`
mid-sync bail, and the ordering of all of it. Reimplementing that ordering in
TypeScript is the single largest correctness risk in the whole npm package, and
it is avoidable.

**Replacement: the module keeps `apply_feed` and gets its bytes from a
`StagedTransport` whose futures are all `Ready`.**

```rust
/// A FeedTransport whose bytes were pre-fetched by TypeScript. Every method is
/// `Ready`. A miss is not an error: it unwinds with `NeedArtifact`, which
/// `apply_staged` converts into a request list for the wrapper.
struct StagedTransport {
    genesis:   Option<Vec<u8>>,
    manifest:  Option<Vec<u8>>,
    epochs:    BTreeMap<u64, Staged>,      // compressed + inflated, paired
    snapshot:  Option<Staged>,
    snap_anchor: Option<Vec<u8>>,
    anchors:   Option<Vec<u8>>,
    head:      Option<(Vec<u8>, String)>,  // payload + ETag; None here = "unchanged"
    head_unchanged: bool,
    missing:   RefCell<Vec<Need>>,         // accumulates what was asked for and absent
}
struct Staged { zst: Vec<u8>, inflated: Vec<u8> }   // keyed by sha256(zst)
```

`StagedTransport::decompress(bytes, cap, artifact)` does **not** decompress: it
looks up `sha256(bytes)` in the staged map and returns the paired `inflated`
buffer, erroring `DECOMPRESS_UNSTAGED` if the pair is absent. This preserves
every hash check in their existing order — ring 1 hashes the compressed bytes
before `decompress` is called, ring 2 hashes the payload after — and it means
the module still verifies the association between what TypeScript claims it
inflated and what it inflated *from*.

ABI:

```rust
#[wasm_bindgen]
impl Engine {
    /// What this engine needs from the network, computed from its own state
    /// and a freshly fetched manifest. Deterministic: same state + same
    /// manifest ⇒ same list, in the same order, for every user.
    /// {"state":"ok"|"behind"|"diverged",
    ///  "cold_start":"snapshot"|"epochs"|"none",
    ///  "snapshot":{"e":1406,"file":"…","zst":"<64-hex>","bytes":N}|null,
    ///  "anchor_files":["snapshots/00001406.anchor.json","anchors.ndjson"],
    ///  "epochs":[{"e":1407,"file":"…","zst":"<64-hex>","bytes":N}, …],
    ///  "head":{"conditional_on_etag":"\"…\""|null},
    ///  "est_bytes":N}
    pub fn plan(&self, manifest_json: &str) -> Result<String, JsError>;

    /// Run the real `apply_feed` over the staged bytes.
    /// `staged` is a JS object: {genesis?:Uint8Array, manifest?:Uint8Array,
    ///   epochs?:{[e:string]:{zst:Uint8Array, inflated:Uint8Array}},
    ///   snapshot?:{zst,inflated}, snapshot_anchor?:Uint8Array,
    ///   anchors?:Uint8Array, head?:Uint8Array|"unchanged"}
    /// Returns
    /// {"status":"complete"|"need_more",
    ///  "need":{…same shape as plan…},          // present iff need_more
    ///  "outcome":{"epochs_applied":N,"tail_rewound":b,"tail_changed":b,
    ///             "head":H,"l1_accepted":L,"last_epoch_to":T,
    ///             "snapshot_basis":B|null,"snapshot_rejected":b,
    ///             "history_floor":F},
    ///  "state_changed":bool,                    // an epoch or snapshot landed
    ///  "verified":"anchored"|"server-asserted"|"replayed"}
    pub fn apply_staged(&mut self, staged: JsValue, cold_start: &str)
        -> Result<String, JsError>;
}
```

The `state` discriminant of §3.7 (`"ok" | "behind" | "diverged"`) survives
verbatim; it moves from a dedicated `check_manifest` into `plan().state`, so
there is one call where there were two and §3.7's rule — **staleness is a return
value, never a throw** — is unchanged and still asserted by leg **q**.

**Batching, and why it is not optional.** `apply_staged` is called in a loop:

```
plan → fetch a batch of K epochs → apply_staged → "need_more" → repeat
```

`apply_feed_once` restarts from the top of its epoch loop on each call and skips
epochs `≤ last_epoch_applied` from `meta`, and each epoch's `replace_range` is
its own transaction, so this is naturally resumable with no new state. **K = 8**
by default: 8 × ~31 KB compressed (16 MB / 515 measured) ≈ 250 KB in flight,
~1.2 MB inflated **[est]** — versus 16 MB compressed and ~80 MB inflated if
everything were staged at once, which on the epochs lane would add ~80 MB to the
~48 MB store and kill a phone tab outright. K is a constructor option
(`applyBatch`), clamped to 1..64.

The rare re-plan path is the `auto` snapshot fallback: a `SnapshotRejected`
triggers `reset_mirror` inside `apply_feed` and re-runs with `Epochs`, whose
`need` list is the whole epoch set. That returns `need_more` with a new plan,
TypeScript fetches, and the loop continues. This is exceptional, not the
cadence.

**One inconsistency this surfaces, worth fixing in the Rust while we are here.**
`apply_feed`'s epoch path never checks `entry.zst` — only the payload hash — so
the native client decompresses an epoch it has not transport-verified, while the
snapshot path (`apply.rs:440`) does check it. R-I says the `.zst` hash is checked
before decompression. The browser wrapper **must** check `entry.zst` before
handing bytes to `fzstd` (§4.7 already mandates it, and with `fzstd` there is no
other bomb defence). Add the same check to the native epoch path: three lines,
and it removes a real asymmetry between the two hosts.

### 2.4 Discovery — replacement for §3.3's `discover` (D4)

> §3.3 currently reads: `pub fn discover(&mut self, owner_hex: &str, key: &mut [u8], sealed: Option<Vec<u8>>, entropy32: &[u8]) -> Result<DiscoverOut, JsError>;` — "one full pass for one owner".

Measured: **1.19 s** for a Sepolia discovery with a 31k-note-class anonymity set
elsewhere and exactly one note of ours. A synchronous call of that length is a
frozen tab in `worker: false` mode and, more importantly, a demo that cannot
show progress and cannot show an honest "time to discover" broken into phases.

The engine is already resumable and we were discarding it: `sync.rs` runs
`sync_incoming_state` in a `for _ in 0..MAX_PASSES` loop with
`IoBudget::new(PASS_BUDGET)` (1,000,000), returning an incomplete cursor when
the budget is exhausted. Shrinking the budget yields control at pass
granularity.

```rust
#[wasm_bindgen]
impl Engine {
    /// Open a discovery session. `key` is copied into a `SecretFelt` and the
    /// caller's staging buffer is zeroized before return. `entropy32` MUST be
    /// 32 fresh bytes from crypto.getRandomValues (§3.6, unchanged); it is
    /// held for the session and consumed by `finish`.
    /// Returns an opaque u32 handle.
    pub fn discover_begin(&mut self, owner_hex: &str, key: &mut [u8],
                          sealed: Option<Vec<u8>>, entropy32: &[u8])
        -> Result<u32, JsError>;

    /// Run engine passes until `deadline_ms` of wall clock has elapsed OR the
    /// current phase completes. Never exceeds the deadline by more than one
    /// pass. Returns
    /// {"done":bool,
    ///  "phase":"ckpt_in"|"ckpt_out"|"live_in"|"live_out"|"spent"|"done",
    ///  "ops":N,                 // IO budget units consumed this step
    ///  "ops_total":N,
    ///  "channels":N,"notes":N,  // progress the UI may show; NOT a result
    ///  "elapsed_ms":M}
    pub fn discover_step(&mut self, handle: u32, deadline_ms: f64)
        -> Result<String, JsError>;

    /// Persist cursors, refresh spent state, seal, and produce the report.
    /// Zeroizes the session key. Legal only when the last step said done.
    pub fn discover_finish(&mut self, handle: u32) -> Result<DiscoverOut, JsError>;

    /// Abandon a session; zeroizes the key and the entropy. Idempotent.
    pub fn discover_abort(&mut self, handle: u32);
}
```

- **Deadline, not op count, is the parameter.** The IO budget is in ops, not
  milliseconds, and the ops-per-millisecond ratio varies by device by an order
  of magnitude. The module calibrates: it starts each session at 20,000 ops,
  measures the pass, and adjusts multiplicatively toward a slice that fits the
  deadline. The wrapper passes **16 ms** on the main thread (one frame) and
  **50 ms** in a worker (the worker has no frames to miss, and larger slices cut
  postMessage overhead). Time comes from `performance.now()` via one imported
  binding — note that this is a new import in the §3.9 allowlist, and it is a
  **clock, not a capability**: it reads no state and sends nothing. The
  allowlist file gains exactly one `(module, field)` pair and CI diffs it like
  every other.
- **The key lives in the session, not on the stack.** A session holds a
  `SecretFelt` (zeroize-on-drop) plus the entropy; `finish` and `abort` both drop
  it, and `Engine`'s `Drop` aborts every open session. §3.6's honest limit
  statement is unchanged: JS cannot zeroize its own buffers, and the guarantee
  is non-transmission.
- **Sealing is unchanged and happens only in `finish`.** A session that never
  finishes never consumes its entropy, so a torn discovery cannot burn a nonce.
  The `ENTROPY_REUSED` guard, the AAD, the counter's demoted role — all as §3.6
  writes them.
- **`DiscoverOut` is unchanged** (`report_json`, `sealed`, `added_json`,
  `spent_json`). The one-call-per-feed-change rationale in §3.3 stands; what
  changes is that the call is now three calls and a loop, which is what makes it
  survivable on a phone.
- **`history()` stays one-shot.** It is already page-bounded by `limit` and its
  cost is proportional to the page, not to history. It keeps the §1.1 paging
  contract verbatim, including "a walk that crosses `history_floor` TERMINATES
  the page set; it does not throw".

### 2.5 State blob — replacement for §3.5's format and `export` (D5)

The §3.5 grammar is canonical NDJSON with hex felts. On the epochs lane that is,
**[est]**, ~15 MB of slot lines + ~24 MB of event lines ≈ **40 MB**, and hex
doubles every felt. Costs per warm load: ~250k+ hex parses, a 40 MB
`Vec<u8>` on the wasm heap, a 40 MB copy into JS, and a structured clone into
IndexedDB. That is seconds of main-thread work to avoid seconds of fold work.

**Replacement — `strk20-state v2`, a framed hybrid:**

```
line 1   : {"t":"hdr","v":2,"kind":"strk20-state",…all §3.5 header fields…,"body":{"enc":"bin1","len":N}}\n
body     : N bytes, little-endian framed, in this order:
             u32 n_slots,  then n_slots × { [u8;32] slot, [u8;32] value, u64 w }
             u32 n_blocks, then n_blocks × { u64 number, [u8;32] hash, [u8;32] parent, u64 ts }
             u32 n_events, then the D3 arena: hdr table, then tx_hashes, then felts
last line: {"t":"end","slots":N,"blocks":P,"events":M,"sha256":"<64-hex over all preceding bytes>"}\n
```

- `jq` still reads the stamp (`head -1`) and the trailer (`tail -1`), which is
  what §3.5's debuggability argument was actually about; nobody was going to
  `jq` 134,879 slot lines.
- ~40 MB → **~17 MB** **[est]** (32-byte felts instead of 66-byte hex strings),
  and zero parsing: the body is `copy_from_slice` into the arena.
- **Every §3.5 structural check survives, unchanged in strength**: no line
  references a block `> last_epoch_to` (now a bounds check over the arrays,
  which is *stronger* than a grammar check because it cannot be defeated by a
  malformed line), no `b`/`ev` below `history_floor`, `snapshot_basis` absent or
  `history_floor == snapshot_basis + 1`, the trailer self-hash, and `load`'s
  three rejection codes (`STATE_CORRUPT` / `STATE_VERSION` / `STATE_FOREIGN`).
  Leg **r** (the blob is byte-identical across a tail fork) is unaffected and is
  in fact easier to assert.
- **Only epoch-derived state is ever exported.** Unchanged and load-bearing.

Chunked transfer, because a 17 MB single buffer still costs three copies:

```rust
    /// Serialize into an internal staging buffer; returns total length.
    /// Call only when an apply reported state_changed.
    pub fn export_begin(&mut self) -> Result<u32, JsError>;
    /// Copy frame `i` (≤ 4 MiB) out. Frames are contiguous and in order.
    pub fn export_chunk(&self, i: u32) -> Result<Vec<u8>, JsError>;
    /// Release the staging buffer.
    pub fn export_end(&mut self);

    /// Restore. Frames are fed in order; `load_finish` verifies the trailer
    /// hash and the stamp against `genesis_json`, and NEVER partially loads.
    pub fn load_begin(genesis_json: &str) -> Result<Loader, JsError>;
    // Loader: push_chunk(&[u8]) -> Result<(), JsError>; finish() -> Result<Engine, JsError>
```

TypeScript stores frames as separate IndexedDB records under
`state/folded/<i>`, in one transaction, with `state/folded/meta` carrying
`{frames, len, sha256, stamp}`. Rationale, all browser-real: ≤4 MB records keep
Safari's structured clone responsive; a partial write is detectable (frame count
mismatch) and is simply a cache miss; and nothing ever holds two full copies.

### 2.6 The note registry in `MemStore`

Per-call scratch, exactly as §0.4.1 intends but against the real trait:

- `discover_begin` decrypts the supplied sealed blob (§3.6) and loads its
  `notes[]` into `Inner.notes`;
- `upsert_note` / `set_note_spent` / `delete_note` / `notes` /
  `delete_owner_notes` operate on that map; `refresh_spent` and
  `prune_missing_notes` are the R-A1 default methods and touch nothing
  browser-specific;
- `discover_finish` diffs the map against the decrypted prior set (§0.4.1's
  `added` / `spent` pure diff), re-seals, and **clears the map**;
- `discover_abort` clears it too.

**Nothing key-derived outlives a session.** That is the property that lets §3.5's
state blob stay key-independent and lets IndexedDB hold ciphertext only.

### 2.7 Error additions to §3.7

The closed set gains four codes; nothing is removed and `STATE_STALE` stays
deleted.

| code | details | retryable | raised by |
|---|---|---|---|
| `BOUND_UNSUPPORTED` | `{bound, last_epoch_to}` | no | `MemStore` view/`full_slot_set_as_of` between basis and `last_epoch_to` (§2.2) |
| `DECOMPRESS_UNSTAGED` | `{artifact, zst_sha256}` | no | `StagedTransport::decompress` — the wrapper inflated bytes it did not stage |
| `SESSION_INVALID` | `{handle}` | no | `discover_step`/`finish` on an unknown, finished or aborted handle |
| `SESSION_INCOMPLETE` | `{phase}` | no | `discover_finish` before the last step said `done` |

`plan()`'s `"diverged"` remains a **return value**, and `apply_staged`'s
`need_more` is a **status**, not a throw. Both are control flow the wrapper
switches on; neither is an error a user ever sees.

### 2.8 Purity and size gates — amendments to §3.9

- The import allowlist gains exactly one entry for the monotonic clock used by
  `discover_step`'s deadline. The §3.9 restatement of what the audit proves is
  unchanged and still correct: **the module cannot open a network handle, a
  storage handle, a timer or a randomness source of its own** — a clock read is
  none of those, and it is asserted key-clean like every other import.
- Add the **Pedersen delta gate** of §1.4, run at step 3 with its own FILL-IN.
- The single-denominator rule (gzip of `engine_bg.wasm` + glue + `fzstd`) is
  unchanged; if the split module of §1.4 ships, the denominator is the **sum of
  what a cold snapshot-lane session downloads**, which includes `engine_mpt`,
  so the split cannot be used to make the number look smaller.

---

## 3. Revised §A4 — the npm package

### 3.1 Positioning: our customer holds a key

Verified from the official Wallet API docs: *"No viewing keys in your app. The
wallet holds the user's viewing key"* and *"The wallet discovers notes, builds
the proof"*. A dapp on the Wallet API therefore has nothing for us to do — it
cannot call `getNotes(key)` because it has no key, and it does not need to,
because its wallet already discovered the notes.

**Our customer is the party that holds a viewing key: a wallet, or a
key-holding backend or app built on `@starkware-libs/starknet-privacy-sdk`.**
That audience has exactly one integration point, and it is not `getNotes`:

```ts
const transfers = createPrivateTransfers({ account, viewingKey, discoveryProvider })
```

Today `discoveryProvider` is an `IndexerDiscoveryProvider` pointed at someone's
indexer, with the viewing key travelling to it. Our value proposition, stated in
the package's own terms, is: **swap that one field and the key stops leaving the
browser.**

Consequences for §4.1/§4.2 (D7):

- `LocalDiscoveryProvider` moves from the `/sdk` subpath to the **package root**
  and is the first thing the README shows. It keeps every base §12.1 semantic:
  `discoverNotes` / `discoverChannels` / `discoverRequirement` + `fetchHistory`,
  the `notesCursorToApiCursor` / `apiCursorToNotesCursor` /
  `buildSubchannelCursors` / `convertIncomingNotes` conversions, so
  `NotesCursor` / `ChannelCursor` round-trip identically to
  `IndexerDiscoveryProvider` and a Tier-0 user migrates without resync (§7.4
  cursor interop);
- `KeylessClient` / `DelegatedClient` stay, documented as **the layer
  underneath** — for a wallet that wants notes and subscriptions without the SDK,
  or a backend that wants the delegated split;
- the README opens with a **"Is this for you?"** table, because sending
  Wallet-API dapp authors down this path wastes their day:

  | You are building | Use |
  |---|---|
  | a dapp that asks the user's wallet to act | the Starknet Wallet API. Not this package — you never hold a viewing key. |
  | a wallet, or an app/backend that holds its own viewing key | **this package**, as `discoveryProvider` |
  | a self-hosted deployment where the key may reach your own server | `DelegatedClient`, or `strk20-sync serve` |

- the package name stays unscoped `strk20-discovery` (§4.1 unanimous).

### 3.2 The TypeScript surface

Additions and changes only; everything not shown is §4.2 verbatim.

```ts
// ---------------------------------------------------------------- core types
export interface KeyRef {
  address: `0x${string}`;
  viewingKey: Uint8Array;              // 32-byte BE, Uint8Array ONLY (§4.2 unchanged)
}

export interface Note {
  token: string; index: number; noteId: string; nullifier: string;
  amount: bigint; blockNumber: number; sender: string; spent: boolean;
}

export interface NotesResult {
  notes: Note[]; balances: Map<string, bigint>;
  head: number; l1Accepted: number; complete: boolean;
  historyFrom: number; snapshotRejected: boolean;
  raw: unknown;                        // untouched SyncReport (oracle equality)
  timing: SyncTiming;                  // NEW — see §3.5
  network: NetworkSummary;             // NEW — see §3.5
}

// ------------------------------------------------------------------- timing
export interface SyncTiming {
  totalMs: number;
  phases: {
    open: number;        // IndexedDB open + genesis compare
    plan: number;        // manifest fetch + Engine.plan
    fetch: number;       // wall time inside fetch(), summed
    decompress: number;  // fzstd
    apply: number;       // Engine.apply_staged (verify + fold)
    load: number;        // Engine.load from the folded cache (0 on a cold run)
    export: number;      // Engine.export_* + IndexedDB write (0 when unchanged)
    discover: number;    // discover_begin..finish
  };
  cold: boolean;         // true iff this run applied a snapshot or epoch 0..n from empty
  fromCache: 'folded' | 'raw' | 'none';
}

// ------------------------------------------------------------------ network
export interface RequestRecord {
  url: string;                                    // absolute, exactly as fetched
  method: 'GET';
  status: number;
  bytes: number;                                  // response body bytes over the wire
  ms: number;
  source: 'network' | 'etag-304' | 'idb-cache';
  artifact: 'genesis' | 'manifest' | 'epoch' | 'snapshot' | 'anchor' | 'head' | 'live';
  at: number;                                     // performance.now()
}
export interface NetworkSummary {
  requests: number; bytes: number;
  byArtifact: Record<string, { requests: number; bytes: number }>;
}

// ------------------------------------------------------------------- events
export type DiscoveryEvent =
  | { type: 'notes';    added: Note[]; spent: Note[]; head: number; timing: SyncTiming }
  | { type: 'reorg';    rewoundTo: number }
  | { type: 'status';   state: 'live' | 'polling' | 'degraded' }
  | { type: 'progress'; phase: SyncPhase; done: number; total: number | null }  // NEW
  | { type: 'request';  record: RequestRecord }                                  // NEW
  | { type: 'error';    error: Strk20Error; recovering: boolean };

export type SyncPhase =
  | 'open' | 'plan' | 'fetch' | 'decompress' | 'apply' | 'load' | 'export' | 'discover';

// ------------------------------------------------------------------- client
export interface DiscoveryClient {
  getNotes(k: KeyRef): Promise<NotesResult>;
  subscribe(k: KeyRef, cb: (ev: DiscoveryEvent) => void): () => void;
  history(k: KeyRef, opts?: { fromBlock?: number; limit?: number }): Promise<HistoryPage>;
  status(): ClientStatus;
  network(): { records: readonly RequestRecord[]; summary: NetworkSummary };   // NEW
  resetCache(): Promise<void>;                                                 // NEW
  close(): Promise<void>;
}

export interface ClientStatus {
  mode: 'keyless' | 'delegated';
  transport: 'sse' | 'polling';
  head: number; l1Accepted: number; lastEpoch: number; historyFrom: number;
  verified: 'anchored' | 'server-asserted' | 'replayed';   // §1.5.1
  persistence: 'indexeddb' | 'memory';
  persisted: boolean;        // NEW — navigator.storage.persisted()
  blocking: boolean;         // NEW — true when worker:false (work runs on the caller's thread)
  leader: boolean;           // NEW — this tab owns the SSE connection
  engineBytes: number;       // NEW — wasm linear memory currently held
}

export class KeylessClient implements DiscoveryClient {
  constructor(opts: {
    feedUrl: string;
    network?: 'mainnet' | 'sepolia' | ChainProfile;
    coldStart?: 'auto' | 'snapshot' | 'epochs';        // ONE vocabulary (§4.2)
    persistence?: 'indexeddb' | 'memory' | StorageAdapter;
    persist?: 'raw' | 'folded' | 'both';               // CHANGED — see §3.7
    live?: boolean;
    pollIntervalMs?: number;                           // default 30_000
    worker?: boolean;                                  // default true
    applyBatch?: number;                               // NEW — default 8, clamp 1..64
    stepBudgetMs?: number;                             // NEW — default 50 (worker) / 16 (main)
    anchorRpcUrl?: string;
    requestPersistentStorage?: boolean;
    wasmUrl?: string | URL;
    fetch?: typeof fetch;
    onRequest?: (r: RequestRecord) => void;            // NEW
  });
}
```

`Strk20Error` is unchanged (`code` from the §3.7 closed set, `details`,
`retryable`), now including the four codes of §2.7 plus npm's own `TRANSPORT`
and `CONFIG_INVALID`.

### 3.3 The worker protocol, and why it is not optional

Everything expensive runs in the worker: wasm instantiation, `fzstd`,
`apply_staged`, every `discover_step`, `export_*`. The main thread does
`fetch`, IndexedDB and rendering. Concretely:

- **the key crosses by `ArrayBuffer` transfer** (§4.2 unchanged), detaching the
  caller's buffer so exactly one copy is in flight;
- **`close()` terminates the worker.** This is the only way to return the
  ~80 MB **[est]** of wasm linear memory the epochs lane holds (§1.3); a
  main-thread engine holds it for the life of the page. `status().engineBytes`
  reports it so an integrator can see the cost;
- `worker: false` remains supported for Node and tests. It sets
  `status().blocking = true`, and the README says plainly what that means: a
  cold apply occupies the calling thread for seconds, and a discovery pass for
  ~1.19 s per the measurement. `stepBudgetMs` defaults to 16 there so at least
  the yields are frame-sized, but a `requestAnimationFrame`-driven loop cannot
  make a 3–5 s fold feel fast — it can only keep the page from being killed;
- the `/worker` subpath still ships the recipe as code (§4.2 unchanged).

### 3.4 SSE, proxies, and the six-connection cap (D9)

§A2's framing already handles buffering middleboxes (2 KB `:` padding,
`X-Accel-Buffering: no`, `retry: 15000`, 15 s keepalives) and §2.5 already
degrades to polling on 404/405 with nothing surfaced. Three browser realities
§A4 does not cover:

1. **HTTP/1.1 caps concurrent connections at 6 per origin, and an `EventSource`
   holds one for its lifetime.** Four tabs on the same feed origin leave two
   connections for every fetch the page makes, including the epoch downloads. Six
   tabs deadlock. Mitigation, in the package:

   ```ts
   // one tab holds the stream; the rest hear about pokes over BroadcastChannel
   navigator.locks.request(`strk20:sse:${dbName}`, { mode: 'exclusive' }, async () => {
     leader = true;
     const es = new EventSource(`${feedUrl}/live`);
     es.onmessage = ev => { channel.postMessage(ev.data); handlePoke(ev); };
     await neverResolves(abortSignal);      // hold the lock while we are leader
   });
   ```
   Followers subscribe to the `BroadcastChannel` and run their own verified
   fetch on a poke — identical semantics, one connection. `status().leader`
   reports which tab holds it. When Web Locks are unavailable, every tab opens
   its own stream and the package logs a one-line warning; the fallback poll
   cadence bounds the damage.
2. **Operators should serve the feed over HTTP/2** — then the cap is per-stream
   and the whole issue evaporates. This belongs in the ops docs next to
   §2.7's operator notes, not in client code.
3. **`EventSource` cannot be given headers**, which §4.8 already resolves for
   `DelegatedClient` (fetch + `ReadableStream` + `Authorization`). For the
   keyless indexer stream, native `EventSource` stays, because the endpoint takes
   no auth and no parameters — and that parameterlessness is exactly the privacy
   property (§2.6). Nothing about the leader election changes it: the leader's
   request is byte-identical to any other client's.

### 3.5 Network instrumentation as a shipped feature (D8)

The client wraps the injected `fetch` and records every request. This is not
demo scaffolding — it ships, for three reasons: an integrator needs a real cost
meter; the "no key, no address" claim should be verifiable from the library's
own record rather than from a wrapper the observer must trust; and the
identical-stream property becomes a runtime assertion instead of a
test-time-only one.

```ts
// inside KeylessClient
private async fetchRecorded(url: string, artifact: RequestRecord['artifact'],
                            init?: RequestInit): Promise<Response> {
  const t0 = performance.now();
  const res = await this.fetchImpl(url, init);
  const buf = res.status === 304 ? new ArrayBuffer(0) : await res.arrayBuffer();
  const rec: RequestRecord = {
    url, method: 'GET', status: res.status, bytes: buf.byteLength,
    ms: performance.now() - t0,
    source: res.status === 304 ? 'etag-304' : 'network',
    artifact, at: t0,
  };
  this.records.push(rec); this.opts.onRequest?.(rec); this.emit({type:'request', record: rec});
  return new Response(buf, res);
}
```

Two invariants, asserted in the package's own test suite and re-assertable by a
consumer at runtime:

- **no request URL contains anything key- or address-derived.** Mechanically:
  every URL the client builds comes from a fixed set of templates
  (`/genesis.json`, `/manifest.json`, `/epochs/{e:08}.strk20e.zst`,
  `/snapshots/{e:08}.strk20s.zst`, `/snapshots/{e:08}.anchor.json`,
  `/anchors.ndjson`, `/head.ndjson`, `/live`) whose only variable is an integer
  from the manifest. A test asserts the record set is a subset of that grammar;
- **two `KeylessClient`s with different `KeyRef`s produce identical record
  sequences.** The demo computes this live (§4.7). The existing measurement is
  the ground truth: 609 requests / 64,509 bytes on Sepolia, byte-identical
  between two wallets.

`bytes` is response-body bytes, not wire bytes — headers and TLS are not
visible to `fetch`. The demo says so rather than implying otherwise (§4.5).

### 3.6 IndexedDB: layout amendments and the quirks §4.4 misses (D10)

Layout is §4.4's, with `state` re-shaped for framed chunks:

| store | key | value |
|---|---|---|
| `meta` | string | `format_v`, `last_epoch`, `last_epoch_hash`, `snapshot_e`, persist mode, `genesis` (raw bytes) |
| `artifacts` | `"snapshot"` \| `"anchor"` \| epoch idx | `{hash, zbytes}` — compressed **exactly as served** |
| `state` | `"folded/meta"` \| `"folded/<i>"` | `{frames,len,sha256,stamp}` / `ArrayBuffer` ≤4 MiB |
| `cursors` | `keyId` | `{sealed: ArrayBuffer, updatedAt}` |

`keyId` is the **full 32-byte HKDF output as 64 lowercase hex** (§4.4 as
corrected). Database name per chain and pool. Never stored: `head.ndjson`
bytes, the head ETag, anything tail-derived.

Quirks 1–5 of §4.4 stand. Add:

6. **Safari evicts after 7 days of no interaction** (ITP), unless
   `navigator.storage.persist()` was granted. For a wallet this is the
   difference between a 0.03 s-class warm start and a full cold fold on the
   user's second visit a week later. `requestPersistentStorage: true` should be
   the recommended setting for wallets, `status().persisted` reports the actual
   grant, and the README states that a denied grant on Safari means periodic
   cold starts. There is no way to detect the eviction after the fact — the
   flag that would have recorded it is evicted too — so an empty store is a cold
   start (quirk 3), never an alarm.
7. **Firefox private browsing gives an in-memory IndexedDB**; it opens
   successfully and loses everything on close. Indistinguishable from eviction
   and handled identically.
8. **Structured clone of a large `ArrayBuffer` is a copy on the main thread.**
   ≤4 MiB frames (§2.5), one transaction, and the write happens after `getNotes`
   resolves (§4.4 quirk 5, unchanged).
9. **`onblocked` during an upgrade** with other tabs open: the package does not
   force-close other tabs. It falls back to `persistence: 'memory'` for the
   session and reports it, and a `BroadcastChannel` message asks other tabs to
   release — advisory only.

### 3.7 Persistence: the gate is half-decided already (D6)

§4.6 pre-registers a fold-time gate with three lanes and a decision rule. One
arm of that rule is already answered by measurement, and pretending otherwise
would be theatre:

> §4.6: *"p95 `t_cold(L2)` > 2000 ms → M is enabled for `coldStart:'epochs'`
> sessions regardless of L1's verdict"*.

Native cold fold of the full mainnet history is **5.97 s**, and the browser is
slower on every term (§1.2). L2 exceeds 2000 ms by a factor no measurement
error can close. Therefore:

- **Design M is built.** `export_*` / `load_*` are shipped code, not dormant ABI.
- **`persist` becomes `'raw' | 'folded' | 'both'`, default `'both'`.** `'both'`
  is R with an M cache over it: raw artifacts remain the verifiable truth and
  the folded blob is a cache that is always safe to delete. This is the shape
  §4.5 describes as "Design M — folded-mirror cache over R", now named in the
  option because it is the default rather than a variant. `'raw'` and `'folded'`
  stay available: `'raw'` for a caller who wants no folded blob on disk at all
  (accepting the refold), `'folded'` for a caller who wants minimum bytes stored
  (accepting a full network refetch when the blob is rejected).
- **`CONFIG_INVALID` still applies** to a mode the shipped build does not
  implement, and the published union is still narrowed at publish time (§4.5's
  review-finding-14g resolution, unchanged in mechanism).
- **What remains open is L1** — the snapshot lane, which cannot be measured
  until snapshots exist (roadmap item 1). If p95 `t_cold(L1)` ≤ 500 ms, then on
  the snapshot lane `'raw'` is genuinely sufficient and becomes its default,
  which is a *better* trust posture (§4.5's honest statement about M trusting
  IndexedDB integrity between loads). The FILL-IN stays, scoped to L1:

  > **FILL-IN (fold gate L1, pending snapshots + step 5):** `t_zstd` L1 = ___ ms;
  > p95 `t_cold` L1 = ___ ms; throttled profile = ___; reference device = ___.
  > **Decision for the snapshot lane: raw / both.** Date: ___.

  L2 and L3 remain as trend lines with the 3× regression alarm.

**Cache-invalidation rules, complete and normative:**

| trigger | action |
|---|---|
| `/feed/genesis.json` bytes ≠ stored `meta.genesis` | `CHAIN_MISMATCH` **before any row is written**; nothing is invalidated because nothing is trusted (§4.4, unchanged — the re-fetch is what catches a feed that changes its own genesis) |
| `plan().state == "ok"` | nothing to do; folded blob and artifacts stay |
| `plan().state == "behind"` | fetch and apply epochs `> last_epoch`; rewrite the folded blob **only** when `apply_staged` reports `state_changed` |
| `plan().state == "diverged"` | delete `state` and `artifacts` wholesale; keep `meta.genesis` and `cursors`; cold start |
| `load` throws `STATE_CORRUPT` / `STATE_VERSION` / `STATE_FOREIGN` | delete `state` only; fall through to `artifacts` (R), then to the network. A folded blob is always safe to delete |
| any `FEED_HASH_MISMATCH` on a stored artifact | delete that one artifact row, refetch it once; a second failure is a hard error naming both hashes |
| sealed blob fails AEAD open | treated as **no cursor** (§3.6): fresh discovery, `details.cursor_reset = true` surfaced |
| `resetCache()` | delete `state`, `artifacts`; keep `meta.genesis`; **`cursors` are kept** unless `resetCache({identities:true})` |
| an apply reports `state_changed == false` | **no write.** The epoch cadence is ~4.7 h; a head poke must never rewrite the folded blob (the discussion-§7 hazard) |
| M-tamper mitigation | an opportunistic `requestIdleCallback` full refold + byte-compare every N loads (default N = 20), flagging divergence through `{type:'error', error: STATE_CORRUPT, recovering:true}` |

---

## 4. The demo

### 4.1 What it may claim, and the two things it must not pretend

The brief asks for deposit / send / withdraw buttons. Two facts constrain them:

1. **We have no write path, deliberately** (roadmap; discussion §4). We are the
   read half of every write.
2. **Our customer holds a key.** A demo that "connects a wallet" and then expects
   to discover notes is incoherent: a Wallet-API wallet never hands over the
   viewing key, and it discovers notes itself.

So the demo is honest about being **a wallet-shaped app that holds its own
viewing key**, which is exactly our customer. Its three action buttons do the
half we actually do:

| button | what it really does |
|---|---|
| **Deposit** | shows the shield instruction and starts a **watch**: the log line becomes `waiting for the note…` and mutates until discovery finds a new note, then commits with the elapsed time. The write happens in the user's own wallet (or, in dev mode, via the SDK — see below). |
| **Send** | builds the *spend inputs* from discovered notes — the thing the SDK cannot do without knowing your notes — and shows them; then watches for the nullifier to land and the change note to appear. |
| **Withdraw** | same shape as Send; watches for the nullifier. |

That is not a diminished demo. "The note appeared in 1.2 s and here is the
nullifier landing, and we never saw a key" is the product.

**Key handling in the demo, non-negotiable:**

- the demo accepts a **viewing key** only, pasted or generated locally, held in
  memory, never persisted in plaintext, never logged, never placed in a URL;
- it **never** asks a user for an account private key;
- the optional dev mode that actually submits transactions through the Privacy
  SDK on Sepolia reads a throwaway funded account key from the developer's own
  environment at build time (`VITE_DEMO_SEPOLIA_ACCOUNT`, absent in the
  published build). When absent — the default — the action buttons are in
  **watch mode** and say so. The demo works identically either way; only who
  presses "submit" changes.

### 4.2 Layout

One page, three regions. No routing, no framework requirement beyond whatever
renders text.

```
┌─────────────────────────────────────────────────────────────────────────┐
│ COLD                       │ WARM                    │ FEED             │
│ ─────────────────────────  │ ──────────────────────  │ ──────────────── │
│ total      —               │ total      —            │ ● live (sse)     │
│  fetch     —               │  load      —            │ head   14 340 535│
│  inflate   —               │  apply     —            │ epoch  606       │
│  verify+fold —             │  discover  —            │ verified replayed│
│  discover  —               │                         │ store  ~48 MB    │
│ [ run cold ]               │ [ run warm ]            │ [subscription ⏻] │
├─────────────────────────────────────────────────────────────────────────┤
│ NETWORK — what went to the network                                      │
│ 609 requests · 64 509 B ·  identity A ≡ identity B  ✓                   │
│  GET /feed/genesis.json                       200    412 B    18 ms     │
│  GET /feed/manifest.json                      200  41 208 B    31 ms    │
│  GET /feed/epochs/00000000.strk20e.zst        200   1 204 B     9 ms    │
│  … (scrolls; grouped when > 50)                                          │
│ [ show identity B ]  [ diff A/B ]                                        │
├─────────────────────────────────────────────────────────────────────────┤
│ LOG                                                                      │
│ 14:02:11  open        indexeddb, persisted=false                 12 ms  │
│ 14:02:11  plan        behind: 606 epochs, 16.0 MB                31 ms  │
│ 14:02:17  fold        606 epochs verified and folded            5 812 ms│
│ 14:02:18  discover    1 note, 3.0 STRK, 0 spent                 1 190 ms│
│ 14:02:18  ▸ waiting for the note…                                 4.2 s │
├─────────────────────────────────────────────────────────────────────────┤
│ [ deposit ]  [ send ]  [ withdraw ]        identity: [A ▾]  [ reset ]   │
└─────────────────────────────────────────────────────────────────────────┘
```

Cold and warm are **side by side and always visible** (the orchestrator's
requirement): the contrast is the product's central number and a scrolling log
destroys it. Each column has its own button, and each shows a per-phase
breakdown so the reader can see *where* the six seconds went and *why* the warm
path does not pay it.

### 4.3 Stages and state machine

Stages in the spirit of approve-then-swap: each is enabled only when the one
before it has produced what it needs.

```
        ┌──────────┐  key entered / generated
        │ IDENTITY │───────────────────────────┐
        └──────────┘                           ▼
                                        ┌────────────┐
                          ┌────────────►│   SYNC     │  cold or warm
                          │             └─────┬──────┘
                          │                   │ notes result
                          │             ┌─────▼──────┐
                          │             │  DISCOVER  │  (folded into SYNC's
                          │             └─────┬──────┘   discover phase; shown
                          │                   │          as its own stage line)
                          │             ┌─────▼──────┐
                          │             │    ACT     │  deposit / send / withdraw
                          │             └─────┬──────┘
                          │                   │ submitted (or "watch armed")
                          │             ┌─────▼──────┐
                          └─────────────│  WAITING   │  pending line mutates
                            note found  └────────────┘  until discovery resolves
```

Machine, exactly:

```ts
type Stage =
  | { s: 'idle' }
  | { s: 'identity', id: 'A' | 'B' }
  | { s: 'syncing', kind: 'cold' | 'warm', phase: SyncPhase, startedAt: number }
  | { s: 'ready',   notes: Note[], head: number, timing: SyncTiming }
  | { s: 'acting',  action: 'deposit' | 'send' | 'withdraw' }
  | { s: 'waiting', action: 'deposit' | 'send' | 'withdraw',
                    armedAt: number, baseline: { noteIds: Set<string>, spent: Set<string> } }
  | { s: 'error',   error: Strk20Error };
```

Transitions worth pinning:

- `syncing → ready` on `getNotes` resolving; `phase` is driven by the
  `{type:'progress'}` events, which is why §3.2 adds them.
- `ready → waiting` **arms a baseline**: the exact set of note ids and spent
  nullifiers at the moment of arming. Resolution is "a note id appears that is
  not in the baseline" (deposit / change note) or "a nullifier appears in
  `spent` that was not in the baseline" (send / withdraw). Comparing against a
  captured baseline rather than against "count went up" is what makes the
  elapsed number mean something.
- `waiting` resolves on the **first discovery pass that sees the change**,
  whether that pass was triggered by SSE or by "check now". The log line records
  which (`via sse` / `via manual` / `via poll`), because the two are different
  latencies and merging them would flatter the subscription.
- `waiting` has no timeout; it has a **cancel**. A timeout would produce a
  number that looks like a measurement and is not one.

### 4.4 The log

One line per event, appended. The last line mutates in place while pending and
commits when it resolves, carrying its elapsed time — the brief's shape.

```
<hh:mm:ss>  <event>  <detail>  <right-aligned duration>
```

| event | detail | duration is |
|---|---|---|
| `open` | `indexeddb, persisted=<bool>` or `memory (<reason>)` | `timing.phases.open` |
| `plan` | `ok` / `behind: N epochs, X MB` / `diverged: dropping cache` | `timing.phases.plan` |
| `snapshot` | `applied at block B, S slots, verified=<grade>` | apply time for the snapshot |
| `fold` | `N epochs verified and folded` | `fetch+decompress+apply` |
| `load` | `folded cache hit, N frames, X MB` | `timing.phases.load` |
| `discover` | `N notes, X TOKEN, M spent` | `timing.phases.discover` |
| `export` | `folded cache written, N frames, X MB` | `timing.phases.export` |
| `head` | `→ B (l1 B′)` | — |
| `epoch` | `cut e=N` | — |
| `reorg` | `tail replaced, rewound to B` | — |
| `▸ waiting for the note…` | mutates: `…`, then `note 0xce52… 3.0 STRK` | now − `armedAt` |
| `▸ waiting for the spend…` | mutates: `nullifier 0x6f37… landed` | now − `armedAt` |
| `check` | `manual` / `via sse` / `via poll` — *no change* or *N new* | full pass time |
| `error` | `<code>: <message>` | — |

The pending line ticks at 10 Hz off `requestAnimationFrame`, and its committed
duration is computed from the two `performance.now()` stamps, not from the tick
counter.

The subscription toggle sits in the FEED panel: **on** ⇒ discovery runs on every
`head`/`epoch` poke and each run appends a `check … via sse` line; **off** ⇒ the
`check now` button appears and appends `check … manual`. Either way the elapsed
time is logged, which is the brief's requirement, and the two are labelled
differently so nobody reads a manual number as a subscription number.

### 4.5 How each number is obtained — the honesty rules

**Clock.** `performance.now()` only, captured inside the package (`SyncTiming`)
and by the demo around the package call. The demo shows the package's own
numbers; where the demo measures (the waiting lines), it says so.

**What is inside each phase.**

| number | includes | excludes |
|---|---|---|
| cold `total` | everything from `getNotes()` call to resolve, on a store the demo just cleared | wasm instantiation on the very first load (reported separately as `boot`), user think time |
| cold `fetch` | wall time inside `fetch()`, summed, for the whole plan | queueing behind other tabs' connections (visible as inflated `fetch`, and that is honest) |
| cold `inflate` | `fzstd` only | the sha256 that precedes it (counted in `verify+fold`) |
| cold `verify+fold` | `Engine.apply_staged` across every batch | anything the wrapper did between batches |
| `discover` | `discover_begin` … `discover_finish` | the IndexedDB write of the sealed blob (counted in `export`) |
| warm `load` | `load_begin` … `finish`, including reading frames from IndexedDB | — |
| time-to-discover | `armedAt` → the resolving pass's completion | the block time of the chain itself, which the demo shows separately as `note block B, head was H at arm time` |

**Rules, enforced by review of the demo source:**

1. **No number is ever displayed that was not produced in this session.** The
   cold column starts empty and reads `not measured in this session` until a
   cold run happens. The measured 5.97 s from the live findings appears **only**
   in the demo's About panel, cited, labelled *native CLI, mainnet, 2026-08-31*,
   and never in the live columns.
2. **A cold run is really cold.** `run cold` calls `client.resetCache()`,
   `close()` (terminating the worker, freeing wasm memory — §3.3), and
   constructs a fresh client. Anything less measures a partially warm path. The
   HTTP cache is *not* cleared, because a page cannot clear it; the demo
   therefore appends `(browser http cache may serve some artifacts)` to the cold
   line, and the NETWORK panel's `source` column shows what actually came from
   the network. Understating this would be the easiest lie in the whole demo.
3. **Warm is the run immediately after cold**, same session, no reload — and a
   second button `run warm (after reload)` exercises the real returning-user
   path, which is the one that matters and the one that Safari's ITP can break.
4. **Bytes are response-body bytes** (`fetch` cannot see headers or TLS). The
   panel footnotes it.
5. **No projections in the live panels.** "518 requests today → ~5 with
   snapshots" belongs in the About panel with its arithmetic (§1.1), not in a
   number that looks measured. If the feed *does* publish a snapshot,
   `plan()` says so and the panel shows the real snapshot-lane request count —
   measured, not projected.
6. **Failures are logged, not swallowed.** A `FEED_HASH_MISMATCH` or a
   `verified: server-asserted` grade appears in the log in the same typeface as
   a success. The §1.5.1 grade is displayed in the FEED panel at all times.

### 4.6 Cold vs warm, side by side

Both columns show the same phase rows so the reader can see subtraction happen:
cold has `fetch / inflate / verify+fold / discover`; warm has `load / apply /
discover` with `fetch` reduced to the conditional head GET. The visual point is
that warm's `fetch`, `inflate` and `verify+fold` rows are **absent, not small**
— they are struck through with the byte count they did not download.

Under both columns, one line of provenance: `mainnet · 515 epochs · 16 MB feed`
or `sepolia · 606 epochs · 64.5 kB`, read from the manifest at run time.

### 4.7 The network panel and the second identity

Populated purely from `client.network()` (§3.5). Columns: method, URL, status,
bytes, ms, source. Grouped after 50 rows (`epochs 000000–000605  606 requests
· 61 902 B`) with a disclosure that expands to every row, because the claim is
"you can read every URL" and a truncated list is not that claim.

Above it, one line, computed live:

```
609 requests · 64 509 B ·  identity A ≡ identity B  ✓
```

The second-identity toggle:

1. constructs a second `KeylessClient` against the same feed with an unrelated
   address and viewing key, in a **separate IndexedDB database name suffix** so
   its cold path is genuinely cold;
2. runs the same sync;
3. compares the two record sequences:
   - **`(method, url)` sequences must be exactly equal, in order** — this is the
     strict claim and it is what turns the test-asserted property into something
     checkable by eye;
   - **byte totals are reported separately**, because a `304` on one run and a
     `200` on the other (a head cut between the two runs) legitimately changes
     bytes without changing the URL sequence. The panel says
     `bytes differ: identity B saw a head cut mid-run` rather than showing a red
     ✗, which would be a false alarm and would teach viewers to ignore the
     indicator;
4. any URL-sequence difference is a **red ✗ with the diff shown**. There is no
   presentation in which a mismatch is hidden.

Below it, a scanner line the viewer can act on: `search these URLs for your key
[paste]` runs the same 13-encoding search the recording proxy ran (minimal hex,
padded, decimal, upper/lower, 0x-prefixed, raw BE/LE) over the URL list, in the
page, and reports `not found in any of 13 encodings` — plus a **self-test**
button that plants the key in a synthetic URL and shows the scanner finding it,
because a detector that never fires proves nothing. This mirrors
live-run-findings §5 exactly, which is the point: the demo reproduces the
recorded experiment rather than asserting its conclusion.

### 4.8 Configuration and reproducibility

- Network selector: **Sepolia** by default, because our own note (block
  14,339,115, 3 STRK, discovered keylessly in 1.19 s) and our own spend (block
  14,340,785) live there and can be re-discovered on demand; mainnet selectable
  for the volume numbers.
- Identity A defaults to the address whose note is on Sepolia, with its viewing
  key entered by the operator — never checked into the repo, never in the URL.
  Identity B is a fixed unrelated address with a randomly generated key, checked
  in, because it holds nothing.
- Every run appends a JSON line to an in-page export (`download run log`)
  carrying the full `SyncTiming`, the `NetworkSummary`, the manifest hash, the
  chain id, the user agent and whether CPU throttling was detected. This is what
  makes a demo number reproducible instead of anecdotal, and it is the same
  record the §4.6 bench harness consumes.
- The demo's About panel carries the citation list: every quoted historical
  number with its source line in `live-run-findings.md`.

### 4.9 What the demo must never do

A checklist, because these are the failure modes that turn a real demo into a
staged one:

- never display a constant where a measurement belongs;
- never reuse a previous session's timing after a reload without labelling it;
- never resolve a `waiting` line on a timer, a poll count, or anything other
  than an actual discovery result diffed against the armed baseline;
- never hide a request from the network panel — including the SSE connection,
  which appears as `GET /feed/live  (open)` with a live byte counter;
- never show a "with snapshots" number as if measured;
- never let identity B share identity A's IndexedDB, which would make its "cold"
  run warm and its request list short;
- never send the viewing key anywhere, including to the demo's own analytics.
  The demo has no analytics.

---

## 5. Acceptance legs this design adds

To §8's list, in the same style, all runnable:

| leg | asserts |
|---|---|
| **α** | `MemStore` + `FeedStore` conformance over the R-A1 default methods: identical `refresh_spent` / `prune_missing_notes` results over the same fixture, including the live-observed case *a spent note's slot is not cleared* (live-run-findings §7) |
| **β** | `plan()` is a pure function of (state, manifest): same inputs ⇒ byte-identical plan JSON, for two different owners — the mechanical form of the identical-request-stream property, one layer below the wire capture |
| **γ** | `apply_staged` batching equivalence: applying 606 epochs in batches of 1, 8 and 64 yields byte-identical `export` blobs and identical `SyncReport`s |
| **δ** | `discover_step` slicing equivalence: a session stepped at 16 ms slices produces the same `DiscoverOut` as one stepped with an infinite deadline, and no step exceeds its deadline by more than one pass |
| **ε** | state blob v2 round-trip + the §3.5 bounds as array checks, plus leg **r** unchanged (blob byte-identical across a tail fork) |
| **ζ** | peak wasm linear memory on the L2 fixture stays under a recorded budget; the arena layout is asserted by that budget, not by inspection |
| **η** | leader election: N simulated tabs open exactly one `EventSource`; killing the leader promotes another within one lock timeout |
| **θ** | the demo's own e2e: run cold, run warm, arm a waiting line against a synthetic note appearing in the fixture feed, and assert the committed elapsed time equals the fixture's injection delay within tolerance — i.e. *the demo's clock is measuring the thing it claims* |

Leg **q**'s wrapper scanner (no cached `entropy32`, every call site a fresh
`crypto.getRandomValues`) extends to the demo source unchanged, and
`capture-scan` (§4.9) remains the single scanner implementation over the TS
proxy capture, the IndexedDB dump and now the demo's exported run log.

---

## 6. Open items

1. **Pedersen table cost** (§1.4) — measured at step 3, before any npm code.
   Decides whether the module splits.
2. **L1 fold time** (§3.7) — cannot be measured until snapshots exist. Decides
   `persist` default on the snapshot lane only; `both` is the default meanwhile.
3. **Snapshot payload size** — estimated at ~15 MB payload / ~4 MB compressed
   **[est]** from 134,879 slots. If it lands materially above that, the
   ~50 MB PIR trigger in the roadmap gets closer and the number should be
   re-checked against `docs/research/q9-pir.md`.
4. **`entry.zst` on the epoch path** (§2.3) — a three-line native fix that
   removes a real asymmetry between the two hosts. Belongs to whoever owns
   `crates/consumer/src/apply.rs` next.
5. **Whether `full_slot_set_as_of` is ever called between the snapshot basis and
   `last_epoch_to`** by the §11.3 reachability check. If it is, `Base` needs a
   bounded write log above the basis and §2.2's table gains a row; the
   `BOUND_UNSUPPORTED` error exists so this shows up as a loud failure in a test
   rather than a wrong root.
