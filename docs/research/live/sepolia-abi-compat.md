# Sepolia pool: field-level ABI compatibility vs our decoder

Date: 2026-08-30. Network work was read-only: `starknet_getClass`,
`starknet_getClassHashAt`, `starknet_getBlockWithTxHashes` against
`https://starknet-sepolia-rpc.publicnode.com`, GitHub API reads, and a
blobless `git clone` of `starkware-libs/starknet-privacy`. No transactions,
no accounts, no credentials.

**Verdict up front: every one of the 5 Sepolia pool classes is byte-layout
compatible with our existing typed decode path.** Only 3 distinct ABIs exist
among the 5 class hashes, and every event our decoder consumes is
field-for-field identical (same members, same types, same key/data split)
in all of them. Sepolia support is a decoder-map entry per class hash —
no new decode logic.

## 1. What "our decoder" actually is

The decoder-map version string is a **label, never dispatched on**. Verified
in source:

- `crates/indexerd/src/config.rs` — `decoder_map: HashMap<Felt, String>`;
  values `"v1"`/`"v2"`.
- Consumers of the map (`grep -rn decoder_map crates/`): only
  `contains_key` checks — `ingest.rs:216` (degrade on unknown class at
  upgrade), `ingest.rs:406`/`422` (INIT recompute + live-class warning),
  `main.rs:72` (`--allow-class` inserts label `"custom"`). Nothing reads the
  value.

The actual typed decoding is one fixed selector-keyed path, applied
uniformly to all raw events:

| consumer | events decoded | fields relied on |
|---|---|---|
| `crates/indexerd/src/stats.rs` | Deposit, Withdrawal, EncNoteCreated, NoteUsed, OpenNoteCreated, OpenNoteDeposited, ViewingKeySet, ExternalContractInvoked | Deposit: `keys[2]`=token, `data[0]`=amount; Withdrawal: `keys[2]`=token, `data[3]`=amount (after 3-felt `enc_user_addr`); OpenNoteDeposited: `keys[2]`=token, `data[0]`=amount; ExternalContractInvoked: `keys[1]`=target; others counted only |
| upstream `discovery-core` `privacy_pool/events.rs` (rev `74841ca`, via `crates/indexerd/src/bridge.rs` `RawEventAccess`) | Deposit, Withdrawal, EncNoteCreated, OpenNoteDeposited, ViewingKeySet | Deposit: `keys[1..2]`, `data[0]`; Withdrawal: `keys[1..2]`, `data[3]`; EncNoteCreated: `keys[1]`, `data[0]`; OpenNoteDeposited: `keys[1..3]`, `data[0]`; ViewingKeySet: `keys[1..2]` |

Unknown selectors are skipped by both consumers, so events *added* in newer
classes cannot break older-class decoding and vice versa. Union of consumed
events: the 7 discovery events + ExternalContractInvoked.

`ingest.rs` itself stores raw keys/data untyped, plus pool storage diffs and
`replaced_classes` from `starknet_getStateUpdate` (that is how class history
is tracked — not via the ImplementationReplaced event).

## 2. Sepolia class timeline — VERIFIED on-chain

`starknet_getClassHashAt(pool=0x0254a6b2997ef52e9f830ce1f543f6b29768295e8d17e2267d672c552cfe0d91)`
at both sides of every boundary (RPC: publicnode Sepolia, 2026-08-30):

| block | class hash at block | timestamp (UTC) |
|---|---|---|
| 8271124 | CONTRACT_NOT_FOUND (error code 20) | — |
| **8271125 (deploy)** | `0x715b22ab…` | 2026-03-31 17:02:28 |
| 10829819 / **10829820** | `0x715b22ab…` → `0x30b8c540…` | 2026-06-15 13:50:43 |
| 11111945 / **11111946** | `0x30b8c540…` → `0x1a78d2da…` | 2026-06-23 13:23:44 |
| 11612078 / **11612079** | `0x1a78d2da…` → `0x67dddd89…` | 2026-07-05 16:51:06 |
| 12932674 / **12932675** | `0x67dddd89…` → `0x56ab118a…` | 2026-08-04 11:07:29 |
| latest | `0x56ab118a…` | — |

**Deploy block pinned: 8271125.** Binary search over
`starknet_getClassHashAt` in [8000000, 8271125], 21 RPC calls;
`CONTRACT_NOT_FOUND` at 8271124, class present at 8271125. This equals the
first-pool-event block, so `genesis_block = 8271125` for Sepolia config —
no pre-event deploy gap.

