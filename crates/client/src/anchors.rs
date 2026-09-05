//! The native host's window onto the chain for §1.5 ring 6.
//!
//! The *decision* — which blocks to try, what a mismatch means, why a
//! capability gap is not corruption (LIVE-6) — is Block B's and lives in
//! `strk20_consumer::anchors`. What lives here is the one thing a browser
//! cannot share: an HTTP JSON-RPC call.

use anyhow::{anyhow, bail, Context, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use starknet_types_core::felt::Felt;

pub use strk20_consumer::anchors::{
    ground_mirror_against_rpc, is_proof_unavailable, verify_anchors, Grounding, ProofSource,
};

/// `starknet_getStorageProof` over HTTP against an endpoint the USER chose.
pub struct RpcProofSource {
    rpc: String,
    http: reqwest::Client,
}

impl RpcProofSource {
    pub fn new(rpc: &str) -> Result<Self> {
        Ok(Self {
            rpc: rpc.to_owned(),
            http: reqwest::Client::builder()
                .user_agent(concat!("strk20-sync/", env!("CARGO_PKG_VERSION")))
                .build()?,
        })
    }
}

#[async_trait]
impl ProofSource for RpcProofSource {
    fn label(&self) -> String {
        self.rpc.clone()
    }

    async fn checkpoint(
        &self,
        pool: &Felt,
        block: u64,
    ) -> Result<strk20_feed::checkpoint::TrustedCheckpoint> {
        let call = |method: &str, params: Value| {
            self.http
                .post(&self.rpc)
                .json(&json!({"jsonrpc":"2.0","id":1,"method":method,"params":params}))
                .send()
        };
        let header: Value = call(
            "starknet_getBlockWithTxHashes",
            json!([{"block_number":block}]),
        )
        .await?
        .error_for_status()?
        .json()
        .await?;
        let chain: Value = call("starknet_chainId", json!([]))
            .await?
            .error_for_status()?
            .json()
            .await?;
        let h = header
            .get("result")
            .ok_or_else(|| anyhow!("CHECKPOINT_UNAVAILABLE: header RPC failed"))?;
        anyhow::ensure!(
            h["block_number"].as_u64() == Some(block)
                && matches!(
                    h["status"].as_str(),
                    Some("ACCEPTED_ON_L1" | "ACCEPTED_ON_L2")
                ),
            "CHECKPOINT_UNAVAILABLE: expected an accepted header at block {block}"
        );
        let hex = chain["result"]
            .as_str()
            .ok_or_else(|| anyhow!("CHECKPOINT_UNAVAILABLE: chainId RPC failed"))?;
        let felt = Felt::from_hex(hex)?;
        let bytes = felt.to_bytes_be();
        let chain_id = String::from_utf8(bytes.into_iter().skip_while(|b| *b == 0).collect())?;
        Ok(strk20_feed::checkpoint::TrustedCheckpoint {
            chain_id,
            pool: *pool,
            block_number: h["block_number"]
                .as_u64()
                .ok_or_else(|| anyhow!("missing block_number"))?,
            block_hash: Felt::from_hex(
                h["block_hash"]
                    .as_str()
                    .ok_or_else(|| anyhow!("missing block_hash"))?,
            )?,
            state_root: Felt::from_hex(
                h["new_root"]
                    .as_str()
                    .ok_or_else(|| anyhow!("missing state root"))?,
            )?,
        })
    }

    async fn storage_proof(&self, pool: &Felt, block: u64) -> Result<Value> {
        // `[]` rather than `null` for the list params: some backends reject
        // null outright and every backend accepts the empty list (LIVE-7).
        let body = json!({
            "jsonrpc": "2.0", "id": 1, "method": "starknet_getStorageProof",
            "params": [
                {"block_number": block},
                [],
                [strk20_feed::felt_hex(pool)],
                []
            ]
        });
        let v: Value = self
            .http
            .post(&self.rpc)
            .json(&body)
            .send()
            .await
            .with_context(|| format!("POST {}", self.rpc))?
            .error_for_status()?
            .json()
            .await?;
        if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
            bail!("storage proof for block {block} unavailable: {err}");
        }
        v.get("result")
            .cloned()
            .ok_or_else(|| anyhow!("no result in storage proof response"))
    }
}
