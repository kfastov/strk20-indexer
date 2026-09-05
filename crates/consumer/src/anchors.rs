//! Checkpoint verification shared by native and browser hosts.
//! A root match proves state at B, not the history of writes leading to B.

use crate::store::ConsumerStore;
use crate::transport::FeedTransport;
use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use serde_json::{json, Value};
use starknet_types_core::felt::Felt;

/// Ring 6's window onto the chain: a `starknet_getStorageProof` the USER's own
/// endpoint answers. Behind a trait because the request is host code (HTTP
/// natively, `fetch` in a browser) while the decision made from the answer is
/// Block B's.
///
/// The request names only a public pool and a public block, so it is
/// byte-identical for every user: keyless-compatible by construction.
#[async_trait]
pub trait ProofSource: Send + Sync {
    /// How to name this endpoint in a log line. Never a secret.
    fn label(&self) -> String;
    /// The `result` object of `starknet_getStorageProof` for `pool` at `block`.
    async fn storage_proof(&self, pool: &Felt, block: u64) -> Result<Value>;
    async fn checkpoint(
        &self,
        _pool: &Felt,
        _block: u64,
    ) -> Result<strk20_feed::checkpoint::TrustedCheckpoint> {
        bail!("CHECKPOINT_UNAVAILABLE: proof source has no independent block header")
    }
}

/// Check every published anchor at or below the mirror's head.
///
/// A transport failure PROPAGATES: "I could not reach the feed" must never be
/// reported as "nothing was wrong". And `all_ok` requires that something was
/// actually checked — a run that verified zero anchors has not verified the
/// mirror, whatever the reason, and must not exit green.
pub async fn verify_anchors<S: ConsumerStore>(
    store: &S,
    transport: &dyn FeedTransport,
) -> Result<Value> {
    let fetched = transport.fetch_anchors().await?;
    let publishes_anchors = fetched.is_some();
    let published = match fetched {
        Some(bytes) => strk20_feed::anchors::parse_anchors(&bytes)?,
        None => Vec::new(),
    };
    let head: u64 = store
        .meta_get("head_number")?
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    // A snapshot-started mirror cannot answer for anything below its basis:
    // slots zeroed before the basis are simply absent, so recomputing a root
    // there would manufacture a mismatch out of a known, declared gap.
    let basis: u64 = store
        .meta_get("snapshot_basis")?
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    let mut problems: Vec<String> = Vec::new();
    let mut checked = 0u64;
    let mut pending = 0u64;
    for a in &published {
        if a.block > head {
            // Ahead of what we have folded — nothing to compare against yet.
            pending += 1;
            continue;
        }
        if a.block < basis {
            continue;
        }
        let set = store.full_slot_set_as_of(a.block)?;
        let local = strk20_feed::mpt::storage_root(&set);
        if local != a.storage_root {
            problems.push(format!(
                "anchor mismatch at block {}: mirror storage_root {} != published {}",
                a.block,
                strk20_feed::felt_hex(&local),
                strk20_feed::felt_hex(&a.storage_root)
            ));
        }
        if let Some(stored) = store.block_hash(a.block)? {
            if stored != a.block_hash {
                problems.push(format!(
                    "anchor mismatch at block {}: mirror block_hash {} != published {}",
                    a.block,
                    strk20_feed::felt_hex(&stored),
                    strk20_feed::felt_hex(&a.block_hash)
                ));
            }
        }
        checked += 1;
    }

    let status = if !problems.is_empty() {
        "mismatch"
    } else if checked > 0 {
        "verified"
    } else if !publishes_anchors {
        "no-anchors-published"
    } else if published.is_empty() {
        "empty-anchor-log"
    } else {
        "all-anchors-above-head"
    };

    Ok(json!({
        "all_ok": problems.is_empty() && checked > 0,
        "status": status,
        "anchors_published": published.len(),
        "anchors_checked": checked,
        "anchors_pending": pending,
        "head": head,
        "problems": problems,
    }))
}

/// Outcome of ring 6. Three-valued for the same reason `verify-root` is
/// (§11.4/§11.5): a provider that does not implement `starknet_getStorageProof`,
/// or whose window has moved past every block we can ask about, has told us
/// nothing about the data. Reporting that as a verification failure is LIVE-6 —
/// a capability gap presented as mirror corruption.
#[derive(Debug)]
pub enum Grounding {
    /// The MIRROR's own recomputed root equalled the chain's at this block,
    /// and the proof was pinned to a block hash the mirror itself holds.
    Anchored(u64),
    /// No candidate block was provable — the endpoint could not answer, or its
    /// answer could not be pinned to a block this mirror knows. The mirror is
    /// not implicated either way.
    Unavailable(String),
}

