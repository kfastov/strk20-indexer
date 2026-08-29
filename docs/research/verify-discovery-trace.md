# Adversarial verification: discovery-core trace (Q2/Q3/Q10)

Verifier: verify-discovery-trace. Date: 2026-08-29.
Target of attack: the researcher's claims in `q2-q3-q10-discovery-trace.md` (summary reproduced in tasking).
Method: independent line-by-line read of `starknet-privacy-rc0` (tag PRIVACY-0.14.3-RC.0), tag diffs in the `starknet-privacy` main clone, and 3 live mainnet RPC spot-checks.

**Overall verdict: CONFIRMED.** Every load-bearing claim survived. I found no server-only data dependency, no missed fetch-decrypt-fetch chain that breaks the pool-mirror architecture, no termination condition needing state a mirror lacks, and no formula errors. I did find four precision refinements and one new fact (deployment tags) listed at the end.

Paths below are relative to
`/private/tmp/claude-501/-Users-konstantinfastov-Projects-strk20-indexer/b9b259a5-132a-4a96-b7c3-68d3231f50a6/scratchpad/starknet-privacy-rc0/` unless noted.

---

## 1. Hunt #1 — server-contributed data a client could not derive

**Result: none found (VERIFIED).**

- Every request carries the viewing key: `crates/discovery-service/src/api/types.rs:159,300` (`pub viewing_key: SecretFelt` in the base request structs; JSON examples at lines 191-250 show `"viewing_key": "0x..."` in POST bodies).
- The server validates it purely against chain data: `crates/discovery-service/src/api/validators.rs:196-236` — fetches `public_key[user]` from the snapshot (or a cache of the same), derives `starknet_crypto::get_public_key(viewing_key)` and compares. Unregistered users (pk==0) skip validation.
- Server-side state is exactly two things, both derived from public chain data:
  - `crates/discovery-service/src/public_key_cache.rs` — cache of immutable on-chain public keys ("Public keys are immutable once registered on-chain, so cache entries never go stale").
  - `crates/discovery-service/src/indexer.rs` — a WebSocket `newHeads` subscriber feeding `ChainState` (chain head + canonicity checks for `block_ref` validation). No event index, no per-user state, no calldata parsing.
- All discovery reads go through `RawStorageAccess { read_slot, read_slots, read_slots_with_block }` (`crates/discovery-core/src/storage_backend.rs:34-48`) and `RawEventAccess::get_events` (`crates/discovery-core/src/events_backend.rs:15-39`). The `IViews` blanket impl (`privacy_pool/views.rs:121-278`) maps every view to point slot reads of computed addresses. No RPC method other than getStorageAt-style slot reads and getEvents is used by the flows.
- History (`history/transactions.rs:31-157`) consumes `IViews + IEvents` only; channel keys come from the client-supplied `HistoryCursor.subchannels[].channel_key`. The registration tx is reconstructed from `public_key` slot last-update-block + `ViewingKeySet` event (`transactions.rs:416-445`) — both public.
- Cursors are round-tripped to the client and serialize the channel keys via `secret_felt_serde` (`discovery/incoming_channels.rs:33-42`) — i.e. the secrets live client-side between calls; the server keeps nothing.

## 2. Hunt #2 — fetch-decrypt-fetch dependency chains

**Result: chains exist and are slightly DEEPER than the summary wording, but all links resolve against the single pool contract's storage — architecture claim unharmed.**

Exact dependency graph established from code:

- **Incoming, level 0 (key-independent addresses):** `recipient_channels` base = `sn_keccak("recipient_channels") + pedersen(recipient)`; length at base; element i at `pedersen(base, i)`, 3 consecutive slots (`storage_slots.rs:110-124`). A server/mirror can serve these without any key.
- **Incoming, level 1:** `decrypt_channel_info` (`decryption.rs:36-54`): recover ephemeral point from stored x-coordinate, `shared = ephemeral * k`, `channel_key = enc_channel_key − poseidon(ENC_CHANNEL_KEY_TAG, shared.x)`, same for sender_addr. Needs only k + fetched slots. VERIFIED.
- **Incoming, level 2:** subchannel addresses need decrypted `channel_key`: `subchannel_id = H(SUBCHANNEL_ID_TAG, ck, j, 0)` (`hashes.rs:80-87`), slots at `sn_keccak("subchannel_tokens")+pedersen(id)` +0/+1.
- **Incoming, level 3 (REFINEMENT — summary understated):** note and nullifier addresses need the decrypted **token** from level 2 in addition to `channel_key`: `note_id = H(NOTE_ID_TAG, ck, token, i, 0)`, `nullifier = H(NULLIFIER_TAG, ck, token, i, 0, k)`. So incoming is a **two-stage** fetch-decrypt-fetch (channel info → token → notes), not one. All stages are point reads of pool storage.
- **Outgoing (REFINEMENT — summary imprecise):** `outgoing_channel_id = H(OUTGOing_CHANNEL_ID_TAG, sender, k, i, 0)` is pure-k, but the outgoing **channel_key** is NOT a poseidon chain over k alone: `discover_outgoing_channels` (`discovery/outgoing_channels.rs:192-217`) decrypts `recipient_addr` from the fetched slot pair, then **fetches `public_key[recipient_addr]`** — a slot whose address depends on a decrypted value — and only then computes `channel_key = H(CHANNEL_KEY_TAG, sender, k, recipient, recipient_pk)` (`hashes.rs:146-159`). Depth-2 chain: k → outgoing slot → decrypt → public_key slot → derive. Again: the extra fetch is still a pool-storage point read, so a full mirror answers it; but a *pre-filtered* diff feed for a wallet cannot be computed keylessly (researcher already conceded this for subchannels/notes; it equally applies here).
- **Preflight** (`sync/preflight_check.rs:34-91`): exactly 4 point reads (pk sender, pk recipient, channel_exists marker, subchannel_exists marker), markers derived from k + fetched recipient_pk. VERIFIED.

No chain requires anything outside `(k, user_addr, pool storage, pool events)`.

## 3. Hunt #3 — termination conditions vs current-state requirements

**Result: exactly as the researcher described (VERIFIED).**

- Incoming channels: explicit on-chain counter — the Vec length slot at `recipient_channels` base (`views.rs:129-136` `get_num_of_channels`), consumed in `discover_incoming_channels_paginated` (`incoming_channels.rs:191-211`; completion when `remaining == 0` or all fetched).
- Outgoing channels: sentinel `salt == 0` at the next computed id (`outgoing_channels.rs:197-199`).
- Subchannels: sentinel `salt == 0` (`subchannels.rs:93-96`).
- Notes: exponential probe at offsets `[0,1,3,7,...,2^30−1]` + bisection to first zero slot (`last_note_index.rs:146-184,106-128`); dense indexing assumed; errors out if >2^max_note_log_index notes.
- No head pointers, no registries outside pool storage. **Initial sync needs current state** (counter value, pre-cursor channel elements, pk slots, existing notes) — the researcher's "append-only diffs after an arbitrary cursor are NOT sufficient for initial sync" is correct and its converse ("snapshot + diffs suffice") holds because:
  - `_apply_write_once` (`packages/privacy/src/privacy.cairo:893-908`) asserts every written felt's slot reads zero first (`errors::NON_ZERO_VALUE`) — all discovery writes except two go through it (incl. nullifiers at `privacy.cairo:575-581`, notes, markers, subchannel/outgoing infos, public_key, enc_private_key via `set_viewing_key` at `privacy.cairo:312-350`).
  - The two mutable exceptions, confirmed: Vec length increments via `_apply_append` → `recipient_channels.push` (`privacy.cairo:910-913`), and the single open-note funding rewrite in `_deposit_to_open_note` (`privacy.cairo:~947-975`: asserts `salt == OPEN_NOTE_SALT`, `current_amount == 0`, writes new packed_value once — so even this is effectively write-twice-monotonic).
  - Nothing is ever zeroed/deleted.
- **Caveat worth stating for the indexer design (not a refutation):** post-initial-sync resume from diffs is data-sufficient, but the client/indexer must *extend its watch-set* as diffs reveal new channels → new subchannels → new notes (each discovery adds newly-computable addresses to watch). A dumb slot-subscription list fixed at cursor time is not enough; a mirror or a recompute-on-diff loop is.

## 4. Hunt #4 — formula errors

**Result: none. Rust and Cairo match exactly.**

Cross-checked `crates/discovery-core/src/privacy_pool/hashes.rs` against `packages/privacy/src/hashes.cairo` (RC0):

