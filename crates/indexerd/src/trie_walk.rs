//! Structural enumeration of the chain's pool storage trie — the completing
//! form of the sound-ingest.md §4.2 closure loop.
//!
//! §4.2 step 2 localises a divergence by binary-searching the ACTIVE-BLOCK
//! index with `verify-root`, then flat-scanning the gap it lands in. That
//! works, but it inherits two properties of the mirror's own index: it can
//! only ever land between two blocks the mirror already knows are active, and
//! its predicate is monotone only for write-once slots (§7.10 — a missed write
//! to a mutable admin slot can be masked by a later one the mirror did
//! capture, healing the root while the block stays absent, and the bisection
//! then walks straight past it).
//!
//! This walk has neither property, because it never consults the mirror's
//! index at all. A `starknet_getStorageProof` answer returns nodes keyed by
//! their own hash, and every node names its children by hash. A child hash is
//! therefore a commitment to the whole subtree beneath that bit-prefix — and
//! the identical quantity is computable from the mirror's slot set
//! (`mpt::subtree_hash_at`). So:
//!
//! - where the two hashes agree, the subtrees are identical and neither side
//!   needs fetching — the overwhelming majority of the tree is pruned at the
//!   first few levels;
//! - where they disagree, descend;
//! - a disagreement that reaches depth 251 is a leaf, and a leaf the mirror
//!   scores as empty is a slot the mirror has never seen.
//!
//! Descending needs the chain's node for a hash we have not been shown yet. A
//! proof for ANY key returns the entire root→key path, so one crafted key
//! (the unexplained prefix, zero-padded to 251 bits) reveals it, and crafted
//! keys batch many-per-request. The cost is proportional to the size of the
//! DIVERGENCE, not to the size of the tree or of the block range: measured on
//! Sepolia, 221 missing slots out of 23,155 chain leaves in ~260 proof calls,
//! with no bisection over blocks at all.
//!
//! What the walk yields is slots. `attribute_to_blocks` turns those into the
//! block numbers `strk20 rescan --blocks` wants, by bisecting each slot's
//! first non-zero `getStorageAt` — then reading that block's state update to
//! claim every other missing slot written alongside it, so the bisection runs
//! once per BLOCK, not once per slot.
//!
//! **Where the cost actually sits.** The RPC side is cheap and stays cheap.
//! The local side is not free: `mpt::subtree_hash_at` re-folds the mirror's
//! entries under a prefix on every call, which is O(n) near the root, and the
//! walk asks once per visited prefix. Measured on the mainnet mirror at
//! 135,308 slots with 29 missing, the whole walk is ~90 s wall clock, nearly
//! all of it in that fold, against 15 proof calls. That is a fine trade for an
//! operator command run after a `verify-root` MISMATCH, and it is bounded
//! because a matching subtree prunes immediately — the common case
//! (`chain_root == local_root`) returns after ONE proof call and one root
//! fold. But the fold is the thing to memoise if a future divergence is ever
//! wide enough to make this the slow step.

use crate::cutter::Cutter;
use crate::rpc::BlockRef;
use anyhow::{anyhow, bail, Context, Result};
use starknet_types_core::felt::Felt;
use std::collections::{HashMap, HashSet};
use strk20_feed::mpt::{self, ProofNode, ProofNodeBody};
use strk20_feed::{felt_from_hex, felt_hex};

/// Crafted keys per `getStorageProof`. The request carries one key list for
/// one contract; 64 keeps a response comfortably inside what lava returns in
/// one piece while still collapsing a whole frontier into a couple of calls.
const KEYS_PER_REQUEST: usize = 64;

/// A walk that has not closed by here is REPORTED, never looped. Each round
/// strictly deepens the explained frontier, so the real bound is the tree
/// height; this is the guard against an endpoint that answers with nodes that
/// explain nothing.
const MAX_ROUNDS: usize = 300;

#[derive(Debug, Default)]
pub struct SlotDiff {
    pub block: u64,
    pub chain_root: Felt,
    pub local_root: Felt,
    /// Chain leaves the mirror scores as empty: (slot, chain value).
    pub missing: Vec<(Felt, Felt)>,
    /// Slots both sides hold at different values: (slot, ours, chain's).
    pub divergent: Vec<(Felt, Felt, Felt)>,
    /// Slots the mirror holds that the chain does not. Should always be empty;
    /// a non-empty list means the mirror invented a write, which is a
    /// different and worse defect than a missing one.
    pub extra: Vec<Felt>,
    pub chain_leaves: usize,
    pub local_leaves: usize,
    pub proof_calls: usize,
    pub rounds: usize,
}

