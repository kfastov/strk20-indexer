# Q2 / Q3 / Q10 — Full trace of reference discovery (starknet-privacy, tag PRIVACY-0.14.3-RC.0)

Date: 2026-08-29. All paths below are relative to
`/private/tmp/claude-501/-Users-konstantinfastov-Projects-strk20-indexer/b9b259a5-132a-4a96-b7c3-68d3231f50a6/scratchpad/starknet-privacy-rc0`
unless prefixed `main:` (= the `starknet-privacy` clone @ 980da8a).

## 0. TL;DR verdicts

- **Q2 (keyless local discovery reproducible client-side): YES** (VERIFIED by full code trace).
  Every reference flow is a pure function of `(viewing_key, user_addr, recipients?, storage slot reads of the pool contract, pool events)`.
  The reference discovery-service holds **no private state**: the wallet POSTs its viewing key in the request body
  (`viewing_key` field in every `/v1/sync/*` and `/v1/history` request — `crates/discovery-service/src/api/handlers.rs:96-102`,
  spec `specs/06-api-design.md` §6.5), and the server just runs discovery-core against `starknet_getStorageAt`/`starknet_getEvents`.
  A client with the key and the same public data reproduces bit-identical results.

- **Q3 (minimal public dataset):** **"snapshot + diffs" OR "full diffs from pool deployment"; append-only diffs after an arbitrary cursor are NOT sufficient for initial sync; arbitrary point `getStorageAt` is NOT required if the indexer mirrors the pool's full storage.**
  All storage reads are point reads of slots of the single pool contract; slot addresses depend on **decrypted secrets** (channel keys), so a keyless server cannot precompute per-user slot lists — but a keyless server CAN mirror the *entire* pool contract storage (slot → value [+ last-written block]) from state diffs, and that mirror answers every read discovery-core ever makes. Incremental resume from a block cursor works without rescan (see §5).

- **Q10 (nullifiers):** nullifier = `poseidon('NULLIFIER_TAG:V1', channel_key, token, note_index, 0, private_key)` (hashes.rs:129-143).
  Spent test = storage slot `sn_keccak('nullifiers') ⊕ pedersen-map(nullifier)` ≠ 0 (storage_slots.rs:162-164, views.rs:240-247).
  The contract ALSO emits `NoteUsed { #[key] nullifier }` on every spend (packages/privacy/src/events.cairo:90-95;
  write path privacy.cairo:570-583). **VERIFIED on mainnet**: recent pool blocks contain events with key[0] =
  `0x247fc60d…be2322e` = sn_keccak("NoteUsed"), key[1] = nullifier, empty data. So spent-state is maintainable
  incrementally from either an event stream or a storage-diff stream — no point queries needed after initial sync.

---

## 1. Crypto & key model (privacy_pool/hashes.rs, decryption.rs, contract utils.cairo)

- Single secret: the **viewing key** `k` (a Stark-curve scalar; called `private_key` in code, `SecretFelt` wrapper with zeroize-on-drop, types.rs:43-51). Public key = x-coordinate of `k·G` on the Stark curve (`starknet_crypto::get_public_key`; contract `derive_public_key`, utils.cairo:236-239). Registered on-chain write-once in `public_key: Map<ContractAddress, felt252>`.
  - **No viewing-key rotation exists**: `set_viewing_key` writes `public_key` and `enc_private_key` via `_apply_write_once`, which asserts every target slot is currently zero (privacy.cairo:893-908). One key per address, forever. `ViewingKeySet` is emitted exactly once per user. (VERIFIED in RC0 code; unchanged in main.)
  - NOTE: `k` is not merely a *viewing* key — the contract derives nullifiers and channel keys from it inside `apply_actions` (the tx carries it to the contract/TEE-server path), so it authorizes spending. For the indexer only its derivation role matters.
- Hash = `poseidon_hash_many` over felts, with 31-char ASCII domain tags (hashes.rs:11-52). All `:V1`.
- Symmetric decryption is **additive masking**: `plain = cipher − poseidon(tag, secret…)` (felt subtraction). ECDH only for incoming-channel info: recover ephemeral point from stored x (even-y), `shared = ephemeral · k`, mask keyed by `shared.x` (decryption.rs:36-54).

