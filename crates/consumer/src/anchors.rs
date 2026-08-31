//! Anchor verification (spec §7.8): fold the local mirror to each published
//! anchor block, recompute the pool storage root with the shared MPT, and
//! compare.
//!
//! Trust meaning, precisely: `anchors.ndjson` is NOT content-addressed and is
//! not part of the epoch hash chain, so a published record proves nothing on
//! its own — a hostile feed can write whatever it likes there. What the check
//! establishes is a CONSISTENCY claim: the mirror this client folded and the
//! root the publisher claims to have read from a chain storage proof agree at
//! block N. An operator who has independently obtained the chain's storage
//! root at N (their own node, an explorer, another mirror) can therefore
//! validate the whole mirror below N with one felt comparison, because pool
//! slots are write-once: a root match at N subsumes every write below N.
//! A mismatch means the mirror and the publisher's own proof disagree, which
//! is always a defect on one side.

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
    /// The MIRROR's own recomputed root equalled the chain's at this block.
    Anchored(u64),
    /// No candidate block was provable. The mirror is not implicated.
    Unavailable(String),
}

/// §1.5 ring 6 — chain grounding through the USER'S OWN RPC.
///
/// What is compared, and why it changed: the earlier form fetched
/// `anchors.ndjson` a SECOND time and compared that record against the chain,
/// while §11.3 reachability had compared the mirror against a record from its
/// OWN fetch. Those two comparisons compose only if both fetches returned the
/// same record — which a hostile feed can trivially prevent (serve a fabricated
/// log to the first GET and the honest one to the second), and which an honest
/// feed breaks by simply appending a new anchor between them. The mirror was
/// then never compared to the chain at all, and `"anchored"` was forgeable.
///
/// So ring 6 no longer trusts the anchors log for anything but the CHOICE of
/// block: it recomputes the storage root from the client's own folded slot set
/// and compares that with the chain. The feed is out of the proof path
/// entirely, which is what §1.5 ring 6 claims. Any block at or above the basis
/// is a sound choice — pool slots are write-once, so a root match at B attests
/// every write at or below B, the snapshot included — so letting the server
/// name a recent block costs nothing.
pub async fn ground_mirror_against_rpc<S: ConsumerStore>(
    store: &S,
    transport: &dyn FeedTransport,
    proofs: &dyn ProofSource,
    basis: u64,
    head: u64,
) -> Result<Grounding> {
    let pool_hex = store
        .meta_get("pool")?
        .ok_or_else(|| anyhow!("mirror has no pool metadata; sync first"))?;
    let pool = Felt::from_hex(&pool_hex).map_err(|_| anyhow!("bad pool metadata"))?;

    let candidates = grounding_candidates(transport, basis, head).await?;
    if candidates.is_empty() {
        return Ok(Grounding::Unavailable(format!(
            "no block at or above the snapshot basis {basis} is available to ground \
             against (mirror head {head})"
        )));
    }

    let mut mismatches: Vec<String> = Vec::new();
    let mut unavailable: Vec<String> = Vec::new();
    for block in candidates {
        let result = match proofs.storage_proof(&pool, block).await {
            Ok(v) => v,
            Err(e) if is_proof_unavailable(&e) => {
                unavailable.push(format!("{block}: {e}"));
                continue;
            }
            Err(e) => return Err(e),
        };
        let leaf = &result["contracts_proof"]["contract_leaves_data"][0];
        let chain_root = Felt::from_hex(
            leaf["storage_root"]
                .as_str()
                .ok_or_else(|| anyhow!("storage proof has no storage_root"))?,
        )
        .map_err(|_| anyhow!("bad storage_root in storage proof"))?;
        let local = strk20_feed::mpt::storage_root(&store.full_slot_set_as_of(block)?);
        if local != chain_root {
            mismatches.push(format!(
                "block {block}: mirror storage root {} != chain {}",
                strk20_feed::felt_hex(&local),
                strk20_feed::felt_hex(&chain_root)
            ));
            continue;
        }
        if let (Some(chain_hash), Some(stored)) = (
            result["global_roots"]["block_hash"]
                .as_str()
                .and_then(|s| Felt::from_hex(s).ok()),
            store.block_hash(block)?,
        ) {
            if chain_hash != stored {
                mismatches.push(format!(
                    "block {block}: mirror block hash {} != chain {}",
                    strk20_feed::felt_hex(&stored),
                    strk20_feed::felt_hex(&chain_hash)
                ));
                continue;
            }
        }
        return Ok(Grounding::Anchored(block));
    }

    if mismatches.is_empty() {
        return Ok(Grounding::Unavailable(format!(
            "no candidate block was provable by {}: {}",
            proofs.label(),
            unavailable.join("; ")
        )));
    }
    // A tail block can be reorged between our head fetch and this call, so a
    // single mismatch is not proof of tampering — every candidate the endpoint
    // could answer has to disagree before the verdict is the mirror's.
    bail!(
        "ANCHOR_NOT_ON_CHAIN: your own RPC disagrees with this mirror at every block it \
         could answer for, so the slot set it was cold-started from is not the chain's. \
         {}",
        mismatches.join("; ")
    )
}

/// The blocks ring 6 will ask the user's endpoint about, in the order it will
/// ask — head first, then published anchors newest-first.
///
/// Split out of [`ground_mirror_against_rpc`] so a host that cannot make the
/// call itself (the browser, where the proof is *pushed in* rather than
/// fetched) can be told which blocks a proof has to cover. Two copies of this
/// list would drift, and a drifted list degrades silently: the host stages a
/// proof for a block ring 6 never asks about.
pub async fn grounding_candidates(
    transport: &dyn FeedTransport,
    basis: u64,
    head: u64,
) -> Result<Vec<u64>> {
    let mut candidates: Vec<u64> = Vec::new();
    if head >= basis {
        candidates.push(head);
    }
    if let Some(bytes) = transport.fetch_anchors().await? {
        let mut blocks: Vec<u64> = strk20_feed::anchors::parse_anchors(&bytes)?
            .iter()
            .filter(|a| a.block >= basis && a.block <= head)
            .map(|a| a.block)
            .collect();
        blocks.sort_unstable_by(|a, b| b.cmp(a));
        candidates.extend(blocks);
    }
    candidates.dedup();
    // Bound the RPC calls: the newest handful is all the window can serve
    // anyway, and an unbounded list would let a feed dictate our request count.
    candidates.truncate(MAX_GROUNDING_CANDIDATES);
    Ok(candidates)
}

/// How many blocks ring 6 will ask the user's RPC about before giving up.
const MAX_GROUNDING_CANDIDATES: usize = 4;

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
    msg.contains("PROOF_NOT_STAGED")
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
