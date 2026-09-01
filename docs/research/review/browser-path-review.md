# Browser-path adversarial review — crates/wasm, ring 6, ts/

**Date:** 2026-09-01 · **Branch:** `proto/keyless-indexer` · **Mode:** read-only, nothing fixed.

**Surface:** `crates/wasm` (package `strk20-engine`), the anchored/ring-6 path in
`crates/consumer/src/anchors.rs`, `ts/strk20-discovery`, `ts/demo`.

**Why this review exists.** This surface was built by single agents with no review phase; the one
workflow that would have reviewed the TypeScript was cancelled before its review ran. It is also
where the privacy claims cross into the browser and into other people's hands.

**Process note.** Two skeptics were dispatched. Skeptic 1's input findings list arrived empty, so it
derived candidates from the six stated claims and attacked each. Skeptic 2 received an empty list
too and correctly declined to adjudicate nothing, returning only a bounded sanity pass. As editor I
re-verified every load-bearing claim below against the working tree myself; dispositions are mine,
attribution is noted per finding.

**Tree state caveat.** Another agent was actively editing `ts/**` during this review. Line numbers
are as of the working tree at review time.

---

## Findings, most severe first

### 1. CRITICAL — the `block_hash` binding is self-referential: the module never checks the proof against a hash it knows

**Where:** `crates/wasm/src/proofs.rs:150-162` (`StagedProofs::put`);
`crates/consumer/src/anchors.rs:204-218` (`ground_mirror_against_rpc`);
`crates/consumer/src/mem.rs:166-168` (`MemStore::block_hash`);
`crates/consumer/src/apply.rs:304` (`head_hash` meta write).

**Disposition:** CONFIRMED. Skeptic 1 rated it HIGH and reproduced it. I reproduced it independently
and rate it CRITICAL, because it falsifies Claim 5 outright and it is the claim that carries the
strongest trust grade.

**The claim it falsifies (Claim 5):** "The `block_hash` binding is enforced INSIDE the module:
`global_roots.block_hash` must equal the header hash before a root is believed, because the proof
endpoint is an anonymous load-balanced pool."

**Failure scenario, reproduced.** Against the shipped `crates/wasm/pkg` and the checked-in fixture:

```js
e.stage_storage_proof(99n, proofFor(anchor.storage_root, "0x0badc0de"), "0x0badc0de");
JSON.parse(e.discover(owner, key)).verified   // => "anchored"
```

The mirror's own hash for block 99 is `0xb10c0063`. It is in the staged `head.ndjson`
(`"head":99,"head_hash":"0xb10c0063"`), in the staged `anchors.ndjson`, and it is written to the
store's meta as `head_hash`. The module is holding the correct answer and awards the strongest
grade to a proof that contradicts it.

**Mechanism.** `StagedProofs::put` compares `result.global_roots.block_hash` against
`block_hash_hex` — two values *both supplied by the caller in the same call*. Neither is ever
compared against anything the module knows. The second line of defence in
`ground_mirror_against_rpc` is guarded:

```rust
if let (Some(chain_hash), Some(stored)) = (
    result["global_roots"]["block_hash"].as_str().and_then(|s| Felt::from_hex(s).ok()),
    store.block_hash(block)?,
) {
```

`MemStore::block_hash` reads the `blocks` map, which for a snapshot-basis cold start with an empty
tail has **no row for the basis block**. The guard silently degrades to a no-op in exactly the
configuration ring 6 exists for — and `mem.rs`'s own `head_hash` meta value, which *is* populated,
is never consulted.

**What the binding actually buys, precisely.** It catches a wrapper that fetches two responses and
pairs them wrongly — `PROOF_BLOCK_MISMATCH` reproduces correctly for that case. It does **not**
catch the threat `proofs.rs` states in its own comment ("two requests can land on two nodes"): if
both land on the *same* lagging replica, the proof and the header agree with each other, disagree
with the canonical chain, and the module returns `anchored`. Nor does it catch a wrapper that reads
`block_hash_hex` out of the proof itself — which is the obvious way to write the wrapper and is
forbidden nowhere in the ABI. What is enforced inside the module is self-consistency of the caller's
two arguments, not a binding to the chain.

This is the same root cause as the three earlier defects in this project: a single-attempt probe
against a nondeterministic aggregating endpoint, trusted because it answered.

