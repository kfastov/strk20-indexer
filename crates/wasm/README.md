# `strk20-engine` — Block B as a wasm module

The browser runs **the same engine code as the native client**. Not a port, not
a reimplementation: `crates/consumer` compiled for `wasm32-unknown-unknown`
behind a `wasm-bindgen` facade. The equality that matters is checked, not
asserted — `test/smoke.mjs` folds a real feed through the module and demands the
resulting `SyncReport` be **byte-identical** to the one the native path produces
from the same bytes.

```
$ ./build.sh
  ok    alice: report is byte-identical to the native fold
  ok    alice: viewing key was zeroized in the caller's buffer
  ...
PASS
```

## The contract

**The module is a pure synchronous computer: bytes in, notes out.**

TypeScript owns `fetch`, IndexedDB, zstd inflation and SSE. Rust owns
verification, folding, discovery and the report. Nothing crosses that line:

| The module has no | because |
|---|---|
| network | no `reqwest`, no `web-sys` fetch — the import section proves it (below) |
| storage | no `rusqlite`, no filesystem; the mirror is `BTreeMap`s |
| async runtime | no `tokio`, no `wasm-bindgen-futures`; every ABI method is synchronous |
| zstd | `zstd-sys` compiles C and has no wasm32 backend — see *Decompression* |
| randomness | `getrandom` is in the dependency tree via `lambdaworks-math`, and is **dead code**: a live call would need a `crypto.getRandomValues` import, and the module has none |

Block B's `async` is real but never suspends: over an in-memory view and a
staged transport, every leaf resolves on its first poll. `src/drive.rs` runs the
whole pipeline with one `poll` and **panics** if anything pends — a
programming-error tripwire, not a runtime path.

## Build

```sh
./build.sh              # fixture + golden, wasm-pack, import audit, smoke test
```

or by hand:

```sh
cargo run -p strk20-engine --example make_fixture     # feed + native golden
wasm-pack build --release --target web --out-dir pkg
node test/imports.mjs && node test/smoke.mjs
```

**Target `web`, not `bundler`.** One artifact serves both consumers: a browser
loads it from a `<script type="module">` with no build step, and Node loads the
*same* file by handing `init` the bytes — which is exactly what the smoke test
does, so the thing under test is the thing that ships. `bundler` output cannot
be loaded without webpack/Vite resolving a bare `.wasm` import, which would have
forced a second `nodejs` build and split the artifact under test from the artifact
shipped. Vite and webpack both consume `web` output fine.

Size flags live in `.cargo/config.toml` (scoped to the wasm target) rather than
in `[profile.release]`, because cargo honours `[profile]` only in the workspace
root manifest, which is shared with the native binaries.

## ABI

All inputs are `Uint8Array` or strings; all outputs are JSON strings or
`Uint8Array`. Every fallible method throws a `JsError` whose `message` is one
canonical JSON object (below).

### Lifecycle

```ts
new Engine(genesisJson: string): Engine        // pins chain identity
Engine.load(blob: Uint8Array, genesisJson: string): Engine
Engine.version(): string
set_panic_hook(): void                         // call once at startup
engine.free()
```

### Staging — push bytes in (no folding happens here)

```ts
engine.stage_manifest(manifestJson: string): void         // required before apply
engine.stage_epoch(e: bigint, payload: Uint8Array): void   // INFLATED payload
engine.stage_snapshot(e: bigint, zst: Uint8Array, payload: Uint8Array): void
engine.stage_snapshot_anchor(e: bigint, json: Uint8Array): void
engine.stage_anchors(payload: Uint8Array): void            // required for a snapshot cold start
engine.stage_head(payload: Uint8Array, etag: string): void
```

`stage_snapshot` takes **both halves**: ring 1 of the §1.5 ladder hashes the
compressed file in Rust, rings 2–5 parse the inflated one. Epochs are raw-only —
Block B checks no `.zst` hash for them anywhere.

### Staging a storage proof — §1.5 ring 6, the `"anchored"` grade

```ts
engine.proof_candidates(): string
// {"pool","basis","head","blocks":number[],"staged":number[],"reason":string|null}

engine.stage_storage_proof(block: bigint, proofJson: string, blockHashHex: string): void
engine.clear_storage_proofs(): void
```

