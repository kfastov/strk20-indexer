//! Live-network robustness legs (docs/research/live/live-run-findings.md).
//!
//! Every leg here reproduces a defect MEASURED against real networks and pins
//! the property the fix must establish — never the shape of the fix. The
//! fixture RPC injects the provider behaviours that produced each defect
//! (`FaultSpec`): lava's pruned backends, pathfinder's ~1024-block
//! storage-proof window, publicnode's missing proofs, 429 throttling.
//!
//! T1  LIVE-1  backfill survives a pruned-range JSON-RPC error
//! T2  LIVE-3  a 429 storm does not fail over off the primary
//! T3  LIVE-4  verify-root picks a block inside the live proof window
//! T4  LIVE-4/6 no provable block => UNAVAILABLE, never failure/DEGRADED
//! T5  LIVE-4  a real divergence still MISMATCHes inside the window
//! T6  LIVE-5  anchors.ndjson: canonical, stable, monotonic, client-checked
//! T7  LIVE-5  a tampered anchor is rejected by the client
//! T8  pack B  --network sepolia selects the whole Sepolia profile
//! T9  pack B  the client refuses a feed from another chain
//! T10 LIVE-2  the scan phase reports progress
//! T14 LIVE-8  the scan presents no continuation token, so a foreign backend
//!             cannot silently drop the events in between
//! T15 LIVE-8  the union over subdivided windows is the true active-block set
//! T16 LIVE-8  an irreducible window is a loud error, never a truncation
//! T17 §12 B1  a storage proof survives transient error 42s (retry, not fail)
//! T18 §12 B1  an exhausted retry budget is UNAVAILABLE, not MISMATCH
//! T19 §12 B2  a proof whose global_roots.block_hash is not the block's is
//!             rejected as a hard error and never becomes a root
//!
//! The R legs are the mirror REPAIR path (docs/pre-submission-corrections.md
//! plan A): a hole below the frontier is found, patched and republished
//! without a full re-backfill.
//!
//! R1 repair  the seeker pass names every missing/undercounted block exactly
//! R2 repair  targeted re-ingest restores them, block for block
//! R3 repair  a backward re-cut rewrites the affected epoch and all above
//! R4 repair  a re-cut with nothing changed is refused, nothing written
//! R5 repair  verify-root MATCHes where it previously MISMATCHed

use e2e_tests::bins::{bin, ensure_built, pick_free_port, run_capture, spawn_with_logs, ChildGuard};
use e2e_tests::chain::{ActiveBlock, FixtureChain, FxEvent, ENC_NOTE_CREATED_SELECTOR};
use e2e_tests::fixture::load_devnet_fixture;
use e2e_tests::rpc_server::{FaultSpec, FixtureRpc};
use serde_json::Value;
use starknet_types_core::felt::Felt;
use std::path::Path;
use std::process::Command;
use strk20_feed::felt_hex;
use strk20_indexerd::db::Db;

const CHAIN_ID: &str = "SN_TEST";
const GENESIS_BLOCK: u64 = 10;
const EPOCH_SIZE: u64 = 16;

// Verified on-chain, docs/research/live/sepolia-abi-compat.md.
const SEPOLIA_POOL: &str =
    "0x0254a6b2997ef52e9f830ce1f543f6b29768295e8d17e2267d672c552cfe0d91";
const SEPOLIA_CHAIN_ID: &str = "SN_SEPOLIA";
const SEPOLIA_GENESIS: u64 = 8_271_125;
const SEPOLIA_CLASSES: [(u64, &str); 5] = [
    (8_271_125, "0x715b22abfb60815623f4127ba64bd2f93613d8a5c1e519841eaab444659d2af"),
    (8_271_130, "0x30b8c540cf04d8ef0f4db2a9098d9cc0e35e83af1cb3325f5a4f40144b4b30b"),
    (8_271_140, "0x1a78d2daee64d1da6e7903b32676c92fcc301d4c03f688cd64e731f46033d18"),
    (8_271_150, "0x67dddd89d80fedadc06b6f160798f94800a4a70164e5a24301cd0d6076b554d"),
    (8_271_160, "0x56ab118a8a6e38efc93ad758cefe909fee421fa931ce3cf72df624d345623b2"),
];

fn base_args(dir: &Path, primary: &str, fallback: &str, pool_hex: &str) -> Vec<String> {
    vec![
        "--db".into(),
        dir.join("strk20.db").display().to_string(),
        "--feed-dir".into(),
        dir.join("feed").display().to_string(),
        "--rpc-url".into(),
        primary.to_owned(),
        "--rpc-fallback".into(),
        fallback.to_owned(),
        "--pool".into(),
        pool_hex.to_owned(),
        "--chain-id".into(),
        CHAIN_ID.into(),
        "--genesis-block".into(),
        GENESIS_BLOCK.to_string(),
        "--epoch-size".into(),
        EPOCH_SIZE.to_string(),
        "--chunk-size".into(),
        "5".into(),
    ]
}

fn backfill(dir: &Path, primary: &str, fallback: &str, pool_hex: &str) -> (String, String, bool) {
    let mut cmd = Command::new(bin("strk20"));
    cmd.arg("backfill")
        .args(base_args(dir, primary, fallback, pool_hex));
    run_capture(cmd, false)
}

fn verify_root(dir: &Path, primary: &str, fallback: &str, pool_hex: &str) -> (String, String, bool) {
    let mut cmd = Command::new(bin("strk20"));
    cmd.arg("verify-root")
        .args(base_args(dir, primary, fallback, pool_hex));
    run_capture(cmd, false)
}

fn meta(dir: &Path, key: &str) -> Option<String> {
    let db = Db::open(&dir.join("strk20.db")).expect("open indexer db");
    db.meta_get(key).expect("meta read")
}

/// A pool-active block: one storage write plus one pool event, so the
/// production events-first scan finds it.
fn active_block(seed: u64) -> ActiveBlock {
    ActiveBlock {
        diffs: vec![(Felt::from(0x10_0000u64 + seed), Felt::from(seed + 1))],
        events: vec![FxEvent {
            keys: vec![
                Felt::from_hex(ENC_NOTE_CREATED_SELECTOR).unwrap(),
                Felt::from(0xe000u64 + seed),
            ],
            data: vec![Felt::from(seed)],
        }],
        deployed_class: None,
        replaced_class: None,
    }
}

// ------------------------------------------------------------------ T1

/// LIVE-1: lava routes the same request to archive or pruned backends
/// nondeterministically, so a "block N has been pruned" JSON-RPC error is a
/// PROVIDER-CAPABILITY error — retryable — not a semantic one. One such error
/// must never abort a backfill.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t1_backfill_survives_pruned_range_error() {
    ensure_built();
    let fixture = load_devnet_fixture();
    let pool_hex = felt_hex(&fixture.constants.contract_address);
    let rpc = FixtureRpc::with_faults(
        FixtureChain::build(&fixture),
        CHAIN_ID,
        FaultSpec {
            pruned_floor: Some(20),
            pruned_budget: 3,
            ..Default::default()
        },
    );
    let addr = rpc.serve().await;
    let url = format!("http://{addr}/");
    let dir = tempfile::tempdir().unwrap();

    let (stdout, stderr, ok) = backfill(dir.path(), &url, &url, &pool_hex);

    assert!(
        rpc.pruned_errors() > 0,
        "vacuous test: the fixture never served a pruned-history error"
    );
    assert!(
        ok,
        "LIVE-1: a pruned-history error (code -32603, \"has been pruned\") must be \
         retried with backoff, not treated as fatal.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(
        meta(dir.path(), "head_number").as_deref(),
        Some("46"),
        "backfill must reach head after the retry"
    );
    let db = Db::open(&dir.path().join("strk20.db")).unwrap();
    assert_eq!(
        db.epoch_rows().unwrap().len(),
        2,
        "both ready epochs must still be cut"
    );
}

// ------------------------------------------------------------------ T2

/// LIVE-3: HTTP 429 is throttling, not a transport failure. It must back off
/// in place; counting it toward the consecutive-failure budget flips a deep
/// backfill onto an endpoint that may not be able to serve the range at all.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t2_throttling_never_causes_endpoint_failover() {
    ensure_built();
    let fixture = load_devnet_fixture();
    let pool_hex = felt_hex(&fixture.constants.contract_address);
    // 6 throttled requests: one more than the 5-consecutive-failure failover
    // threshold, still inside the per-call retry budget.
    let primary = FixtureRpc::with_faults(
        FixtureChain::build(&fixture),
        CHAIN_ID,
        FaultSpec {
            throttle_first: 6,
            ..Default::default()
        },
    );
    let fallback = FixtureRpc::new(FixtureChain::build(&fixture), CHAIN_ID);
    let primary_addr = primary.serve().await;
    let fallback_addr = fallback.serve().await;
    let primary_url = format!("http://{primary_addr}/");
    let fallback_url = format!("http://{fallback_addr}/");
    let dir = tempfile::tempdir().unwrap();

    let (stdout, stderr, ok) = backfill(dir.path(), &primary_url, &fallback_url, &pool_hex);

    assert!(
        primary.throttled() >= 5,
        "vacuous test: the fixture throttled {} times, below the failover threshold",
        primary.throttled()
    );
    assert_eq!(
        fallback.request_count(),
        0,
        "LIVE-3: a 429 storm must back off in place; the run failed over to the \
         fallback endpoint instead ({} requests reached it)",
        fallback.request_count()
    );
    assert!(
        ok,
        "the run must still complete on the primary.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(meta(dir.path(), "head_number").as_deref(), Some("46"));
}

// ------------------------------------------------------------------ T3

/// LIVE-4: the storage-proof window is ~1024 blocks behind head while
/// l1_accepted lags ~5000, so verifying at min(l1_accepted, frontier) is by
/// construction outside the window. The verification block must be chosen
/// INSIDE the live window and at or below our frontier. Pool slots are
/// write-once, so a root match at block B subsumes every write below B;
/// finality is a separate concern already handled by the epoch floor.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t3_verify_root_uses_a_block_inside_the_proof_window() {
    ensure_built();
    let fixture = load_devnet_fixture();
    let pool_hex = felt_hex(&fixture.constants.contract_address);
    // provable = [43, 46]; l1_accepted (40) is outside, frontier (46) inside.
    let rpc = FixtureRpc::with_faults(
        FixtureChain::build(&fixture),
        CHAIN_ID,
        FaultSpec {
            proof_window: Some(3),
            ..Default::default()
        },
    );
    let addr = rpc.serve().await;
    let url = format!("http://{addr}/");
    let dir = tempfile::tempdir().unwrap();

    let (bo, be, ok) = backfill(dir.path(), &url, &url, &pool_hex);
    assert!(ok, "backfill failed\nstdout:\n{bo}\nstderr:\n{be}");

    let (stdout, stderr, ok) = verify_root(dir.path(), &url, &url, &pool_hex);
    assert!(
        ok,
        "LIVE-4: verify-root must probe INSIDE the live proof window (blocks 43..=46 \
         here) instead of at l1_accepted=40.\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        stdout.contains("verify-root OK"),
        "verify-root must report success: {stdout}"
    );
    let block = block_after(&stdout, "at block ")
        .unwrap_or_else(|| panic!("verify-root must name the verified block: {stdout}"));
    assert!(
        (43..=46).contains(&block),
        "verified block {block} is outside the provable window 43..=46"
    );
    assert_ne!(
        meta(dir.path(), "verify_root_failed").as_deref(),
        Some("1"),
        "a window miss must never latch verify_root_failed"
    );
}

// ------------------------------------------------------------------ T4

/// LIVE-4/6: an endpoint that serves no storage proofs at any height
/// (publicnode) is a CAPABILITY gap, not evidence about the mirror. The
/// result is UNAVAILABLE — distinct from MISMATCH — and must never set
/// verify_root_failed or degrade health.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t4_no_provable_block_reports_unavailable_not_failure() {
    ensure_built();
    let fixture = load_devnet_fixture();
    let pool_hex = felt_hex(&fixture.constants.contract_address);
    let rpc = FixtureRpc::with_faults(
        FixtureChain::build(&fixture),
        CHAIN_ID,
        FaultSpec {
            proofs_unsupported: true,
            ..Default::default()
        },
    );
    let addr = rpc.serve().await;
    let url = format!("http://{addr}/");
    let dir = tempfile::tempdir().unwrap();

    let (bo, be, ok) = backfill(dir.path(), &url, &url, &pool_hex);
    assert!(
        rpc.proofs_denied() > 0,
        "vacuous test: the fixture never denied a storage proof"
    );
    assert!(
        ok,
        "an endpoint without storage proofs must not break ingest\nstdout:\n{bo}\nstderr:\n{be}"
    );
    let db = Db::open(&dir.path().join("strk20.db")).unwrap();
    assert_eq!(db.epoch_rows().unwrap().len(), 2, "epochs must still be cut");
    assert_ne!(
        meta(dir.path(), "verify_root_failed").as_deref(),
        Some("1"),
        "LIVE-6: a capability gap must never latch verify_root_failed (health DEGRADED)"
    );
    assert_eq!(
        meta(dir.path(), "decode_state").as_deref(),
        Some("ok"),
        "LIVE-6: a capability gap must never degrade the mirror"
    );

    let (stdout, stderr, ok) = verify_root(dir.path(), &url, &url, &pool_hex);
    let out = format!("{stdout}\n{stderr}");
    assert!(
        out.contains("UNAVAILABLE"),
        "LIVE-4: with no provable block, verify-root must report UNAVAILABLE \
         (distinct from MISMATCH), not an RPC error.\n{out}"
    );
    assert!(
        !out.contains("MISMATCH"),
        "an unavailable proof must never be reported as a mismatch:\n{out}"
    );
    assert!(ok, "UNAVAILABLE is not a verification failure:\n{out}");
    assert_ne!(
        meta(dir.path(), "verify_root_failed").as_deref(),
        Some("1"),
        "verify-root UNAVAILABLE must not latch verify_root_failed"
    );
}