**Suggested fix.** One comparison the module can already make: reject at staging (or at consumption)
when the proof's `global_roots.block_hash` disagrees with the mirror's own hash for that block,
falling back to `head_hash` from meta when the `blocks` row is absent. Note this is *not* the same as
making the existing `Some/Some` guard unconditional — that form must stay for blocks the mirror
genuinely cannot answer for. But for `head`, the first candidate `grounding_candidates` emits, the
mirror always can.

**Related, minor, same file:** `ProofSource::storage_proof` keys the map on the caller's `block`
argument, and nothing ties that number to the proof's own subject block. The root recomputation makes
this fail closed in practice, so it is not filed separately — it is the same missing binding seen
from the other side.

---

### 2. MAJOR — both TypeScript packages run ZERO tests and exit green

**Where:** `ts/strk20-discovery/package.json` (`"test": "node --test --experimental-strip-types \"test/**/*.test.ts\""`);
`ts/demo/package.json` (same script).

**Disposition:** CONFIRMED. My own finding — neither skeptic reported it. Skeptic 2 asserted
`ts/strk20-discovery/{src,test,scripts}` exists; the `test/` directory does not exist and, per
`git log --all`, never has.

**Failure scenario.** `npm test` in either package:

```
ℹ tests 0
ℹ suites 0
ℹ pass 0
ℹ fail 0
ℹ duration_ms 7.1
```

Exit code 0. The glob matches nothing and `node --test` treats an empty match set as success. Any CI
step or agent that shells out to `npm test` on this surface gets a green light from a suite that
does not exist.

This is the LIVE-8 failure mode with the volume turned all the way up: LIVE-8 was a silent data loss
a green 123-test suite did not notice; here the suite is empty and still reports green. It also
explains how findings 3 through 6 below survived — nothing on this path was ever executed under
assertion.

**Suggested fix.** Two independent changes, both needed: make the script fail on an empty match set
(`node --test` with an explicit file list, or a guard that counts matches), and write tests for the
paths in findings 3–6, all of which are cheap to assert.

---

### 3. MAJOR — the chain-identity pin is dropped on the wasm path, which is the default engine

**Where:** `ts/strk20-discovery/src/engine-wasm.ts:829-835` (`create`), `:837-859` (`load`),
`:393-397` (`sync_supply` case `'genesis'`); `ts/strk20-discovery/src/profiles.ts` header;
`ts/strk20-discovery/src/engine-mock.ts:747,756,772`; `ts/demo/src/engine-binding.ts:9`;
`ts/demo/src/main.ts:101`.

**Disposition:** CONFIRMED. Skeptic 1 rated MEDIUM; I raise it to MAJOR because it reopens a hole the
code's own documentation says it closes, on the path that ships, reachable from a URL parameter.

**The claim it falsifies:** `profiles.ts` states the profile values "are the identity the client PINS
BEFORE a byte is requested, which is what closes the trust-on-first-use hole: an empty mirror must
not adopt whatever chain the feed declares (§3.10 item 3)."

**Failure scenario.** The mock enforces the pin — `engine-mock.ts:747` throws `CHAIN_MISMATCH` when
the feed genesis `chain_id`/`pool` differ from the profile, `:756` checks the geometry, `:772` checks
the manifest. The wasm adapter does not. Both factory entry points discard the profile:

```ts
async create(profileJson: string): Promise<Engine> { void profileJson; … }
async load(profileJson: string, frames): Promise<Engine | null> { void profileJson; … }
```

and `sync_supply` case `'genesis'` does `this.#inner = new this.#glue.Engine(this.#genesisJson!)` on
whatever the feed served, with no comparison. Grepping all of `ts/**/src`, `#profile` is used only
for `databaseName` and `deriveKeyId` — never compared against `info().chain_id`/`pool`.

`ts/demo/src/engine-binding.ts` makes wasm the default ("The REAL engine is the default"), and
`ts/demo/src/main.ts:101` takes the feed base from a `?feed=` query parameter. So: point a client at
a feed for a fabricated pool and it folds it, and reports `verified: "replayed"` — the grade that
means "folded from genesis" — for a genesis the attacker wrote. Self-consistency is all that grade
then attests.

**Suggested fix.** Port the mock's three checks into the wasm adapter: compare feed genesis
`chain_id`/`pool`/`genesis_block`/`epoch_size` against `profileJson` before constructing the engine,
and against `info()` after `load`. `CHAIN_MISMATCH` already exists in the error set and `load`
already treats it as a cache miss.

---

