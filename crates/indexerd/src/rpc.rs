//! Minimal Starknet JSON-RPC client for the five ingest methods (spec §5.3).
//! Plain reqwest + our own serde structs; no starknet-providers. Primary /
//! fallback endpoints with consecutive-failure failover and 429 backoff.

use anyhow::{anyhow, bail, Context, Result};
use serde::Deserialize;
use serde_json::{json, Value};
use starknet_types_core::felt::Felt;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

const USER_AGENT: &str = concat!("strk20-indexer/", env!("CARGO_PKG_VERSION"));
const FAILOVER_AFTER: usize = 5;
/// Transport-failure attempts before a call gives up (unchanged).
const TRANSPORT_ATTEMPTS: usize = 8;
/// Transport-failure attempts for a storage proof. Proofs already retry across
/// the whole endpoint list (`proof_order`), and an unverified batch is retried
/// on the next ingest cycle anyway, so burning the full 8-attempt exponential
/// budget on a dead endpoint only delays reaching a live one.
const PROOF_TRANSPORT_ATTEMPTS: usize = 2;
/// Provider-capability answers (pruned history) retried in place before a call
/// gives up. Lava is an aggregator: the same request is routed to an archive
/// or a pruned backend nondeterministically, so the retry is on the SAME
/// endpoint and must be bounded (docs/research/live/live-run-findings.md
/// LIVE-1).
const CAPABILITY_RETRIES: usize = 5;
/// HTTP 429 answers absorbed in place per call. Throttling is not a failure of
/// the endpoint (LIVE-3): counting it toward failover flips a deep backfill
/// onto an endpoint that cannot serve the range at all.
const THROTTLE_RETRIES: usize = 12;
const THROTTLE_MAX_DELAY: Duration = Duration::from_secs(1);

/// Whole error chain as text: classification must see the JSON-RPC body even
/// when a caller wrapped the error in `.context(..)`.
fn error_text(e: &anyhow::Error) -> String {
    format!("{e:#}")
}

/// A JSON-RPC error saying the endpoint no longer retains the requested
/// history. Provider capability, not semantics: retryable.
pub fn is_pruned_history(e: &anyhow::Error) -> bool {
    let msg = error_text(e);
    msg.contains("has been pruned") || msg.contains("oldest retained block")
}

/// A JSON-RPC error saying this endpoint cannot serve a storage proof for the
/// requested block — pathfinder's sliding trie window (code 42), or a provider
/// that does not implement proofs at all. Never evidence about the mirror.
pub fn is_proof_unavailable(e: &anyhow::Error) -> bool {
    let msg = error_text(e);
    msg.contains("too far in the past")
        || (msg.contains("starknet_getStorageProof") && msg.contains("\"code\":42"))
}

