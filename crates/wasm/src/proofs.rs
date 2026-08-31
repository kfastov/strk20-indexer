//! `StagedProofs` — a [`ProofSource`] that fetches nothing.
//!
//! The twin of [`crate::staged::StagedFeed`], for the same reason and by the
//! same mechanism. §1.5 ring 6 grounds the folded mirror in the chain by
//! comparing the client's own recomputed storage root against one a
//! `starknet_getStorageProof` reports — and that proof has to come from an
//! endpoint the **user** chose, because the whole point of the ring is to take
//! the feed server out of the trust path. Block B expresses "where a proof
//! comes from" as a trait; the native client backs it with HTTP. A browser
//! cannot: the module makes no network calls and the import-section audit is
//! the mechanical proof of that.
//!
//! So the browser inverts the call exactly as it inverts the feed fetch.
//! TypeScript performs the JSON-RPC round trip against the user's own node and
//! **pushes the answer in**; this type hands it back when ring 6 asks. Nothing
//! verification-bearing moved: the root recomputation, the comparison, the
//! candidate-block choice and the LIVE-6 capability/corruption distinction all
//! still run inside `strk20-consumer` over these bytes.
//!
//! ## The binding this type enforces, and why it is here
//!
//! A public `starknet_getStorageProof` endpoint is an **anonymous
//! load-balanced pool**: two requests can land on two nodes, and the second may
//! answer for a different block than the one you believe you asked about — a
//! lagging replica, or a fork the rest of the network dropped. Measured
//! earlier in this project, not assumed. A root read out of such an answer is
//! worthless until it is pinned to the block the user's node calls canonical.
//!
//! The pin is one felt comparison: the proof's `global_roots.block_hash` must
//! equal the `block_hash` of `starknet_getBlockWithTxHashes(block)`. Both
//! values pass through this module, so the module makes the comparison —
//! leaving it to TypeScript would make the strongest grade in the system
//! depend on a wrapper remembering to do something. It is checked at STAGING
//! time, before anything is folded, so a mispaired proof fails immediately and
//! by name rather than surfacing later as a root mismatch.
//!
//! ## What "anchored" does and does not rest on
//!
//! It rests on: the user's chosen node reporting honestly about its own state,
//! and the two answers (proof, block header) agreeing about which block they
//! describe. It does not rest on the feed server for anything — not even the
//! choice of block, which ring 6 re-derives and whose root it recomputes from
//! the client's own slot set.
//!
//! What is NOT walked here is the proof's own Merkle path up to
//! `global_roots.contracts_tree_root` (`strk20_feed::mpt::verify_storage_proof`
//! is compiled in and does that for the *storage* tree, but the contracts-tree
//! leaf preimage is not reconstructed). That is deliberate parity with the
//! native client: the endpoint is the trust anchor by construction, so a walk
//! would only defend against a lying anchor, which is not a position this ring
//! can improve from. The load-bearing check against a *load-balanced* endpoint
//! is the block-hash binding above, and that one is enforced.

use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use serde_json::Value;
use starknet_types_core::felt::Felt;
use std::collections::BTreeMap;
use std::sync::Mutex;
use strk20_consumer::anchors::ProofSource;

fn felt(hex: &str, what: &str) -> Result<Felt> {
    strk20_feed::felt_from_hex(hex)
        .map_err(|e| anyhow!("PROOF_MALFORMED: {what} {hex:?} is not a felt: {e}"))
}

/// One staged `starknet_getStorageProof` answer, already bound to the block
/// hash the user's own node reports for that block.
struct Proof {
    /// The `result` object, exactly as ring 6 will read it.
    result: Value,
    /// `global_roots.block_hash`, which equalled the staged header hash.
    block_hash: Felt,
    /// The leaf's `storage_root` — kept only so a staged proof can be
    /// described in an error without re-walking the JSON.
    storage_root: Felt,
}

#[derive(Default)]
struct Inner {
    /// block -> the proof TypeScript fetched for it
    proofs: BTreeMap<u64, Proof>,
    /// blocks ring 6 asked about during the last `discover`
    asked: Vec<u64>,
}

/// Push-side handle for ring 6's proofs. Cheap to share.
#[derive(Default)]
pub struct StagedProofs {
    inner: Mutex<Inner>,
}