### Exact formulas (all VERIFIED against Cairo reference fixture tests)

| Quantity | Formula |
|---|---|
| `channel_key` (outgoing/self) | `H(CHANNEL_KEY_TAG, sender_addr, k, recipient_addr, recipient_public_key)` (hashes.rs:146-159) |
| `channel_marker` | `H(CHANNEL_MARKER_TAG, channel_key, sender_addr, recipient_addr, recipient_public_key)` (:162-175) |
| `subchannel_id` | `H(SUBCHANNEL_ID_TAG, channel_key, index, 0)` (:80-87) |
| `subchannel_marker` | `H(SUBCHANNEL_MARKER_TAG, channel_key, recipient_addr, recipient_public_key, token)` (:178-191) |
| `note_id` | `H(NOTE_ID_TAG, channel_key, token, note_index, 0)` (:101-109) |
| `nullifier` | `H(NULLIFIER_TAG, channel_key, token, note_index, 0, k_owner)` (:129-143) |
| `outgoing_channel_id` | `H(OUTGOING_CHANNEL_ID_TAG, sender_addr, k, index, 0)` (:194-206) |
| token decrypt | `token = enc_token − H(ENC_TOKEN_TAG, channel_key, index, 0, salt)` (decryption.rs:70-77) |
| incoming channel decrypt | `channel_key = enc_channel_key − H(ENC_CHANNEL_KEY_TAG, shared_x)`; `sender = enc_sender_addr − H(ENC_SENDER_ADDR_TAG, shared_x)` (decryption.rs:36-54) |
| outgoing recipient decrypt | `recipient = enc_recipient_addr − H(ENC_RECIPIENT_ADDR_TAG, sender_addr, k, index, 0, salt)` (decryption.rs:131-139) |
| note amount | packed felt = `salt·2^128 + enc_amount`; open note: `salt == 1` ⇒ plaintext amount; else `amount = (enc_amount − low128(H(ENC_AMOUNT_TAG, channel_key, token, index, 0, salt))) mod 2^128` (decryption.rs:84-126) |

## 2. Storage slot formulas (privacy_pool/storage_slots.rs; Cairo layout privacy.cairo:72-105)

Base = `get_storage_var_address(name, keys)` = `sn_keccak(name)` then, per key, `pedersen(prev, key)` (standard Cairo map addressing). Structs occupy consecutive slots `base+0, +1, …`.

| Cairo storage var | Addressing | Read by |
|---|---|---|
| `public_key: Map<addr, felt>` | `slot("public_key",[addr])` | preflight, outgoing, validation |
| `enc_private_key: Map<addr, EncPrivateKey>` | base+0 auditor_pk, +1 ephemeral, +2 enc_key | (auditor recovery; not core discovery) |
| `recipient_channels: Map<addr, Vec<EncChannelInfo>>` | len at `slot("recipient_channels",[addr])`; element i at `pedersen(base, i)` → 3 slots: ephemeral_pubkey, enc_channel_key, enc_sender_addr (storage_slots.rs:110-124) | incoming channels |
| `channel_exists: Map<felt,bool>` | `slot("channel_exists",[marker])` | preflight |
| `subchannel_tokens: Map<felt, EncSubchannelInfo>` | base+0 salt, +1 enc_token, base = `slot("subchannel_tokens",[subchannel_id])` | subchannels |
| `subchannel_exists: Map<felt,bool>` | `slot("subchannel_exists",[marker])` | preflight |
| `outgoing_channels: Map<felt, EncOutgoingChannelInfo>` | base+0 salt, +1 enc_recipient_addr; base keyed by outgoing_channel_id | outgoing channels |
| `notes: Map<felt, Note>` | `slot("notes",[note_id])` = packed_value (token field at +1 unused by discovery) | notes |
| `nullifiers: Map<felt,bool>` | `slot("nullifiers",[nullifier])` | spent check |
| `auditor_public_key` | `slot("auditor_public_key",[])` | (registration flow) |

