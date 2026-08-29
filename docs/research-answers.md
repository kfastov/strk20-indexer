# STRK20 Open Note Indexer — research answers

**Date:** 2026-08-29 · **Answers:** [research-open-questions.md](research-open-questions.md) (Q1–Q20)
**Method:** 9 parallel research agents over the upstream code (`starkware-libs/starknet-privacy`, tags RC.0…RC.5 + deployment tags), the live mainnet pool (raw JSON-RPC), and the hackathon repo; 3 adversarial verifiers re-derived every load-bearing claim independently; 1 completeness critic adjudicated cross-agent conflicts. Detailed evidence with file:line references and raw RPC data lives in [research/](research/).

Every claim below is marked **VERIFIED** (observed in code or on-chain during this investigation) or **INFERRED** (grounded, not directly reproduced). All three verifier reports returned CONFIRMED or PARTIAL-with-minor-corrections; corrections are already folded in here.

---

## 0. Verdicts at a glance

| Q | Question | Verdict |
|---|---|---|
| Q1 | Exact mainnet version | **Answered.** Two classes ever deployed; full version table below. Upstream README's class hash for mainnet is wrong. |
| Q2 | Keyless local discovery reproducible? | **YES** (verified by full code trace + adversarial re-trace). |
| Q3 | Minimal public dataset | **Snapshot + diffs** (≡ full diff replay from pool deployment). Append-only diffs after an arbitrary cursor are NOT enough for initial sync; arbitrary `getStorageAt` is NOT needed once a full pool mirror exists. |
| Q4 | Why an indexer beats a raw node | **Quantified.** Reference: ~2 storage reads/note/user/session, ~1 s @1125 notes, 7–9 req/s ceiling per RPC node, per-user reads uncacheable. Indexer: O(1 RPC call per active block) once, for everyone. |
| Q5 | Ingest from ordinary RPC? | **YES.** `starknet_getStateUpdate` (spec 0.8.1) has everything incl. upgrade signals; two free public archive endpoints reach pool genesis. No server-side per-contract filter — irrelevant at these sizes. |
| Q6 | Public classifiability | **Answered precisely.** Note creations/spends/open-note legs ARE public (events); channels/subchannels are opaque and event-silent; enc-note token/amount/parties hidden. |
| Q7 | Privacy of keyless targeted slots | **Weaker than assumed:** incoming-channel slots are keyed by the public recipient address → targeted mode de-anonymizes the first sync. Bulk/epoch mode is the honest privacy mode. |
| Q8 | Bulk epoch sync practical? | **Trivially yes.** Full history ≈ 19 MB raw / ~6 MB zstd; ~80 KB/day now; <5 MB/day at the historical peak. |
| Q9 | PIR needed? | **Not now.** Trigger condition defined (~8×10⁵ records + demand for cryptographic point-lookup privacy). Prefix-bucket endpoint is the near-free halfway. |
| Q10 | Keyless spent-state | **Solved.** Nullifier formula pinned; `NoteUsed` event exposes the nullifier verbatim; spent-state maintainable incrementally from events or diffs. |
| Q11 | Trust model | **Storage proofs work on public RPC today** (verified live) → spot-check verification is real; stream completeness is not provable → mirrors + content addressing. |
| Q12 | Reorgs/finality | ACCEPTED_ON_L2 is revocable **en masse** (Grinta precedent ~2 h); l1_accepted lag ≈ 3 h; epoch bundles cut only ≤ l1_accepted. |
| Q13 | Upgrades | `upgrade_delay = 0`, not finalized → instant surprise upgrades are how it actually happened once already. Detection via `replaced_classes` + `ImplementationReplaced`. |
| Q14 | Compatible mode | **Reuse, don't fork:** implement 4 small async traits over a local DB and the unmodified discovery-core engine (optionally the whole reference `ApiServer`) runs on top. Wire format frozen RC.0→RC.5. |
| Q15 | Client library | **Wrap the crate:** discovery-core compiles to wasm32-unknown-unknown untouched (verified empirically). Rust core → wasm → 3-method TS `DiscoveryProviderInterface` adapter. |
| Q16 | Push design | Global diff/event stream is both privacy-optimal AND cheap (<1 MB/day). Per-user slot subscriptions are a continuous fingerprint — opt-in only. |
| Q17 | DB choice | (Gap closed here, §17) **SQLite + content-addressed epoch files** by default; Postgres as an optional backend behind the same trait. Measured volumes don't justify more. |
| Q18 | Explorer metrics | Honest set defined (volumes/TVL per token, note count = anonymity set, spend count, DeFi-interaction breakdown, indexer health). Harmful set defined (address joins, per-tx timelines, per-token enc-note claims). |
| Q19 | Success metrics | Concrete B1–B10 benchmark table with baselines from upstream's own measurements. |
| Q20 | Mainnet demo | Fully mapped: scoring is machine-checked; 3 txs can legitimately be made via the official app; no competing IDEA-23 project exists. |

