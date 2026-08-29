# Adversarial verification: Q5/Q8 ingestion & volume claims (key: verify-ingestion-volume)

Verifier ran independent RPC calls (curl, sequential, lava primary / publicnode secondary) on 2026-08-29,
deliberately choosing sample blocks NOT in the researcher's 30-block sample where possible.
Raw outputs: `verify_c12.json`, `verify_c3.json` in this directory. Scripts: `verify_rpc2.py`, `verify_rpc3.py`.

Overall verdict: **CONFIRMED** (with three minor corrections, none within 10x — all well inside 1.5x).

Note on tooling: lava returns **HTTP 403 to Python urllib's default User-Agent** but works fine via curl.
Worth a note for the indexer implementation (set a real User-Agent).

## Check 1 — getStateUpdate on 5 pool-active blocks (lava, spec 0.8.1)

4 blocks chosen from the researcher's event index but NOT in their diff sample, plus 1 overlap block
(9,778,539, claimed 19 entries / 2,607 B) as an exact cross-check.

| block | pool in storage_diffs | entries | pool JSON bytes | contracts in block diff |
|---|---|---|---|---|
| 9,439,303 | YES | **62** | 9,391 | 13 |
| 9,439,322 | YES | **68** | 10,291 | 7 |
| 9,424,389 | YES | 8 | 1,033 | 8 |
| 9,114,292 | YES | 7 | 882 | 17 |
| 9,778,539 (overlap) | YES | 19 | **2,607 (exact match to claim)** | 16 |

- Pool address appears with plain `{key,value}` felt pairs in every case. VERIFIED.
- Entry counts track the researcher's own per-block event counts ~1:1 (their event index says 62 and 68
  events at 9,439,303/9,439,322; the state diffs have exactly 62 and 68 entries) — the "~1 write per event"
  model VERIFIED on independent blocks.
- The overlap block reproduces byte-for-byte (2,607 B pool JSON, 19 entries).
- **Correction (minor):** the claim "1–19 entries in active blocks" describes only their 30-block sample.
  The true tail is larger: at least 68 entries / ~10.3 KB in one block. This does NOT break the volume
  estimates because mean entries/active block over the FULL population = 118,372 / 28,260 = 4.19,
  matching their sample mean 4.3 — the sample is representative on the mean, just not on the max.

## Check 2 — historical depth on two providers

| provider | block | result |
|---|---|---|
| lava | 2,000,000 | OK — 27 storage_diffs, 30,884 B |
| lava | 8,978,970 (pool deploy) | OK — pool in `deployed_contracts` with class `0x30b8c540cf04d8ef0f4db2a9098d9cc0e35e83af1cb3325f5a4f40144b4b30b`, 17 pool storage entries |
| publicnode | 1,234,567 | OK — 63 storage_diffs, 110,832 B |
| publicnode | 8,978,970 | OK — same deploy data, 17 pool entries |

Archive depth to pool genesis on both free providers: VERIFIED (my probe blocks differ from theirs).
Deploy class hash and 17-entry deploy diff: VERIFIED on both providers independently.

Upgrade boundary re-verified with 2 fresh `starknet_getClassHashAt` calls:
- block 11,632,885 → `0x30b8c540...` (old)
- block 11,632,886 → `0x67dddd89d80fedadc06b6f160798f94800a4a70164e5a24301cd0d6076b554d` (current)
Exactly one upgrade at block 11,632,886: VERIFIED.

## Check 3 — getEvents selectors (4 pages fetched independently)

3 pages from deployment (chunk_size 1000; lava paginates by ~82k-block windows, so early pages return
252/45/6 events — consistent with the researcher needing 145 pages total) + 1 page from block 13,900,000
(970 events). 19 distinct selectors observed; I independently recomputed starknet_keccak (eth_utils keccak,
masked to 250 bits) for all event names and every named selector matches the researcher's `selector_map.json`:
Withdrawal 339, EncNoteCreated 280, NoteUsed 266, Deposit 93, ExternalContractInvoked 88, OpenNoteCreated 75,
OpenNoteDeposited 75, ViewingKeySet 33, plus admin/roles events. Rank order matches the full-scan histogram
(Withdrawal > EncNoteCreated > NoteUsed > Deposit). VERIFIED.