## 3. ABI identity: 5 classes, 3 distinct ABIs — VERIFIED

All five classes fetched from **Sepolia** via `starknet_getClass`
(`block_id: latest`; mainnet fallback never needed). sha256 over the raw
`abi` string as returned by the node:

| class | raw-ABI sha256 (prefix) | ABI group |
|---|---|---|
| `0x715b22ab…` (deploy) | `052e6a8837c577a1…` | **A** |
| `0x30b8c540…` (= mainnet v1) | `052e6a8837c577a1…` | **A** — byte-identical to deploy class |
| `0x1a78d2da…` | `cf73da89994eb1f6…` | **B** |
| `0x67dddd89…` (= mainnet v2) | `de8534ad9ec1b087…` | **C** |
| `0x56ab118a…` (current) | `de8534ad9ec1b087…` | **C** — byte-identical to mainnet v2 |

Cross-check against the repo's mainnet ABI dumps (normalized-JSON sha256):
`docs/research/data/old-pool-abi.json` (mainnet v1) ==
Sepolia `0x715b22ab` ABI; `docs/research/data/live-pool-abi.json`
(mainnet v2) == Sepolia `0x56ab118a` ABI. So the mainnet↔Sepolia ABI
identities hold in both directions. (Class hashes are content-derived —
the Sierra class hash commits to the ABI string — so the same hash on two
networks is the same class; fetching `0x30b8c540`/`0x67dddd89` from mainnet
would be redundant, and Sepolia served both anyway.)

Group B's full ABI is preserved at
`docs/research/data/sepolia-mid-pool-abi.json` (85 entries).

## 4. Field-level diff of `privacy::events::*` — VERIFIED

Extraction: every `"type": "event"` ABI entry (struct members with
`kind: key|data`; enum variants with `kind: nested|flat`), plus the nested
`privacy::objects::*` structs, compared across the three ABI groups
(script: sha256 over canonicalized member lists; then manual dump).

### 4.1 Events consumed by our decoder — identical in ALL groups

| event | layout (identical in A, B, C where present) |
|---|---|
| Deposit | key `user_addr: ContractAddress`, key `token: ContractAddress`, data `amount: u128` |
| Withdrawal | data `enc_user_addr: EncUserAddr`(3 felts), key `to_addr: ContractAddress`, key `token: ContractAddress`, data `amount: u128` → amount at `data[3]` in every class |
| EncNoteCreated | key `note_id: felt252`, data `packed_value: felt252` |
| NoteUsed | key `nullifier: felt252` |
| OpenNoteCreated | data `enc_recipient_addr: EncUserAddr`, key `token: ContractAddress`, key `note_id: felt252` |
| OpenNoteDeposited | key `depositor: ContractAddress`, key `token: ContractAddress`, key `note_id: felt252`, data `amount: u128` |
| ViewingKeySet | key `user_addr: ContractAddress`, key `public_key: felt252`, data `enc_private_key: EncPrivateKey`(3 felts) |
| ExternalContractInvoked | key `contract_address: ContractAddress`, key `selector: felt252` — **exists only in group C**; absent classes simply never emit it |

Nested structs are identical in all three groups:
`EncUserAddr { auditor_public_key, ephemeral_pubkey, enc_user_addr }` (3
felts), `EncPrivateKey { auditor_public_key, ephemeral_pubkey,
enc_private_key }` (3 felts). So every offset our decoders use
(`Withdrawal.data[3]`, discovery-core's `required_key`/`required_amount`
positions) is stable across all 5 classes.

Variant names in the main `Privacy::Event` enum are unchanged for all shared
events (all `nested` kind), so the `starknet_keccak(name)` selectors —
`docs/research/data/selector_map.json`, and computed at runtime by
discovery-core — are identical across classes.

### 4.2 Other `privacy::events::*` — identical wherever present

AuditorPublicKeySet (data `auditor_public_key`), FeeAmountSet (data,
u128), FeeCollectorSet (data, ContractAddress), ProofValidityBlocksSet
(data, u64): identical in A, B, C. New in B and C:
ScreenerPublicKeySet (data `screener_public_key: felt252`),
OpenNoteDepositorBlockSet (key `depositor: ContractAddress`, data
`blocked: bool`). Neither is consumed by us; both are skipped as unknown
selectors by discovery-core and ignored by stats.rs.

### 4.3 Added / removed events by group