impl StagedProofs {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().expect("staged proofs poisoned")
    }

    /// Accept one `starknet_getStorageProof` answer for `block`, paired with
    /// the `block_hash` field of `starknet_getBlockWithTxHashes(block)`.
    ///
    /// Everything structural is checked here, so `discover` cannot later fail
    /// for a reason that was already visible: the envelope must not be an RPC
    /// error, the leaf must carry a storage root, and the proof must be about
    /// the block the user's node named.
    pub fn put(&self, block: u64, proof_json: &str, block_hash_hex: &str) -> Result<()> {
        let v: Value = serde_json::from_str(proof_json)
            .map_err(|e| anyhow!("PROOF_MALFORMED: storage proof for block {block} is not JSON: {e}"))?;

        // Accept either the bare `result` object or the whole JSON-RPC
        // envelope — a wrapper that forwards the response verbatim is doing
        // the more obvious thing, and an `error` envelope must not be mistaken
        // for a proof with missing fields.
        if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
            bail!(
                "PROOF_MALFORMED: the endpoint returned a JSON-RPC error for block {block}, \
                 not a storage proof: {err}"
            );
        }
        let result = match v.get("result") {
            Some(r) if !r.is_null() => r.clone(),
            _ => v,
        };

        let leaf = &result["contracts_proof"]["contract_leaves_data"][0];
        let storage_root = felt(
            leaf["storage_root"].as_str().ok_or_else(|| {
                anyhow!(
                    "PROOF_MALFORMED: storage proof for block {block} has no \
                     contracts_proof.contract_leaves_data[0].storage_root. Ask for the pool \
                     as the one contract address, and send the whole `result` object."
                )
            })?,
            "storage_root",
        )?;

        let proof_hash = felt(
            result["global_roots"]["block_hash"].as_str().ok_or_else(|| {
                anyhow!(
                    "PROOF_MALFORMED: storage proof for block {block} has no \
                     global_roots.block_hash, so it cannot be bound to a block and must \
                     not be believed."
                )
            })?,
            "global_roots.block_hash",
        )?;
        let header_hash = felt(block_hash_hex, "block hash")?;
        if proof_hash != header_hash {
            bail!(
                "PROOF_BLOCK_MISMATCH: the storage proof for block {block} is about block \
                 hash {}, but starknet_getBlockWithTxHashes({block}) says {}. A public \
                 proof endpoint is an anonymous load-balanced pool, so this means the \
                 proof came from a node on a different (lagging or forked) view of the \
                 chain. It proves nothing about the canonical block {block} and is \
                 refused.",
                strk20_feed::felt_hex(&proof_hash),
                strk20_feed::felt_hex(&header_hash)
            );
        }

        self.lock().proofs.insert(
            block,
            Proof {
                result,
                block_hash: header_hash,
                storage_root,
            },
        );
        Ok(())
    }

    /// Forget every staged proof. A new head makes the old candidate blocks
    /// stale, so a live client re-stages rather than accumulating.
    pub fn clear(&self) {
        self.lock().proofs.clear();
    }

    pub fn staged_blocks(&self) -> Vec<u64> {
        self.lock().proofs.keys().copied().collect()
    }

    /// What is on file for `block`, for reporting.
    pub fn describe(&self, block: u64) -> Option<(String, String)> {
        self.lock().proofs.get(&block).map(|p| {
            (
                strk20_feed::felt_hex(&p.block_hash),
                strk20_feed::felt_hex(&p.storage_root),
            )
        })
    }

    /// Start recording which blocks ring 6 asks about. Called once per
    /// `discover`, so the answer describes that run and not a previous one.
    pub fn begin(&self) {
        self.lock().asked.clear();
    }

    /// The blocks ring 6 asked about during the run that `begin` opened.
    pub fn asked(&self) -> Vec<u64> {
        self.lock().asked.clone()
    }
}

#[async_trait]
impl ProofSource for StagedProofs {
    fn label(&self) -> String {
        "the storage proof staged by the caller".to_owned()
    }

    async fn storage_proof(&self, pool: &Felt, block: u64) -> Result<Value> {
        let mut g = self.lock();
        g.asked.push(block);
        let Some(proof) = g.proofs.get(&block) else {
            // Ring 6 tries several blocks; a gap in what the host staged is a
            // statement about the HOST, so it must not fail the sync here.
            // `is_proof_unavailable` knows this token, the loop moves to the
            // next candidate, and `discover` refuses to return a downgraded
            // grade while a staged proof went unconsumed.
            bail!("PROOF_NOT_STAGED: no storage proof was staged for block {block}");
        };
        // Opportunistic: the JSON-RPC response shape carries no contract
        // address, but an endpoint that adds one must not be describing some
        // other contract.
        for key in ["address", "contract_address"] {
            let named = proof.result["contracts_proof"]["contract_leaves_data"][0][key].as_str();
            if let Some(addr) = named.and_then(|s| strk20_feed::felt_from_hex(s).ok()) {
                if addr != *pool {
                    bail!(
                        "PROOF_MALFORMED: the storage proof staged for block {block} is about \
                         contract {}, not the pool {}.",
                        strk20_feed::felt_hex(&addr),
                        strk20_feed::felt_hex(pool)
                    );
                }
            }
        }
        Ok(proof.result.clone())
    }
}
