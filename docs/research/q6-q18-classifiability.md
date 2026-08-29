# Q6 / Q18 — Classifiability of the deployed STRK20 privacy pool

Key: `q6-q18-classifiability`. Answers Q6 (what a public storage/event diff reveals) and Q18
(honest vs misleading explorer metrics), grounded in the DEPLOYED contract source.

## 0. Which source matches the deployed mainnet class (VERIFIED)

- Mainnet pool `0x0403...812a`, `starknet_getClassHashAt(latest)` =
  `0x67dddd89d80fedadc06b6f160798f94800a4a70164e5a24301cd0d6076b554d` (from CONTEXT, re-confirmed).
- On-chain `get_version()` (selector `0x2a4bb42...245fab`) returned `0x322e30` = ASCII `"2.0"`. VERIFIED via RPC.
- The rc0 clone (`PRIVACY-0.14.3-RC.0`) is **NOT** the deployed class. The deployed ABI
  (fetched via `starknet_getClass` on the live class hash) contains
  `privacy::events::ExternalContractInvoked` — which does **not** exist in rc0 — while still
  containing `privacy::events::OpenNoteDepositorBlockSet` (renamed to `OpenNoteScreeningPolicySet`
  on `main`). It also uses `CommonRolesComponent` (rc0 uses `RolesComponent`).
- Matching the marker set `{ExternalContractInvoked present, OpenNoteDepositorBlockSet present,
  CONTRACT_VERSION='2.0', CommonRoles}` across tags: the deployed class corresponds to the tag
  **`CONTRACT_V2_DEPLOYED_MAINNET_2026-07-08`** (git `74841ca`, 2026-07-05), which is between RC.3
  and RC.5 in event surface. RC.0/RC.1/RC.2 lack `ExternalContractInvoked`; `main` (branch) has
  renamed the block event and added `OpenNoteScreeningPolicy`/`ComputeAndInvoke` — neither is on the
  deployed class.
- **Therefore all event/storage answers below are given for the DEPLOYED tag
  `CONTRACT_V2_DEPLOYED_MAINNET_2026-07-08`, cross-checked against the live class ABI.** Where rc0
  differs I note it. This matters for the indexer: rc0's `events.cairo` is missing
  `ExternalContractInvoked`.

Live-ABI event structs (from `starknet_getClass` on the deployed class hash), field kinds:

| Event | fields (kind) |
|---|---|
| ViewingKeySet | user_addr(key), public_key(key), enc_private_key(data) |
| Withdrawal | enc_user_addr(data), to_addr(key), token(key), amount(data) |
| Deposit | user_addr(key), token(key), amount(data) |
| AuditorPublicKeySet | auditor_public_key(data) |
| ScreenerPublicKeySet | screener_public_key(data) |
| OpenNoteCreated | enc_recipient_addr(data), token(key), note_id(key) |
| EncNoteCreated | note_id(key), packed_value(data) |
| OpenNoteDeposited | depositor(key), token(key), note_id(key), amount(data) |
| ExternalContractInvoked | contract_address(key), selector(key) |
| NoteUsed | nullifier(key) |
| FeeAmountSet | fee_amount(data) |
| FeeCollectorSet | fee_collector(data) |
| ProofValidityBlocksSet | proof_validity_blocks(data) |
| OpenNoteDepositorBlockSet | depositor(key), blocked(data) |
| + component events: Pausable(Paused/Unpaused), Replaceability(ImplementationAdded/Removed/Replaced/Finalized), CommonRoles, AccessControl(RoleGranted/…), SRC5, ReentrancyGuard |

## 1. COMPLETE EVENT INVENTORY (deployed version)

Event structs are defined in `packages/privacy/src/events.cairo`; the emit sites are in
`packages/privacy/src/privacy.cairo`. rc0 line refs given for quotes (identical field layout to the
deployed tag for every event except the added `ExternalContractInvoked`).

### Contract-domain events (privacy::events)

1. **ViewingKeySet** — `events.cairo:4-14`
   ```cairo
   struct ViewingKeySet {
       #[key] pub user_addr: ContractAddress,
       #[key] pub public_key: felt252,
       pub enc_private_key: EncPrivateKey,   // 3 felts (auditor_pk, ephemeral_pubkey, enc_priv)
   }
   ```
   Emitted on viewing-key registration (`set_viewing_key`, `privacy.cairo:344`). Keys expose the
   registering address AND its plaintext public viewing key. `enc_private_key` is auditor-encrypted.