| change | A (0x715b22ab, 0x30b8c540) | B (0x1a78d2da) | C (0x67dddd89, 0x56ab118a) |
|---|---|---|---|
| ScreenerPublicKeySet | absent | **added** | present |
| OpenNoteDepositorBlockSet | absent | **added** | present |
| ExternalContractInvoked | absent | absent | **added** |
| starkware `roles::interface::*` (20 events: GovernanceAdminAdded/Removed, OperatorAdded/Removed, SecurityAgentAdded/Removed, …) via `RolesComponent::Event` | present | **removed** — replaced by `CommonRolesComponent::Event`, an **empty** enum | removed |

The only *removed* events are the starkware roles admin events; our decoder
consumes none of them (they appear in `selector_map.json` as documentation
only; stats.rs never matches them). OpenZeppelin AccessControl events
(RoleGranted/RoleRevoked/RoleAdminChanged/RoleGrantedWithDelay) are
identical in all groups.

The main `privacy::privacy::Privacy::Event` enum differs across groups
exactly and only by the four rows above (flat `RolesEvent` →
`CommonRolesEvent`; nested additions). Enum variant *order* differs, but
order is irrelevant on the wire: emitted events carry
`starknet_keccak(variant_name)` as `keys[0]`, which is what both consumers
key on.

## 5. Verdict per class

Since the decode path is selector-keyed and identical for "v1"/"v2", the
only question per class is whether the fixed path decodes its events
correctly. It does, for all five:

| class | verdict | reason |
|---|---|---|
| `0x715b22ab…` | **compatible — map it** (label irrelevant; "v1" natural) | ABI byte-identical to mainnet v1 (`0x30b8c540`), which is already mapped |
| `0x30b8c540…` | **already in the map** (mainnet v1) | same content-addressed class on both networks |
| `0x1a78d2da…` | **compatible — map it** ("v1"-like: no ExternalContractInvoked) | all 7 consumed events byte-identical; adds only events we ignore; removes only roles events we ignore |
| `0x67dddd89…` | **already in the map** (mainnet v2) | same class on both networks |
| `0x56ab118a…` | **compatible — map it** ("v2" natural) | ABI byte-identical to mainnet v2 |

**No new decoder version is needed.** A Sepolia `ChainConfig` wants:
`pool = 0x0254a6b2…`, `genesis_block = 8_271_125`, `chain_id = "SN_SEPOLIA"`,
and a decoder map containing all five hashes. Without the three extra
entries the indexer would enter degraded mode at block 8271125 (genesis
class unknown) and typed stats would freeze at the deploy block.