**On-chain check (VERIFIED)**: `starknet_getStorageAt(pool, 0x18223681ac4182236a5f10794ec6fa3530a5cb1a18aff2005fbbed58772ec28 /* auditor_public_key */, latest)` on `https://rpc.starknet.lava.build` → `0x1eed60b8d483b3bede62d1cc0f32874aea30747e6943437c858359b41801bf7` (nonzero) — deployed contract uses this storage naming.

## 3. Flow-by-flow trace

### 3.1 Incoming (`POST /v1/sync/incoming_state` → sync/incoming_state.rs::sync_incoming_state)
Inputs: public `recipient_addr`, cursor, block_ref; secret `viewing_key k`.
1. **Channel count**: read `recipient_channels` Vec length slot for recipient (views.rs:129-136). **Termination: explicit on-chain counter.**
2. **Channel batch**: for i in `last_channel_index+1 .. count`, read 3 slots per element (address = `pedersen(base,i)+{0,1,2}`) — addresses depend only on `recipient_addr` and `i` (public!). Decrypt each with ECDH(k) → `{channel_key, sender_addr}` (incoming_channels.rs:100-160).
3. **Subchannels per channel**: for j = 0,1,2,…: compute `subchannel_id = H(…, channel_key, j, 0)` → read 2 slots → **sentinel: salt == 0 stops** (subchannels.rs:77-102). Slot address requires decrypted `channel_key` ⇒ NOT precomputable without the key.
4. **Notes per subchannel**: 2-phase (notes.rs, last_note_index.rs):
   - boundary: batch existence probes of `note_id(channel_key, token, start+offset)` at offsets `0,1,3,7,…,2^30−1`, then bisection → exact `total_n_notes`. **Termination: first zero-valued note slot (notes are allocated densely per subchannel index).**
   - linear scan `start..=last`: per note compute nullifier → batch-read `nullifiers` slots (spent → skip), then batch-read `notes` slots **with `last_update_block`** → decrypt amount. `block_number` is reported per note for the 10-block maturity rule (notes.rs:38-59).
5. Cursor (`DiscoveryCursor{channels{ChannelCursor{subchannels{SubchannelCursor}}}}`, cursor.rs) is client-held opaque resume state; server enforces only an IO budget (io_budget.rs; costs discovery/mod.rs:17-56).

Values that must be **decrypted before the next slot address can be derived**: `channel_key` (gates subchannel_id, note_id, nullifier slots); decrypted `token` (gates note_id/nullifier). For outgoing: decrypted `recipient_addr` (gates the `public_key` read and channel_key derivation). This is the crux of Q3: per-user slot lists are key-dependent, so a keyless server can only serve discovery by holding *all* pool slots.

### 3.2 Outgoing (`POST /v1/sync/outgoing_state` → sync/outgoing_state.rs)
Inputs: public `sender_addr`, optional `recipients` filter; secret k.
1. For i = 0,1,2,…: `outgoing_channel_id = H(OUTGOING_CHANNEL_ID_TAG, sender_addr, k, i, 0)` → read 2 slots → **sentinel salt == 0 stops** (outgoing_channels.rs:181-220). Decrypt recipient; read recipient's `public_key` slot; derive `channel_key = H(CHANNEL_KEY_TAG, sender_addr, k, recipient, recipient_pk)`.
2. Subchannels: same as incoming (sentinel).
3. Notes: only boundary probing (`find_last_note_index`) → `last_note_index` per subchannel; no amount decryption in this flow.
4. `precompute_channels`: for requested recipients without an on-chain channel, batch-read their `public_key` slots and derive future channel keys purely locally.

### 3.3 Preflight (`POST /v1/sync/preflight_check` → sync/preflight_check.rs:34-91)
≤4 point reads: sender `public_key`, recipient `public_key`, `channel_exists[channel_marker]`, `subchannel_exists[subchannel_marker]`, with markers derived from k. Pure function of (k, sender, recipient, token, 4 slot values).