`blocks` is the list of blocks ring 6 will ask about, **in the order it will
ask** — computed by Block B (`grounding_candidates`), not by this wrapper, so
the two cannot drift. Stage a proof for one of them; see
[What TypeScript must do](#what-typescript-must-do) for the two RPC calls.

### Folding

```ts
engine.apply(coldStart: "auto" | "snapshot" | "epochs"): string
// {"epochs_applied","last_epoch","last_epoch_to","head","l1_accepted",
//  "tail_rewound","history_floor","snapshot_basis","snapshot_rejected",
//  "state_changed"}

engine.apply_head(payload: Uint8Array, etag: string): string   // SSE hot path
// {"head","l1_accepted","tail_rewound"}
```

`apply` is incremental and idempotent: applied epochs are skipped, an unchanged
ETag skips the tail rebuild. It runs the whole trust pipeline — epoch hash chain,
chain/pool binding, snapshot ladder, reachability grounding, reorg supersede.

`state_changed` is true only when **epoch-derived** state moved. A tail-only
change is not a state change, which is the rule that keeps an exported blob
un-stale-able by a reorg.

### Reading

```ts
engine.info(): string
// {"chain_id","pool","genesis_block","epoch_size","last_epoch","last_epoch_hash",
//  "last_epoch_to","history_floor","snapshot_basis","head","l1_accepted",
//  "slots","tail_generation","engine_version"}

engine.check_manifest(manifestJson: string): "ok" | "behind" | "diverged"
```

Staleness is a **return value, never a throw**.

### Persistence

```ts
engine.export_state(): Uint8Array
```

Call after an `apply` that reported `state_changed`. `Engine.load` restores
**bytes, not a folded mirror** — stage a live head and call `apply` afterwards.
That is the real flow: the tail is never exported, so a client always has to
fetch a head anyway.

### Discovery — the only key-accepting entry point

```ts
engine.discover(ownerHex: string, key: Uint8Array /* 32 bytes BE */): string
engine.forget_owner(ownerHex: string): void
```

Returns the canonical `SyncReport` JSON, field-identical to
`strk20-sync sync --json`: `notes`, `balances`, `newly_spent`,
`incoming_senders`, `outgoing_recipients`, completion flags, `history_from`,
`snapshot_basis`, `verified`.

`verified` is the integrity grade, and the three values are three different
claims:

| grade | what the client checked | who it trusts |
|---|---|---|
| `"replayed"` | every epoch folded, hash chain intact | the feed, to be internally consistent |
| `"server-asserted"` | snapshot start, reached an anchor in `anchors.ndjson` | a number the **server** published |
| `"anchored"` | that mirror's own recomputed storage root equals one in a `starknet_getStorageProof` from a node the **user** chose | the chain |

Only `"anchored"` puts the indexer outside the trust path. Without it a hostile
indexer can publish an internally consistent **lie** — snapshot, anchors and
hashes all agreeing with each other and with nothing on Starknet.

All three are reachable in a browser. `"anchored"` needs a storage proof, which
is a network call, so — exactly as with the feed itself — **TypeScript fetches
and the module computes**: stage the proof with `stage_storage_proof` before
calling `discover`.

**A staged proof never degrades quietly.** If it disagrees with the mirror,
`discover` throws `ANCHOR_NOT_ON_CHAIN` (and Block B resets the mirror, so the
refuted slot set cannot be built on); if it is mispaired or malformed,
`stage_storage_proof` throws before anything is folded; if nothing consumed it,
`discover` throws `PROOF_UNUSED` rather than returning `"server-asserted"`. A
grade that cannot fail would be unfalsifiable, which is worse than not having
it.

## Key handling

**The viewing key enters the module and never leaves it.**

* It appears in exactly one entry point, `discover`, as bytes.
* It is never returned, never in an error `message` or `details`, never logged —
  this crate has no logging sink, and `SecretFelt` (the one type that holds it)
  renders as `[REDACTED]` and zeroes on drop.
* It is never persisted. The state blob carries feed artifacts only. Discovery
  cursors — which *do* hold key-derived channel keys — live in the module's
  in-memory store and no method exports them.
* The staging buffer is zeroized in place before `discover` returns. Because
  `wasm-bindgen` copies a `&mut [u8]` back out, **that zeroing reaches the
  caller's `Uint8Array`** — the smoke test asserts it.

**Pass the key as a `Uint8Array`, never a string.** JS strings are immutable and
cannot be cleared.

**The honest limit.** The guarantee is **non-transmission**: the module never
sends the key anywhere and zeroes what it owns. It is *not* memory hygiene in the
host. JavaScript cannot reliably zeroize its own buffers, wasm linear memory is
readable by the page, and any copy the caller made before calling is the
caller's problem.

## What TypeScript must do

1. **Fetch** `genesis.json`, `manifest.json`, `epochs/*.zst`, `snapshots/*.zst`,
   `snapshots/*.anchor.json`, `anchors.ndjson`, `head.ndjson`.
2. **Verify the `.zst` sha256 against the manifest BEFORE inflating**, and cap
   the output. The module hashes the compressed snapshot itself, but for epochs
   this check exists only in TypeScript — an unbounded inflate is a zip bomb.
3. **Inflate** with a JS zstd decoder (`fzstd`, ~10 KB gzip). The module cannot.
4. **Stage** the inflated payloads (and, for the snapshot, the compressed bytes
   too), then `apply`.
5. **Persist** `export_state()` in IndexedDB after any `apply` reporting
   `state_changed`; restore with `Engine.load`, then stage a fresh head and
   `apply`.
6. **Ground the mirror in the chain** (ring 6) — see below. Skipping this step
   is what leaves the grade at `"server-asserted"`.
7. **Poll or subscribe**: on a new head, `apply_head(bytes, etag)` then
   `discover` again. A new head moves the candidate blocks, so
   `clear_storage_proofs()` and re-stage.
8. **Never** put the viewing key in a string, a URL, a log, or IndexedDB.

### Ring 6: the two RPC calls, and the binding between them

Against **the user's own Starknet RPC endpoint**, never the feed's:

```ts
const { blocks, pool } = JSON.parse(engine.proof_candidates());
const block = blocks[0];                       // ring 6 asks head-first
if (block === undefined) { /* replayed mirror, or nothing groundable yet */ }

// 1. the proof. `[]` (not null) for the list params — some backends reject null.
const proof = await rpc("starknet_getStorageProof", [
    { block_number: block },   // block_id
    [],                        // class_hashes
    [pool],                    // contract_addresses  <- exactly the pool
    [],                        // contracts_storage_keys
]);

// 2. the block header, for the binding below.
const header = await rpc("starknet_getBlockWithTxHashes", { block_number: block });

engine.stage_storage_proof(BigInt(block), JSON.stringify(proof), header.block_hash);
const report = JSON.parse(engine.discover(owner, key));   // -> verified: "anchored"
```

Both calls are **address-blind**: they name a public pool and a public block, so
the request is byte-identical for every user. Nothing about the viewing key, the
owner address, or the notes leaves the page.

**Why the second call exists.** A public storage-proof endpoint is an anonymous
load-balanced pool — measured earlier in this project, not assumed. Two requests
can land on two nodes, and the second may answer for a lagging replica or a fork
the network dropped. So the proof's `global_roots.block_hash` **must equal**
`starknet_getBlockWithTxHashes(block).block_hash` before its root is believed.

**That rule is enforced inside the module**, not left to the wrapper: both
values pass through `stage_storage_proof`, so it compares them and throws
`PROOF_BLOCK_MISMATCH` on disagreement. Passing a header hash you did not
actually fetch defeats it, and is the one part of ring 6 that TypeScript can
still get wrong.

**What `"anchored"` rests on, precisely.** The user's chosen node reporting
honestly about its own state, and its two answers agreeing about which block
they describe. The module does **not** walk the proof's own Merkle path up to
`global_roots.contracts_tree_root` — deliberate parity with the native client:
the endpoint is the trust anchor by construction, so that walk would only defend
against a lying anchor, which this ring cannot improve on. The load-bearing
check against a *load-balanced* endpoint is the block-hash binding, and that one
is enforced.

**A proof for the wrong block is not silently ignored.** Ring 6 tries several
candidate blocks and treats "nothing staged for this one" as a statement about
the host, not the data — but if the run ends without any staged proof being
consumed, `discover` throws `PROOF_UNUSED` and names both lists.

## Errors

```json
{"code":"FEED_HASH_MISMATCH","message":"…","details":{"epoch":3,"expected":"…","actual":"…"},"retryable":false}
```

`strk20-feed`'s typed errors are downcast out of the `anyhow` chain and projected
onto codes and `details` structurally. `strk20-consumer` has no error enum, so
its errors are matched on the code token at the head of the message and carry
empty `details`. Only `FEED_ADVANCED_MIDSYNC` is `retryable`.

Ring 6's codes: `ANCHOR_NOT_ON_CHAIN` (Block B's verdict — the user's own
endpoint refutes this mirror at every block it could answer for),
`PROOF_BLOCK_MISMATCH`, `PROOF_MALFORMED`, `PROOF_UNUSED`. Every one of them is
a **refusal to report a grade**, never a downgraded one.