impl SlotDiff {
    pub fn is_clean(&self) -> bool {
        self.missing.is_empty() && self.divergent.is_empty() && self.extra.is_empty()
    }
}

/// Enumerate every pool slot the chain's trie holds at `block` that this
/// mirror does not, by structure alone.
pub async fn enumerate_missing_slots(cutter: &Cutter<'_>, block: u64) -> Result<SlotDiff> {
    let set = cutter.db.full_slot_set_as_of(block)?;
    let sorted = mpt::bit_entries(&set);
    let local_root = mpt::storage_root(&set);

    // The same binding every other proof consumer applies (§12 B2): an
    // aggregator's proof is believed only once `global_roots.block_hash`
    // equals the block's real header hash. A walk seeded from an unbound root
    // would enumerate a different chain's tree.
    let (proof, raw) = cutter
        .bound_proof(block)
        .await
        .with_context(|| format!("bound storage proof at block {block} to seed the trie walk"))?;
    let leaf = proof
        .contracts_proof
        .contract_leaves_data
        .first()
        .ok_or_else(|| anyhow!("proof for block {block} has no contract leaf"))?;
    let chain_root = crate::rpc::parse_felt(&leaf.storage_root)?;

    let mut diff = SlotDiff {
        block,
        chain_root,
        local_root,
        local_leaves: sorted.len(),
        ..Default::default()
    };

    let mut nodes: HashMap<[u8; 32], ProofNodeBody> = HashMap::new();
    absorb(&mut nodes, &raw)?;
    diff.proof_calls = 1;

    if chain_root == local_root {
        diff.chain_leaves = sorted.len();
        return Ok(diff);
    }

    // (prefix, the chain's child hash for that prefix) still to explain.
    let mut frontier: Vec<(Vec<bool>, Felt)> = vec![(Vec::new(), chain_root)];
    let mut chain_leaves = 0usize;

    while !frontier.is_empty() {
        diff.rounds += 1;
        if diff.rounds > MAX_ROUNDS {
            bail!(
                "trie walk did not close in {MAX_ROUNDS} rounds at block {block} \
                 ({} unexplained subtrees left, {} proof calls spent). Reporting rather \
                 than looping.",
                frontier.len(),
                diff.proof_calls
            );
        }

        let mut unexplained: Vec<(Vec<bool>, Felt)> = Vec::new();
        let mut queue = std::mem::take(&mut frontier);
        while let Some((prefix, chain_hash)) = queue.pop() {
            let local = mpt::subtree_hash_at(&sorted, &prefix);
            // The prune that makes this affordable: equal child hashes commit
            // to equal subtrees, so everything below is known-identical.
            if local == chain_hash {
                chain_leaves += mpt::entries_with_prefix(&sorted, &prefix).len();
                continue;
            }
            if chain_hash == Felt::ZERO {
                for (bits, _) in mpt::entries_with_prefix(&sorted, &prefix) {
                    diff.extra.push(mpt::path_to_key(bits));
                }
                continue;
            }
            if prefix.len() == mpt::TREE_HEIGHT {
                // At full depth the "child hash" IS the leaf value.
                chain_leaves += 1;
                let key = mpt::path_to_key(&prefix);
                if local == Felt::ZERO {
                    diff.missing.push((key, chain_hash));
                } else {
                    diff.divergent.push((key, local, chain_hash));
                }
                continue;
            }
            match nodes.get(&chain_hash.to_bytes_be()) {
                Some(ProofNodeBody::Binary { left, right }) => {
                    let l = felt_from_hex(left)?;
                    let r = felt_from_hex(right)?;
                    let mut lp = prefix.clone();
                    lp.push(false);
                    let mut rp = prefix;
                    rp.push(true);
                    queue.push((lp, l));
                    queue.push((rp, r));
                }
                Some(ProofNodeBody::Edge {
                    path,
                    length,
                    child,
                }) => {
                    let child = felt_from_hex(child)?;
                    let bits = edge_bits(path, *length)?;
                    let mut p = prefix;
                    p.extend_from_slice(&bits);
                    if p.len() > mpt::TREE_HEIGHT {
                        bail!("edge at block {block} overruns the tree height");
                    }
                    queue.push((p, child));
                }
                None => unexplained.push((prefix, chain_hash)),
            }
        }

        if unexplained.is_empty() {
            break;
        }

        // Ask the chain to reveal each unexplained subtree. A proof for any key
        // under the prefix returns the whole root→key path, so the prefix
        // zero-padded to full depth is enough, and they batch.
        let before = nodes.len();
        let keys: Vec<Felt> = unexplained
            .iter()
            .map(|(p, _)| {
                let mut full = p.clone();
                full.resize(mpt::TREE_HEIGHT, false);
                mpt::path_to_key(&full)
            })
            .collect();
        for chunk in keys.chunks(KEYS_PER_REQUEST) {
            let (_, raw) = cutter
                .rpc
                .get_storage_proof(BlockRef::Number(block), &cutter.cfg.pool, chunk)
                .await
                .with_context(|| {
                    format!(
                        "storage proof for {} crafted keys at block {block} (round {})",
                        chunk.len(),
                        diff.rounds
                    )
                })?;
            diff.proof_calls += 1;
            absorb(&mut nodes, &raw)?;
        }
        if nodes.len() == before {
            bail!(
                "trie walk stalled at block {block}: {} subtrees are unexplained and a \
                 round of {} proof calls taught the walk nothing new. The endpoint is \
                 answering with proofs that do not cover the keys asked for; this is a \
                 provider fault, not a mirror fault.",
                unexplained.len(),
                keys.len().div_ceil(KEYS_PER_REQUEST)
            );
        }
        tracing::info!(
            round = diff.rounds,
            unexplained = unexplained.len(),
            nodes = nodes.len(),
            missing_so_far = diff.missing.len(),
            proof_calls = diff.proof_calls,
            "trie walk round"
        );
        frontier = unexplained;
    }

    diff.chain_leaves = chain_leaves;
    diff.missing.sort();
    diff.divergent.sort();
    diff.extra.sort();
    Ok(diff)
}