One latent hazard, ruled out for on-chain history: upstream tag
`PRIVACY-0.14.2-RC.1` had a **different** layout (`OpenNoteCreated` with an
extra `#[key] depositor`; AuditorPublicKeySet/FeeAmountSet/FeeCollectorSet/
ProofValidityBlocksSet fields as keys instead of data). That layout never
reached Sepolia: the deployed genesis class ABI is already RC.2-shaped
(VERIFIED from the fetched class itself, not from tags). Scope note: this
report covers the **event** ABI; storage-slot layout across classes (read by
the compat path's `read_slot_as_of`) was not audited here.

## 6. Upstream tag correlation

`git clone --filter=blob:none https://github.com/starkware-libs/starknet-privacy.git`;
19 tags. GitHub code search for all five class hashes in `org:starkware-libs`
returns 0 hits (`gh api search/code`), so no published hash registry exists;
correlation is via ABI shape + dates. Blob-level facts (VERIFIED in git):

- `events.cairo` (`packages/privacy/src/events.cairo`) blob generations:
  RC.1 `a90719a2` → RC.2–RC.6 `841d6fdc` → SCREENING-AUDIT_BASE `832f49ba`
  (event still named `DepositorBlockSet`) → POST-INTERNAL-AUDIT & 0.14.3-RC.0–RC.2
  `f75a032d` (renamed `OpenNoteDepositorBlockSet`) → V2-tag & 0.14.3-RC.3–RC.5
  `18e836f4` (adds `ExternalContractInvoked`).
- `privacy.cairo` Event enum: `RolesComponent` until 0.14.3-RC.0 inclusive;
  `CommonRolesComponent` from 0.14.3-RC.1 (same commit adds
  ComputeAndInvoke). Enum at RC.2 matches ABI group A verbatim; at RC.1/RC.2
  (0.14.3) matches group B; at the V2 tag matches group C.
- `CONTRACT_V1_DEPLOYED_MAINNET_2026-04-20` and `PRIVACY-0.14.2-RC.3` point
  at the same commit `37fddf0f4e`; `CONTRACT_V2_DEPLOYED_MAINNET_2026-07-08`
  = `74841caf04` (our pinned Cargo rev).

| class | tag correspondence | status |
|---|---|---|
| `0x30b8c540…` | `CONTRACT_V1_DEPLOYED_MAINNET_2026-04-20` = `PRIVACY-0.14.2-RC.3` (`37fddf0`) | given fact; corroborated: tag identity VERIFIED in git, ABI matches RC.3 source |
| `0x67dddd89…` | `CONTRACT_V2_DEPLOYED_MAINNET_2026-07-08` (`74841ca`) | given fact; corroborated: Sepolia switched to it 2026-07-05 16:51 UTC, 73 min after the tagged commit (15:38 UTC), and only the V2-era source has ExternalContractInvoked |
| `0x715b22ab…` | **PRIVACY-0.14.2-RC.2-era** (`ca8dd47` ±): ABI byte-equals the RC.2/RC.3 surface; RC.1 excluded by field layout, RC.3 excluded by date (2026-04-23 > deploy 03-31). Deploy (03-31 17:02 UTC) precedes the RC.2 commit timestamp (04-01 10:10 UTC) by ~17 h, so likely an immediately-pre-RC.2 build of the same source. Same-ABI/different-hash vs `0x30b8c540` is explained by RC.2→RC.3 compiler bump (`starknet 2.17.0-rc.4` → `2.17.0`) + non-event source changes (hashes.cairo, interface.cairo, utils.cairo) | INFERRED |
| `0x1a78d2da…` | **PRIVACY-0.14.3-RC.1-era** (`c0d040d` ±): Event enum matches RC.1/RC.2 source exactly (CommonRoles + Screener + OpenNoteDepositorBlockSet, no ExternalContractInvoked); RC.0 excluded (still RolesComponent), AUDIT_BASE excluded (event named DepositorBlockSet there). Deployed 06-23, six days before the RC.1 commit date (06-29) → a pre-RC.1 untagged build of the RC.1 surface | INFERRED |
| `0x56ab118a…` | **PRIVACY-0.14.3-RC.4-era** (`722d1cf` ±): ABI byte-identical to V2; events.cairo and privacy.cairo unchanged across V2-tag/RC.3/RC.4/RC.5, so ABI cannot discriminate; deployed 2026-08-04, between RC.4 (07-22) and RC.5 (08-12) → latest tag before deploy is RC.4. Not yet on mainnet | INFERRED |

## 7. Evidence log

RPC (all POST `https://starknet-sepolia-rpc.publicnode.com`, JSON-RPC 2.0):

- `starknet_getClass {block_id:"latest", class_hash:<each of the 5>}` — all
  5 served by Sepolia; `result.abi` lengths 44093 (A), 32874 (B), 33748 (C).
- `starknet_getClassHashAt {block_id:{block_number:N}, contract_address:pool}`
  — binary search 8000000→8271125 (21 calls, transcript in session; endpoints
  8271124 = error code 20 CONTRACT_NOT_FOUND, 8271125 = `0x715b22ab…`), plus
  8 boundary calls (§2 table) and `block_id:"latest"`.
- `starknet_getBlockWithTxHashes {block_id:{block_number:N}}` for the 5
  boundary blocks → timestamps in §2. (publicnode 403s Python-urllib's
  default UA; `User-Agent: curl/8.7.1` works.)

GitHub:

- `GET https://api.github.com/repos/starkware-libs/starknet-privacy/tags?per_page=100`
  → 19 tags (list in §6).
- `gh api search/code?q=org:starkware-libs+<class hash>` → `total_count: 0`
  for all hashes tried.
- `git clone --filter=blob:none --no-checkout` of the repo; blob/commit
  inspection via `git rev-parse <tag>:packages/privacy/src/{events,privacy}.cairo`,
  `git diff <blob> <blob>`, `git show <tag>:…`, `git log -1 --format=… <tag>`.

Local:

- Decoder inventory: `crates/indexerd/src/{ingest.rs,stats.rs,config.rs,bridge.rs,main.rs}`;
  upstream engine at `~/.cargo/git/checkouts/starknet-privacy-c9b91989124c4d4d/74841ca/crates/discovery-core/src/privacy_pool/events.rs`.
- ABI extraction/diff script and the five raw `starknet_getClass` responses:
  session scratchpad (`abi/extract_events.py`, `abi/class_0x*.json`).
- Repo-dump cross-check hashes: §3 (old-pool-abi.json == group A,
  live-pool-abi.json == group C, normalized JSON).