// ------------------------------------------------------------------ T5

/// Regression guard for T3/T4: a mirror that really is missing a write must
/// still MISMATCH once a provable block is found. Without this, "pick a
/// different block" and "call it unavailable" could both pass vacuously.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t5_real_divergence_still_mismatches_inside_the_window() {
    ensure_built();
    let fixture = load_devnet_fixture();
    let pool_hex = felt_hex(&fixture.constants.contract_address);
    // A silent write: the chain's storage root covers a slot no state update
    // ever exposes, so the mirror can never learn it.
    let rpc = FixtureRpc::with_faults(
        FixtureChain::build(&fixture),
        CHAIN_ID,
        FaultSpec {
            proof_window: Some(3),
            hidden_slot: Some((Felt::from(0xdead_beefu64), Felt::from(7u64))),
            ..Default::default()
        },
    );
    let addr = rpc.serve().await;
    let url = format!("http://{addr}/");
    let dir = tempfile::tempdir().unwrap();

    let _ = backfill(dir.path(), &url, &url, &pool_hex);

    let (stdout, stderr, ok) = verify_root(dir.path(), &url, &url, &pool_hex);
    let out = format!("{stdout}\n{stderr}");
    assert!(
        out.contains("VERIFY-ROOT MISMATCH"),
        "a mirror missing a chain write must be reported as VERIFY-ROOT MISMATCH \
         once a provable block is chosen:\n{out}"
    );
    assert!(
        !out.contains("too far in the past"),
        "the divergence must be found inside the proof window, not masked by a \
         window error:\n{out}"
    );
    assert!(!ok, "a mismatch must be a hard failure:\n{out}");
}

// ------------------------------------------------------------------ T6

/// LIVE-5: per-epoch anchors are always absent in production (an epoch's end
/// block is thousands of blocks old at cut time). The real artifact is an
/// append-only anchors log in the feed, captured opportunistically while a
/// block is still inside the proof window, and verifiable by any client
/// against its own mirror.
///
/// The proof window is deliberately NARROWER than the distance from head back
/// to any epoch end (epochs end at 15 and 31; head is 46): the old per-epoch
/// anchor path — the one measured firing 0 times in 515 mainnet epochs — cannot
/// contribute a single record here, so every published anchor must come from
/// the head-side capture this defect is about.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t6_anchors_log_is_canonical_stable_and_client_verifiable() {
    ensure_built();
    let fixture = load_devnet_fixture();
    let pool_hex = felt_hex(&fixture.constants.contract_address);
    let bob = fixture.constants.bob_address;
    const WINDOW: u64 = 3;
    let chain = FixtureChain::build(&fixture);
    let head = chain.head;
    let rpc = FixtureRpc::with_faults(
        chain.clone(),
        CHAIN_ID,
        FaultSpec {
            proof_window: Some(WINDOW),
            ..Default::default()
        },
    );
    let addr = rpc.serve().await;
    let url = format!("http://{addr}/");
    let dir = tempfile::tempdir().unwrap();

    let (bo, be, ok) = backfill(dir.path(), &url, &url, &pool_hex);
    assert!(ok, "backfill failed\nstdout:\n{bo}\nstderr:\n{be}");

    let path = dir.path().join("feed/anchors.ndjson");
    assert!(
        path.exists(),
        "LIVE-5: the feed must publish an append-only anchors log at \
         feed/anchors.ndjson (0 of 515 mainnet epochs ever carried a per-epoch anchor)"
    );
    let text = std::fs::read_to_string(&path).unwrap();
    assert!(
        text.ends_with('\n') && !text.ends_with("\n\n"),
        "anchors.ndjson must be NDJSON: one record per line, trailing newline, no blanks"
    );
    let records = parse_anchors(&text, &chain);
    assert!(
        !records.is_empty(),
        "no anchor was captured even though the frontier reached a provable block"
    );
    for block in &records {
        assert!(
            head - block <= WINDOW,
            "anchor at block {block} is {} blocks behind head {head}: outside the \
             {WINDOW}-block proof window, so it cannot have been captured live — \
             this log is the old per-epoch sidecar under a new name",
            head - block
        );
    }

    // Hash stability: an independent operator must produce the same bytes.
    let dir2 = tempfile::tempdir().unwrap();
    let (bo, be, ok) = backfill(dir2.path(), &url, &url, &pool_hex);
    assert!(ok, "second backfill failed\nstdout:\n{bo}\nstderr:\n{be}");
    let text2 = std::fs::read_to_string(dir2.path().join("feed/anchors.ndjson")).unwrap();
    assert_eq!(
        text, text2,
        "anchors.ndjson must be byte-identical across independent backfills"
    );

    // Client side: fold the local mirror to each anchor block, recompute the
    // storage root, compare.
    let feed = dir.path().join("feed").display().to_string();
    let db = dir.path().join("bob.db").display().to_string();
    sync_client(dir.path(), &feed, &bob, "0xb0b", &db);
    let (stdout, stderr, ok) = verify_anchors(&feed, &db);
    assert!(
        ok,
        "LIVE-5: the client must verify its mirror against the published anchors \
         (`strk20-sync verify-anchors --feed <feed> --db <db> --json`)\
         \nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert!(
        !stderr.contains("unexpected argument") && !stderr.contains("unrecognized"),
        "strk20-sync needs a verify-anchors subcommand:\n{stderr}"
    );
    let report: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("verify-anchors --json must print a report ({e}): {stdout}"));
    assert_eq!(report["all_ok"], Value::Bool(true), "report: {report}");
    assert_eq!(
        report["anchors_checked"].as_u64(),
        Some(records.len() as u64),
        "every published anchor must be checked: {report}"
    );
}

// ------------------------------------------------------------------ T7