## Size

Measured on this build (`./build.sh` prints it):

| artifact | raw | gzip | brotli |
|---|---|---|---|
| `strk20_engine_bg.wasm` | 872 KB | 413 KB | 352 KB |
| `strk20_engine.js` glue | 26 KB | 6 KB | 5 KB |
| **total** | **898 KB** | **419 KB** | **357 KB** |

Add ~10 KB gzip for `fzstd` in the wrapper to reach §3.9's denominator: **~429 KB
gzip total published wire cost**, against a provisional 300 KB budget. **It is
over, by ~43 %.** §3.9 says a breach is a review event, not silent creep — so
here it is, with the cause.

The spike's 231 KB gzip predates the real dependency graph. The dominant cost is
`starknet-types-core` + `lambdaworks-math` (field arithmetic, unavoidable — the
Poseidon and Pedersen hashes are the engine) and `starknet-rust-core`, which
`strk20-consumer` pulls in for exactly **three type definitions** (`BlockId`,
`EmittedEvent`, `StorageResult`) and which drags `url`, `serde_with` and
`serde_json_pythonic` along with it. That last one is the tractable win and it is
not in this crate's gift. Brotli (357 KB, and every browser accepts it) is the
cheap immediate mitigation.

Tried and not worth it: nightly `-Z build-std` with `panic_immediate_abort` did
not beat the stable `opt-level=z` + `panic=abort` + `wasm-opt -Oz` build by
enough to justify a nightly toolchain in the demo path.