### 4. MAJOR — the grade the user sees is computed in TypeScript, and `anchored` is unreachable on the shipped path

**Where:** `ts/strk20-discovery/src/engine-wasm.ts:324-327` (`#grade`), `:587` (`#fold` →
`StepDone.verified`), `:76,83` (unused `WasmEngine` declarations); `ts/strk20-discovery/src/client.ts:154-159`,
`:298`.

**Disposition:** CONFIRMED as an accuracy defect. Skeptic 1 rated MEDIUM; I keep MEDIUM/MAJOR because
the gap is between the shipped code and a commit message, not a break in the code's own logic.

**Failure scenario.** The grade reaching `FeedState.verified` and `status().verified` is computed in
TypeScript, not read from the module:

```ts
#grade() {
  const basis = this.#info()?.snapshot_basis ?? null;
  return basis === null ? 'replayed' : 'server-asserted';
}
```

`'anchored'` is unreachable from that function by construction. Corroborating: `stage_storage_proof`
and `proof_candidates` appear in `ts/` only as declarations on the `WasmEngine` interface — no call
site anywhere in `ts/**/src`; `client.ts:154-159` throws `CONFIG_INVALID` on `anchorRpcUrl` or
`anchorPolicy: 'require'`; `sync_supply_rpc` throws "this engine emits no RPC step; ring 6 is not
wired in this build".

So commit 1195257's message — "TypeScript fetches the storage proof from the user's own RPC and
stages it exactly like every other artifact" — is not true of the `ts/` tree under review. Both files
are internally honest about this in comments; the claim as carried in the commit log is not. This is
not a downgrade attack. It is that the strongest grade cannot be earned on the shipped browser path,
and a reader of the commit message would believe otherwise.

**Suggested fix.** Either wire ring 6 (call `proof_candidates`, fetch from the user's RPC, call
`stage_storage_proof`, and read the grade out of the module rather than recomputing it), or retract
the claim in the docs and commit log and have `#grade` read the module's own verdict so the two can
never drift.

---

### 5. MINOR — the invariant gate is currently RED, so it can no longer catch a real secret

**Where:** `ts/demo/dist/assets/index-B1cU2wDj.js:4` (tracked build output, committed at 9ff21be);
`ts/demo/.gitignore` (uncommitted addition of `dist/`); `ts/strk20-discovery/dist/**` (also tracked).

**Disposition:** CONFIRMED, my own finding, and it is a *change since the skeptics ran*. Skeptic 2
recorded check 4 as `[PASS] 190 tracked files`. At my run it is `[FAIL] 322 tracked files`, and the
script exits nonzero.

**Failure scenario.** `ts/demo/dist/` was committed. The bundle inlines the demo fixture identities:

```
address:"0x04f2…0a9b", viewingKey:"<64 hex>"
```

The key is elided above and that elision is part of the point: check 4 reads *this* file too, and its
shape rule is `viewingKey:` followed by 40-plus hex characters, whoever wrote them and for whatever
reason. Quoting the value in full — which this section did until CI started running the check on
every push — made the review of a permanently-red gate the thing keeping it red. The literal itself
lives, on purpose and with a comment saying why, in `ts/demo/fixtures/replay-identities.json`.

These are fixture keys, not real key material, so there is no disclosure. The defect is the gate:
`scripts/check-invariants.py` now fails on every run, and a check that is always red stops being
read. The next run that *would* have caught genuine key material is indistinguishable from this one.
Adding `dist/` to `.gitignore` (which the concurrent edit does) does not untrack already-committed
files.

**Suggested fix.** `git rm -r --cached ts/demo/dist ts/strk20-discovery/dist` alongside the
`.gitignore` change, so the gate goes green and stays meaningful. If the built demo must be tracked
for publishing, exempt that path in check 4 explicitly and say why — an acknowledged exemption is
readable, a permanently failing check is not.

---

### 6. MINOR — the decompression cap is checked after decompression, so it is not a bound

**Where:** `ts/strk20-discovery/src/client.ts:889-902` (`inflateWithin`); obligation stated at
`crates/wasm/src/lib.rs:22-23`.

**Disposition:** CONFIRMED (skeptic 1, LOW-MEDIUM; I concur at MINOR).

**Failure scenario.**

```ts
const out = decompress(compressed);
if (out.length > cap) throw new Strk20Error('DECOMPRESS_LIMIT', …);
```

