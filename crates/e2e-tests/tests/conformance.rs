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
        db.insert_block_data(&block, &diffs, &events, None, 46)
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

// ---------------------------------------------------------------------------
// 4. The seam is real: Block B over SQLite == Block B over the in-memory store
// ---------------------------------------------------------------------------
//
// Why this leg exists. The whole design is two blocks with one seam, and Block
// B is supposed to run in two hosts: the native client over SQLite and the
// browser over an in-memory view. `strk20-consumer` is that extraction — but a
// suite that exercises exactly one store CANNOT detect a missing abstraction.
// A `ConsumerStore` that had quietly kept a SQL assumption would keep every
// existing test green forever, and the browser would then need a second
// implementation of the fold, at which point the equality claim the project
// rests on ("the same public bytes give every host the same answer") is gone.
//
// So: one feed, byte-identical, folded twice — once through
// `strk20_client::store::FeedStore` (rusqlite, WAL, blobs) and once through
// `strk20_consumer::mem::MemStore` (BTreeMaps) — by the SAME `sync_once`. Then
// demand equality of everything a user can observe: the notes, the balances,
// the spent-state, the whole report, and the storage root of the folded mirror
// itself.

mod seam {
    use super::*;
    use starknet_types_core::felt::Felt;
    use std::path::{Path, PathBuf};
    use strk20_client::store::FeedStore;
    use strk20_client::transport::DirTransport;
    use strk20_consumer::mem::MemStore;
    use strk20_consumer::store::ConsumerStore;
    use strk20_consumer::sync::{sync_once, SyncOptions, SyncReport};
    use strk20_feed::codec::{
        self, BlockLine, Epoch, EpochHeader, Finality, Footer, Head, HeadHeader,
    };
    use strk20_feed::manifest::{Genesis, Manifest, ManifestEpoch, ManifestHead};

    const CHAIN_ID: &str = "SN_SEPOLIA";
    const EPOCH_SIZE: u64 = 100;
    const FIXTURE_BLOCK: u64 = 46;
    pub(super) const EPOCH_END: u64 = EPOCH_SIZE - 1;
    pub(super) const SPEND_BLOCK: u64 = 100;

    pub(super) fn blk(number: u64, diffs: Vec<(Felt, Felt)>, finality: Option<Finality>) -> BlockLine {
        let mut diffs = diffs;
        diffs.sort_by_key(|(k, _)| k.to_bytes_be());
        BlockLine {
            number,
            hash: Felt::from(0xb10c0000u64 + number),
            parent: Felt::from(0xb10c0000u64 + number - 1),
            timestamp: 1_700_000_000 + number,
            diffs,
            events: Vec::new(),
            replaced_class: None,
            finality,
        }
    }

    pub(super) fn footer_of(blocks: &[BlockLine]) -> Footer {
        Footer {
            blocks: blocks.len() as u64,
            diffs: blocks.iter().map(|b| b.diffs.len() as u64).sum(),
            events: blocks.iter().map(|b| b.events.len() as u64).sum(),
            class: Felt::from(0xc1a55u64),
        }
    }

    /// Write `head.ndjson` for a tail of `blocks` above the epoch floor.
    pub(super) fn write_head(feed: &Path, blocks: Vec<BlockLine>, head: u64, l1_accepted: u64) {
        let footer = footer_of(&blocks);
        let payload = codec::encode_head(&Head {
            header: HeadHeader {
                tail_from: EPOCH_SIZE,
                head,
                head_hash: Felt::from(0xb10c0000u64 + head),
                l1_accepted,
            },
            blocks,
            footer,
        });
        std::fs::write(feed.join("head.ndjson"), payload).unwrap();
    }