| item | Rust | Cairo | match |
|---|---|---|---|
| nullifier | `H(NULLIFIER_TAG:V1, ck, token, i, 0, k)` (hashes.rs:129-143) | hashes.cairo:212-219 | YES |
| note_id | `H(NOTE_ID_TAG, ck, token, i, 0)` (101-109) | 189-193 | YES |
| subchannel_id | `H(SUBCHANNEL_ID_TAG, ck, j, 0)` (80-87) | 159-161 | YES |
| enc_token mask | `H(ENC_TOKEN_TAG, ck, j, 0, salt)` (90-98) | 63-65 | YES |
| enc_amount mask | `H(ENC_AMOUNT_TAG, ck, token, i, 0, salt)` (112-126) | 199-205 | YES |
| channel_key | `H(CHANNEL_KEY_TAG, sender, k, recipient, r_pk)` (146-159) | 102-115 | YES |
| channel/subchannel markers | (162-191) | 138-181 | YES |
| outgoing id / enc_recipient mask | (194-223) | 121-131, 85-95 | YES |
| ECDH channel decrypt | point-from-x, `*k`, mask-subtract (decryption.rs:36-54) | (encrypt side in utils) | YES (fixture-tested) |
| packed note | `salt*2^128 + enc_amount`, open salt==1 plaintext, wrapping-sub mod 2^128 (decryption.rs:79-126) | objects.cairo Note doc (91-99) | YES |

- Storage addresses (`storage_slots.rs`): standard `get_storage_var_address` (sn_keccak + pedersen per key), structs consecutive (+1/+2), Vec element `pedersen(base, index)`. All names match the contract Storage struct (`privacy.cairo:73-116`): `recipient_channels`, `outgoing_channels`, `channel_exists`, `subchannel_tokens`, `subchannel_exists`, `notes`, `nullifiers`, `public_key`, `enc_private_key`, `auditor_public_key`. Fixture test `test_storage_slots_with_cairo_vectors` (storage_slots.rs:171-226) pins them to Cairo-generated vectors.
- **Stale comment, not a bug:** storage_slots.rs:155 says `notes: LegacyMap<NoteId, bool>`; the contract actually stores `Note { packed_value, token }` (2 slots, objects.cairo:91-99; token populated only for open notes). Discovery reads only the base slot (packed_value) — correct behavior, misleading comment.
- Spent test: `nullifiers` map is `Map<felt252, bool>`; `nullifier_exists` = slot != 0 (`views.rs:240-247`, `privacy.cairo:1013-1014`). `use_note` writes the nullifier write-once and emits `EmitNoteUsed(NoteUsed { nullifier })` in the same action list (`privacy.cairo:570-583`). VERIFIED.
- Events schemas (`events.cairo`, RC0): ViewingKeySet(user, pk keyed), Withdrawal(to_addr, token keyed; enc_user_addr in data), Deposit(user, token keyed), OpenNoteCreated(token, note_id keyed), OpenNoteDeposited(depositor, token, note_id keyed), EncNoteCreated(note_id keyed; packed_value data — duplicates storage), NoteUsed(nullifier keyed). All as claimed.

## 5. Live-chain spot checks (reproduced independently, 2026-08-29)

