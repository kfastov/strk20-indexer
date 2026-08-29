# Q1 — Exact deployed mainnet STRK20 version (ground truth)

Date of investigation: 2026-08-29. Chain head at time of measurement: block 14,055,237.
Pool: `0x040337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a` (Starknet mainnet).
RPC used: `https://rpc.starknet.lava.build` (primary), `https://starknet-mainnet.public.blastapi.io/rpc/v0_8` (fallback). Nethermind free RPC did not resolve from this machine.

## TL;DR version table

| Block range | Class hash | Upstream tag / source | Evidence |
|---|---|---|---|
| < 8,978,970 | (contract does not exist — `CONTRACT_NOT_FOUND`) | — | binary search of `starknet_getClassHashAt`, VERIFIED |
| 8,978,970 – 11,632,885 (2026-04-20 → 2026-07-09) | `0x30b8c540cf04d8ef0f4db2a9098d9cc0e35e83af1cb3325f5a4f40144b4b30b` | **PRIVACY-0.14.2-RC.3** (commit `37fddf0`), the "pre-screening / compatibility" pool | hash→tag binding VERIFIED in upstream README at that tag + SDK `pool-mode.ts`; on-chain ABI matches that tag's source field-for-field |
| 11,632,886 – head (2026-07-09 → now) | `0x67dddd89d80fedadc06b6f160798f94800a4a70164e5a24301cd0d6076b554d` | **PRIVACY-0.14.3-RC.3** (commit `efc61cb`), the "screening" pool — INFERRED (strong; see §5) | live ABI matches RC.3 source exactly incl. `ExternalContractInvoked` (added at RC.3, 1 day before the upgrade); hash itself is recorded nowhere in the repo |

**The README class hash `0x52107fad...633` (labelled PRIVACY-0.14.3-RC.0) was NEVER deployed on mainnet.** It appears in the README of every 0.14.3 tag and current main, and is stale/incorrect for mainnet.

## 1. Deployment block — VERIFIED

Binary search over `starknet_getClassHashAt(pool, {block_number:N})` (26 sequential probes, `CONTRACT_NOT_FOUND` = not yet deployed):

- Block 8,978,969: `CONTRACT_NOT_FOUND`
- Block **8,978,970**: `0x30b8c540cf04d8ef0f4db2a9098d9cc0e35e83af1cb3325f5a4f40144b4b30b`

Cross-check via `starknet_getStateUpdate({block_number: 8978970})`:

```json
"deployed_contracts": [{"address": "0x40337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a",
                        "class_hash": "0x30b8c540cf04d8ef0f4db2a9098d9cc0e35e83af1cb3325f5a4f40144b4b30b"}]
```

It was a plain deploy (not the Primer/replace-class pattern from `artifacts/Primer.contract_class.json` — no `replaced_classes` entry in the deploy block; that Primer artifact appears unused for this pool).

Block 8,978,970 timestamp: **1776679728 = 2026-04-20T10:08:48Z** (5 txs in block).

## 2. Upgrade history — VERIFIED

Change-point bisection of the class-hash step function between deploy block and head (22 probes, all cached probe values consistent with a single step):

- Block 11,632,885: `0x30b8c5...4b30b` (old)
- Block **11,632,886**: `0x67dddd...b554d` (new) — the only change point.

Cross-check via `starknet_getStateUpdate({block_number: 11632886})`:

```json
"replaced_classes": [{"contract_address": "0x40337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a",
                      "class_hash": "0x67dddd89d80fedadc06b6f160798f94800a4a70164e5a24301cd0d6076b554d"}]
```

Block 11,632,886 timestamp: **1783591252 = 2026-07-09T10:00:52Z**.

Upgrade transaction: `0x4be26fa7600175c400d0a552ef5b21d46f1e103790e1580ce7de1563342ad36` — INVOKE v3, sender `0x663cc699d9c51b7d4d434e06f5982692167546ce525d9155edb476ac9a117d6`, a 3-call multicall against the pool. Pool events in that tx (selectors resolved by computing `sn_keccak` of Starkware roles/replaceability component event names):

1. `RoleGranted` (key `0x9d4a59b8...`)
2. `UpgradeGovernorAdded` (key `0x2143175c...`) — governor `0x663cc699...` (the tx sender itself)
3. `ImplementationAdded` (key `0x38a81c7f...`) — data `[0x67dddd..., 0x1, 0x0]` (impl hash, no EIC, final=false)
4. `ImplementationReplaced` (key `0x34bb683f...`) — data `[0x67dddd..., 0x1, 0x0]`

