//! Conformance vs upstream (spec §10.2):
//! 1. trait-bridge proof: the unmodified engine over our SQLite `DbBackend`
//!    produces results identical to the engine over upstream's `MockBackend`
//!    for the same slots — full struct equality via canonical JSON;
//! 2. Cairo reference vectors: the crypto/slot functions we ship compute the
//!    exact values blessed by the Cairo contract's reference fixture;
//! 3. cursor JSON round-trip through the reference serde schema.

use discovery_core::privacy_pool::types::SecretFelt;
use discovery_core::privacy_pool::{hashes, storage_slots};
use discovery_core::storage_backend::{MockBackend, StorageBackend};
use e2e_tests::fixture::load_devnet_fixture;
use e2e_tests::oracle;
use starknet_types_core::felt::Felt;
use strk20_indexerd::bridge::DbBackend;
use strk20_indexerd::db::{BlockRow, Db, EventRow};

/// Engine over DbBackend == engine over MockBackend, same 48 fixture slots.
#[tokio::test(flavor = "multi_thread")]
async fn engine_over_sqlite_equals_engine_over_mock() {
    let f = load_devnet_fixture();
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("conformance.db");
    {
        let mut db = Db::open(&db_path).unwrap();
        let block = BlockRow {
            number: 46,
            hash: Felt::from(0xb10cu64),
            parent_hash: Felt::from(0xb0ffu64),
            timestamp: 1046,
            l1_accepted: true,
        };
        let mut diffs: Vec<(Felt, Felt)> = f.slots.iter().map(|(k, v)| (*k, *v)).collect();
        diffs.sort_by_key(|a| a.0.to_bytes_be());
        let events: Vec<EventRow> = Vec::new();
        db.insert_block_data(&block, &diffs, &events, None, 46, None)
            .unwrap();
        db.meta_set("head_number", "46").unwrap();
        db.meta_set("head_hash", &strk20_feed::felt_hex(&block.hash))
            .unwrap();
    }
    let backend = DbBackend::new(db_path, f.constants.contract_address);
    let snapshot = backend
        .snapshot(f.constants.contract_address, None)
        .await
        .unwrap();

    // MockBackend with the same write block for every slot
    let mut mock = MockBackend::empty();
    for (k, v) in &f.slots {
        mock.insert_with_block(*k, *v, 46);
    }

    for (owner, key) in [
        (f.constants.alice_address, f.constants.alice_viewing_key),
        (f.constants.bob_address, f.constants.bob_viewing_key),
    ] {
        let ours = oracle::incoming(&snapshot, owner, &SecretFelt::new(key)).await;
        let reference = oracle::incoming(&mock, owner, &SecretFelt::new(key)).await;
        assert_eq!(
            oracle::notes_canonical(&ours.notes),
            oracle::notes_canonical(&reference.notes),
            "notes must be identical for {}",
            strk20_feed::felt_hex(&owner)
        );
        assert_eq!(
            ours.cursor.channels.len(),
            reference.cursor.channels.len(),
            "channel sets must match"
        );
        // preflight over both backends
        let p_ours = discovery_core::sync::preflight_check::preflight_check(
            &snapshot,
            f.constants.alice_address,
            &SecretFelt::new(f.constants.alice_viewing_key),
            owner,
            f.constants.strk_token,
        )
        .await
        .unwrap();
        let p_ref = discovery_core::sync::preflight_check::preflight_check(
            &mock,
            f.constants.alice_address,
            &SecretFelt::new(f.constants.alice_viewing_key),
            owner,
            f.constants.strk_token,
        )
        .await
        .unwrap();
        assert_eq!(p_ours.sender_registered, p_ref.sender_registered);
        assert_eq!(p_ours.channel_exists, p_ref.channel_exists);
        assert_eq!(p_ours.subchannel_exists, p_ref.subchannel_exists);
    }
}