fzstd has already allocated and written the full output before the comparison. `lib.rs:22-23` states
the obligation as "check the `.zst` sha256 before inflating, **and cap the output**" — the first half
is honoured, the second is a post-mortem. Exploiting it needs a hostile feed publishing a manifest
whose `zst` hash matches a zstd bomb (`SNAPSHOT_CAP` is 512 MiB, so the useful attack declares far
more), which is inside this project's threat model since the feed is untrusted by construction.
Impact is a tab OOM, not a disclosure — hence the modest severity.

**Suggested fix.** fzstd exposes a streaming `Decompress` API that can abort on the declared frame
size, or on accumulated output crossing `cap`, before allocating it.

---

### 7. MINOR — a poisoned artifact cache never heals

**Where:** `ts/strk20-discovery/src/client.ts:363` (`#satisfy`), `:427-431` (prefetch persist),
`:728` (`resetCache`, the sole `artifactClear()` caller), `:881` (`FEED_HASH_MISMATCH`), `:246`
(`stateClear()` on load failure, for contrast).

**Disposition:** CONFIRMED (skeptic 1, LOW; I concur at MINOR).

**Failure scenario.** `#satisfy` returns IndexedDB bytes, `verifyServedHash` throws
`FEED_HASH_MISMATCH`, and nothing evicts the row. §4.5's invalidation table is honoured for the
folded blob (`stateClear()` on a load failure) but not for `artifacts`. Worse, prefetch hints are
persisted without ever having been hash-checked:

```ts
if (s.source === 'network') await storage.artifactPut(pathKey(path), { hash: '', zbytes: s.compressed });
```

Note the `hash: ''`. One corrupt response — including an unconsumed prefetch hint — becomes a
permanent sync failure until the caller happens to call `resetCache()`.

**Suggested fix.** Call `artifactDelete(pathKey(path))` on the `FEED_HASH_MISMATCH` path before
rethrowing, and either hash-check prefetch bytes before persisting them or store them with the
manifest's expected hash so a later read can reject them cheaply.

---

### 8. MINOR — `PROOF_UNUSED` over-enforces on a replayed mirror, and its error text mislabels the grade

**Where:** `crates/wasm/src/proofs.rs` (`PROOF_UNUSED` construction).

**Disposition:** CONFIRMED as over-enforcement (skeptic 1, LOW). Fail-closed, so low.

**Failure scenario.** A mirror replayed from genesis — the strongest provenance in the system — plus
a staged proof throws `PROOF_UNUSED` instead of returning a report (reproduced). A defensive caller
that always stages a proof gets an exception on the one mirror that needed no proof. The error text
then tells the caller to `clear_storage_proofs()` "to accept the weaker grade", which mislabels
`replayed` as weaker than `anchored`.

**Suggested fix.** Treat `replayed` as a terminal success for a staged proof rather than an unused
one, and reword the message so it does not rank `replayed` below `anchored`.

---

## Refuted — attacks that were tried and failed

Recorded so they are not re-derived. Each was attacked against the shipped `crates/wasm/pkg` from
Node, or by exhaustive grep of `ts/**/src`, not merely read.