2. **Withdrawal** — `events.cairo:16-28`
   ```cairo
   struct Withdrawal {
       pub enc_user_addr: EncUserAddr,       // 3 felts, auditor-encrypted source
       #[key] pub to_addr: ContractAddress,
       #[key] pub token: ContractAddress,
       pub amount: u128,
   }
   ```
   Emitted in `withdraw` (`privacy.cairo:520`). Destination address, token, and **plaintext amount**
   are public. The *source* (who inside the pool paid) is only auditor-decryptable.

3. **Deposit** — `events.cairo:30-40`
   ```cairo
   struct Deposit { #[key] pub user_addr, #[key] pub token, pub amount: u128 }
   ```
   Emitted in `deposit` (`privacy.cairo:494`). Depositor address, token, **plaintext amount** all
   public.

4. **AuditorPublicKeySet** — `events.cairo:42-46`. Data-only. Emitted `_set_auditor_public_key`
   (`privacy.cairo:1110`), i.e. at construction and on admin rotation. Governance/config signal.

5. **ScreenerPublicKeySet** — `events.cairo:48-52`. Data-only. Emitted `_set_screener_public_key`
   (`privacy.cairo:1120`). Governance/config signal.

6. **OpenNoteCreated** — `events.cairo:54-64`
   ```cairo
   struct OpenNoteCreated {
       pub enc_recipient_addr: EncUserAddr,  // auditor-encrypted recipient
       #[key] pub token: ContractAddress,
       #[key] pub note_id: felt252,
   }
   ```
   Emitted `create_open_note` (`privacy.cairo:662`). **Token is a plaintext key.** `note_id` is a
   Poseidon commitment (opaque). Recipient only auditor-decryptable.

7. **EncNoteCreated** — `events.cairo:81-88`
   ```cairo
   struct EncNoteCreated { #[key] pub note_id: felt252, pub packed_value: felt252 }
   ```
   Emitted `create_enc_note` (`privacy.cairo:621`). Both fields are opaque commitments/ciphertext:
   `note_id = h(NOTE_ID_TAG, channel_key, token, index, 0)`; `packed_value = pack(salt, enc_amount)`
   where `enc_amount = (h(ENC_AMOUNT_TAG,channel_key,token,index,0,salt)+amount) mod 2^128`
   (`hashes.cairo:189-205`, `utils.cairo:269-274`). **No token key here** (unlike OpenNoteCreated) —
   an encrypted note does not reveal its token in the event.

8. **OpenNoteDeposited** — `events.cairo:66-79`
   ```cairo
   struct OpenNoteDeposited { #[key] pub depositor, #[key] pub token, #[key] pub note_id, pub amount: u128 }
   ```
   Emitted `_deposit_to_open_note` (`privacy.cairo:975`), when an external contract funds an open
   note. **Depositor address, token, and plaintext amount are public**; `note_id` opaque.

9. **NoteUsed** — `events.cairo:90-95`
   ```cairo
   struct NoteUsed { #[key] pub nullifier: felt252 }
   ```
   Emitted `use_note` (`privacy.cairo:582`). **The nullifier value is emitted as an event key.**
   `nullifier = h(NULLIFIER_TAG, channel_key, token, index, 0, owner_private_key)`
   (`hashes.cairo:212-219`) — a one-way commitment, not linkable to an address/note without secrets.
   VERIFIED on mainnet: NoteUsed events have `keys=[selector, nullifier]`, `data=[]`.

10. **ExternalContractInvoked** — DEPLOYED-ONLY (absent in rc0). Live ABI +
    `CONTRACT_V2_DEPLOYED...:privacy.cairo` `_apply_invoke_and_deposits`:
    ```cairo
    struct ExternalContractInvoked { #[key] pub contract_address, #[key] pub selector }
    self.emit(events::ExternalContractInvoked { contract_address, selector });
    ```
    Emitted for every `Invoke`/`InvokeWithComputation` server action. **Reveals the external contract
    address and which selector (`privacy_invoke` vs `privacy_invoke_with_computation`) was called;
    calldata is NOT emitted.** VERIFIED on mainnet (3 in a 25-event sample, keys=3 data=0).

