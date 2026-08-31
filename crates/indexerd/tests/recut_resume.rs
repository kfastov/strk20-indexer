//! A backward re-cut that dies part-way must leave a SERVABLE feed and must be
//! resumable by re-running the documented command.
//!
//! Why this file exists, and why it drives `Cutter` directly: the failure it
//! pins is an abort in the middle of `recut_epochs_from`'s loop, and no
//! end-to-end fixture can stop a subprocess between two epochs. Here the abort
//! is injected deterministically — a directory sitting where one epoch's
//! `.zst` has to be written makes that one `rename` fail and nothing else.
//!
//! Two properties, both learned the hard way from the 515-epoch mainnet feed:
//!
//! 1. **The manifest never lags the files it names.** It is ring 1 of every
//!    client's ladder, so a manifest that still carries pre-repair hashes over
//!    post-repair bytes fails every client at once — fail-closed, but a total
//!    feed outage rather than a partial repair.
//! 2. **The refusal guard must not trap the operator.** The epochs an aborted
//!    run already rewrote now rebuild byte-identically, so a guard that asks
//!    only about the first epoch named refuses the retry — with a message that
//!    is false in that state — while everything above stays stale.

use starknet_types_core::felt::Felt;
use strk20_feed::codec;
use strk20_indexerd::config::ChainConfig;
use strk20_indexerd::cutter::Cutter;
use strk20_indexerd::db::{BlockRow, Db};
use strk20_indexerd::rpc::RpcClient;

const CHAIN_ID: &str = "SN_TEST";
const EPOCH_SIZE: u64 = 16;
/// Epochs 0..=3 are published; the repair lands in epoch 1.
const LAST_EPOCH: u64 = 3;
const REPAIRED_EPOCH: u64 = 1;
/// The epoch whose write is made to fail, so the abort happens with epochs
/// 1 and 2 already committed.
const ABORT_EPOCH: u64 = 3;

fn cfg() -> ChainConfig {
    let mut c = ChainConfig::mainnet();
    c.chain_id = CHAIN_ID.to_owned();
    c.pool = Felt::from_hex("0x0f002").unwrap();
    c.genesis_block = 0;
    c.epoch_size = EPOCH_SIZE;
    c
}

fn block(n: u64) -> BlockRow {
    BlockRow {
        number: n,
        hash: Felt::from(0x2000_0000u64 + n),
        parent_hash: Felt::from(0x2000_0000u64 + n.saturating_sub(1)),
        timestamp: 1_700_000_000 + n,
        l1_accepted: true,
    }
}

/// One pool-active block per epoch, plus the head block.
fn mirror(dir: &std::path::Path) -> Db {
    let mut db = Db::open(&dir.join("strk20.db")).expect("open db");
    for e in 0..=LAST_EPOCH {
        let n = e * EPOCH_SIZE + 4;
        db.insert_block_data(
            &block(n),
            &[(Felt::from(0x1000u64 + n), Felt::from(0x7000u64 + n))],
            &[],
            None,
            n,
        )
        .expect("insert block data");
    }
    let head = (LAST_EPOCH + 1) * EPOCH_SIZE;
    db.insert_block_data(&block(head), &[], &[], None, head)
        .expect("insert head block");
    db.meta_set("head_number", &head.to_string()).unwrap();
    db
}

/// Publish epochs 0..=LAST_EPOCH exactly as the forward cut does: chained
/// `prev`, canonical payload, zstd, DB row, then the manifest.
fn publish_all(cutter: &Cutter<'_>) {
    cutter.ensure_layout().expect("layout");
    let mut prev: Option<[u8; 32]> = None;
    for idx in 0..=LAST_EPOCH {
        let (from, to) = cutter.cfg.epoch_range(idx);
        let epoch = cutter.build_epoch(idx, prev).expect("build epoch");
        let payload = codec::encode_epoch(&epoch);
        let content_hash = strk20_feed::payload_sha256(&payload);
        let compressed = strk20_feed::compress(&payload);
        std::fs::write(
            cutter.epochs_dir().join(format!("{idx:08}.strk20e.zst")),
            &compressed,
        )
        .expect("write epoch file");
        cutter
            .db
            .insert_epoch(
                idx,
                from,
                to,
                &content_hash,
                &strk20_feed::payload_sha256(&compressed),
                compressed.len() as u64,
                prev.as_ref(),
                None,
                0,
            )
            .expect("insert epoch");
        prev = Some(content_hash);
    }
    cutter.rewrite_manifest().expect("manifest");
}

/// (epoch, content hash) as the DB has it.
fn db_hashes(db: &Db) -> Vec<(u64, String)> {
    db.epoch_rows()
        .expect("epoch rows")
        .iter()
        .map(|r| (r.idx, hex::encode(r.content_hash)))
        .collect()
}

