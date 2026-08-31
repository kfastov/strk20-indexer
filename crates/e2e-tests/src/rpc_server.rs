//! In-process fixture JSON-RPC server (spec §10.3 topology): serves the six
//! Starknet methods the indexer ingests from, backed by a mutable
//! FixtureChain. Forces getEvents page size to 2 to exercise pagination.
//! Captures every request body for the server-side no-key scan.
//!
//! `FaultSpec` reproduces the provider behaviours measured on live networks
//! (docs/research/live/live-run-findings.md): lava's nondeterministic pruned
//! backends, pathfinder's sliding storage-proof window, publicnode's total
//! lack of storage proofs, and 429 throttling. Every fault is BUDGETED, never
//! random, so the fixture stays deterministic.

use crate::chain::FixtureChain;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};
use starknet_types_core::felt::Felt;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use strk20_feed::felt_hex;

pub const FORCED_CHUNK: usize = 2;

/// Error code pathfinder/juno answer with when a block is outside the
/// storage-proof retention window (measured on lava mainnet: ~1024 blocks).
pub const PROOF_TOO_OLD_CODE: i64 = 42;
pub const PROOF_TOO_OLD_MESSAGE: &str =
    "the node doesn't support storage proofs for blocks that are too far in the past";

/// Injectable provider misbehaviour. All fields default to "well-behaved".
#[derive(Debug, Clone, Default)]
pub struct FaultSpec {
    /// getEvents requests reaching below this block answer the lava
    /// pruned-history error (JSON-RPC -32603) while `pruned_budget` lasts.
    pub pruned_floor: Option<u64>,
    pub pruned_budget: usize,
    /// getStorageProof succeeds only within this many blocks of head.
    pub proof_window: Option<u64>,
    /// getStorageProof answers code 42 at EVERY height (publicnode class).
    pub proofs_unsupported: bool,
    /// The first N requests of any method answer HTTP 429.
    pub throttle_first: usize,
    /// A slot present in the chain's storage root that no state update ever
    /// exposes — a silent write the mirror can never learn of.
    pub hidden_slot: Option<(Felt, Felt)>,
}

#[derive(Debug)]
pub struct FaultCounters {
    requests: AtomicUsize,
    pruned_budget: AtomicUsize,
    pruned_errors: AtomicUsize,
    throttle_budget: AtomicUsize,
    throttled: AtomicUsize,
    proofs_denied: AtomicUsize,
}

impl FaultCounters {
    fn new(spec: &FaultSpec) -> Self {
        Self {
            requests: AtomicUsize::new(0),
            pruned_budget: AtomicUsize::new(spec.pruned_budget),
            pruned_errors: AtomicUsize::new(0),
            throttle_budget: AtomicUsize::new(spec.throttle_first),
            throttled: AtomicUsize::new(0),
            proofs_denied: AtomicUsize::new(0),
        }
    }
}

fn take(budget: &AtomicUsize) -> bool {
    budget
        .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |v| {
            if v > 0 {
                Some(v - 1)
            } else {
                None
            }
        })
        .is_ok()
}

#[derive(Clone)]
pub struct FixtureRpc {
    pub chain: Arc<RwLock<FixtureChain>>,
    pub captured: Arc<Mutex<Vec<u8>>>,
    pub chain_id: String,
    pub faults: Arc<FaultSpec>,
    pub counters: Arc<FaultCounters>,
    /// Runtime override of `FaultSpec::proofs_unsupported`, so a test can
    /// model an endpoint gaining or losing the storage-proof capability while
    /// the indexer runs (LIVE-6: capability is per-endpoint and not stable).
    proofs_supported: Arc<AtomicBool>,
}

impl FixtureRpc {
    pub fn new(chain: FixtureChain, chain_id: &str) -> Self {
        Self::with_faults(chain, chain_id, FaultSpec::default())
    }