    /// A one-epoch feed carrying the devnet fixture's slots at block 46.
    /// Returns the feed directory.
    pub(super) fn build_feed(dir: &Path, pool: Felt, slots: &[(Felt, Felt)]) -> PathBuf {
        let feed = dir.join("feed");
        std::fs::create_dir_all(feed.join("epochs")).unwrap();

        let blocks = vec![blk(FIXTURE_BLOCK, slots.to_vec(), None)];
        let footer = footer_of(&blocks);
        let payload = codec::encode_epoch(&Epoch {
            header: EpochHeader {
                chain_id: CHAIN_ID.to_owned(),
                pool,
                epoch: 0,
                from: 0,
                to: EPOCH_END,
                prev: None,
            },
            blocks,
            footer,
        });
        let zst = strk20_feed::compress(&payload);
        std::fs::write(feed.join("epochs/00000000.strk20e.zst"), &zst).unwrap();

        let genesis = Genesis {
            format: "strk20-feed".to_owned(),
            v: 1,
            chain_id: CHAIN_ID.to_owned(),
            pool: strk20_feed::felt_hex(&pool),
            genesis_block: 0,
            epoch_size: EPOCH_SIZE,
        };
        std::fs::write(
            feed.join("genesis.json"),
            serde_json::to_vec_pretty(&genesis).unwrap(),
        )
        .unwrap();

        let manifest = Manifest {
            v: 1,
            chain_id: CHAIN_ID.to_owned(),
            pool: strk20_feed::felt_hex(&pool),
            genesis_block: 0,
            epoch_size: EPOCH_SIZE,
            head: ManifestHead {
                number: EPOCH_END,
                hash: strk20_feed::felt_hex(&Felt::from(0xb10c0000u64 + EPOCH_END)),
                l1_accepted: EPOCH_END,
                class: strk20_feed::felt_hex(&Felt::from(0xc1a55u64)),
                decode_state: "ok".to_owned(),
            },
            latest_epoch: Some(0),
            epochs: vec![ManifestEpoch {
                e: 0,
                from: 0,
                to: EPOCH_END,
                hash: hex::encode(strk20_feed::payload_sha256(&payload)),
                zst: hex::encode(strk20_feed::payload_sha256(&zst)),
                bytes: zst.len() as u64,
                anchor: None,
            }],
            snapshot: None,
        };
        std::fs::write(
            feed.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();

        write_head(&feed, Vec::new(), EPOCH_END, EPOCH_END);
        feed
    }

    fn canonical(r: &SyncReport) -> serde_json::Value {
        serde_json::to_value(r).unwrap()
    }

    /// The mirror itself, not just the answer: fold both stores' slot sets to a
    /// storage root. Two mirrors that agree here hold the same state, which is
    /// a stronger statement than two reports that happen to match.
    fn mirror_root<S: ConsumerStore>(store: &S, block: u64) -> Felt {
        strk20_feed::mpt::storage_root(&store.full_slot_set_as_of(block).unwrap())
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn block_b_over_sqlite_equals_block_b_over_the_in_memory_store() {
        let f = load_devnet_fixture();
        let dir = tempfile::tempdir().unwrap();
        let slots: Vec<(Felt, Felt)> = f.slots.iter().map(|(k, v)| (*k, *v)).collect();
        let feed = build_feed(dir.path(), f.constants.contract_address, &slots);
        let transport = DirTransport::new(feed.clone());
        let opts = SyncOptions::default();

        let sqlite = FeedStore::open(&dir.path().join("sync.db")).unwrap();
        let mem = MemStore::new();

        // ------------------------------------------------ pass 1: cold fold
        let mut discovered = 0usize;
        let mut owners: Vec<(Felt, SecretFelt, SyncReport)> = Vec::new();
        for (owner, key) in [
            (f.constants.alice_address, f.constants.alice_viewing_key),
            (f.constants.bob_address, f.constants.bob_viewing_key),
        ] {
            let key = SecretFelt::new(key);
            let a = sync_once(&sqlite, &transport, owner, &key, &opts)
                .await
                .expect("sqlite sync");
            let b = sync_once(&mem, &transport, owner, &key, &opts)
                .await
                .expect("in-memory sync");
            assert_eq!(
                canonical(&a),
                canonical(&b),
                "the same feed bytes folded by the same state machine must give the same \
                 report over both stores, for {}",
                strk20_feed::felt_hex(&owner)
            );
            // Registry rows, not only their rendering.
            assert_eq!(
                sqlite.notes(&owner).unwrap(),
                mem.notes(&owner).unwrap(),
                "note registries must be row-for-row identical"
            );
            discovered += a.notes.len();
            owners.push((owner, key, a));
        }
        assert!(
            discovered > 0,
            "this leg is only worth something if discovery actually found notes; it found \
             none, so the equality above is vacuous"
        );
        assert_eq!(
            mirror_root(&sqlite, EPOCH_END),
            mirror_root(&mem, EPOCH_END),
            "the two folded mirrors must reproduce the same pool storage root"
        );

        // ------------------------------- pass 2: a spend arrives in the tail
        //
        // Spent-state is the one part of the report that comes from the
        // nullifier slot rather than from the engine, and the live run pinned
        // its semantics (a spent note's own slot is NOT cleared). Write the
        // nullifier of the first discovered note into a tail block and re-sync
        // both stores: the flip, the newly_spent list and the balance drop must
        // all be identical.
        let (spent_owner, spent_key, first) = owners
            .iter()
            .find(|(_, _, r)| !r.notes.is_empty())
            .expect("a discovered note to spend");
        let nullifier = Felt::from_hex(&first.notes[0].nullifier).unwrap();
        let slot = discovery_core::privacy_pool::storage_slots::nullifiers(nullifier);
        write_head(
            &feed,
            vec![blk(
                SPEND_BLOCK,
                vec![(slot, Felt::ONE)],
                Some(Finality::L2),
            )],
            SPEND_BLOCK,
            EPOCH_END,
        );

        let a = sync_once(&sqlite, &transport, *spent_owner, spent_key, &opts)
            .await
            .expect("sqlite re-sync");
        let b = sync_once(&mem, &transport, *spent_owner, spent_key, &opts)
            .await
            .expect("in-memory re-sync");
        assert_eq!(
            canonical(&a),
            canonical(&b),
            "the incremental tail apply, the spent-state refresh and the balances must \
             agree across the two stores"
        );
        assert!(
            a.notes.iter().any(|n| n.spent),
            "the tail write should have flipped a note to spent; it did not, so the \
             spent-state half of this leg proved nothing"
        );
        assert_eq!(
            a.newly_spent,
            vec![strk20_feed::felt_hex(&nullifier)],
            "exactly the nullifier we wrote must be reported newly spent"
        );
        assert_eq!(
            sqlite.notes(spent_owner).unwrap(),
            mem.notes(spent_owner).unwrap(),
            "spent flags must be identical in both registries"
        );
        assert_eq!(
            mirror_root(&sqlite, SPEND_BLOCK),
            mirror_root(&mem, SPEND_BLOCK),
            "the two mirrors must still agree after the tail apply"
        );
    }
}


// ---------------------------------------------------------------------------
// 5. History-independence: a cold start == a client that watched the spend
// ---------------------------------------------------------------------------
//
// Upstream's note scan is nullifier-first: `process_note_batch`
// (discovery/notes.rs) reads every index's nullifier and `continue`s past the
// spent ones before it ever fetches the note slot. So `sync_incoming_state`
// answers "the notes that are unspent at this bound", not "the notes", and
// registering only what it returns made the report a function of when the
// client started — a client whose registry predates the spend keeps the row
// and reports `spent: true`, a cold start over the identical bytes reported
// nothing at all. Balances agreed; the report shape did not.
//
// The seam leg above cannot catch this. Its spend lands in the head TAIL, and
// `sync_once`'s checkpoint pass is bound to the epoch floor BELOW the tail, so
// even a fresh client sees the note unspent there and registers it. The bug
// needs a spend that is already inside a cut epoch when the client arrives,
// which is what this fixture builds: two epochs, the note minted in the first
// and spent in the second.

mod history_independence {
    use super::*;
    use starknet_types_core::felt::Felt;
    use std::path::Path;
    use strk20_client::transport::DirTransport;
    use strk20_consumer::mem::MemStore;
    use strk20_consumer::store::ConsumerStore;
    use strk20_consumer::sync::{sync_once, SyncOptions, SyncReport};
    use strk20_feed::codec::{self, BlockLine, Epoch, EpochHeader, Head, HeadHeader};
    use strk20_feed::manifest::{Genesis, Manifest, ManifestEpoch, ManifestHead};

    use super::seam::{blk, footer_of};

    const CHAIN_ID: &str = "SN_SEPOLIA";
    const EPOCH_SIZE: u64 = 100;
    /// The fixture's slots, inside epoch 0 = [0, 99].
    const MINT_BLOCK: u64 = 46;
    const E0_TO: u64 = 99;
    /// The spend, inside epoch 1 = [100, 199] — an L1-final epoch, not a tail.
    const SPEND_BLOCK: u64 = 100;
    const E1_TO: u64 = 199;

    /// Write one epoch file; return its payload hash and its manifest entry.
    fn cut_epoch(
        feed: &Path,
        pool: Felt,
        e: u64,
        from: u64,
        to: u64,
        blocks: Vec<BlockLine>,
        prev: Option<[u8; 32]>,
    ) -> ([u8; 32], ManifestEpoch) {
        let footer = footer_of(&blocks);
        let payload = codec::encode_epoch(&Epoch {
            header: EpochHeader {
                chain_id: CHAIN_ID.to_owned(),
                pool,
                epoch: e,
                from,
                to,
                prev,
            },
            blocks,
            footer,
        });
        let zst = strk20_feed::compress(&payload);
        std::fs::write(feed.join(format!("epochs/{e:08}.strk20e.zst")), &zst).unwrap();
        let hash = strk20_feed::payload_sha256(&payload);
        (
            hash,
            ManifestEpoch {
                e,
                from,
                to,
                hash: hex::encode(hash),
                zst: hex::encode(strk20_feed::payload_sha256(&zst)),
                bytes: zst.len() as u64,
                anchor: None,
            },
        )
    }

    /// Publish `epochs` and an empty tail sitting directly on top of them.
    fn publish(feed: &Path, pool: Felt, epochs: Vec<ManifestEpoch>) {
        let top = epochs.last().expect("at least one epoch").to;
        let latest = epochs.last().unwrap().e;
        let manifest = Manifest {
            v: 1,
            chain_id: CHAIN_ID.to_owned(),
            pool: strk20_feed::felt_hex(&pool),
            genesis_block: 0,
            epoch_size: EPOCH_SIZE,
            head: ManifestHead {
                number: top,
                hash: strk20_feed::felt_hex(&Felt::from(0xb10c0000u64 + top)),
                l1_accepted: top,
                class: strk20_feed::felt_hex(&Felt::from(0xc1a55u64)),
                decode_state: "ok".to_owned(),
            },
            latest_epoch: Some(latest),
            epochs,
            snapshot: None,
        };
        std::fs::write(
            feed.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest).unwrap(),
        )
        .unwrap();

        let blocks: Vec<BlockLine> = Vec::new();
        let payload = codec::encode_head(&Head {
            header: HeadHeader {
                tail_from: top + 1,
                head: top,
                head_hash: Felt::from(0xb10c0000u64 + top),
                l1_accepted: top,
            },
            footer: footer_of(&blocks),
            blocks,
        });
        std::fs::write(feed.join("head.ndjson"), payload).unwrap();
    }

    fn write_genesis(feed: &Path, pool: Felt) {
        let genesis = Genesis {
            format: "strk20-feed".to_owned(),
            v: 1,
            chain_id: CHAIN_ID.to_owned(),
            pool: strk20_feed::felt_hex(&pool),
            genesis_block: 0,
            epoch_size: EPOCH_SIZE,
        };
        std::fs::write(
            feed.join("genesis.json"),
            serde_json::to_vec_pretty(&genesis).unwrap(),
        )
        .unwrap();
    }

    fn canonical(r: &SyncReport) -> serde_json::Value {
        serde_json::to_value(r).unwrap()
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 4)]
    async fn cold_start_after_a_spend_equals_warm_sync_across_the_spend() {
        let f = load_devnet_fixture();
        let dir = tempfile::tempdir().unwrap();
        let pool = f.constants.contract_address;
        let feed = dir.path().join("feed");
        std::fs::create_dir_all(feed.join("epochs")).unwrap();
        write_genesis(&feed, pool);

        let alice = f.constants.alice_address;
        let key = SecretFelt::new(f.constants.alice_viewing_key);
        let bob = f.constants.bob_address;
        let bob_key = SecretFelt::new(f.constants.bob_viewing_key);

        // ------------------------------------------- epoch 0: the note exists
        let slots: Vec<(Felt, Felt)> = f.slots.iter().map(|(k, v)| (*k, *v)).collect();
        let (e0_hash, e0) = cut_epoch(
            &feed,
            pool,
            0,
            0,
            E0_TO,
            vec![blk(MINT_BLOCK, slots, None)],
            None,
        );
        publish(&feed, pool, vec![e0.clone()]);

        let transport = DirTransport::new(feed.clone());
        let opts = SyncOptions::default();

        // The warm client folds the feed while the note is still unspent.
        let warm = MemStore::new();
        let before = sync_once(&warm, &transport, alice, &key, &opts)
            .await
            .expect("warm sync before the spend");
        assert_eq!(
            before.notes.len(),
            1,
            "fixture sanity: alice holds exactly one note here: {before:?}"
        );
        assert!(
            !before.notes[0].spent,
            "fixture sanity: it is unspent, so the spend below is a real transition"
        );

        // ------------------------------- epoch 1: the nullifier is written
        let nullifier = Felt::from_hex(&before.notes[0].nullifier).unwrap();
        let nullifier_slot = discovery_core::privacy_pool::storage_slots::nullifiers(nullifier);
        let (_, e1) = cut_epoch(
            &feed,
            pool,
            1,
            E0_TO + 1,
            E1_TO,
            vec![blk(SPEND_BLOCK, vec![(nullifier_slot, Felt::ONE)], None)],
            Some(e0_hash),
        );
        publish(&feed, pool, vec![e0, e1]);

        let warm_report = sync_once(&warm, &transport, alice, &key, &opts)
            .await
            .expect("warm sync across the spend");

        // ------------------------- the cold client: same bytes, no history
        let cold = MemStore::new();
        let cold_report = sync_once(&cold, &transport, alice, &key, &opts)
            .await
            .expect("cold sync after the spend");

        assert_eq!(
            canonical(&cold_report),
            canonical(&warm_report),
            "a client folding this feed for the first time and one that watched the spend \
             land must produce the same report, field for field: the report describes the \
             pool at {E1_TO}, not how long the client has been running"
        );
        assert_eq!(
            cold.notes(&alice).unwrap(),
            warm.notes(&alice).unwrap(),
            "and the registries behind them must be row-for-row identical"
        );

        // Non-vacuity: the equality must not be "both of them report nothing".
        assert_eq!(
            cold_report.notes.len(),
            1,
            "the spent note must still be REPORTED, not dropped: {cold_report:?}"
        );
        let note = &cold_report.notes[0];
        assert!(note.spent, "and it must be reported as spent: {cold_report:?}");
        assert_eq!(
            note.note_id, before.notes[0].note_id,
            "it is the same note the pre-spend report carried"
        );
        assert_eq!(
            note.nullifier, before.notes[0].nullifier,
            "with the same nullifier, re-derived from the cursor's channel key"
        );
        assert_eq!(
            note.block_number, before.notes[0].block_number,
            "and the same creation block — for a note the engine never returns, that \
             value can only come from the note slot's own write block"
        );
        assert_eq!(
            note.amount, before.notes[0].amount,
            "and the same decrypted amount, unpacked without the engine"
        );
        assert!(
            cold_report.balances.is_empty(),
            "a spent note contributes nothing to the balance: {cold_report:?}"
        );

        // ------ a note spent before ANY client existed is reported too (bob)
        //
        // The devnet fixture's bob note is spent in the seed slots themselves,
        // so the engine returns bob zero notes at every bound (pinned by
        // tests/oracle_probe.rs). Whatever bob reports here came from the
        // scanned-range sweep and from nowhere else.
        let fresh = MemStore::new();
        let bob_report = sync_once(&fresh, &transport, bob, &bob_key, &opts)
            .await
            .expect("cold sync for bob");
        assert_eq!(
            bob_report.notes.len(),
            1,
            "bob's seed note is spent in the feed's first epoch and the engine never \
             returns it; a cold start must still report it: {bob_report:?}"
        );
        assert!(bob_report.notes[0].spent, "{bob_report:?}");
        assert!(bob_report.balances.is_empty(), "{bob_report:?}");
    }
}
