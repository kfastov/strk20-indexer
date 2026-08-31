# Sepolia backfill sizing + storage-proof support (live measurements)

Measured 2026-08-30 against Starknet Sepolia, head ~14,299,841 at scan time.
Pool: `0x0254a6b2997ef52e9f830ce1f543f6b29768295e8d17e2267d672c552cfe0d91`.
Raw evidence files in [`data/`](data/) next to this doc (scan totals, proof probes, full proof
response). The 11 MB raw event NDJSON stayed in the session scratchpad (regenerable in ~35 s).

## TL;DR

- **Sepolia history is ~6.3x smaller than mainnet**: 18,772 pool events in 4,380
  pool-active blocks (mainnet 2026-08-29: 118,372 in 28,260). Full backfill ≈
  **13.2k RPC calls ≈ 44 min at 5 req/s**; the getEvents census alone took **35 s**.
- **Feed ≈ 11 MB raw / ~1.5–2.5 MB zstd; SQLite DB ≈ 12 MB.** Trivial.
- **`starknet_getStorageProof` on publicnode Sepolia works ONLY at the exact current
  head block** — a block stops being provable the moment the next block lands
  (~1.7 s). `l1_accepted` fails too. **Epoch-anchor storage roots cannot be fetched
  retroactively on Sepolia**; they must be captured opportunistically at head.
- **Lava Sepolia (`rpc.starknet-testnet.lava.build`) was 100% down** for the entire
  session (~25 min, every method, 5 retry rounds): gateway error "No pairings
  available". Zero of the planned cross-checks could run there.
- publicnode: spec **0.10.2**, **JSON batch supported**, **chunk_size 1000 honored
  exactly**, deep archive verified to the pool's deploy block. One gotcha: it
  **403s python-urllib's default User-Agent** (fine with a curl UA).

## 1. Event census (VERIFIED)

One paged `starknet_getEvents` loop on `https://starknet-sepolia-rpc.publicnode.com`:

```json
{"jsonrpc":"2.0","id":1,"method":"starknet_getEvents","params":[{
  "from_block":{"block_number":8200000},
  "to_block":{"block_number":14299841},
  "address":"0x0254a6b2997ef52e9f830ce1f543f6b29768295e8d17e2267d672c552cfe0d91",
  "chunk_size":1000, "continuation_token":"..."}]}
```

Results (`data/scan_result.json`):

| Metric | Value |
|---|---|
| Total events | **18,772** |
| Distinct pool-active blocks | **4,380** |
| First / last event block | **8,271,125** / 14,298,384 (first matches the known deploy block) |
| Pages | **19** (18 x exactly 1000 + 772) — chunk_size 1000 honored, no clamping |
| Wall time | **34.9 s**, 0 errors (0.54 pages/s; ~1.6 s server latency per page + 0.15 s sleep) |
| Events per active block | min 1 / med 3 / mean 4.29 / **max 298** (no block over 1000 → per-block getEvents is always 1 page) |
| Felts per event (keys+data) | med 3 / mean 3.94 |
| Raw NDJSON as fetched | 10,958,024 B (584 B/event) → **zstd -19: 1,389,808 B (7.9x)**, gzip -9: 1,716,717 B |

### Per-event-key counts — every selector mapped (VERIFIED)

Selectors computed locally with `starkli selector <Name>` (`~/.starkli/bin/starkli`):