/// The shipped crate computes the Cairo-blessed reference values — proof the
/// pinned engine version matches the deployed contract's crypto.
#[test]
fn cairo_reference_vectors() {
    let raw: serde_json::Value = serde_json::from_str(include_str!(
        "../../../fixtures/upstream/cairo-reference-data.json"
    ))
    .unwrap();
    let felt = |v: &serde_json::Value| Felt::from_hex(v.as_str().unwrap()).unwrap();
    let inp = &raw["inputs"];
    let out = &raw["outputs"];
    let slots = &raw["slots"];

    let sender = felt(&inp["sender"]);
    let recipient = felt(&inp["recipient"]);
    let sender_priv = SecretFelt::new(felt(&inp["senderPrivateKey"]));
    let recipient_pk = felt(&inp["recipientPublicKey"]);
    let channel_key = SecretFelt::new(felt(&inp["channelKey"]));
    let token = felt(&inp["token"]);
    let index = inp["index"].as_u64().unwrap();

    assert_eq!(
        *hashes::compute_channel_key(sender, &sender_priv, recipient, recipient_pk),
        felt(&out["channelKey"]),
        "channelKey"
    );
    assert_eq!(
        hashes::compute_channel_marker(&channel_key, sender, recipient, recipient_pk),
        felt(&out["channelMarker"]),
        "channelMarker"
    );
    assert_eq!(
        hashes::compute_subchannel_id(&channel_key, index),
        felt(&out["subchannelId"]),
        "subchannelId"
    );
    assert_eq!(
        hashes::compute_note_id(&channel_key, token, index),
        felt(&out["noteId"]),
        "noteId"
    );
    assert_eq!(
        hashes::compute_nullifier(&channel_key, token, index, &sender_priv),
        felt(&out["nullifier"]),
        "nullifier"
    );

    // slot addresses (the exact storage addressing the whole system rests on)
    assert_eq!(
        storage_slots::auditor_public_key(),
        felt(&slots["auditorPublicKeyAddress"]),
        "auditor slot"
    );
    assert_eq!(
        storage_slots::public_key(sender),
        felt(&slots["senderPublicKeyAddress"]),
        "sender pk slot"
    );
    assert_eq!(
        storage_slots::public_key(recipient),
        felt(&slots["recipientPublicKeyAddress"]),
        "recipient pk slot"
    );
    assert_eq!(
        storage_slots::notes(felt(&inp["noteId"])),
        felt(&slots["notesAddress"]),
        "notes slot"
    );
    assert_eq!(
        storage_slots::nullifiers(felt(&inp["nullifier"])),
        felt(&slots["nullifiersAddress"]),
        "nullifiers slot"
    );
    assert_eq!(
        storage_slots::recipient_channels_base(recipient),
        felt(&slots["recipientChannelsBaseAddress"]),
        "recipient channels base slot"
    );
    assert_eq!(
        storage_slots::channel_exists(felt(&inp["channelMarker"])),
        felt(&slots["channelExistsAddress"]),
        "channel exists slot"
    );
    assert_eq!(
        storage_slots::subchannel_exists(felt(&inp["subchannelMarker"])),
        felt(&slots["subchannelExistsAddress"]),
        "subchannel exists slot"
    );
}

/// The client's persisted cursor uses the exact reference serde schema.
#[test]
fn cursor_reference_schema_round_trip() {
    use discovery_core::discovery::{ChannelCursor, DiscoveryCursor, SubchannelCursor};
    let mut cursor = DiscoveryCursor {
        channel_discovery_complete: true,
        total_n_channels: Some(2),
        last_channel_index: Some(1),
        channels: Default::default(),
    };
    let mut ch = ChannelCursor {
        channel_key: SecretFelt::new(Felt::from(0xdeadu64)),
        subchannel_discovery_complete: true,
        last_subchannel_index: Some(0),
        subchannels: Default::default(),
    };
    ch.subchannels.insert(
        Felt::from(0x4718u64),
        SubchannelCursor {
            note_discovery_complete: true,
            last_note_index: Some(3),
            total_n_notes: Some(4),
        },
    );
    cursor.channels.insert(Felt::from(0x1234u64), ch);
    let json = serde_json::to_string(&cursor).unwrap();
    let back: DiscoveryCursor = serde_json::from_str(&json).unwrap();
    assert_eq!(serde_json::to_string(&back).unwrap(), json);
    assert!(back.is_complete());
}