---

## Q1 — Exact deployed mainnet version (VERIFIED on-chain)

Pool: `0x040337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a`

| Block range | Class hash | Source pin | Notes |
|---|---|---|---|
| < 8,978,970 | — | — | contract does not exist |
| 8,978,970 – 11,632,885 (2026-04-20 → 2026-07-09) | `0x30b8c540…4b4b30b` | tag **`PRIVACY-0.14.2-RC.3`** = **`CONTRACT_V1_DEPLOYED_MAINNET_2026-04-20`** | "pre-screening / compatibility" pool; hash→tag binding verified in that tag's README + SDK `pool-mode.ts`; on-chain ABI matches field-for-field |
| 11,632,886 – head (2026-07-09 → now) | `0x67dddd89…76b554d` | tag **`CONTRACT_V2_DEPLOYED_MAINNET_2026-07-08`** (commit `74841ca`; contract source byte-identical to `PRIVACY-0.14.3-RC.3`) | "screening" pool; live ABI matches member-for-member; hash itself recorded nowhere upstream |

- Deployment block **8,978,970** (`deployed_contracts` in that block's state update). Exactly **one** upgrade ever: block **11,632,886**, tx `0x4be26fa7…2ad36` — confirmed both by class-hash bisection and by a full-history `ImplementationReplaced` event scan (exactly 1 occurrence).
- **Upstream README is wrong for mainnet:** `0x52107fad…633` (labelled PRIVACY-0.14.3-RC.0) was never deployed at this address, at any block. Never key logic off README hashes; read the chain.
- The upgrade tx itself: sender granted itself UpgradeGovernor and executed add+replace **in one transaction** (`upgrade_delay = 0`).
- Matching SDK for the live pool: published `@starkware-libs/starknet-privacy-sdk@0.14.3-rc.5` is ABI-compatible with the deployed class.
- **Live event schema is authoritative from the on-chain ABI** ([research/data/live-pool-abi.json](research/data/live-pool-abi.json)): 14 pool events incl. `ExternalContractInvoked` (absent in RC.0!). The **7 discovery-relevant events (`Deposit`, `Withdrawal`, `ViewingKeySet`, `OpenNoteCreated`, `EncNoteCreated`, `OpenNoteDeposited`, `NoteUsed`) are byte-identical across both deployed classes** → one decoder covers the entire pool history from block 8,978,970.

## Q2 — Keyless local discovery: YES (VERIFIED, adversarially confirmed)

The reference discovery-service holds **zero private state**: the wallet POSTs its raw viewing key in every request body (`viewing_key` field of every `/v1/sync/*` and `/v1/history` request), and the server just runs `discovery-core` against `getStorageAt`/`getEvents`. Every flow (incoming, outgoing, preflight, history) is a **pure function of (viewing_key k, user_addr, pool storage slots, pool events)** — a client with the key and the same public data reproduces results bit-identically.

Key mechanics (all formulas cross-checked Rust ↔ Cairo ↔ fixture vectors, no mismatches):

- Single secret `k` (Stark-curve scalar). All decryption is additive poseidon-masking (`plain = cipher − H(tag, secret…)`); ECDH only for incoming channel info.
- `nullifier = H(NULLIFIER_TAG:V1, channel_key, token, note_index, 0, k_owner)`; `note_id = H(NOTE_ID_TAG, channel_key, token, note_index, 0)`; `channel_key = H(CHANNEL_KEY_TAG, sender, k, recipient, recipient_pk)`; slots use standard Cairo addressing (`sn_keccak(name)` + pedersen per key).
- Termination conditions: incoming channels — explicit on-chain Vec length counter; outgoing channels & subchannels — `salt == 0` sentinel; notes — exponential probe + bisection to first zero slot.
- Fetch-decrypt-fetch depth (verifier refinement): incoming is **two-stage** (channel info → token → note/nullifier slots); outgoing channel_key needs a `public_key[recipient]` read at an address depending on a decrypted value. Consequence: **a keyless server can never precompute per-user slot lists — but every read is a point read of one contract's storage, so a full pool-storage mirror answers everything.**
- **No viewing-key rotation exists**: `public_key`/`enc_private_key` are write-once (assert-zero-before-write). One key per address, forever.
- One non-standard reference dependency: `getStorageAt` with `response_flags=[INCLUDE_LAST_UPDATE_BLOCK]` (patched Pathfinder + starknet-rust fork). Public RPCs reject it (measured). A diff-stream indexer gets last-update blocks **for free** — an argument for this project, not a blocker.

## Q3 — Minimal dataset: snapshot + diffs (VERIFIED)

- The contract's write discipline makes pool storage an **append-only accumulating map**: all discovery slots are write-once except the channel-Vec length (increments) and a single open-note funding rewrite. Nothing is ever zeroed. Replaying `storage_diffs[pool]` from deployment reproduces `getStorageAt(latest)` exactly and yields per-slot write blocks.
- **Initial sync** needs state written before any cursor (old channel elements, counters, public keys) → full replay from deployment or a snapshot. Deployment is recent; full replay ≈ 19 MB raw (§Q8).
- **Resume from a block cursor works from diffs alone**, no rescan — with the operational caveat that the client's watch-set grows as decryption reveals new channels/notes, so mirror-everything beats fixed slot subscriptions.

## Q4 — Why the indexer wins (measured upstream + our projections)

| Claim | Reference route | With indexer | Status |
|---|---|---|---|
| Chain reads per sync | ~`2×notes + 3×channels + probes` per user **per session** (upstream spec: ~2250 reads @1125 notes) | **0 at query time**; ingest = 1 `getStateUpdate` per active block, once, for all users | upstream MEASURED / ours structural |
| Latency | ~0.93–1.03 s @1125 notes on a dedicated RPC node; 0.37–0.41 s per read on public RPC (measured) | indexed local query, target <50 ms p50 | upstream MEASURED / ours to benchmark |
| Throughput | 7.0–8.9 req/s **per RPC node** (node is the bottleneck) | DB-bound; RPC load O(blocks), independent of users | upstream MEASURED |
| Caching | "little overlap between connections" — per-user reads uncacheable (upstream's own finding) | keyless slabs/epochs are user-independent → CDN-cacheable | upstream MEASURED / ours structural |
| Key custody | viewing key uploaded per request; server sees decrypted notes | keyless: key never leaves the wallet | both VERIFIED in code |
| Note maturity data | needs a **forked RPC extension** unavailable on public endpoints | write blocks known natively from diffs | VERIFIED |
| Ecosystem | no public hosted discovery endpoint exists; SDK's no-indexer fallback un-exported (issue #121); teams asking for `INDEXER_URL` (issue #221) | self-hosted + hosted instance | VERIFIED from issues |

Honest framing: the indexer does not reduce cryptographic work (trial decryption is the wallet's either way); it eliminates the per-user chain-read bill and the key-custody requirement, and turns discovery from O(state) probing into O(delta) feed consumption.

## Q5 — Ingestion from ordinary RPC: YES (VERIFIED with live measurements)

- `starknet_getStateUpdate` (spec 0.8.1) returns per-block `storage_diffs` incl. the pool's `(key, value)` pairs, plus `deployed_contracts` / `replaced_classes` (both verified at the actual deploy and upgrade blocks).
- **No server-side per-contract filter in standard JSON-RPC** — you fetch whole-block diffs and filter locally. Whole-block diffs are tiny (median ~4 KB). The efficient pattern is **events-first**: `getEvents(address=pool)` finds active blocks (0.23–0.25% of all blocks), then `getStateUpdate` only for those.
- Provider reality (2026-08-29): **lava** (`rpc.starknet.lava.build`, spec 0.8.1) and **publicnode** (`starknet.publicnode.com`, spec 0.10.2) both serve full archive depth back to pool genesis — verified independently by two agents on different blocks. blastapi is dead (403 → "use Alchemy"), free-rpc.nethermind.io does not resolve, 1rpc rate-limits after ~1 call. → **No self-hosted archive node is required for backfill**; archive your own raw ingest as insurance.
- Full-history `getEvents` scan: **118,372 events in 28,260 active blocks over 131 days**, ~5 minutes on lava. All selectors mapped ([research/data/selector_map.json](research/data/selector_map.json)).
- Optional upgrades: Apibara DNA has contract-filtered storage-diff streaming (hosted, API key); Pathfinder adds only proof extensions. Neither is needed at current scale.
- Integrity anchor: `starknet_getStorageProof` returns the pool's `storage_root` → periodic completeness self-check of the reconstructed mirror.
- Implementation notes: lava 403s Python-urllib's default User-Agent (set a real one); a per-block entry tail reaches ≥68 entries (size buffers accordingly).

## Q6 — What a public observer can classify (VERIFIED against deployed source + live ABI)

**The classifiability rule:** a raw `(slot, value)` write is classifiable iff the slot's `(var_name, key)` preimage is composed only of public felts. 15 non-component storage vars: **9 publicly classifiable** (4 address-keyed maps: `public_key`, `enc_private_key`, `recipient_channels`, `blocked_open_note_depositors`; 5 config singletons), **6 opaque** (`outgoing_channels`, `channel_exists`, `subchannel_tokens`, `subchannel_exists`, `notes`, `nullifiers` — all keyed by hashes folding a secret).

| Quantity | Public? | Source | Caveat |
|---|---|---|---|
| Shields count/amount/token | **YES** | `Deposit` event (plaintext u128 + token key) | depositor address public — shielding is not private |
| Unshields count/amount/token | **YES** | `Withdrawal` event | `to_addr` public; in-pool source auditor-encrypted |
| Note creations | **YES** | `EncNoteCreated` + `OpenNoteCreated` events | count only; amounts/parties/token(enc) hidden |
| Nullifier uses (spends) | **YES** | `NoteUsed` event — **nullifier emitted verbatim as key** | count public; linkage to note/owner cryptographically hidden |
| Open-note funding | **YES** | `OpenNoteDeposited` (depositor, token, amount public) | |
| DeFi interactions | **YES** (post-upgrade only) | `ExternalContractInvoked` (target + selector; no calldata) | absent before block 11,632,886 |
| TVL per token | **YES** | deposits − withdrawals, or pool `balanceOf` | |
| Channel/subchannel creations | **NO** | — | **event-silent** + secret-keyed slots; counting unreliable |
| Token of an encrypted note | **NO** | — | zeroed in storage, absent from event, masked in subchannel |
| Per-token anonymity set | **PARTIAL** | open-note subset only | enc notes are one global bucket across tokens — splitting them per token would be fabricated precision |
| Cross-leg linkage (shield↔note↔spend↔unshield) | **NO** | — | the point of the pool |

**Correction to this project's README:** "the pool emits events only for deposits, withdrawals and key registration; notes leave no events" is **FALSE**. Note creation, note spend, and open-note funding all emit commitment events. Only **channel/subchannel** creation is genuinely event-silent — which means storage-diff ingestion (not just events) remains necessary, and is exactly what generic event indexers cannot see.

## Q7 — What keyless targeted-slot lookup actually provides (VERIFIED)

Storage slots split into **Class A** (keyed by public address: `public_key`, `enc_private_key`, `recipient_channels[*]`) and **Class B** (keyed by secret-derived commitments: markers, note_ids, nullifiers).

- **Incoming discovery must read Class-A slots** — the channel-count and channel-element slot addresses are pure functions of the recipient's public address. **A keyless targeted sync therefore reveals WHO is syncing on essentially every enumerating sync.** No targeting trick avoids it; only bulk/epoch or PIR removes it.
- Keyless targeted mode removes exactly two things vs the reference service: the viewing key and server-side decryption. It does NOT remove: address leakage (incoming path), slot-fingerprint linkability across syncs, note-count-via-request-size, timing. A counterparty who knows a shared channel_key can even de-pseudonymize nullifier probes.
- **Keyless-targeted == direct RPC `getStorageAt` in leakage** — no worse, no better.
- OHTTP only decouples IP from content (upstream's own spec says so); it hides nothing the responder computes from the request.

Leakage table (visible-to-whom):

| Data | Compatible | Keyless targeted (= direct RPC) | Keyless bulk/epoch | PIR |
|---|---|---|---|---|
| Viewing key | **operator** | nobody | nobody | nobody |
| Decrypted amounts | **operator** | nobody | nobody | nobody |
| Exact queried slots | operator (derives) | **operator** | nobody | nobody |
| User public address | **operator** | **operator** (incoming path) | nobody | depends |
| Client IP | operator (unless OHTTP) | operator (unless OHTTP) | operator/CDN | operator |
| Sync timing | operator | operator | coarse (epoch pulls) | operator |
| Note count | operator | operator (request size) | nobody | mostly hidden |

**Consequence:** bulk/epoch is the headline privacy mode; targeted mode is a documented lower-privacy convenience.

## Q8 — Bulk epoch sync: trivially practical (MEASURED, independently re-measured)

| Quantity | Value |
|---|---|
| Chain cadence | 1.67 s/block ≈ 51,650 blocks/day |
| Pool activity (7-day avg) | ~120 active blocks/day, ~526 events/day (~0.23% of blocks) |
| Pool-only diffs | mean 671 B raw JSON per active block (~156 B/entry; zstd ≈ ÷3.2) |
| **Bytes/day now** | **~80 KB raw / ~25 KB zstd** |
| Historical peak (sliding window, ~59× current) | ~4.9 MB/day raw |
| **Full backfill (131 days of history)** | **~19 MB raw / ~6 MB zstd** |
| At 100× lifetime activity | ~1.9 GB / ~0.6 GB |
| Full event archive | 69 MB raw / 10.4 MB zstd |
| Whole-block (unfiltered) firehose | 200–240 MB/day raw / ~30 MB/day zstd |

Epoch design: 10k blocks (~4.6 h) or 50k (~1 day); bundle = compact NDJSON (or 32-B binary felts) of pool storage_diffs + `replaced_classes` entries + the pool `storage_root` from `getStorageProof` at the epoch head as integrity anchor; zstd per epoch; content-addressed; **cut only ≤ l1_accepted** (§Q12). Even mobile clients can pull full epochs for years of 10–100× growth — no finer-grained filtering needed.

## Q9 — PIR: not justified now (surveyed; verdict with trigger)

- 2025–26 landscape mapped (SimplePIR/DoublePIR, FrodoPIR, ChalametPIR, Spiral, Respire, YPIR, HintlessPIR, Piano/Plinko, 2-server DPF). **No PIR scheme has a maintained production-grade Rust implementation in 2026.** Only ChalametPIR is natively keyword-PIR; pseudorandom slot keys make prefix-bucketing the natural index-free reduction.
- Steady-state sync never needs PIR (deltas are KB/day). Cold-start full snapshot fits a 50 MB mobile budget up to ~8×10⁵ records — orders of magnitude above current N.
- **Cheap halfway:** `GET /slots?prefix=P&bits=k` — Pedersen-uniform buckets, client-chosen k trades bandwidth vs k-bit leakage (k=0 degenerates to full-range = perfect). Near-zero engineering on top of the range endpoint.
- **Trigger to revisit:** snapshot > 50 MB (≈ 8×10⁵ records) AND demand for cryptographic point-lookup privacy → hintless single-server (YPIR-family) over prefix-bucketed layout; 2-server DPF only if independent mirror operators emerge.

## Q10 — Keyless spent-state (VERIFIED live on mainnet)

- `nullifier = H(NULLIFIER_TAG:V1, channel_key, token, note_index, 0, k_owner)`; spent test = `nullifiers[nullifier]` slot ≠ 0.
- The contract writes the nullifier **and emits `NoteUsed { #[key] nullifier }` in the same action** — the raw nullifier felt is public on the event bus (observed live, block 14,040,840).
- Client state machine `unknown → discovered/unspent → spent` runs incrementally off either feed (NoteUsed events matched against the wallet's precomputed nullifier set, or storage diffs on precomputed nullifier slots). No point queries after initial sync.
- Trust bound: an indexer that omits a spend makes the wallet *believe* a note is spendable, but the on-chain nullifier check makes an actual double-spend revert — damage is bounded to UX/metadata, not theft.

## Q11 — Trust model (VERIFIED where marked)

- **`starknet_getStorageProof` works on free public RPC today** (verified on lava, spec 0.8.1): returns storage-trie + contracts-trie nodes, the pool's `storage_root`, and — free bonus — the pool's current `class_hash` in the same proof. Retention window ~25–55k blocks on lava → verify promptly, not archivally.
- Verification chain slot→state-root documented (pedersen MPT walk → contract leaf hash → contracts trie → poseidon state commitment → header `new_root`); end-to-end verifier not yet implemented (INFERRED at the final combination step) — a concrete roadmap item.
- **Per-note non-membership IS provable**: a storage proof of the nullifier slot returning 0 proves un-spent-ness at that block. Wallets can spot-check exactly what they care about.
- **Not provable cheaply:** completeness of a diff/event stream (omission), negative statements over ranges. Mitigations: content-addressed epoch bundles (any mirror regenerates from any RPC and must reproduce the identical hash → omission becomes a visible fork), second-RPC sampling, client spot-proofs of discovered notes/nullifiers.
- Honest positioning: **delegated trust with random audits**; the trustless fallback is self-hosting.

## Q12 — Reorgs & finality (VERIFIED + web evidence)

- Tiers: `pre_confirmed` (no hash, display-only, never in cursors), `ACCEPTED_ON_L2`/latest (sequencer-final, **revocable en masse** — the Sept 2025 Grinta incident rolled back ~2 h ≈ 4,000+ blocks), `ACCEPTED_ON_L1`/l1_accepted (irreversible without an L1 reorg). Measured l1_accepted lag: ~2.96 h (single sample; treat 2–6 h as the envelope).
- Per-mode semantics: live stream labels finality tier and emits explicit `rollback{to_block}`; queryable DB keeps `(number, hash, parent_hash)` linkage with upstream-style rollback and `(number, hash)` cursors; **epoch bundles contain only blocks ≤ l1_accepted → immutable by construction**; snapshots verify hash-chain on import.
- Improvement over upstream's `BLOCK_REORGED` → "re-sync from scratch": clients rewind to their last L1-final checkpoint, because we publish finalized checkpoints.

## Q13 — Upgrades (VERIFIED, incl. the real upgrade)

- Mechanism: StarkWare `ReplaceabilityComponent` — `replace_class_syscall` in place, optional EIC migration hook, `final` flag. **On-chain now: `upgrade_delay = 0`, `finalized = false`** → the operator can upgrade instantly, at any time, with zero notice — and that is exactly how the one real upgrade happened (self-grant governor + add + replace in one tx).
- Detection (all verified on the real upgrade): `state_diff.replaced_classes` (free if you ingest diffs), `ImplementationReplaced` event, periodic `getClassHashAt`, plus every storage proof carries the current class hash.
- Layout discipline (per upstream spec 10 + observed drift): map class hashes → decoder versions in config; real peripheral drift already happened within one release cycle (roles substorage rename, blocklist map rename+retype, event selector change) while **all 10 discovery storage vars stayed identical**.
- **Failure mode on unknown class hash:** keep ingesting and archiving RAW diffs/events (layout-agnostic, keeps mirrors reproducible), STOP typed decoding at that block, mark API degraded, resume after a human maps the new class. Strictly better than upstream's "return SERVICE_UNAVAILABLE".

## Q14 — Compatible mode: reuse core, zero forked logic (VERIFIED)

- Reference HTTP surface: 5 routes (`/health`, `/v1/sync/{incoming_state,outgoing_state,preflight_check}`, `/v1/history`) + optional OHTTP fallback envelope. Wire format **frozen across RC.0→RC.5→main**; the published SDK binds to exactly it (409 reserved for `BLOCK_REORGED`).
- The engine is generic over 4 small object-safe async traits (`RawStorageAccess` 3 methods, `RawEventAccess` 1, `StorageSnapshot`, `StorageBackend`) + `ChainState` in the service crate; blanket impls provide all 15 view methods and typed events. **Implement the traits over a local DB and the unmodified engine runs on top.** The fork-only RPC extension (`IncludeLastUpdateBlock`) becomes trivial with a DB.
- Two reuse depths: max (depend on `discovery-service` as a lib, mount its `ApiServer`) or medium (thin axum layer + copied serde types, conformance-tested).
- Consumable as a git-tag cargo dependency (Apache-2.0; not publishable to crates.io due to git deps — pin the same starknet-rust fork rev `7caedfe` for type identity). `discovery-core` is byte-identical RC.0→RC.5; main adds only a cosmetic refactor.
- Positioning stands as the original doc suggested: compatible mode = self-hosting/performance/migration, explicitly NOT the privacy story (it still receives viewing keys).

## Q15 — Client library: wrap, don't reimplement (VERIFIED empirically)

- **`cargo check -p discovery-core --target wasm32-unknown-unknown` passes untouched** at both RC.0 and main (rustc 1.95.0). No AES/ChaCha/RNG — decryption is Stark-curve ECDH + poseidon masking + felt subtraction; all deps pure-Rust wasm-clean. Remaining friction: `Send` bounds for JS-callback-backed backends (`#[async_trait(?Send)]` fork or `SendWrapper` — small, known).
- Drop-in surface: 3-method `DiscoveryProviderInterface` (`discoverNotes`/`discoverChannels`/`discoverRequirement`); `createPrivateTransfers` accepts any instance. The hosted `IndexerDiscoveryProvider` serializes the **raw viewing key** into every request body (`viewing_key: toHex(...)`) — the exposure the keyless client removes.
- `ContractDiscoveryProvider` (SDK's no-indexer fallback): hackathon claim "cannot be imported" is **partially refuted** — it IS reachable via the undocumented `/testing` subpath at rc.5, but it's testing-namespace, needs a hand-built `PoolContractInterface`, and returns `created: 0` for every note → **maturity-blind** (10-block rule unknowable). Issue #121 open, unanswered, unaware of this. Our provider fixes all three gaps.
- Layers: `strk20-client-core` (Rust, wraps discovery-core, pluggable transports: plain RPC or our raw endpoints) → `strk20-client-wasm` (key in as bytes, JSON shapes identical to the reference wire format so cursors interop) → TS adapter class.
- Zeroization honesty: `SecretFelt` hygiene survives in wasm linear memory; the JS boundary value is GC-managed and unscubbable — accept the key as `Uint8Array`, state the limits.
- **Conformance assets already exist upstream** (inventoried): `cairo-reference-data.json` (identical Rust/TS copies — cross-language crypto vectors), `devnet-state.json` (48-slot engine-level fixture), service devnet dump + 11 HTTP tests, SDK's 667-line wire-format test file. Enough to prove byte-compatibility without writing new vectors.

## Q16 — Push design (analysis + measured bandwidth)

Ranking by leakage: **global diff/event stream < per-epoch stream ≪ per-user slot subscription**. The global stream costs **<1 MB/day** at current activity (measured) — the privacy-optimal choice is also the cheap choice today. Per-user subscriptions are a durable pseudonymous fingerprint (and leak the address outright if Class-A slots are included) — offer only as documented opt-in or behind self-hosting. Switch default to epoch batches at ~100× growth.

## Q17 — Storage engine (decision, closing the gap the sprint left)

Measured needs: ~19 MB raw backfill today, ~80 KB/day, 100× headroom still <2 GB; queries = slot point-reads as-of-block, key-position-filtered events, block-hash-chain metadata, atomic reorg rollback; product includes immutable epoch files anyway.

**Decision: SQLite (embedded, WAL) + content-addressed epoch files as the canonical feed; Postgres as an optional backend behind the same storage trait.**

- The epoch files ARE the product (mirrorable, CDN-able, verifiable); the DB is a local index over them — this is the "hybrid" option from the original doc, now justified by numbers.
- SQLite gives the true one-command/self-host story (single binary + single file), trivially handles 100× scale, and upstream's own cold-start spec already uses SQLite snapshots as the distribution format.
- Postgres adds value only for multi-instance hosted serving; keep it behind the trait, not as the default. (The README's current "PostgreSQL" default is not justified by any measurement.)

## Q18 — Explorer: honest metrics only (VERIFIED foundations)

- **Ship:** gross deposits/withdrawals per token, TVL (or direct `balanceOf`), cumulative note count (= anonymity-set size, global), spend count, `ExternalContractInvoked` breakdown by target, registrations, governance/upgrade history, and indexer-health (head lag, class-hash match, decode-error/unknown-selector counters, coverage).
- **Do not ship:** anything joining `Deposit.user_addr` with `Withdrawal.to_addr` (amount/timing correlation is the classic attack — an explorer must not hand it to analysts), per-tx event timelines, per-token anonymity claims including enc notes, nullifier↔note "linkage" views, channel counts from raw diffs, recipient leaderboards.
- The explorer needs its own privacy review section in the threat model; "worse than no explorer" was the right instinct — the honest subset above survives it.

## Q19 — Benchmarks (definitions ready to fill)

B1 backfill wall-clock · B2 ingest lag p95 · B3 RPC reads per user sync (ours: **0** at query time vs reference ~2N/session) · B4 keyed-mode sync latency (reference baseline: ~1 s @1125 notes) · B5 keyless slab fetch bytes+time vs K-block gap · B6 client trial-decryption throughput · B7 concurrent-user throughput (reference ceiling: 7–9 req/s/node) · B8 amortization crossover (indexer wins for U ≥ 1) · B9 reorg correctness · B10 footprint. Minimum credible: B1, B3, B4/B5, B8. Methodology: pinned block hash, p50/p95 over ≥20 runs, script in `bench/`.

## Q20 — Hackathon facts (informational; verified from the hackathon repo & issues)

- Scoring is machine-checked every 30 min: demo URL (repo Website field is the most reliable source), any video URL, ≥3 mainnet txs each existing/SUCCEEDED/emitting a pool event. **A read-only project must NOT declare a `contracts` field** in `strk20.json` — then any pool-event tx counts, and app-made txs are explicitly sanctioned ("a project that deploys nothing is judged on the pool alone").
- Judge weights: 30% integration depth / 30% working mainnet product / 25% innovation / 15% docs+open-source; explicit bonus if other teams depend on what you publish — and issues #121/#221 show teams literally asking for a hosted `INDEXER_URL`.
- **No competing IDEA-23 project exists** among 174 registered entries (nearest neighbors inventoried; none do discovery-as-a-service). Registration for this repo is already merged (category Infra, `inspired_by: IDEA-23`).
- Practicalities: mainnet proving-service URL still unpublished (every pool write needs a proof; the official app / Ready wallet reach a prover themselves — Braavos does not implement the STRK20 wallet API); pool fee = 6 STRK per private tx; demo sequence and cut-lines detailed in [research/q4-q19-q20-value-demo.md](research/q4-q19-q20-value-demo.md).

---

## Assessment of the proposals (§4–5 of the open-questions doc)

| Proposal | Verdict |
|---|---|
| A. Threat model doc | **Yes — now largely pre-written.** The leakage table (Q7), trust statements (Q11), and upstream's own §5.5 admissions give it real content. Must include the Class-A address-leak finding and the explorer privacy review. |
| B. Architecture doc + diagrams | Yes; the reorg/finality table (Q12) and ingestion algorithm (Q5) are ready to be drawn. |
| C. Side-by-side privacy/benchmark dashboard | Yes; the "request body contains `viewing_key`: true/false" contrast is verified and demo-able. Use bulk mode for the privacy column (targeted mode would leak the address — the dashboard must not oversell it). |
| D. Immutable compressed sync feed | **Yes — this is the flagship artifact.** Measured sizes make it trivially cheap; cut ≤ l1_accepted; content-address; include `replaced_classes` + storage_root anchor per epoch. |
| E. SDK drop-in adapter | Yes, and cheaper than hoped: wrap discovery-core in wasm (compiles untouched), implement the 3-method interface. The `viewingKeyProvider`-consumed-locally design is exactly right. |
| F. Conformance tests vs upstream | Yes, and free: upstream ships cross-language crypto vectors, an engine-level devnet fixture, HTTP-level test suites, and SDK wire-format tests — inventory in Q15. |
| G. One-command self-hosting | Yes; SQLite default makes it a single binary + `STARKNET_RPC_URL`; ship health/lag/backfill status (the safe explorer metrics). |
| H. Snapshot/bootstrap | Yes but right-sized: full backfill is ~19 MB / ~47 min of RPC calls today — snapshots are a convenience, not a necessity. Publish finalized snapshots as content-addressed artifacts; verify hash-chain on import. |
| I. Multi-mirror support | Yes — content-addressed epochs make mirroring trivial and turn omission attacks into visible hash forks (Q11). More valuable than an elaborate explorer, as suspected. |
| J. Compatibility matrix | Yes — the Q1 version table is its first two rows; extend with SDK/discovery-core pins per class hash. |
| Full transaction CLI | **Deprioritize, confirmed** — duplicates the SDK, and every pool write needs a prover the ecosystem hasn't published a URL for. The narrow infra CLI (`run/backfill/status/snapshot/sync --keyless/bench`) is the right scope. |
| Fancy explorer | **Restrict, confirmed** — the honest metric set (Q18) is a small dashboard; anything deeper is either misleading or a correlation aid. |
| PIR before measurement | **Confirmed unnecessary** — measurements done; trigger condition documented (Q9). |
| User-specific WS subscriptions | **Confirmed harmful as default** — global stream is privacy-optimal and <1 MB/day. |

## The resolved product shape

The "two products" tension in §0 of the open-questions doc resolves cleanly because both share Layer A and the measurements remove the trade-off:

1. **Layer A — `indexerd`** (pool mirror): events-first ingestion → SQLite mirror `slot → (value, write_block)` + event archive; class-hash watch with halt-typed-decode; `(number, hash)` cursors; **content-addressed epoch bundles cut at l1_accepted** as the canonical public artifact. Raw archiving continues across unknown upgrades.
2. **Layer B — `client-core`** (secret-bearing, local): wraps upstream `discovery-core` (unmodified, wasm-clean); viewing key never serialized; conformance-tested against upstream's own fixtures.
3. **Layer C — privacy ladder**, re-ordered by what the research proved:
   - **Mode 3 (bulk/epoch) is the headline** — the only mode whose privacy claim survives scrutiny (hides key, slots, AND address), and it costs KB/day.
   - Mode 2 (targeted slots) demoted to documented convenience — it leaks the requesting address on the incoming path; equal to direct RPC.
   - Mode 1 (compatible) is an adapter for SDK drop-in and self-hosted migration — key visible, say so.
   - Mode 4 (PIR) stays a stretch with a written trigger; prefix-bucket endpoint is the near-free intermediate.

The strong one-line pitch is now evidence-backed:

> **STRK20 discovery as a public verified sync feed: every wallet downloads the same compact pool updates (~KB/day, ~6 MB full history) and discovers its notes locally — the server learns neither the viewing key, nor the slots, nor even who is syncing.**

## What remains genuinely open (research roadmap)

1. **Live slot smoke test** on the user-facing maps (`public_key`, `recipient_channels`, `notes`, `nullifiers`) — the formulas are verified at source level and against one live singleton slot, but the cheapest full falsifier is: register a key, compute the slots, read them on mainnet, decrypt an own note end-to-end. Do this before building on the slot stack.
2. **End-to-end storage-proof verifier** (pedersen MPT walk + poseidon state commitment) — turns "delegated trust with audits" into client-verifiable reads; nothing blocks it, just work.
3. **wasm end-to-end** with a JS-backed storage transport (`?Send` bounds) — compile-level feasibility proven, binding untested.
4. **Epoch bundle wire format** — freeze the schema (NDJSON vs binary felts, manifest, hash chain); a design deliverable, not research.
5. **Provider monitoring** — free archive RPC depth is empirically there today but contractually guaranteed by no one; the feed itself is the long-term mitigation.
6. Minor: two unmapped roles-component selectors (6 events, irrelevant to notes); l1_accepted lag variance; Sepolia identity of the phantom `0x52107f…` hash.
