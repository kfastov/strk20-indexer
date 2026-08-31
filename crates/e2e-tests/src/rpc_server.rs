//! In-process fixture JSON-RPC server (spec §10.3 topology): serves the six
//! Starknet methods the indexer ingests from, backed by a mutable
//! FixtureChain. Captures every request body for the server-side no-key scan.
//!
//! `FaultSpec` reproduces the provider behaviours measured on live networks
//! (docs/research/live/live-run-findings.md): lava's nondeterministic pruned
//! backends, publicnode's total lack of storage proofs, 429 throttling, the
//! aggregator's per-request routing of storage proofs (§12), and — LIVE-8 —
//! a continuation token handed to a backend that did not issue it. Every
//! fault is BUDGETED or deterministic, never random.
//!
//! **Page size (changed for LIVE-8).** The fixture used to force every
//! getEvents page to 2 events regardless of `chunk_size`, to exercise the
//! token-following scan loop. LIVE-8 says that loop must not exist: a token is
//! node-local state and following one across requests silently loses events,
//! so the indexer has to subdivide the block range until each window is
//! answered in ONE page. A cap the caller cannot raise would make that
//! impossible for any block holding more events than the cap (the devnet
//! fixture's block 20 holds three), so the fixture now honours the requested
//! `chunk_size` up to a provider cap — lava's documented 1000 by default, and
//! `FaultSpec::max_page` when a test needs a window that CANNOT be reduced.

use crate::chain::FixtureChain;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde_json::{json, Value};
use starknet_types_core::felt::Felt;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};
use strk20_feed::felt_hex;

/// Provider cap on events per getEvents page (lava's documented maximum).
/// The fixture serves `min(requested chunk_size, max_page)`.
pub const DEFAULT_MAX_PAGE: usize = 1000;

/// How many events a foreign backend skips when it is handed a continuation
/// token it never issued (LIVE-8). Measured behaviour is "resumes from
/// somewhere else, silently"; the fixture picks a fixed somewhere else so the
/// loss is deterministic and countable.
pub const FOREIGN_TOKEN_SKIP: usize = 3;

/// Error code a backend answers with when IT cannot serve a storage proof for
/// the requested block — no archive trie, or no proof support at all. The
/// "~1024-block window" this project once measured is retracted
/// (proof-window.md §3): the code names the backend that answered, not the
/// block.
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
    /// getStorageProof succeeds only within this many blocks of head — a node
    /// with a genuinely narrow trie retention, and a way for a test to make a
    /// given block unprovable by construction. Not a model of the aggregator:
    /// that is `proof_flaky_attempts`, where depth is irrelevant and the
    /// answer depends on which backend replied.
    pub proof_window: Option<u64>,
    /// getStorageProof answers code 42 at EVERY height (publicnode class).
    pub proofs_unsupported: bool,
    /// The first N requests of any method answer HTTP 429.
    pub throttle_first: usize,
    /// A slot present in the chain's storage root that no state update ever
    /// exposes — a silent write the mirror can never learn of.
    pub hidden_slot: Option<(Felt, Felt)>,
    /// Provider cap on events per getEvents page; `None` = `DEFAULT_MAX_PAGE`.
    /// Set it below the event count of a single block to make that block's
    /// window IRREDUCIBLE: no subdivision can get it answered in one page.
    pub max_page: Option<usize>,
    /// LIVE-8: every continuation token presented is answered by a DIFFERENT
    /// backend than the one that issued it. The answer is a plausible page
    /// from the WRONG offset — `FOREIGN_TOKEN_SKIP` events are dropped — and
    /// carries no error, which is exactly why the mirror lost 139 blocks
    /// without anything noticing.
    pub foreign_token: bool,
    /// getStorageProof answers code 42 for the first N attempts AT EACH BLOCK
    /// and succeeds afterwards: the aggregator routes each call to a different
    /// backend and only some run archive tries (proof-window.md §1). Error 42
    /// names the BACKEND, not the block — which is what makes a bounded retry
    /// the right response. Distinct from `proofs_unsupported`, which means
    /// "this endpoint never serves a proof, at any height, ever".
    pub proof_flaky_attempts: usize,
    /// The first N `getEvents` answers that would otherwise have been complete
    /// come back SHORT — fewer events than the requested `chunk_size` — and
    /// carrying a continuation token anyway. `chunk_size` is a maximum in the
    /// JSON-RPC spec, and a provider is free to stop early on an internal
    /// budget (a scanned-block-range limit is the usual one), so a token is not
    /// proof that the page was filled. Distinct from `max_page`, which is a
    /// page limit expressed in events: this one carries no information about
    /// event density at all, and treating it as if it did collapses every
    /// later window.
    pub range_budget_tokens: usize,
    /// A proof whose `global_roots.block_hash` is not the block's hash while
    /// its `storage_root` is honest: the anonymous, load-balanced proof pool
    /// answering for something other than the block we asked about. Only the
    /// §12 chain binding catches it.
    pub lying_proof: bool,
}