| # | Attack | Why it failed |
|---|--------|---------------|
| R1 | **Claim 1** — the module has a capability its import audit misses | `node crates/wasm/test/imports.mjs`: 4 entries, all from `./strk20_engine_bg.js`. The audit rejects on `i.module !== GLUE \|\| i.kind !== "function"`, so a memory/table/global import or a second module fails it too. The "`getrandom` is dead code" argument follows correctly from the absence of a `crypto.getRandomValues` import. No capability found that this misses. |
| R2 | **Claim 2** — the viewing key escapes on some path | Measured on five paths: success, `PROOF_UNUSED` throw, invalid `owner_hex` (early return), wrong-length key, and `export_state()`. Buffer all-zero afterwards in every case. The wrong-length message carries the length only ("got 3"), no bytes. No 8-byte window of the key appears anywhere in the 17197-byte state blob. The zeroize sits outside `discover_inner` (`lib.rs:479-485`) so it runs before the `Result` is converted; the intermediate `raw: [u8;32]` is zeroized at `lib.rs:498`. Error messages come from the closed `CODES` set in `err.rs`; no logging sink exists in the crate. |
| R3 | **Claim 3** — wasm and native reports diverge | `node crates/wasm/test/smoke.mjs` PASS: `JSON.stringify(report) === JSON.stringify(golden)` for 2 owners × 2 cold-start modes plus an export/load round trip, with a non-vacuity guard. **Scope note, not a finding:** the equality is demonstrated over one fixture — 48 slots, one epoch, 2 owners, 1 note. That supports "the same code, recompiled". It does not support byte-identity over arbitrary feeds, and the claim should not be stated more broadly than the evidence. |
| R4 | **Claim 4, Rust side** — a silent downgrade to "server-asserted" | All four refusals reproduced by name: disagreeing proof → `ANCHOR_NOT_ON_CHAIN`; mispaired proof → `PROOF_BLOCK_MISMATCH`; non-candidate block → `PROOF_UNUSED`; replayed mirror → `PROOF_UNUSED`. No input produced a quiet fall-back. (Findings 1 and 4 are about a *different* failure: a forged proof earning `anchored`, and the grade being recomputed in TS.) |
| R5 | **Claim 6a** — decompression happens before the hash check | `client.ts:338-339` runs `verifyServedHash(got.compressed, …)` and only then `inflateWithin`. IndexedDB hits go through the same line (the check is on `got.compressed` regardless of `got.source`), so a cached artifact is re-verified on every read. Holds. |
| R6 | **Claim 6b** — a user-derived value reaches a URL, header, log or store | `net.ts` is the sole IO chokepoint; `FEED_PATH_ALLOWLIST` is whole-path anchored at both ends, so a query string is unmatched rather than merely forbidden; `request()` sets only `Accept` and `If-None-Match`, with `credentials:'omit'`, `redirect:'error'`. `keyId` is HKDF-SHA256 (salt `strk20-idb-keyid-v1`, info chain‖pool‖owner), full 32 bytes — key-*derived* and unguessable without the key, not key-revealing; `kdf.ts` zeroes prk/ipad/opad/okm. `delegatedPost`'s `Authorization` header is on a function the keyless path cannot reach. The demo's `console.error(what, e)` (`main.ts:1011`) logs `Strk20Error`s whose fields come from the closed code set. Holds. Note this is also what `check-invariants.py` check 1 mechanises, and it passes. |
| R7 | **Claim 6c** — "every client's feed requests are byte-identical" | **Overstated rather than defective.** `If-None-Match` on `/head.ndjson` makes requests literally non-identical, and a server can mint a per-client ETag. But `storage.ts:16` and `client.ts:861` both state the ETag is never persisted, and `cacheable()` excludes `'head'`, so it dies with the tab — not an ETag supercookie. Within a session the server already has the connection. Not filed; the *wording* of the claim should be tightened to "carry nothing user-derived", which is what actually holds. |
| R8 | `is_proof_unavailable` swallowing a real refutation | The only `StagedProofs` error matching its substring set is `PROOF_NOT_STAGED`, the intended one. The contract-address mismatch bail (`proofs.rs:231`) matches none of the tokens and correctly propagates. |
| R9 | Ring-6 candidate drift between the wrapper and Block B | `proof_candidates()` calls the same `grounding_candidates` the sync path calls, so the two cannot diverge. `MAX_GROUNDING_CANDIDATES = 4` bounds what a feed can dictate. |
| R10 | `clear_storage_proofs()` not clearing `asked` | Cosmetic only — `begin()` clears it per discover, so the `PROOF_UNUSED` message always describes the current run. Not a finding. |

### Claims that survived a real attack

Worth recording as such, since a survived claim is evidence and not merely an absence:

- **Claim 1 (pure synchronous computer).** Survived. The four-import evidence is sound and the audit
  that produces it is stronger than the claim needs.
- **Claim 2 (the viewing key never leaves).** Survived every path I could construct, including three
  error paths and the exported state blob. The README's stated limit — non-transmission, not host
  memory hygiene — is the honest one, and it is stated.
- **Claim 3 (byte-identical reports).** Survived, over one fixture. True as far as tested; the
  evidence is narrower than the sentence sounds.
- **Claim 4 (no silent downgrade), Rust side only.** Survived.
- **Claim 6, sub-claims a and b.** Survived.

**Claim 5 is falsified** (finding 1). **Claim 4 does not hold of the TypeScript** (finding 4). The
pin that `profiles.ts` claims closes trust-on-first-use is absent on the default engine (finding 3).

---

## Note on the concurrent edit

`scripts/check-invariants.py` check 3 (`WARN`, 5 hits) is entirely outside this surface — all five
are in `crates/indexerd/**`. Any finding citing that WARN as evidence against the browser path is
misattributed on its face; none here does.