    pub fn with_faults(chain: FixtureChain, chain_id: &str, faults: FaultSpec) -> Self {
        let counters = FaultCounters::new(&faults);
        let proofs_supported = Arc::new(AtomicBool::new(!faults.proofs_unsupported));
        Self {
            chain: Arc::new(RwLock::new(chain)),
            captured: Arc::new(Mutex::new(Vec::new())),
            chain_id: chain_id.to_owned(),
            faults: Arc::new(faults),
            counters: Arc::new(counters),
            proofs_supported,
        }
    }

    /// Turn `starknet_getStorageProof` support on or off at runtime.
    pub fn set_proofs_supported(&self, on: bool) {
        self.proofs_supported.store(on, Ordering::SeqCst);
    }

    pub fn router(&self) -> Router {
        Router::new().route("/", post(handle)).with_state(self.clone())
    }

    pub async fn serve(&self) -> std::net::SocketAddr {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let router = self.router();
        tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        addr
    }

    /// Total requests this endpoint received (failover detector).
    pub fn request_count(&self) -> usize {
        self.counters.requests.load(Ordering::SeqCst)
    }
    pub fn pruned_errors(&self) -> usize {
        self.counters.pruned_errors.load(Ordering::SeqCst)
    }
    pub fn throttled(&self) -> usize {
        self.counters.throttled.load(Ordering::SeqCst)
    }
    pub fn proofs_denied(&self) -> usize {
        self.counters.proofs_denied.load(Ordering::SeqCst)
    }
}

fn ok(id: Value, result: Value) -> Response {
    Json(json!({"jsonrpc": "2.0", "id": id, "result": result})).into_response()
}

fn rpc_err(id: Value, code: i64, message: &str) -> Response {
    Json(json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}))
        .into_response()
}

fn rpc_err_data(id: Value, code: i64, message: &str, data: Value) -> Response {
    Json(json!({
        "jsonrpc": "2.0", "id": id,
        "error": {"code": code, "message": message, "data": data}
    }))
    .into_response()
}

fn short_string_felt(s: &str) -> Felt {
    let bytes = s.as_bytes();
    let mut arr = [0u8; 32];
    arr[32 - bytes.len()..].copy_from_slice(bytes);
    Felt::from_bytes_be(&arr)
}

fn block_id_number(chain: &FixtureChain, v: &Value) -> Option<u64> {
    if let Some(n) = v.get("block_number").and_then(Value::as_u64) {
        return Some(n);
    }
    match v.as_str() {
        Some("latest") | Some("pre_confirmed") => Some(chain.head),
        Some("l1_accepted") => Some(chain.l1_accepted),
        _ => None,
    }
}

