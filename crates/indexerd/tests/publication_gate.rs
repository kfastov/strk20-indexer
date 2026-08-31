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
//! Everything here is a pure function of DB rows. The RPC is a dead address on
//! purpose: §12 B4 makes publication try for a basis-block proof first, and an
//! endpoint that cannot answer must leave the §11.3 reachability gate — the
//! thing these legs are about — deciding on its own.

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
        )
        .expect("insert block data");
    }
    db.insert_block_data(&block(ANCHOR_BLOCK), &[], &[], None, ANCHOR_BLOCK)
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
#[tokio::test]
async fn the_gate_publishes_when_the_mirror_last_matched_the_chain() {
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
    cutter.maybe_publish_snapshot().await.expect("publish");
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
#[tokio::test]
async fn a_latched_verify_root_failure_blocks_publication_even_with_an_anchor() {
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
    cutter.maybe_publish_snapshot().await.expect("no error, just no publication");
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
    cutter.maybe_publish_snapshot().await.expect("publish after recovery");
    cutter.rewrite_manifest().unwrap();
    assert_eq!(
        manifest_snapshot(&dir.path().join("feed"))["e"].as_u64(),
        Some(1),
        "the latch is a gate, not a permanent ban"
    );
}

/// The other half of the gate: no anchor at or above the basis means nothing
/// grounds the snapshot, so none may be published.
#[tokio::test]
async fn no_anchor_at_or_above_the_basis_publishes_nothing() {
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
    cutter.maybe_publish_snapshot().await.expect("no error");
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

// --------------------------------------------------------------- §12 B1/B4
//
// The legs below need an RPC that ANSWERS, so they bring a minimal fixture
// rather than the dead address above: what is under test is what happens to a
// snapshot when a basis-block proof is obtained and disagrees, and when one is
// refused for a while and then served.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

/// A storage-proof endpoint with two knobs: how many attempts it refuses with
/// error 42 before serving anything, and which storage root it serves. Its
/// `global_roots.block_hash` is always the block's real hash, so §12 B2 binding
/// passes and the legs below are about the ROOT and nothing else.
#[derive(Clone)]
struct ProofFixture {
    refusals_left: Arc<AtomicUsize>,
    attempts: Arc<AtomicUsize>,
    root: Arc<std::sync::Mutex<Felt>>,
}

impl ProofFixture {
    fn new(refusals: usize, root: Felt) -> Self {
        Self {
            refusals_left: Arc::new(AtomicUsize::new(refusals)),
            attempts: Arc::new(AtomicUsize::new(0)),
            root: Arc::new(std::sync::Mutex::new(root)),
        }
    }
    fn attempts(&self) -> usize {
        self.attempts.load(Ordering::SeqCst)
    }
    /// Stop refusing: the next call is routed to a backend that runs archive
    /// tries, which is what a later cycle amounts to against an aggregator.
    fn stop_refusing(&self) {
        self.refusals_left.store(0, Ordering::SeqCst);
    }
    fn set_root(&self, root: Felt) {
        *self.root.lock().unwrap() = root;
    }
    async fn serve(&self) -> String {
        let me = self.clone();
        let app = axum::Router::new().route(
            "/",
            axum::routing::post(move |body: axum::body::Bytes| {
                let me = me.clone();
                async move {
                    let req: serde_json::Value = serde_json::from_slice(&body).unwrap();
                    let id = req["id"].clone();
                    let method = req["method"].as_str().unwrap_or_default().to_owned();
                    let number = req["params"][0]["block_number"].as_u64().unwrap_or(0);
                    let result = match method.as_str() {
                        "starknet_getStorageProof" => {
                            me.attempts.fetch_add(1, Ordering::SeqCst);
                            if me
                                .refusals_left
                                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| {
                                    (v > 0).then(|| v - 1)
                                })
                                .is_ok()
                            {
                                return axum::Json(serde_json::json!({
                                    "jsonrpc": "2.0", "id": id,
                                    "error": {
                                        "code": 42,
                                        "message": "the node doesn't support storage proofs \
                                                    for blocks that are too far in the past"
                                    }
                                }));
                            }
                            serde_json::json!({
                                "classes_proof": [],
                                "contracts_proof": {
                                    "nodes": [],
                                    "contract_leaves_data": [{
                                        "nonce": "0x0",
                                        "class_hash": "0x67dd",
                                        "storage_root": strk20_feed::felt_hex(
                                            &me.root.lock().unwrap().clone()),
                                    }]
                                },
                                "contracts_storage_proofs": [[]],
                                "global_roots": {
                                    "contracts_tree_root": "0x0",
                                    "classes_tree_root": "0x0",
                                    // §12 B2: the block's real hash, so the
                                    // binding is never what fails here.
                                    "block_hash": strk20_feed::felt_hex(&block(number).hash),
                                }
                            })
                        }
                        "starknet_getBlockWithTxHashes" => serde_json::json!({
                            "block_number": number,
                            "block_hash": strk20_feed::felt_hex(&block(number).hash),
                            "parent_hash": strk20_feed::felt_hex(&block(number).parent_hash),
                            "timestamp": block(number).timestamp,
                            "status": "ACCEPTED_ON_L1",
                            "new_root": "0x0",
                            "transactions": [],
                        }),
                        other => {
                            return axum::Json(serde_json::json!({
                                "jsonrpc": "2.0", "id": id,
                                "error": {"code": -32601, "message": format!("no {other}")}
                            }))
                        }
                    };
                    axum::Json(serde_json::json!({"jsonrpc": "2.0", "id": id, "result": result}))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        format!("http://{addr}/")
    }
}

/// The storage root of the slot set `gated_mirror` writes at or below the
/// basis — what an honest chain proof for block BASIS says.
fn honest_basis_root() -> Felt {
    strk20_feed::mpt::storage_root(&[
        (Felt::from(0xaa_u64), Felt::from(0x11_u64)),
        (Felt::from(0xbb_u64), Felt::from(0x22_u64)),
        (Felt::from(0xcc_u64), Felt::from(0x33_u64)),
    ])
}

/// A basis-block proof that DISAGREES with the slot set must not merely fail
/// once: it must LATCH, because the fallback grounding would otherwise publish
/// the very slot set the chain has just contradicted.
///
/// This is the concrete hole the review found. The basis probe is budgeted per
/// epoch, and the budget used to be spent BEFORE the call; on a mismatch the
/// error left the marker behind, so the next call (`cut_epochs_with_recovery`
/// makes one immediately, in the same function) skipped the proof entirely,
/// found the §11.3 anchor gate met, and published — with
/// `grounding: "reachability"` and health still OK.
#[tokio::test]
async fn a_basis_proof_that_contradicts_the_slot_set_latches_instead_of_falling_back() {
    let dir = tempfile::tempdir().unwrap();
    let (db, cfg) = gated_mirror(dir.path(), Some(ANCHOR_BLOCK));
    // Chain-bound, obtainable — and wrong. This is a mirror missing a write at
    // or below the basis, which is exactly the LIVE-8 loss this release exists
    // to prevent.
    let fixture = ProofFixture::new(0, honest_basis_root() + Felt::ONE);
    let url = fixture.serve().await;
    let rpc = RpcClient::new(url, None);
    let cutter = Cutter {
        db: &db,
        rpc: &rpc,
        cfg: &cfg,
        feed_dir: dir.path().join("feed"),
    };
    cutter.ensure_layout().unwrap();

    let err = cutter
        .maybe_publish_snapshot()
        .await
        .expect_err("the chain contradicts this slot set; publication must fail");
    let text = format!("{err:#}");
    assert!(
        text.contains("VERIFY-ROOT MISMATCH") && text.contains(&BASIS.to_string()),
        "the §5.6 recovery path keys on this name and the operator needs the block: {text}"
    );
    assert!(
        fixture.attempts() > 0,
        "vacuity guard: no proof was ever fetched, so the failure above was not about a \
         basis proof at all"
    );
    assert_eq!(
        db.meta_get("verify_root_failed").unwrap().as_deref(),
        Some("1"),
        "a mirror caught disagreeing with the chain must be latched: the latch is what \
         stops the FALLBACK grounding publishing the same slot set on the next cycle, and \
         it is what /health reports as DEGRADED"
    );

    // The second call is not hypothetical: cut_epochs_with_recovery retries
    // within the same function after its rescan.
    cutter
        .maybe_publish_snapshot()
        .await
        .expect("the latched state is a refusal, not a second error");
    assert!(
        snapshot_files(&dir.path().join("feed")).is_empty(),
        "the slot set the chain contradicted must never be published — not on the basis \
         anchor, and not on the §11.3 fallback either: {:?}",
        snapshot_files(&dir.path().join("feed"))
    );
    cutter.rewrite_manifest().unwrap();
    assert!(
        manifest_snapshot(&dir.path().join("feed")).is_null(),
        "...and no manifest entry may name one"
    );

    // Control: the same fixture, now honest, publishes. So the refusal above
    // was about the contradiction and not about a fixture that cannot publish.
    fixture.set_root(honest_basis_root());
    db.meta_set("verify_root_failed", "").unwrap();
    cutter.maybe_publish_snapshot().await.expect("publish after recovery");
    cutter.rewrite_manifest().unwrap();
    let entry = manifest_snapshot(&dir.path().join("feed"));
    assert_eq!(entry["e"].as_u64(), Some(1), "{entry}");
    assert_eq!(
        entry["grounding"].as_str(),
        Some("basis-anchor"),
        "with the chain agreeing, the snapshot is grounded on the basis proof: {entry}"
    );
}

/// §12 B1 across CYCLES, not just within a call: a refusal is per-call routing
/// luck, so a basis proof that could not be obtained this cycle is asked for
/// again on the next one. Without that, one unlucky group of retries costs a
/// snapshot its primary grounding permanently.
#[tokio::test]
async fn an_unobtainable_basis_proof_is_retried_on_the_next_cycle() {
    let dir = tempfile::tempdir().unwrap();
    // No reachability anchor at all: publication cannot happen on the fallback,
    // so what the second call does with the proof is the only thing that can
    // produce a snapshot here.
    let (db, cfg) = gated_mirror(dir.path(), None);
    // Refuses everything for now; the first cycle cannot obtain a proof no
    // matter how many times it retries.
    let fixture = ProofFixture::new(usize::MAX, honest_basis_root());
    let url = fixture.serve().await;
    let rpc = RpcClient::new(url, None);
    let cutter = Cutter {
        db: &db,
        rpc: &rpc,
        cfg: &cfg,
        feed_dir: dir.path().join("feed"),
    };
    cutter.ensure_layout().unwrap();

    cutter.maybe_publish_snapshot().await.expect("a refusal is not an error");
    let first = fixture.attempts();
    assert!(
        first >= 2,
        "§12 B1: error 42 names the backend that answered, so a single attempt is not an \
         answer about the block; only {first} attempt(s) were made"
    );
    assert!(
        snapshot_files(&dir.path().join("feed")).is_empty(),
        "nothing grounds a snapshot yet, so nothing may be published"
    );

    // Next cycle, and this time the aggregator routes us to a backend with
    // archive tries. The proof is obtainable now — but only if it is ASKED
    // for again.
    fixture.stop_refusing();
    cutter.maybe_publish_snapshot().await.expect("publish on the second cycle");
    assert!(
        fixture.attempts() > first,
        "the basis probe was not re-attempted: after {first} attempts on the first cycle \
         the endpoint was never asked again, so one unlucky cycle would cost this \
         snapshot its primary grounding for good"
    );
    cutter.rewrite_manifest().unwrap();
    let entry = manifest_snapshot(&dir.path().join("feed"));
    assert_eq!(
        entry["grounding"].as_str(),
        Some("basis-anchor"),
        "the retry obtained the proof, so this snapshot is grounded on it: {entry}"
    );
    assert_eq!(entry["anchor"]["block"].as_u64(), Some(BASIS), "{entry}");
}

/// ...and the outer budget is BOUNDED. An endpoint that implements no proofs
/// at any height (publicnode) must not be asked once per poll interval for the
/// life of the process.
#[tokio::test]
async fn the_basis_probe_budget_is_spent_and_then_left_alone() {
    let dir = tempfile::tempdir().unwrap();
    let (db, cfg) = gated_mirror(dir.path(), None);
    let fixture = ProofFixture::new(usize::MAX, honest_basis_root());
    let url = fixture.serve().await;
    let rpc = RpcClient::new(url, None);
    let cutter = Cutter {
        db: &db,
        rpc: &rpc,
        cfg: &cfg,
        feed_dir: dir.path().join("feed"),
    };
    cutter.ensure_layout().unwrap();

    let budget = strk20_indexerd::cutter::BASIS_PROBE_ATTEMPTS;
    for _ in 0..budget {
        cutter.maybe_publish_snapshot().await.expect("refusals are not errors");
    }
    let spent = fixture.attempts();
    assert!(spent > 0, "vacuity guard: the endpoint was never asked");
    for _ in 0..3 {
        cutter.maybe_publish_snapshot().await.expect("refusals are not errors");
    }
    assert_eq!(
        fixture.attempts(),
        spent,
        "after {budget} cycles the basis probe must stop: an endpoint that answers 42 at \
         every height would otherwise be asked forever"
    );
}
