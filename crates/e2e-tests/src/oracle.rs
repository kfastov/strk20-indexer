//! Independent oracle O1 (spec §10.3): the unmodified discovery-core engine
//! over upstream's own MockBackend, loaded with the same slots + write
//! blocks. Also the note-minting helpers for the reorg/spent legs (valid
//! ciphertexts constructed with the engine's own crypto).

use discovery_core::discovery::notes::DecryptedNote;
use discovery_core::discovery::{CursorLimits, DiscoveryCursor};
use discovery_core::io_budget::IoBudget;
use discovery_core::privacy_pool::hashes::{
    compute_enc_amount_hash, compute_note_id, compute_nullifier,
};
use discovery_core::privacy_pool::types::SecretFelt;
use discovery_core::storage_backend::MockBackend;
use discovery_core::sync::incoming_state::sync_incoming_state;
use starknet_types_core::felt::Felt;

pub struct OracleResult {
    pub cursor: DiscoveryCursor,
    pub notes: Vec<DecryptedNote>,
}

/// Build a MockBackend mirroring the fixture chain state (values + write
/// blocks) as of `block`.
pub fn backend_at(chain: &crate::chain::FixtureChain, block: u64) -> MockBackend {
    let mut backend = MockBackend::empty();
    for (slot, value) in chain.state_at(block) {
        let wb = chain.write_block_of(&slot).unwrap_or(0);
        backend.insert_with_block(slot, value, wb);
    }
    backend
}

/// Run incoming discovery to completion for `owner` over any engine backend.
pub async fn incoming<S: discovery_core::privacy_pool::views::IViews>(
    backend: &S,
    owner: Felt,
    key: &SecretFelt,
) -> OracleResult {
    let mut cursor = DiscoveryCursor::default();
    let mut notes = Vec::new();
    for _ in 0..1000 {
        let budget = IoBudget::new(1_000_000);
        let out = sync_incoming_state(
            backend,
            owner,
            key,
            cursor,
            CursorLimits::default(),
            &budget,
        )
        .await
        .expect("oracle incoming discovery");
        notes.extend(out.notes);
        cursor = out.cursor;
        if cursor.is_complete() {
            return OracleResult { cursor, notes };
        }
    }
    panic!("oracle discovery did not complete");
}

/// The channel key for `owner`'s first incoming channel from `sender`.
pub fn channel_key_of(result: &OracleResult, sender: &Felt) -> SecretFelt {
    result
        .cursor
        .channels
        .get(sender)
        .map(|c| c.channel_key.clone())
        .expect("oracle cursor has the sender channel")
}

pub struct MintedNote {
    pub note_id: Felt,
    pub slot: Felt,
    pub packed_value: Felt,
    pub nullifier: Felt,
    pub amount: u128,
    pub index: u64,
}

/// Mint a valid encrypted note for an existing channel using the engine's
/// own crypto (the inverse of `decrypt_packed_value`).
pub fn mint_note(
    channel_key: &SecretFelt,
    token: Felt,
    index: u64,
    amount: u128,
    owner_key: &SecretFelt,
) -> MintedNote {
    let salt: u128 = 2; // >= 2 = encrypted note
    let mask = compute_enc_amount_hash(channel_key, token, index, salt);
    // low 128 bits of the mask
    let mask_low: u128 = {
        let d = mask.to_le_digits();
        d[0] as u128 | (d[1] as u128) << 64
    };
    let enc_amount = amount.wrapping_add(mask_low);
    // packed = salt * 2^128 + enc_amount
    let mut bytes = [0u8; 32];
    bytes[0..16].copy_from_slice(&salt.to_be_bytes());
    bytes[16..32].copy_from_slice(&enc_amount.to_be_bytes());
    let packed_value = Felt::from_bytes_be(&bytes);
    let note_id = compute_note_id(channel_key, token, index);
    MintedNote {
        note_id,
        slot: discovery_core::privacy_pool::storage_slots::notes(note_id),
        packed_value,
        nullifier: compute_nullifier(channel_key, token, index, owner_key),
        amount,
        index,
    }
}

/// Canonical, comparison-friendly JSON form of an oracle / client note set.
pub fn notes_canonical(notes: &[DecryptedNote]) -> Vec<serde_json::Value> {
    let mut v: Vec<_> = notes
        .iter()
        .map(|n| {
            serde_json::json!({
                "sender": strk20_feed::felt_hex(&n.sender_addr),
                "token": strk20_feed::felt_hex(&n.token),
                "index": n.index,
                "note_id": strk20_feed::felt_hex(&n.note_id),
                "amount": n.amount.to_string(),
                "block_number": n.block_number,
            })
        })
        .collect();
    v.sort_by_key(|j| {
        (
            j["token"].as_str().unwrap_or("").to_owned(),
            j["index"].as_u64().unwrap_or(0),
        )
    });
    v
}