### 3.4 History (`POST /v1/history` → history/transactions.rs, history/notes.rs)
Backward scan. Client supplies per-subchannel `channel_key`s in a `HistoryCursor` (types.rs:62-80). Per note the flow reads the note slot **with `last_update_block`** to learn which block created it, then:
- `get_block_events(block)` for selectors {Deposit, Withdrawal, EncNoteCreated, OpenNoteDeposited} to attach tx hashes/context (events.rs:223-235);
- `get_withdrawal_events(user, gap_range)` — Withdrawal has `#[key] to_addr` so it's filterable by address;
- synthetic registration tx via `get_public_key_with_block` + `ViewingKeySet` event at that block (transactions.rs:416-445).
Needs: archival events + per-slot last-written block. Nothing secret beyond the channel keys already derived.

## 4. The one non-standard dependency: `IncludeLastUpdateBlock`

`RawStorageAccess::read_slots_with_block` (storage_backend.rs:44-48) is implemented via `starknet_getStorageAt` with `response_flags: ["INCLUDE_LAST_UPDATE_BLOCK"]` (discovery-service/src/rpc_backend.rs:195-211), using the software-mansion `starknet-rust` fork (Cargo.toml rev 7caedfe) and a patched Pathfinder (TODO at rpc_backend.rs:338-343 references equilibriumco/pathfinder#3348).
**MEASURED**: public `https://rpc.starknet.lava.build` (spec 0.8.1) rejects the flag: `{"code":-32602,…"unexpected field: \"response_flags\""}`. So the reference history/notes `block_number` source is unavailable on today's public RPCs.
**Implication for our indexer**: a diff-stream indexer gets this for free — it knows the block of every slot write; alternatively `EncNoteCreated`/`OpenNoteDeposited` events give note creation blocks. This is an argument FOR the self-hosted indexer, not a blocker.

## 5. Q3 analysis — minimal public dataset

Facts:
- Every discovery read is a **point read of one contract's storage** (plus events for history). No proofs, no Merkle paths, no cross-contract reads (ERC-20 balances are not part of discovery).
- Contract write discipline (privacy.cairo `_apply_write_once`:893-908, `_apply_append`:911-914): all discovery-relevant slots are **write-once** (assert-zero-before-write) except (a) the `recipient_channels` length slot (increments on append) and (b) an open note's `packed_value`, rewritten exactly once at funding time (`_deposit_to_open_note`, asserts previous amount zero). Nothing is ever deleted or zeroed.
- Therefore the pool's storage is an **append-only accumulating map**: replaying `state_update.storage_diffs[pool]` from deployment reproduces `getStorageAt(latest)` exactly, and the last diff touching a slot gives `last_update_block`.

Consequences:
1. **Full mirror (snapshot or replay-from-deployment) is sufficient**: an indexer that maintains `slot → (value, last_block)` for the pool answers every `IViews` call; adding the 7 event selectors covers history. A keyless wallet syncs by running discovery-core logic locally against this mirror (or against a diff feed it folds into its own local mirror).
2. **Diffs after an arbitrary cursor are NOT sufficient alone**: a fresh wallet must read slots written before its cursor — channel vec elements at old indices, the vec length, subchannel/token slots, note slots, its counterparty public keys, and nullifier slots for old notes. So initial sync needs either (a) a current-state snapshot of the pool contract at the cursor block, or (b) the complete diff history from pool deployment (which is just a snapshot in流 form). Pool deployment is recent (mainnet 2026; contract already live with current class at block 13,400,000 — MEASURED), so full replay is cheap today.
3. **Resume from a block cursor works with diffs only, no rescan**: after a wallet has state at block N, everything new is visible in diffs > N: new channels append new element slots + bump the length slot; new subchannels/notes/nullifiers write fresh slots whose addresses the wallet can compute from its known channel keys (or discover generically by holding the mirror). A wallet holding a full local mirror trivially resumes; a "thin" wallet without a mirror can subscribe to exactly its predicted slots (nullifiers of its unspent notes, next-index sentinel slots for each channel_key, its channel-vec length slot) — all client-computable.
4. Reorg handling: reference uses hash-pinned `block_ref` + `last_known_block` canonicity check (specs/06, 11). A diff-stream indexer needs its own reorg rollback (undo diffs of orphaned blocks).

**Verdict Q3**: needs **snapshot + diffs** (equivalently: full diffs since deployment). Arbitrary/current `getStorageAt` is a convenience, not a necessity, once the mirror exists. No current-state RPC snapshot API is needed if you replay from deployment.

## 6. Q10 — nullifier/spent-state specifics

- Derivation inputs: `channel_key` (secret, per counterparty), `token`, `note_index`, owner viewing key `k`. Formula §1. The *recipient* of a note derives its nullifier from the **incoming** channel_key (decrypted from chain) + own k; the sender cannot (needs owner's k) — spending is owner-only.
- Spent test in reference: batch `getStorageAt(slot("nullifiers",[nullifier])) != 0` (views.rs:240-247), pre-checked *before* fetching note values to save reads (notes.rs:215-247).
- Event: `NoteUsed { #[key] nullifier }`, emitted in the same tx that writes the nullifier slot (privacy.cairo:570-583). **VERIFIED live on mainnet** (selector `0x247fc60d782e0094e7f98c47f277d92a3345d07a436f1f56b27a9b62be2322e`; sample event block 14,040,840 with nullifier as key[1]).
- Incremental client spent-state: YES, two equivalent feeds — (a) `NoteUsed` events (address-filterable only by selector; nullifier is in keys[1], so the wallet matches against its own precomputed nullifier set), (b) storage diffs on `nullifiers` map slots (wallet precomputes `slot("nullifiers",[nf])` for each of its notes). Both are pure diff-stream consumption; no point queries after initial sync.

## 7. Version drift RC0 → deployed → main

- `crates/discovery-core` RC0 vs main: only `views.rs` changed (+47 lines: `as_chunks` refactor + a new test). **No formula/layout changes.** (`git diff --stat PRIVACY-0.14.3-RC.0..main -- crates/discovery-core/src`)
- Contract RC0 vs RC5/main: adds `ExternalContractInvoked` event + `IDENTITY_KEY_TAG` (identity key for external invokes), renames `blocked_open_note_depositors` → `open_note_depositor_screening_policies` (admin-only), reworks screening. **No changes** to discovery storage vars, discovery hashes, or the 7 discovery-relevant event schemas.
- Deployed mainnet contract (class `0x67dddd89…76b554d`, ≠ RC0 README's `0x52107fad…`): recent events include `ExternalContractInvoked` (`0xa8fb36d0894f5e87797c38533a55c4486a1f35e9e9eced10f995b9639a8955`, 4 occurrences in a 15k-block sample) ⇒ deployed version is **≥ PRIVACY-0.14.3-RC.3** (first tag containing that event; VERIFIED via `git grep` across tags). Discovery logic is unaffected by RC0→RC5 drift.
- Mainnet event mix sampled (blocks ≥14,040,000, first 30): EncNoteCreated 9, Withdrawal 6, NoteUsed 5, ExternalContractInvoked 4, Deposit 2, ViewingKeySet 2, OpenNoteCreated 1, OpenNoteDeposited 1 — all RC0 selectors confirmed live.

## 8. Extra observations for the indexer design

- IO budget (io_budget.rs) & cursor caps (cursor.rs: max_channels 256, max_subchannels 64, max_note_log_index 30) are purely server-protection; a local client can ignore them (`usize::MAX` budget).
- `EncNoteCreated` events carry `(note_id, packed_value)` — an event-only pipeline could even skip note-slot reads for encrypted notes; open notes need `OpenNoteDeposited` for the funded amount.
- Withdrawal events: sender is encrypted (`enc_user_addr`, auditor-only); only `to_addr` filterable — privacy caveat noted at transactions.rs:314-316.
- specs/13-alternative-data-sources.md explicitly contemplates event-driven and hybrid cache designs; specs/06 confirms current implementation is direct `getStorageAt` per request.
- Public RPC archival reads work on lava (getStorageAt at block 13.4M OK — MEASURED); `response_flags` do not.
