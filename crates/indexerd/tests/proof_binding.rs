//! §12 B2 chain binding, and the one distinction it has to make: "the chain
//! moved" versus "the pool lied".
//!
//! `bound_proof` compares a storage proof's `global_roots.block_hash` with the
//! block header's own hash. The two come from independent, independently routed
//! calls, and the block they are asked about is deliberately near head
//! (`verify_root_at_target` targets `min(frontier, rpc_head)`), thousands of
//! blocks above `l1_accepted`. At that depth a block number legitimately
//! changes hash: an ordinary one-block reorg landing between the two calls
//! makes them disagree without anything having lied.
//!
//! That matters because `PROOF_NOT_BOUND` is the loudest alarm in the system —
//! it halts the cut batch and is the only signal meaning "an endpoint answered
//! with a proof about some other block". A channel that also carries routine
//! reorg noise is a channel nobody can act on.

use starknet_types_core::felt::Felt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use strk20_indexerd::config::ChainConfig;
use strk20_indexerd::cutter::Cutter;
use strk20_indexerd::db::Db;
use strk20_indexerd::rpc::RpcClient;

const BLOCK: u64 = 4242;
/// The hash the header serves while `header_flips_after` calls remain.
const OLD_HASH: &str = "0xaaa1";
/// The hash the proof always names, and the header names once it has settled.
const NEW_HASH: &str = "0xbbb2";

/// Header endpoint that changes its mind once: the first `flip_after` answers
/// carry `OLD_HASH`, every answer after that carries `NEW_HASH`. The proof
/// endpoint always names `NEW_HASH`. With `flip_after = 1` that is a reorg
/// landing between the proof call and the header call; with a huge value it is
/// an endpoint serving a proof that genuinely belongs to another block.
#[derive(Clone)]
struct BindingFixture {
    flip_after: usize,
    headers: Arc<AtomicUsize>,
    proofs: Arc<AtomicUsize>,
}

impl BindingFixture {
    fn new(flip_after: usize) -> Self {
        Self {
            flip_after,
            headers: Arc::new(AtomicUsize::new(0)),
            proofs: Arc::new(AtomicUsize::new(0)),
        }
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
                    let result = match req["method"].as_str().unwrap_or_default() {
                        "starknet_getStorageProof" => {
                            me.proofs.fetch_add(1, Ordering::SeqCst);
                            serde_json::json!({
                                "classes_proof": [],
                                "contracts_proof": {
                                    "nodes": [],
                                    "contract_leaves_data": [{
                                        "nonce": "0x0",
                                        "class_hash": "0x1",
                                        "storage_root": "0x2",
                                    }]
                                },
                                "contracts_storage_proofs": [[]],
                                "global_roots": {"block_hash": NEW_HASH},
                            })
                        }
                        "starknet_getBlockWithTxHashes" => {
                            let seen = me.headers.fetch_add(1, Ordering::SeqCst);
                            let hash = if seen < me.flip_after { OLD_HASH } else { NEW_HASH };
                            serde_json::json!({
                                "block_number": BLOCK,
                                "block_hash": hash,
                                "parent_hash": "0x1",
                                "timestamp": 1_700_000_000u64,
                                "status": "ACCEPTED_ON_L2",
                                "new_root": "0x0",
                                "transactions": [],
                            })
                        }
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

fn cfg() -> ChainConfig {
    let mut c = ChainConfig::mainnet();
    c.chain_id = "SN_TEST".to_owned();
    c.pool = Felt::from_hex("0x0f001").unwrap();
    c
}

/// A reorg between the two calls must NOT be reported as a lying proof pool.
#[tokio::test]
async fn a_reorg_between_the_proof_and_the_header_is_re_tested_not_alarmed() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("strk20.db")).unwrap();
    let fixture = BindingFixture::new(1);
    let url = fixture.serve().await;
    let rpc = RpcClient::new(url, None);
    let cfg = cfg();
    let cutter = Cutter {
        db: &db,
        rpc: &rpc,
        cfg: &cfg,
        feed_dir: dir.path().join("feed"),
    };

    let (proof, _raw) = cutter
        .bound_proof(BLOCK)
        .await
        .expect("a chain that moved between two independent calls is not a lying endpoint");
    assert_eq!(
        proof.global_roots["block_hash"].as_str(),
        Some(NEW_HASH),
        "the accepted proof must be the one that agrees with the settled header"
    );
    assert!(
        fixture.headers.load(Ordering::SeqCst) >= 2,
        "vacuity guard: the header was fetched {} time(s), so the first answer never \
         disagreed and this leg proved nothing",
        fixture.headers.load(Ordering::SeqCst)
    );
    assert!(
        fixture.proofs.load(Ordering::SeqCst) >= 2,
        "the proof must be re-fetched too: keeping the first proof and re-reading only the \
         header would accept a proof that was never re-tested against the chain"
    );
}

/// ...and a disagreement that SURVIVES the re-test is still the hard error it
/// has to be. This is the §12 B2 property itself: without it, retry-until-
/// success is indistinguishable from accepting whichever answer we liked.
#[tokio::test]
async fn a_disagreement_that_survives_the_re_test_is_a_hard_error() {
    let dir = tempfile::tempdir().unwrap();
    let db = Db::open(&dir.path().join("strk20.db")).unwrap();
    let fixture = BindingFixture::new(usize::MAX);
    let url = fixture.serve().await;
    let rpc = RpcClient::new(url, None);
    let cfg = cfg();
    let cutter = Cutter {
        db: &db,
        rpc: &rpc,
        cfg: &cfg,
        feed_dir: dir.path().join("feed"),
    };

    let err = cutter
        .bound_proof(BLOCK)
        .await
        .expect_err("a proof about another block must never be accepted");
    let text = format!("{err:#}");
    assert!(
        strk20_indexerd::rpc::is_proof_unbound(&err),
        "the failure must be classified as PROOF_NOT_BOUND — filing it as a capability gap \
         would hide a lie behind LIVE-6: {text}"
    );
    assert!(
        !strk20_indexerd::rpc::is_proof_unavailable(&err),
        "an endpoint that ANSWERS has not had a capability gap: {text}"
    );
    assert!(
        text.contains(NEW_HASH) && text.contains(OLD_HASH),
        "the operator needs both hashes to tell a reorg from a lie: {text}"
    );
    assert_eq!(
        fixture.proofs.load(Ordering::SeqCst),
        2,
        "the re-test is a single bounded retry, not a loop: an endpoint that always \
         disagrees must be given up on"
    );
}