| Event | Count | Selector (first key) |
|---|---:|---|
| EncNoteCreated | 7,030 | `0x23c20207be8b1ef4430c25eef8ce779c9745ebe04139555ae81bd4f8fdd6ec5` |
| Withdrawal | 3,482 | `0x2eed7e29b3502a726faf503ac4316b7101f3da813654e8df02c13449e03da8` |
| NoteUsed | 3,378 | `0x247fc60d782e0094e7f98c47f277d92a3345d07a436f1f56b27a9b62be2322e` |
| Deposit | 1,810 | `0x9149d2123147c5f43d258257fef0b7b969db78269369ebcf5ebb9eef8592f2` |
| ExternalContractInvoked | 1,096 | `0xa8fb36d0894f5e87797c38533a55c4486a1f35e9e9eced10f995b9639a8955` |
| ViewingKeySet | 694 | `0x1321a492485b4f19851fb787ab3800a0030b595332cba93cd5fe40dfb5a4daf` |
| OpenNoteCreated | 620 | `0x22330482fd296a27cf9096807b4a3622cd619d31cce42c1e55655914e8459ee` |
| OpenNoteDeposited | 620 | `0x25b6da03c4858d11cb0708d5cb6be79b190fb32eb7a7ce83804e07cbbb9bead` |
| RoleGranted | 11 | `0x9d4a59b844ac9d98627ddba326ab3707a7d7e105fd03c777569d0f61a91f1e` |
| RoleAdminChanged | 10 | `0x2b23b0c08c7b22209aea4100552de1b7876a49f04ee5a4d94f83ad24bc4ec1c` |
| ImplementationAdded | 6 | `0x38a81c7fd04bac40e22e3eab2bcb3a09398bba67d0c5a263c6665c9c0b13a3` |
| **ImplementationReplaced** | **4** | `0x34bb683f971572e1b0f230f3dd40f3dbcee94e0b3e3261dd0a91229a1adc4b7` |
| UpgradeGovernorAdded | 2 | `0x2143175c365244751ccde24dd8f54f934672d6bc9110175c9e58e1e73705531` |
| GovernanceAdminAdded | 1 | `0x3ae95723946e49d38f0cf844cef1fb25870e9a74999a4b96271625efa849b4c` |
| AppGovernorAdded | 1 | `0x1f9961b3744c1e017cbcfafecec635be98ae8c6aeb9f70be5b7e93f2f52e2e5` |
| AppRoleAdminAdded | 1 | `0x209ff368803f5de65188245078e888d4462f8d98697699c1dcdd8b02ffb250f` |
| AuditorPublicKeySet | 1 | `0x1201d99a15f3d88fe402ca349f486e5d3f92bd6bf41c0990d74b48c0f7b2ea1` |
| ScreenerPublicKeySet | 1 | `0x24a3c770102a21d765f1e5478b480aeb39ebc6f0a158cef07e722d74564009f` |
| FeeAmountSet | 1 | `0x3a71cae33f889d328d50250566d1f55971af0792b89c5b3f5fbea1f7aafc4d7` |
| FeeCollectorSet | 1 | `0x125aaf53a346c4e00244d4b9b35ef8366df1831e45931cd22d8d0211eea7347` |
| ProofValidityBlocksSet | 1 | `0x35ded6c81008684ea271437e09bf788dda262449efb89b0ef0ad492e0a81381` |
| ImplementationRemoved | 1 | `0x7633a8d8b49c5c6002a1329e2c9791ea2ced86e06e01e17b5d0d1d5312c792` |
| **Total** | **18,772** | (sum checks) |

Zero occurrences of: OpenNoteDepositorBlockSet, Paused, Unpaused,
ImplementationFinalized, RoleRevoked, RoleGrantedWithDelay.

Cross-checks against known facts: first event at 8,271,125 (matches);
ImplementationReplaced count 4 matches the four known class replacements
(10829820, 11111946, 11612079, 12932675).

**Bonus — mainnet mystery selectors retired.** The "4 unmapped selectors" in
`../q5-q8-ingestion-volume.md` are starkware-libs Roles-component events (VERIFIED
via `starkli selector`): `0x209ff368…` = **AppRoleAdminAdded**, `0x198116a…` =
**AppRoleAdminRemoved**. (`0x15e9615…`, `0x3940d40…` still unmapped — not seen
on Sepolia.)

### Activity profile (VERIFIED from the same scan)

| Block range | active blocks | events |
|---|---:|---:|
| 8M–9M | 925 | 5,239 |
| 9M–10M | 625 | 1,926 |
| 10M–11M | 306 | 1,060 |
| 11M–12M | 490 | 2,294 |
| 12M–13M | 593 | 1,941 |
| 13M–14M | 918 | 4,006 |
| 14M–head | 523 | 2,306 |

Sepolia block time: **1.666 s** over the last 10k blocks (timestamps of head vs
head−10000) → **~51,850 blocks/day**, same as mainnet. History span ≈ 116 days.
Current rate (last 100k blocks ≈ 1.9 days): **~50 active blocks/day, ~172
events/day** — about 0.4x mainnet's current 120/526.

## 2. Events-first backfill estimate (INFERRED from verified counts)

