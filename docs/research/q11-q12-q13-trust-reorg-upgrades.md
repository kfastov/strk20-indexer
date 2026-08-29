# Q11 / Q12 / Q13 — Trust model, reorg & finality semantics, contract upgrades

Investigated 2026-08-29 against Starknet mainnet (pool `0x040337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a`),
RPC `https://rpc.starknet.lava.build` (specVersion **0.8.1** at base path, **0.9.0** at `/rpc/v0_9` — both VERIFIED by call),
local clones of `starknet-privacy` (main @ 980da8a) and tag `PRIVACY-0.14.3-RC.0`.

Legend: **VERIFIED** = observed in code or raw RPC data during this session. **INFERRED** = plausible, grounded in docs/reasoning, not directly reproduced.

---

## Q11 — Trust model of a public indexer

### Q11.1 starknet_getStorageProof exists and works (VERIFIED)

`starknet_getStorageProof` is served by the public lava endpoint on the **0.8** path (specVersion 0.8.1) and on `/rpc/v0_9`.
Raw proof for one real pool slot saved at `findings/proof_lava.json` (11,139 bytes).

Slot tested: `auditor_public_key` → sn_keccak("auditor_public_key") = `0x18223681ac4182236a5f10794ec6fa3530a5cb1a18aff2005fbbed58772ec28`.
`starknet_getStorageAt(pool, slot, latest)` = `0x1eed60b8d483b3bede62d1cc0f32874aea30747e6943437c858359b41801bf7` (nonzero — a real value, not a vacuous proof).

Proof response structure (VERIFIED):

```json
{
  "classes_proof": [],
  "contracts_proof": {
    "nodes": [ 26 binary/edge nodes of the global contracts trie ],
    "contract_leaves_data": [{
      "nonce": "0x0",
      "class_hash": "0x67dddd89d80fedadc06b6f160798f94800a4a70164e5a24301cd0d6076b554d",
      "storage_root": "0x289a72db76870854ec3264aeeebe7a3058d9331c01614c1f835411281e14896"
    }]
  },
  "contracts_storage_proofs": [ [ 18 nodes of the pool's own storage trie ] ],
  "global_roots": {
    "contracts_tree_root": "0x6d87cf334568a171ba153f43d152e74dd20eea73335812f2df5e77933a5573d",
    "classes_tree_root":   "0x42fc7eb0dbe214586ae52014c4a7a007a33e01565bf0c7cdcf04d8464365e1e",
    "block_hash":          "0x51cafb95f6b56507db63b52698ee93d001528ec720fd7ee703cfe7c67bfd560"
  }
}
```

### Q11.2 Verification chain: slot value → block state root

1. Storage trie: walk `contracts_storage_proofs` (Pedersen Merkle-Patricia) from the slot key/value up to `contract_leaves_data.storage_root`.
2. Contract leaf: `h_Ped(h_Ped(h_Ped(class_hash, storage_root), nonce), 0)` — formula VERIFIED against docs.starknet.io/architecture/state (fetched this session).
3. Contracts trie: walk `contracts_proof.nodes` up to `global_roots.contracts_tree_root`.
4. State commitment: `state_root = h_Pos("STARKNET_STATE_V0", contracts_tree_root, classes_tree_root)` — formula VERIFIED against official docs; **not recomputed locally** (no Poseidon implementation on this machine — INFERRED that the returned roots combine to the header root).
5. The state root to compare against comes from `starknet_getBlockWithTxHashes(block).new_root`. VERIFIED that the header of the proof's `global_roots.block_hash` (block 14055308) has `new_root = 0x5f758be180ad672db678587e5b025145d2af0f84bb0791d7e90dc962a5a870e`, which differs from `contracts_tree_root` — consistent with the combined-commitment formula (post-v0.13.2 semantics).
6. Ultimate trust anchor (INFERRED, standard): the Starknet core contract on Ethereum stores the L1-accepted state root; a maximally paranoid client checks the header chain against that instead of trusting the RPC's header.

Free bonus (VERIFIED): every storage proof carries the pool's **class_hash** in `contract_leaves_data` — a client verifying any slot simultaneously verifies which implementation class the pool had at that block. Relevant to Q13 detection.

### Q11.3 Proof availability window (VERIFIED, provider-specific)

| block_id | Δ from head | result |
|---|---|---|
| latest | 0 | proof OK |
| 14048929 (l1_accepted height) | −6,352 | proof OK |
| 14030000 | −25,281 | proof OK |
| 14000000 | −55,281 | error code 42: "The node doesn't support storage proofs for blocks that are too far in the past" |

