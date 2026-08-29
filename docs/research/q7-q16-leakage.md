# Q7 (keyless targeted-slot leakage) & Q16 (privacy-safe push design)

Key: `q7-q16-leakage`. All file refs into
`scratchpad/starknet-privacy-rc0/` (contract tag PRIVACY-0.14.3-RC.0, matches the
originally-deployed pool) and `scratchpad/starknet-privacy/` (upstream main specs).

---

## 0. The single most important structural fact

**Storage slots split cleanly into two classes: PUBLIC-address-keyed and SECRET-derived.**
This split is what decides whether a targeted-slot lookup de-anonymizes the user.

Source: `crates/discovery-core/src/privacy_pool/storage_slots.rs` +
`hashes.rs`. Slot derivation is **VERIFIED** against Cairo reference vectors
(`storage_slots.rs:171` `test_storage_slots_with_cairo_vectors`, `hashes.rs:237+`).

### Class A — keyed by the user's PUBLIC on-chain address (de-anonymizing)

| Slot fn | Cairo var | Key input | storage_slots.rs |
|---|---|---|---|
| `public_key(user_address)` | `public_key: LegacyMap<ContractAddress, PublicKey>` | **public addr** | :85 |
| `enc_private_key(user_address)` (3 slots) | `enc_private_key: LegacyMap<ContractAddress,...>` | **public addr** | :92 |
| `recipient_channels_base(recipient_address)` | `recipient_channels: LegacyMap<ContractAddress, Vec<...>>` — **base slot = channel count** | **public addr** | :110 |
| `recipient_channels_element(recipient_address, index)` (3 slots each) | element of that Vec (`pedersen_hash(base, index)`) | **public addr** | :116 |

`get_storage_var_address("recipient_channels", [recipient_addr])` is a *deterministic
function of the public address*. Anyone who sees the queried slot can invert it for any
candidate address (address space is enumerable for known/active users, and the value is
directly recomputable) → **the slot address itself is a pseudonym-free label for the user.**

### Class B — keyed by SECRET-derived markers/ids (do not name the user)

| Slot fn | Key input (all funnel through a secret) | storage_slots.rs / hashes.rs |
|---|---|---|
| `channel_exists(channel_marker)` | marker = H(TAG, **channel_key**, sender, recipient, recip_pk) | slots:103 / hashes:162 |
| `subchannel_exists(subchannel_marker)` | H(TAG, **channel_key**, recipient, recip_pk, token) | slots:128 / hashes:178 |
| `subchannel_tokens(subchannel_id)` (2) | subchannel_id = H(TAG, **channel_key**, index, 0) | slots:135 / hashes:80 |
| `outgoing_channels(outgoing_channel_id)` (2) | H(TAG, sender, **private_key**, index, 0) | slots:146 / hashes:194 |
| `notes(note_id)` | note_id = H(TAG, **channel_key**, token, index, 0) | slots:156 / hashes:101 |
| `nullifiers(nullifier)` | H(TAG, **channel_key**, token, index, 0, **private_key**) | slots:162 / hashes:129 |

`channel_key = H(CHANNEL_KEY_TAG, sender, private_key, recipient, recip_pk)`
(`hashes.rs:146`). Every Class-B slot is a Poseidon image that includes a secret
(channel_key or private_key), so the raw slot address does **not** reveal the participant
addresses — *unless the observer independently knows the pre-image* (e.g. is a
counterparty, or has separately mapped note_id→owner via the public `EncNoteCreated`
event, whose key is `note_id` itself — `privacy_pool/events.rs:56`).

### Consequence for "keyless targeted lookup"

The IDEA-23 pitch is: *"A public, self-hostable indexer wallets can query without handing
over a viewing key"* (`strk20-hackathon/IDEAS.md:121`). The privacy that buys you depends
entirely on which slot class the query touches:

- **Incoming discovery MUST read Class-A slots.** To learn how many incoming channels a
  user has and enumerate them, the flow reads
  `recipient_channels_base(recipient_addr)` then `..._element(recipient_addr, i)`
  — `discovery/incoming_channels.rs:80` (`get_num_of_channels`) → `views.rs:129-136`,
  and `views.rs:194-217` (`get_channel_info_batch`). **These are keyed by the public
  recipient address.** So a keyless client that discovers incoming channels reveals *which
  user it is* to whoever answers the storage read, on essentially every sync (the count
  read repeats each session to catch new channels;
  `discover_incoming_channels_paginated` only short-circuits when the *cached* cursor says
  `channel_discovery_complete`, `incoming_channels.rs:181`). **VERIFIED.**
  → **Keyless does NOT hide the public address for the incoming path. It only withholds
    the viewing key (so the server can't decrypt amounts) and the note plaintexts.**

- **Nullifier / note / subchannel probes read Class-B slots.** Once the client already
  holds the `channel_key` (cached in the cursor, `discovery/cursor.rs:94`), re-sync of
  notes/nullifiers touches only secret-derived slots (`notes.rs:224-262`,
  `views.rs:240-264`). The slot *address* does not name the user. But see leakage below.

---

## 1. What a keyless targeted probe actually reveals (per query type)

- **Nullifier-slot probe** (`nullifiers(nullifier)`, `views.rs:240`): asks
  "is this exact note spent?". The slot = H(channel_key, token, index, 0, private_key).
  It reveals to the responder *that some client is testing a specific note-position for
  spentness*. The responder cannot invert it to an address, BUT: (a) the set of
  nullifier slots you query is a stable fingerprint of your account (same channel_key ⇒
  same note_id/nullifier sequence every sync) → **repeated syncs are linkable to one
  pseudonymous identity even without the address**; (b) a counterparty who sent you the
  note knows your channel_key and can recompute exactly these nullifiers → your sync
  traffic is linkable *by them* to you specifically.

- **Note-slot probe** (`notes(note_id)`): returns the packed (salt‖enc_amount). Client
  decrypts locally (`notes.rs:287` `decrypt_note` / `decryption.rs`), so **the amount is
  NOT visible to the responder** in keyless mode. The responder learns which note_ids you
  fetched and, via `last_update_block`, the creation block.

- **Boundary-finding probes** (`discovery/last_note_index.rs`): the client fires a fixed
  offset ladder `[0,1,3,7,...,2^k-1]` (`last_note_index.rs:154`) then bisects. This is a
  **very recognizable access pattern** — 31 note-existence reads in geometric spacing per
  subchannel. Even encrypted-at-rest, the *shape and count* of the request leaks
  "this is a strk20 discovery sync" and roughly how many subchannels/notes are being
  scanned (→ note-count / activity-level inference via request size).

- **Channel-count / channel-element reads** (Class A): reveal the public address (above)
  AND the channel count (a metadata signal: how many distinct senders have opened a
  channel to you).

- **Cursor reuse**: the cursor carries `channel_key` per channel and `total_n_notes`,
  `last_note_index` per subchannel (`cursor.rs`). In *compatible mode* the cursor is sent
  to the operator each page → the operator directly reads your channel_keys and note
  counts. In a *keyless* design the cursor stays client-side, but if the same cursor-derived
  slot set is requested across sessions it is a stable linkable fingerprint.

---

## 2. Comparison vs direct RPC `getStorageAt` today (VERIFIED against code + chain)

A wallet can already call `starknet_getStorageAt(pool, slot, block)` on any public RPC
(e.g. `https://rpc.starknet.lava.build`, specVersion 0.8.1). The reference discovery
service's storage layer is literally that call under the hood
(`api-design 06`, note: *"The current implementation uses direct RPC calls
(getStorageAt) for all storage access"*).

⇒ **Keyless-targeted against an indexer has the SAME on-the-wire leakage as direct-RPC
getStorageAt**: both expose the exact slot list + IP + timing to whoever answers. The
indexer does not *worsen* privacy vs direct RPC, but it does not *improve* it either —
unless it adds OHTTP/relay, epoch-bulk, or PIR. Crucially the **Class-A incoming-channel
read leaks the public address in BOTH** direct-RPC and keyless-indexer modes; there is no
targeting trick that avoids it, because the slot is a pure function of the address.

The reference *compatible* service is strictly worse than direct RPC on one axis: the
client hands the **viewing key** to the operator, who then decrypts amounts server-side
(`api-design 06.5`, `05-security-considerations §5.5`). Direct-RPC / keyless never expose
the key.

---

## 3. Q7 LEAKAGE TABLE — visible-to-whom per mode

Modes:
- **Compatible** = reference `discovery-service` today: client POSTs `viewing_key` +
  `recipient_address` + cursor; server derives slots, reads storage, **decrypts**, returns
  plaintext notes (`specs/06-api-design.md §6.5`).
- **Keyless targeted** = client derives slots locally with its own secret, asks indexer/RPC
  for a specific list of storage keys; client decrypts. (== direct `getStorageAt`.)
- **Keyless bulk / epoch** = client pulls *all* pool storage-diffs / events for a block
  range (or a per-epoch filter) and matches locally; no per-slot targeting.
- **PIR** = client retrieves specific slots without the server learning which (private
  information retrieval / homomorphic).

"Operator" = whoever answers the storage query (indexer op or RPC provider). "Relay" row
folds in OHTTP+privacy-relay, which only moves the IP column (see §4).

| Data item | Compatible | Keyless targeted (= direct RPC) | Keyless bulk / epoch | PIR |
|---|---|---|---|---|
| **Viewing key** | **Operator** (sent in body) | nobody (never leaves client) | nobody | nobody |
| **Decrypted amounts / balances** | **Operator** (server decrypts) | nobody (client decrypts) | nobody | nobody |
| **Exact queried slots** | Operator (derives them) | **Operator** (client sends slot list) | nobody (bulk, not targeted) | **nobody** (hidden by PIR) |
| **User PUBLIC address** | **Operator** (in body) | **Operator** for incoming path (Class-A slot = address); hidden for note/nullifier-only resync | nobody (generic feed) | Operator only if it must fetch Class-A by address; hidden if bulk-fed |
| **Client IP** | Operator (unless OHTTP+relay → relay only) | Operator (unless OHTTP+relay) | Operator/CDN (unless relay) | Operator (unless relay) |
| **Sync timing / liveness** | Operator | Operator | Operator (coarser: only "pulled epoch N") | Operator |
| **Note / channel count** (via request size or count slot) | **Operator** (reads count slot + returns notes) | **Operator** (# slots in request ≈ note count; Class-A count slot) | nobody (fixed-size epoch pull) | mostly hidden (uniform PIR volume) |
| **Token interests** | **Operator** (decrypts subchannel tokens) | hidden (subchannel/token slots are secret-hashes) but *which* subchannel slots queried is a fingerprint | nobody | nobody |

Notes on cells:
- Keyless-targeted "User public address" is the crux: **incoming discovery cannot avoid
  the Class-A slot**, so keyless does NOT anonymize an incoming sync. It only stops key
  and amount leakage. A wallet that has already cached its channel_keys and only re-checks
  nullifiers/notes can do a *fully secret-slot* resync that hides the address — but the
  first/enumerating sync always burns it.
- Even where the address is hidden, **stable slot fingerprints make repeated keyless syncs
  linkable to one pseudonymous identity**, and any counterparty can de-pseudonymize them
  by recomputing the same note_ids/nullifiers.

---

## 4. What OHTTP does and does NOT hide (from the reference threat model)

Source: `specs/20-ohttp-integration.md`, `specs/05-security-considerations.md`.

The reference service **already documents these leaks** — it does not claim keyless/PIR;
it is a *compatible* (key-in-body) service and is explicit about the trust:

- `05 §5.5 Privacy Model`: *"Users trust the service operator with request content. The
  operator can observe: which recipients are active and when; how many channels,
  subchannels, and notes each recipient has; token addresses used per channel; timing of
  sync activity."* → i.e. **everything in the Compatible column above is acknowledged.**
- OHTTP + privacy relay (`20 §20.9`, `05 §5.5`) **only removes the client-IP↔content
  correlation**: relay sees IP but not content; operator sees content but not IP. It does
  **not** hide viewing key, amounts, channel counts, or token addresses from the operator
  (`20 §20.9`: OHTTP "does not protect against … content-level metadata the operator
  already observes (viewing keys, channel counts, token addresses)").
- OHTTP explicitly does **not** stop traffic analysis (request sizes/timing) under
  relay-operator collusion, and the without-relay mode still leaks IP to the operator.
- Timing/side-channels are declared **out of scope** (`05 §5.2`): no constant-time
  padding, cache hit/miss timing not obfuscated.
- Body-size limit (default 100KB) is a DoS control, not privacy (`05 §5.3.1`).

**Gap the reference model does not close, and IDEA-23 is meant to:** the operator still
learns the viewing key and all plaintext. A keyless indexer removes the key/amount
columns but, per §0, **cannot remove the public-address column for incoming discovery**
without moving to bulk/epoch or PIR.

---

## 5. Q16 — privacy-safe WebSocket / push designs, ranked by leakage

Three candidate push models, best-privacy first:

### A. Global diff / event stream (RECOMMENDED default)
Server streams *every* pool storage-diff (or every pool event) to every subscriber; client
filters locally with its secret keys. Server learns only "someone subscribed", plus IP +
connection timing.
- **Leakage: minimal.** No address, no slots, no key, no amounts, no per-note interest.
  Not even note-count (everyone gets the same stream). Only liveness + IP (killable with a
  relay/Tor, or by serving the stream from a CDN edge).
- **Linkability across syncs: none** at the server (all subscribers identical).
- **Cost: the only downside**, and at current mainnet activity it is negligible (see §6).

### B. Per-epoch batched stream / filter (fallback for scale)
Chain is chunked into epochs; per epoch the server publishes a compact structure (all
changed note/nullifier slots, or a Bloom/cuckoo filter of them). Client pulls the epochs
covering its gap and matches locally.
- **Leakage: low.** Server learns which *epoch ranges* a client pulled → coarse
  first-activity / account-age and liveness. No per-slot targeting, no address, no key,
  no amounts. Note-count hidden (fixed-size epoch payloads).
- **Linkability: weak** (only via epoch-range + IP).
- Good when a full global stream gets too large; trades a little metadata (epoch cursor)
  for bandwidth.

### C. User-specific slot subscription (AVOID as default)
Client subscribes to a specific slot set (its note/nullifier/channel slots); server pushes
updates when those slots change.
- **Leakage: high — equivalent to keyless-targeted, made continuous.** Server sees the
  exact slot fingerprint (stable across the connection → a durable pseudonymous handle),
  and if the subscription includes Class-A incoming-channel slots, the **public address**.
  Long-lived connection makes timing/liveness correlation trivial.
- Only acceptable behind PIR-style private subscriptions or when the user runs their own
  indexer.

**Ranking: A (global) < B (epoch) ≪ C (targeted sub).** More targeting = more leakage.

### PIR overlay
Orthogonal: any of the above can be hardened with PIR so the server cannot see which
slots/epochs a client cares about. PIR is the only way to get C-style targeting with
A-style privacy, at high compute cost. Out of scope for a hackathon MVP; note it as the
"strong mode" upgrade path.

---

## 6. Bandwidth estimate for a global stream (MEASURED, mainnet, 2026-08-29)

RPC: `https://rpc.starknet.lava.build`, current block 14055281.
- **Block time:** blocks 14025000→14055000 span ts 1787971408→1788021532 ⇒
  **1.67 s/block ≈ 51,700 blocks/day. VERIFIED.**
- **Pool activity** (address `0x0403...812a`, `starknet_getEvents`, blocks
  13955000..14055281 ≈ 1.94 days): **1030 events across 247 pool-active blocks**
  (0.25% of blocks are pool-active). ⇒ **≈530 events/day, ≈127 active blocks/day. VERIFIED.**
  Event mix (selectors resolved via starknet_keccak): EncNoteCreated 166,
  Withdrawal 223, Deposit 81, NoteUsed 126, ExternalContractInvoked 73, ViewingKeySet 32,
  OpenNoteCreated 26, OpenNoteDeposited 26. avg 4.4 felts/event.
- **Storage-diff volume:** sampled active block 14042629 wrote **4 pool storage entries**
  (`starknet_getStateUpdate`). ⇒ ≈510 pool storage entries/day.

**Global storage-diff stream:** ~510 entries/day × (key+value ≈ 64 B, ~100 B framed) ≈
**~32–50 KB/day ≈ 0.9–1.5 MB/month.**
**Global event stream:** ~530 events/day × ~150–250 B ≈ **~78–130 KB/day.**

**Assumption (stated):** current mainnet pool activity is sparse (~0.25% of blocks touch
the pool). Under that load a global stream costs a client **well under 1 MB/day** — a
mobile wallet can hold it open trivially, and even a cold-start full replay of all pool
storage diffs is a few MB. The global stream only becomes a concern if pool activity grows
~100–1000×, at which point switch the default to epoch batches (model B). **This makes the
global stream the privacy-optimal AND bandwidth-feasible default today.**

---

## 7. Recommended default mode + threat-model statements

**Recommended default for the open indexer:** serve a **keyless global (or per-epoch)
diff/event stream that clients filter locally**, with the viewing key NEVER sent to the
server. Offer OHTTP+relay to strip IP. Treat compatible (key-in-body) mode as an explicit,
labeled opt-in convenience, not the default. Provide keyless-targeted `getStorageAt`
passthrough for wallets that want it, with a clear warning that it leaks the slot
fingerprint (and, for incoming discovery, the public address).

**Explicit statements for the threat-model doc:**

1. Storage slots divide into public-address-keyed (Class A: `public_key`,
   `enc_private_key`, `recipient_channels[*]`) and secret-derived (Class B: channel/
   subchannel markers, note_ids, nullifiers). **VERIFIED** against Cairo vectors.
2. **Incoming-channel discovery is inherently de-anonymizing at the storage layer:** the
   channel-count and channel-element slots are pure functions of the recipient's public
   address. No keyless/targeting trick hides the address for an enumerating incoming sync.
   Only bulk/epoch streaming or PIR removes this leak.
3. **Keyless mode removes exactly two things vs the reference service: the viewing key and
   server-side amount decryption.** It does NOT remove address leakage (incoming path),
   slot-fingerprint linkability, note-count-via-request-size, or timing.
4. **Keyless-targeted lookup == direct `getStorageAt`** in leakage; the indexer neither
   helps nor hurts on-wire privacy unless it adds a relay, bulk streaming, or PIR.
5. **Nullifier probes reveal which note-position is being tested for spentness**; the slot
   is not invertible to an address by a stranger, but a counterparty who knows the
   channel_key can recompute and de-pseudonymize the client's sync traffic.
6. **Repeated syncs are linkable** through the stable set of slots a given account queries,
   even without the address and even under OHTTP (traffic-analysis / same-fingerprint).
7. **OHTTP only decouples IP from content** and, only with a non-colluding relay; it hides
   nothing the operator already computes (keys, counts, tokens) — this matches the
   reference `05 §5.5` / `20 §20.9`.
8. **Push design ordering by leakage: global stream < per-epoch stream ≪ per-user slot
   subscription.** Default to global/epoch. A per-user slot subscription is a continuous
   keyless-targeted channel and should be gated behind PIR or self-hosting.
9. At current mainnet activity a global stream is < 1 MB/day, so the privacy-optimal choice
   is also the cheap choice today; revisit only if pool volume grows ~100×.