/// Validate a complete state at one independently chosen checkpoint. Check the
/// cheap contract proof before calling this expensive state comparison.
pub fn verify_state<S: ConsumerStore>(
    store: &S,
    checkpoint: &strk20_feed::checkpoint::TrustedCheckpoint,
    expected_root: Felt,
) -> Result<()> {
    store.meta_set("verification_failed", "1")?;
    crate::apply::check_bound_above_basis(store, checkpoint.block_number)?;
    let head = store
        .meta_get("head_number")?
        .and_then(|s| s.parse::<u64>().ok())
        .unwrap_or(0);
    anyhow::ensure!(
        checkpoint.block_number <= head,
        "CHECKPOINT_AHEAD: feed has not reached checkpoint"
    );
    anyhow::ensure!(
        store.meta_get("chain_id")?.as_deref() == Some(&checkpoint.chain_id),
        "CHAIN_MISMATCH: checkpoint network"
    );
    let pool = store
        .meta_get("pool")?
        .ok_or_else(|| anyhow!("missing pool"))?;
    anyhow::ensure!(
        strk20_feed::felt_from_hex(&pool)? == checkpoint.pool,
        "CHAIN_MISMATCH: checkpoint pool"
    );
    let root = store.storage_root_at(checkpoint.block_number)?;
    anyhow::ensure!(
        root == expected_root,
        "CHECKPOINT_STATE_MISMATCH: complete pool state does not match block {}",
        checkpoint.block_number
    );
    store.meta_set("verified_checkpoint", &serde_json::to_string(checkpoint)?)?;
    store.meta_set("verification_failed", "0")?;
    Ok(())
}

pub async fn ground_mirror_against_rpc<S: ConsumerStore>(
    store: &S,
    _transport: &dyn FeedTransport,
    proofs: &dyn ProofSource,
    _basis: u64,
    head: u64,
) -> Result<Grounding> {
    let pool = Felt::from_hex(
        &store
            .meta_get("pool")?
            .ok_or_else(|| anyhow!("missing pool"))?,
    )?;
    let checkpoint = match proofs.checkpoint(&pool, head).await {
        Ok(cp) => cp,
        Err(e) if is_proof_unavailable(&e) => return Ok(Grounding::Unavailable(e.to_string())),
        Err(e) => return Err(e),
    };
    let proof = match proofs.storage_proof(&pool, checkpoint.block_number).await {
        Ok(proof) => proof,
        Err(e) if is_proof_unavailable(&e) => return Ok(Grounding::Unavailable(e.to_string())),
        Err(e) => return Err(e),
    };
    let root = strk20_feed::checkpoint::verify_checkpoint(&checkpoint, &proof.to_string())?;
    verify_state(store, &checkpoint, root)?;
    Ok(Grounding::Anchored(checkpoint.block_number))
}

/// Client-side twin of the indexer's `rpc::is_proof_unavailable` (§11.5): the
/// answer is about the ENDPOINT, never about the data.
pub fn is_proof_unavailable(e: &anyhow::Error) -> bool {
    let msg = format!("{e:#}");
    // window (pathfinder/juno code 42), method not implemented (publicnode,
    // drpc -32601), and an endpoint simply lagging behind the block we asked
    // about (code 24). All three are facts about the ENDPOINT.
    // A host that is PUSHED its proofs (the browser: TypeScript fetches, the
    // module computes) has no proof for a candidate block it was never given
    // one for. That is a gap in what the HOST offered, exactly like a window
    // that has moved — never evidence about the mirror. The wrapper still has
    // to notice a staged proof that nothing consumed; it does, loudly.
    msg.contains("CHECKPOINT_UNAVAILABLE")
        || msg.contains("PROOF_NOT_STAGED")
        || msg.contains("too far in the past")
        || msg.contains("\"code\":42")
        || msg.contains("\"code\": 42")
        || msg.contains("-32601")
        || msg.contains("Method not found")
        || msg.contains("method not found")
        || msg.contains("Block not found")
        || msg.contains("\"code\":24")
        || msg.contains("\"code\": 24")
}