/// Turn missing slots into the blocks that wrote them.
///
/// Pool slots are write-once (96.95% of writes are first writes), so the first
/// block at which `getStorageAt` returns non-zero IS the writing block, found
/// in ⌈log₂(range)⌉ calls. The saving is the second half: that block's state
/// update names every slot it wrote, which claims the rest of the cluster for
/// free — so the bisection runs once per BLOCK, not once per slot.
pub async fn attribute_to_blocks(
    cutter: &Cutter<'_>,
    slots: &[Felt],
    from: u64,
    to: u64,
) -> Result<Vec<u64>> {
    let mut pending: HashSet<[u8; 32]> = slots.iter().map(|s| s.to_bytes_be()).collect();
    let mut blocks: Vec<u64> = Vec::new();
    let mut calls = 0usize;

    for slot in slots {
        if !pending.contains(&slot.to_bytes_be()) {
            continue; // already claimed by a block found for a sibling slot
        }
        // Invariant: zero at `lo`, non-zero at `hi`. `from - 1` is the pool's
        // pre-deployment state, where every slot is zero by construction.
        let (mut lo, mut hi) = (from.saturating_sub(1), to);
        while lo + 1 < hi {
            let mid = lo + (hi - lo) / 2;
            let v = cutter
                .rpc
                .get_storage_at(&cutter.cfg.pool, slot, BlockRef::Number(mid))
                .await
                .with_context(|| {
                    format!("getStorageAt({}, block {mid})", felt_hex(slot))
                })?;
            calls += 1;
            if v == Felt::ZERO {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        let block = hi;

        // Claim every missing slot this block wrote, so the next bisection
        // starts on a genuinely new cluster.
        let su = cutter
            .rpc
            .get_state_update(block)
            .await
            .with_context(|| format!("getStateUpdate({block}) after bisecting a missing slot"))?;
        let mut claimed = 0usize;
        for d in &su.state_diff.storage_diffs {
            let addr = crate::rpc::parse_felt(&d.address)?;
            if addr != cutter.cfg.pool {
                continue;
            }
            for e in &d.storage_entries {
                let k = crate::rpc::parse_felt(&e.key)?;
                if pending.remove(&k.to_bytes_be()) {
                    claimed += 1;
                }
            }
        }
        if claimed == 0 {
            // The bisection landed on a block whose state update does not name
            // the slot. Do not silently drop it: that would publish a feed
            // still missing the write.
            bail!(
                "slot {} first turns non-zero at block {block}, but that block's state \
                 update writes no pool slot. The chain is answering two questions \
                 inconsistently; stopping rather than repairing the wrong block.",
                felt_hex(slot)
            );
        }
        tracing::info!(
            block,
            claimed,
            remaining = pending.len(),
            bisect_calls = calls,
            "missing slots attributed to a block"
        );
        blocks.push(block);
    }

    blocks.sort_unstable();
    blocks.dedup();
    Ok(blocks)
}

fn absorb(nodes: &mut HashMap<[u8; 32], ProofNodeBody>, raw: &serde_json::Value) -> Result<()> {
    let Some(arr) = raw
        .get("contracts_storage_proofs")
        .and_then(|v| v.get(0))
        .and_then(|v| v.as_array())
    else {
        return Ok(());
    };
    for v in arr {
        let n: ProofNode =
            serde_json::from_value(v.clone()).context("decode a contracts_storage_proofs node")?;
        let h = felt_from_hex(&n.node_hash)?;
        nodes.entry(h.to_bytes_be()).or_insert(n.node);
    }
    Ok(())
}

/// The `length` low bits of an edge's `path` felt, MSB-first — the inverse of
/// the packing `mpt::path_to_key` performs.
fn edge_bits(path: &str, length: u64) -> Result<Vec<bool>> {
    let f = felt_from_hex(path)?;
    let bytes = f.to_bytes_be();
    let len = length as usize;
    if len > mpt::TREE_HEIGHT {
        bail!("edge length {len} exceeds tree height");
    }
    Ok((0..len)
        .map(|i| {
            let from_lsb = len - 1 - i;
            (bytes[31 - from_lsb / 8] >> (from_lsb % 8)) & 1 == 1
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn felt(n: u64) -> Felt {
        Felt::from(n)
    }

    /// The walk's prune test and the root computation must be the same
    /// quantity at the empty prefix, or the walk would descend into a tree it
    /// already agrees with (or worse, prune one it does not).
    #[test]
    fn empty_prefix_subtree_hash_is_the_root() {
        let set: Vec<(Felt, Felt)> = (1u64..64).map(|i| (felt(i * 7919), felt(i))).collect();
        let sorted = mpt::bit_entries(&set);
        assert_eq!(
            mpt::subtree_hash_at(&sorted, &[]),
            mpt::storage_root(&set),
            "subtree_hash_at(&[]) is the root by construction"
        );
    }

    /// A key whose top bit is `top`, so a test set can straddle the root's
    /// binary split. Plain small felts all share a top bit of 0 and would put
    /// the whole set in one half.
    fn key_with_top(top: bool, i: u64) -> Felt {
        let mut bits = vec![top];
        bits.extend_from_slice(&mpt::key_path(&felt(i * 104729))[1..]);
        mpt::path_to_key(&bits)
    }

    /// Descending one level must reproduce the parent from its two children
    /// exactly as a binary node does, otherwise a chain node's child hash is
    /// not comparable with the mirror's and every comparison the walk makes is
    /// noise.
    #[test]
    fn children_of_the_root_rebuild_it() {
        let mut set: Vec<(Felt, Felt)> = Vec::new();
        for i in 1u64..100 {
            set.push((key_with_top(false, i), felt(i)));
            set.push((key_with_top(true, i), felt(i + 1000)));
        }
        let sorted = mpt::bit_entries(&set);
        let l = mpt::subtree_hash_at(&sorted, &[false]);
        let r = mpt::subtree_hash_at(&sorted, &[true]);
        assert_ne!(l, Felt::ZERO, "left half is populated");
        assert_ne!(r, Felt::ZERO, "right half is populated");
        assert_eq!(
            starknet_crypto::pedersen_hash(&l, &r),
            mpt::storage_root(&set),
            "the root is the binary node over its two children"
        );
    }

    /// An empty half must score exactly zero, because that is the value a real
    /// binary node stores for an absent child — and it is what tells the walk
    /// "the chain has something here and we have nothing".
    #[test]
    fn an_empty_half_is_zero_not_a_hash() {
        let set: Vec<(Felt, Felt)> = (1u64..50).map(|i| (key_with_top(false, i), felt(i))).collect();
        let sorted = mpt::bit_entries(&set);
        assert_eq!(mpt::subtree_hash_at(&sorted, &[true]), Felt::ZERO);
        assert_ne!(mpt::subtree_hash_at(&sorted, &[false]), Felt::ZERO);
    }

    /// The descent rule, against a real mainnet proof rather than a synthetic
    /// one: absorb the node set the way the walk does, then follow the key's
    /// bit path from the root using only child hashes and `edge_bits`. It must
    /// arrive at the known leaf value.
    ///
    /// This is the load-bearing decode. If `edge_bits` unpacked a path even one
    /// bit differently, the walk would descend into the wrong subtree and
    /// report slots as missing that are not.
    #[test]
    fn the_descent_follows_a_live_proof_to_its_leaf() {
        let raw: serde_json::Value =
            serde_json::from_str(include_str!("../../../fixtures/proof_lava.json")).unwrap();
        let result = &raw["result"];
        let mut nodes = HashMap::new();
        absorb(&mut nodes, result).unwrap();
        assert_eq!(nodes.len(), 18, "the fixture's whole node set was absorbed");

        let root = felt_from_hex(
            result["contracts_proof"]["contract_leaves_data"][0]["storage_root"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        let key =
            felt_from_hex("0x18223681ac4182236a5f10794ec6fa3530a5cb1a18aff2005fbbed58772ec28")
                .unwrap();
        let path = mpt::key_path(&key);

        let mut cur = root;
        let mut depth = 0usize;
        while depth < mpt::TREE_HEIGHT {
            match nodes.get(&cur.to_bytes_be()).expect("node on the path") {
                ProofNodeBody::Binary { left, right } => {
                    cur = felt_from_hex(if path[depth] { right } else { left }).unwrap();
                    depth += 1;
                }
                ProofNodeBody::Edge {
                    path: p,
                    length,
                    child,
                } => {
                    let bits = edge_bits(p, *length).unwrap();
                    assert_eq!(
                        bits,
                        path[depth..depth + bits.len()],
                        "the edge's unpacked path must equal the key's bits there"
                    );
                    cur = felt_from_hex(child).unwrap();
                    depth += bits.len();
                }
            }
        }
        assert_eq!(depth, mpt::TREE_HEIGHT, "the walk lands exactly on a leaf");
        assert_eq!(
            cur,
            felt_from_hex("0x1eed60b8d483b3bede62d1cc0f32874aea30747e6943437c858359b41801bf7")
                .unwrap(),
            "and the leaf is the value the chain served"
        );
    }

    /// A leaf prefix resolves to the stored value: this is the walk's
    /// termination rule, and it is what lets a full-depth disagreement be read
    /// as "the mirror is missing this slot" rather than as a structural bug.
    #[test]
    fn a_full_depth_prefix_resolves_to_the_value() {
        let set = vec![(felt(12345), felt(999)), (felt(67890), felt(1000))];
        let sorted = mpt::bit_entries(&set);
        let path = mpt::key_path(&felt(12345));
        assert_eq!(mpt::subtree_hash_at(&sorted, &path), felt(999));
        assert_eq!(mpt::path_to_key(&path), felt(12345));
    }

    /// A slot absent from the mirror scores zero at its own prefix while the
    /// chain's leaf there is non-zero — the exact inequality the walk reports
    /// as `missing`.
    #[test]
    fn an_absent_slot_scores_zero_at_its_prefix() {
        let full = vec![(felt(12345), felt(999)), (felt(67890), felt(1000))];
        let holed = vec![(felt(12345), felt(999))];
        let sorted = mpt::bit_entries(&holed);
        let path = mpt::key_path(&felt(67890));
        assert_eq!(mpt::subtree_hash_at(&sorted, &path), Felt::ZERO);
        assert_ne!(mpt::storage_root(&holed), mpt::storage_root(&full));
    }

    #[test]
    fn edge_bits_invert_the_path_packing() {
        for (bits, len) in [
            (vec![true, false, true], 3usize),
            (vec![false, false, true, true, false], 5),
            (vec![true; 17], 17),
        ] {
            let packed = mpt::path_to_key(&bits);
            let back = edge_bits(&felt_hex(&packed), len as u64).unwrap();
            assert_eq!(back, bits);
        }
    }
}
