# Consumer-path addendum, proposal P3 — the DX lens

Status: council proposal. Extends `docs/spec/architecture.md` (v1); every place it
amends the base spec is marked **AMENDMENT** with the old text quoted. The
roadmap's decisions (`docs/roadmap.md`) are taken as given: two blocks with the
`FeedTransport` seam, WASM as a pure synchronous computer, keyless+delegated dual
API, no write path, deferred items deferred. Nothing below re-litigates them.

Optimization target of this proposal: the npm API a wallet engineer actually
wants, browser realities (bundle size, IndexedDB quirks, SSE through proxies),
and time-to-first-note in a web app. Every design choice is argued from merit;
no deadline reasoning appears anywhere below.

The one-sentence pitch this addendum has to make true:

```ts
import { KeylessClient } from 'strk20-discovery';

const client = new KeylessClient({ feedUrl: 'https://feed.example.org/feed' });
const { notes, balances } = await client.getNotes({ address, viewingKey });
const stop = client.subscribe({ address, viewingKey }, ev => {
  if (ev.type === 'notes') render(ev.added, ev.spent);
});
```

— cold start in seconds, updates in ~1 s of chain head movement, the viewing key
never leaving the tab, and the request stream provably identical to every other
user's.

---

## A1 — Snapshots in the cutter, storage-root anchor verified client-side

### A1.1 What a snapshot is

A snapshot is the **folded state** of the mirror at an epoch boundary: the full
pool slot set with per-slot write blocks, content-addressed and deterministic,
so a cold-start client loads one file instead of replaying every epoch. It is
cut by the server's cutter, verified client-side by recomputing the pedersen-MPT
storage root with the same `feed::mpt` module the server and the U6 verifier
already share (spike fact: `strk20-feed` + `mpt` builds for wasm32).

**Boundary alignment (settled): snapshots exist ONLY at epoch boundaries.** The
snapshot for epoch `E` captures state as of block `to(E) = (E+1)*epoch_size − 1`,
and records the content hash of epoch `E` so the epoch hash chain continues from
it seamlessly. No mid-epoch snapshots, ever — that would create a second
alignment concept and a second trust seam for zero benefit.

### A1.2 What a snapshot carries, and why events are absent — explicit resolution

The base spec's three consumers of mirrored data:

| Consumer | Needs | Snapshot answer |
|---|---|---|
| `discovery-core` engine (`sync_incoming_state` / `sync_outgoing_state` / `preflight_check`) | `RawStorageAccess` only: slot values + `last_update_block` (blanket `IViews` is over `RawStorageAccess`; verified in `crates/client/src/store.rs` — discovery never calls `get_events`) | **Fully served.** Snapshot carries `(slot, value, write_block)` triples. Note-creation-block metadata and the 10-block maturity rule survive because `write_block` is per-slot in the snapshot, not reconstructed. |
| Spent-state machine (`FeedStore::refresh_spent`) | nullifier **slot** values (`nullifiers(n) != 0`); `NoteUsed` events are an accelerator, not the source of truth (the shipped code reads slots) | **Fully served.** A note spent pre-snapshot has its nullifier slot ≠ 0 in the snapshot. |
| `history::fetch_transactions` (compat `/v1/history`, future serve/npm history) | `RawEventAccess` over the full block range | **Explicitly partial.** Events exist only for blocks after the snapshot. This is surfaced, never hidden — see below. |

**Resolution: snapshots carry slots only** (with write blocks). Including events
would make the snapshot grow linearly with history — exactly the property the
snapshot exists to escape — while buying nothing for discovery or spent-state.
Pool slots are write-once in practice, so the slot set is already the compressed
form of the entire diff history; the savings over epochs are the events, block
framing, and NDJSON overhead, and — decisive for the browser — **the fold
itself**: a snapshot-seeded client performs zero folding for the covered range.

The partiality is made honest by a **history floor**:

- The client stores `history_floor = snapshot_block + 1` in meta (Rust:
  `sync.db` meta row; WASM: state-blob header field).
- `RawEventAccess::get_events` clamps `from_block` to the floor.
- Every history-shaped API response carries the floor:
  `{"history_from": <block>, "transactions": [...], ...}` — compat `/v1/history`
  responses gain the field (additive, reference clients ignore unknown fields),
  serve and npm surfaces carry it natively.
- npm: `client.history()` returns `{ completeFrom: number, transactions: [...] }`
  and the TSDoc says exactly why.

A wallet that needs full transaction history from genesis syncs from epochs
(one flag / one option away); a wallet that needs notes and balances — the
overwhelming default — cold-starts O(1).

As-of reads below the snapshot block are undefined on a snapshot-seeded mirror
(intermediate values of a twice-written slot are not represented). The client
never issues them: every engine bound is ≥ `last_epoch_to` ≥ snapshot block.
Documented as a mirror invariant.

### A1.3 Snapshot wire format v1 (byte-precise, frozen)

File `feed/snapshots/{e:08}.strk20s.zst` = zstd-19 over a canonical NDJSON
payload. Content identity = sha256 over the **uncompressed** payload, exactly
like epochs (`zst` hash is transport-only). Same canonical JSON rules as spec
§4.3: fixed field order, no whitespace, minimal lowercase hex, `\n` after every
line including the last.

```
line 1 (header):
{"t":"hdr","v":1,"kind":"strk20-snapshot","chain_id":"SN_MAIN","pool":"0x…","epoch":1405,"block":14059999,"epoch_hash":"<64-hex content hash of epoch 1405's payload>","storage_root":"0x…","class":"0x<pool class as of block>"}

one line per slot, ascending by the 32-byte BE slot bytes:
{"t":"slot","s":"0x<slot>","v":"0x<value>","w":<last write block ≤ block>}
  - slots with value 0 and no write are ABSENT (Cairo map semantics; zero-default on read)

last line (footer):
{"t":"end","slots":<n_slot_lines>}
```

`storage_root` in the header is the pedersen-MPT root **the payload must hash
to** — it is part of the content-addressed bytes, so an operator cannot serve a
slot set and a root that disagree without every verifying client noticing
(§A1.5). Determinism: the payload is a pure function of the DB slot set as of
`block` → byte-identical across operators (acceptance leg n).

Size today: at the current mainnet scale (tens of thousands of slots; the Q9
PIR trigger is ~8×10⁵ records ≈ 50 MB), a slot line is ~110 bytes raw → a
10⁴-slot snapshot ≈ 1.1 MB raw ≈ ~300 KB zstd. The snapshot stays well under
the full feed and, unlike the feed, does not grow with event volume.

### A1.4 Cutter changes and manifest schema

Cutter (`crates/indexerd/src/cutter.rs`) gains, inside `cut_ready_epochs` after
the last epoch of a batch is cut:

```rust
/// Cut a snapshot at epoch `e`'s end block if the cadence says so.
/// Requires a verify_root success AT EXACTLY to(e) — the snapshot anchor is
/// not best-effort like epoch anchors: no proof, no snapshot (the file's
/// storage_root would be unverifiable at birth).
pub async fn cut_snapshot(&self, e: u64) -> Result<Option<SnapshotMeta>>;
```

- Cadence: `snapshot_every` epochs (chain-profile field; mainnet default 25 ≈
  250k blocks). Retention: keep the newest `snapshot_keep = 2`, delete older
  files, prune manifest entries.
- The anchor for the snapshot is fetched at cut time at exactly `to(e)` (inside
  the getStorageProof window by construction — we are at the cut). On proof
  failure the snapshot is skipped and retried at the next cadence point; the
  feed is never blocked on it.
- `strk20 snapshot create [--epoch <e>]` runs the same code path manually;
  `strk20 snapshot import` is **removed** (superseded — mirrors regenerate, see
  A1.6).

**AMENDMENT — spec §4.2.** Old text:

> `snapshots/latest.sqlite.zst       # optional convenience; epochs are canon`

New text:

> ```
> snapshots/{e:08}.strk20s.zst      # folded slot set at epoch e's end block;
>                                   # content-addressed, deterministic, wasm-parseable
> ```
> Epochs remain canon; a snapshot is a verified shortcut, never a second source
> of truth. The SQLite snapshot is dropped: it was unusable by the browser
> client and unverifiable without SQLite-aware tooling; the NDJSON snapshot is
> one codec family, one verifier, three consumers (server cutter, Rust client,
> WASM module).

**AMENDMENT — spec §4.4 manifest.** Old field:

> `"snapshot":{"block":14049912,"sha256":"<64-hex>","bytes":123456}}`

New field (plural; `snapshot` stays `null` forever for old readers):

```json
"snapshots":[
  {"e":1405,"block":14059999,"hash":"<64-hex sha256 of uncompressed payload>",
   "zst":"<64-hex>","bytes":301234,
   "epoch_hash":"<64-hex, = epochs[e].hash>",
   "storage_root":"0x…","class":"0x…",
   "anchor":{"block":14059999,"block_hash":"0x…","storage_root":"0x…","class":"0x…"}}
]
```

`anchor` here is never null (A1.4 rule). The full `getStorageProof` response is
stored as the existing epoch-anchor sidecar `epochs/{e:08}.anchor.json` for that
epoch — no new sidecar kind.

### A1.5 Client-side verification — the trust story, layer by layer

A cold-start client (Rust and browser identically; the checks live in
`strk20-feed` + `client-core`, compiled to both targets):

1. **Transport integrity**: sha256(uncompressed payload) == `snapshots[i].hash`.
2. **Internal consistency**: parse; slots ascending; footer count; header
   `chain_id`/`pool` match genesis; `block == to(e)`.
3. **State integrity (the anchor check)**: recompute the pedersen-MPT storage
   root over the slot set with `feed::mpt::storage_root` and require equality
   with the header's `storage_root` AND with `snapshots[i].anchor.storage_root`
   (the value the chain proved to the operator at cut time). A paranoid client
   (U6) additionally fetches `starknet_getStorageProof` for `block` from its
   OWN RPC and checks the same root — the server stays out of the trust path.