11. **FeeAmountSet** — `events.cairo:97-101`. Data-only. `set_fee_amount` (`privacy.cairo:1074`).
12. **FeeCollectorSet** — `events.cairo:103-107`. Data-only. `set_fee_collector` (`privacy.cairo:1083`).
13. **ProofValidityBlocksSet** — `events.cairo:109-113`. Data-only. `_set_proof_validity_blocks`
    (`privacy.cairo:1126`).
14. **OpenNoteDepositorBlockSet** — `events.cairo:115-122` (deployed). `#[key] depositor`,
    `blocked: bool`. Emitted `set_open_note_depositor_blocked` (`privacy.cairo:1097`). Admin block-list
    signal. (On `main` this is renamed `OpenNoteScreeningPolicySet` with an enum policy — NOT deployed.)

### Inherited component events (also emitted by the pool, all present in live ABI)
- **PausableComponent**: `Paused`, `Unpaused` — operational.
- **ReplaceabilityComponent**: `ImplementationAdded`, `ImplementationRemoved`,
  `ImplementationReplaced`, `ImplementationFinalized` — **upgrade lifecycle**. These are the events an
  indexer must watch to detect the class-hash change we observed. `ImplementationReplaced` carries the
  new impl hash; `ImplementationAdded` announces a scheduled upgrade.
- **CommonRoles / AccessControl**: `RoleGranted`, `RoleGrantedWithDelay`, `RoleRevoked`,
  `RoleAdminChanged` — admin/governance changes.
- **SRC5**, **ReentrancyGuard**: effectively silent.

### README claim test (Q1 sub-question)

> README: "The STRK20 pool contract emits events only for deposits, withdrawals and key
> registration. Private transfers and notes leave no events."

**FALSE for the deployed contract.** VERIFIED both from the ABI and from live mainnet events. In a
25-event sample from blocks ≥14,040,000 the pool emitted: EncNoteCreated ×8, Withdrawal ×5,
NoteUsed ×4, ExternalContractInvoked ×3, Deposit ×2, ViewingKeySet ×2, OpenNoteCreated ×1. So:
- **Note creation DOES emit** `EncNoteCreated` (encrypted notes) and `OpenNoteCreated` (open notes).
- **Note usage DOES emit** `NoteUsed` (the nullifier).
- **Open-note funding DOES emit** `OpenNoteDeposited`.
- **External invokes DOES emit** `ExternalContractInvoked`.

What the README *should* say: private-transfer *content* (amounts, parties, token for enc notes) is
not in the events — but the *existence* of each note creation, each note spend, and each external
call is publicly logged with a commitment id. The claim "notes leave no events" is wrong; notes leave
**commitment** events. This is significant for the indexer's own value proposition: a wallet can get
per-block note-creation/nullifier feeds straight from events without a storage walk (see §3).

**Does NoteUsed expose the nullifier value?** YES — the nullifier is emitted verbatim as the event
key. It is a hash commitment and cannot be reversed to an address or a specific note without the
owner's private key, but the raw felt is public and is exactly the value written to the `nullifiers`
storage map (see §2). This lets a public observer count spends and detect double-spend attempts, and
lets an *owner* who knows their own nullifiers confirm on-chain spend via events instead of a storage
read.

## 2. COMPLETE STORAGE INVENTORY (deployed version)

Storage struct in `privacy.cairo:72-116` (deployed tag identical layout to rc0 except the block-list
map name). For each `Map<K,V>`, Starknet's slot address = `get_storage_var_address(name, [key...])`
(Pedersen-based on the ASCII var name + keys) — see `discovery-core/.../storage_slots.rs:73`. The
**preimage of the slot** is `(var_name, key)`; whether an observer can *classify* a raw
`(slot, value)` write hinges entirely on whether the key felt is a **public value** (address, token)
or a **secret-derived commitment** (channel_key/marker/note_id/nullifier hashes).