## Import section

```
$ node test/imports.mjs
import section: 4 entries, all from ./strk20_engine_bg.js
  __wbindgen_object_drop_ref
  __wbg___wbindgen_throw_bb96b2010945f0bc
  __wbg_Error_408e67f47ca7b58b
  __wbg___wbindgen_copy_to_typed_array_c7f28e53671b41e8
PASS — no network, storage, timer or randomness import
```

**Stated at its real strength**: the section is not empty, and those four are
calls *into* JS carrying arbitrary bytes — they are how every ABI method returns
its JSON. What the audit proves is that the module **cannot open a network
handle, a storage handle, a timer, or a randomness source of its own**. It can
only hand bytes to the wrapper.

## Layout

```
src/lib.rs      the #[wasm_bindgen] facade (the crate's one unsafe_code allow)
src/staged.rs   StagedFeed — a FeedTransport that fetches nothing
src/drive.rs    one-poll executor + the "never pends" tripwire
src/blob.rs     the state blob container
src/err.rs      §3.7 error codes
examples/       fixture + native golden generator (native only, never in the module)
test/           smoke test (equality) and import audit
```

## Where §A3 turned out to be wrong

§A3 was written against a spike, not against `crates/consumer`. See the crate
docs for the full reasoning; the short list:

* **The per-artifact apply ABI does not exist.** §A3 specifies
  `apply_epoch(payload, manifestEntry)` / `apply_snapshot(payload, …)`.
  `strk20-consumer` has no byte-level apply — it has `apply_feed(store,
  transport, cold_start)`, one incremental pass that *pulls*. Hence
  `stage_*` + `apply`.
* **The §3.5 state blob is not writable from outside the crate.**
  `ConsumerStore` cannot enumerate events at all and drops slot write blocks, so
  a wrapper cannot emit `s`/`b`/`ev` lines. This blob carries verified artifacts
  and re-folds them instead — which also re-verifies, and a folded blob would
  not have.
* **`FeedError` does not spell the §3.7 codes.** §3.7 claims a 1:1 mapping;
  `Display` spells only `DECOMPRESS_LIMIT`. `src/err.rs` is where the mapping
  actually lives.
* **`getrandom` is in the tree.** §3.9's dependency-walk gate fails as written.
  The import audit is the check that actually holds.
* **Not built (out of scope for the demo):** §3.6 sealed per-key state and its
  entropy contract, `history()` (no such API in Block B), and
  `export_reference_cursor` (needs the sealed blob).