- `starknet_getEvents` on pool, blocks 14,040,000–14,041,000, chunk 30 → selectors seen: NoteUsed `0x247fc60d782e0094e7f98c47f277d92a3345d07a436f1f56b27a9b62be2322e` (2, incl. block **14,040,840** with nullifier `0x4818d433...` as keys[1] — matches researcher's sample), EncNoteCreated `0x23c20207...` (4), Withdrawal `0x2eed7e29...` (2), ExternalContractInvoked `0xa8fb36d0...` (2).
- `starknet_getStorageAt` with `response_flags:["INCLUDE_LAST_UPDATE_BLOCK"]` on lava → `{"code":-32602,...,"reason":"unexpected field: \"response_flags\""}` — rejection reproduced verbatim.
- Same call without flags on auditor slot `0x18223681...ec28` → `0x1eed60b8d483b3bede62d1cc0f32874aea30747e6943437c858359b41801bf7` — matches researcher's value.
- Fork pin confirmed: `crates/*/Cargo.toml` → `software-mansion/starknet-rust` `rev = "7caedfe"` for core/crypto/providers/tungstenite.
- IO cost constants and cursor defaults confirmed (`discovery/mod.rs:18-45`: 1/3/2/2/3/1/1/10/1024/10; `discovery/cursor.rs:34-36`: 256/64/30).

## 6. Version drift — confirmed and strengthened with NEW facts

- `git diff PRIVACY-0.14.3-RC.0..main -- packages/privacy/src/{hashes,events,privacy}.cairo`:
  - hashes.cairo: purely additive `IDENTITY_KEY_TAG:V1` + `compute_identity_key` (ComputeAndInvoke feature). No discovery formula touched.
  - events.cairo: adds `ExternalContractInvoked{contract_address, selector keyed}`; renames admin event `OpenNoteDepositorBlockSet` → `OpenNoteScreeningPolicySet`. None of the 7 discovery events changed.
  - privacy.cairo storage: only `blocked_open_note_depositors` → `open_note_depositor_screening_policies` (admin) and a roles-component rename. All 10 discovery vars identical.
  - discovery-core RC0..main: only `views.rs` (cosmetic chunk-destructuring refactor + one new test). Claim "functionally identical" VERIFIED.
- **NEW (researcher missed these):** the repo carries deployment tags
  - `CONTRACT_V1_DEPLOYED_MAINNET_2026-04-20` → commit `37fddf0f4e` (2026-04-23)
  - `CONTRACT_V2_DEPLOYED_MAINNET_2026-07-08` → commit `74841caf04` (2026-07-05), an ancestor of RC.5, containing `ExternalContractInvoked` and `IDENTITY_KEY_TAG`.
  These (a) confirm at least one upgrade V1→V2 (consistent with the on-chain class hash differing from the RC.0 README hash — README is stale even at the V2 tag and on main, still listing `0x52107f...ad633` for tag RC.0), and (b) pin the deployed source much tighter than "≥ RC.3": the deployed V2 code is commit `74841caf04`. Note RC.3's release commit (`efc61cbbda`, 2026-07-08) is NOT an ancestor of the V2 tag — so "deployed ≥ RC.3" is technically wrong as a tag ordering, but right in the sense that matters: V2 contains the RC.3-era feature set, and V2..main diffs contain zero discovery-relevant changes. Upgrade date bound: V2 deployed 2026-07-08 (refines the researcher's "already current at block 13,400,000").

## 7. Refinements / corrections to the researcher's report (none fatal)

1. **Outgoing channel_key is not derivable from k alone** — it requires fetching `public_key[recipient]` at an address that depends on the *decrypted* recipient_addr (`outgoing_channels.rs:202-217`). Depth-2 fetch-decrypt-fetch. Fine for a full mirror; impossible for keyless per-user precomputation (same class as the subchannel/note limitation the researcher did flag).
2. **Note/nullifier addresses need the decrypted token too**, not just channel_key — the incoming pipeline has two decryption stages before note slots are addressable.
3. **"Deployed ≥ RC.3" should read "deployed = CONTRACT_V2_DEPLOYED_MAINNET_2026-07-08 (commit 74841caf04)"** — a repo tag the researcher missed; it also resolves the "which exact version" open unknown at source level (class-hash equality with `0x67dddd89...` remains unverified without building, but the tag name + feature census make it the overwhelming candidate). V1 tag pins the original deployment era (April 2026).
4. **Stale Rust comment** (`storage_slots.rs:155`) describes `notes` as `LegacyMap<NoteId,bool>`; actual contract type is `Map<felt252, Note{packed_value, token}>` (2 slots). Discovery reads only base slot; an indexer mirroring raw diffs is unaffected, but anyone "re-deriving" per-slot semantics should know slot base+1 of a note holds the token for **open** notes (plaintext token on-chain).
5. Resume-from-diffs claim holds *data-wise*, but note the operational caveat: the watch-set of relevant slots grows as decryption progresses; an indexer should mirror all pool storage rather than subscribing to a fixed slot list.

## 8. Bottom line per question

- **Q2 (keyless client-side discovery reproducible): CONFIRMED.** All four flows are pure functions of (k, user_addr, pool storage, pool events); the service adds validation, budgeting, and block-pinning only.
- **Q3 (minimal dataset = snapshot + diffs / full diff replay): CONFIRMED**, including the write-once analysis and the two mutable exceptions.
- **Q10 (nullifier formula, spent test, NoteUsed event): CONFIRMED** in code (Rust+Cairo agree) and live on mainnet.
- Version-drift and RPC-extension claims: CONFIRMED, with the V1/V2 deployment tags as an improvement over the report's open unknowns.