| Storage var | Cairo type | Key derivation | Value | Publicly classifiable from a raw slot write? | Why |
|---|---|---|---|---|---|
| `public_key` | Map<ContractAddress, felt252> | key = user **address** (public) | plaintext viewing pubkey | **YES — fully.** Observer can enumerate `slot("public_key",[addr])` for any candidate address and see if set; value is the plaintext key. | Slot preimage is a known public address. Also redundantly public via ViewingKeySet event. |
| `enc_private_key` | Map<Addr, EncPrivateKey> (3 slots) | key = user **address** | auditor-encrypted priv key (3 felts) | **YES to classify (it's a viewing-key registration for a known addr); NO to decrypt.** | Slot preimage public → observer knows *which address* registered; contents are auditor-only. |
| `recipient_channels` | Map<Addr, Vec<EncChannelInfo>> | key = **recipient address** (public); Vec base holds length, elements at `pedersen(base,i)` | ephemeral_pubkey, enc_channel_key, enc_sender_addr | **PARTIALLY.** Observer can tell *that recipient X received an incoming channel* and **count** channels per recipient (Vec length slot), but not who the sender is or the channel key. | Map key = plaintext recipient addr → the recipient (an address the observer can guess/enumerate) is leaked; ciphertext body is ECDH-encrypted to recipient. This is the one storage var keyed by a *plaintext party address*. |
| `outgoing_channels` | Map<felt252, EncOutgoingChannelInfo> | key = `outgoing_channel_id = h(TAG, sender_addr, sender_private_key, index, 0)` (`hashes.cairo:121`) | salt, enc_recipient_addr | **NO.** Opaque. | Slot preimage includes `sender_private_key` (secret). Observer cannot compute the id, so cannot even locate the slot, let alone read the recipient. |
| `channel_exists` | Map<felt252, bool> | key = `channel_marker = h(TAG, channel_key, sender_addr, recipient_addr, recipient_pubkey)` (`hashes.cairo:138`) | bool true | **NO.** Opaque. | `channel_key` is itself secret (`h(TAG, sender, sender_priv, recip, recip_pk)`), so the marker preimage is unknown. A raw write to some `channel_exists` slot tells the observer "a channel was opened" **only if they already know the var-name namespace of the slot** — but they cannot bind it to any pair of parties. |
| `subchannel_tokens` | Map<felt252, EncSubchannelInfo> (2 slots) | key = `subchannel_id = h(TAG, channel_key, index, 0)` (`hashes.cairo:159`) | salt, enc_token (`enc_token = h(ENC_TOKEN_TAG,channel_key,index,0,salt)+token`) | **NO.** Opaque; **token is encrypted.** | id depends on secret channel_key; token masked by a hash keyed on channel_key. Public observer learns neither the subchannel identity nor its token. |
| `subchannel_exists` | Map<felt252, bool> | key = `subchannel_marker = h(TAG, channel_key, recipient_addr, recipient_pubkey, token)` (`hashes.cairo:168`) | bool true | **NO.** Opaque. | Marker preimage needs secret channel_key. |
| `notes` | Map<felt252, Note> (packed_value, token) | key = `note_id = h(TAG, channel_key, token, index, 0)` (`hashes.cairo:189`) | packed_value (salt+enc/plain amount), token | **NO for enc notes; token slot is zero for enc notes. Open notes: token is stored in plaintext but the slot is still unaddressable without note_id.** | note_id needs secret channel_key. For enc notes `token` field is initialized zero (`privacy.cairo:618`), amount encrypted. For open notes `token` is plaintext in `Note.token` and `packed_value` salt=1 plaintext amount — BUT the observer cannot compute note_id to find the slot, and even reading it back requires already knowing note_id (which OpenNoteCreated/OpenNoteDeposited expose as an opaque key). |
| `nullifiers` | Map<felt252, bool> | key = `nullifier = h(TAG, channel_key, token, index, 0, owner_private_key)` (`hashes.cairo:212`) | bool true | **Existence YES (also via NoteUsed event); linkage NO.** | The nullifier felt is public (emitted + is the slot key). Observer can count set nullifiers and detect a re-spend attempt, but cannot link a nullifier to a note, channel, token, or owner (preimage includes secret channel_key AND owner_private_key). |
| `blocked_open_note_depositors` | Map<Addr, bool> | key = depositor **address** | bool | **YES — fully.** | Public address key; also via OpenNoteDepositorBlockSet event. |
| `auditor_public_key` | felt252 (single slot) | none | plaintext | **YES.** Global config, plaintext, also event. |
| `screener_public_key` | felt252 | none | plaintext | **YES.** Global config. |
| `fee_amount` | u128 | none | plaintext | **YES.** |
| `fee_collector` | ContractAddress | none | plaintext | **YES.** |
| `proof_validity_blocks` | u64 | none | plaintext | **YES.** |
| component storage (roles, access_control, pausable, replaceability, reentrancy, src5) | — | — | — | **YES** for admin/role/upgrade/pause state; standard component slots with public preimages. |

**Core classifiability rule:** a raw `(slot, value)` write is classifiable by a public observer iff
the slot's var-name+key preimage is composed only of public felts. The "leaky" vars are exactly those
keyed by a **plaintext address or token**: `public_key`, `enc_private_key`, `recipient_channels`,
`blocked_open_note_depositors`, and all the global config singletons. Everything keyed by a
**secret-derived commitment** (`channel_key`/`channel_marker`/`subchannel_id`/`subchannel_marker`/
`note_id`/`nullifier`) is opaque: the observer cannot even reconstruct the slot address, because
`channel_key = h(…, sender_private_key, …)` and `nullifier` additionally folds in
`owner_private_key`. The privacy design's crux: *every private-transfer storage slot is addressed by
a hash of a secret*, so the address space is unlinkable even though the writes are on a public chain.

Caveat (INFERRED): an observer who watches the *transaction* (not just the diff) sees the account
that submitted it, but the pool is an **account contract** invoked by the OS with a zero caller and a
validity proof (`__validate__`/`assert_valid_os_call`, `privacy.cairo:175`, `utils.cairo:481`), and
server actions arrive via an L1 message; the per-user linkage is intended to live off-chain. Slot
writes are applied by `_apply_write_once` at addresses that are opaque commitments.

## 3. Q6 DECISION TABLE — publicly derivable?

Source column: E = from events, S = from state/storage diff, T = from tx metadata.

| Quantity | Publicly derivable? | Best source | Privacy caveat |
|---|---|---|---|
| **Shields (deposits) count** | **YES** | E: `Deposit` (also `OpenNoteDeposited`) | Depositor address + token + amount all public. No hiding on the shield leg. |
| **Shield amounts + tokens (TVL input)** | **YES** | E: `Deposit.amount/token`, `OpenNoteDeposited.amount/token` (u128 plaintext) | Fully public; enables gross deposit volume per token. |
| **Unshields (withdrawals) count** | **YES** | E: `Withdrawal` | `to_addr`, `token`, `amount` public; the pool-internal source is auditor-encrypted only. |
| **Unshield amounts + tokens (TVL output)** | **YES** | E: `Withdrawal.amount/token` | Public. Deposit-minus-withdrawal per token ≈ pool TVL (see §4). |
| **Note creations count** | **YES** | E: `EncNoteCreated` + `OpenNoteCreated` | Count of commitments is public; amounts/parties/token(for enc) are NOT. Anonymity-set size = cumulative note count. |
| **Nullifier uses (spends) count** | **YES** | E: `NoteUsed` (nullifier key) or S: `nullifiers` map writes | Count public; which note/owner/token spent is NOT linkable. |
| **Open-note deposits (external funding) count + amount** | **YES** | E: `OpenNoteDeposited` | Depositor (external contract), token, amount public; the recipient note owner is not. |
| **External-contract interactions** | **YES (deployed only)** | E: `ExternalContractInvoked` (target addr + selector) | Reveals which DeFi contract the pool touched and invoke-vs-compute; calldata hidden. NOT in rc0. |
| **Channel creations count** | **NO (not reliably)** | S: `channel_exists`/`outgoing_channels` writes | No event is emitted for OpenChannel (see below). A diff shows *some* `channel_exists`/`outgoing_channels` slots turned nonzero, but only if the indexer already knows the var-name namespace and can attribute a slot to that var — the key is a secret commitment, so **you cannot bind a channel-creation to any parties, and counting is fragile** (writes are indistinguishable from other opaque WriteOnce slots without slot-namespace reverse lookup). |
| **Subchannel creations count** | **NO (not reliably)** | S: `subchannel_exists`/`subchannel_tokens` writes | Same as channels: no event, opaque commitment keys. |
| **Token association of a private note/channel** | **NO** | — | Enc-note `token` field is zeroed in storage & absent from `EncNoteCreated`; subchannel `enc_token` is hash-masked. Token leaks ONLY on the transparent legs (Deposit/Withdrawal/OpenNoteDeposited/OpenNoteCreated key). |
| **Anonymity-set size per token** | **PARTIAL** | E: OpenNoteCreated(token key) + OpenNoteDeposited(token key) | Open notes reveal their token, so you can size the *open-note* set per token. **Encrypted notes hide their token**, so the enc-note anonymity set is a single global bucket across all tokens — you cannot split it per token. So "per-token anonymity set" is only computable for the transparent subset. |
| **Per-user activity / linkage of shields↔notes↔unshields** | **NO** | — | This is the whole point of the pool. Deposit→note→spend→withdraw are unlinkable across legs to a public observer (secret-keyed commitments; auditor-only address ciphertext). Only the auditor (via `EncUserAddr`/`EncPrivateKey`) or the note owner (via viewing keys) can link. |

Note on OpenChannel/OpenSubchannel: `open_channel` and `open_subchannel` (`privacy.cairo:355,428`)
return **only WriteOnce/Append server actions — no `Emit*` action**. Confirmed: `ClientAction`
handlers for channel/subchannel produce no event variant; `ServerAction` has no
`EmitChannelOpened`. So channel/subchannel creation is the one class of private action that is
*event-silent* and only visible as opaque storage writes — the README's intuition ("notes leave no
events") is actually true for **channels**, not notes.

## 4. Q18 — HONEST vs MISLEADING EXPLORER METRICS

### Honest & useful (publicly, verifiably correct)
- **Gross deposits per token** (sum `Deposit.amount` + `OpenNoteDeposited.amount` by `token` key).
  Honest: amounts are plaintext u128 in events.
- **Gross withdrawals per token** (sum `Withdrawal.amount` by `token`). Honest.
- **Net pool balance / TVL per token** = deposits − withdrawals, or better, read the pool's ERC20
  `balanceOf` directly (most honest, avoids event-replay drift). **Shield/unshield amounts+tokens ARE
  public**, so TVL is fully derivable. VERIFIED: event fields are plaintext `u128`.
- **Cumulative note-creation count** (EncNoteCreated + OpenNoteCreated) = a proxy for the
  **anonymity set size**. Honest and genuinely useful — bigger set = better privacy.
- **Cumulative nullifier/spend count** (NoteUsed). Honest.
- **External-contract interaction count and target breakdown** (ExternalContractInvoked, by
  `contract_address`/`selector` keys). Honest; useful for "what DeFi does the pool touch."
- **Governance/lifecycle**: current impl class hash + upgrade history (Replaceability events), pause
  state, fee params, auditor/screener keys, role grants. Honest, all plaintext/config.
- **Unique registered users** (count of ViewingKeySet / `public_key` entries). Honest but see below.

### Misleading or privacy-harmful (do NOT surface, or surface with heavy caveats)
- **"Active users" / per-address dashboards keyed off Deposit.user_addr or Withdrawal.to_addr.**
  Harmful **correlation aid**: these two addresses live on *opposite, unlinkable legs*. Presenting
  them together (or a leaderboard) invites analysts to guess deposit↔withdraw links by
  amount/timing, which is exactly the deanonymization the pool defends against. Amount-matching
  (a deposit of X closely followed by a withdrawal of X) is the classic attack; an explorer that
  lists timestamped amounts side by side hands it to attackers.
- **Timing/latency correlation**: any metric that plots note-creation, NoteUsed, and Withdrawal on a
  shared fine-grained timeline for the same tx/block is a timing-correlation aid. NoteUsed +
  Withdrawal in the same tx already narrows linkage; an explorer should avoid emphasizing
  per-tx event co-occurrence.
- **"Withdrawal recipients" ranking** (to_addr): technically public, but a chart ranking recipient
  addresses by amount nudges toward clustering. Publish aggregate only.
- **Per-token anonymity-set claims that include encrypted notes**: MISLEADING — enc notes hide their
  token, so you cannot honestly attribute them to a token bucket. An explorer that splits the *full*
  note set per token is fabricating precision. Only the open-note subset can be split per token.
- **Nullifier "linkage" or "note lifespan" views**: any UI implying it can connect a NoteUsed
  nullifier to a specific EncNoteCreated note_id is false — the mapping is cryptographically hidden.
  Presenting them as a linked ledger is misleading.
- **"Channel/subchannel created" counts derived from raw storage diffs**: unreliable (§3) and, if
  attributed to addresses, privacy-harmful. Best omitted.

### Indexer-health metrics as safe defaults (no privacy exposure)
These are the metrics an open indexer *should* foreground because they carry zero deanonymization
risk:
- Last indexed block / head lag (blocks behind chain tip).
- Sync status, reorg/rollback counter, last-reorg depth.
- Current pool class hash + whether it matches the indexer's expected/whitelisted class (this
  directly surfaces the upgrade we detected — deployed class ≠ rc0 README class).