4. **Chain continuation**: seed the epoch hash chain with
   (`last_epoch_applied = e`, `last_epoch_hash = header.epoch_hash`); the
   existing `FeedStore::apply_feed` divergence check (`store.rs` — "manifest's
   entry for our last applied epoch must carry OUR hash") then runs verbatim
   for epochs `e+1…`. A feed whose epoch `e` hash disagrees with the snapshot's
   `epoch_hash` is rejected loudly before any state lands.

What this buys: the snapshot bypasses replaying the hash chain, and the MPT
root — an on-chain commitment — replaces it as the integrity anchor for the
covered range. An operator who serves a snapshot omitting one note slot
produces a root mismatch on every verifying client (leg m).

Cost honesty: the root recompute is O(N) pedersen hashes. In wasm this is the
one potentially-noticeable cold-start cost (~2N pedersen ops; measured by the
same bench harness as the fold gate, §A4.6). The npm client runs it before
`getNotes` resolves (`verify: 'block'`, the default — the snapshot has no other
integrity); `verify: 'background'` exists for apps that prefer showing unverified
balances a second earlier and reacting to a `verification-failed` event.

### A1.6 Mirror-pull interplay

Snapshots are a pure function of the epoch set, so mirrors **regenerate rather
than copy**: `strk20 mirror pull` ingests verified epochs as today, then the
mirror's own cutter cuts snapshots at its own cadence — byte-identical to the
origin's when cadences align (leg n asserts it). If the origin's manifest lists
a snapshot the mirror also has, `strk20 epoch verify --all` extends to compare
snapshot hashes; divergence = the same loud fork signal as epoch divergence.
Plain-file-copy mirrors serve whatever snapshot files they copied; the client
verifies regardless (A1.5 does not trust the mirror).

`strk20 mirror pull` itself gains a fast path: seed from the origin's newest
snapshot (verified per A1.5 §1–3, root check with the puller's own RPC when
`--rpc-url` is present), then epochs `e+1…`. This turns mirror bootstrap O(1)
too, with the same trust posture as the client.

### A1.7 Client flows

Rust `strk20-sync` (`FeedStore` grows `apply_snapshot`):

```rust
impl FeedStore {
    /// Seed an EMPTY mirror from a verified snapshot. Errors if any epoch or
    /// tail rows exist (snapshots never overwrite a live mirror).
    pub fn apply_snapshot(&self, payload: &[u8], entry: &ManifestSnapshot) -> Result<()>;
}
```

Seeding writes each slot line as a `storage_log(slot, block = w, value)` row —
as-of reads at bounds ≥ snapshot block are exact, `write_block` is native, and
`ClientView` needs zero changes. Meta rows written: `last_epoch_applied`,
`last_epoch_hash`, `last_epoch_to`, `history_floor`. `apply_feed` picks snapshot
seeding automatically when the mirror is empty and the manifest lists snapshots
(opt out: `strk20-sync sync --from-genesis`).

Browser: the WASM module owns `load_snapshot` (§A3); the TS wrapper only moves
bytes.

### A1.8 Acceptance criteria

Legs l, m, n in §H below. Headline assertions: cold-start request multiset =
{genesis, manifest, snapshot, epochs > e, head} with **no** epoch ≤ e fetched;
discovery output field-identical to O1/O2 including per-note `block_number` ==
committed partition block (proves write_block survives the snapshot); root
mismatch on a tampered snapshot is caught client-side with a named error; two
independent backfills cut byte-identical snapshots.

---

## A2 — SSE on the indexer

### A2.1 Posture

**AMENDMENT — resolution R3 / §12.2.** Old text (R3):

> v1 tail = `head.ndjson` refetched wholesale on ETag change; roadmap SSE is a
> single GLOBAL stream only, never per-user (§12.2).

New text: the roadmap now schedules the SSE tail (roadmap item 2). Everything
R3 preserved survives intact: **one global stream, identical bytes for every
subscriber, never per-user, never per-address**; polling remains a first-class
path that every client can fall back to (and that static-file mirrors — which
have no SSE at all — serve exclusively). The SSE stream is a **notification
plane**: it tells clients *that* and *what kind of* change happened; the
payload always comes from the content-addressed files. "New-head diffs" on the
stream are structured summaries (which blocks appended, whether the tail was
rewritten), not payload bytes — keeping the single trust path (hash chain +
ETag'd files) and keeping the identical-stream privacy test byte-exact.

### A2.2 Endpoint and framing

`GET /feed/live` → `text/event-stream`. No query parameters — **any** query
string is rejected with 400 `INVALID_QUERY` (stronger than ignoring: the
address-blindness test gets a hard guarantee). The only client-supplied input
is the standard `Last-Event-ID` header, which — like `If-None-Match` on
`head.ndjson` — is a function of feed progress only, permitted by dataflow
invariant §2.3 ("a function of download progress only").

Response headers: `Cache-Control: no-cache`, `X-Accel-Buffering: no`,
`Content-Type: text/event-stream`. On connect the server sends a 2 KB `:`
padding comment (defeats buffering middleboxes), then `retry: 3000`, then a
`hello` event. A `: ping` comment goes out every 15 s.

Event grammar (every `data:` is one line of canonical JSON):

```
event: hello
id: <boot_id>-<seq>
data: {"chain_id":"SN_MAIN","pool":"0x…","head":14056430,"head_hash":"0x…","l1_accepted":14049912,"latest_epoch":1405,"head_etag":"\"<64hex>\"","decode_state":"ok"}

event: head
id: <boot_id>-<seq>
data: {"head":14056431,"head_hash":"0x…","l1_accepted":14049930,"tail_from":8980000,"etag":"\"<64hex of new head.ndjson>\"","change":"append","appended":[14056431]}
  - change: "append" (pure extension; appended = new pool-active block numbers, may be [])
          | "rewrite" (reorg or epoch-cut trimmed the tail; client refetches head.ndjson)

event: epoch
id: <boot_id>-<seq>
data: <the exact ManifestEpoch JSON object for the newly cut epoch>

event: snapshot
id: <boot_id>-<seq>
data: <the exact snapshots[] entry JSON object>

event: status
id: <boot_id>-<seq>
data: {"decode_state":"ok"|"degraded","verify_root_failed":false}
```

`boot_id` = 6 random hex chars per server process start; `seq` = monotonically
increasing u64. Together they make resume unambiguous across server restarts.

### A2.3 Resume, fallback, proxies

- **Resume**: the server keeps a ring of the last 256 events. A reconnect with
  a known `Last-Event-ID` replays the suffix; an unknown or foreign-boot ID
  gets a fresh `hello`. Resume is therefore *best-effort by design*: the client
  treats `hello` as "reconcile via manifest + head ETag" — the exact polling
  code path. Missed events are never a correctness problem, only a latency one.
- **Fallback to polling**: unchanged v1 behavior (`manifest.json` max-age=30,
  `head.ndjson` ETag). The npm client runs a watchdog: no event or ping for
  45 s → close, poll once, reconnect with backoff. A 404/405 on `/feed/live`
  (static mirror, old server) permanently degrades that session to polling —
  **feedUrl pointed at a dumb static host is a fully supported deployment**.
- **Proxy friendliness**: SSE is plain HTTP GET — it traverses CDNs and
  corporate proxies that WebSockets fail; h2 multiplexes it beside file fetches.
  Operator docs: route `/feed/live` uncached to origin; identity encoding
  (events are tiny); idle timeout > 60 s or rely on the 15 s pings.
- **Privacy invariant, mechanically**: one broadcast channel feeds every
  connection; per-connection state is only the attach/replay offset. Acceptance
  leg o captures two concurrent subscribers through the recording proxy and
  asserts byte-identical streams modulo attach point, and runs the leg-d
  scanner over the SSE capture.

### A2.4 Server implementation sketch

Ingest loop publishes to a `tokio::sync::broadcast` after: tail regen (`head`),
epoch cut (`epoch`), snapshot cut (`snapshot`), decode-state flips (`status`).
Axum `Sse` responder per connection; ring buffer under a small mutex. The
`appended` list is computed by the cutter's tail regen (it knows old vs new
tail contents). No flag: SSE is always on in `strk20 run` (it serves no bytes
that files don't; a mirror without it is the fallback story, not a config).

**AMENDMENT — spec §6.1 table.** Add rows:

> | `GET /feed/live` | global SSE notification stream (§A2) | `no-cache`, uncacheable |
> | `GET /feed/snapshots/{e:08}.strk20s.zst` | snapshot file, `X-Content-Sha256-Raw` | immutable |

---

## A3 — the WASM package of Block B

### A3.1 Crate split

```
crates/client-core   lib strk20-client-core   — wasm-portable Block B logic
crates/client-wasm   cdylib strk20-engine     — wasm-bindgen ABI over client-core
```

`client-core` is extracted from `crates/client`: the cursor reopen logic
(`reopen_cursor`), note registration, checkpoint/live two-pass orchestration,
spent-state — everything in `sync.rs` that is not SQLite or tokio — plus a new
in-memory mirror. `crates/client` becomes a consumer of `client-core` (SQLite
store + CLI stay put; behavior of `strk20-sync` is unchanged — the conformance
suite pins it). Dependency facts from the spike, honored: `client-core` deps =
`strk20-feed` (features `mpt`, **not** `compress`), `discovery-core`
(`default-features = false` — the feature-gated fork, §A3.6), `serde`,
`chacha20poly1305` (§A3.5), no tokio, no rusqlite, no zstd.

**Async without an executor**: `discovery-core`'s entry points are `async`, but
over the in-memory view every future resolves without ever returning
`Pending`. `client-core` drives them with a no-op-waker poll loop
(`fn block_on_ready<F: Future>(f: F) -> F::Output` that panics on `Pending` —
the panic is a programming-error tripwire, not a runtime path). This is what
"WASM is a pure synchronous computer" compiles down to. `Send` bounds are
satisfied naturally (the mem view is `Send`); `SendWrapper` stays available as
the §7.6 escape hatch if a future upstream rev grows non-Send internals.

### A3.2 The in-memory view replacing `ClientView`

```rust
/// Epoch-derived base state + tail overlay. Two bounds, zero SQL.
pub struct MemStore {
    genesis: Genesis,
    base: BTreeMap<[u8; 32], SlotRec>,      // folded from snapshot + epochs
    base_events: Vec<EventRec>,             // blocks > history_floor, ≤ last_epoch_to
    tail: BTreeMap<[u8; 32], SlotRec>,      // folded from head.ndjson; rebuilt wholesale
    tail_events: Vec<EventRec>,
    tail_blocks: Vec<(u64, Felt)>,          // for contradiction detection
    chain: ChainCursor,                     // last_epoch, last_epoch_hash, last_epoch_to,
                                            // history_floor, head, l1_accepted, tail_generation
}
struct SlotRec { value: Felt, write_block: u64 }

pub struct MemView<'a> { store: &'a MemStore, bound: u64 }  // impl RawStorageAccess + RawEventAccess
```

Reads at `bound ≤ last_epoch_to` use `base` only; reads at head use
tail-then-base. `apply_head` replaces the tail wholesale and bumps
`tail_generation` on contradiction — a line-for-line port of the shipped
`FeedStore` reorg discipline (`store.rs`), including the mid-sync
`tail_from > last_epoch_to + 1` gap bail. The browser client persists nothing
from the tail (notes §7), so this generation only rewinds in-memory cursors —
the "no reorg logic at all" property for persisted state holds.

### A3.3 Exported ABI

wasm-bindgen, `--target web` build (+ a `nodejs` build for tests). All fallible
methods throw a `JsError` whose message is canonical JSON
`{"code":"<SCREAMING_SNAKE>","message":"…","details":{…}}` (§A3.7).

```rust
#[wasm_bindgen]
pub struct Engine { /* MemStore + per-owner cursor cache */ }

#[wasm_bindgen]
impl Engine {
    /// Fresh state for a chain. genesis_json = the fetched /feed/genesis.json.
    #[wasm_bindgen(constructor)]
    pub fn new(genesis_json: &str) -> Result<Engine, JsError>;

    /// Restore from a persisted state blob (§A3.4). Verifies trailer + stamp.
    pub fn load(blob: &[u8], genesis_json: &str) -> Result<Engine, JsError>;

    /// {"chain_id","pool","last_epoch","last_epoch_hash","last_epoch_to",
    ///  "history_floor","head","l1_accepted","slots","engine_version"}
    pub fn info(&self) -> String;

    /// Compare persisted chain position against a freshly fetched manifest.
    /// "ok" (blob current) | "behind" (apply epochs last_epoch+1..) |
    /// "diverged" (manifest hash for last_epoch != ours: discard blob, cold-start).
    pub fn check_manifest(&self, manifest_json: &str) -> Result<String, JsError>;

    /// Uncompressed snapshot payload + its manifest snapshots[] entry.
    /// Runs A1.5 checks 1–4 INSIDE wasm (incl. the MPT root). Empty-state only.
    pub fn load_snapshot(&mut self, payload: &[u8], entry_json: &str)
        -> Result<String, JsError>;                    // ApplyInfo JSON

    /// Uncompressed epoch payload + its manifest epochs[] entry. Verifies
    /// sha256 == entry.hash, header/range, prev-linkage against internal
    /// chain cursor. Must be last_epoch + 1 (EPOCH_GAP otherwise).
    pub fn apply_epoch(&mut self, payload: &[u8], entry_json: &str)
        -> Result<String, JsError>;                    // {"applied":e,"state_changed":true}

    /// head.ndjson bytes. Returns {"head":…,"l1_accepted":…,"tail_rewound":bool}.
    pub fn apply_head(&mut self, payload: &[u8]) -> Result<String, JsError>;

    /// Persistable state blob: epoch-derived state ONLY — the tail is never
    /// exported (notes §7: never persist the tail). Cheap to skip: only call
    /// when apply_epoch/load_snapshot reported state_changed.
    pub fn export(&self) -> Vec<u8>;

    /// One full discovery pass for one owner: checkpoint pass at
    /// last_epoch_to + live pass at head + spent-state refresh — the same
    /// two-pass structure as sync_once in crates/client/src/sync.rs.
    /// `key` is zeroized in place before return (honest-limit note: JS may
    /// hold other copies; document `crypto.getRandomValues`-style hygiene).
    /// `cursor_blob`: sealed blob from a previous call, or absent.
    pub fn discover(&mut self, owner_hex: &str, key: &mut [u8],
                    cursor_blob: Option<Vec<u8>>)
        -> Result<DiscoverOut, JsError>;

    /// Reference-schema DiscoveryCursor JSON (spec §7.4 interop) extracted
    /// from a sealed blob — the migration path to compat/SDK. Requires key.
    pub fn export_reference_cursor(&self, key: &mut [u8], cursor_blob: &[u8])
        -> Result<String, JsError>;
}

#[wasm_bindgen(getter_with_clone)]
pub struct DiscoverOut {
    pub report_json: String,   // the SyncReport JSON, field-identical to
                               // `strk20-sync sync --json` (§A5.4 schema)
    pub cursor_blob: Vec<u8>,  // sealed; hand back next time
    pub added_json: String,    // notes not present in the previous cursor_blob
    pub spent_json: String,    // nullifiers newly spent this pass
}
```

Why one `discover` instead of the sketch's `discover(owner, key, cursor)` per
flow: a wallet wants exactly one call per feed change; incoming, outgoing,
checkpoint/live split, and spent refresh are internal mechanics. The module
owns the two-pass + reorg-rewind logic, so the TS wrapper — the least testable
layer — contains none of it.

### A3.4 State blob format (`export`/`load`) — versioning + compatibility stamp

```
offset  field
0       magic "S20S"
4       u16 LE format_version = 1
6       u16 LE flags = 0
8       u32 LE header_len
12      header: canonical JSON
        {"v":1,"engine":"<client-core semver>","chain_id":"SN_MAIN","pool":"0x…",
         "epoch_size":10000,"genesis_block":8978970,
         "last_epoch":1405,"last_epoch_hash":"<64hex>","last_epoch_to":14059999,
         "history_floor":14050000,"slots":N,"events":M}
…       slot section: N × (32B slot ‖ 32B value ‖ 8B LE write_block), slot-ascending
…       event section: M × framed EventRec (u64 block, u32 event_index, u32 tx_index,
        32B tx_hash, u16 n_keys, keys, u16 n_data, data), (block,event_index)-ascending
end−32  sha256 over bytes [0, end−32)
```

`load` rejects: bad magic/trailer → `BLOB_CORRUPT`; format_version ≠ 1 →
`BLOB_VERSION`; engine semver major ≠ current → `BLOB_VERSION`;
chain_id/pool/epoch_size/genesis_block ≠ the passed genesis → `BLOB_FOREIGN`.
`check_manifest` then arbitrates staleness against the live feed via
`last_epoch`+`last_epoch_hash` — this is the notes-§7 compatibility stamp
(format version, chain id, hash of last applied epoch), made executable. A
rejected blob is never partially loaded.

### A3.5 Sealed cursor blob — the registry-fingerprint answer

Notes §7 flags the persisted per-key cursor/registry as a fingerprint on shared
machines and suggests encrypting under a viewing-key-derived key. Adopted, and
placed **inside the module** so the TS layer cannot get it wrong:

```
"S20C" ‖ u16 version=1 ‖ 16B salt ‖ 24B nonce ‖ AEAD ciphertext
key   = HKDF-SHA256(ikm = 32B BE viewing key, salt, info = "strk20-cursor-seal-v1")
AEAD  = XChaCha20-Poly1305, AAD = chain_id ‖ 0x00 ‖ pool (canonical hex, ASCII)
plaintext = canonical JSON:
  {"v":1,"generation":<tail_generation>,"ckpt_at":<block>,
   "in_ckpt":<reference DiscoveryCursor JSON>,"out_ckpt":…,"in_live":…,"out_live":…,
   "notes":[{"note_id","owner","sender","token","index","nullifier","amount","block","spent"},…]}
```

`discover` accepts a sealed blob and returns a new one; an AEAD failure (wrong
key, tampered store, foreign chain via AAD) is treated as *no cursor* — fresh
discovery, with `details.cursor_reset = true` surfaced so the wrapper can log
it. To IndexedDB — and to any same-origin snoop — the blob is uniform noise.
The reference-schema JSON remains extractable (`export_reference_cursor`) so
the §7.4 cursor-interop mandate (migrate to compat/SDK without resync) holds in
the browser exactly as it does in `sync.db`.

### A3.6 The discovery-core fork, until the upstream PR lands

Spike fact: the 2-line `starknet-providers` feature gate is the whole delta.
Management:

1. Fork `starkware-libs/starknet-privacy` under our org; branch
   `strk20/providers-feature-74841ca` = the pinned upstream rev + one commit
   touching **only** `discovery-core/Cargo.toml` (feature `providers`,
   default-on, gating the dependency).
2. Workspace pin: wasm-facing crates depend on the fork rev with
   `default-features = false`; native crates keep default features → their
   dependency graph is byte-identical to today's.
3. CI guard `fork-delta-check`: `git diff --stat <upstream-rev> <fork-rev>`
   must list exactly one file, `discovery-core/Cargo.toml`. The
   "upstream consumed UNMODIFIED" invariant stays machine-audited: engine
   *source* identity, manifest-only delta.
4. Roadmap item 7 (upstream PR) proceeds in parallel; on merge+tag, repoint the
   pin, delete the fork branch, delete the CI guard.

### A3.7 Error model

Every thrown `JsError` message parses as `{"code","message","details"}`. Codes
(closed set, versioned with the ABI):

| code | thrown by | details |
|---|---|---|
| `BLOB_CORRUPT` / `BLOB_VERSION` / `BLOB_FOREIGN` | `load` | expected/actual stamp fields |
| `FEED_HASH_MISMATCH` | `apply_epoch`, `load_snapshot` | `{epoch, expected, actual}` — same naming as the Rust client's U5 error |
| `FEED_CHAIN_BROKEN` | `apply_epoch` | `{epoch, expected_prev, actual_prev}` |
| `FEED_MALFORMED` | any parser | line/field |
| `EPOCH_GAP` | `apply_epoch` | `{expected, got}` |
| `TAIL_GAP` | `apply_head` | `{tail_from, floor}` — the mid-sync race; retry |
| `SNAPSHOT_ROOT_MISMATCH` | `load_snapshot` | `{computed, header, anchor}` |
| `SNAPSHOT_NOT_EMPTY` | `load_snapshot` | — |
| `CHAIN_MISMATCH` | `new`, `load`, `check_manifest` | `{expected, got}` |
| `KEY_INVALID` | `discover` | key not a valid felt |
| `DISCOVERY_INCOMPLETE` | `discover` | pass cap hit (mirrors `MAX_PASSES`) |
| `INTERNAL` | anywhere | panic-catch shim message |

Size budget: spike measured 231 KB gzip; CI gate at **≤ 300 KB gzip** for the
`.wasm` artifact (a failing size check is a review event, not a silent bloat).

---

## A4 — the npm package

### A4.1 Naming and shape

Package: **`strk20-discovery`** (unscoped — adopting the notes-§6 lean: no
dependency on someone else's npm org, no implication of officialdom).

**AMENDMENT — spec §12.1** named the roadmap package `@strk20/discovery-provider`.
Superseded: the deliverable is unscoped `strk20-discovery`; the SDK adapter
(`LocalDiscoveryProvider implements DiscoveryProviderInterface`, everything
§12.1 specifies about cursor conversion semantics) ships **inside it** as the
subpath export `strk20-discovery/sdk`. One install, both audiences.

```
strk20-discovery
├── package.json         type: module; main/exports ESM + CJS; types; sideEffects:false
├── dist/index.js|cjs|d.ts
├── dist/sdk.js|d.ts     LocalDiscoveryProvider adapter (§12.1 semantics verbatim)
├── dist/engine_bg.wasm  strk20-engine (lazy-instantiated on first use)
└── README.md            the five-line quickstart, verbatim from this doc's intro
```

Engines: `node >= 20` (for the Node/backend audience of KeylessClient),
evergreen browsers. Bundler note in README: the wasm is loaded via
`new URL('engine_bg.wasm', import.meta.url)` — works untouched in Vite,
webpack 5, Next; a `wasmUrl` option overrides for exotic setups. Total install
cost on the wire: wasm ~231 KB gzip + glue ~15 KB + fzstd ~8 KB ≈ **~255 KB
gzip**, CI-gated ≤ 300 KB.

### A4.2 One interface, two clients

```ts
export interface ViewingKeyRef {
  address: string;                 // "0x…" recipient/owner account
  viewingKey: Uint8Array | string; // 32B BE or hex; Uint8Array is zeroized after use
}

export type DiscoveryEvent =
  | { type: 'notes';  added: Note[]; spent: Note[]; head: number }
  | { type: 'reorg';  rewoundTo: number }                    // epoch floor
  | { type: 'status'; state: 'live' | 'polling' | 'degraded' | 'verifying' }
  | { type: 'error';  error: Strk20Error; recovering: boolean };

export interface Note {
  token: string; index: number; noteId: string; nullifier: string;
  amount: bigint; blockNumber: number; sender: string; spent: boolean;
}
export interface NotesResult {
  notes: Note[]; balances: Map<string, bigint>;
  head: number; l1Accepted: number; complete: boolean;
}

export interface DiscoveryClient {
  getNotes(key: ViewingKeyRef): Promise<NotesResult>;
  subscribe(key: ViewingKeyRef, cb: (ev: DiscoveryEvent) => void): () => void;
  history(key: ViewingKeyRef): Promise<{ completeFrom: number; transactions: HistoryTx[] }>;
  status(): ClientStatus;   // {mode:'keyless'|'delegated', transport:'sse'|'polling',
                            //  head, l1Accepted, verified: boolean, persistence: 'indexeddb'|'memory'}
  close(): Promise<void>;
}

export class KeylessClient implements DiscoveryClient {
  constructor(opts: {
    feedUrl: string;
    network?: 'mainnet' | 'sepolia' | ChainProfile;   // §A6; default 'mainnet'
    persistence?: 'auto' | 'indexeddb' | 'memory' | StorageAdapter;  // default 'auto'
    coldStart?: 'snapshot' | 'genesis';               // default 'snapshot'
    verify?: 'block' | 'background';                  // default 'block' (§A1.5)
    live?: boolean;                                   // default true; false = poll only
    pollIntervalMs?: number;                          // default 30_000
    requestPersistentStorage?: boolean;               // navigator.storage.persist()
    wasmUrl?: string | URL;
    fetch?: typeof fetch;
  });
}

export class DelegatedClient implements DiscoveryClient {
  constructor(opts: { serverUrl: string; authToken?: string; fetch?: typeof fetch });
}

export class Strk20Error extends Error {
  code: string;                       // the §A3.7 / §A5.6 closed sets
  details?: Record<string, unknown>;
}
```

The roadmap's contract — `getNotes(key)`, `subscribe(key)` behind one
interface — is honored literally; `ViewingKeyRef` bundles the address because
upstream discovery is (address, key)-parameterized and hiding that would be a
lie. Switching Keyless↔Delegated is a constructor swap; acceptance leg r
asserts deep-equal results from both against the same fixture.

`subscribe` mechanics (keyless): SSE `head`/`epoch` events → conditional GET →
`apply_head`/`apply_epoch` → `discover` per subscribed key → emit only deltas
(`added`/`spent` from `DiscoverOut`). Post-submit UX (the roadmap's
"read half of every write"): the wallet fires a transfer, then `subscribe`
delivers `{type:'notes', spent:[theInput], added:[theChange]}` when the
nullifier lands — confirmation without a write path.

### A4.3 IndexedDB layout

One database, versioned stores, everything namespaced by chain+pool so one
origin can serve mainnet and sepolia UIs concurrently:

```
DB "strk20-discovery", version 1
  store "state"     key `${chainId}:${pool}`
                    val {blob: ArrayBuffer,            // Engine.export()
                         lastEpoch: number, lastEpochHash: string, savedAt: number}
  store "epochs"    key `${chainId}:${pool}:${e08}`    // e zero-padded for range scans
                    val {zst: ArrayBuffer, hash: string}      // AS FETCHED (compressed)
  store "snapshot"  key `${chainId}:${pool}`
                    val {zst: ArrayBuffer, e: number, hash: string}
  store "cursors"   key `${chainId}:${pool}:${ownerTag}`
                    val {sealed: ArrayBuffer, savedAt: number}
  store "meta"      key string → small JSON
```

- `ownerTag = hex(sha256(utf8(addressLowerHex)))[0:16]` — the address itself
  never appears in IndexedDB keys (fingerprint hygiene layered on top of the
  sealed blob, which already hides everything else).
- **Never stored**: `head.ndjson` bytes, head ETag, anything tail-derived — the
  no-persisted-reorg-logic property from notes §7 is enforced by the schema
  having nowhere to put a tail.
- Quirks engineering (each has a test): (1) IndexedDB transactions auto-commit
  at microtask end — no `await fetch` inside a txn; the wrapper stages bytes
  first, writes in one txn. (2) `open` can throw synchronously or fire
  `onblocked` (private windows, eviction, another tab mid-upgrade) — every
  path falls back to `persistence:'memory'` and reports it via `status()`.
  (3) Eviction is normal — every read path treats an empty store as a cold
  start, never as corruption. (4) Multi-tab: sync passes run inside
  `navigator.locks.request('strk20:${chainId}:${pool}', …)` when Web Locks
  exists; without it, last-writer-wins is safe because every persisted value is
  self-verifying (blobs re-checked on load, epochs re-hashed). (5) Safari
  first-write latency: the initial persist happens post-`getNotes`-resolve,
  never on the critical path.

### A4.4 Raw epochs vs folded mirror — both designs, and the gate that picks

**Design R — raw epochs are the persisted truth.** Persist `snapshot` +
`epochs` stores exactly as fetched (compressed). Load path: read all →
decompress (fzstd) → `load_snapshot` + `apply_epoch` chain → full
re-verification of every byte on every load. Tamper with IndexedDB and the wasm
module rejects it (`FEED_HASH_MISMATCH` / `SNAPSHOT_ROOT_MISMATCH`) and the
wrapper refetches. No trust is ever placed in the browser's storage.

**Design M — folded mirror as a cache over R.** Additionally persist the
`state` store (Engine.export blob, keyed by `lastEpochHash`). Load path:
`Engine.load(blob)` → `check_manifest` → `"ok"`/`"behind"` skips all folding;
`"diverged"` or any `BLOB_*` error falls back to Design R (refold), which falls
back to the network. The blob's sha256 trailer + stamp make the cache
self-invalidating; the hash chain is still the truth because the blob is only
ever *born* from verified applies.

Both are implemented behind one internal `StatePersister` interface; the gate
decides the default:

**The fold-time measurement gate** (from notes §7, made operational — measured
before the persistence layer is finalized):

- Harness: `bench/fold.html` + `npm run bench:fold` (Playwright: chromium +
  webkit). Input A: the full recorded mainnet feed as of the measurement date
  (fetched by the nightly-smoke tooling, stored under `fixtures/live/`).
  Input B: a synthetic 10× slot-count feed from the fixture generator
  (headroom). Output JSON per run: `{input, decompressMs, verifyMs, foldMs,
  rootVerifyMs, totalMs, slots, epochs, ua}`.
- Reference device: the maintainer's laptop, recorded in
  `docs/research/data/fold-bench.json` with the hardware named; CI headless
  chromium runs the same bench as a trend line (informative, not the gate —
  CI hardware variance would make the gate arbitrary).
- Decision rule on `totalMs` for input A (thresholds verbatim from notes §7):
  - **≤ 200 ms** → ship Design R only; the `state` store and `Engine.export`
    stay in the ABI (the Rust side costs nothing) but the wrapper never calls
    them — a whole persistence layer disappears from TS.
  - **≥ 2 s** → Design M mandatory, default on.
  - between → Design M ships default-on; `persistence` option exposes
    `'indexeddb'` (M) vs `'indexeddb-raw'` (R) for integrators who want
    re-verification per load.
- Input B ≥ 2 s additionally mandates keeping M's code path alive regardless of
  A's result (growth headroom), just not necessarily default.
- The measured numbers and the decision are recorded in this document's
  fill-in box:

> **FILL-IN (fold gate):** input A total = ___ ms (decompress ___ / verify ___ /
> fold ___ / root ___), input B total = ___ ms, device = ___. Decision: R / M /
> M-default-on. Date: ___.

`export()` cadence (notes-§7 hazard "writing megabytes on every head poll"):
the wrapper calls `export` only when an apply reported `state_changed` — i.e.
once per epoch cut (~4.7 h) or snapshot load, never on head events.

### A4.5 zstd in TypeScript

Chosen: **fzstd** (pure-JS decompress-only, ~8 KB gzip, MIT). Rationale over
wasm decoders (zstddec, @oneidentity/zstd-js): no second wasm artifact to load
and version, decompression of a ~6 MB feed costs ~50–150 ms — invisible next
to network; and over waiting for `DecompressionStream('zstd')`: not yet
portable. The wrapper feature-detects `DecompressionStream('zstd')` and prefers
it when present (free native speed), fzstd otherwise — both behind one
`inflateZstd(bytes)` utility. Compression never happens in TS (nothing
uploads). The server keeps serving `.zst` files as opaque bytes
(`Content-Type: application/zstd`), so no reliance on `Content-Encoding`
negotiation or proxy transparency.

### A4.6 Cold start, time-to-first-note budget

Sequence (keyless, empty IndexedDB, snapshot available):
`genesis+manifest` (1 RTT, ~10 KB) → snapshot `.zst` (~300 KB today) → fzstd
(~30 ms) → `load_snapshot` incl. MPT root (§A1.5 — the measured `rootVerifyMs`;
if it alone breaches ~1 s on the reference device, the README's worker recipe
becomes the headline integration) → epochs > e (usually 0–2 files) → head
(~16 KB) → `discover` (spike: 32 ms for devnet; full-history discovery is the
`foldMs`-adjacent number the bench also records) → notes on screen. Everything
after the first paint runs off the main thread if the integrator uses the
provided worker recipe (`strk20-discovery/worker` subpath: a ~40-line wrapper
that proxies the same `DiscoveryClient` interface over `postMessage` — shipped,
because "run it in a worker" advice without code never gets followed).

### A4.7 The TS e2e against the real server binary

New workspace member `ts/packages/strk20-discovery` with `e2e/` driven by
vitest. Topology reuses the Rust acceptance harness — the fixture RPC and the
recording proxy are **promoted to bin targets** in `crates/e2e-tests`
(`fixture-rpc`, `recording-proxy` — same code, new `[[bin]]` sections) so a
non-Rust process can spawn them:

```
vitest globalSetup:
  1. cargo build -p strk20-indexerd -p e2e-tests --bins   (or use $STRK20_PREBUILT)
  2. spawn fixture-rpc         → :A
  3. spawn strk20 run --rpc-url :A --feed-dir tmp --listen :B
  4. spawn recording-proxy :C → :B   (capture file: proxy-capture.bin)
  5. expose FEED_URL=http://127.0.0.1:C/feed to tests
teardown: kill children; run `cargo run -p e2e-tests --bin capture-scan -- proxy-capture.bin idb-dump.json`
```

- Node polyfills: `fake-indexeddb` (persistence tests), `eventsource` (SSE in
  Node), global fetch native. The browser path is additionally exercised by one
  Playwright smoke (real chromium, real EventSource, real IndexedDB) against
  the same spawned stack.
- The no-key scanner is **not** reimplemented in TS: `capture-scan` is a third
  new bin reusing the leg-d Rust scanner verbatim over (i) the proxy capture,
  (ii) a JSON dump of fake-indexeddb the TS test writes at the end. One scanner
  implementation, every capture surface (leg p/q).
- Golden truth: the TS tests read the same checked-in O2 golden JSON the Rust
  acceptance uses — byte-one source of expected notes.

---

## A5 — `strk20-sync serve`

### A5.1 What it is

The self-host surface `DelegatedClient` talks to: a **keyed** HTTP+SSE server
on the client binary. It runs Block B server-side over the same `FeedStore` +
`sync_once` code the CLI uses, so keyless and delegated results are equal by
construction (and by acceptance leg r). It links no `strk20-indexerd` code —
R5's dependency direction is untouched; the flagship server binary still never
sees a key.

Relation to compat (`strk20 --enable-compat`), stated once:

| | wire | audience | subscription | key handling |
|---|---|---|---|---|
| compat | upstream reference `/v1/sync/*` | stock SDK `IndexerDiscoveryProvider` | none (poll) | key per request, engine-direct |
| serve | `/v1/delegated/*` (ours, below) | `DelegatedClient` | SSE per registered watch | key per request or in-memory watch |

`DelegatedClient` speaks the serve wire only. Wallets already on the upstream
SDK point it at compat; both self-host paths coexist on one box.

### A5.2 The third `FeedTransport` impl: in-process DB

New crate `crates/dbfeed` (lib `strk20-dbfeed`; deps: `strk20-feed`,
`rusqlite`, `zstd` — **not** `strk20-indexerd`, preserving "nothing links the
server crate"). It opens `strk20.db` read-only (`?mode=ro`,
`PRAGMA query_only`) and implements `FeedTransport` by rebuilding feed
artifacts from rows — the same pure function the cutter runs, checked against
the cutter's stored hashes:

- `fetch_manifest` → assembled from the `epochs` table + `meta` (shared shape
  with `Cutter::rewrite_manifest`).
- `fetch_epoch(e)` → serialize block lines from rows via `strk20-feed::codec`,
  assert `sha256(payload) == epochs.content_hash` (a divergent rebuild is a
  hard error naming the epoch — the DB is never trusted to be self-consistent),
  return raw payload.
- `fetch_head` → tail rows above the epoch floor, encoded; ETag = payload
  sha256 as in `DirTransport`.

**AMENDMENT — spec §7.2 `FeedTransport`.** Old signature:

> `async fn fetch_epoch(&self, idx: u64) -> Result<Vec<u8>>;   // compressed bytes`

New signature (the only trait change; the compile-fail signature lock is
updated in the same commit):

```rust
pub enum EpochPayload { Compressed(Vec<u8>), Raw(Vec<u8>) }
async fn fetch_epoch(&self, idx: u64) -> Result<EpochPayload>;
async fn fetch_snapshot(&self, e: u64) -> Result<Vec<u8>>;   // compressed; A1
```

Rationale: forcing the in-process transport to zstd-19 bytes it just rebuilt,
so the caller can immediately un-zstd them, is waste with no privacy or trust
payoff — content identity is over uncompressed bytes everywhere already. HTTP
and Dir transports return `Compressed`; dbfeed returns `Raw`. Still zero
user-derived parameters; the keyless property is untouched.

Deployment shapes for serve: `--feed https://…` (remote feed),
`--feed /path/feed` (local dir), `--feed db:/path/strk20.db` (colocated with a
`strk20 run`, zero HTTP hops, zero duplicate verification).

### A5.3 CLI

```
strk20-sync serve --feed <URL|DIR|db:PATH> --listen 127.0.0.1:8787
                  [--db <serve-sync.db>] [--auth-token-file <path>]
                  [--cors-origin <origin>]... [--allow-remote]
```

### A5.4 Wire

All bodies JSON; errors use the global envelope
`{"error":{"code","message","details"}}`. Every response carries
`X-Strk20-Mode: delegated-keyed`.

```
GET  /v1/delegated/health
  → {"status":"OK"|"DEGRADED","chain_id":"SN_MAIN","pool":"0x…",
     "head":14056430,"l1_accepted":14049912,"last_epoch_to":14059999,
     "history_from":14050000,"feed_source":"https://…|dir|db"}

POST /v1/delegated/notes
  body {"address":"0x…","viewing_key":"0x…"}
  → the SyncReport JSON, byte-shape-identical to `strk20-sync sync --json`:
    {"address","head","l1_accepted","last_epoch_to","tail_rewound",
     "incoming_complete","outgoing_complete","incoming_senders":[…],
     "outgoing_recipients":[…],
     "notes":[{"token","index","note_id","nullifier","amount","block_number","sender","spent"},…],
     "balances":{"0x<token>":"<amount>"},"newly_spent":[…]}

POST /v1/delegated/history
  body {"address":"0x…","viewing_key":"0x…"}
  → {"history_from":<block>,"transactions":[…reference history shapes…]}

POST /v1/delegated/subscribe
  body {"address":"0x…","viewing_key":"0x…"}
  → {"stream":"/v1/delegated/stream","token":"<22-char base64url, 128-bit random>","expires_in":300}

GET  /v1/delegated/stream?token=<…>            (SSE)
  event: sync   data: {"head":…,"l1_accepted":…,"added":[Note…],"newly_spent":[…],"tail_rewound":false}
  event: reorg  data: {"rewound_to":<epoch floor>}
  : ping every 15s
```

Subscribe semantics: the key is held **in memory only** for the life of the
stream (registered watch → serve's internal loop runs `sync_once` per feed
change and pushes deltas); disconnect or token expiry drops it; nothing keyed
is ever persisted by serve (its own `sync.db` holds mirrors and sealed-cursor
meta exactly as the CLI does, chmod 0600 rules inherited). The stream token is
a random capability, not key material — it may appear in a query string
because EventSource cannot send headers; it is single-use-bound to the
connection, expires in 300 s if unclaimed, and grants nothing but "receive this
watch's events".

### A5.5 Security posture

- Binds `127.0.0.1` by default; a non-loopback `--listen` is **refused** unless
  both `--allow-remote` and `--auth-token-file` are given (fail-shut).
- Auth: `Authorization: Bearer <token>` on every `/v1/delegated/*` request
  (except the stream, which uses its own token); constant-time compare; 401
  `AUTH_REQUIRED` / `INVALID_TOKEN`.
- Bodies carry raw viewing keys → the compat hardening rules apply verbatim
  (spec §6.4): request/response bodies never logged (hard-coded, no config),
  malformed bodies rejected without echoing (400 `INVALID_BODY`), keys
  zeroized after each request, cursors sealed at rest.
- TLS: terminate at a reverse proxy (documented recipe); serve itself stays
  TLS-free in v1 (less code in the keyed binary, and self-host loopback is the
  default deployment).
- CORS: off unless `--cors-origin` given (browser `DelegatedClient` against a
  self-hosted box is expected to be same-origin behind the operator's proxy).

### A5.6 Serve error codes

`AUTH_REQUIRED`(401), `INVALID_TOKEN`(401), `STREAM_TOKEN_EXPIRED`(401),
`INVALID_BODY`(400), `KEY_INVALID`(400), `SERVICE_UNAVAILABLE`(503, feed
unreachable or mirror degraded). HTTP 409 stays reserved for compat's
`BLOCK_REORGED` (spec §6 global rule); serve reports reorgs in-band via the
`reorg` event / `tail_rewound` field because its clients hold no block refs.

---

## A6 — chain profiles

### A6.1 Mechanism

`ChainConfig` (`crates/indexerd/src/config.rs`) is promoted to a shared
`ChainProfile` in `strk20-feed` (it is pure data; both binaries and the wasm
crate need it) with a named registry and file loading:

```rust
pub struct ChainProfile {
    pub name: String,             // "mainnet" | "sepolia" | custom
    pub chain_id: String,         // "SN_MAIN" | "SN_SEPOLIA"
    pub pool: Felt,
    pub genesis_block: u64,
    pub epoch_size: u64,
    pub snapshot_every: u64,      // epochs; A1 cadence
    pub rpc_primary: String,      // indexerd-only fields ignored by clients
    pub rpc_fallback: Option<String>,
    pub decoder_map: BTreeMap<Felt, String>,
}
impl ChainProfile {
    pub fn builtin(name: &str) -> Option<Self>;      // "mainnet", "sepolia"
    pub fn from_toml(s: &str) -> Result<Self>;
}
```

CLI: `strk20 run --network sepolia` / `--network ./mychain.toml`
(`--network mainnet` is the default; every existing mainnet default constant
moves into `builtin("mainnet")` unchanged). Same flag on `strk20-sync` and the
same values behind the npm `network` option. TOML mirrors the struct:

```toml
name = "sepolia"
chain_id = "SN_SEPOLIA"
pool = "0x0254a6b2997ef52e9f830ce1f543f6b29768295e8d17e2267d672c552cfe0d91"
genesis_block = 0        # FILL-IN (see A6.3)
epoch_size = 10000
snapshot_every = 25
rpc_primary = ""         # FILL-IN
[decoder_map]
# "0x<class-hash>" = "v2"   FILL-IN (see A6.3)
```

One process = one network (spec non-goal "multi-pool routes" upheld): two
networks = two processes, two feed dirs, two DBs. Feeds self-identify — that is
the whole point of the stamping below.

### A6.2 Chain-id stamping, end to end

The stamp already flows through most of the wire (genesis, epoch `hdr`,
manifest). The addendum closes every remaining gap so a mainnet client pointed
at a sepolia feed (or a stale blob from the other network) fails loudly at the
first byte, on every surface:

| surface | stamp | check added by this addendum |
|---|---|---|
| `genesis.json`, epoch `hdr`, `manifest` | `chain_id` + `pool` | already present |
| client `sync.db` | meta `chain_id` | **gap fix**: `FeedStore::apply_feed` currently rejects only pool mismatch (`store.rs` — `Some(stored) if stored != genesis.pool`); it now also stores and compares `chain_id` (`CHAIN_MISMATCH`) |
| snapshot `hdr` | `chain_id` + `pool` | new format, stamped from birth (A1.3) |
| SSE `hello` | `chain_id` + `pool` | A2.2; the TS client verifies against its profile before applying any event |
| wasm state blob | header `chain_id` + `pool` + `epoch_size` + `genesis_block` | `BLOB_FOREIGN` on mismatch (A3.4) |
| sealed cursor blob | AAD = `chain_id ‖ pool` | AEAD failure on cross-network reuse (A3.5) |
| IndexedDB | every key prefixed `${chainId}:${pool}` | A4.3 |
| serve `/v1/delegated/health` | `chain_id` + `pool` | `DelegatedClient` verifies on construction |
| compat | unchanged | reference wire has no chain field; compat is per-deployment |

npm: `network` resolves a `ChainProfile`; the client fetches `genesis.json`
and requires `genesis.chain_id == profile.chain_id && genesis.pool ==
profile.pool` before anything else (`CHAIN_MISMATCH` with both values named).
`feedUrl` stays mandatory — this project ships no hosted endpoint assumption.

### A6.3 Sepolia fill-in slot

The mechanism above is complete; the verified per-chain constants land here
when the parallel research task reports:

| field | value | status |
|---|---|---|
| pool | `0x0254a6b2997ef52e9f830ce1f543f6b29768295e8d17e2267d672c552cfe0d91` | given (roadmap item 6) |
| chain_id | `SN_SEPOLIA` | given |
| genesis_block (deployment block) | ___ | **FILL-IN** |
| class hash list → decoder versions | ___ | **FILL-IN** (research task in flight) |
| rpc_primary / fallback | ___ | **FILL-IN** |

Tooling so the fill-in is verified, not transcribed:
`strk20 profile verify --network sepolia --rpc <url>` — checks the pool exists,
`getClassHashAt(latest)` ∈ decoder_map, and locates the deployment block by
bisection on `getClassHashAt` (contract-not-found below, class above), printing
the values in TOML form. The command is also the recovery tool for the next
mainnet class upgrade (spec §5.7 degraded-mode exit: run it, paste the line).

---

## Cross-cutting spec amendments (collected)

1. **§4.2 / §4.4 snapshots** — replaced as quoted in A1.4 (NDJSON snapshot
   format, `snapshots[]` manifest array, SQLite snapshot dropped).
2. **§6.1 table** — two new rows as quoted in A2.4 (`/feed/live`, snapshot
   files).
3. **R3 / §12.2** — SSE scheduled; global-stream-only invariant preserved
   (A2.1).
4. **§7.2 `FeedTransport`** — `EpochPayload` + `fetch_snapshot` as quoted in
   A5.2; compile-fail lock updated in the same change.
5. **§8 CLI** — `strk20 snapshot create` semantics per A1.4, `snapshot import`
   removed, `strk20 profile verify` added, `--network` added to both binaries;
   `strk20-sync serve` added per A5.3.
6. **§12.1** — package renamed to unscoped `strk20-discovery`; SDK adapter
   becomes the `/sdk` subpath (A4.1). All §12.1 cursor-conversion semantics
   carry over verbatim.
7. **§6.4 compat** — `/v1/history` responses gain `"history_from"` (additive)
   when the mirror is snapshot-seeded (A1.2). A compat deployment backfilled
   from genesis serves `history_from = genesis_block` — no behavior change.
8. **§5.5 cutter** — snapshot cutting appended to the cut sequence (A1.4);
   snapshot requires a same-block verify-root success, unlike best-effort epoch
   anchors.

Unchanged and re-affirmed: feed requests identical for every user (the two new
GET targets and the SSE stream are parameterless); `SecretFelt` never
serializable (the wasm boundary passes raw bytes in and zeroizes; the sealed
cursor blob contains cursor material, encrypted, never the key itself);
epochs immutable, only the tail rewrites (snapshots exist strictly at L1-final
epoch boundaries); upstream `discovery-core` consumed unmodified
(manifest-only fork delta, CI-audited, A3.6).

---

## Implementation order (dependency-ordered; tests first; no time estimates)

Roadmap order 1 → 2 → 3 → 4 with 5/6/7 parallel is preserved; this refines it
to concrete steps. Every step begins by landing its (red) tests.

```
 S1  A6 profiles + stamping gap-fix          [tests: leg s; chain_id unit tests]
     └─ smallest change, but its stamps flow through every later format — first.
 S2  A1 snapshots                            [tests: snapshot golden byte vectors,
     (feed format → cutter → Rust client)     legs l, m, n]
 S3  A2 SSE                                  [tests: leg o + framing unit tests]
 S4  discovery-core fork branch + CI delta guard; upstream PR filed (roadmap 7)
 S5  A3 client-core extraction + MemStore    [tests: conformance — engine-over-
     (no wasm yet; native tests)              MemStore ≡ engine-over-MockBackend ≡
                                              engine-over-FeedStore; blob round-trip;
                                              sealed-cursor round-trip + AEAD-reject]
 S6  A3 wasm crate + ABI                     [tests: nodejs-target smoke = spike
                                              repro + ABI error-code table test;
                                              size gate ≤300KB gzip]
 S7  fold-gate bench (A4.6 harness)          → records the FILL-IN, fixes the
                                              A4 persistence default
 S8  A4 npm package + TS e2e                 [tests first: legs p, q, t vitest
                                              suites against spawned binaries]
 S9  A5 dbfeed + serve                       [tests: leg r; FeedTransport
     (parallel with S5–S8 after S3)           signature-lock update]
 S10 §12.1 SDK adapter (/sdk subpath), README/quickstarts, ops docs for SSE
     proxying + serve deployment
```

Dependencies, explicitly: S2 needs S1 (profile carries `snapshot_every`).
S5 needs S4 (portable engine dep) and S2 (snapshot load). S6 needs S5.
S7 needs S6 + the recorded mainnet feed fixture. S8 needs S3, S6, S7.
S9 needs S3 (SSE patterns) and the S1 profile plumbing; independent of wasm.
Roadmap item 6 (Sepolia constants) unblocks only the A6.3 fill-in, nothing
structural.

---

## H — New acceptance-test legs (written before the code they gate)

Extending spec §10.3's a–k. Same harness, same dual-oracle discipline, same
recording proxy; the scanner from leg d is reused on every new capture surface.

| leg | surface | assertion |
|---|---|---|
| **l** | A1 cold start (Rust) | Fixture cuts epochs 0–1 + snapshot at epoch 1. Fresh `strk20-sync` with empty db: request multiset == {genesis, manifest, snapshot, head} — **no epoch file fetched**; output == O1/O2 field-for-field incl. per-note `block_number` == committed partition block; meta `history_floor` set; a subsequent epoch-2 extension applies with chain seeded from `epoch_hash`. |
| **m** | A1 tamper | (i) bit-flip in the served snapshot → named `FEED_HASH_MISMATCH`-class error, no state applied; (ii) re-serve with a *consistent* sha256 but one slot value altered and header root left stale → `SNAPSHOT_ROOT_MISMATCH` naming computed vs anchor roots; client falls back to full epoch replay with a logged warning and still equals O1. |
| **n** | A1 determinism | Two independent backfills cut byte-identical snapshot files (sha256 equality); `strk20 epoch verify --all` covers snapshot hashes cross-mirror. |
| **o** | A2 SSE | Subscriber sees `hello` → `head`(append, correct `appended` list) → `epoch` → `snapshot` events matching the files served; two concurrent subscribers' captures byte-identical modulo attach offset; `Last-Event-ID` resume replays the suffix; foreign boot-id → fresh `hello`; any query string → 400; leg-d scanner over the SSE capture finds nothing. |
| **p** | A4 TS equality + no-key | TS `KeylessClient` through the recording proxy against the real spawned `strk20`: `getNotes` deep-equals the O2 golden pins; `capture-scan` (Rust scanner) over the TS proxy capture + fake-indexeddb dump finds no key/address/channel-key encoding; alice/bob request-URL multisets identical (SSE connection included). |
| **q** | A4 persistence | Second client instance over the same fake-indexeddb: request multiset delta = {manifest, head} only (plus SSE); results equal. Tampered persisted epoch/state blob → detected (`BLOB_CORRUPT`/`FEED_HASH_MISMATCH`), refetched, correct output; wrong-key sealed cursor → fresh discovery with `cursor_reset` surfaced, same final notes. |
| **r** | A5 delegated | Spawn `strk20-sync serve --feed db:` against the fixture server's DB; `DelegatedClient` result deep-equals `KeylessClient` result on the same fixture; subscribe stream delivers the leg-k spent event; non-loopback bind without token refused at startup; 401 without bearer; scanner over serve's stdout/stderr + its sync.db finds no raw key (sealed blobs excluded by construction, asserted anyway). |
| **s** | A6 mismatch | Sepolia-stamped fixture feed vs mainnet-profile client: Rust `CHAIN_MISMATCH` naming both ids before any row lands; TS the same; wasm `load` of a mainnet blob against sepolia genesis → `BLOB_FOREIGN`. |
| **t** | A2/A4 fallback | Kill the SSE route mid-test (proxy drops the stream, then 404s it): TS client degrades to polling (status event `polling`), still converges to post-extension ground truth; reconnect restores `live`. |
| **u** | A3 reorg in-memory | wasm engine: fixture fork 44→44′ per leg g; `apply_head` reports `tail_rewound`; next `discover` output == post-fork O1; persisted state blob is byte-identical before/after (tail never exported). |

Compile-fail / lock updates: `FeedTransport` lock regenerated for the
`EpochPayload` signature (still no user-derived parameter is expressible);
a new doctest lock asserting `MemStore`/`Engine` expose no method taking an
address or key except `discover`/`export_reference_cursor` (the two that must,
and that never touch a transport).

Bench additions (§10.5): B1 gains a snapshot-path variant (cold-start bytes/time
via snapshot vs via epochs — the number the README quotes); new B9 = fold-gate
harness output tracked over time.

---

## Open items this addendum leaves open, deliberately

1. The fold-gate FILL-IN (A4.4) — measured, then written here.
2. Sepolia constants FILL-IN (A6.3) — parallel research task.
3. Worker-first npm API (making the worker wrapper the default rather than a
   subpath) — decide after the fold gate: if fold+root-verify lands under
   ~200 ms there is nothing worth moving off the main thread.
4. `DecompressionStream('zstd')` promotion from feature-detect to default —
   revisit when baseline browser support exists.
5. OHTTP, PIR/prefix-bucket, write path — deferred exactly as the roadmap
   states; nothing above narrows or widens their triggers.
