//! In-process fixture JSON-RPC server (spec §10.3 topology): serves the six
//! Starknet methods the indexer ingests from, backed by a mutable
//! FixtureChain. Forces getEvents page size to 2 to exercise pagination.
//! Captures every request body for the server-side no-key scan.

use crate::chain::FixtureChain;
use axum::extract::State;
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};
use starknet_types_core::felt::Felt;
use std::sync::{Arc, Mutex, RwLock};
use strk20_feed::felt_hex;

pub const FORCED_CHUNK: usize = 2;

#[derive(Clone)]
pub struct FixtureRpc {
    pub chain: Arc<RwLock<FixtureChain>>,
    pub captured: Arc<Mutex<Vec<u8>>>,
    pub chain_id: String,
}

impl FixtureRpc {
    pub fn new(chain: FixtureChain, chain_id: &str) -> Self {
        Self {
            chain: Arc::new(RwLock::new(chain)),
            captured: Arc::new(Mutex::new(Vec::new())),
            chain_id: chain_id.to_owned(),
        }
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
}

fn ok(id: Value, result: Value) -> Json<Value> {
    Json(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

fn rpc_err(id: Value, code: i64, message: &str) -> Json<Value> {
    Json(json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}}))
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

async fn handle(State(rpc): State<FixtureRpc>, body: axum::body::Bytes) -> Json<Value> {
    rpc.captured.lock().unwrap().extend_from_slice(&body);
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
            let set = chain.state_at(n);
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
