//! Fixture generator for the Node smoke test — **native only**, never compiled
//! into the module (it is an example, and its zstd dependency is a dev-dep).
//!
//! It does two things:
//!
//! 1. builds a real, fully-verifiable feed out of the upstream devnet fixture
//!    (`fixtures/upstream/devnet-state.json` — the same 48 pool slots the
//!    conformance suite folds), complete with an epoch, a snapshot, an anchors
//!    log and a head tail;
//! 2. folds it **the native way** — `sync_once` over `MemStore` through a
//!    transport that really runs zstd — and writes the resulting `SyncReport`
//!    out as the golden.
//!
//! The Node smoke test then folds the same bytes through the wasm module and
//! demands byte-equal report JSON. That equality is the deliverable: it says
//! the browser is running the engine, not a simulation of it.
//!
//! Run: `cargo run -p strk20-engine --example make_fixture`

use anyhow::Result;
use async_trait::async_trait;
use discovery_core::privacy_pool::types::SecretFelt;
use serde::Deserialize;
use starknet_types_core::felt::Felt;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use strk20_consumer::mem::MemStore;
use strk20_consumer::store::ColdStart;
use strk20_consumer::sync::{sync_once, SyncOptions};
use strk20_consumer::transport::FeedTransport;
use strk20_feed::anchors::AnchorRecord;
use strk20_feed::codec::{self, BlockLine, Epoch, EpochHeader, Footer, Head, HeadHeader};
use strk20_feed::manifest::{
    Genesis, Manifest, ManifestEpoch, ManifestHead, ManifestSnapshot, GROUNDING_REACHABILITY,
};
use strk20_feed::snapshot::{SnapSlot, SnapshotHeader, KIND_SNAPSHOT, SNAPSHOT_VERSION};

const CHAIN_ID: &str = "SN_SEPOLIA";
const EPOCH_SIZE: u64 = 100;
/// The block the fixture's slots are written at (same as the conformance leg).
const FIXTURE_BLOCK: u64 = 46;
const EPOCH_END: u64 = EPOCH_SIZE - 1;
const CLASS: u64 = 0xc1a55;

// ---------------------------------------------------------------- fixture

#[derive(Debug, Clone, Deserialize)]
struct DevnetConstants {
    contract_address: Felt,
    alice_address: Felt,
    alice_viewing_key: Felt,
    bob_address: Felt,
    bob_viewing_key: Felt,
}

#[derive(Debug, Clone, Deserialize)]
struct DevnetFixture {
    constants: DevnetConstants,
    slots: HashMap<Felt, Felt>,
}

// ------------------------------------------------------- native transport

/// The native path, minus HTTP: real zstd, real hashes. This is what
/// `strk20-sync` does over the wire, so the golden it produces is the native
/// answer and not an artefact of the test harness.
#[derive(Default)]
struct RealTransport {
    genesis: Vec<u8>,
    manifest: Vec<u8>,
    epochs: HashMap<u64, Vec<u8>>,
    snapshots: HashMap<u64, Vec<u8>>,
    anchors: Vec<u8>,
    head: Vec<u8>,
}

#[async_trait]
impl FeedTransport for RealTransport {
    async fn fetch_genesis(&self) -> Result<Genesis> {
        Ok(serde_json::from_slice(&self.genesis)?)
    }
    async fn fetch_manifest(&self) -> Result<Manifest> {
        Ok(serde_json::from_slice(&self.manifest)?)
    }
    async fn fetch_epoch(&self, idx: u64) -> Result<Vec<u8>> {
        Ok(self.epochs[&idx].clone())
    }
    async fn fetch_snapshot(&self, e: u64) -> Result<Vec<u8>> {
        Ok(self.snapshots[&e].clone())
    }
    async fn fetch_anchor(&self, _idx: u64) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
    async fn fetch_snapshot_anchor(&self, _e: u64) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
    async fn fetch_anchors(&self) -> Result<Option<Vec<u8>>> {
        Ok(Some(self.anchors.clone()))
    }
    async fn fetch_head(&self, etag: Option<&str>) -> Result<Option<(Vec<u8>, String)>> {
        let tag = hex::encode(strk20_feed::payload_sha256(&self.head));
        if etag == Some(tag.as_str()) {
            return Ok(None);
        }
        Ok(Some((self.head.clone(), tag)))
    }
    fn decompress(&self, bytes: &[u8], cap: u64, artifact: &str) -> Result<Vec<u8>> {
        Ok(strk20_feed::decompress_capped(bytes, cap, artifact)?)
    }
}