The pool's constructor initializes `ReplaceabilityComponent` with `upgrade_delay: Zero::zero()` (starknet-privacy-rc0 `packages/privacy/src/privacy.cairo:159`), so add+replace in one tx is expected.

Full-range `starknet_getEvents` scan for the `ImplementationReplaced` selector across the pool's whole life (deploy→head, ~62 paginated pages on lava): see §7 — used to rule out any missed A→B→A double transitions the bisection could theoretically skip.

## 3. Hash → tag mapping

### Old class `0x30b8c5...4b30b` = PRIVACY-0.14.2-RC.3 — VERIFIED

- `git grep` across all 15 `PRIVACY-*` tags: the hash appears in the README of tags `PRIVACY-0.14.2-RC.3`, `RC.4`, `RC.5`, `RC.5-fix`, each stating (RC.3, README.md:57):
  `| Privacy Pool | ... | PRIVACY-0.14.2-RC.3 | 0x30b8c540cf04d8ef0f4db2a9098d9cc0e35e83af1cb3325f5a4f40144b4b30b |`
- SDK `sdk/src/internal/pool-mode.ts` (present at tags 0.14.3-RC.0..RC.2, line 19) pins it as the **SN_MAIN entry of `COMPATIBILITY_POOL_CLASS_HASHES`** ("deployed pre-screening pools"). The SN_SEPOLIA entry there, `0x715b22ab...`, is exactly the pool hash in 0.14.2-RC.2's README — consistent (Sepolia ran RC.2, mainnet RC.3).
- On-chain `starknet_getClass(0x30b8c5...)` ABI event enum matches 0.14.2-RC.3 source **exactly**: 11 pool events, `RolesComponent` (`starkware_utils::components::roles`), NO `ScreenerPublicKeySet` / `OpenNoteDepositorBlockSet` / `ExternalContractInvoked`. All struct fields and `#[key]` markings match `PRIVACY-0.14.2-RC.3:packages/privacy/src/events.cairo`.
- Timing note: deploy was 2026-04-20; the RC.3 tag commit is dated 2026-04-23 — the tag was cut ~3 days after deployment on the already-built source; the tag's README then documented the deployed hash.

### Live class `0x67dddd...b554d` = PRIVACY-0.14.3-RC.3 — INFERRED (strong)

The hash appears **nowhere** in any tag or main of the upstream repo (git grep across all tags), so the binding is by ABI + timing:

- Live ABI (authoritative, from `starknet_getClass`) has `privacy::privacy::Privacy::Event` with **14 pool events** including `ExternalContractInvoked`, and uses `CommonRolesComponent`.
- `ExternalContractInvoked` **first appears in `packages/privacy/src/privacy.cairo` at tag PRIVACY-0.14.3-RC.3** (grep count RC.2: 0, RC.3: 3). `CommonRolesComponent` first appears at RC.1. So the source is ≥ RC.3.
- RC.3 tagged 2026-07-08T15:28+03:00; mainnet upgraded 2026-07-09T10:00Z — the day after.
- RC.4 (2026-07-22) and RC.5 (2026-08-12) were tagged **after** the upgrade, and there was no later on-chain upgrade, so the deployed build predates them. (RC.3→RC.4 contract diff is whitespace-only outside tests — one blank line in snip12.cairo — so an RC.4 build would produce the same class hash anyway; RC.4→RC.5 really changes `hashes.cairo`/`utils.cairo` + dep revs, so RC.5's class hash differs from the deployed one.)
- Every live ABI event struct matches `PRIVACY-0.14.3-RC.3:packages/privacy/src/events.cairo` field-for-field, key-for-key (verified member-by-member; see §6).

Caveat: an untagged commit between RC.2 and RC.3 containing the same contract source would be indistinguishable without recompiling. Recompilation was not attempted (needs scarb/Cairo matching `starknet = "2.17.0"`; local scarb is 2.6.3, and the exact compiler build used by the team is unknown, so a hash match would not be guaranteed even from the right source).

### Never deployed on mainnet

- `0x52107fadffab71bdcbb6b2ccb68ba3e1b5558d94036538053e159d3076ad633` — README of 0.14.3-RC.0 through RC.5 **and current main** all label this "Privacy Pool / PRIVACY-0.14.3-RC.0". Not seen on mainnet at any block. Presumably a Sepolia deployment or a release candidate build that mainnet skipped; mainnet went pre-screening RC.3 (0.14.2) → screening RC.3 (0.14.3). **Upstream README is stale/wrong for mainnet.**
- `0x21a53b2c...` (0.14.2-RC.1 README), `0x715b22ab...` (0.14.2-RC.2 README; = Sepolia compat pool per pool-mode.ts), `0x53236397...` (0.14.2-RC.6 README) — none ever on mainnet at this address.

## 4. Matching repo revisions for the live pool

| What | Value |
|---|---|
| Monorepo tag | `PRIVACY-0.14.3-RC.3` = commit `efc61cbbdab5b714b5cf915f9735d88948e2ea82` (2026-07-08) |
| SDK at that tag | `@starkware-libs/starknet-privacy-sdk` **0.14.3-rc.3** (`sdk/package.json`) |
| discovery-core at that tag | `crates/discovery-core`, crate version `0.1.0` (version is static across all tags — pin by monorepo commit `efc61cb`, not by crate version) |
| Newest SDK (hackathon docs use it) | 0.14.3-rc.5 (tag `66e3caa`, 2026-08-12) — ABI-compatible with the deployed contract: RC.3→RC.5 touches only `hashes.cairo`/`utils.cairo`/tests on the contract side, and `interface.cairo`, `privacy.cairo`, `events.cairo`, `objects.cairo` are byte-identical, so events/entry points are unchanged |
| SDK compatibility-mode removal | `sdk/src/internal/pool-mode.ts` (class-hash → "screening" vs "compatibility" calldata mode) was **deleted in the SDK at 0.14.3-RC.3**, same release train as the on-chain upgrade; from rc.3 on the SDK assumes a screening pool |

## 5. Authoritative live event schema (from on-chain ABI, saved to `live-pool-abi.json`)

`starknet_getClass(latest, 0x67dddd...)` → `contract_class_version` 0.1.0, 20,546 sierra felts, ABI 33,748 chars. Full ABI: `findings/live-pool-abi.json`. Old class ABI: `findings/old-pool-abi.json` (1.14 MB class, fetched at block 11,632,885).

Event enum `privacy::privacy::Privacy::Event` — pool-specific events with their `sn_keccak` selectors (key[0] on the wire) and payload layout (`key`/`data` per member, in order):

| Event | selector (key[0]) | members |
|---|---|---|
| ViewingKeySet | `0x1321a492485b4f19851fb787ab3800a0030b595332cba93cd5fe40dfb5a4daf` | user_addr:ContractAddress(key), public_key:felt252(key), enc_private_key:EncPrivateKey(data) |
| Withdrawal | `0x2eed7e29b3502a726faf503ac4316b7101f3da813654e8df02c13449e03da8` | enc_user_addr:EncUserAddr(data), to_addr:ContractAddress(key), token:ContractAddress(key), amount:u128(data) |
| Deposit | `0x9149d2123147c5f43d258257fef0b7b969db78269369ebcf5ebb9eef8592f2` | user_addr:ContractAddress(key), token:ContractAddress(key), amount:u128(data) |
| AuditorPublicKeySet | `0x1201d99a15f3d88fe402ca349f486e5d3f92bd6bf41c0990d74b48c0f7b2ea1` | auditor_public_key:felt252(data) |
| ScreenerPublicKeySet † | `0x24a3c770102a21d765f1e5478b480aeb39ebc6f0a158cef07e722d74564009f` | screener_public_key:felt252(data) |
| OpenNoteCreated | `0x22330482fd296a27cf9096807b4a3622cd619d31cce42c1e55655914e8459ee` | enc_recipient_addr:EncUserAddr(data), token:ContractAddress(key), note_id:felt252(key) |
| EncNoteCreated | `0x23c20207be8b1ef4430c25eef8ce779c9745ebe04139555ae81bd4f8fdd6ec5` | note_id:felt252(key), packed_value:felt252(data) |
| OpenNoteDeposited | `0x25b6da03c4858d11cb0708d5cb6be79b190fb32eb7a7ce83804e07cbbb9bead` | depositor:ContractAddress(key), token:ContractAddress(key), note_id:felt252(key), amount:u128(data) |
| ExternalContractInvoked † | `0xa8fb36d0894f5e87797c38533a55c4486a1f35e9e9eced10f995b9639a8955` | contract_address:ContractAddress(key), selector:felt252(key) |
| NoteUsed | `0x247fc60d782e0094e7f98c47f277d92a3345d07a436f1f56b27a9b62be2322e` | nullifier:felt252(key) |
| FeeAmountSet | `0x3a71cae33f889d328d50250566d1f55971af0792b89c5b3f5fbea1f7aafc4d7` | fee_amount:u128(data) |
| FeeCollectorSet | `0x125aaf53a346c4e00244d4b9b35ef8366df1831e45931cd22d8d0211eea7347` | fee_collector:ContractAddress(data) |
| ProofValidityBlocksSet | `0x35ded6c81008684ea271437e09bf788dda262449efb89b0ef0ad492e0a81381` | proof_validity_blocks:u64(data) |
| OpenNoteDepositorBlockSet † | `0x1559ab4cffe5da273f7b9a981d025ee6ff6661f7b6f12f5e5e45c66b7a70b83` | depositor:ContractAddress(key), blocked:bool(data) |

† = **not present in the old class** (pre-upgrade block range 8,978,970–11,632,885). All other events are byte-identical between the two classes.

Flat component events also emitted by the pool (live class): Pausable (Paused/Unpaused), Replaceability (ImplementationAdded/Removed/Replaced/Finalized), CommonRoles, OZ AccessControl (RoleGranted/RoleGrantedWithDelay/RoleRevoked/RoleAdminChanged), SRC5, ReentrancyGuard. The old class differs only in Roles: `starkware_utils::components::roles::RolesComponent` instead of `common_roles::CommonRolesComponent` (and no RoleGrantedWithDelay).

### Indexer consequence

A note indexer keyed on the RC.3 event schema (`Deposit`, `Withdrawal`, `ViewingKeySet`, `OpenNoteCreated`, `EncNoteCreated`, `OpenNoteDeposited`, `NoteUsed`) **decodes the entire pool history from block 8,978,970**, because those seven events are identical in both deployed classes. Only `ScreenerPublicKeySet`, `OpenNoteDepositorBlockSet`, `ExternalContractInvoked` (and `RoleGrantedWithDelay`) cannot appear before block 11,632,886.

## 6. Verification details

- Old ABI ↔ `PRIVACY-0.14.2-RC.3:packages/privacy/src/events.cairo`: all 11 structs, members, and `#[key]` attributes match (dumped both sides, compared manually — no differences).
- Live ABI ↔ `PRIVACY-0.14.3-RC.3:packages/privacy/src/events.cairo`: all 14 structs match, incl. `ExternalContractInvoked {contract_address(key), selector(key)}`.
- Namespace check: both on-chain classes use `privacy::...` paths (Scarb package `privacy`), matching `packages/privacy` at those tags. The `artifacts/Primer.contract_class.json` on main uses `contracts::primer::...` and is unrelated to the deployed pool.
- 0.14.3-RC.0 event enum (starknet-privacy-rc0 `packages/privacy/src/privacy.cairo:118-146`) has 13 pool events (no `ExternalContractInvoked`) and `RolesComponent` — proving the live class is NOT an RC.0 build; the README's RC.0 pin is doubly wrong for mainnet (wrong hash AND wrong source revision).

## 7. Full-history ImplementationReplaced event scan

`starknet_getEvents` filtered to the pool address with `keys=[[0x34bb683f971572e1b0f230f3dd40f3dbcee94e0b3e3261dd0a91229a1adc4b7]]` (sn_keccak("ImplementationReplaced")), from block 8,978,970 to 14,055,237, paginated (~82k blocks/page on lava, ~62 pages):

**VERIFIED — scan completed (59 pages): exactly ONE `ImplementationReplaced` event in the pool's entire history**, at block 11,632,886, tx `0x4be26fa7600175c400d0a552ef5b21d46f1e103790e1580ce7de1563342ad36`, data `[0x67dddd89d80fedadc06b6f160798f94800a4a70164e5a24301cd0d6076b554d, 0x1, 0x0]`. This rules out any A→B→A transitions the two-endpoint bisection could theoretically miss: the upgrade history is exactly one replace. (Raw log: `scan_replaced.out`.)

## Raw artifacts in this directory

- `live-pool-abi.json` — authoritative ABI of live class `0x67dddd...` (extracted from `class_live.json`)
- `old-pool-abi.json` — ABI of original class `0x30b8c5...` (from `class_old.json`)
- `class_live.json`, `class_old.json` — full `starknet_getClass` responses (1.17 MB / 1.14 MB)
- `su_8978970.json`, `su_11632886.json` — state updates proving deploy and replace
- `ev_upgrade_window.json` — pool events in blocks 11,632,880–11,632,890
- `blk_deploy.json`, `blk_upgrade.json` — block headers with timestamps
- `bisect_deploy.sh`, `bisect_changes.py`, `scan_replaced.py` — reproduction scripts