/// Negative of T6: an anchor that disagrees with the folded mirror must be
/// rejected. anchors.ndjson is not content-addressed, so recomputation is the
/// only thing standing behind it — and BOTH fields a client can recompute
/// (`storage_root` from the slot set, `block_hash` from the stored header) must
/// be guarded. The chain here is given a pool-active block at head, so the
/// captured anchor lands on a block the client's mirror actually holds and the
/// block-hash comparison is live rather than dead code.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t7_client_rejects_an_anchor_that_disagrees_with_the_mirror() {
    ensure_built();
    let fixture = load_devnet_fixture();
    let pool_hex = felt_hex(&fixture.constants.contract_address);
    let bob = fixture.constants.bob_address;
    let mut chain = FixtureChain::build(&fixture);
    let head = chain.head;
    chain.add_note_block(
        head,
        Felt::from(0x4600_0000u64),
        Felt::from(0x77u64),
        FxEvent {
            keys: vec![
                Felt::from_hex(ENC_NOTE_CREATED_SELECTOR).unwrap(),
                Felt::from(0x4646u64),
            ],
            data: vec![Felt::from(0u64)],
        },
    );
    let rpc = FixtureRpc::with_faults(
        chain.clone(),
        CHAIN_ID,
        FaultSpec {
            proof_window: Some(3),
            ..Default::default()
        },
    );
    let addr = rpc.serve().await;
    let url = format!("http://{addr}/");
    let dir = tempfile::tempdir().unwrap();

    let (bo, be, ok) = backfill(dir.path(), &url, &url, &pool_hex);
    assert!(ok, "backfill failed\nstdout:\n{bo}\nstderr:\n{be}");
    let path = dir.path().join("feed/anchors.ndjson");
    assert!(path.exists(), "LIVE-5: feed/anchors.ndjson must be published");

    let feed = dir.path().join("feed").display().to_string();
    let db = dir.path().join("bob.db").display().to_string();
    sync_client(dir.path(), &feed, &bob, "0xb0b", &db);

    let original = std::fs::read_to_string(&path).unwrap();
    let anchored: Vec<u64> = original
        .lines()
        .map(|l| serde_json::from_str::<Value>(l).unwrap()["block"].as_u64().unwrap())
        .collect();
    assert!(
        anchored.contains(&head),
        "the fixture must anchor the pool-active head block {head} so the client's \
         mirror holds it; anchored blocks: {anchored:?}"
    );

    // Each field is forged on its own; both must be rejected on their own.
    for field in ["storage_root", "block_hash"] {
        let mut lines: Vec<String> = original.lines().map(str::to_owned).collect();
        let idx = lines
            .iter()
            .position(|l| serde_json::from_str::<Value>(l).unwrap()["block"] == head)
            .expect("a record for the head block");
        let mut record: Value = serde_json::from_str(&lines[idx]).expect("anchor line is JSON");
        let real = record[field].as_str().unwrap_or_default().to_owned();
        assert_ne!(real, "0x1", "tamper must actually change {field}");
        record[field] = Value::String("0x1".into());
        lines[idx] = serde_json::to_string(&record).unwrap();
        std::fs::write(&path, format!("{}\n", lines.join("\n"))).unwrap();

        let (stdout, stderr, ok) = verify_anchors(&feed, &db);
        let out = format!("{stdout}\n{stderr}").to_lowercase();
        assert!(
            !out.contains("unexpected argument") && !out.contains("unrecognized"),
            "strk20-sync needs a verify-anchors subcommand:\n{stderr}"
        );
        assert!(
            !ok,
            "a forged anchor {field} must be rejected\nstdout:\n{stdout}\nstderr:\n{stderr}"
        );
        assert!(
            out.contains("anchor") && out.contains("mismatch") && out.contains(field),
            "the rejection must name the forged field {field}:\n{stdout}\n{stderr}"
        );
    }

    // Control: restored bytes verify, so the rejections above are about the
    // tamper and not about the mirror being unable to check anything.
    std::fs::write(&path, &original).unwrap();
    let (stdout, stderr, ok) = verify_anchors(&feed, &db);
    assert!(
        ok,
        "the untampered log must verify\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

// ------------------------------------------------------------------ T8

/// Fix pack B: `--network sepolia` selects the whole verified Sepolia profile
/// — pool, genesis block, chain id and a decoder map covering all five
/// deployed classes — while every explicit override still wins.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t8_network_sepolia_selects_the_verified_profile() {
    ensure_built();
    let pool = Felt::from_hex(SEPOLIA_POOL).unwrap();
    let mut chain = FixtureChain::synthetic(pool, 8_271_200, 8_271_180);
    for (i, (block, class)) in SEPOLIA_CLASSES.iter().enumerate() {
        let mut blk = active_block(i as u64);
        let class = Felt::from_hex(class).unwrap();
        if i == 0 {
            blk.deployed_class = Some(class);
        } else {
            blk.replaced_class = Some(class);
        }
        chain.active.insert(*block, blk);
    }
    let rpc = FixtureRpc::new(chain, SEPOLIA_CHAIN_ID);
    let addr = rpc.serve().await;
    let url = format!("http://{addr}/");
    let dir = tempfile::tempdir().unwrap();

    // No --pool, --chain-id or --genesis-block: the profile must supply them.
    let mut cmd = Command::new(bin("strk20"));
    cmd.arg("backfill")
        .args(["--network", "sepolia"])
        .args(["--db", &dir.path().join("strk20.db").display().to_string()])
        .args(["--feed-dir", &dir.path().join("feed").display().to_string()])
        .args(["--rpc-url", &url])
        .args(["--rpc-fallback", &url])
        .args(["--epoch-size", "16"])
        .args(["--chunk-size", "5"]);
    let (stdout, stderr, ok) = run_capture(cmd, false);
    assert!(
        !stderr.contains("unexpected argument") && !stderr.contains("unrecognized"),
        "fix pack B: `strk20` needs a --network mainnet|sepolia flag:\n{stderr}"
    );
    assert!(
        ok,
        "--network sepolia backfill failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    assert_eq!(meta(dir.path(), "chain_id").as_deref(), Some(SEPOLIA_CHAIN_ID));
    assert_eq!(
        meta(dir.path(), "pool_address").as_deref(),
        Some(felt_hex(&pool).as_str()),
        "the Sepolia pool address must come from the profile"
    );
    assert_eq!(
        meta(dir.path(), "genesis_block").as_deref(),
        Some(SEPOLIA_GENESIS.to_string().as_str()),
        "the Sepolia deploy block (CONTRACT_NOT_FOUND at 8271124) must come from the profile"
    );

    let db = Db::open(&dir.path().join("strk20.db")).unwrap();
    assert_eq!(
        db.blocks_in_range(SEPOLIA_GENESIS, 8_271_200).unwrap().len(),
        SEPOLIA_CLASSES.len(),
        "every pool-active Sepolia block must be ingested from the profile genesis"
    );
    for (block, class) in SEPOLIA_CLASSES {
        assert_eq!(
            db.class_as_of(block).unwrap().map(|c| felt_hex(&c)),
            Some(felt_hex(&Felt::from_hex(class).unwrap())),
            "class history must record {class} at block {block}"
        );
    }
    assert_eq!(
        meta(dir.path(), "decode_state").as_deref(),
        Some("ok"),
        "all five Sepolia classes are field-level compatible: none may degrade decoding"
    );
    assert!(
        !stderr.contains("UNKNOWN pool class hash"),
        "no Sepolia class may be unknown to the decoder map:\n{stderr}"
    );

    // Explicit flags still override the profile.
    let dir2 = tempfile::tempdir().unwrap();
    let mut cmd = Command::new(bin("strk20"));
    cmd.arg("backfill")
        .args(["--network", "sepolia"])
        .args(["--genesis-block", "8271130"])
        .args(["--db", &dir2.path().join("strk20.db").display().to_string()])
        .args(["--feed-dir", &dir2.path().join("feed").display().to_string()])
        .args(["--rpc-url", &url])
        .args(["--rpc-fallback", &url])
        .args(["--epoch-size", "16"])
        .args(["--chunk-size", "5"]);
    let (stdout, stderr, ok) = run_capture(cmd, false);
    assert!(
        ok,
        "override backfill failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(
        meta(dir2.path(), "genesis_block").as_deref(),
        Some("8271130"),
        "an explicit --genesis-block must override the network profile"
    );
}

// ------------------------------------------------------------------ T9

/// Fix pack B: chain id is stamped end to end. A client told which network it
/// is on must refuse a feed built for a different chain — before it applies
/// a single epoch.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t9_client_refuses_a_feed_from_another_chain() {
    ensure_built();
    let fixture = load_devnet_fixture();
    let pool_hex = felt_hex(&fixture.constants.contract_address);
    let bob = fixture.constants.bob_address;
    let rpc = FixtureRpc::new(FixtureChain::build(&fixture), CHAIN_ID);
    let addr = rpc.serve().await;
    let url = format!("http://{addr}/");
    let dir = tempfile::tempdir().unwrap();

    let (bo, be, ok) = backfill(dir.path(), &url, &url, &pool_hex);
    assert!(ok, "backfill failed\nstdout:\n{bo}\nstderr:\n{be}");
    let feed = dir.path().join("feed").display().to_string();

    let manifest: Value =
        serde_json::from_slice(&std::fs::read(dir.path().join("feed/manifest.json")).unwrap())
            .unwrap();
    assert_eq!(
        manifest["chain_id"], CHAIN_ID,
        "the manifest must carry the chain id"
    );

    // control: without an expectation the same feed syncs fine
    let ok_db = dir.path().join("control.db").display().to_string();
    let (stdout, stderr, ok) = try_sync(dir.path(), &feed, &bob, "0xb0b", &ok_db, &[]);
    assert!(
        ok,
        "control sync must succeed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    // expectation mismatch: SN_MAIN client, SN_TEST feed
    let bad_db = dir.path().join("mismatch.db").display().to_string();
    let (stdout, stderr, ok) = try_sync(
        dir.path(),
        &feed,
        &bob,
        "0xb0b",
        &bad_db,
        &["--network", "mainnet"],
    );
    assert!(
        !stderr.contains("unexpected argument") && !stderr.contains("unrecognized"),
        "fix pack B: `strk20-sync sync` needs a --network expectation flag:\n{stderr}"
    );
    assert!(
        !ok,
        "the client must refuse a feed whose chain id is not the expected one\
         \nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let out = format!("{stdout}\n{stderr}");
    assert!(
        out.to_lowercase().contains("chain") && out.contains("SN_MAIN") && out.contains(CHAIN_ID),
        "the refusal must name both chain ids:\n{out}"
    );
    // "Before it applies a single epoch" is the actual claim: nothing from the
    // foreign feed may have landed in the mirror.
    let rejected = strk20_client::store::FeedStore::open(Path::new(&bad_db)).unwrap();
    assert_eq!(
        rejected.meta_get("last_epoch_applied").unwrap(),
        None,
        "the refusal must happen before any epoch is applied"
    );
    assert_eq!(rejected.meta_get("pool").unwrap(), None);

    // The pin also holds without --network: a mirror built from one chain must
    // refuse a feed for another even when the operator says nothing (F8).
    let other = FixtureRpc::new(FixtureChain::build(&fixture), "SN_OTHER");
    let other_addr = other.serve().await;
    let other_url = format!("http://{other_addr}/");
    let dir2 = tempfile::tempdir().unwrap();
    let (bo, be, ok) = {
        let mut args = base_args(dir2.path(), &other_url, &other_url, &pool_hex);
        let at = args.iter().position(|a| a == "--chain-id").unwrap();
        args[at + 1] = "SN_OTHER".into();
        let mut cmd = Command::new(bin("strk20"));
        cmd.arg("backfill").args(args);
        run_capture(cmd, false)
    };
    assert!(ok, "SN_OTHER backfill failed\nstdout:\n{bo}\nstderr:\n{be}");
    let other_feed = dir2.path().join("feed").display().to_string();
    let (stdout, stderr, ok) = try_sync(dir.path(), &other_feed, &bob, "0xb0b", &ok_db, &[]);
    assert!(
        !ok,
        "a mirror pinned to {CHAIN_ID} must refuse an SN_OTHER feed with no flag\
         \nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let out = format!("{stdout}\n{stderr}");
    assert!(
        out.to_lowercase().contains("chain id") && out.contains("SN_OTHER"),
        "the refusal must be about the chain id, not an incidental hash divergence:\n{out}"
    );
}

// ------------------------------------------------------------------ T10

/// LIVE-2: a multi-hour scan logged nothing between start and the final
/// summary. The scan phase must report progress periodically — cursor,
/// blocks ingested, events, and which endpoint is serving — on a TIME-based
/// cadence, not per page. Two legs over the identical scan pin both halves:
/// `--progress-secs 0` must report often, and a long interval must suppress
/// all but the first report. A build that ignored the knob and logged every
/// page would pass the first leg and fail the second.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t10_scan_phase_reports_progress() {
    ensure_built();
    let pool = Felt::from(0x9001u64);
    let mut chain = FixtureChain::synthetic(pool, 200, 190);
    for i in 0..60u64 {
        chain.active.insert(100 + i, active_block(i));
    }
    let rpc = FixtureRpc::new(chain, CHAIN_ID);
    let addr = rpc.serve().await;
    let url = format!("http://{addr}/");

    let run_with = |progress_secs: &str| -> (String, String, bool) {
        let dir = tempfile::tempdir().unwrap();
        let mut cmd = Command::new(bin("strk20"));
        cmd.arg("backfill")
            .args(["--db", &dir.path().join("strk20.db").display().to_string()])
            .args(["--feed-dir", &dir.path().join("feed").display().to_string()])
            .args(["--rpc-url", &url])
            .args(["--rpc-fallback", &url])
            .args(["--pool", &felt_hex(&pool)])
            .args(["--chain-id", CHAIN_ID])
            .args(["--genesis-block", "100"])
            .args(["--epoch-size", "16"])
            .args(["--chunk-size", "5"])
            .args(["--progress-secs", progress_secs]);
        let out = run_capture(cmd, false);
        drop(dir);
        out
    };

    // Leg 1: 0 = report every scan page.
    let (stdout, stderr, ok) = run_with("0");
    assert!(
        !stderr.contains("unexpected argument") && !stderr.contains("unrecognized"),
        "LIVE-2: `strk20` needs a --progress-secs knob for the scan progress cadence:\n{stderr}"
    );
    assert!(ok, "backfill failed\nstdout:\n{stdout}\nstderr:\n{stderr}");

    let lines = progress_lines(&stderr);
    assert!(
        lines.len() >= 2,
        "LIVE-2: the scan phase must emit periodic progress lines; found {} in:\n{stderr}",
        lines.len()
    );
    for line in &lines {
        for field in ["cursor=", "blocks_ingested=", "events=", "endpoint="] {
            assert!(
                line.contains(field),
                "a progress line must carry {field}: {line}"
            );
        }
    }
    assert!(
        lines.iter().any(|l| l.contains(&url)),
        "progress must name the active endpoint {url}:\n{stderr}"
    );
    let frequent = lines.len();

    // Leg 2: the same scan under an interval it can never reach.
    let (stdout, stderr, ok) = run_with("3600");
    assert!(ok, "backfill failed\nstdout:\n{stdout}\nstderr:\n{stderr}");
    let throttled = progress_lines(&stderr);
    assert!(
        throttled.len() <= 1,
        "LIVE-2 asks for a TIME-based cadence: with --progress-secs 3600 a scan \
         that finishes in milliseconds must emit at most one progress line, got \
         {} (the 0-second leg emitted {frequent}):\n{stderr}",
        throttled.len()
    );
}

fn progress_lines(stderr: &str) -> Vec<&str> {
    stderr
        .lines()
        .filter(|l| l.to_lowercase().contains("scan progress"))
        .collect()
}

// ------------------------------------------------------------------ T11

/// A pool write that rides a block with NO pool event, above l1_accepted.
///
/// The scenario is unchanged; what recovers it is not. This test used to
/// require that the write produce a `VERIFY-ROOT MISMATCH` first, because the
/// §5.6 recovery rescan was the only code that ever asked the chain about a
/// block `getEvents` cannot name — and the point being guarded was that the
/// rescan's range moved with LIVE-4's verification block (`min(frontier,
/// rpc_head)`, not `min(l1_accepted, frontier)`), since a rescan still capped
/// at l1_accepted could not reach this block and the mismatch would reproduce
/// on every retry, latching DEGRADED forever.
///
/// `run_cycle` now sweeps state diffs across the blocks a cycle moves past
/// (`TAIL_STATE_DIFF_SPAN`), so on a chain this short the write is ingested
/// before verify-root ever runs and the mismatch does not happen. Requiring
/// one would now assert the bug rather than the fix. The consequences are what
/// this test pins, and all three still hold whichever layer got there first:
/// the block is in the mirror, both ready epochs are cut, and health is not
/// latched. The §5.6 rescan remains the backstop for a divergence older than
/// the sweep's span, where it is still the only path.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t11_a_divergence_above_l1_accepted_is_recovered_not_latched() {
    ensure_built();
    let fixture = load_devnet_fixture();
    let pool_hex = felt_hex(&fixture.constants.contract_address);
    let mut chain = FixtureChain::build(&fixture);
    // Silent write at 45: l1_accepted is 40, head is 46. The block emits NO
    // pool event, so the events-first scan cannot see it; only the per-block
    // state-update rescan can.
    let silent = 45u64;
    assert!(silent > chain.l1_accepted && silent <= chain.head);
    chain.active.insert(
        silent,
        ActiveBlock {
            diffs: vec![(Felt::from(0x5100_0000u64), Felt::from(0x99u64))],
            events: vec![],
            deployed_class: None,
            replaced_class: None,
        },
    );
    let rpc = FixtureRpc::new(chain, CHAIN_ID);
    let addr = rpc.serve().await;
    let url = format!("http://{addr}/");
    let dir = tempfile::tempdir().unwrap();

    let (stdout, stderr, ok) = backfill(dir.path(), &url, &url, &pool_hex);
    assert!(ok, "backfill failed\nstdout:\n{stdout}\nstderr:\n{stderr}");

    let db = Db::open(&dir.path().join("strk20.db")).unwrap();
    assert!(
        db.blocks_in_range(silent, silent).unwrap().len() == 1,
        "block {silent} must reach the mirror: it writes pool storage, emits no pool \
         event, and sits above l1_accepted but at or below the frontier — which is \
         where verify-root checks. Either the tail state-diff sweep ingests it or the \
         §5.6 rescan recovers it; neither doing so is a permanent root divergence.\
         \nstderr:\n{stderr}"
    );
    assert_eq!(
        db.epoch_rows().unwrap().len(),
        2,
        "after recovery both ready epochs must be cut; a rescan that cannot reach \
         the mismatching block stops epoch publication forever\nstderr:\n{stderr}"
    );
    assert_ne!(
        meta(dir.path(), "verify_root_failed").as_deref(),
        Some("1"),
        "a recovered divergence must not leave /health latched DEGRADED"
    );
}

// ------------------------------------------------------------------ T12

/// The documented limit of the LIVE-4 fix, pinned so it stays a decision rather
/// than a latent surprise: the verification target can never rise above the
/// frontier (the chain root above it covers writes we have not ingested), so
/// while `head - frontier` exceeds the proof window — the whole deep-backfill
/// phase — verify-root can only answer UNAVAILABLE. That is a provider/liveness
/// statement and must stay distinct from a mismatch.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t12_a_frontier_far_below_head_is_unavailable_not_a_failure() {
    ensure_built();
    let fixture = load_devnet_fixture();
    let pool_hex = felt_hex(&fixture.constants.contract_address);
    let rpc = FixtureRpc::with_faults(
        FixtureChain::build(&fixture),
        CHAIN_ID,
        FaultSpec {
            proof_window: Some(3),
            ..Default::default()
        },
    );
    let addr = rpc.serve().await;
    let url = format!("http://{addr}/");
    let dir = tempfile::tempdir().unwrap();

    let (bo, be, ok) = backfill(dir.path(), &url, &url, &pool_hex);
    assert!(ok, "backfill failed\nstdout:\n{bo}\nstderr:\n{be}");

    // The chain runs far ahead of what we mirrored, as during a real backfill.
    rpc.chain.write().unwrap().head = 200;

    let (stdout, stderr, ok) = verify_root(dir.path(), &url, &url, &pool_hex);
    let out = format!("{stdout}\n{stderr}");
    assert!(
        out.contains("UNAVAILABLE"),
        "with the frontier 154 blocks below head and a 3-block proof window, no \
         block is both provable and mirrored: the answer is UNAVAILABLE.\n{out}"
    );
    assert!(
        !out.contains("MISMATCH"),
        "a liveness gap must never be reported as a divergence:\n{out}"
    );
    assert!(ok, "UNAVAILABLE is not a verification failure:\n{out}");
    assert_ne!(
        meta(dir.path(), "verify_root_failed").as_deref(),
        Some("1")
    );
}

// ------------------------------------------------------------------ T13