// ---------------------------------------------------------------- builders

fn blk(number: u64, diffs: Vec<(Felt, Felt)>) -> BlockLine {
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
        finality: None,
    }
}

fn footer_of(blocks: &[BlockLine]) -> Footer {
    Footer {
        blocks: blocks.len() as u64,
        diffs: blocks.iter().map(|b| b.diffs.len() as u64).sum(),
        events: blocks.iter().map(|b| b.events.len() as u64).sum(),
        class: Felt::from(CLASS),
    }
}

fn write(path: &Path, bytes: &[u8]) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, bytes)?;
    Ok(())
}

#[tokio::main(flavor = "current_thread")]
async fn main() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let out = root.join("fixture");
    let fixture: DevnetFixture = serde_json::from_str(&std::fs::read_to_string(
        root.join("../../fixtures/upstream/devnet-state.json"),
    )?)?;
    let pool = fixture.constants.contract_address;
    let mut slots: Vec<(Felt, Felt)> = fixture.slots.iter().map(|(k, v)| (*k, *v)).collect();
    slots.sort_by_key(|(k, _)| k.to_bytes_be());

    // ------------------------------------------------------------- epoch 0
    let blocks = vec![blk(FIXTURE_BLOCK, slots.clone())];
    let epoch_payload = codec::encode_epoch(&Epoch {
        header: EpochHeader {
            chain_id: CHAIN_ID.to_owned(),
            pool,
            epoch: 0,
            from: 0,
            to: EPOCH_END,
            prev: None,
        },
        footer: footer_of(&blocks),
        blocks,
    });
    let epoch_zst = strk20_feed::compress(&epoch_payload);
    let epoch_hash = hex::encode(strk20_feed::payload_sha256(&epoch_payload));

    // ------------------------------- snapshot at the epoch boundary (b=99)
    //
    // A snapshot is the folded slot state as of its basis, carrying each
    // slot's REAL write block — which is what makes per-note `block_number`
    // survive a cold start that has no events at all.
    let snap_slots: Vec<SnapSlot> = slots
        .iter()
        .filter(|(_, v)| *v != Felt::ZERO)
        .map(|(k, v)| SnapSlot {
            k: *k,
            v: *v,
            w: FIXTURE_BLOCK,
        })
        .collect();
    let live: Vec<(Felt, Felt)> = snap_slots.iter().map(|s| (s.k, s.v)).collect();
    let storage_root = strk20_feed::mpt::storage_root(&live);
    let snapshot = strk20_feed::snapshot::Snapshot {
        header: SnapshotHeader {
            v: SNAPSHOT_VERSION,
            kind: KIND_SNAPSHOT.to_owned(),
            chain_id: CHAIN_ID.to_owned(),
            pool,
            epoch: 0,
            block: EPOCH_END,
            epoch_hash: epoch_hash.clone(),
            storage_root,
            class: Felt::from(CLASS),
        },
        slots: snap_slots,
    };
    let snap_payload = strk20_feed::snapshot::encode(&snapshot);
    let snap_zst = strk20_feed::compress(&snap_payload);

    // The §11.3 grounding: an anchor at the basis whose root the client must
    // reproduce by folding the snapshot itself.
    let anchors = strk20_feed::anchors::encode_anchors(&[AnchorRecord {
        block: EPOCH_END,
        block_hash: Felt::from(0xb10c0000u64 + EPOCH_END),
        storage_root,
        class: Felt::from(CLASS),
    }])?;

    // ------------------------------------------------------------ head/tail
    let head_payload = codec::encode_head(&Head {
        header: HeadHeader {
            tail_from: EPOCH_SIZE,
            head: EPOCH_END,
            head_hash: Felt::from(0xb10c0000u64 + EPOCH_END),
            l1_accepted: EPOCH_END,
        },
        blocks: Vec::new(),
        footer: footer_of(&[]),
    });

    // ------------------------------------------------------ genesis/manifest
    let genesis = Genesis {
        format: "strk20-feed".to_owned(),
        v: 1,
        chain_id: CHAIN_ID.to_owned(),
        pool: strk20_feed::felt_hex(&pool),
        genesis_block: 0,
        epoch_size: EPOCH_SIZE,
    };
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
            class: strk20_feed::felt_hex(&Felt::from(CLASS)),
            decode_state: "ok".to_owned(),
        },
        latest_epoch: Some(0),
        epochs: vec![ManifestEpoch {
            e: 0,
            from: 0,
            to: EPOCH_END,
            hash: epoch_hash.clone(),
            zst: hex::encode(strk20_feed::payload_sha256(&epoch_zst)),
            bytes: epoch_zst.len() as u64,
            anchor: None,
        }],
        snapshot: Some(ManifestSnapshot {
            e: 0,
            block: EPOCH_END,
            epoch_hash: epoch_hash.clone(),
            file: strk20_feed::snapshot::snapshot_file_name(0),
            hash: hex::encode(strk20_feed::payload_sha256(&snap_payload)),
            zst: hex::encode(strk20_feed::payload_sha256(&snap_zst)),
            bytes: snap_zst.len() as u64,
            slots: snapshot.slots.len() as u64,
            storage_root: strk20_feed::felt_hex(&storage_root),
            anchor: None,
            grounding: GROUNDING_REACHABILITY.to_owned(),
        }),
    };
    let genesis_json = serde_json::to_vec_pretty(&genesis)?;
    let manifest_json = serde_json::to_vec_pretty(&manifest)?;

    // ------------------------------------------------------- write the feed
    write(&out.join("genesis.json"), &genesis_json)?;
    write(&out.join("manifest.json"), &manifest_json)?;
    write(&out.join("epochs/0.ndjson"), &epoch_payload)?;
    write(&out.join("snapshots/0.ndjson"), &snap_payload)?;
    write(&out.join("snapshots/0.zst"), &snap_zst)?;
    write(&out.join("anchors.ndjson"), &anchors)?;
    write(&out.join("head.ndjson"), &head_payload)?;

    let owners = [
        (
            "alice",
            fixture.constants.alice_address,
            fixture.constants.alice_viewing_key,
        ),
        (
            "bob",
            fixture.constants.bob_address,
            fixture.constants.bob_viewing_key,
        ),
    ];
    write(
        &out.join("owners.json"),
        serde_json::to_string_pretty(&serde_json::json!(owners
            .iter()
            .map(|(name, owner, key)| serde_json::json!({
                "name": name,
                "owner": strk20_feed::felt_hex(owner),
                // 64-hex, big-endian — exactly the 32 bytes `discover` takes.
                "key": hex::encode(key.to_bytes_be()),
            }))
            .collect::<Vec<_>>()))?
        .as_bytes(),
    )?;

    // ------------------------------------------- the golden: the native fold
    let transport = RealTransport {
        genesis: genesis_json.clone(),
        manifest: manifest_json.clone(),
        epochs: [(0u64, epoch_zst.clone())].into_iter().collect(),
        snapshots: [(0u64, snap_zst.clone())].into_iter().collect(),
        anchors: anchors.clone(),
        head: head_payload.clone(),
    };

    let mut total_notes = 0usize;
    for (mode, cold) in [("auto", ColdStart::Auto), ("epochs", ColdStart::Epochs)] {
        for (name, owner, key) in &owners {
            // One store per (mode, owner) so each golden is a cold start, which
            // is what the Node test replays.
            let store = MemStore::new();
            let opts = SyncOptions {
                cold_start: cold,
                anchor_proofs: None,
            };
            let report = sync_once(
                &store,
                &transport,
                *owner,
                &SecretFelt::new(*key),
                &opts,
            )
            .await?;
            total_notes += report.notes.len();
            println!(
                "  golden {mode}/{name}: {} notes, verified={}, history_from={}",
                report.notes.len(),
                report.verified,
                report.history_from
            );
            write(
                &out.join(format!("golden/{mode}/{name}.json")),
                serde_json::to_vec_pretty(&report)?.as_slice(),
            )?;
        }
    }

    // A golden of zero notes would make the whole equality claim vacuous.
    anyhow::ensure!(
        total_notes > 0,
        "the fixture produced no notes; the smoke test would prove nothing"
    );
    println!("fixture written to {}", out.display());
    Ok(())
}
