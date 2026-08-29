# Q5 + Q8 — State-diff ingestion via ordinary RPC, and epoch-bundle volume (REAL mainnet measurements)

Measured 2026-08-29 against Starknet mainnet (latest block ~14,055,229).
Pool: `0x040337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a`.
All scripts + raw data live next to this file (prefix `q5q8_`, plus `sample-pool-diffs.json`, `pool_events.json`, `selector_map.json`).

## TL;DR verdicts

- **Q5: YES (VERIFIED).** `starknet_getStateUpdate` on spec 0.8.1 returns per-block `state_diff.storage_diffs` with the pool address and `(key, value)` pairs, plus `replaced_classes` / `deployed_contracts` that reliably capture the pool's deployment and its one on-chain upgrade. There is **no server-side per-contract filter** in standard JSON-RPC — you download the whole-block diff and filter locally — but whole-block diffs are tiny (median ~4 KB). Two free public archive endpoints (Lava, PublicNode) serve state updates and events all the way back to the pool's deployment block.
- **Q8: YES, trivially practical (VERIFIED).** Pool-only diffs are minuscule: ~80 KB/day raw JSON at current activity, ~19 MB raw (~6 MB zstd) for a full backfill of 131 days of history. Even at the historical peak (43x current) it's <4 MB/day. Epoch bundles of pool-only diffs are a non-issue; even whole-block bundles (~200 MB/day raw, ~30 MB/day zstd) are tolerable for a self-hosted server.

---

## Chain facts established along the way (all VERIFIED via RPC)

| Fact | Value | Evidence |
|---|---|---|
| Latest block at measurement time | 14,055,229 | `starknet_blockNumber` on lava |
| Spec version (lava, 1rpc) | 0.8.1 | `starknet_specVersion` |
| Spec version (publicnode) | 0.10.2 (still answers 0.8-style calls) | `starknet_specVersion` |
| Pool deployment block | **8,978,970** (2026-04-20 10:08 UTC) | binary search on `starknet_getClassHashAt`; `deployed_contracts` of that block's state update names the pool with class `0x30b8c540...4b4b30b` |
| Pool upgrade | **block 11,632,886**: class `0x30b8c...` → `0x67dddd89...76b554d` (current) | binary search on `getClassHashAt`; `replaced_classes` of block 11,632,886 = `[{"contract_address": "0x40337b1af...", "class_hash": "0x67dddd89..."}]` |
| README RC.0 class hash `0x52107fa...` | **matches neither** deployed nor current class | on-chain hashes above |
| Avg block time (100-block window) | 1.64 s | timestamps 14,055,000 vs 14,055,100 |
| Avg block time (10,000-block window) | 1.6728 s → **~51,650 blocks/day** | timestamps 14,045,100 vs 14,055,100 |

The upgrade at 11,632,886 explains the "unknown" `ExternalContractInvoked` events (below): the deployed contract now runs a **newer schema than tag PRIVACY-0.14.3-RC.0** — it includes `ExternalContractInvoked`, which only exists in the main-branch `events.cairo` (`scratchpad/starknet-privacy/packages/privacy/src/events.cairo:82`), not in the RC.0 tree.

## Q5 — Ingesting state diffs from ordinary RPC

### 5.1 Method and shape (VERIFIED)

`starknet_getStateUpdate` with `[{"block_number": N}]` works on spec 0.8.1. Result shape observed (block 14,055,200 on lava, saved as `su_recent.json`):

```
result: { block_hash, new_root, old_root, state_diff }
state_diff: {
  storage_diffs:               [ { address, storage_entries: [ {key, value}, ... ] }, ... ]
  nonces:                      [ ... ]
  deployed_contracts:          [ { address, class_hash } ]      <- pool deployment appears here (block 8,978,970)
  replaced_classes:            [ { contract_address, class_hash } ]  <- pool upgrade appears here (block 11,632,886)
  declared_classes:            [ ... ]
  deprecated_declared_classes: [ ... ]
}
```

The pool appears in `storage_diffs` with plain `(key, value)` felt pairs — 17 entries in its deployment block, 1–19 entries in sampled active blocks. Upgrade detection is therefore fully covered by watching `replaced_classes` (+ `deployed_contracts` for the genesis case); the contract also emits `ImplementationReplaced` (1 occurrence in the event scan, consistent with the single upgrade).

### 5.2 Server-side filtering (VERIFIED for standard RPC; docs-verified for Apibara)