ExternalContractInvoked in live events: VERIFIED in my own recent page (88 occurrences); code claim
VERIFIED: `starknet-privacy/packages/privacy/src/events.cairo:82` defines `pub struct ExternalContractInvoked`,
and `grep -r ExternalContractInvoked starknet-privacy-rc0/packages/privacy/src/` returns nothing — live
schema is newer than tag PRIVACY-0.14.3-RC.0, as claimed.

**Bonus resolution of their open unknowns:** two of the four "unmapped" selectors DO map to standard
starkware roles-component events (starknet_keccak match):
- `0x209ff368803f5de65188245078e888d4462f8d98697699c1dcdd8b02ffb250f` = **AppRoleAdminAdded** (3 events)
- `0x198116a5c5421876feb02bdb0b472ace223bdde3dbd87f92db8d735a233fbb0` = **AppRoleAdminRemoved** (3 events)
Their INFERRED guess (roles-component, irrelevant to notes) is confirmed for these. `0x15e9615...` and
`0x3940d40...` (2 events total) remain unmapped against ~40 candidate names — still irrelevant to note indexing.

## Check 4 — arithmetic re-derivation from their raw files

| claim | recomputed | verdict |
|---|---|---|
| total events 118,372 | sum of selector_counts = 118,372 | MATCH |
| active blocks 28,260 | len(active_blocks) = 28,260 | MATCH |
| pool JSON mean/med 671/340 B | 671.3 / 340 from sample-pool-diffs.json | MATCH |
| entries mean/med 4.3/2 | 4.3 / 2.0 (population mean 4.19 cross-check) | MATCH |
| pool share of block diff ~4% | median 4.3% | MATCH |
| 7d avg 120 active blocks/day, 526 events/day | 120.0 / 526.3 (window = latest − 7×51,650) | MATCH |
| ~80 KB/day raw | 120 × 671 = 80,520 B | MATCH |
| ~25 KB/day zstd | 80,520 / 3.23 = 24,929 B | MATCH |
| backfill ~19 MB raw / ~6 MB zstd | 28,260 × 671 = 19.0 MB; /3.23 = 5.9 MB | MATCH |
| blocks/day 51,650 | 86,400 / 1.6728 s = 51,650 | MATCH |
| peak day ~3.7 MB raw | 5,507 × 671 = 3.7 MB | MATCH (see correction 3) |
| 100x → ~8 MB/day | 12,000 × 671 = 8.1 MB | MATCH |
| whole-chain 203 MB/day raw | 51,650 × 4,649 B (their stated mean) = **240 MB** | ~18% inconsistency, see below |
| whole-chain 30 MB/day zstd | 51,650 × (114,310/197 = 580 B) = 30.0 MB | MATCH |

**Correction (minor, internal inconsistency):** the 203 MB/day whole-chain figure derives from their
197-block concat measurement (774,956 B / 197 = 3,934 B/block → 203 MB/day), while their reported
`full_block_state_update_bytes_mean` = 4,649 comes from a different 25-block sample and implies 240 MB/day.
Both are the same order; the honest range is 200–240 MB/day raw. zstd figure (30 MB/day) is exact from
their own compressed file. No 10x error anywhere.

**Correction (minor, understatement):** "peak 22,491 events per 50k blocks" is the FIXED window
11.20–11.25M (recomputed exactly: 22,491 events, 5,507 active blocks). The SLIDING-window peak is
**31,224 events / 7,278 active blocks** (starting block 11,218,819) → true peak-day equivalent is
~59x current, ~4.9 MB/day raw pool diffs — still trivially small, conclusion unchanged.

## Not re-verified (accepted as reported, low risk)
- Compression ratios (their .gz/.zst files exist on disk with the stated sizes; I recomputed zstd/raw byte
  ratios from file sizes: pool 20,942/6,487 = 3.23x, full blocks 774,956/114,310 = 6.78x — MATCH, though I
  did not re-run the compressors).
- Dead endpoints (blastapi/nethermind) — not re-probed; irrelevant to the positive claims.
- getStorageProof behavior — not re-probed.

## Bottom line
Every load-bearing number reproduces from independent RPC calls or from re-derivation of their raw data.
Q5 verdict (reliable free-RPC state-diff ingestion, archive depth to genesis, events-first pipeline) and
Q8 verdict (epoch bundles trivially practical) both stand. The three corrections are: (a) per-block entry
tail reaches ≥68 entries (claim said 1–19, sample-only); (b) whole-chain raw/day is 200–240 MB (203
claimed); (c) historical peak is ~31k events per sliding 50k window (~59x current), not 22.5k (~43x).
None affect any recommendation.
