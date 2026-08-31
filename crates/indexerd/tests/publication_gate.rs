//! The §11.3 snapshot publication gate, driven directly rather than through a
//! backfill (spec §8 leg m(v)).
//!
//! Why directly: the gate has two conditions and the e2e legs can only reach
//! one of them. `verify_and_capture` runs at the TOP of a cut batch and returns
//! `Err` on a MISMATCH, so a fixture that forces a real divergence never gets
//! as far as `maybe_publish_snapshot` — the `verify_root_failed == "1"` branch
//! is guarding a state no end-to-end fixture can construct. Before this file
//! that `if` could be deleted with the whole suite still green.
//!
//! Everything here is a pure function of DB rows, so no RPC is involved.

use starknet_types_core::felt::Felt;
use strk20_indexerd::config::ChainConfig;
use strk20_indexerd::cutter::{Anchor, Cutter};
use strk20_indexerd::db::{BlockRow, Db};
use strk20_indexerd::rpc::RpcClient;

const CHAIN_ID: &str = "SN_TEST";
const EPOCH_SIZE: u64 = 16;
/// Epoch 1 = [16, 31]; the snapshot's basis is 31.
const BASIS: u64 = 31;
/// The head-captured anchor that satisfies the gate, above the basis (§11.2:
/// captures are head-driven, never at an epoch boundary).
const ANCHOR_BLOCK: u64 = 40;

fn cfg() -> ChainConfig {
    let mut c = ChainConfig::mainnet();
    c.chain_id = CHAIN_ID.to_owned();
    c.pool = Felt::from_hex("0x0f001").unwrap();
    c.genesis_block = 0;
    c.epoch_size = EPOCH_SIZE;
    c
}

fn block(n: u64) -> BlockRow {
    BlockRow {
        number: n,
        hash: Felt::from(0x1000_0000u64 + n),
        parent_hash: Felt::from(0x1000_0000u64 + n.saturating_sub(1)),
        timestamp: 1_700_000_000 + n,
        l1_accepted: true,
    }
}

/// A mirror with one cut epoch, some slot writes at or below the basis, and a
/// head-captured anchor above it — i.e. the §11.3 gate MET.
fn gated_mirror(dir: &std::path::Path, anchor_at: Option<u64>) -> (Db, ChainConfig) {
    let mut db = Db::open(&dir.join("strk20.db")).expect("open db");
    for (n, slot, value) in [(20u64, 0xaa_u64, 0x11_u64), (28, 0xbb, 0x22), (BASIS, 0xcc, 0x33)] {
        db.insert_block_data(
            &block(n),
            &[(Felt::from(slot), Felt::from(value))],
            &[],
            None,
            n,
            None,
        )
        .expect("insert block data");
    }
    db.insert_block_data(&block(ANCHOR_BLOCK), &[], &[], None, ANCHOR_BLOCK, None)
        .expect("insert anchor block");
    db.meta_set("head_number", &ANCHOR_BLOCK.to_string()).unwrap();

    db.insert_epoch(1, 16, BASIS, &[7u8; 32], &[8u8; 32], 123, None, None, ANCHOR_BLOCK)
        .expect("insert epoch");
    if let Some(at) = anchor_at {
        db.insert_anchor(&Anchor {
            block: at,
            block_hash: block(at).hash,
            storage_root: Felt::from(0x5eed_u64),
            class_hash: Felt::from(0x67dd_u64),
        })
        .expect("insert anchor");
    }
    (db, cfg())
}

fn snapshot_files(feed_dir: &std::path::Path) -> Vec<String> {
    std::fs::read_dir(feed_dir.join("snapshots"))
        .map(|rd| {
            rd.flatten()
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .filter(|n| n.ends_with(".strk20s.zst"))
                .collect()
        })
        .unwrap_or_default()
}

fn manifest_snapshot(feed_dir: &std::path::Path) -> serde_json::Value {
    let bytes = std::fs::read(feed_dir.join("manifest.json")).expect("manifest.json");
    let v: serde_json::Value = serde_json::from_slice(&bytes).expect("manifest is JSON");
    v["snapshot"].clone()
}

/// Control: with the gate met and no latched verify-root failure, a snapshot
/// IS published. Every negative below is therefore about the latch and not
/// about a fixture that could never publish anything.
#[test]
fn the_gate_publishes_when_the_mirror_last_matched_the_chain() {
    let dir = tempfile::tempdir().unwrap();
    let (db, cfg) = gated_mirror(dir.path(), Some(ANCHOR_BLOCK));
    let rpc = RpcClient::new("http://127.0.0.1:1/unused".into(), None);
    let cutter = Cutter {
        db: &db,
        rpc: &rpc,
        cfg: &cfg,
        feed_dir: dir.path().join("feed"),
    };
    cutter.ensure_layout().unwrap();
    cutter.maybe_publish_snapshot().expect("publish");
    cutter.rewrite_manifest().unwrap();

    assert_eq!(
        snapshot_files(&dir.path().join("feed")),
        vec!["00000001.strk20s.zst".to_owned()],
        "§11.3: an anchor at {ANCHOR_BLOCK} >= basis {BASIS} with no verified mismatch \
         since is the gate, and it is met here"
    );
    let entry = manifest_snapshot(&dir.path().join("feed"));
    assert_eq!(entry["e"].as_u64(), Some(1), "{entry}");
    assert_eq!(entry["block"].as_u64(), Some(BASIS), "{entry}");
}