#[derive(Debug)]
pub struct FaultCounters {
    requests: AtomicUsize,
    pruned_budget: AtomicUsize,
    pruned_errors: AtomicUsize,
    throttle_budget: AtomicUsize,
    throttled: AtomicUsize,
    proofs_denied: AtomicUsize,
    /// getEvents responses that carried a continuation_token.
    tokens_issued: AtomicUsize,
    /// getEvents requests that PRESENTED a continuation_token. Under LIVE-8
    /// this is the number the indexer must keep at zero.
    tokens_presented: AtomicUsize,
    /// Pages served from the wrong offset because a token was presented.
    foreign_pages: AtomicUsize,
    /// Remaining `range_budget_tokens` budget, and how much of it was spent.
    range_budget: AtomicUsize,
    short_token_pages: AtomicUsize,
    /// Every (from, to) getEvents window asked for, in order — the record of
    /// whether the scan subdivided or asked one big question.
    event_windows: Mutex<Vec<(u64, u64)>>,
    /// getStorageProof requests seen per block, counted before any refusal
    /// (drives `proof_flaky_attempts`).
    proof_attempts: Mutex<BTreeMap<u64, usize>>,
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
            tokens_issued: AtomicUsize::new(0),
            tokens_presented: AtomicUsize::new(0),
            foreign_pages: AtomicUsize::new(0),
            range_budget: AtomicUsize::new(spec.range_budget_tokens),
            short_token_pages: AtomicUsize::new(0),
            event_windows: Mutex::new(Vec::new()),
            proof_attempts: Mutex::new(BTreeMap::new()),
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
    /// Runtime override of `FaultSpec::lying_proof`, so a mirror can be built
    /// honestly and only then asked for a proof that does not belong to the
    /// block it names.
    lying_proof: Arc<AtomicBool>,
}

impl FixtureRpc {
    pub fn new(chain: FixtureChain, chain_id: &str) -> Self {
        Self::with_faults(chain, chain_id, FaultSpec::default())
    }

    pub fn with_faults(chain: FixtureChain, chain_id: &str, faults: FaultSpec) -> Self {
        let counters = FaultCounters::new(&faults);
        let proofs_supported = Arc::new(AtomicBool::new(!faults.proofs_unsupported));
        let lying_proof = Arc::new(AtomicBool::new(faults.lying_proof));
        Self {
            chain: Arc::new(RwLock::new(chain)),
            captured: Arc::new(Mutex::new(Vec::new())),
            chain_id: chain_id.to_owned(),
            faults: Arc::new(faults),
            counters: Arc::new(counters),
            proofs_supported,
            lying_proof,
        }
    }

    /// Turn `starknet_getStorageProof` support on or off at runtime.
    pub fn set_proofs_supported(&self, on: bool) {
        self.proofs_supported.store(on, Ordering::SeqCst);
    }