- **Standard JSON-RPC (0.8/0.9/0.10): no per-contract filter for state updates.** The method takes only `block_id`. (PublicNode silently ignored an extra address param and returned the whole-block diff anyway.) The granular alternatives in standard RPC are exactly:
  - `starknet_getEvents` — server-side filter by contract `address` and key selectors (works, used below);
  - `starknet_getStorageAt(contract, key, block_id)` — point reads, incl. historical blocks (verified at block 9,000,000 on both lava and publicnode);
  - `starknet_getStorageProof` — **works on publicnode** with positional params `["latest", null, [pool], null]` and returns the pool's `storage_root` (`0xa52c76f6dbd6...`) inside `contracts_proof.contract_leaves_data` — usable as an integrity check that a locally reconstructed pool storage matches chain state. Lava answered the malformed-params probe with a param error; positional form not retried there (likely works — INFERRED).
- **Apibara DNA** (docs, [filter reference](https://www.apibara.com/docs/networks/starknet/filter)): `StorageDiffFilter` has a `contractAddress` field — i.e. **server-side contract-filtered storage-diff streaming exists**, but only through Apibara's DNA gRPC protocol (hosted, API key) — not through any public JSON-RPC.
- **Pathfinder extensions**: only proof-related (`pathfinder_getProof`, deprecated in favor of standard `starknet_getStorageProof`). No contract-filtered diff method. (Docs pass; INFERRED-from-docs.)

**Conclusion:** an indexer on ordinary RPC must fetch whole-block state updates and filter locally — and that is cheap (see Q8). The efficient pattern is *events-first*: use `getEvents(address=pool)` to find active blocks, then `getStateUpdate` only for those blocks (0.23% of blocks currently).

Caveat (INFERRED): a storage write that emits no event and nets to zero within the block (e.g. reentrancy-guard toggles) never shows in either events or the squashed block diff — harmless. A *persistent* write with no event would be missed by events-first discovery; none exist in the current contract (every state-changing entrypoint emits), and `starknet_getStorageProof`'s storage_root gives a cheap periodic completeness check.

### 5.3 Historical depth of free endpoints (VERIFIED)

`starknet_getStateUpdate` probed at blocks 1,000,000 / 8,978,970 / 14,000,000:

| Endpoint | block 1,000,000 | 8,978,970 | 14,000,000 | Notes |
|---|---|---|---|---|
| `https://rpc.starknet.lava.build` | OK (58,022 B, 48 diffs) | OK | OK | spec 0.8.1; also served the full 118k-event scan |
| `https://starknet.publicnode.com` | OK | OK | OK | spec 0.10.2; occasional TLS resets under sustained load (rate limiting) |
| `https://1rpc.io/starknet` | OK | HTTP 400 | HTTP 400 | works but rate-limits after ~1 request; last-resort only |
| `https://starknet-mainnet.public.blastapi.io/rpc/v0_8` | — | — | — | **DEAD**: 403 "Blast API is no longer available… use Alchemy" |
| `https://free-rpc.nethermind.io/mainnet-juno/v0_8` | — | — | — | **DEAD**: DNS does not resolve |
| `https://starknet.drpc.org` | — | — | — | starknet_* methods "not available" on public path |

**No archive-node self-hosting is needed for backfill to pool deployment**: lava and publicnode both serve full-depth state updates, events, and historical `getStorageAt`. (Whether they keep full depth forever is outside our control — INFERRED risk; mitigated by the indexer archiving what it ingests, which is the whole point of the project.)

### 5.4 `starknet_getEvents` behaviour + real deployed event schema (VERIFIED)

Full scan: `address=pool`, `from_block` 8,978,970 → latest, `chunk_size=1000`, 145 pages, ~5 minutes on lava, sequential. Continuation tokens look like `"<block>-<offset>"`; lava returns partial chunks (<1000) when a page's internal scan window (~82k blocks) is sparse — both fine for pagination. **118,372 events total in 28,260 distinct blocks.**

Distinct first-key selectors seen, mapped by `starknet_keccak(name)` (pycryptodome; map in `selector_map.json`):

| Event | Selector (starknet_keccak) | Count | felts/event (keys+data, median) |
|---|---|---|---|
| Withdrawal | 0x2eed7e29b3502a726faf503ac4316b7101f3da813654e8df02c13449e03da8 | 40,022 | 7 |
| EncNoteCreated | 0x23c20207be8b1ef4430c25eef8ce779c9745ebe04139555ae81bd4f8fdd6ec5 | 29,477 | 3 |
| NoteUsed | 0x247fc60d782e0094e7f98c47f277d92a3345d07a436f1f56b27a9b62be2322e | 25,590 | 2 |
| Deposit | 0x9149d2123147c5f43d258257fef0b7b969db78269369ebcf5ebb9eef8592f2 | 16,163 | 4 |
| ViewingKeySet | 0x1321a492485b4f19851fb787ab3800a0030b595332cba93cd5fe40dfb5a4daf | 2,592 | 6 |
| **ExternalContractInvoked** (NOT in RC.0!) | 0xa8fb36d0894f5e87797c38533a55c4486a1f35e9e9eced10f995b9639a8955 | 1,596 | 3 |
| OpenNoteCreated | 0x22330482fd296a27cf9096807b4a3622cd619d31cce42c1e55655914e8459ee | 1,439 | 6 |
| OpenNoteDeposited | 0x25b6da03c4858d11cb0708d5cb6be79b190fb32eb7a7ce83804e07cbbb9bead | 1,439 | 5 |
| RoleGranted / RoleRevoked / RoleAdminChanged | (std) | 12 / 7 / 10 | 4 |
| Fee/Auditor/Screener/ProofValidity/Governance admin events | (see selector_map.json) | 1–2 each | 2–4 |
| ImplementationAdded / ImplementationReplaced | 0x38a81c7...13a3 / 0x34bb683...dc4b7 | 1 / 1 | 4 |
| 4 unmapped selectors (0x209ff368…, 0x198116a…, 0x15e9615…, 0x3940d40…) | | 8 total | 3 |

The 4 unmapped selectors (8 events of 118,372) did not match any struct name in either the RC.0 or main tree nor common starkware-roles names — best guess: governance-component events from a starkware-libs version not vendored here (INFERRED; irrelevant for note indexing).

**Schema warning for the indexer:** the live contract emits `ExternalContractInvoked`, which exists only in main-branch `events.cairo` (line 82), not in RC.0 → decode against the *current* class ABI (fetch via `starknet_getClass(0x67dddd89...)`), not the RC.0 tag; keep per-class ABI versioning keyed off `replaced_classes`.

Activity profile (from the same scan; `q5q8_event_stats.json`):

| Window | active blocks/day | events/day |
|---|---|---|
| last 1 d | 132 | 574 |
| last 7 d | 120 | 526 |
| last 30 d | 71 | 319 |
| whole 131 d history | 216 avg | 903 avg |
| **peak** (50k-block window ≈1 day, blocks 11.20–11.25M) | 5,507 | 22,491 |

Median 3 events per active block (mean 4.2, max 68). Peak was ~43x the current 7-day average.

## Q8 — "Download all pool diffs per epoch, filter locally": measured volumes

### 8.1 Per-block pool-only diff sizes (VERIFIED; `sample-pool-diffs.json`)

Sample: 30 pool-active blocks (25 uniform-random across history + 5 most recent active) + 22 random recent blocks (14.05M range), each via `getStateUpdate`, pool's entry extracted locally:

| Metric | active blocks (n=30) | random recent (n=22) |
|---|---|---|
| pool storage entries/block | min 1 / med 2 / mean 4.3 / max 19 | **0 in all 22** |
| pool-only JSON bytes/block | min 188 / med 340 / mean 671 / max 2,607 | 100 (empty stub) |
| full-block state update JSON | med 9,907 / max 15,666 | med 6,177 / max 37,415 |
| pool share of full-block diff | median **4%** | 0% |

Roughly 1 storage write per event (mean 4.3 entries vs 4.2 events per active block); ~156 B raw JSON per entry.

Cross-check on 25 random recent blocks (`q5q8_fullblock_sizes.json`): full-block updates min 575 B / med 4,270 B / mean 4,649 B / max 11,808 B; 6.6 contracts and 26.7 entries per block on average.

### 8.2 Compression (VERIFIED; macOS has both gzip and zstd)

| Bundle | raw | gzip -9 | zstd -19 | zstd ratio |
|---|---|---|---|---|
| 30 pool-only diffs, compact NDJSON (`q5q8_pool_diffs_concat.ndjson`) | 20,942 B | 7,606 B (2.75x) | 6,487 B (**3.23x**) | ~50 B/entry compressed |
| 197 consecutive full-block updates (14,055,000–14,055,199) | 774,759 B (3,933 B/blk) | 156,123 B (5.0x) | 114,310 B (**6.8x**, 580 B/blk) | |
| all 118,372 pool events, JSON (`pool_events.json`) | 69.38 MB (586 B/event) | 10.77 MB | 10.35 MB (zstd -9, **6.7x**) | |

Hex-felt payloads (random-looking 251-bit values) cap pool-diff compression near ~3x; the JSON framing is what compresses. A binary encoding (32-byte felts) would halve raw size before compression.

### 8.3 Volume estimates (anchored to measurements above)

Assumptions: 51,650 blocks/day; current activity = 7-day avg (120 active blocks/day, 526 events≈writes/day); mean 671 B pool-JSON per active block; zstd ÷3.2 for pool-only, ÷6.8 for full-block.

| Scenario | pool-only raw JSON | pool-only zstd |
|---|---|---|
| **bytes/day, current** | ~80 KB | ~25 KB |
| bytes/day, historical peak (43x) | ~3.7 MB | ~1.2 MB |
| bytes/day, 10x current | ~0.8 MB | ~0.25 MB |
| bytes/day, 100x current | ~8 MB | ~2.5 MB |
| **initial full sync (131 d, 28,260 active blocks / 118k events)** | ~19 MB | **~6 MB** |
| full sync at 10x lifetime activity | ~190 MB | ~60 MB |
| full sync at 100x | ~1.9 GB | ~0.6 GB |

Events-based archive as an alternative/companion: full history = 69 MB raw / 10.4 MB zstd; ~308 KB/day raw at current rate.

Whole-block (unfiltered) comparison: 51,650 × 3,933 B ≈ **203 MB/day raw, ~30 MB/day zstd** — tolerable for a self-hosted indexer server that then discards non-pool data, but 99.96% waste for end clients (pool is in 0.23% of blocks, and 4% of the diff even in those).

RPC call budget for backfill (events-first): one `getEvents` scan (145 pages, ~5 min measured) + 28,260 `getStateUpdate` calls (~47 min at 10 rps, spread over lava+publicnode). Streaming every block instead would be 5.08M calls / ~20 GB — avoid.

### 8.4 Epoch design observations

- ~51,650 blocks/day (1.67 s blocks). Natural epoch choices: **10,000 blocks (~4.6 h)** or **50,000 blocks (~1 day)**.
- Pool-only epoch bundle at current activity: 10k-block epoch ≈ 23 active blocks ≈ 15 KB raw / ~5 KB zstd; daily epoch ≈ 80 KB raw / 25 KB zstd. At peak-historical rates: daily epoch ≈ 3.7 MB raw / 1.2 MB zstd. **Even mobile/light clients can pull full epochs; there is no need for finer-grained filtering, PIR, or partial downloads at these sizes for years of 10–100x growth.**
- Bundle format suggestion: compact NDJSON `{block, diff}` (or 32-B binary felts), zstd-compressed per epoch, plus per-epoch `replaced_classes`/`deployed_contracts` entries for upgrade tracking, and the pool `storage_root` from `starknet_getStorageProof` at the epoch head as an integrity anchor.
- Whole-block epoch bundles (39 MB raw / ~5.7 MB zstd per 10k blocks) are also viable server-side if one wants a pool-agnostic archive.

## Decision output: ingestion source hierarchy

1. **Primary: `https://rpc.starknet.lava.build`** (spec 0.8.1) — full archive depth (verified to block 1M and pool genesis), fast getEvents (118k events in ~5 min), no key.
2. **Secondary/failover: `https://starknet.publicnode.com`** (spec 0.10.2) — same depth incl. `starknet_getStorageProof`; throttles sustained loops (TLS resets) → keep <5 rps with retry/rotate.
3. **Last resort: `https://1rpc.io/starknet`** — correct but rate-limits after ~1 call/burst.
4. **Do not use:** blastapi (dead, 403 → Alchemy ad), free-rpc.nethermind.io (DNS gone), drpc public path (starknet_* not exposed).
5. **Optional upgrade path:** Apibara DNA `StorageDiffFilter(contractAddress=pool)` for server-side filtered streaming (hosted, API key), or a keyed Alchemy/QuickNode endpoint; a self-hosted Pathfinder/Juno archive only if free endpoints degrade — measurements show it is not required today.

Ingestion algorithm (measured-cheap): tail head via `blockNumber` + `getEvents(pool)` per new range → for each active block, `getStateUpdate` → extract pool `storage_diffs` + any `replaced_classes` → append to epoch bundle → periodically cross-check reconstructed storage against `getStorageProof` storage_root.