async fn handle(State(rpc): State<FixtureRpc>, body: axum::body::Bytes) -> Response {
    rpc.counters.requests.fetch_add(1, Ordering::SeqCst);
    rpc.captured.lock().unwrap().extend_from_slice(&body);
    if take(&rpc.counters.throttle_budget) {
        rpc.counters.throttled.fetch_add(1, Ordering::SeqCst);
        return (StatusCode::TOO_MANY_REQUESTS, "rate limited").into_response();
    }
    let req: Value = match serde_json::from_slice(&body) {
        Ok(v) => v,
        Err(_) => return rpc_err(Value::Null, -32700, "parse error"),
    };
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(Value::Null);
    let chain = rpc.chain.read().unwrap();

    match method {
        "starknet_chainId" => ok(id, json!(felt_hex(&short_string_felt(&rpc.chain_id)))),
        "starknet_blockNumber" => ok(id, json!(chain.head)),
        "starknet_getBlockWithTxHashes" => {
            let Some(n) = params.get(0).and_then(|v| block_id_number(&chain, v)) else {
                return rpc_err(id, 24, "Block not found");
            };
            if n > chain.head {
                return rpc_err(id, 24, "Block not found");
            }
            let txs: Vec<String> = chain
                .active
                .get(&n)
                .map(|b| {
                    (0..b.events.len())
                        .map(|i| felt_hex(&chain.tx_hash(n, i)))
                        .collect()
                })
                .unwrap_or_default();
            ok(
                id,
                json!({
                    "block_number": n,
                    "block_hash": felt_hex(&chain.block_hash(n)),
                    "parent_hash": felt_hex(&chain.parent_hash(n)),
                    "timestamp": chain.timestamp(n),
                    "new_root": "0x0",
                    "status": if n <= chain.l1_accepted { "ACCEPTED_ON_L1" } else { "ACCEPTED_ON_L2" },
                    "sequencer_address": "0x1",
                    "starknet_version": "0.14.0",
                    "l1_da_mode": "BLOB",
                    "transactions": txs,
                }),
            )
        }
        "starknet_getEvents" => {
            let filter = params.get(0).cloned().unwrap_or_default();
            let from = filter
                .get("from_block")
                .and_then(|v| block_id_number(&chain, v))
                .unwrap_or(0);
            let to = filter
                .get("to_block")
                .and_then(|v| block_id_number(&chain, v))
                .unwrap_or(chain.head)
                .min(chain.head);
            // Aggregator routed us to a pruned backend: the SAME request may
            // succeed on the next attempt.
            if let Some(floor) = rpc.faults.pruned_floor {
                if from < floor && take(&rpc.counters.pruned_budget) {
                    rpc.counters.pruned_errors.fetch_add(1, Ordering::SeqCst);
                    return rpc_err_data(
                        id,
                        -32603,
                        "Internal error",
                        json!(format!(
                            "block {from} has been pruned; oldest retained block is {floor}"
                        )),
                    );
                }
            }
            let address = filter.get("address").and_then(Value::as_str);
            if let Some(a) = address {
                if Felt::from_hex(a).ok() != Some(chain.pool) {
                    return ok(id, json!({"events": []}));
                }
            }
            // flatten (block, idx) pairs in range
            let mut all: Vec<(u64, usize)> = Vec::new();
            for (n, b) in chain.active.range(from..=to) {
                for i in 0..b.events.len() {
                    all.push((*n, i));
                }
            }
            // continuation "<block>-<idx>" = first item of THIS page
            let start = match filter.get("continuation_token").and_then(Value::as_str) {
                Some(tok) => {
                    let (b, i) = tok.split_once('-').unwrap_or(("0", "0"));
                    let key = (b.parse().unwrap_or(0), i.parse().unwrap_or(0));
                    all.iter().position(|x| *x >= key).unwrap_or(all.len())
                }
                None => 0,
            };
            let page: Vec<Value> = all[start..]
                .iter()
                .take(FORCED_CHUNK)
                .map(|(n, i)| {
                    let b = &chain.active[n];
                    let e = &b.events[*i];
                    json!({
                        "from_address": felt_hex(&chain.pool),
                        "keys": e.keys.iter().map(felt_hex).collect::<Vec<_>>(),
                        "data": e.data.iter().map(felt_hex).collect::<Vec<_>>(),
                        "block_hash": felt_hex(&chain.block_hash(*n)),
                        "block_number": n,
                        "transaction_hash": felt_hex(&chain.tx_hash(*n, *i)),
                    })
                })
                .collect();
            let next = start + page.len();
            let mut result = json!({"events": page});
            if next < all.len() {
                let (b, i) = all[next];
                result["continuation_token"] = json!(format!("{b}-{i}"));
            }
            ok(id, result)
        }
        "starknet_getStateUpdate" => {
            let Some(n) = params.get(0).and_then(|v| block_id_number(&chain, v)) else {
                return rpc_err(id, 24, "Block not found");
            };
            if n > chain.head {
                return rpc_err(id, 24, "Block not found");
            }
            let blk = chain.active.get(&n);
            let storage_diffs: Vec<Value> = match blk {
                Some(b) if !b.diffs.is_empty() => vec![json!({
                    "address": felt_hex(&chain.pool),
                    "storage_entries": b.diffs.iter().map(|(k, v)| json!({
                        "key": felt_hex(k), "value": felt_hex(v)
                    })).collect::<Vec<_>>(),
                })],
                _ => vec![],
            };
            let deployed: Vec<Value> = blk
                .and_then(|b| b.deployed_class.as_ref())
                .map(|c| vec![json!({"address": felt_hex(&chain.pool), "class_hash": felt_hex(c)})])
                .unwrap_or_default();
            let replaced: Vec<Value> = blk
                .and_then(|b| b.replaced_class.as_ref())
                .map(|c| {
                    vec![json!({"contract_address": felt_hex(&chain.pool), "class_hash": felt_hex(c)})]
                })
                .unwrap_or_default();
            ok(
                id,
                json!({
                    "block_hash": felt_hex(&chain.block_hash(n)),
                    "new_root": "0x0",
                    "old_root": "0x0",
                    "state_diff": {
                        "storage_diffs": storage_diffs,
                        "nonces": [],
                        "deployed_contracts": deployed,
                        "replaced_classes": replaced,
                        "declared_classes": [],
                        "deprecated_declared_classes": [],
                    }
                }),
            )
        }
        "starknet_getClassHashAt" => {
            let Some(n) = params.get(0).and_then(|v| block_id_number(&chain, v)) else {
                return rpc_err(id, 24, "Block not found");
            };
            match chain.class_at(n) {
                Some(c) => ok(id, json!(felt_hex(&c))),
                None => rpc_err(id, 20, "Contract not found"),
            }
        }
        "starknet_getStorageProof" => {
            let Some(n) = params.get(0).and_then(|v| block_id_number(&chain, v)) else {
                return rpc_err(id, 24, "Block not found");
            };
            if !rpc.proofs_supported.load(Ordering::SeqCst) {
                rpc.counters.proofs_denied.fetch_add(1, Ordering::SeqCst);
                return rpc_err(id, PROOF_TOO_OLD_CODE, PROOF_TOO_OLD_MESSAGE);
            }
            if let Some(window) = rpc.faults.proof_window {
                if chain.head.saturating_sub(n) > window {
                    rpc.counters.proofs_denied.fetch_add(1, Ordering::SeqCst);
                    return rpc_err(id, PROOF_TOO_OLD_CODE, PROOF_TOO_OLD_MESSAGE);
                }
            }
            let mut set = chain.state_at(n);
            if let Some((slot, value)) = rpc.faults.hidden_slot {
                set.push((slot, value));
            }
            let root = strk20_feed::mpt::storage_root(&set);
            let class = chain.class_at(n).unwrap_or(Felt::ZERO);
            ok(
                id,
                json!({
                    "classes_proof": [],
                    "contracts_proof": {
                        "nodes": [],
                        "contract_leaves_data": [{
                            "nonce": "0x0",
                            "class_hash": felt_hex(&class),
                            "storage_root": felt_hex(&root),
                        }]
                    },
                    "contracts_storage_proofs": [[]],
                    "global_roots": {
                        "contracts_tree_root": "0x0",
                        "classes_tree_root": "0x0",
                        "block_hash": felt_hex(&chain.block_hash(n)),
                    }
                }),
            )
        }
        "starknet_getStorageAt" => {
            let slot = params
                .get(1)
                .and_then(Value::as_str)
                .and_then(|s| Felt::from_hex(s).ok())
                .unwrap_or(Felt::ZERO);
            let n = params
                .get(2)
                .and_then(|v| block_id_number(&chain, v))
                .unwrap_or(chain.head);
            let value = chain
                .state_at(n)
                .iter()
                .find(|(s, _)| *s == slot)
                .map(|(_, v)| *v)
                .unwrap_or(Felt::ZERO);
            ok(id, json!(felt_hex(&value)))
        }
        other => rpc_err(id, -32601, &format!("method not found: {other}")),
    }
}