    /// Start (or stop) answering proofs whose `global_roots.block_hash` is not
    /// the requested block's.
    pub fn set_lying_proof(&self, on: bool) {
        self.lying_proof.store(on, Ordering::SeqCst);
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
    /// getEvents responses that carried a continuation token.
    pub fn tokens_issued(&self) -> usize {
        self.counters.tokens_issued.load(Ordering::SeqCst)
    }
    /// getEvents requests that presented a continuation token (LIVE-8: the
    /// indexer must never present one).
    pub fn tokens_presented(&self) -> usize {
        self.counters.tokens_presented.load(Ordering::SeqCst)
    }
    /// Pages answered from the wrong offset because a token was presented.
    pub fn foreign_pages(&self) -> usize {
        self.counters.foreign_pages.load(Ordering::SeqCst)
    }
    /// Complete pages that were truncated and answered with a token anyway
    /// (`FaultSpec::range_budget_tokens`).
    pub fn short_token_pages(&self) -> usize {
        self.counters.short_token_pages.load(Ordering::SeqCst)
    }
    /// Every (from, to) getEvents window asked for, in request order.
    pub fn event_windows(&self) -> Vec<(u64, u64)> {
        self.counters.event_windows.lock().unwrap().clone()
    }
    /// getStorageProof requests seen at `block`, refused ones included.
    pub fn proof_attempts(&self, block: u64) -> usize {
        self.counters
            .proof_attempts
            .lock()
            .unwrap()
            .get(&block)
            .copied()
            .unwrap_or(0)
    }
    /// Forget every recorded proof attempt, so the next process meets a fresh
    /// `proof_flaky_attempts` budget and its retries can be counted exactly.
    pub fn reset_proof_attempts(&self) {
        self.counters.proof_attempts.lock().unwrap().clear();
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
            rpc.counters
                .event_windows
                .lock()
                .unwrap()
                .push((from, to));
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
            let mut start = match filter.get("continuation_token").and_then(Value::as_str) {
                Some(tok) => {
                    rpc.counters.tokens_presented.fetch_add(1, Ordering::SeqCst);
                    let (b, i) = tok.split_once('-').unwrap_or(("0", "0"));
                    let key = (b.parse().unwrap_or(0), i.parse().unwrap_or(0));
                    all.iter().position(|x| *x >= key).unwrap_or(all.len())
                }
                None => 0,
            };
            // LIVE-8: the token was issued by another backend in the pool.
            // This one resumes from somewhere else and says nothing about it.
            if rpc.faults.foreign_token && filter.get("continuation_token").is_some() {
                let wrong = (start + FOREIGN_TOKEN_SKIP).min(all.len());
                if wrong != start {
                    rpc.counters.foreign_pages.fetch_add(1, Ordering::SeqCst);
                }
                start = wrong;
            }
            let page_size = filter
                .get("chunk_size")
                .and_then(Value::as_u64)
                .unwrap_or(DEFAULT_MAX_PAGE as u64)
                .max(1)
                .min(rpc.faults.max_page.unwrap_or(DEFAULT_MAX_PAGE) as u64)
                as usize;
            let page: Vec<Value> = all[start..]
                .iter()
                .take(page_size)
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
            let mut page = page;
            // A range-budget token: the answer is cut short even though the
            // page is not full, and a token is issued for the remainder.
            if rpc.faults.range_budget_tokens > 0
                && !page.is_empty()
                && start + page.len() >= all.len()
                && page.len() < page_size
                && take(&rpc.counters.range_budget)
            {
                let keep = page.len().saturating_sub(1);
                page.truncate(keep);
                rpc.counters.short_token_pages.fetch_add(1, Ordering::SeqCst);
            }
            let next = start + page.len();
            let mut result = json!({"events": page});
            if next < all.len() {
                let (b, i) = all[next];
                result["continuation_token"] = json!(format!("{b}-{i}"));
                rpc.counters.tokens_issued.fetch_add(1, Ordering::SeqCst);
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
            // Counted for EVERY request, before any refusal: a test asking
            // "was a proof for this block ever sought" must not be answered
            // "only the ones that got past the fault gates below".
            let seen = {
                let mut attempts = rpc.counters.proof_attempts.lock().unwrap();
                let seen = attempts.entry(n).or_insert(0);
                *seen += 1;
                *seen
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
            // Aggregator routing: the first N attempts at THIS block reach a
            // backend without archive tries. The block is provable; this
            // answer is about the backend (proof-window.md §3).
            if seen <= rpc.faults.proof_flaky_attempts {
                rpc.counters.proofs_denied.fetch_add(1, Ordering::SeqCst);
                return rpc_err(id, PROOF_TOO_OLD_CODE, PROOF_TOO_OLD_MESSAGE);
            }
            let mut set = chain.state_at(n);
            if let Some((slot, value)) = rpc.faults.hidden_slot {
                set.push((slot, value));
            }
            let root = strk20_feed::mpt::storage_root(&set);
            let class = chain.class_at(n).unwrap_or(Felt::ZERO);
            // A proof from the anonymous pool that does not belong to the
            // block it names: the storage root is a real root, the block hash
            // is not this block's. Only the §12 chain binding separates the
            // two, and without it "retry until one succeeds" degenerates into
            // "accept whichever answer we liked".
            let proof_block_hash = if rpc.lying_proof.load(Ordering::SeqCst) {
                chain.block_hash(n) + Felt::ONE
            } else {
                chain.block_hash(n)
            };
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
                        "block_hash": felt_hex(&proof_block_hash),
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