/// A verification command that exits green when it verified nothing is the
/// failure mode the whole anchors mechanism exists to avoid. Neither an
/// unreachable feed nor a feed that publishes no anchors may report success.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t13_verify_anchors_never_reports_success_without_checking_anything() {
    ensure_built();
    let dir = tempfile::tempdir().unwrap();
    let db = dir.path().join("empty.db").display().to_string();

    // Unreachable feed: a transport failure must not be swallowed into "no
    // anchors published, all_ok".
    let (stdout, stderr, ok) = verify_anchors("http://127.0.0.1:1/nope", &db);
    assert!(
        !ok,
        "verify-anchors reported success against a feed it could not reach\
         \nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    // Reachable feed that publishes no anchors: honest, but still not a
    // verification.
    let empty_feed = dir.path().join("feed");
    std::fs::create_dir_all(&empty_feed).unwrap();
    let (stdout, stderr, ok) =
        verify_anchors(&empty_feed.display().to_string(), &db);
    assert!(
        !ok,
        "verify-anchors reported success against a feed with no anchors log\
         \nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let report: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("--json must still print a report ({e}): {stdout}"));
    assert_eq!(report["anchors_checked"].as_u64(), Some(0));
    assert_eq!(report["all_ok"], Value::Bool(false));
    assert!(
        report["status"].as_str().is_some_and(|s| s != "verified"),
        "the report must say WHY nothing was checked: {report}"
    );
}

// ------------------------------------------------------- LIVE-8 harness

/// `count` consecutive pool-active blocks from `from`, one storage write and
/// one pool event each. Dense on purpose: with a small `--chunk-size` the
/// whole scan range cannot be answered in one page, so an implementation
/// either subdivides or pages — and paging is what LIVE-8 forbids.
fn dense_chain(pool: Felt, from: u64, count: u64, head: u64) -> FixtureChain {
    let mut chain = FixtureChain::synthetic(pool, head, head.saturating_sub(10));
    for i in 0..count {
        chain.active.insert(from + i, active_block(i));
    }
    chain
}

/// Backfill a synthetic (non-devnet) chain: explicit pool, genesis and page
/// size, everything else as in `base_args`.
fn backfill_synthetic(
    dir: &Path,
    url: &str,
    pool: &Felt,
    genesis: u64,
    chunk: &str,
) -> (String, String, bool) {
    let mut cmd = Command::new(bin("strk20"));
    cmd.arg("backfill")
        .args(["--db", &dir.join("strk20.db").display().to_string()])
        .args(["--feed-dir", &dir.join("feed").display().to_string()])
        .args(["--rpc-url", url])
        .args(["--rpc-fallback", url])
        .args(["--pool", &felt_hex(pool)])
        .args(["--chain-id", CHAIN_ID])
        .args(["--genesis-block", &genesis.to_string()])
        .args(["--epoch-size", "16"])
        .args(["--chunk-size", chunk]);
    run_capture(cmd, false)
}

/// One raw `starknet_getEvents` call against the fixture, returning the whole
/// JSON-RPC response. Used to prove a fault mode is armed rather than assuming
/// it (the scanner self-test pattern of session 5).
async fn raw_get_events(
    url: &str,
    pool: &Felt,
    from: u64,
    to: u64,
    chunk: u64,
    token: Option<&str>,
) -> Value {
    let mut filter = serde_json::json!({
        "from_block": {"block_number": from},
        "to_block": {"block_number": to},
        "address": felt_hex(pool),
        "chunk_size": chunk,
    });
    if let Some(t) = token {
        filter["continuation_token"] = Value::String(t.to_owned());
    }
    reqwest::Client::new()
        .post(url)
        .json(&serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "method": "starknet_getEvents",
            "params": [filter],
        }))
        .send()
        .await
        .expect("fixture reachable")
        .json()
        .await
        .expect("fixture answers JSON")
}

/// Block numbers of the events in a getEvents result, in order.
fn page_blocks(result: &Value) -> Vec<u64> {
    result["result"]["events"]
        .as_array()
        .unwrap_or_else(|| panic!("no events array in {result}"))
        .iter()
        .map(|e| e["block_number"].as_u64().expect("event block_number"))
        .collect()
}

// ------------------------------------------------------------------ T14

/// LIVE-8, the critical one: a `getEvents` continuation token is NODE-LOCAL
/// state. `rpc.starknet.lava.build` is an aggregator, so the next request goes
/// to a different backend, which does not reject the token — it resumes from
/// somewhere else and the events in between vanish with no error. Measured on
/// the same range and endpoint: 13 pages found 2,628 blocks, 62 pages found
/// 2,608. A full mainnet backfill lost 139 blocks and 489 events this way, and
/// verify-root reported a genuine root mismatch because of it.
///
/// The property is therefore not "handle tokens carefully" but **never present
/// one**: every window must be answered in a single page with no continuation
/// token, and the scan must subdivide until that holds. A single response
/// carries no cross-request state, so it is sound under any routing.
///
/// The fixture corrupts any token presented to it, so an implementation that
/// pages loses blocks here. The zero-token assertion is what stops the leg
/// passing by luck (a corruption that happened to skip nothing).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t14_the_scan_never_presents_a_continuation_token() {
    ensure_built();
    let pool = Felt::from(0x9101u64);
    const FROM: u64 = 100;
    const COUNT: u64 = 40;
    let chain = dense_chain(pool, FROM, COUNT, 200);
    let active: Vec<u64> = chain.active.keys().copied().collect();
    let rpc = FixtureRpc::with_faults(
        chain,
        CHAIN_ID,
        FaultSpec {
            foreign_token: true,
            // publicnode's measured posture (LIVE-6): no storage proofs at any
            // height. verify-root is therefore UNAVAILABLE and its rescan
            // backstop cannot run, so the scan is on its own — which is the
            // production situation LIVE-8 was found in, and the reason the
            // loss went unnoticed for a whole mainnet backfill.
            proofs_unsupported: true,
            ..Default::default()
        },
    );
    let addr = rpc.serve().await;
    let url = format!("http://{addr}/");
    let dir = tempfile::tempdir().unwrap();

    let (stdout, stderr, ok) = backfill_synthetic(dir.path(), &url, &pool, FROM, "3");
    assert!(ok, "backfill failed\nstdout:\n{stdout}\nstderr:\n{stderr}");

    let db = Db::open(&dir.path().join("strk20.db")).unwrap();
    let ingested: Vec<u64> = db
        .blocks_in_range(FROM, FROM + COUNT - 1)
        .unwrap()
        .iter()
        .map(|b| b.number)
        .collect();
    assert_eq!(
        ingested, active,
        "LIVE-8: the mirror is missing pool-active blocks. Chain has {} active blocks, \
         the mirror has {}. This is exactly the 139-block loss measured on mainnet.",
        active.len(),
        ingested.len()
    );
    for n in &active {
        assert_eq!(
            db.events_of_block(*n).unwrap().len(),
            1,
            "block {n} lost its pool event"
        );
    }

    // ...and completeness must not be luck. The only sound way to get it from
    // an aggregating endpoint is never to present a token at all.
    assert_eq!(
        rpc.tokens_presented(),
        0,
        "LIVE-8: the indexer presented {} continuation token(s) back to the endpoint. \
         A token is node-local state; the aggregator's next backend resumes from \
         somewhere else and drops the events in between WITHOUT an error. The scan must \
         subdivide the block range until every window is answered in one page instead.",
        rpc.tokens_presented()
    );

    // The fault is armed — proven, not assumed. Presenting a token to this
    // endpoint yields a plausible page from the WRONG offset and no error, so
    // the assertions above are about the indexer's discipline and not about a
    // fixture that quietly behaves.
    let first = raw_get_events(&url, &pool, FROM, FROM + COUNT - 1, 3, None).await;
    let token = first["result"]["continuation_token"]
        .as_str()
        .expect("the fixture must page a 40-event range at chunk 3")
        .to_owned();
    let honest_next = page_blocks(&first).last().copied().unwrap() + 1;
    let second = raw_get_events(&url, &pool, FROM, FROM + COUNT - 1, 3, Some(&token)).await;
    assert!(
        second.get("error").filter(|e| !e.is_null()).is_none(),
        "the foreign backend must answer silently, never with an error: {second}"
    );
    let got = page_blocks(&second);
    assert!(
        got.first().copied() != Some(honest_next),
        "fixture self-test: presenting a token must resume from the WRONG offset \
         (honest next block {honest_next}, got {got:?})"
    );
    assert!(
        rpc.foreign_pages() >= 1,
        "fixture self-test: the foreign-token fault never fired"
    );
}

// ------------------------------------------------------------------ T15

/// LIVE-8, the positive half: the union of single-page windows is the WHOLE
/// truth. The range here cannot be answered in one page at `--chunk-size 2`
/// (60 events across 40 blocks), so a correct scan subdivides and unions; the
/// result must equal the chain's active-block set exactly, block for block and
/// event for event, with nothing dropped and nothing duplicated. Both halves
/// matter: lava at chunk=100 returned MORE events than the truth as well as
/// fewer.
///
/// Vacuity guards: the fixture is asserted to be unanswerable in one page, and
/// the endpoint is asserted to have been asked more than one window.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t15_the_union_over_subdivided_windows_is_the_true_active_set() {
    ensure_built();
    let pool = Felt::from(0x9201u64);
    const FROM: u64 = 100;
    const COUNT: u64 = 40;
    const CHUNK: u64 = 2;
    let mut chain = dense_chain(pool, FROM, COUNT, 200);
    // Uneven density: every third block carries a second event, so a scan that
    // subdivides on block COUNT rather than on what the endpoint answered gets
    // it wrong.
    for i in (0..COUNT).step_by(3) {
        let n = FROM + i;
        let blk = chain.active.get_mut(&n).unwrap();
        blk.events.push(FxEvent {
            keys: vec![
                Felt::from_hex(ENC_NOTE_CREATED_SELECTOR).unwrap(),
                Felt::from(0xf000u64 + i),
            ],
            data: vec![Felt::from(i)],
        });
        blk.diffs.push((Felt::from(0x20_0000u64 + i), Felt::from(i + 1)));
    }
    let expected: Vec<(u64, usize)> = chain
        .active
        .iter()
        .map(|(n, b)| (*n, b.events.len()))
        .collect();
    let total_events: usize = expected.iter().map(|(_, c)| c).sum();
    assert!(
        total_events as u64 > CHUNK,
        "fixture precondition: {total_events} events at chunk {CHUNK} must not fit in \
         one page, or subdivision is never exercised"
    );

    let rpc = FixtureRpc::new(chain, CHAIN_ID);
    let addr = rpc.serve().await;
    let url = format!("http://{addr}/");
    let dir = tempfile::tempdir().unwrap();

    let (stdout, stderr, ok) =
        backfill_synthetic(dir.path(), &url, &pool, FROM, &CHUNK.to_string());
    assert!(ok, "backfill failed\nstdout:\n{stdout}\nstderr:\n{stderr}");

    let db = Db::open(&dir.path().join("strk20.db")).unwrap();
    let got: Vec<(u64, usize)> = db
        .blocks_in_range(FROM, FROM + COUNT - 1)
        .unwrap()
        .iter()
        .map(|b| (b.number, db.events_of_block(b.number).unwrap().len()))
        .collect();
    assert_eq!(
        got, expected,
        "LIVE-8: the union over the scan's windows must equal the chain's active-block \
         set exactly — no block dropped (mainnet lost 139) and no event duplicated \
         (lava at chunk=100 returned MORE than the truth)"
    );

    assert_eq!(
        rpc.tokens_presented(),
        0,
        "the union must be built from single-page answers only"
    );
    let windows = rpc.event_windows();
    let full = (FROM, FROM + COUNT - 1);
    assert!(
        windows.len() > 1 && windows.iter().any(|w| *w != full),
        "vacuity guard: the endpoint was asked {} window(s) {:?}, none of them narrower \
         than the whole range. The range holds {total_events} events at chunk {CHUNK}, so \
         one window cannot answer it — together with the zero-token assertion above, that \
         means the scan must have SUBDIVIDED to have seen everything it stored.",
        windows.len(),
        windows
    );
}

// ------------------------------------------------------------------ T16

/// LIVE-8's boundary: a window that STILL returns a continuation token at
/// single-block granularity cannot be subdivided further. There is exactly one
/// honest answer — a loud, hard error naming the block. Silently keeping the
/// first page would be the original defect in miniature: a truncated block,
/// published as if complete.
///
/// The fixture caps pages at 2 events (providers really do cap `chunk_size`;
/// lava's maximum is 1000) and gives one block 4 events, so no request the
/// indexer can construct answers that block in one page.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t16_an_irreducible_window_is_a_loud_error_not_a_truncation() {
    ensure_built();
    let pool = Felt::from(0x9301u64);
    const FROM: u64 = 100;
    const FAT_BLOCK: u64 = 105;
    const FAT_EVENTS: usize = 4;
    let mut chain = dense_chain(pool, FROM, 10, 200);
    let blk = chain.active.get_mut(&FAT_BLOCK).unwrap();
    for extra in 0..(FAT_EVENTS - 1) as u64 {
        blk.events.push(FxEvent {
            keys: vec![
                Felt::from_hex(ENC_NOTE_CREATED_SELECTOR).unwrap(),
                Felt::from(0xfa70u64 + extra),
            ],
            data: vec![Felt::from(extra)],
        });
    }
    assert_eq!(chain.active[&FAT_BLOCK].events.len(), FAT_EVENTS);

    let rpc = FixtureRpc::with_faults(
        chain,
        CHAIN_ID,
        FaultSpec {
            // Below the fat block's event count: irreducible by construction.
            max_page: Some(2),
            ..Default::default()
        },
    );
    let addr = rpc.serve().await;
    let url = format!("http://{addr}/");
    let dir = tempfile::tempdir().unwrap();

    let (stdout, stderr, ok) = backfill_synthetic(dir.path(), &url, &pool, FROM, "1000");
    let out = format!("{stdout}\n{stderr}");
    assert!(
        !ok,
        "LIVE-8: block {FAT_BLOCK} holds {FAT_EVENTS} events and this endpoint will not \
         return more than 2 per page, so no single-page window covers it. Following the \
         token is unsound and truncating is silent data loss — the run must FAIL \
         loudly.\n{out}"
    );
    assert!(
        out.contains(&format!("block {FAT_BLOCK}")),
        "the error must name the block that could not be answered ({FAT_BLOCK}), and name \
         it as a block: a bare \"{FAT_BLOCK}\" could be a timing or another number that \
         happened to appear:\n{out}"
    );
    let lower = out.to_lowercase();
    assert!(
        ["continuation", "irreducible", "single page", "one page"]
            .iter()
            .any(|w| lower.contains(w)),
        "the error must say what happened — a window that cannot be answered in one page \
         even at single-block granularity — in words an operator can act on:\n{out}"
    );

    // Whatever was written must never be a PARTIAL view of the fat block: a
    // mirror that stores 2 of 4 events and moves on is the defect, dressed up.
    let db = Db::open(&dir.path().join("strk20.db")).unwrap();
    let stored = db.events_of_block(FAT_BLOCK).unwrap().len();
    assert!(
        stored == 0 || stored == FAT_EVENTS,
        "block {FAT_BLOCK} was stored with {stored} of {FAT_EVENTS} events: a silent \
         truncation is exactly what this leg forbids"
    );
}