- Event-decode error rate / unknown-selector count (catches ABI drift after an upgrade; note the
  live `EncNoteCreated`/`ExternalContractInvoked` handling — rc0's events.rs lacks the latter).
- RPC call volume / latency / rate-limit backoff counters.
- Total events processed by type (the honest aggregate counters above), storage slots scanned.
- Coverage: block range indexed vs contract deploy block.

## 5. Cross-check: what discovery-core's events.rs consumes vs what the pool emits

`crates/discovery-core/src/privacy_pool/events.rs` decodes only **5** of the deployed events and
subscribes to only **4**:
- Typed variants (`PrivacyPoolEventContent`, events.rs:87-93): Deposit, Withdrawal, EncNoteCreated,
  OpenNoteDeposited, ViewingKeySet.
- `get_block_events` selector filter (events.rs:227-232) requests only
  `[Deposit, Withdrawal, EncNoteCreated, OpenNoteDeposited]` — **ViewingKeySet is decoded but not in
  the block-scan filter** (it is fetched on demand via `get_viewing_key_set_events`, events.rs:253).
- **NOT consumed at all: `OpenNoteCreated`, `NoteUsed`, `ExternalContractInvoked`**, and all
  config/governance/component events. This is expected for a *note-discovery* service (it walks
  storage for the private data), but it means:
  - The reference service does **not** use `NoteUsed`/nullifier events — it derives spend state by
    reading the `nullifiers` storage map during the storage walk. An independent indexer wanting a
    cheap public spend feed can use `NoteUsed` events instead.
  - `ExternalContractInvoked` (deployed) is invisible to discovery-core — if the hackathon indexer
    wants DeFi-interaction analytics it must add this selector; note rc0's `events.cairo` doesn't even
    define it, so an indexer built against the rc0 clone would miss it. Build against the deployed
    tag `CONTRACT_V2_DEPLOYED_MAINNET_2026-07-08` (or newer) for the correct event surface.
  - Field decoding is verified consistent with the Cairo layout: Withdrawal amount is read at
    `data[3]` after the 3-felt `enc_user_addr` (events.rs:168, test at :404) — matches
    `Withdrawal { enc_user_addr(3), to_addr#key, token#key, amount }`. Deposit amount at `data[0]`.
    EncNoteCreated `packed_value` at `data[0]`, note_id at `keys[1]`. All correct against events.cairo.
  - `storage_slots.rs` covers exactly the leaky+opaque vars (public_key, enc_private_key,
    channel_exists, recipient_channels, subchannel_exists, subchannel_tokens, outgoing_channels,
    notes, nullifiers, auditor_public_key) and its slot derivation matches
    `get_storage_var_address`. It does **not** derive slots for `blocked_open_note_depositors` or the
    fee/config singletons (not needed for note discovery).

## 6. Bottom line

- README's "notes leave no events" is **FALSE** — note creations (EncNoteCreated/OpenNoteCreated),
  note spends (NoteUsed, nullifier exposed as key), and open-note funding (OpenNoteDeposited) all emit
  commitment events; only **channel/subchannel** creation is genuinely event-silent.
- A public observer can honestly compute: deposit/withdrawal volumes and TVL per token, anonymity-set
  size (note count), spend count, and external-contract interactions. They CANNOT link legs, recover
  amounts/parties/token of encrypted notes, or attribute channels/nullifiers — because every private
  slot is addressed by a hash of a secret (channel_key folds in sender_private_key; nullifier also
  folds in owner_private_key).
- Explorer honesty line: aggregate transparent-leg metrics = safe; anything joining deposit and
  withdrawal addresses, per-tx event timelines, per-token enc-note anonymity claims, or
  nullifier↔note linkage = misleading/harmful. Indexer-health metrics (head lag, class-hash match,
  decode-error rate) are the safe default dashboard.
- Deployed class ≠ rc0: build the indexer against `CONTRACT_V2_DEPLOYED_MAINNET_2026-07-08` event set
  (adds `ExternalContractInvoked`, `CommonRoles`), and watch Replaceability events for future upgrades.