So on lava, proofs cover roughly the last few tens of thousands of blocks (somewhere between 25k and 55k; boundary not bisected). Clients must verify **promptly**, not archivally.

Provider status (VERIFIED 2026-08-29): `starknet-mainnet.public.blastapi.io` returns "Blast API is no longer available. Please update your integration to use Alchemy's API instead" (the hackathon doc's fallback list is stale). `free-rpc.nethermind.io/mainnet-juno/v0_8` was unreachable (HTTP 000). Lava was the only working public endpoint of the three during the session.

### Q11.4 What CANNOT be verified cheaply

- **Completeness of a diff/event stream (omission attack).** A proof shows a slot's value; nothing proves the indexer told you about *all* relevant slots/blocks. A malicious or broken indexer can omit a deposit (user doesn't discover a note — funds invisible until re-scan elsewhere) or omit a spend (wallet believes a note unspent — a later spend attempt reverts on-chain since the pool checks `nullifiers` in-contract; the on-chain check bounds the damage to bad UX/metadata leakage, not theft).
- **Negative statements over ranges** ("no event for you in blocks N..M") have no succinct proof at the RPC layer.
- However, **per-note non-membership IS verifiable**: the `nullifiers` map is `Map<felt252, bool>` (privacy.cairo storage struct, RC.0 clone) — a storage proof of the note's nullifier slot returning 0 proves un-spent-ness at that block. A wallet can spot-check exactly the slots it cares about.

### Q11.5 Practical mitigations for an open indexer (design recommendations)

1. **Cross-check a second RPC**: run ingest against one provider, periodically re-read a random sample of slots + the head hash from a second (note: only one of three public endpoints tested actually worked, so make providers configurable and support paid endpoints).
2. **Content-addressed epoch bundles**: publish finalized epochs as files whose name/manifest includes their hash and the covered block hashes; independent mirrors regenerate the bundle from any RPC and must reproduce the identical hash — omission by one operator becomes detectable by diffing manifests, converting a silent omission attack into a visible fork of published hashes.
3. **Spot-check storage proofs client-side**: for every note a client discovers via the indexer, fetch `starknet_getStorageProof` for (a) the note slot and (b) its nullifier slot, verify against `new_root` — cheap (one ~11 KB response covers several keys) and must happen within the proof window.
4. **Cursor canonicity discipline** (adopted from upstream §11.1): clients pin `(block_number, block_hash)`; the indexer answers "is this still canonical" with the hash→height→hash round trip.
5. Honest positioning: discovery data is **delegated trust with random audits**, not trustless; the trustless fallback is always "run the indexer yourself" (the project is self-hostable by design).

### Q11.6 Reference service's own trust statements (VERIFIED, specs/05-security-considerations.md)

- §5.5: "Users trust the service operator with request content." Operator observes which recipients are active, channel/subchannel/note counts, tokens per channel, sync timing.
- OHTTP + relay (spec 20) removes only IP↔content linkage; "Users requiring stronger privacy guarantees should run their own instance."
- §5.3: stricter RPC-fallback budgets during cold start/reorg to prevent amplification; body-size/timeout hardening table from a 2026-02-04 audit.
- Note the trust asymmetry vs. our project: the reference *discovery service* receives the user's **viewing key per request** (§5.1 SecretFelt handling) — a public STRK20 indexer that serves raw encrypted channel/note data and lets the wallet trial-decrypt locally never sees keys at all, which is a materially weaker trust requirement. (Verified §5.1 for the reference behavior; the contrast is a design statement, not upstream's.)

---

## Q12 — Finality and reorg semantics

### Q12.1 Finality tiers on Starknet, 2026 (VERIFIED by RPC)

- `pre_confirmed` (RPC ≥0.9; replaced `pending`): block being built, **no block hash**, no status. VERIFIED: `getBlockWithTxHashes({block_id:"pre_confirmed"})` works on `/rpc/v0_9` (returned block_number 14055508, status null) and is rejected on the 0.8 path ("Invalid block id"). The SDK route in the hackathon docs pins reads to `pre_confirmed` (MAINNET-DAY-0.md:82); upstream passes the tag through unchanged (discovery-service/tests/test_api.rs:93-130).
- `ACCEPTED_ON_L2` = `latest`: sequencer-final, replaceable.
- `ACCEPTED_ON_L1` = `l1_accepted` tag: state update proven & accepted on Ethereum, irreversible. Quirk (VERIFIED): the `l1_accepted` tag is accepted even on lava's 0.8.1 path, though it is nominally an 0.9 addition.