/// (epoch, content hash) as the PUBLISHED manifest has it — what a client
/// checks its downloaded bytes against.
fn manifest_hashes(feed_dir: &std::path::Path) -> Vec<(u64, String)> {
    let v: serde_json::Value =
        serde_json::from_slice(&std::fs::read(feed_dir.join("manifest.json")).expect("manifest"))
            .expect("manifest is JSON");
    v["epochs"]
        .as_array()
        .expect("epochs array")
        .iter()
        .map(|e| {
            (
                e["e"].as_u64().unwrap(),
                e["hash"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

/// sha256 of each published `.zst`, as a client would hash it before
/// decompressing.
fn file_zst_hashes(cutter: &Cutter<'_>, epochs: &[u64]) -> Vec<(u64, String)> {
    epochs
        .iter()
        .map(|idx| {
            let bytes = std::fs::read(cutter.epochs_dir().join(format!("{idx:08}.strk20e.zst")))
                .expect("epoch file");
            (*idx, hex::encode(strk20_feed::payload_sha256(&bytes)))
        })
        .collect()
}

/// What the manifest promises about the `.zst` files, per epoch.
fn manifest_zst_hashes(feed_dir: &std::path::Path) -> Vec<(u64, String)> {
    let v: serde_json::Value =
        serde_json::from_slice(&std::fs::read(feed_dir.join("manifest.json")).expect("manifest"))
            .expect("manifest is JSON");
    v["epochs"]
        .as_array()
        .expect("epochs array")
        .iter()
        .map(|e| {
            (
                e["e"].as_u64().unwrap(),
                e["zst"].as_str().unwrap().to_owned(),
            )
        })
        .collect()
}

#[test]
fn an_aborted_re_cut_leaves_a_consistent_manifest_and_the_retry_resumes_it() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = cfg();
    let rpc = RpcClient::new("http://127.0.0.1:1/unused".into(), None);
    let mut db = mirror(dir.path());
    let feed_dir = dir.path().join("feed");
    {
        let cutter = Cutter {
            db: &db,
            rpc: &rpc,
            cfg: &cfg,
            feed_dir: feed_dir.clone(),
        };
        publish_all(&cutter);
    }
    let before = db_hashes(&db);
    assert_eq!(
        manifest_hashes(&feed_dir),
        before,
        "control: the published manifest describes the published epochs"
    );

    // The repair: a block the mirror had lost, inside epoch 1. Every epoch from
    // 1 up now disagrees with the database — 1 by its own content, 2 and 3
    // through `prev`.
    let repaired_block = REPAIRED_EPOCH * EPOCH_SIZE + 9;
    db.insert_block_data(
        &block(repaired_block),
        &[(Felt::from(0xbeefu64), Felt::from(0xcafeu64))],
        &[],
        None,
        repaired_block,
    )
    .expect("repair the hole");

    // The abort: a directory where epoch 3's file has to be written, so the
    // rename fails after epochs 1 and 2 have been committed.
    let blocked = feed_dir
        .join("epochs")
        .join(format!("{ABORT_EPOCH:08}.strk20e.zst"));
    std::fs::remove_file(&blocked).expect("clear the epoch file");
    std::fs::create_dir(&blocked).expect("block the write");

    let cutter = Cutter {
        db: &db,
        rpc: &rpc,
        cfg: &cfg,
        feed_dir: feed_dir.clone(),
    };
    let err = cutter
        .recut_epochs_from(REPAIRED_EPOCH)
        .expect_err("the injected fault must abort the re-cut");
    assert!(
        format!("{err:#}").contains(&format!("{ABORT_EPOCH:08}.strk20e.zst")),
        "the abort must be the injected one and not something else: {err:#}"
    );

    // PROPERTY 1 — the partially repaired feed is still internally consistent:
    // the manifest names exactly the bytes on disk for every epoch it lists.
    // Committed after the whole loop instead, it would still promise the
    // PRE-repair hashes for epochs 1 and 2, whose files have already been
    // replaced, and every client would hard-fail ring 1.
    let partial = db_hashes(&db);
    assert_eq!(
        manifest_hashes(&feed_dir),
        partial,
        "after an abort the manifest must describe the epochs as they now stand"
    );
    let landed: Vec<u64> = (0..=LAST_EPOCH).filter(|e| *e != ABORT_EPOCH).collect();
    let on_disk = file_zst_hashes(&cutter, &landed);
    let promised: Vec<(u64, String)> = manifest_zst_hashes(&feed_dir)
        .into_iter()
        .filter(|(e, _)| *e != ABORT_EPOCH)
        .collect();
    assert_eq!(
        on_disk, promised,
        "ring 1 is the FIRST thing a client checks: every .zst the aborted run wrote must \
         hash to what the manifest promises for it"
    );
    assert_ne!(
        partial[REPAIRED_EPOCH as usize].1, before[REPAIRED_EPOCH as usize].1,
        "fixture precondition: the aborted run must really have rewritten epoch \
         {REPAIRED_EPOCH}, or there is no partial state to resume from"
    );
    assert_eq!(
        partial[ABORT_EPOCH as usize].1, before[ABORT_EPOCH as usize].1,
        "fixture precondition: epoch {ABORT_EPOCH} must still be stale, or the abort did \
         not abort anything"
    );

    // PROPERTY 2 — re-running the documented command finishes the job. The
    // epochs the aborted run wrote now rebuild byte-identically, which is
    // exactly the shape the old first-epoch-only guard mistook for "nothing
    // changed" and refused, leaving the operator with a half-repaired feed and
    // no path forward.
    std::fs::remove_dir(&blocked).expect("clear the injected fault");
    let out = cutter
        .recut_epochs_from(REPAIRED_EPOCH)
        .expect("the retry must resume, not refuse");
    assert_eq!(
        out.already_current,
        vec![REPAIRED_EPOCH, ABORT_EPOCH - 1],
        "the epochs the aborted run finished must be recognised and left alone"
    );
    assert_eq!(
        out.rewritten.iter().map(|(e, _, _)| *e).collect::<Vec<_>>(),
        vec![ABORT_EPOCH],
        "only the epoch that never landed is rewritten by the resumed run"
    );

    let after = db_hashes(&db);
    assert_eq!(
        manifest_hashes(&feed_dir),
        after,
        "the resumed run republishes the manifest"
    );
    let all: Vec<u64> = (0..=LAST_EPOCH).collect();
    assert_eq!(
        file_zst_hashes(&cutter, &all),
        manifest_zst_hashes(&feed_dir),
        "every published file must hash to what the manifest promises once the repair is \
         complete"
    );
    for (idx, (old, new)) in before.iter().zip(after.iter()).enumerate() {
        if (idx as u64) < REPAIRED_EPOCH {
            assert_eq!(old, new, "epoch {idx} is below the repair and must not move");
        } else {
            assert_ne!(
                old, new,
                "epoch {idx} is at or above the repair: its content or its `prev` changed, \
                 so its hash must have changed too"
            );
        }
    }

    // PROPERTY 3 — and the guard is still a guard. With the whole range now
    // consistent, a third run is refused with nothing written.
    let err = cutter
        .recut_epochs_from(REPAIRED_EPOCH)
        .expect_err("a re-cut of a fully consistent range must be refused");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("REFUSING TO RE-CUT"),
        "the refusal must say the tool declined rather than broke: {msg}"
    );
    assert_eq!(
        db_hashes(&db),
        after,
        "a refused re-cut must leave every epoch row exactly as it was"
    );
    assert_eq!(
        file_zst_hashes(&cutter, &all),
        manifest_zst_hashes(&feed_dir),
        "a refused re-cut must not have rewritten a single published byte"
    );
}

/// The other half of the guard, on a non-contiguous stale run: a repair that
/// touched two distant epochs, re-cut from the HIGHER one. Stopping the
/// downward scan at the first epoch that still matches sees only the
/// contiguous run and lets the lower repair stay unpublished forever — under a
/// hash chain `epoch-verify` calls OK, because nothing else in the system
/// compares feed bytes against the database.
#[test]
fn the_stale_below_guard_sees_a_non_contiguous_lower_repair() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = cfg();
    let rpc = RpcClient::new("http://127.0.0.1:1/unused".into(), None);
    let mut db = mirror(dir.path());
    let feed_dir = dir.path().join("feed");
    {
        let cutter = Cutter {
            db: &db,
            rpc: &rpc,
            cfg: &cfg,
            feed_dir: feed_dir.clone(),
        };
        publish_all(&cutter);
    }

    // Epoch 0 and epoch 2 both gained a block; epoch 1 did not. Re-cutting from
    // 2 would leave epoch 0 publishing pre-repair bytes.
    for e in [0u64, 2] {
        let n = e * EPOCH_SIZE + 9;
        db.insert_block_data(
            &block(n),
            &[(Felt::from(0xd00du64 + n), Felt::from(0xf00du64 + n))],
            &[],
            None,
            n,
        )
        .expect("repair");
    }

    let cutter = Cutter {
        db: &db,
        rpc: &rpc,
        cfg: &cfg,
        feed_dir: feed_dir.clone(),
    };
    let err = cutter
        .recut_epochs_from(2)
        .expect_err("a re-cut that leaves a lower stale epoch behind must be refused");
    let msg = format!("{err:#}");
    assert!(
        msg.contains("epoch 0"),
        "the guard must name the LOWEST stale epoch (0), not the nearest one: {msg}"
    );
    assert_eq!(
        manifest_hashes(&feed_dir),
        db_hashes(&db),
        "a refused re-cut writes nothing"
    );
}