/// §8 leg m(v) — after a verify-root failure, NO snapshot file and NO manifest
/// snapshot entry are produced, even though the §11.3 anchor gate is otherwise
/// met.
///
/// The two conditions are independent: an anchor at or above the basis says the
/// mirror matched the chain at some point, while `verify_root_failed` says it
/// has since been caught NOT matching. Publishing on the first while the second
/// is latched would ship a slot set the operator already knows is wrong.
#[test]
fn a_latched_verify_root_failure_blocks_publication_even_with_an_anchor() {
    let dir = tempfile::tempdir().unwrap();
    let (db, cfg) = gated_mirror(dir.path(), Some(ANCHOR_BLOCK));
    db.meta_set("verify_root_failed", "1").unwrap();
    let rpc = RpcClient::new("http://127.0.0.1:1/unused".into(), None);
    let cutter = Cutter {
        db: &db,
        rpc: &rpc,
        cfg: &cfg,
        feed_dir: dir.path().join("feed"),
    };
    cutter.ensure_layout().unwrap();
    cutter.maybe_publish_snapshot().expect("no error, just no publication");
    cutter.rewrite_manifest().unwrap();

    assert!(
        db.newest_anchor_block().unwrap().unwrap_or(0) >= BASIS,
        "non-vacuity: the anchor half of the gate IS met, so the refusal below comes \
         from the latched failure and not from an ungrounded mirror"
    );
    assert!(
        snapshot_files(&dir.path().join("feed")).is_empty(),
        "a mirror known to disagree with the chain must publish no snapshot file: {:?}",
        snapshot_files(&dir.path().join("feed"))
    );
    assert!(
        manifest_snapshot(&dir.path().join("feed")).is_null(),
        "...and no manifest entry naming one"
    );

    // Recovery: once the failure is cleared (the §5.6 rescan re-verified),
    // publication resumes with no new epoch cut.
    db.meta_set("verify_root_failed", "").unwrap();
    cutter.maybe_publish_snapshot().expect("publish after recovery");
    cutter.rewrite_manifest().unwrap();
    assert_eq!(
        manifest_snapshot(&dir.path().join("feed"))["e"].as_u64(),
        Some(1),
        "the latch is a gate, not a permanent ban"
    );
}

/// The other half of the gate: no anchor at or above the basis means nothing
/// grounds the snapshot, so none may be published.
#[test]
fn no_anchor_at_or_above_the_basis_publishes_nothing() {
    let dir = tempfile::tempdir().unwrap();
    // The only anchor lies BELOW the basis, so it attests nothing about the
    // snapshot's slot set.
    let (db, cfg) = gated_mirror(dir.path(), Some(20));
    let rpc = RpcClient::new("http://127.0.0.1:1/unused".into(), None);
    let cutter = Cutter {
        db: &db,
        rpc: &rpc,
        cfg: &cfg,
        feed_dir: dir.path().join("feed"),
    };
    cutter.ensure_layout().unwrap();
    cutter.maybe_publish_snapshot().expect("no error");
    cutter.rewrite_manifest().unwrap();
    assert!(snapshot_files(&dir.path().join("feed")).is_empty());
    assert!(manifest_snapshot(&dir.path().join("feed")).is_null());
}

/// Retention on the anchor log: the file is rewritten in full on every capture
/// and downloaded in full by every grounded client, so it cannot be allowed to
/// grow without bound (~2 900 records/day on mainnet).
#[test]
fn the_anchor_log_is_bounded() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("strk20.db")).expect("open db");
    let keep = Db::ANCHOR_KEEP as u64;
    for n in 1..=keep + 25 {
        db.insert_anchor(&Anchor {
            block: n,
            block_hash: Felt::from(n),
            storage_root: Felt::from(n),
            class_hash: Felt::from(1u64),
        })
        .unwrap();
    }
    db.prune_anchors().unwrap();
    let kept = db.anchors().unwrap();
    assert_eq!(kept.len(), Db::ANCHOR_KEEP, "retention keeps a bounded window");
    assert_eq!(
        kept.last().map(|a| a.block),
        Some(keep + 25),
        "the NEWEST records survive — reachability and ring 6 both want the recent end"
    );
    assert_eq!(kept.first().map(|a| a.block), Some(26));
}