### Q12.2 Measured L1-acceptance lag (VERIFIED, single measurement 2026-08-29 ~20:06 UTC)

| tag | block | timestamp |
|---|---|---|
| latest | 14,055,281 (`0x4d64d78...`) | 1788021995 |
| l1_accepted | 14,048,929 (`0x60a9847...`) | 1788011348 |

Lag: **6,352 blocks / 10,647 s ≈ 2.96 hours**. Block cadence measured ≈ **1.7 s/block** (227 blocks in 381 s across two header reads). Upstream's spec 11.1 cites ~1.5k blocks on Sepolia; mainnet is deeper. The lag varies with proof-submission cadence — treat 2–6 h as the planning envelope (single sample: INFERRED envelope).

### Q12.3 Realistic reorg depth (web evidence)

Starknet's sequencer set is operated by StarkWare (3 sequencers post-Grinta, still one operator). Normal parent-hash reorgs are rare, but **operational rollbacks are real and deep**: after the Grinta (v0.14.0) upgrade in September 2025, mainnet halted and "a chain reorganization wiped transactions submitted during a roughly two-hour window" (root cause per incident report: ungraceful recovery from P2P issues; unprovable transaction stream forced a rollback). At current cadence a 2-hour rollback ≈ **~4,000+ blocks**. Consequence: `ACCEPTED_ON_L2` must be treated as revocable **en masse**, not just at the tip; only `ACCEPTED_ON_L1` is safe to treat as immutable.
Sources: [Blockworks on the Grinta turbulence](https://blockworks.com/news/starknet-grinta-upgrade), [Starknet Grinta blog](https://www.starknet.io/blog/starknet-grinta-the-architecture-of-a-more-decentralized-future/), [halt/restore report](https://www.bitget.com/news/detail/12560605131863).

### Q12.4 What the reference design does (VERIFIED in code + specs)

- **Canonicity check** (spec 11.1; implemented at discovery-service/src/rpc_backend.rs:460-477): hash → height → hash round trip; "a node keeps serving an orphaned block by hash", so existence is deliberately not the test; the second call is skipped when the first reports `AcceptedOnL1` (rpc_backend.rs:467-472). Pre-confirmed resolutions count as "not canonical" (no hash).
- **Planned cache rollback** (spec 11.3-11.5): store `(block_number, block_hash, parent_hash)` per ingested block; detect non-linking head; walk back to common ancestor; delete above it; re-apply. Arbitrary depth supported. During reconciliation: `BLOCK_REORGED` for rolled-back refs, RPC fallback (stricter budget) for not-yet-reindexed blocks, `SERVICE_UNAVAILABLE` while reconciling (spec 11.6 error table).
- **Cursors** (spec 06): `block_ref` (hash) pins paginated reads; the completed sync's hash becomes the next session's `last_known_block`; `BLOCK_REORGED` ⇒ client re-syncs from scratch.
- **Cold start** (spec 17): backfill with RPC-fallback serving; SQLite snapshot export/import with "block hash chain validation" on import; distribution via object storage or p2p.

### Q12.5 Per-mode semantics for OUR indexer (recommendation)

| Mode | Feed from | Reorg semantics |
|---|---|---|
| Live stream (WS/SSE) | `pre_confirmed` + `latest` | Items labeled with finality tier. `pre_confirmed` items carry no block hash and MAY vanish without any reorg signal — display-only, never persisted, never in cursors. On a detected reorg emit an explicit `rollback{to_block}` message, then replay. |
| Queryable DB / REST | `latest`, tracked with `(number, hash, parent_hash)` linkage | Full upstream-style rollback (§11.3). Cursors are `(block_number, block_hash)`; every resume validates the hash via the two-round-trip check (one round trip once ≤ l1_accepted); mismatch ⇒ `BLOCK_REORGED`, client rewinds to its last L1-final checkpoint (not "from scratch" — improvement over upstream's client action, possible because we publish finalized checkpoints). |
| Epoch bundles | only blocks ≤ current `l1_accepted` height | **Immutable by construction.** See cut rule below. |
| DB snapshots | snapshot head ≤ `l1_accepted` | Verify block-hash chain on import (spec 17.2); resuming node re-syncs the L2-tentative tail itself. |

**Concrete rule — when is an epoch bundle immutable:** cut epoch `[N, N+k)` only once `getBlockWithTxHashes({block_id:"l1_accepted"}).block_number ≥ N+k−1`; record each covered block hash in the bundle manifest and content-address the bundle. Given the measured ~3 h lag, bundles trail the head by ~3-6 h — acceptable for discovery data, and it is exactly the boundary the Grinta-class incident cannot cross (an L1-accepted state update cannot be rolled back without an L1 reorg of the core contract). Everything newer is served from the mutable live tier only.

---

## Q13 — Contract upgrades and layout versioning

### Q13.1 Upgrade mechanism in the Cairo (VERIFIED in code)

The pool is **not** behind a proxy and does **not** use OZ Upgradeable. It embeds StarkWare's **ReplaceabilityComponent** plus RolesComponent (rc0 packages/privacy/src/privacy.cairo:56-66):

```cairo
use starkware_utils::components::replaceability::ReplaceabilityComponent;   // :57
component!(path: ReplaceabilityComponent, storage: replaceability, ...);    // :64
self.roles.initialize(:governance_admin);                                   // :156
self.replaceability.initialize(upgrade_delay: Zero::zero());                // :157
```

Component semantics (verified against starkware-starknet-utils @ pinned rev 0e12df09, `packages/utils/src/components/replaceability/replaceability.cairo`, fetched from GitHub):
- `add_new_implementation(ImplementationData)` — only UpgradeGovernor; activation_time = now + upgrade_delay.
- `replace_to(ImplementationData)` — checks not-finalized, known impl, activation window; executes **`replace_class_syscall(impl_hash)`** in place (same address, same storage); optional **EIC** (external initializer contract) runs a `library_call` to `eic_initialize` for storage migration; `final: true` would permanently finalize (no further upgrades).
- `ImplementationData = { impl_hash, eic: Option<EICData>, final: bool }`.

On-chain governance state (VERIFIED via getStorageAt, latest): `upgrade_delay` slot = **0x0** and `finalized` slot = **0x0** — i.e. the operator can upgrade the pool **instantly, at any time, with zero notice**, and has not renounced that power. An indexer must be built for surprise upgrades.

### Q13.2 On-chain upgrade history of the mainnet pool (VERIFIED via binary search on getClassHashAt)

| Block | UTC time | Class hash | Event |
|---|---|---|---|
| 8,978,970 | 2026-04-20 10:08:48 | `0x30b8c540cf04d8ef0f4db2a9098d9cc0e35e83af1cb3325f5a4f40144b4b30b` | deployed (CONTRACT_NOT_FOUND at 8,978,969) |
| 11,632,886 | 2026-07-09 10:00:52 | `0x67dddd89d80fedadc06b6f160798f94800a4a70164e5a24301cd0d6076b554d` | **upgraded** (tx `0x4be26fa7600175c400d0a552ef5b21d46f1e103790e1580ce7de1563342ad36`) |
| latest (14,055,281+) | — | `0x67dddd...554d` | current |

Exactly one class transition found between deploy and head (caveat: the change-point search cannot see a change-and-revert entirely inside an interval; monotone upgrades are all found).

The upgrade transaction (INVOKE v3, sender `0x663cc699d9c51b7d4d434e06f5982692167546ce525d9155edb476ac9a117d6`) emitted four pool events in one tx — decoded by computing sn_keccak selectors:
1. `RoleGranted` (`0x9d4a59b8...`) — role granted to 0x663cc... (itself),
2. `UpgradeGovernorAdded` (`0x2143175c...`),
3. `ImplementationAdded` (`0x38a81c7f...`) data `[0x67ddd..., 0x1, 0x0]` = (impl_hash, eic=None, final=false),
4. `ImplementationReplaced` (`0x34bb683f971572e1b0f230f3dd40f3dbcee94e0b3e3261dd0a91229a1adc4b7`) same data.

So one account granted itself UpgradeGovernor and executed add+replace in a single transaction — zero-delay upgrades are not theoretical, they are how the last upgrade actually happened.

**Docs discrepancy (VERIFIED):** upstream README pins Privacy Pool `PRIVACY-0.14.3-RC.0` → class `0x52107fad...ad633`, but that hash was **never** observed at this address (deployed as `0x30b8c...`, now `0x67ddd...`). Tag dates (git): RC.0 = 06-18, RC.3 = 07-08, RC.5 = 08-12. The 07-09 upgrade is one day after RC.3 ⇒ the live class is *probably* the RC.3 build (INFERRED; class hashes are compiler-sensitive and no built artifacts exist locally to compare). Deploy (04-20) predates every RC tag ⇒ the original class is an untagged pre-RC build. Practical conclusion: **never key logic off README class hashes; read the chain.**

### Q13.3 How an indexer detects an upgrade (both channels VERIFIED on the real upgrade)

1. **State updates:** `starknet_getStateUpdate({block_number: 11632886}).state_diff.replaced_classes` = `[{class_hash: 0x67ddd..., contract_address: <pool>}]` — the canonical signal, present exactly at the upgrade block; an indexer already pulling per-block state diffs gets it for free.
2. **Event:** `ImplementationReplaced`, key `0x34bb683f971572e1b0f230f3dd40f3dbcee94e0b3e3261dd0a91229a1adc4b7`, from the pool address (there is no OZ-style `Upgraded` event on this contract). Watch `ImplementationAdded`/`ImplementationFinalized` too — with a nonzero future upgrade_delay these would give advance warning; today delay=0 makes Added and Replaced simultaneous.
3. **Defense in depth:** periodic `starknet_getClassHashAt(pool, latest)` compare; plus every storage proof already returns the current `class_hash` in `contract_leaves_data` (Q11.2), so proof-checking clients detect upgrades independently of the indexer.

### Q13.4 Layout versioning discipline (spec + concrete evidence it matters)

specs/10-contract-versioning.md (VERIFIED): bind to one whitelisted pool address; on upgrade determine the upgrade block; use old slot logic before it and new logic at/after it; keep both calculators; config maps versions to block ranges (`layout_versions: [{version, from_block, to_block}]`). Failure mode (§10.3): alert, return `SERVICE_UNAVAILABLE` for compatibility-broken queries, require manual config to resume.

Real layout drift RC.0 → main (VERIFIED by diff of privacy.cairo/events.cairo between the two checkouts):
- substorage rename `roles: RolesComponent::Storage` → `common_roles: CommonRolesComponent::Storage` (different slot addresses for governance state);
- `blocked_open_note_depositors: Map<ContractAddress, bool>` → `open_note_depositor_screening_policies: Map<ContractAddress, OpenNoteScreeningPolicy>` (renamed + retyped ⇒ new slots, new event `OpenNoteScreeningPolicySet` replacing `OpenNoteDepositorBlockSet` ⇒ **changed event selector**);
- new event `ExternalContractInvoked` (main events.cairo:82-93).
- The core discovery surfaces — `recipient_channels`, `outgoing_channels`, `channel_exists`, `subchannel_tokens`, `subchannel_exists`, `notes`, `nullifiers`, `public_key`, `enc_private_key` — are **identical** between RC.0 and main. So upgrades so far preserve note-discovery layout, but peripheral layout/events already changed within one release cycle.

### Q13.5 Recommended upgrade-handling design (adopting spec 10 + evidence)

- Persist a `class_hash_history` table: `(class_hash, from_block, first_seen_tx)`, seeded by scanning `replaced_classes` during backfill (the whole history is recoverable — archive queries worked back to the 2026-04-20 deploy on the public endpoint).
- Map class hashes → layout/decoder version in **config**, per spec 10.2, with per-range slot calculators and event-selector tables (selectors change when events are renamed — seen above).
- **Failure mode (confirmed recommendation):** on an unknown class hash appearing in `replaced_classes`: (1) keep ingesting and archiving **raw** state diffs and events — they are layout-agnostic and make reprocessing possible; (2) **stop typed decoding** at that block — do not guess slots; (3) mark epochs/API degraded (`contract_version_unknown`) and alert; (4) resume typed decoding from the upgrade block after a human maps the new class hash to a decoder version. This is spec 10.3 adapted from "return SERVICE_UNAVAILABLE" to "serve verified-raw, withhold typed" — strictly better for an open indexer whose mirrors need the raw stream to stay reproducible.
- Because `upgrade_delay = 0` and `finalized = false`, there is no advance-warning window to design around; the unknown-class path must be safe to hit at any block.

---

## Caveats / open unknowns

- Poseidon combination of `global_roots` into the header `new_root` was not recomputed locally (formula from official docs); a Rust/TS verifier in the project should do this end-to-end.
- The l1_accepted lag (2.96 h) is a single sample; it depends on proof cadence.
- Storage-proof retention window measured only on lava (>25k, <55k blocks); other providers will differ.
- The README-vs-chain class-hash mismatch (0x52107 never on chain) is unexplained; likely compiler-version drift or an unreleased build. Treat chain data as authoritative.
- Change-point search would miss an upgrade that was later reverted to the identical prior class hash (no evidence of this; considered implausible).
- Blast API is dead and Nethermind's free endpoint was down during the session — the hackathon doc's provider list needs updating; secondary-provider cross-checks require finding another live endpoint.