// ------------------------------------------------------------------ T17

/// §12 B1: `getStorageProof` error 42 says "the backend that answered this
/// call has no archive trie", not "this block has no proof". Measured against
/// mainnet, proofs come back for ANY block on retry — 2 successes in 10
/// attempts at 2.89M blocks behind head, 2 in 4 at 5.15M behind — and the
/// ~1024-block window this project once recorded was a bisection over a
/// nondeterministic predicate (proof-window.md §3).
///
/// So a bounded retry against the SAME endpoint must recover, and verify-root
/// must report MATCH. Failing over instead is forbidden by LIVE-6: publicnode
/// implements no proofs at any height, so a failover guarantees a false alarm.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t17_a_storage_proof_survives_transient_error_42s() {
    ensure_built();
    let fixture = load_devnet_fixture();
    let pool_hex = felt_hex(&fixture.constants.contract_address);
    let chain = FixtureChain::build(&fixture);
    const FLAKY: usize = 3;
    let rpc = FixtureRpc::with_faults(
        chain.clone(),
        CHAIN_ID,
        FaultSpec {
            proof_flaky_attempts: FLAKY,
            ..Default::default()
        },
    );
    let addr = rpc.serve().await;
    let url = format!("http://{addr}/");
    let dir = tempfile::tempdir().unwrap();

    let (bo, be, ok) = backfill(dir.path(), &url, &url, &pool_hex);
    assert!(ok, "backfill failed\nstdout:\n{bo}\nstderr:\n{be}");

    // A fresh budget, so the retries counted below are this command's.
    rpc.reset_proof_attempts();
    let denied_before = rpc.proofs_denied();
    let (stdout, stderr, ok) = verify_root(dir.path(), &url, &url, &pool_hex);
    let out = format!("{stdout}\n{stderr}");
    let denied = rpc.proofs_denied() - denied_before;

    assert!(
        ok && stdout.contains("verify-root OK"),
        "§12 B1: this endpoint refuses the first {FLAKY} proof attempts at a block and \
         serves the proof afterwards — exactly what lava does. A bounded retry on error \
         42 must reach it.\n{out}"
    );
    assert!(
        (FLAKY..=16).contains(&denied),
        "vacuity guard: the fixture denied {denied} proof(s) during verify-root. Below \
         {FLAKY} the proof was never actually retried past a refusal; far above it the \
         retry is not bounded."
    );
    let block = block_after(&stdout, "at block ")
        .unwrap_or_else(|| panic!("verify-root must name the verified block: {stdout}"));
    let want = felt_hex(&strk20_feed::mpt::storage_root(&chain.state_at(block)));
    assert!(
        stdout.contains(&want),
        "the proof that was finally served must be the chain's root at block {block} \
         ({want}):\n{stdout}"
    );
    assert_ne!(
        meta(dir.path(), "verify_root_failed").as_deref(),
        Some("1"),
        "a proof that succeeded on retry must not leave health latched DEGRADED"
    );
}

// ------------------------------------------------------------------ T18

/// §12 B1's other end, and the half §11.4 contributed that survives the
/// retraction: when the retry budget IS exhausted, the answer is UNAVAILABLE —
/// a statement about the provider — and never MISMATCH. Conflating the two is
/// what made a capability-poor endpoint look like mirror corruption (LIVE-6).
///
/// The retry must also be bounded: an endpoint that refuses forever must not
/// spin.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t18_an_exhausted_proof_retry_budget_is_unavailable_not_mismatch() {
    ensure_built();
    let fixture = load_devnet_fixture();
    let pool_hex = felt_hex(&fixture.constants.contract_address);
    let rpc = FixtureRpc::with_faults(
        FixtureChain::build(&fixture),
        CHAIN_ID,
        // Refuses far beyond any sane budget, but never says "unsupported":
        // the retry has to give up on its own.
        FaultSpec {
            proof_flaky_attempts: 10_000,
            ..Default::default()
        },
    );
    let addr = rpc.serve().await;
    let url = format!("http://{addr}/");
    let dir = tempfile::tempdir().unwrap();

    let (bo, be, ok) = backfill(dir.path(), &url, &url, &pool_hex);
    assert!(
        ok,
        "an endpoint whose proofs never arrive must not break ingest\nstdout:\n{bo}\nstderr:\n{be}"
    );

    rpc.reset_proof_attempts();
    let denied_before = rpc.proofs_denied();
    let (stdout, stderr, ok) = verify_root(dir.path(), &url, &url, &pool_hex);
    let out = format!("{stdout}\n{stderr}");
    let denied = rpc.proofs_denied() - denied_before;

    assert!(
        out.contains("UNAVAILABLE"),
        "§12 B1 + §11.4: after the retry budget is spent the answer is UNAVAILABLE, not \
         an error and not a verdict about the mirror.\n{out}"
    );
    assert!(
        !out.contains("MISMATCH"),
        "a provider that will not answer must never be reported as a divergence:\n{out}"
    );
    assert!(ok, "UNAVAILABLE is not a verification failure:\n{out}");
    assert_ne!(
        meta(dir.path(), "verify_root_failed").as_deref(),
        Some("1"),
        "UNAVAILABLE must never latch verify_root_failed (health DEGRADED)"
    );
    assert!(
        denied >= 3,
        "vacuity guard: only {denied} proof attempt(s) were made. Asking each configured \
         endpoint once is not a retry — §12 B1 requires a bounded retry against the SAME \
         endpoint, because error 42 names the backend and not the block."
    );
    assert!(
        denied <= 64,
        "the retry must be BOUNDED; {denied} attempts against an endpoint that always \
         refuses is a spin, not a budget"
    );
}

// ------------------------------------------------------------------ T19

/// §12 B2, the check that makes retry-until-success safe rather than
/// wishful: the proof pool is anonymous and load-balanced, so an accepted
/// proof must be BOUND to the chain — `global_roots.block_hash` compared with
/// `getBlockWithTxHashes(block).block_hash` — before its `storage_root` is
/// believed. Without it, "retry until one succeeds" is indistinguishable from
/// "accept whichever answer we liked".
///
/// This is the negative of T17: there the retried proof is genuine, here the
/// served proof carries an honest-looking root under the WRONG block hash. It
/// must be a hard error — never a retry-and-hope, never UNAVAILABLE (that
/// would file a lying endpoint under "capability gap"), and the root must
/// never reach the published anchors.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t19_a_proof_that_is_not_bound_to_the_block_is_rejected() {
    ensure_built();
    let fixture = load_devnet_fixture();
    let pool_hex = felt_hex(&fixture.constants.contract_address);
    let rpc = FixtureRpc::new(FixtureChain::build(&fixture), CHAIN_ID);
    let addr = rpc.serve().await;
    let url = format!("http://{addr}/");
    let dir = tempfile::tempdir().unwrap();

    // Built against an honest endpoint, so nothing below is about a broken
    // mirror.
    let (bo, be, ok) = backfill(dir.path(), &url, &url, &pool_hex);
    assert!(ok, "backfill failed\nstdout:\n{bo}\nstderr:\n{be}");
    let (stdout, _, ok) = verify_root(dir.path(), &url, &url, &pool_hex);
    assert!(
        ok && stdout.contains("verify-root OK"),
        "control: with an honest endpoint the mirror verifies: {stdout}"
    );
    let anchors_path = dir.path().join("feed/anchors.ndjson");
    let anchors_before = std::fs::read(&anchors_path).unwrap_or_default();

    // Now the pool answers for a block that is not the one we asked about.
    rpc.set_lying_proof(true);
    let (stdout, stderr, ok) = verify_root(dir.path(), &url, &url, &pool_hex);
    let out = format!("{stdout}\n{stderr}");

    assert!(
        !ok,
        "§12 B2: the proof's global_roots.block_hash is not the block's hash, so it is \
         not a proof about this block at all. Accepting its storage_root is how a \
         load-balanced pool gets to choose our answer. This must be a hard error.\n{out}"
    );
    assert!(
        !out.contains("verify-root OK"),
        "the unbound proof must never be reported as a match:\n{out}"
    );
    assert!(
        !out.contains("UNAVAILABLE"),
        "an endpoint that ANSWERS with a proof that does not belong to the block has not \
         had a capability gap; filing it as UNAVAILABLE hides a lie behind LIVE-6:\n{out}"
    );
    let lower = out.to_lowercase();
    assert!(
        lower.contains("block_hash") || lower.contains("block hash"),
        "the rejection must name the chain binding that failed:\n{out}"
    );
    assert_eq!(
        std::fs::read(&anchors_path).unwrap_or_default(),
        anchors_before,
        "§12 B2: a root from an unbound proof must never be published as an anchor"
    );
}

// ------------------------------------------------------------------ T20

/// A continuation token does NOT mean "the page was filled". `chunk_size` is a
/// maximum in the JSON-RPC spec, and a provider is free to stop early on an
/// internal budget — a scanned-block-range limit is the usual one — and hand
/// back a short page plus a token. That is the same class of provider variance
/// as LIVE-1 and LIVE-8: an answer about the backend, not about the data.
///
/// Two things must follow, and neither did before. The short page must not be
/// read as evidence about event density (the scan's page estimate is
/// monotonically non-increasing, so one short page clamps every later window
/// for the rest of a multi-hour scan), and — the sharp end, pinned here — a
/// one-block window that comes back short with a token must be RE-REQUESTED
/// rather than declared irreducible. A fresh single-page request carries no
/// cross-request state, so asking again is sound, and it is the only thing
/// that distinguishes "this block holds more events than a page" from "this
/// answer was cut short".
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t20_a_short_page_with_a_token_is_re_requested_not_called_irreducible() {
    ensure_built();
    let pool = Felt::from(0x9401u64);
    const ONLY: u64 = 105;
    // Exactly one block in the whole scan range, so the FIRST window the scan
    // asks is already a one-block window: there is nothing left to subdivide
    // and the re-request is the only way forward.
    let chain = dense_chain(pool, ONLY, 1, ONLY);
    let rpc = FixtureRpc::with_faults(
        chain,
        CHAIN_ID,
        FaultSpec {
            // One short-and-tokened answer, then the endpoint behaves.
            range_budget_tokens: 1,
            ..Default::default()
        },
    );
    let addr = rpc.serve().await;
    let url = format!("http://{addr}/");
    let dir = tempfile::tempdir().unwrap();

    let (stdout, stderr, ok) = backfill_synthetic(dir.path(), &url, &pool, ONLY, "1000");
    let out = format!("{stdout}\n{stderr}");
    assert!(
        ok,
        "the endpoint cut ONE answer short and then served the same window whole. Calling \
         that block irreducible aborts a backfill over an endpoint that can answer it \
         perfectly well:\n{out}"
    );
    assert!(
        rpc.short_token_pages() >= 1,
        "vacuity guard: the fixture never truncated a page, so nothing about short-page \
         tokens was exercised"
    );
    assert_eq!(
        rpc.tokens_presented(),
        0,
        "LIVE-8 is unchanged by any of this: a token is never presented, only re-asked \
         around"
    );
    let windows = rpc.event_windows();
    assert!(
        windows.iter().filter(|w| **w == (ONLY, ONLY)).count() >= 2,
        "the same window must be asked again — that is the re-request. Windows asked: \
         {windows:?}"
    );

    let db = Db::open(&dir.path().join("strk20.db")).unwrap();
    assert_eq!(
        db.events_of_block(ONLY).unwrap().len(),
        1,
        "and the block must be mirrored in full: a short page kept as if it were complete \
         is the LIVE-8 data loss under another name"
    );
}

/// The other side of T20: an endpoint that cuts EVERY answer short is not a
/// tuning problem and must not be described as one. It cannot serve a
/// single-page window for one block at all, so the run fails — with advice
/// that can be acted on. "Raise --chunk-size" is the wrong answer here (the
/// page was not full; raising the cap changes nothing), and telling an
/// operator to raise a limit that is not the limit costs a support cycle.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t20b_an_endpoint_that_always_truncates_is_named_as_the_problem() {
    ensure_built();
    let pool = Felt::from(0x9402u64);
    const ONLY: u64 = 105;
    let chain = dense_chain(pool, ONLY, 1, ONLY);
    let rpc = FixtureRpc::with_faults(
        chain,
        CHAIN_ID,
        FaultSpec {
            range_budget_tokens: usize::MAX,
            ..Default::default()
        },
    );
    let addr = rpc.serve().await;
    let url = format!("http://{addr}/");
    let dir = tempfile::tempdir().unwrap();

    let (stdout, stderr, ok) = backfill_synthetic(dir.path(), &url, &pool, ONLY, "1000");
    let out = format!("{stdout}\n{stderr}");
    assert!(
        !ok,
        "no single-page request covers this block, and keeping a truncated page would be \
         silent data loss:\n{out}"
    );
    assert!(
        out.contains(&format!("block {ONLY}")),
        "the error must name the block:\n{out}"
    );
    assert!(
        out.contains("chunk_size of 1000"),
        "the error must report what was ASKED for as well as what came back — the two \
         together are what say the page was not full:\n{out}"
    );
    assert!(
        !out.contains("Raise --chunk-size"),
        "the page was not full, so raising the page size cannot help. Advice that cannot \
         work is worse than none:\n{out}"
    );
    assert!(
        out.contains("replaced"),
        "the operator has to be told the endpoint itself is the problem:\n{out}"
    );
}

