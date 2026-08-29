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

pub struct RpcClient {
    http: reqwest::Client,
    endpoints: Vec<String>,
    active: AtomicUsize,
    consecutive_failures: AtomicUsize,
}

impl RpcClient {
    pub fn new(primary: String, fallback: Option<String>) -> Self {
        let mut endpoints = vec![primary];
        endpoints.extend(fallback);
        Self {
            http: reqwest::Client::builder()
                .user_agent(USER_AGENT)
                .timeout(Duration::from_secs(30))
                .build()
                .expect("reqwest client"),
            endpoints,
            active: AtomicUsize::new(0),
            consecutive_failures: AtomicUsize::new(0),
        }
    }

    pub fn active_endpoint(&self) -> &str {
        &self.endpoints[self.active.load(Ordering::Relaxed) % self.endpoints.len()]
    }

    async fn call(&self, method: &str, params: Value) -> Result<Value> {
        let mut delay = Duration::from_millis(300);
        for attempt in 0..8 {
            let url = self.active_endpoint().to_owned();
            let body = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
            let resp = self.http.post(&url).json(&body).send().await;
            match resp {
                Ok(r) if r.status() == reqwest::StatusCode::TOO_MANY_REQUESTS => {
                    tracing::warn!(%url, method, "rate limited, backing off");
                }
                Ok(r) => match r.error_for_status() {
                    Ok(ok) => {
                        let v: Value = ok.json().await.context("decode json-rpc response")?;
                        if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
                            // JSON-RPC level errors are not transport failures.
                            self.consecutive_failures.store(0, Ordering::Relaxed);
                            bail!("rpc error from {method}: {err}");
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
            let fails = self.consecutive_failures.fetch_add(1, Ordering::Relaxed) + 1;
            if fails >= FAILOVER_AFTER && self.endpoints.len() > 1 {
                let next = (self.active.load(Ordering::Relaxed) + 1) % self.endpoints.len();
                self.active.store(next, Ordering::Relaxed);
                self.consecutive_failures.store(0, Ordering::Relaxed);
                tracing::warn!(endpoint = %self.endpoints[next], "failing over rpc endpoint");
            }
            if attempt < 7 {
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(Duration::from_secs(60));
            }
        }
        bail!("rpc call {method} failed after retries on all endpoints")
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
        Ok(serde_json::from_value(v).context("decode getEvents")?)
    }

    pub async fn get_state_update(&self, block: u64) -> Result<StateUpdate> {
        let v = self
            .call("starknet_getStateUpdate", json!([{ "block_number": block }]))
            .await?;
        Ok(serde_json::from_value(v).context("decode getStateUpdate")?)
    }

    pub async fn get_block(&self, r: BlockRef) -> Result<BlockHeader> {
        let v = self
            .call("starknet_getBlockWithTxHashes", json!([r.to_json()]))
            .await?;
        Ok(serde_json::from_value(v).context("decode getBlockWithTxHashes")?)
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
        let keys_param: Value = if keys.is_empty() {
            Value::Null
        } else {
            json!([{
                "contract_address": strk20_feed::felt_hex(contract),
                "storage_keys": keys.iter().map(strk20_feed::felt_hex).collect::<Vec<_>>(),
            }])
        };
        let params = json!([
            r.to_json(),
            Value::Null,
            [strk20_feed::felt_hex(contract)],
            keys_param
        ]);
        let raw = self.call("starknet_getStorageProof", params).await?;
        let typed: StorageProof =
            serde_json::from_value(raw.clone()).context("decode getStorageProof")?;
        Ok((typed, raw))
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