Model (matches the indexer's ingest loop): one scan `getEvents` page per ~1000
events, plus per pool-active block `getBlockWithTxHashes` + `getStateUpdate` +
one per-block `getEvents` page (max 298 events/block, so never more than one).

| Component | Calls |
|---|---:|
| getEvents census scan | 19 (measured) |
| getBlockWithTxHashes x 4,380 | 4,380 |
| getStateUpdate x 4,380 | 4,380 |
| per-block getEvents x 4,380 | 4,380 |
| **Total** | **~13,160** |

At the 5 req/s budget: **~2,630 s ≈ 44 minutes**. Batch requests (supported,
§4) can pack the 3 per-block calls into one HTTP round trip → ~4,400 round
trips, so HTTP latency (~0.3–1.7 s observed) never becomes the bottleneck at
5 req/s. Steady-state after backfill: ~50 active blocks/day → ~150 calls/day.

Deep-history feasibility spot checks (VERIFIED, all on publicnode):

- `starknet_getStateUpdate [{"block_number":8271125}]` → OK, 3,774 B; pool in
  `deployed_contracts`, **17 pool storage entries** (same genesis pattern as
  mainnet block 8,978,970).
- `starknet_getStateUpdate [{"block_number":11612079}]` → OK; `replaced_classes`
  = pool → `0x67dddd89d80fedadc06b6f160798f94800a4a70164e5a24301cd0d6076b554d`
  (matches the known v2 replacement at that block).
- `starknet_getStateUpdate [{"block_number":14298384}]` → OK, 3 pool entries.
- `starknet_getBlockWithTxHashes [{"block_number":8271125}]` → OK (ts 1774976548, 1 tx).

## 3. `starknet_getStorageProof` on Sepolia (VERIFIED) — head-only

Probe (publicnode), latest and head−{1..10, 1000, 30000, 60000}:

```json
{"jsonrpc":"2.0","id":1,"method":"starknet_getStorageProof","params":{
  "block_id":"latest",
  "contract_addresses":["0x0254a6b2…d91"],
  "contracts_storage_keys":[{"contract_address":"0x0254a6b2…d91","storage_keys":["0x1"]}]}}
```

| block_id | Result |
|---|---|
| `latest` | **OK** — 10,481 B; 23 contract-trie nodes + 17 storage-trie nodes |
| head−0 (by number, immediately) | OK once; **failed one block later for the same number** |
| head−1 … head−10, −1000, −30000, −60000 | **FAIL**, error code **42**: "the node doesn't support storage proofs for blocks that are too far in the past" |
| `l1_accepted` | **FAIL**, same error 42 |

So the proof window is **exactly one block: the current head**. Verified
live: block 14,299,879 answered while it was head, then errored ~1.7 s later
when 14,299,880 landed (`data/probe_fine.txt`).

The successful response is self-identifying and complete (`data/proof_latest_full.json`):

- `contracts_proof.contract_leaves_data[0]` = `{nonce: 0x0, class_hash:`
  `0x56ab118a8a6e38efc93ad758cefe909fee421fa931ce3cf72df624d345623b2, storage_root:`
  `0x2f83522d4439ac88a97fb2c8a95f42dfa15fd85e907f7050ddf95bd6ed3df6f}` — the
  class hash matches the known current Sepolia class (cross-check).
- `global_roots` carries `contracts_tree_root`, `classes_tree_root` **and
  `block_hash`** — the response names the block it proves, so the safe pattern is
  `block_id:"latest"` + read `global_roots.block_hash` back, never
  "fetch head number, then prove that number" (that race loses ~half the time
  at 1.7 s blocks).
- `contract_leaves_data` is populated **only when `contract_addresses` is passed**;
  with `contracts_storage_keys` alone it comes back `[]` (both variants VERIFIED).

**Consequence for verify-root on Sepolia:** anchor storage roots can never be
fetched retroactively (epoch cuts are below `l1_accepted`, which is already
unprovable). The indexer must grab proofs opportunistically at the live head and
associate them with the block the response names. A verify-root rescan that
wants "the root as of block N" for any past N is impossible against publicnode
Sepolia. Mainnet publicnode is known to serve proofs at `latest`
(REPORTED, `../q5-q8-ingestion-volume.md`); its window depth was not measured
there — plausibly head-only too (INFERRED, same Juno 0.10.2 error string;
worth one probe before relying on it).

## 4. Endpoint capabilities (VERIFIED)

### `https://starknet-sepolia-rpc.publicnode.com` — worked for everything

| Check | Result |
|---|---|
| `starknet_specVersion` | `"0.10.2"` |
| Batch (JSON array of specVersion + blockNumber) | **Supported** — HTTP 200, both results in one array (order not preserved; match by id) |
| `getEvents` chunk_size 1000 | **Honored exactly** (18 consecutive pages of 1000) |
| Archive depth | Full — events from 8.27M, state updates + block headers at 8.27M (VERIFIED above) |
| `pending` block tag | Not supported (REPORTED, prior session; not retested — `latest` used throughout) |
| Rate limiting | None observed: ~75 calls this session, bursts ~3–6 req/s during scan+probes overlap, 0 errors/429/TLS resets |
| User-Agent filter | **HTTP 403 for python-urllib's default UA**; fine with `curl/8.7.1` UA. Indexer should send an explicit UA. |

Observed request rates: census 19 pages / 34.9 s; proof probes at ~2.5 req/s
(0.3–0.4 s spacing) with zero failures.

### `https://rpc.starknet-testnet.lava.build` — down all session

Every method (specVersion, blockNumber, getStorageProof) across 5 retry rounds
spanning ~25 min returned HTTP 500:

```
{"error":"…STRKS could not get a provider address from blocked provider list
ErrMsg: No pairings available. {csm.currentlyBlockedProviderAddresses:
lava@12y90f9…, lava@1tu7g64…, lava@1hxtq28…}"}
```

That is the Lava gateway reporting all three of its Sepolia providers blocked —
an upstream outage, not rate limiting of us. **No Sepolia cross-checks (proof
window, batch, chunk) could run on a second endpoint.** The mainnet lava
endpoint (`rpc.starknet.lava.build`) was healthy on 2026-08-29 (REPORTED,
q5-q8); the testnet pool is evidently thinner. A backup Sepolia endpoint is an
open TODO — publicnode is currently a single point of failure for Sepolia CI.

## 5. Volume: SQLite DB + feed for a full Sepolia backfill

Anchors:

- **Sepolia measured here (VERIFIED):** 18,772 events / 4,380 active blocks /
  584 B/event raw RPC JSON / 7.9x zstd on that NDJSON.
- **Mainnet per-unit figures (REPORTED, `../q5-q8-ingestion-volume.md`):**
  586 B/event raw JSON (Sepolia reproduces this at 584), 671 B pool-diff JSON
  per active block, ~1 storage write per event, zstd 3.2x (diffs) – 6.7x (events).
- **Real partial mainnet DB (VERIFIED):** `data/mainnet/strk20.db` (schema v1,
  felts as 32-B blobs), checkpointed copy at blocks 8,978,970–9,571,715+:
  815 events / 163 blocks / 1,015 storage_log rows → **520,192 B checkpointed =
  638 B/event all-in** (483,328 B vacuumed = 593 B/event). Includes all indexes
  (ev_key0, ev_key1, storage_log_block, blocks_hash). Storage rows ≈ 1.25/event.

Estimates for full Sepolia backfill (INFERRED from the anchors):

| Artifact | Size |
|---|---|
| **SQLite DB** (18,772 events x ~600–640 B all-in) | **~11–12 MB** (+ transient WAL) |
| **Feed, raw NDJSON** — events part 7.09 MB measured-encoded at 378 B/event (feed framing over actual Sepolia felts, minimal hex) + 19–23k diff entries x ~140 B + 4,380 block lines x ~210 B framing | **~10.6–11.3 MB** |
| **Feed, zstd per-epoch** (5–8x observed range on this data shape) | **~1.5–2.5 MB** |
| Raw RPC event archive (kept if we archive as-fetched) | 11.0 MB raw / 1.39 MB zstd (measured) |

Per-epoch: at 10k-block epochs (~4.6 h), the ~603 epochs of Sepolia history
average ~7 active blocks / ~31 events / **~18 KB raw (~4 KB zstd)** each; the
busiest 1M-block stretch (8M–9M) is ~2.4x that. All far below any threshold that
matters (the PIR trigger in the roadmap is 50 MB).

Not measured for Sepolia: actual per-block diff bytes (needs the 4,380
`getStateUpdate` calls of the backfill itself). The 671 B/block and
~1-write-per-event figures are mainnet-derived; Sepolia's genesis block showed
the same 17-entry pattern and sampled blocks 2–3 entries, consistent (VERIFIED
at n=3).

## Bottom line for the roadmap

Sepolia config (roadmap item 6) is cheap on every axis measured: **<1 h
backfill at 5 req/s, ~12 MB DB, ~11 MB feed**. The two real constraints found:
storage-proof anchoring must be **head-opportunistic** on Sepolia (and possibly
everywhere — probe mainnet's window), and there is currently **no working
fallback RPC** for Sepolia.

---

### Appendix: commands run

- Event census: `scan_events.py` (session scratchpad; paged getEvents as quoted
  in §1, 0.15 s inter-page sleep, UA `curl/8.7.1`) → `data/scan_result.json`.
- Selector map: `starkli selector <Name>` for the 24 candidate event names +
  12 Roles-component names.
- Proof probes: `probe_proof.py` / `probe_fine.py` → `data/probe_publicnode.txt`,
  `data/probe_fine.txt`, `data/proof_latest_full.json`.
- Compression: `zstd -19` / `gzip -9` on the fetched NDJSON.
- DB measurement: copied `data/mainnet/strk20.db{,-wal,-shm}` to scratchpad,
  `PRAGMA wal_checkpoint(TRUNCATE)`, `VACUUM INTO`, `dbstat` per-table sums.
- Lava retries: `curl` specVersion x5 rounds over ~25 min, all HTTP 500 as quoted.