// ------------------------------------------------------------------ T21

/// The scan is SEGMENTED, so a failure costs one segment and not the whole
/// backfill.
///
/// The scan buffers every event it finds and the frontier is only written once
/// ingest has run, so a scan over the entire remaining range holds ~120k
/// events on a genesis backfill and throws away every call it made if anything
/// fails at the end of it. Here the last segment contains a block no endpoint
/// can answer in one page (T16's irreducible window), and what must survive is
/// everything the earlier segments already learned.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t21_a_failure_late_in_a_backfill_costs_one_segment_not_all_of_it() {
    ensure_built();
    let pool = Felt::from(0x9501u64);
    const EARLY: u64 = 1_000;
    const FAT: u64 = 210_000;
    const HEAD: u64 = 250_000;
    const SEGMENT: u64 = 100_000;
    let mut chain = FixtureChain::synthetic(pool, HEAD, HEAD - 10);
    chain.active.insert(EARLY, active_block(1));
    let mut fat = active_block(2);
    for extra in 0..3u64 {
        fat.events.push(FxEvent {
            keys: vec![
                Felt::from_hex(ENC_NOTE_CREATED_SELECTOR).unwrap(),
                Felt::from(0xfa70u64 + extra),
            ],
            data: vec![Felt::from(extra)],
        });
    }
    assert_eq!(fat.events.len(), 4);
    chain.active.insert(FAT, fat);

    let rpc = FixtureRpc::with_faults(
        chain,
        CHAIN_ID,
        FaultSpec {
            // Below the fat block's event count: irreducible by construction,
            // and it lives in the THIRD segment.
            max_page: Some(2),
            ..Default::default()
        },
    );
    let addr = rpc.serve().await;
    let url = format!("http://{addr}/");
    let dir = tempfile::tempdir().unwrap();

    let (stdout, stderr, ok) = backfill_synthetic(dir.path(), &url, &pool, 0, "1000");
    let out = format!("{stdout}\n{stderr}");
    assert!(!ok, "the irreducible window at block {FAT} must still fail loudly:\n{out}");

    let db = Db::open(&dir.path().join("strk20.db")).unwrap();
    assert_eq!(
        db.events_of_block(EARLY).unwrap().len(),
        1,
        "block {EARLY} was found in the FIRST segment and ingested there. An unsegmented \
         scan discards everything it accumulated when a later window fails, so this block \
         would not be in the mirror at all:\n{out}"
    );
    let cursor = db.ingest_cursor().unwrap();
    assert!(
        cursor.unwrap_or(0) >= 2 * SEGMENT - 1,
        "the frontier must be checkpointed at each segment boundary, or the work is redone \
         from the start on every restart: cursor = {cursor:?}"
    );
    assert!(
        cursor.unwrap_or(u64::MAX) < FAT,
        "...and it must NOT have advanced past the block that could not be answered: \
         cursor = {cursor:?}"
    );
}

// ------------------------------------------------------------------ helpers

/// Parse `anchors.ndjson` and assert every record is canonical and true to
/// the chain. Returns the parsed anchor blocks.
fn parse_anchors(text: &str, chain: &FixtureChain) -> Vec<u64> {
    let mut blocks = Vec::new();
    for line in text.lines() {
        let v: Value = serde_json::from_str(line)
            .unwrap_or_else(|e| panic!("anchors.ndjson line is not JSON ({e}): {line}"));
        let block = v["block"]
            .as_u64()
            .unwrap_or_else(|| panic!("anchor has no numeric block: {line}"));
        let field = |k: &str| -> String {
            let s = v[k]
                .as_str()
                .unwrap_or_else(|| panic!("anchor field {k} missing: {line}"));
            felt_hex(&Felt::from_hex(s).unwrap_or_else(|_| panic!("anchor {k} not a felt: {line}")))
        };
        // Canonical encoding: fixed field order, no whitespace, minimal hex.
        let canonical = format!(
            "{{\"block\":{},\"block_hash\":\"{}\",\"storage_root\":\"{}\",\"class\":\"{}\"}}",
            block,
            field("block_hash"),
            field("storage_root"),
            field("class")
        );
        assert_eq!(
            line, canonical,
            "anchors.ndjson must be canonically encoded (order, spacing, minimal hex)"
        );
        if let Some(prev) = blocks.last() {
            assert!(
                block > *prev,
                "anchors must be strictly ascending in block: {prev} then {block}"
            );
        }
        assert_eq!(
            field("block_hash"),
            felt_hex(&chain.block_hash(block)),
            "anchor block_hash disagrees with the chain at block {block}"
        );
        assert_eq!(
            field("storage_root"),
            felt_hex(&strk20_feed::mpt::storage_root(&chain.state_at(block))),
            "anchor storage_root disagrees with the chain at block {block}"
        );
        assert_eq!(
            field("class"),
            felt_hex(&chain.class_at(block).unwrap_or(Felt::ZERO)),
            "anchor class disagrees with the chain at block {block}"
        );
        blocks.push(block);
    }
    blocks
}

fn try_sync(
    dir: &Path,
    feed: &str,
    address: &Felt,
    key_hex: &str,
    db: &str,
    extra: &[&str],
) -> (String, String, bool) {
    let key_path = dir.join(format!("{}.key", felt_hex(address)));
    std::fs::write(&key_path, key_hex).unwrap();
    let mut cmd = Command::new(bin("strk20-sync"));
    cmd.arg("sync")
        .args(["--feed", feed])
        .args(["--address", &felt_hex(address)])
        .args(["--key-file", &key_path.display().to_string()])
        .args(["--db", db])
        .args(extra)
        .arg("--json");
    run_capture(cmd, false)
}

fn sync_client(dir: &Path, feed: &str, address: &Felt, key_hex: &str, db: &str) {
    let (stdout, stderr, ok) = try_sync(dir, feed, address, key_hex, db, &[]);
    assert!(ok, "client sync failed\nstdout:\n{stdout}\nstderr:\n{stderr}");
}

fn verify_anchors(feed: &str, db: &str) -> (String, String, bool) {
    let mut cmd = Command::new(bin("strk20-sync"));
    cmd.arg("verify-anchors")
        .args(["--feed", feed])
        .args(["--db", db])
        .arg("--json");
    run_capture(cmd, false)
}

/// First integer following `marker` in `text`.
fn block_after(text: &str, marker: &str) -> Option<u64> {
    let rest = text.split(marker).nth(1)?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    digits.parse().ok()
}

// -------------------------------------------------- repair harness (R1–R5)

/// The repair legs all start from the same shape as the mainnet loss: a mirror
/// that scanned a range, moved its frontier past it, and only later turns out
/// to be missing pool-active blocks DEEP inside it. Nothing above the hole ever
/// looks down again, so no amount of forward ingest can heal it — which is why
/// a seeker pass and a backward re-cut have to exist at all.
const AUDIT_GENESIS: u64 = 100;
const AUDIT_LAST_ACTIVE: u64 = 139;
const AUDIT_HEAD: u64 = 200;
/// A contiguous cluster, the shape the loss actually had on mainnet
/// (11,263,874–11,263,880; 11,265,889–11,265,893; …). Inside epoch 7
/// ([112..127] at epoch size 16), so epochs 8, 9 and 10 sit ABOVE it and must
/// be rewritten by hash propagation alone.
const HOLE: [u64; 3] = [118, 119, 120];
/// Inside epoch 6, and holed by ONE event rather than by the whole block: a
/// seeker that only compares block presence would call this mirror complete.
const UNDERCOUNT_BLOCK: u64 = 105;

/// The chain as the mirror first sees it: pool-active on every block of
/// [AUDIT_GENESIS, AUDIT_LAST_ACTIVE] except the ones the mirror is going to
/// be missing.
fn holey_chain(pool: Felt) -> FixtureChain {
    let mut chain = FixtureChain::synthetic(pool, AUDIT_HEAD, AUDIT_HEAD - 10);
    for n in AUDIT_GENESIS..=AUDIT_LAST_ACTIVE {
        if HOLE.contains(&n) {
            continue;
        }
        chain.active.insert(n, active_block(n));
    }
    chain
}

/// Open the hole AFTER the mirror has passed the range. The bug is not
/// simulated — the divergence is created exactly where LIVE-8 left one: blocks
/// the chain has and the mirror does not, below a frontier that never moves
/// back.
fn open_the_hole(rpc: &FixtureRpc, with_undercount: bool) {
    let mut chain = rpc.chain.write().unwrap();
    for n in HOLE {
        chain.active.insert(n, active_block(n));
    }
    if with_undercount {
        let blk = chain
            .active
            .get_mut(&UNDERCOUNT_BLOCK)
            .expect("the undercount block is pool-active from the start");
        blk.events.push(FxEvent {
            keys: vec![
                Felt::from_hex(ENC_NOTE_CREATED_SELECTOR).unwrap(),
                Felt::from(0xda7au64),
            ],
            data: vec![Felt::from(7u64)],
        });
        blk.diffs.push((Felt::from(0x30_0000u64), Felt::from(9u64)));
        blk.diffs.sort_by_key(|a| a.0.to_bytes_be());
    }
}

/// Chain truth: every pool-active block and how many events it carries.
fn chain_counts(rpc: &FixtureRpc) -> Vec<(u64, usize)> {
    rpc.chain
        .read()
        .unwrap()
        .active
        .iter()
        .map(|(n, b)| (*n, b.events.len()))
        .collect()
}

fn repair_args(dir: &Path, url: &str, pool: &Felt) -> Vec<String> {
    vec![
        "--db".into(),
        dir.join("strk20.db").display().to_string(),
        "--feed-dir".into(),
        dir.join("feed").display().to_string(),
        "--rpc-url".into(),
        url.to_owned(),
        "--rpc-fallback".into(),
        url.to_owned(),
        "--pool".into(),
        felt_hex(pool),
        "--chain-id".into(),
        CHAIN_ID.into(),
        "--genesis-block".into(),
        AUDIT_GENESIS.to_string(),
        "--epoch-size".into(),
        "16".into(),
        "--chunk-size".into(),
        "5".into(),
    ]
}

/// Run the seeker pass and return its machine-readable report.
fn audit_coverage(
    dir: &Path,
    url: &str,
    pool: &Felt,
    extra: &[&str],
) -> (Value, String, String, bool) {
    let json = dir.join("audit.json");
    let _ = std::fs::remove_file(&json);
    let mut cmd = Command::new(bin("strk20"));
    cmd.arg("audit-coverage")
        .args(repair_args(dir, url, pool))
        .args(["--json", &json.display().to_string()])
        .args(extra);
    let (stdout, stderr, ok) = run_capture(cmd, false);
    let report = std::fs::read(&json)
        .map(|b| serde_json::from_slice(&b).expect("audit report is JSON"))
        .unwrap_or(Value::Null);
    (report, stdout, stderr, ok)
}

fn recut_epochs(dir: &Path, url: &str, pool: &Felt, extra: &[&str]) -> (String, String, bool) {
    let mut cmd = Command::new(bin("strk20"));
    cmd.arg("recut-epochs")
        .args(repair_args(dir, url, pool))
        .args(extra);
    run_capture(cmd, false)
}

fn epoch_verify(dir: &Path, url: &str, pool: &Felt) -> (String, String, bool) {
    let mut cmd = Command::new(bin("strk20"));
    cmd.arg("epoch-verify").args(repair_args(dir, url, pool));
    run_capture(cmd, false)
}

fn verify_root_at(dir: &Path, url: &str, pool: &Felt, block: u64) -> (String, String, bool) {
    let mut cmd = Command::new(bin("strk20"));
    cmd.arg("verify-root")
        .args(repair_args(dir, url, pool))
        .args(["--block", &block.to_string()]);
    run_capture(cmd, false)
}

/// (idx, content hash) for every epoch the DB has cut.
fn epoch_hashes(dir: &Path) -> Vec<(u64, String)> {
    let db = Db::open(&dir.join("strk20.db")).expect("open indexer db");
    db.epoch_rows()
        .expect("epoch rows")
        .iter()
        .map(|r| (r.idx, hex::encode(r.content_hash)))
        .collect()
}

/// Blocks named in one section of the report, with their two event counts.
fn gaps(report: &Value, section: &str) -> Vec<(u64, u64, u64)> {
    report[section]
        .as_array()
        .unwrap_or_else(|| panic!("report has no {section} array: {report}"))
        .iter()
        .map(|g| {
            (
                g["block"].as_u64().expect("block"),
                g["chain_events"].as_u64().expect("chain_events"),
                g["mirror_events"].as_u64().expect("mirror_events"),
            )
        })
        .collect()
}

/// Backfill a holed chain and assert the mirror is complete BEFORE the hole is
/// opened — without that, every leg below could pass on a mirror that was
/// broken all along.
async fn mirror_before_the_hole(
    pool: Felt,
    faults: FaultSpec,
) -> (FixtureRpc, String, tempfile::TempDir) {
    let rpc = FixtureRpc::with_faults(holey_chain(pool), CHAIN_ID, faults);
    let addr = rpc.serve().await;
    let url = format!("http://{addr}/");
    let dir = tempfile::tempdir().unwrap();
    let (stdout, stderr, ok) = backfill_synthetic(dir.path(), &url, &pool, AUDIT_GENESIS, "5");
    assert!(ok, "backfill failed\nstdout:\n{stdout}\nstderr:\n{stderr}");
    (rpc, url, dir)
}

// ------------------------------------------------------------------- R1

