//! The `contracts_storage_proofs` node set a real `starknet_getStorageProof`
//! returns, computed from the fixture chain's slot set.
//!
//! The fixture used to answer `"contracts_storage_proofs": [[]]` — enough for
//! `verify-root`, which only ever reads `contract_leaves_data[0].storage_root`,
//! and useless for anything that has to DESCEND. The sound-ingest.md §4.2
//! closure loop descends: it compares the chain's child hash for a bit-prefix
//! against the same quantity folded from the mirror, and asks for a proof of a
//! crafted key whenever it meets a subtree it cannot explain. With an empty
//! node set every one of those requests teaches it nothing, and
//! `trie_walk::enumerate_missing_slots` correctly reports the endpoint as
//! faulty — so the recovery path could not be tested end to end at all.
//!
//! The generator walks the canonical trie from the root towards each requested
//! key and emits every node on the way, which is exactly a proof: the client
//! recomputes each node's hash from its contents, so a node set that is not
//! the real one cannot be believed by `mpt::verify_storage_proof` either. The
//! structure it must reproduce is `mpt::subtree`'s:
//!
//! - a subtree holding one entry is a single edge covering every remaining bit;
//! - a subtree whose entries all share their next `k` bits is an edge of
//!   length `k` over the subtree below them;
//! - otherwise it is a binary node over its two halves.
//!
//! Both quantities come from `mpt` itself (`subtree_hash_at`,
//! `entries_with_prefix`), so a fixture proof and the mirror's own root cannot
//! drift apart by construction.

use serde_json::{json, Value};
use starknet_types_core::felt::Felt;
use std::collections::BTreeMap;
use strk20_feed::felt_hex;
use strk20_feed::mpt;

/// Every node on the root→key path for each of `keys`, deduplicated and
/// ordered by node hash. An empty `keys` list yields an empty set, which is
/// what an endpoint returns for the membership-free probe `verify-root` makes.
pub fn nodes_for(entries: &[(Felt, Felt)], keys: &[Felt]) -> Vec<Value> {
    let sorted = mpt::bit_entries(entries);
    let mut out: BTreeMap<[u8; 32], Value> = BTreeMap::new();
    for key in keys {
        let path = mpt::key_path(key);
        let mut prefix: Vec<bool> = Vec::new();
        loop {
            let slice = mpt::entries_with_prefix(&sorted, &prefix);
            // An empty subtree has no node: its parent stores 0 for it, which
            // is already proof of non-membership.
            if slice.is_empty() || prefix.len() == mpt::TREE_HEIGHT {
                break;
            }
            let node_hash = mpt::subtree_hash_at(&sorted, &prefix);
            // Sorted bit-lexicographically, so the first and last entries
            // agreeing on a bit means every entry between them does too.
            let (first, last) = (&slice[0].0, &slice[slice.len() - 1].0);
            let mut end = prefix.len();
            while end < mpt::TREE_HEIGHT && first[end] == last[end] {
                end += 1;
            }
            if end > prefix.len() {
                let edge: Vec<bool> = first[prefix.len()..end].to_vec();
                let child = mpt::subtree_hash_at(&sorted, &first[..end]);
                out.insert(
                    node_hash.to_bytes_be(),
                    json!({
                        "node_hash": felt_hex(&node_hash),
                        "node": {
                            "path": felt_hex(&mpt::path_to_key(&edge)),
                            "length": edge.len(),
                            "child": felt_hex(&child),
                        }
                    }),
                );
                if path[prefix.len()..end] != edge[..] {
                    // The key leaves the trie here; the edge just emitted is
                    // the whole proof of its absence.
                    break;
                }
                prefix = first[..end].to_vec();
            } else {
                let mut left = prefix.clone();
                left.push(false);
                let mut right = prefix.clone();
                right.push(true);
                out.insert(
                    node_hash.to_bytes_be(),
                    json!({
                        "node_hash": felt_hex(&node_hash),
                        "node": {
                            "left": felt_hex(&mpt::subtree_hash_at(&sorted, &left)),
                            "right": felt_hex(&mpt::subtree_hash_at(&sorted, &right)),
                        }
                    }),
                );
                prefix.push(path[prefix.len()]);
            }
        }
    }
    out.into_values().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use strk20_feed::mpt::{ProofNode, ProofOutcome};

    fn set(n: u64) -> Vec<(Felt, Felt)> {
        (1..=n)
            .map(|i| (Felt::from(i * 104_729), Felt::from(i + 7)))
            .collect()
    }

    fn parsed(entries: &[(Felt, Felt)], keys: &[Felt]) -> Vec<ProofNode> {
        nodes_for(entries, keys)
            .into_iter()
            .map(|v| serde_json::from_value(v).expect("a well-formed proof node"))
            .collect()
    }

    /// The generated set must satisfy the SAME verifier a client runs against
    /// a live endpoint's proof — every node's hash recomputed from its
    /// contents, up to the root the mirror folds independently. If it does
    /// not, the fixture is teaching the recovery path something the chain
    /// never would.
    #[test]
    fn a_generated_proof_verifies_for_every_member() {
        let entries = set(40);
        let root = mpt::storage_root(&entries);
        for (key, value) in &entries {
            let nodes = parsed(&entries, std::slice::from_ref(key));
            assert_eq!(
                mpt::verify_storage_proof(root, &nodes, *key).unwrap(),
                ProofOutcome::Member(*value),
                "key {} must verify to its value",
                felt_hex(key)
            );
        }
    }

    /// Non-membership is the half the trie walk depends on: it asks about
    /// keys nobody has written, purely to be shown the nodes above them.
    #[test]
    fn a_generated_proof_shows_absence_for_a_key_never_written() {
        let entries = set(40);
        let root = mpt::storage_root(&entries);
        let absent = Felt::from(0xdead_beefu64);
        let nodes = parsed(&entries, &[absent]);
        assert_eq!(
            mpt::verify_storage_proof(root, &nodes, absent).unwrap(),
            ProofOutcome::NonMember
        );
    }

    /// One request carrying many keys must answer them all — the walk batches
    /// 64 crafted keys per call and would stall if the union were incomplete.
    #[test]
    fn one_response_answers_every_key_it_was_asked_about() {
        let entries = set(40);
        let root = mpt::storage_root(&entries);
        let keys: Vec<Felt> = entries.iter().map(|(k, _)| *k).take(9).collect();
        let nodes = parsed(&entries, &keys);
        for (key, value) in entries.iter().take(9) {
            assert_eq!(
                mpt::verify_storage_proof(root, &nodes, *key).unwrap(),
                ProofOutcome::Member(*value)
            );
        }
    }

    /// The degenerate shapes: one leaf is a full-height edge, and an empty
    /// trie has no nodes to serve at all.
    #[test]
    fn the_degenerate_tries_are_shaped_right() {
        let one = vec![(Felt::from(12_345u64), Felt::from(9u64))];
        let nodes = parsed(&one, &[Felt::from(12_345u64)]);
        assert_eq!(nodes.len(), 1, "a lone leaf is exactly one edge node");
        assert_eq!(
            mpt::verify_storage_proof(mpt::storage_root(&one), &nodes, Felt::from(12_345u64))
                .unwrap(),
            ProofOutcome::Member(Felt::from(9u64))
        );
        assert!(nodes_for(&[], &[Felt::from(1u64)]).is_empty());
    }
}
