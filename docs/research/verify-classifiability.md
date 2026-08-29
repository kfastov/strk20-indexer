# verify-classifiability — adversarial verification of the classifiability findings

Verifier run: 2026-08-29. Sources: local clones
- rc0 checkout: `.../scratchpad/starknet-privacy-rc0` (tag PRIVACY-0.14.3-RC.0 = fe52334)
- main clone: `.../scratchpad/starknet-privacy` (main @ 980da8a); tags resolved:
  - `CONTRACT_V2_DEPLOYED_MAINNET_2026-07-08` = **74841ca** (matches researcher's claim)
  - `CONTRACT_V1_DEPLOYED_MAINNET_2026-04-20` = 37fddf0
  - `PRIVACY-0.14.3-RC.5` = 66e3caa
- Live chain: pool `0x040337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a` via https://rpc.starknet.lava.build

Overall verdict: **PARTIAL — every topic-level conclusion survives adversarial re-checking, but several evidence details and the storage-count numbers are wrong and need correction.**

---

## 1. Deployed class vs rc0 — CONFIRMED conclusion, one piece of evidence is non-probative

**Re-verified live (raw RPC):**
- `starknet_getClassHashAt(pool, latest)` → `0x67dddd89d80fedadc06b6f160798f94800a4a70164e5a24301cd0d6076b554d` (matches anchor).
- `starknet_getClass` on that hash: ABI contains exactly **14** `privacy::events::*` event structs:
  ViewingKeySet, Withdrawal, Deposit, AuditorPublicKeySet, ScreenerPublicKeySet, OpenNoteCreated, EncNoteCreated, OpenNoteDeposited, **ExternalContractInvoked**, NoteUsed, FeeAmountSet, FeeCollectorSet, ProofValidityBlocksSet, **OpenNoteDepositorBlockSet** — plus component event groups incl. **CommonRolesComponent::Event** (not RolesComponent) and Replaceability (ImplementationAdded/Removed/Replaced/Finalized), AccessControl, SRC5, ReentrancyGuard, Pausable.
- Live ABI's `privacy::actions::ClientAction` enum includes **ComputeAndInvoke**; `ServerAction` includes **InvokeWithComputation**. This exactly matches the CONTRACT_V2 tag source and does NOT match rc0 (rc0 actions end at InvokeExternal / Invoke — rc0 privacy.cairo:9-14, 297, 721).
- Conclusion "deployed = CONTRACT_V2_DEPLOYED_MAINNET_2026-07-08 content, not rc0": **VERIFIED at ABI level** (event set, action enums, component set all match the V2 tag and differ from rc0). Byte-exact class-hash reproduction was not compiled — source-level match is INFERRED-but-strong.

**CORRECTION (a) — get_version is non-discriminating.** rc0 itself has `CONTRACT_VERSION: felt252 = '2.0'` (rc0 `packages/privacy/src/utils.cairo:73`), same as the V2 tag (`utils.cairo:100` at that tag) and RC5 (`:104`). On-chain `get_version()` → `["0x322e30"]` ('2.0') — re-verified via starknet_call — is therefore consistent with BOTH rc0 and V2 and proves nothing about which is deployed. The researcher cited it as evidence of "not rc0"; it isn't. The ABI difference is the actual proof.

**CORRECTION (b) — wrong selector quoted.** The researcher's evidence quotes selector `0x2a4bb4205277617b698a9a2950b938d0a41971a25e464b9819778e2fa7bd5e8` for get_version. The correct starknet_keccak("get_version") is `0x2a4bb4205277617b698a9a2950b938d0a236dd4619f82f05bec02bdbd245fab`; calling with the quoted selector returns RPC error 21 "Requested entry point does not exist" (observed). The reported *result value* (0x322e30) is nonetheless correct.

**CORRECTION (c) — "15 privacy events" in evidence text.** Live ABI has 14 privacy event structs (counted from getClass). The researcher's own `numbers` block says 14; the "15" in the evidence string is an internal inconsistency.

## 2. Event inventory — CONFIRMED; nothing missed

rc0 `events.cairo` (122 lines) defines exactly 13 events: ViewingKeySet(:5), Withdrawal(:17), Deposit(:31), AuditorPublicKeySet(:43), ScreenerPublicKeySet(:49), OpenNoteCreated(:55), OpenNoteDeposited(:67), EncNoteCreated(:82), NoteUsed(:91), FeeAmountSet(:98), FeeCollectorSet(:104), ProofValidityBlocksSet(:110), OpenNoteDepositorBlockSet(:116). The V2 tag adds ExternalContractInvoked (git diff RC.0..V2 on events.cairo, +12 lines: `#[key] contract_address`, `#[key] selector`; doc: "Calldata is not emitted").

All emit sites in rc0 grepped (`.emit(`): privacy.cairo:853-861 (server apply loop), :975 (OpenNoteDeposited), :1074/:1083/:1097/:1110/:1120/:1126 (admin setters). No emit anywhere else in packages/privacy/src (excluding tests/test_contracts, which are not part of the pool class).

**Event-silent paths re-verified:** `open_channel` (rc0 privacy.cairo:355-424) returns only `Append` + 2×`WriteOnce`; `open_subchannel` (:428-475) returns 2×`WriteOnce`. No Emit* variant. Researcher's line citations (355, 428) are exact. In rc0, `ServerAction::Invoke` → `_apply_invoke` (:931-942) emits nothing; V2's `_apply_invoke_and_deposits` emits ExternalContractInvoked for both `INVOKE_SELECTOR` and `INVOKE_WITH_COMPUTATION_SELECTOR` paths (V2 diff hunk shown at privacy.cairo ~:967-1000 of V2).

**Live 50-event sample** (starknet_getEvents from block 14,040,000, chunk 50, continuation=true):
EncNoteCreated 11, Withdrawal 11, NoteUsed 9, ExternalContractInvoked 7, Deposit 3, ViewingKeySet 3, OpenNoteCreated 3, OpenNoteDeposited 3. Consistent (same species, similar shape) with the researcher's 25-event sample. Q1 verdict "README's 'notes leave no events' is FALSE" — **CONFIRMED**.

## 3. NoteUsed / nullifier — CONFIRMED

- Struct: `NoteUsed { #[key] nullifier: felt252 }` (rc0 events.cairo:90-95). Emission: use_note builds `compute_nullifier(channel_key, token, index, owner_private_key)` (privacy.cairo:571), writes the SAME felt as a WriteOnce to `nullifiers.entry(nullifier)` and emits `EmitNoteUsed` (:575-583).
- `compute_nullifier = h(NULLIFIER_TAG, channel_key, token, index, 0, owner_private_key)` — hashes.cairo:212-219, exact match to claim. channel_key itself folds sender_private_key (hashes.cairo:102-115), so nullifier preimage is doubly secret.
- Live NoteUsed event observed: `keys=[0x247fc60d78..., 0x4818d433de...]`, `data=[]` (block 14040840, tx 0x44b912c6...) — nullifier verbatim as second key. **CONFIRMED.**

**Ciphertext location (asked by verifier task #3):**
- Enc-note ciphertext (`packed_value` = (salt, enc_amount)): **BOTH** storage (`notes` map via WriteOnce, privacy.cairo:619-620) and event (`EncNoteCreated.packed_value`, :621). Enc-note `token` field zeroed in storage (comment privacy.cairo:618; objects.cairo Note doc: "token address of the note (zero for encrypted notes)").
- Channel ciphertexts (EncChannelInfo 3 felts, EncOutgoingChannelInfo 2 felts, EncSubchannelInfo 2 felts): **storage only** (recipient_channels Vec via Append :910-913; outgoing_channels; subchannel_tokens). No event.
- ViewingKeySet's EncPrivateKey (3 felts): **both** storage and event (:331-349).
- OpenNoteCreated's enc_recipient_addr (EncUserAddr, 3 felts): **event only** (storage holds open note as (OPEN_NOTE_PACKED_VALUE, plaintext token) — utils open_note, OPEN_NOTE_SALT=1 utils.cairo:44).
- Withdrawal's EncUserAddr: event only. EncUserAddr = 3 felts (objects.cairo) ⇒ Withdrawal data = [enc_user_addr×3, amount] ⇒ amount at data[3]. Confirms decode-position claim.

## 4. Storage inventory — classification CONFIRMED, counts WRONG

rc0 privacy.cairo:72-116 non-component storage = **15 variables**, not 12:
- 10 Maps: recipient_channels (key: recipient ContractAddress — PUBLIC preimage; Vec ⇒ length slot leaks channel COUNT per recipient), outgoing_channels (key: outgoing_channel_id = h(OUTGOING_CHANNEL_ID_TAG, sender_addr, **sender_private_key**, index, 0) — SECRET), channel_exists (key: channel_marker = h(TAG, channel_key, ...) where channel_key folds sender_private_key — SECRET), subchannel_tokens (key: subchannel_id = h(TAG, channel_key, index, 0) — SECRET), subchannel_exists (key: subchannel_marker = h(TAG, channel_key, ...) — SECRET), notes (key: note_id = h(TAG, channel_key, token, index, 0) — SECRET), nullifiers (key: nullifier — SECRET), blocked_open_note_depositors (key: ContractAddress — PUBLIC), public_key (key: ContractAddress — PUBLIC), enc_private_key (key: ContractAddress — PUBLIC).
- 5 singletons (all PUBLIC preimage): auditor_public_key, screener_public_key, fee_amount, fee_collector, proof_validity_blocks.
- Plus 6 component substorages (pausable, replaceability, roles→common_roles in V2, access_control, src5, reentrancy_guard).

**CORRECTION (d):** correct tally = 15 vars: **9 publicly classifiable** (4 address-keyed maps + 5 singletons), **6 opaque secret-keyed maps**. The researcher's `storage_vars_total: 12` and `6/6` split are wrong as numbers; the *lists* in their prose are complete and correctly classified (their "config singletons" bucket just wasn't expanded). The classifiability RULE (slot classifiable iff var-name+key preimage all-public; `get_storage_var_address(name, keys)` — discovery-core storage_slots.rs:73-75) is **CONFIRMED**.

**Nuance (f):** "open-note token plaintext but slot unaddressable without note_id" is true only for a pure state-diff observer. note_id is a `#[key]` of OpenNoteCreated, so an event-reading indexer CAN address open-note slots (and V2/rc0 `get_note(note_id)` view reads them). Not a refutation — their Q2 framing was raw writes — but the caveat should say "unaddressable from the state diff alone; addressable once the OpenNoteCreated event is seen."

## 5. RC.0 → RC.5 / V2 tag drift — CONFIRMED and sharpened

- `git diff RC.0..CONTRACT_V2` (non-test src): actions.cairo +37/-, events.cairo +12, hashes.cairo +17, privacy.cairo ±160, snip12 ±100, utils ±109. Substance: adds **ExternalContractInvoked** event, **ClientAction::ComputeAndInvoke** / **ServerAction::InvokeWithComputation**, `compute_identity_key = h(IDENTITY_KEY_TAG, user_addr, user_private_key, contract_address)` (new hash, V2 hashes.cairo), `_apply_invoke_and_deposits` (emits the event, handles both invoke selectors), RolesComponent → **CommonRolesComponent** (substorage `roles` → `common_roles`), signature scheme change (`assert_valid_signature(:user_addr, :calls, :tx_info)` — SNIP-12 CallSet binding). **Privacy storage schema unchanged** (only the component substorage rename).
- `git diff RC.5..CONTRACT_V2` (non-test src): hashes.cairo 1 comment word, snip12 1 blank line, utils ~60 lines (non-schema). **RC.5 and the deployed V2 tag have identical event/storage schema.** So "build against the deployed tag (or RC.5), not RC.0" is the right recommendation; RC.0 specifically misses ExternalContractInvoked and ComputeAndInvoke.
- main vs V2: events.cairo on main replaces OpenNoteDepositorBlockSet with **OpenNoteScreeningPolicySet** (main events.cairo:128) — confirms "renamed on main"; plus interface/objects changes (+25 objects.cairo) not in the deployed class.

**CORRECTION (e):** the researcher's open-unknown "whether any newer upgrade ... (e.g. toward main's OpenNoteScreeningPolicy/**ComputeAndInvoke**) is scheduled" wrongly lumps ComputeAndInvoke with future features. **ComputeAndInvoke/InvokeWithComputation are already in the deployed class** (verified in live ABI enums and V2 diff). Only OpenNoteScreeningPolicySet (and related screening-policy plumbing) is main-only/future.

## 6. discovery-core events.rs cross-check — CONFIRMED

- `PrivacyPoolEventContent` has exactly 5 variants: Deposit, Withdrawal, EncNoteCreated, OpenNoteDeposited, ViewingKeySet (events.rs:87-93).
- `get_block_events` filters 4 selectors (Deposit, Withdrawal, EncNoteCreated, OpenNoteDeposited) — events.rs:228-233; ViewingKeySet fetched only on demand (`get_viewing_key_set_events`, keyed by user address). Withdrawal-by-address fetch also on demand.
- Decode positions verified in `TryFrom<EmittedEvent>`: Deposit amount=data[0] (:163), Withdrawal to_addr=keys[1], token=keys[2], **amount=data[3]** (:169), EncNoteCreated packed_value=data[0], OpenNoteDeposited amount=data[0]. Unknown selectors are *skipped* (parse_events filter_map) ⇒ NoteUsed / OpenNoteCreated / ExternalContractInvoked / governance events are silently ignored — confirmed.
- Spend state from storage, not events: discovery/notes.rs:223-228 `compute_nullifier(...)` → `pool.nullifier_exists_batch(...)`; views.rs:101-104 + storage_slots slot("nullifiers",[nullifier]). **CONFIRMED.**

## 7. Q6/Q18 verdicts — spot-checked, CONFIRMED

- Deposit/Withdrawal/OpenNoteDeposited carry plaintext u128 amounts and #[key] token addresses (events.cairo:16-79) ⇒ shields/unshields/TVL public. EncNoteCreated has no token field ⇒ per-token anonymity sets only partial (open-note subset). NoteUsed carries only the nullifier. ExternalContractInvoked keys = contract_address + selector, "Calldata is not emitted" (V2 events.cairo doc). Channel/subchannel counts not derivable (no events, secret slots) — all consistent with code.
- Their harmful-metrics list follows from the verified cryptography (nullifier preimage secret; deposit/withdraw legs unlinkable without auditor key). No refutation found.

---

## Corrections summary
1. (a) get_version()='2.0' does not distinguish deployed from rc0 — rc0 utils.cairo:73 also defines CONTRACT_VERSION='2.0'. The ABI diff (ExternalContractInvoked + CommonRoles + ComputeAndInvoke) is the actual proof; conclusion unchanged.
2. (b) Quoted get_version selector is wrong (…0a41971a25e464b9819778e2fa7bd5e8; correct: 0x2a4bb4205277617b698a9a2950b938d0a236dd4619f82f05bec02bdbd245fab). Wrong selector errors with "entry point does not exist".
3. (c) "15 privacy events" in evidence text should be 14 (their numbers block already says 14; live ABI counted: 14).
4. (d) Storage counts: 15 non-component vars (10 maps + 5 singletons); 9 publicly classifiable, 6 opaque — not 12 total / 6+6. Classification lists themselves are correct and complete.
5. (e) ComputeAndInvoke is NOT a future/main-only feature — it is in the deployed V2 class (live ABI ClientAction enum). Only OpenNoteScreeningPolicySet is main-only.
6. (f) Nuance: open-note storage slots become addressable to any event-reading observer (note_id is a #[key] of OpenNoteCreated); "unaddressable" holds only for state-diff-only observers.

## What was independently re-measured (raw chain data)
- getClassHashAt(pool) = 0x67dddd89d80fedadc06b6f160798f94800a4a70164e5a24301cd0d6076b554d
- getClass ABI: 14 privacy events, CommonRolesComponent, ClientAction incl. ComputeAndInvoke, ServerAction incl. InvokeWithComputation (saved at scratchpad/liveclass.json)
- starknet_call get_version → ["0x322e30"]
- getEvents sample (from block 14,040,000, 50 events, saved at scratchpad/events_sample.json): EncNoteCreated 11, Withdrawal 11, NoteUsed 9, ExternalContractInvoked 7, Deposit 3, ViewingKeySet 3, OpenNoteCreated 3, OpenNoteDeposited 3; NoteUsed keys=[selector, nullifier], data=[].