/// A1 — the seeker pass. A mirror with a hole below its frontier can only be
/// found by re-asking the chain about the WHOLE history, and the answer has to
/// be exact: which blocks are absent, and which are present but short of
/// events. Both halves are pinned here, because a check that only compares
/// block presence would call the undercounted block healthy.
///
/// The seeker must also be the SOUND scan (single-page windows, no
/// continuation token). A seeker that paged would re-create the very holes it
/// is looking for and could report a clean mirror over blocks it never saw —
/// hence the zero-token assertion and the subdivision vacuity guard.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn r1_the_seeker_finds_every_missing_block_and_counts_it_exactly() {
    ensure_built();
    let pool = Felt::from(0x9401u64);
    let (rpc, url, dir) = mirror_before_the_hole(
        pool,
        FaultSpec {
            // publicnode's posture: no proofs at any height, so verify-root is
            // UNAVAILABLE and cannot be the thing that finds the hole. The
            // seeker is on its own — the production situation.
            proofs_unsupported: true,
            ..Default::default()
        },
    )
    .await;

    let (clean, stdout, stderr, ok) = audit_coverage(dir.path(), &url, &pool, &[]);
    assert!(ok, "audit-coverage failed\nstdout:\n{stdout}\nstderr:\n{stderr}");
    assert_eq!(
        (gaps(&clean, "missing").len(), gaps(&clean, "undercounted").len()),
        (0, 0),
        "control: a mirror that has just backfilled this chain is complete, so anything \
         the seeker reports below is the hole and not a false positive:\n{clean}"
    );

    open_the_hole(&rpc, true);
    let tokens_before = rpc.tokens_presented();
    let windows_before = rpc.event_windows().len();
    let (report, stdout, stderr, ok) = audit_coverage(dir.path(), &url, &pool, &[]);
    assert!(ok, "audit-coverage failed\nstdout:\n{stdout}\nstderr:\n{stderr}");

    assert_eq!(
        gaps(&report, "missing"),
        HOLE.iter().map(|b| (*b, 1, 0)).collect::<Vec<_>>(),
        "the seeker must name every block the chain has and the mirror does not, with the \
         chain's event count for each:\n{report}"
    );
    assert_eq!(
        gaps(&report, "undercounted"),
        vec![(UNDERCOUNT_BLOCK, 2, 1)],
        "a block that is PRESENT but short of events is a hole too — it is what an \
         events-first scan produces when it loses a page rather than a window:\n{report}"
    );

    let active = chain_counts(&rpc);
    let chain_events: u64 = active.iter().map(|(_, c)| *c as u64).sum();
    assert_eq!(
        (
            report["chain_blocks"].as_u64().unwrap(),
            report["chain_events"].as_u64().unwrap()
        ),
        (active.len() as u64, chain_events),
        "the seeker's own totals must be the chain's totals — this is the number the \
         mainnet audit compared (120,135 events in 28,655 blocks):\n{report}"
    );
    assert_eq!(
        (
            report["mirror_blocks"].as_u64().unwrap(),
            report["mirror_events"].as_u64().unwrap()
        ),
        (
            active.len() as u64 - HOLE.len() as u64,
            chain_events - HOLE.len() as u64 - 1
        ),
        "and the mirror's totals must be the mirror's:\n{report}"
    );

    assert_eq!(
        rpc.tokens_presented(),
        tokens_before,
        "the seeker presented a continuation token. A token is node-local state (LIVE-8); \
         a pass that follows one can silently skip the very blocks it was run to find, and \
         would then certify a holed mirror as complete."
    );
    assert!(
        rpc.event_windows().len() > windows_before + 1,
        "vacuity guard: the seeker asked {} window(s) for a range that cannot be answered \
         in one page at chunk 5, so it never subdivided",
        rpc.event_windows().len() - windows_before
    );
}

// ------------------------------------------------------------------- R2

/// A2 — targeted re-ingest. The seeker names the blocks; re-ingesting exactly
/// those must make the mirror's per-block event counts equal the chain's, with
/// no full re-backfill (the 70-minute option this whole path exists to avoid).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn r2_re_ingest_restores_only_the_blocks_the_seeker_named() {
    ensure_built();
    let pool = Felt::from(0x9402u64);
    let (rpc, url, dir) = mirror_before_the_hole(
        pool,
        FaultSpec {
            proofs_unsupported: true,
            ..Default::default()
        },
    )
    .await;
    open_the_hole(&rpc, true);

    let (before, _, _, _) = audit_coverage(dir.path(), &url, &pool, &[]);
    assert_eq!(gaps(&before, "missing").len(), HOLE.len(), "{before}");

    let (after, stdout, stderr, ok) = audit_coverage(dir.path(), &url, &pool, &["--repair"]);
    assert!(
        ok,
        "audit-coverage --repair failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    assert_eq!(
        (
            gaps(&after, "missing").len(),
            gaps(&after, "undercounted").len(),
            gaps(&after, "overcounted").len()
        ),
        (0, 0, 0),
        "after the repair the seeker must find nothing left to repair:\n{after}\n{stdout}"
    );

    // The mirror, block for block and event for event, against the chain.
    let db = Db::open(&dir.path().join("strk20.db")).unwrap();
    let got: Vec<(u64, usize)> = db
        .blocks_in_range(AUDIT_GENESIS, AUDIT_HEAD)
        .unwrap()
        .iter()
        .map(|b| (b.number, db.events_of_block(b.number).unwrap().len()))
        .collect();
    assert_eq!(
        got,
        chain_counts(&rpc),
        "the repaired mirror's per-block event counts must equal the chain's exactly"
    );
    for n in HOLE {
        assert!(
            !db.diffs_of_block(n).unwrap().is_empty(),
            "block {n} came back without its pool storage writes — the mainnet hole was 6 \
             events AND 4 slot writes, and a repair that recovers only events leaves the \
             storage root wrong"
        );
    }
}

// ------------------------------------------------------------------- R3

/// A3 — backward re-cut. Repairing a block deep in history changes its epoch's
/// content, therefore that epoch's hash, therefore — through `prev` — every
/// epoch hash above it. The forward-only cutter cannot express that: it starts
/// at `last_epoch().idx + 1` and can only ever append. So a repaired database
/// never reaches the published bytes, which is the second half of the repair
/// and the half a production operator needs most.
///
/// Pinned here: the affected epoch's hash changes, EVERY epoch above it
/// changes, the epoch BELOW is left alone, and the rewritten chain verifies.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn r3_a_backward_re_cut_rewrites_the_whole_chain_above_the_repair() {
    ensure_built();
    let pool = Felt::from(0x9403u64);
    let (rpc, url, dir) = mirror_before_the_hole(
        pool,
        FaultSpec {
            proofs_unsupported: true,
            ..Default::default()
        },
    )
    .await;

    let before = epoch_hashes(dir.path());
    let holed_epoch = HOLE[0] / 16;
    assert!(
        before.iter().any(|(i, _)| *i == holed_epoch)
            && before.iter().any(|(i, _)| *i > holed_epoch),
        "fixture precondition: epoch {holed_epoch} must be cut and have epochs above it, \
         or hash propagation is never exercised. Cut: {before:?}"
    );

    open_the_hole(&rpc, false);
    let (_, stdout, stderr, ok) = audit_coverage(dir.path(), &url, &pool, &["--repair"]);
    assert!(ok, "repair failed\nstdout:\n{stdout}\nstderr:\n{stderr}");

    // Starting too HIGH is the easy operator error, and it fails in the worst
    // possible way if it is allowed: the epochs below keep publishing
    // pre-repair bytes while everything above looks freshly repaired.
    let (stdout, stderr, ok) = recut_epochs(
        dir.path(),
        &url,
        &pool,
        &["--from-epoch", &(holed_epoch + 1).to_string()],
    );
    let out = format!("{stdout}\n{stderr}");
    assert!(
        !ok && out.contains(&format!("epoch {holed_epoch}")),
        "a re-cut starting ABOVE the affected epoch must be refused and must name the \
         epoch that actually changed ({holed_epoch}), or the operator republishes a chain \
         that still contradicts its own database:\n{out}"
    );

    let (stdout, stderr, ok) = recut_epochs(
        dir.path(),
        &url,
        &pool,
        &["--from-block", &HOLE[0].to_string()],
    );
    assert!(
        ok,
        "recut-epochs failed\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    let after = epoch_hashes(dir.path());
    assert_eq!(
        before.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
        after.iter().map(|(i, _)| *i).collect::<Vec<_>>(),
        "a re-cut rewrites epochs, it does not add or drop them"
    );
    for ((idx, old), (_, new)) in before.iter().zip(after.iter()) {
        if *idx < holed_epoch {
            assert_eq!(
                old, new,
                "epoch {idx} is BELOW the repair and must not be rewritten: a re-cut that \
                 touches history it had no reason to touch is indistinguishable from one \
                 that rewrites it"
            );
        } else {
            assert_ne!(
                old, new,
                "epoch {idx} kept its content hash across the re-cut. Epoch {holed_epoch} \
                 gained a block, so its hash changes; every epoch above chains through \
                 `prev` and changes with it. An unchanged hash above the repair means the \
                 published chain no longer describes the database."
            );
        }
    }

    let (stdout, stderr, ok) = epoch_verify(dir.path(), &url, &pool);
    assert!(
        ok && stdout.contains("hash chain OK"),
        "the rewritten epoch files must verify against the rewritten chain\nstdout:\n\
         {stdout}\nstderr:\n{stderr}"
    );

    // The manifest is the file clients actually read; a re-cut that leaves it
    // naming the old hashes has published nothing.
    let manifest: Value = serde_json::from_slice(
        &std::fs::read(dir.path().join("feed").join("manifest.json")).expect("manifest"),
    )
    .expect("manifest is JSON");
    let published: Vec<(u64, String)> = manifest["epochs"]
        .as_array()
        .expect("manifest epochs")
        .iter()
        .map(|e| {
            (
                e["e"].as_u64().unwrap(),
                e["hash"].as_str().unwrap().to_owned(),
            )
        })
        .collect();
    assert_eq!(published, after, "the manifest must carry the re-cut hashes");
}

// ------------------------------------------------------------------- R4

/// A4 — the guard. Rewriting published history is the most dangerous thing
/// this binary can do, so it must be impossible to do by accident: a re-cut of
/// an epoch whose content did NOT change is refused, loudly, with nothing
/// written. Without this, a stray invocation (or a future automatic caller)
/// could republish every epoch above an untouched one with new bytes and no
/// reason.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn r4_a_re_cut_is_refused_when_nothing_changed() {
    ensure_built();
    let pool = Felt::from(0x9404u64);
    let (_rpc, url, dir) = mirror_before_the_hole(
        pool,
        FaultSpec {
            proofs_unsupported: true,
            ..Default::default()
        },
    )
    .await;

    let before = epoch_hashes(dir.path());
    let target = HOLE[0] / 16;
    let file = dir
        .path()
        .join("feed")
        .join("epochs")
        .join(format!("{target:08}.strk20e.zst"));
    let bytes_before = std::fs::read(&file).expect("the target epoch file exists");

    let (stdout, stderr, ok) = recut_epochs(
        dir.path(),
        &url,
        &pool,
        &["--from-epoch", &target.to_string()],
    );
    let out = format!("{stdout}\n{stderr}");
    assert!(
        !ok,
        "a re-cut of an unchanged epoch must FAIL: nothing below it moved, so rewriting \
         it and everything above would be a history rewrite with no cause.\n{out}"
    );
    let lower = out.to_lowercase();
    assert!(
        lower.contains("refus") || lower.contains("unchanged") || lower.contains("identical"),
        "the refusal must say why, so an operator knows the tool declined rather than \
         broke:\n{out}"
    );

    assert_eq!(
        epoch_hashes(dir.path()),
        before,
        "a refused re-cut must leave every epoch row exactly as it was"
    );
    assert_eq!(
        std::fs::read(&file).unwrap(),
        bytes_before,
        "a refused re-cut must not have rewritten a single published byte"
    );
}

// ------------------------------------------------------------------- R5