/// An endpoint that ran out of retry budget — transport failures or sustained
/// throttling — as opposed to a JSON-RPC answer. Says nothing about the
/// request itself, so a caller holding other endpoints should ask one of them.
pub fn is_endpoint_exhausted(e: &anyhow::Error) -> bool {
    let msg = error_text(e);
    msg.contains("failed after retries") || msg.contains("still rate limited")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockRef {
    Number(u64),
    Latest,
    L1Accepted,
}

impl BlockRef {
    fn to_json(self) -> Value {
        match self {
            BlockRef::Number(n) => json!({ "block_number": n }),
            BlockRef::Latest => json!("latest"),
            BlockRef::L1Accepted => json!("l1_accepted"),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct RpcEvent {
    pub from_address: String,
    pub keys: Vec<String>,
    pub data: Vec<String>,
    pub block_number: Option<u64>,
    pub block_hash: Option<String>,
    pub transaction_hash: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct EventsPage {
    pub events: Vec<RpcEvent>,
    pub continuation_token: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StorageEntry {
    pub key: String,
    pub value: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ContractStorageDiff {
    pub address: String,
    pub storage_entries: Vec<StorageEntry>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct DeployedContract {
    pub address: String,
    pub class_hash: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ReplacedClass {
    pub contract_address: String,
    pub class_hash: String,
}

#[derive(Debug, Clone, Deserialize, Default)]
pub struct StateDiff {
    #[serde(default)]
    pub storage_diffs: Vec<ContractStorageDiff>,
    #[serde(default)]
    pub deployed_contracts: Vec<DeployedContract>,
    #[serde(default)]
    pub replaced_classes: Vec<ReplacedClass>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StateUpdate {
    pub block_hash: Option<String>,
    pub new_root: Option<String>,
    pub state_diff: StateDiff,
}

#[derive(Debug, Clone, Deserialize)]
pub struct BlockHeader {
    pub block_number: u64,
    pub block_hash: String,
    pub parent_hash: String,
    pub timestamp: u64,
    pub status: Option<String>,
    pub new_root: Option<String>,
    #[serde(default)]
    pub transactions: Vec<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ContractLeafData {
    pub class_hash: String,
    pub storage_root: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ContractsProof {
    pub contract_leaves_data: Vec<ContractLeafData>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct StorageProof {
    pub contracts_proof: ContractsProof,
    #[serde(default)]
    pub contracts_storage_proofs: Vec<Vec<Value>>,
    pub global_roots: Value,
}

/// What one endpoint has been observed to be able to do (LIVE-6). Learned at
/// runtime only: providers do not advertise it, and it is never evidence about
/// the mirror — a capability gap must not degrade health.
#[derive(Debug, Default)]
struct EndpointCaps {
    proofs_served: AtomicUsize,
    proofs_denied: AtomicUsize,
}

pub struct RpcClient {
    http: reqwest::Client,
    endpoints: Vec<String>,
    caps: Vec<EndpointCaps>,
    active: AtomicUsize,
    consecutive_failures: AtomicUsize,
}

impl RpcClient {
    pub fn new(primary: String, fallback: Option<String>) -> Self {
        let mut endpoints = vec![primary];
        endpoints.extend(fallback);
        let caps = endpoints.iter().map(|_| EndpointCaps::default()).collect();
        Self {
            http: reqwest::Client::builder()
                .user_agent(USER_AGENT)
                .timeout(Duration::from_secs(30))
                .build()
                .expect("reqwest client"),
            endpoints,
            caps,
            active: AtomicUsize::new(0),
            consecutive_failures: AtomicUsize::new(0),
        }
    }

    fn active_index(&self) -> usize {
        self.active.load(Ordering::Relaxed) % self.endpoints.len()
    }

    pub fn active_endpoint(&self) -> &str {
        &self.endpoints[self.active_index()]
    }

    /// Endpoints ordered by their fitness to serve a storage proof: ones that
    /// have served a proof first, never-tried ones next, ones that have only
    /// ever refused last. Every endpoint stays in the list — a refusal at an
    /// out-of-window block says nothing about the next block.
    fn proof_order(&self) -> Vec<usize> {
        let active = self.active_index();
        let mut order: Vec<usize> = (0..self.endpoints.len()).collect();
        order.sort_by_key(|&i| {
            let served = self.caps[i].proofs_served.load(Ordering::Relaxed) > 0;
            let denied = self.caps[i].proofs_denied.load(Ordering::Relaxed) > 0;
            let rank = match (served, denied) {
                (true, _) => 0u8,
                (false, false) => 1,
                (false, true) => 2,
            };
            (rank, i != active)
        });
        order
    }

    /// Monotonic-ish marker of which endpoint is active; continuation tokens
    /// are provider-specific and must be dropped when this changes.
    pub fn endpoint_epoch(&self) -> usize {
        self.active.load(Ordering::Relaxed)
    }

    /// True when `e` is a JSON-RPC "Block not found"-class answer rather than
    /// a transport failure — the only errors reorg detection may act on.
    pub fn is_block_not_found(e: &anyhow::Error) -> bool {
        let msg = error_text(e);
        msg.contains("\"code\":24") || msg.contains("Block not found")
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value> {
        self.call_at(
            self.active_index(),
            method,
            &params,
            true,
            TRANSPORT_ATTEMPTS,
        )
        .await
    }

    /// One JSON-RPC call against endpoint `idx`, with three independent
    /// budgets: transport failures (which may fail over), provider-capability
    /// answers (retried in place — see `CAPABILITY_RETRIES`), and HTTP 429
    /// (backed off in place, never counted toward failover). Semantic
    /// JSON-RPC errors are fatal on the first answer.
    async fn call_at(
        &self,
        idx: usize,
        method: &str,
        params: &Value,
        allow_failover: bool,
        transport_attempts: usize,
    ) -> Result<Value> {
        let mut transport_left = transport_attempts;
        let mut capability_left = CAPABILITY_RETRIES;
        let mut throttle_left = THROTTLE_RETRIES;
        let mut throttle_escalated = false;
        let mut delay = Duration::from_millis(300);
        let mut throttle_delay = Duration::from_millis(300);
        loop {
            let url = if allow_failover {
                self.active_endpoint().to_owned()
            } else {
                self.endpoints[idx % self.endpoints.len()].clone()
            };
            let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
            let resp = self.http.post(&url).json(&body).send().await;
            match resp {
                Ok(r) if r.status() == reqwest::StatusCode::TOO_MANY_REQUESTS => {
                    // LIVE-3: throttling is pressure, not endpoint failure —
                    // it must never touch the consecutive-failure counter.
                    if throttle_left == 0 {
                        // ...but a permanently throttled endpoint (a spent
                        // daily quota answers 429 for hours) must not be fatal
                        // either, or an unattended backfill dies with a healthy
                        // fallback configured. One escalation per call, taken
                        // only once the in-place budget is spent, so this is
                        // still not "429 counts toward failover".
                        if allow_failover && !throttle_escalated && self.endpoints.len() > 1 {
                            let next =
                                (self.active.load(Ordering::Relaxed) + 1) % self.endpoints.len();
                            self.active.store(next, Ordering::Relaxed);
                            throttle_escalated = true;
                            throttle_left = THROTTLE_RETRIES;
                            throttle_delay = Duration::from_millis(300);
                            tracing::warn!(
                                from = %url, to = %self.endpoints[next], method,
                                "endpoint throttled for the whole backoff budget; escalating once"
                            );
                            continue;
                        }
                        bail!("rpc call {method} still rate limited by {url} after backoff");
                    }
                    throttle_left -= 1;
                    tracing::warn!(%url, method, "rate limited, backing off in place");
                    tokio::time::sleep(throttle_delay).await;
                    throttle_delay = (throttle_delay * 2).min(THROTTLE_MAX_DELAY);
                    continue;
                }
                Ok(r) => match r.error_for_status() {
                    Ok(ok) => {
                        let v: Value = ok.json().await.context("decode json-rpc response")?;
                        if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
                            // JSON-RPC level errors are not transport failures.
                            self.consecutive_failures.store(0, Ordering::Relaxed);
                            let e = anyhow!("rpc error from {method}: {err}");
                            if !is_pruned_history(&e) {
                                return Err(e);
                            }
                            if capability_left == 0 {
                                return Err(e.context(format!(
                                    "{method}: endpoint kept answering with pruned history \
                                     after {CAPABILITY_RETRIES} retries"
                                )));
                            }
                            capability_left -= 1;
                            tracing::warn!(
                                %url, method, error = %err,
                                "provider served pruned history; retrying"
                            );
                            tokio::time::sleep(delay).await;
                            delay = (delay * 2).min(Duration::from_secs(60));
                            continue;
                        }
                        self.consecutive_failures.store(0, Ordering::Relaxed);
                        return v
                            .get("result")
                            .cloned()
                            .ok_or_else(|| anyhow!("{method}: response has no result"));
                    }
                    Err(e) => {
                        tracing::warn!(%url, method, error = %e, "http error");
                    }
                },
                Err(e) => {
                    tracing::warn!(%url, method, error = %e, "transport error");
                }
            }
            transport_left -= 1;
            if allow_failover {
                let fails = self.consecutive_failures.fetch_add(1, Ordering::Relaxed) + 1;
                if fails >= FAILOVER_AFTER && self.endpoints.len() > 1 {
                    let next = (self.active.load(Ordering::Relaxed) + 1) % self.endpoints.len();
                    self.active.store(next, Ordering::Relaxed);
                    self.consecutive_failures.store(0, Ordering::Relaxed);
                    tracing::warn!(endpoint = %self.endpoints[next], "failing over rpc endpoint");
                }
            }
            if transport_left == 0 {
                bail!("rpc call {method} failed after retries on all endpoints");
            }
            tokio::time::sleep(delay).await;
            delay = (delay * 2).min(Duration::from_secs(60));
        }
    }

    pub async fn chain_id(&self) -> Result<String> {
        let v = self.call("starknet_chainId", json!([])).await?;
        let felt = v.as_str().ok_or_else(|| anyhow!("chainId not a string"))?;
        // decode short-string felt to ascii
        let f = Felt::from_hex(felt).map_err(|_| anyhow!("bad chain id felt"))?;
        let bytes = f.to_bytes_be();
        let ascii: Vec<u8> = bytes.into_iter().skip_while(|b| *b == 0).collect();
        Ok(String::from_utf8(ascii).unwrap_or_else(|_| felt.to_owned()))
    }

    pub async fn get_events(
        &self,
        address: &Felt,
        from_block: u64,
        to_block: BlockRef,
        chunk_size: u64,
        continuation: Option<&str>,
    ) -> Result<EventsPage> {
        let mut filter = json!({
            "from_block": { "block_number": from_block },
            "to_block": to_block.to_json(),
            "address": strk20_feed::felt_hex(address),
            "chunk_size": chunk_size,
        });
        if let Some(token) = continuation {
            filter["continuation_token"] = json!(token);
        }
        let v = self.call("starknet_getEvents", json!([filter])).await?;
        serde_json::from_value(v).context("decode getEvents")
    }

    pub async fn get_state_update(&self, block: u64) -> Result<StateUpdate> {
        let v = self
            .call("starknet_getStateUpdate", json!([{ "block_number": block }]))
            .await?;
        serde_json::from_value(v).context("decode getStateUpdate")
    }

    pub async fn get_block(&self, r: BlockRef) -> Result<BlockHeader> {
        let v = self
            .call("starknet_getBlockWithTxHashes", json!([r.to_json()]))
            .await?;
        serde_json::from_value(v).context("decode getBlockWithTxHashes")
    }

    pub async fn get_class_hash_at(&self, r: BlockRef, contract: &Felt) -> Result<Felt> {
        let v = self
            .call(
                "starknet_getClassHashAt",
                json!([r.to_json(), strk20_feed::felt_hex(contract)]),
            )
            .await?;
        let s = v.as_str().ok_or_else(|| anyhow!("class hash not a string"))?;
        Felt::from_hex(s).map_err(|_| anyhow!("bad class hash felt"))
    }

    pub async fn get_storage_proof(
        &self,
        r: BlockRef,
        contract: &Felt,
        keys: &[Felt],
    ) -> Result<(StorageProof, Value)> {
        // LIVE-7: the optional array params must be sent as empty arrays, not
        // null. Some backends accept null; lava's reject it with
        // `-32602 expected array for "class_hashes"`, and an aggregator routes
        // the same URL to either kind.
        let keys_param: Value = if keys.is_empty() {
            json!([])
        } else {
            json!([{
                "contract_address": strk20_feed::felt_hex(contract),
                "storage_keys": keys.iter().map(strk20_feed::felt_hex).collect::<Vec<_>>(),
            }])
        };
        let params = json!([
            r.to_json(),
            json!([]),
            [strk20_feed::felt_hex(contract)],
            keys_param
        ]);
        // LIVE-6: proofs are a per-endpoint capability. publicnode answers
        // code 42 at EVERY height, so a failover taken for unrelated reasons
        // must not turn every root check into a failure — ask each endpoint in
        // capability order before concluding the proof is unavailable, and
        // never move the active endpoint on account of a proof refusal.
        let mut last: Option<anyhow::Error> = None;
        for idx in self.proof_order() {
            match self
                .call_at(
                    idx,
                    "starknet_getStorageProof",
                    &params,
                    false,
                    PROOF_TRANSPORT_ATTEMPTS,
                )
                .await
            {
                Ok(raw) => {
                    self.caps[idx].proofs_served.fetch_add(1, Ordering::Relaxed);
                    let typed: StorageProof = serde_json::from_value(raw.clone())
                        .context("decode getStorageProof")?;
                    return Ok((typed, raw));
                }
                Err(e) if is_proof_unavailable(&e) => {
                    self.caps[idx].proofs_denied.fetch_add(1, Ordering::Relaxed);
                    last = Some(e);
                }
                // An unreachable or throttled endpoint is not an answer about
                // the proof: keep asking the remaining candidates, or a dead
                // primary defeats the capability fallback entirely. Only
                // semantic answers (block not found, bad params) short-circuit.
                Err(e) if is_endpoint_exhausted(&e) => {
                    tracing::warn!(
                        endpoint = %self.endpoints[idx % self.endpoints.len()],
                        error = %e,
                        "endpoint could not be reached for a storage proof; trying the next"
                    );
                    last = Some(e);
                }
                Err(e) => return Err(e),
            }
        }
        Err(last.unwrap_or_else(|| anyhow!("no rpc endpoint configured for storage proofs")))
    }

    pub async fn get_storage_at(&self, contract: &Felt, slot: &Felt, r: BlockRef) -> Result<Felt> {
        let v = self
            .call(
                "starknet_getStorageAt",
                json!([
                    strk20_feed::felt_hex(contract),
                    strk20_feed::felt_hex(slot),
                    r.to_json()
                ]),
            )
            .await?;
        let s = v.as_str().ok_or_else(|| anyhow!("storage value not a string"))?;
        Felt::from_hex(s).map_err(|_| anyhow!("bad storage felt"))
    }
}

pub fn parse_felt(s: &str) -> Result<Felt> {
    Felt::from_hex(s).map_err(|_| anyhow!("bad felt {s:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    const PRUNED: &str = r#"{"code":-32603,"data":"block 9693374 has been pruned; oldest retained block is 13108361","message":"Internal error"}"#;
    const PROOF_42: &str = r#"{"code":42,"message":"the node doesn't support storage proofs for blocks that are too far in the past"}"#;

    #[test]
    fn classifies_provider_capability_errors_apart_from_semantic_ones() {
        let pruned = anyhow!("rpc error from starknet_getEvents: {PRUNED}");
        assert!(is_pruned_history(&pruned));
        assert!(!is_proof_unavailable(&pruned));

        let proof = anyhow!("rpc error from starknet_getStorageProof: {PROOF_42}");
        assert!(is_proof_unavailable(&proof));
        assert!(!is_pruned_history(&proof));

        // Semantic errors are neither, and stay fatal on the first answer.
        for semantic in [
            r#"rpc error from starknet_getBlockWithTxHashes: {"code":24,"message":"Block not found"}"#,
            r#"rpc error from starknet_getEvents: {"code":-32602,"message":"Invalid params"}"#,
        ] {
            let e = anyhow!("{semantic}");
            assert!(!is_pruned_history(&e), "{semantic}");
            assert!(!is_proof_unavailable(&e), "{semantic}");
        }
    }

    #[test]
    fn classification_sees_through_caller_context() {
        let wrapped = anyhow!("rpc error from starknet_getStorageProof: {PROOF_42}")
            .context("getStorageProof for verify-root");
        assert!(is_proof_unavailable(&wrapped));
    }

    #[test]
    fn proof_order_prefers_an_endpoint_that_has_served_proofs() {
        let c = RpcClient::new("http://a".into(), Some("http://b".into()));
        assert_eq!(c.proof_order(), vec![0, 1], "unknown: active endpoint first");
        // The active endpoint refuses proofs at every height (publicnode
        // class); the other one has served some.
        c.caps[0].proofs_denied.fetch_add(1, Ordering::Relaxed);
        c.caps[1].proofs_served.fetch_add(1, Ordering::Relaxed);
        assert_eq!(c.proof_order(), vec![1, 0]);
        // A refusal never removes an endpoint: an out-of-window block says
        // nothing about the next one.
        assert_eq!(c.proof_order().len(), 2);
    }

    /// LIVE-1: a pruned-history answer is retried in place, but the retry is
    /// bounded and the provider's own message survives to the caller.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pruned_history_retries_are_bounded_and_preserve_the_message() {
        let hits = Arc::new(AtomicUsize::new(0));
        let state = hits.clone();
        let app = axum::Router::new().route(
            "/",
            axum::routing::post(move || {
                let hits = state.clone();
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    axum::Json(serde_json::json!({
                        "jsonrpc": "2.0", "id": 1,
                        "error": serde_json::from_str::<Value>(PRUNED).unwrap()
                    }))
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });

        let client = RpcClient::new(format!("http://{addr}/"), None);
        let err = client.chain_id().await.unwrap_err();

        assert_eq!(
            hits.load(Ordering::SeqCst),
            CAPABILITY_RETRIES + 1,
            "the retry must be bounded by CAPABILITY_RETRIES"
        );
        let text = format!("{err:#}");
        assert!(text.contains("has been pruned"), "{text}");
        assert!(text.contains("13108361"), "the provider's message must survive: {text}");
    }

    /// Serve `body` (a JSON-RPC response) on every POST, counting hits.
    async fn serve_json(body: Value) -> (String, Arc<AtomicUsize>) {
        let hits = Arc::new(AtomicUsize::new(0));
        let state = hits.clone();
        let app = axum::Router::new().route(
            "/",
            axum::routing::post(move || {
                let hits = state.clone();
                let body = body.clone();
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    axum::Json(body)
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}/"), hits)
    }

    /// Serve HTTP `status` on every POST, counting hits.
    async fn serve_status(status: axum::http::StatusCode) -> (String, Arc<AtomicUsize>) {
        let hits = Arc::new(AtomicUsize::new(0));
        let state = hits.clone();
        let app = axum::Router::new().route(
            "/",
            axum::routing::post(move || {
                let hits = state.clone();
                async move {
                    hits.fetch_add(1, Ordering::SeqCst);
                    (status, "nope")
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
        (format!("http://{addr}/"), hits)
    }

    fn proof_response() -> Value {
        serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "result": {
                "classes_proof": [],
                "contracts_proof": {
                    "nodes": [],
                    "contract_leaves_data": [{
                        "nonce": "0x0", "class_hash": "0x1", "storage_root": "0x2"
                    }]
                },
                "contracts_storage_proofs": [[]],
                "global_roots": {"block_hash": "0x3"}
            }
        })
    }

    /// A primary that cannot be reached at all must not consume the capability
    /// fallback: `get_storage_proof` has to ask the next endpoint, not return
    /// the transport error (review finding F4).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_dead_primary_does_not_stop_the_proof_fallback() {
        let (dead, dead_hits) = serve_status(axum::http::StatusCode::BAD_GATEWAY).await;
        let (live, live_hits) = serve_json(proof_response()).await;
        let client = RpcClient::new(dead, Some(live));

        let (proof, _raw) = client
            .get_storage_proof(BlockRef::Number(1), &Felt::from(1u64), &[])
            .await
            .expect("the healthy fallback must serve the proof");
        assert_eq!(
            proof.contracts_proof.contract_leaves_data[0].storage_root,
            "0x2"
        );
        assert!(dead_hits.load(Ordering::SeqCst) > 0, "the primary must be tried");
        assert!(live_hits.load(Ordering::SeqCst) > 0, "the fallback must be tried");
        // A proof refusal must never move the active endpoint (LIVE-6).
        assert_eq!(client.active_index(), 0);
    }

    /// LIVE-3 says 429 must not COUNT toward failover; it must not make a
    /// permanently throttled endpoint fatal either. Once the in-place backoff
    /// budget is spent, one escalation to the fallback keeps the run alive
    /// (review finding F7).
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn sustained_throttling_escalates_once_instead_of_failing() {
        let (throttled, throttled_hits) =
            serve_status(axum::http::StatusCode::TOO_MANY_REQUESTS).await;
        let chain_id_ok = serde_json::json!({
            "jsonrpc": "2.0", "id": 1,
            "result": "0x534e5f54455354" // "SN_TEST"
        });
        let (healthy, healthy_hits) = serve_json(chain_id_ok).await;
        let client = RpcClient::new(throttled, Some(healthy));

        let id = client
            .chain_id()
            .await
            .expect("a permanently throttled primary must escalate, not kill the run");
        assert_eq!(id, "SN_TEST");
        assert_eq!(
            throttled_hits.load(Ordering::SeqCst),
            THROTTLE_RETRIES + 1,
            "the in-place backoff budget must be spent before escalating"
        );
        assert!(healthy_hits.load(Ordering::SeqCst) > 0);
        assert_eq!(client.active_index(), 1, "the escalation must stick");
    }
}