/// A5 — the whole causal chain, closed. Session 10 bisected the mainnet
/// divergence to a single block: `verify-root --block 11263134` OK,
/// `--block 11263135` MISMATCH, with our local root unchanged across the
/// boundary because nothing was ingested there. This leg reproduces exactly
/// that shape and then shows the repair CLEARS it — which is the acceptance
/// criterion for the mainnet mirror.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn r5_after_repair_verify_root_matches_where_it_previously_mismatched() {
    ensure_built();
    // Proofs served: this leg is about the root, not about provider capability.
    let pool = Felt::from(0x9405u64);
    let (rpc, url, dir) = mirror_before_the_hole(pool, FaultSpec::default()).await;
    const ABOVE: u64 = 130;
    let below = HOLE[0] - 1;

    let (stdout, stderr, ok) = verify_root_at(dir.path(), &url, &pool, ABOVE);
    assert!(
        ok && stdout.contains("verify-root OK"),
        "control: before the hole, the mirror reproduces the chain's storage root at \
         block {ABOVE}\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );

    open_the_hole(&rpc, false);

    let (stdout, _, ok) = verify_root_at(dir.path(), &url, &pool, below);
    assert!(
        ok && stdout.contains("verify-root OK"),
        "block {below} is BELOW the first lost block, so the mirror is still exactly \
         right there — the divergence must begin at the hole and not before it:\n{stdout}"
    );
    let (stdout, stderr, ok) = verify_root_at(dir.path(), &url, &pool, ABOVE);
    let out = format!("{stdout}\n{stderr}");
    assert!(
        !ok && out.contains("VERIFY-ROOT MISMATCH"),
        "with {} pool-active blocks missing below it, the recomputed root at block \
         {ABOVE} cannot equal the chain's. A green verify-root here would mean the check \
         that found the mainnet loss cannot see this one:\n{out}",
        HOLE.len()
    );

    let (_, stdout, stderr, ok) = audit_coverage(dir.path(), &url, &pool, &["--repair"]);
    assert!(ok, "repair failed\nstdout:\n{stdout}\nstderr:\n{stderr}");

    let (stdout, stderr, ok) = verify_root_at(dir.path(), &url, &pool, ABOVE);
    assert!(
        ok && stdout.contains("verify-root OK"),
        "after the targeted repair, the mirror must reproduce the chain's root at block \
         {ABOVE} — the same statement the mainnet mirror has to be able to make, without \
         a 70-minute re-backfill\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}

// ------------------------------------------------------------- T22 / T23
//
// The starvation defect, observed on the hosted Sepolia instance
// (sound-ingest.md §2.3 and §8.1). Each `run` cycle ended in an epoch cut, a
// cut began with verify-root, and a MISMATCH sent the cut into a §5.6 rescan
// of a window derived from the PROBE block — which is at the frontier, while
// the divergence can sit arbitrarily far below it. So the rescan could not
// converge (measured: 4 rounds, 2.46 hours, 0 blocks repaired), and nothing
// remembered that it had been tried: the next cycle's head move re-entered it.
// What an operator saw was a frozen head, a silent log, and `/health` DEGRADED
// with no statement of what to do about it.
//
// T22 the closure loop repairs an eventless divergence in ONE attempt, and the
//     latch clears itself
// T23 a divergence the loop cannot repair is attempted ONCE; ingest keeps
//     advancing, `/health` stays DEGRADED and names the commands that repair it

/// Where both legs put the silent write: above the epoch floor (31) and below
/// the frontier, so no published epoch has to be re-cut, and the `run_cycle`
/// tail sweep — which only ever looks ABOVE the frontier — cannot be what
/// finds it.
const SILENT_WRITE_BLOCK: u64 = 35;
const SILENT_SLOT: u64 = 0x5100_0000;

fn spawn_run(dir: &Path, url: &str, pool_hex: &str, port: u16, tag: &str) -> ChildGuard {
    let mut cmd = Command::new(bin("strk20"));
    let mut args = base_args(dir, url, url, pool_hex);
    args.extend(["--listen".into(), format!("127.0.0.1:{port}")]);
    args.extend(["--poll-ms".into(), "150".into()]);
    cmd.arg("run").args(args);
    spawn_with_logs(cmd, dir, tag)
}

async fn health(client: &reqwest::Client, port: u16) -> Option<Value> {
    client
        .get(format!("http://127.0.0.1:{port}/health"))
        .send()
        .await
        .ok()?
        .json::<Value>()
        .await
        .ok()
}

fn log_of(guard: &ChildGuard) -> String {
    let out = std::fs::read_to_string(&guard.stdout_path).unwrap_or_default();
    let err = std::fs::read_to_string(&guard.stderr_path).unwrap_or_default();
    format!("{out}\n{err}")
}

fn count_lines(log: &str, needle: &str) -> usize {
    log.lines().filter(|l| l.contains(needle)).count()
}

/// Poll `/health` for `ticks` cycles, moving the chain head as it goes, and
/// stop early once `done` holds over (health, log).
///
/// Moving the head is not decoration: `verify_and_capture` skips the proof
/// when the frontier has not moved since the last completed probe, so a still
/// chain never re-checks. It is also the exact condition under which the
/// shipped build re-entered its rescan — one head move per cycle was all it
/// took to lose the next half hour.
async fn poll_health(
    client: &reqwest::Client,
    port: u16,
    rpc: &FixtureRpc,
    indexer: &ChildGuard,
    ticks: usize,
    mut done: impl FnMut(&Value, &str) -> bool,
) -> (Option<Value>, Vec<u64>) {
    let mut heads: Vec<u64> = Vec::new();
    let mut last = None;
    for i in 0..ticks {
        if i % 3 == 0 {
            rpc.chain.write().unwrap().head += 1;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        let Some(v) = health(client, port).await else {
            continue;
        };
        if let Some(h) = v["head"]["number"].as_u64() {
            if heads.last() != Some(&h) {
                heads.push(h);
            }
        }
        let stop = done(&v, &log_of(indexer));
        last = Some(v);
        if stop {
            break;
        }
    }
    (last, heads)
}

/// A pool write on a block with NO pool event, injected BELOW the frontier of
/// an already-verified mirror — the eventless class of sound-ingest.md §1, in
/// the one position where every heuristic index is blind to it and the tail
/// state-diff sweep has already gone past.
///
/// One closure-loop attempt has to be enough: the storage-trie walk names the
/// slot the chain holds and the mirror does not, the bisection attributes it
/// to block 35, the targeted rescan re-ingests exactly that block, and the
/// retried cut verifies. The MATCH then clears `verify_root_failed` AND the
/// recorded divergence, with no operator in the loop.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t22_one_closure_loop_attempt_repairs_an_eventless_divergence() {
    ensure_built();
    let fixture = load_devnet_fixture();
    let pool_hex = felt_hex(&fixture.constants.contract_address);
    let rpc = FixtureRpc::new(FixtureChain::build(&fixture), CHAIN_ID);
    let addr = rpc.serve().await;
    let url = format!("http://{addr}/");
    let dir = tempfile::tempdir().unwrap();

    let (so, se, ok) = backfill(dir.path(), &url, &url, &pool_hex);
    assert!(ok, "backfill failed\nstdout:\n{so}\nstderr:\n{se}");
    assert_ne!(
        meta(dir.path(), "verify_root_failed").as_deref(),
        Some("1"),
        "fixture sanity: the mirror starts verified, so the mismatch below is the \
         injected write and nothing else"
    );

    rpc.chain.write().unwrap().active.insert(
        SILENT_WRITE_BLOCK,
        ActiveBlock {
            diffs: vec![(Felt::from(SILENT_SLOT), Felt::from(0x99u64))],
            events: vec![],
            deployed_class: None,
            replaced_class: None,
        },
    );

    let port = pick_free_port();
    let indexer = spawn_run(dir.path(), &url, &pool_hex, port, "t22-indexer");
    let client = reqwest::Client::new();
    // Both halves must be observed: the loop RAN, and health came back. Waiting
    // on health alone would pass on the first poll, before the moved frontier
    // has given verify-root anything to disagree about.
    let (last, heads) = poll_health(&client, port, &rpc, &indexer, 150, |v, log| {
        count_lines(log, "closure loop finished") >= 1 && v["status"] == "OK"
    })
    .await;

    let log = log_of(&indexer);
    let last = last.unwrap_or_else(|| panic!("/health never answered\n{log}"));
    assert_eq!(
        count_lines(&log, "entering the §4.2 closure loop"),
        1,
        "recovery runs at most once per divergence — and once was enough here.\n\
         heads seen: {heads:?}\n{log}"
    );
    assert_eq!(
        last["status"], "OK",
        "the divergence must heal with no operator action: {last}\n{log}"
    );
    assert_eq!(last["mismatch_block"], Value::Null, "{last}");
    assert_eq!(last["reason"], Value::Null, "{last}");
    assert_eq!(
        count_lines(&log, "rescanning range"),
        0,
        "the blind window rescan is gone; localisation is the trie walk's job\n{log}"
    );

    drop(indexer);
    assert_eq!(
        Db::open(&dir.path().join("strk20.db"))
            .unwrap()
            .blocks_in_range(SILENT_WRITE_BLOCK, SILENT_WRITE_BLOCK)
            .unwrap()
            .len(),
        1,
        "the walk attributed the missing slot to block {SILENT_WRITE_BLOCK} and the \
         targeted rescan re-ingested exactly it\n{log}"
    );
    for key in ["recovery_divergence", "recovery_reason"] {
        assert!(
            meta(dir.path(), key).unwrap_or_default().is_empty(),
            "a verify-root MATCH must retire the recorded divergence ({key} still set)\n{log}"
        );
    }
}

/// The same eventless class, made UNREPAIRABLE: the slot is in the chain's
/// storage root but no state update ever names it (`FaultSpec::hidden_slot`),
/// so the walk finds it and the bisection cannot attribute it to any block.
/// This is the shape that used to cost tens of minutes of starved ingest per
/// poll cycle, indefinitely.
///
/// Three properties, in the order they matter: ingest keeps advancing,
/// recovery is attempted exactly once, and `/health` says what to run rather
/// than only that something is wrong.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t23_an_unrepairable_divergence_is_attempted_once_while_ingest_keeps_running() {
    ensure_built();
    let fixture = load_devnet_fixture();
    let pool_hex = felt_hex(&fixture.constants.contract_address);
    let rpc = FixtureRpc::with_faults(
        FixtureChain::build(&fixture),
        CHAIN_ID,
        FaultSpec {
            hidden_slot: Some((Felt::from(SILENT_SLOT), Felt::from(0x99u64))),
            ..Default::default()
        },
    );
    let addr = rpc.serve().await;
    let url = format!("http://{addr}/");
    let dir = tempfile::tempdir().unwrap();

    let port = pick_free_port();
    let indexer = spawn_run(dir.path(), &url, &pool_hex, port, "t23-indexer");
    let client = reqwest::Client::new();
    // Well past the three cycles the property is stated over. The predicate
    // never stops early: what is being measured is what happens when the loop
    // keeps going, which is where the old build lost its afternoons.
    let (last, heads) = poll_health(&client, port, &rpc, &indexer, 60, |_, _| false).await;

    let log = log_of(&indexer);
    let last = last.unwrap_or_else(|| panic!("/health never answered\n{log}"));
    assert_eq!(
        last["status"], "DEGRADED",
        "an unrepaired divergence stays DEGRADED: {last}\n{log}"
    );
    assert_eq!(last["verify_root_failed"], Value::Bool(true), "{last}");
    assert!(
        last["mismatch_block"].as_u64().is_some(),
        "/health must name the block the mismatch was seen at: {last}"
    );
    let reason = last["reason"].as_str().unwrap_or_default();
    for fragment in [
        "mismatch at",
        "recovery attempted once",
        "enumerate-slots --attribute",
        "rescan --blocks",
        "recut-epochs",
    ] {
        assert!(
            reason.contains(fragment),
            "/health reason must name {fragment:?}, got {reason:?}"
        );
    }
    assert!(
        heads.len() >= 4,
        "ingest must keep running while DEGRADED: the head only moved through {heads:?} \
         over 60 poll cycles, which is the starved loop this leg exists to catch\n{log}"
    );
    assert!(
        log.contains("storage-trie walk finished") && log.contains("missing_slots=1"),
        "non-vacuity: the one attempt really did run the walk and really did enumerate \
         the hidden slot — the leg would otherwise pass on a recovery that never \
         started\n{log}"
    );
    assert_eq!(
        count_lines(&log, "entering the §4.2 closure loop"),
        1,
        "recovery must be attempted ONCE per divergence, not once per cycle\n{log}"
    );
    assert_eq!(
        count_lines(&log, "skipping recovery and continuing to ingest"),
        1,
        "and the skip is announced once, not on every one of the dozens of cycles \
         this test ran — a line repeated per cycle is the same silence\n{log}"
    );
    // The reason a fingerprint comparison cannot be the decision, demonstrated
    // rather than argued: the probe block is min(frontier, head) and both roots
    // move with it, so by the very next cycle the "same" divergence has three
    // different numbers. A guard keyed on equality would have called that a new
    // divergence and re-entered recovery — which is the defect.
    assert!(
        log.contains("identical=false"),
        "the divergence's identity moved between cycles and the guard held anyway\n{log}"
    );

    drop(indexer);
    assert_eq!(
        meta(dir.path(), "recovery_attempts").as_deref(),
        Some("1"),
        "the persisted attempt count is what makes the guard survive a restart"
    );
}

// ---------------------------------------------------------------- T24

/// A verify-root MISMATCH during a reorg is the mirror holding the abandoned
/// branch, not a hole — and telling the two apart is what keeps the one
/// recovery attempt for the case that needs it.
///
/// This is not hypothetical: with the closure loop taking seconds, a fork
/// landing between the probe and the walk had the walk enumerate against the
/// NEW branch and re-ingest a block from it into a mirror that had not rolled
/// back. The canonicity walkback then stopped at a block that only looked
/// canonical because recovery had just fetched it, the rewind was too shallow,
/// and blocks on the new branch below it were never scanned (acceptance leg g,
/// intermittently).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn t24_a_mismatch_while_the_tail_is_forking_is_a_reorg_not_a_hole() {
    ensure_built();
    let fixture = load_devnet_fixture();
    let pool_hex = felt_hex(&fixture.constants.contract_address);
    let rpc = FixtureRpc::new(FixtureChain::build(&fixture), CHAIN_ID);
    let addr = rpc.serve().await;
    let url = format!("http://{addr}/");
    let dir = tempfile::tempdir().unwrap();

    let (so, se, ok) = backfill(dir.path(), &url, &url, &pool_hex);
    assert!(ok, "backfill failed\nstdout:\n{so}\nstderr:\n{se}");

    let db = Db::open(&dir.path().join("strk20.db")).unwrap();
    let client = strk20_indexerd::rpc::RpcClient::new(url.clone(), None);
    assert!(
        !strk20_indexerd::recovery::reorg_in_flight(&db, &client)
            .await
            .unwrap(),
        "a settled chain must not read as a reorg, or every real divergence would be \
         waved through as one"
    );

    // The stored head is gone from the chain entirely.
    rpc.chain.write().unwrap().fork_tail(45);
    assert!(
        strk20_indexerd::recovery::reorg_in_flight(&db, &client)
            .await
            .unwrap(),
        "the mirror's head no longer exists on this chain"
    );

    // ...and the case that matters more, because it answers rather than
    // erroring: the height is still there, carrying a different block.
    rpc.chain.write().unwrap().head = 46;
    assert!(
        strk20_indexerd::recovery::reorg_in_flight(&db, &client)
            .await
            .unwrap(),
        "the height is populated again, but by a different block than the mirror holds"
    );
}
