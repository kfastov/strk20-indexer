//! Starknet contract-storage Merkle-Patricia trie (pedersen, height 251).
//!
//! Two consumers share this single implementation (spec §5.6, §7.7):
//! the server's verify-root completeness check (recompute the full root from
//! the mirrored slot set, compare against `starknet_getStorageProof`), and the
//! client's proof verifier (walk a served proof for the user's own slots,
//! including non-membership = unspent).
//!
//! Node hashing rules (docs.starknet.io/architecture/state):
//! - binary node:  h = pedersen(left, right)
//! - edge node:    h = pedersen(child, path) + length   (field addition)
//! - leaf:         h = value
//! - empty subtree: 0

use crate::{FeedError, Felt};
use serde::Deserialize;
use starknet_crypto::pedersen_hash;

pub const TREE_HEIGHT: usize = 251;

/// Bit of `f` at position `depth` counting from the top of the 251-bit key
/// (depth 0 = most significant of the 251 bits).
fn key_bit(bytes: &[u8; 32], depth: usize) -> bool {
    debug_assert!(depth < TREE_HEIGHT);
    let bit_from_lsb = TREE_HEIGHT - 1 - depth; // 0..=250
    let byte = bytes[31 - bit_from_lsb / 8];
    (byte >> (bit_from_lsb % 8)) & 1 == 1
}

fn key_bits(f: &Felt) -> Vec<bool> {
    let bytes = f.to_bytes_be();
    (0..TREE_HEIGHT).map(|d| key_bit(&bytes, d)).collect()
}

/// MSB-first bit path -> felt (bit 0 of the slice is the most significant).
fn bits_to_felt(bits: &[bool]) -> Felt {
    let mut bytes = [0u8; 32];
    let len = bits.len();
    for (i, bit) in bits.iter().enumerate() {
        if *bit {
            let bit_from_lsb = len - 1 - i;
            bytes[31 - bit_from_lsb / 8] |= 1 << (bit_from_lsb % 8);
        }
    }
    Felt::from_bytes_be(&bytes)
}

// ------------------------------------------------------------- full root

/// An internal subtree summarized as an (possibly zero-length) edge over a
/// bottom hash.
struct SubNode {
    path: Vec<bool>,
    bottom: Felt,
}

fn resolve(n: &SubNode) -> Felt {
    if n.path.is_empty() {
        n.bottom
    } else {
        pedersen_hash(&n.bottom, &bits_to_felt(&n.path)) + Felt::from(n.path.len() as u64)
    }
}

fn subtree(entries: &[(Vec<bool>, Felt)], depth: usize) -> Option<SubNode> {
    match entries.len() {
        0 => return None,
        1 => {
            // A lone leaf: one edge covering all remaining bits.
            let (bits, value) = &entries[0];
            return Some(SubNode {
                path: bits[depth..].to_vec(),
                bottom: *value,
            });
        }
        _ => {}
    }
    debug_assert!(depth < TREE_HEIGHT, "duplicate keys in storage set");
    // entries are sorted by bits, so the split point is the first `true`.
    let split = entries.partition_point(|(bits, _)| !bits[depth]);
    let left = subtree(&entries[..split], depth + 1);
    let right = subtree(&entries[split..], depth + 1);
    match (left, right) {
        (Some(l), Some(r)) => Some(SubNode {
            path: Vec::new(),
            bottom: pedersen_hash(&resolve(&l), &resolve(&r)),
        }),
        (Some(mut l), None) => {
            l.path.insert(0, false);
            Some(l)
        }
        (None, Some(mut r)) => {
            r.path.insert(0, true);
            Some(r)
        }
        (None, None) => unreachable!(),
    }
}

/// Recompute a contract's storage root from the complete slot set.
/// Zero-valued entries must be excluded by the caller (a zero write never
/// occurs in the pool: all discovery slots are write-once non-zero).
pub fn storage_root(entries: &[(Felt, Felt)]) -> Felt {
    subtree_hash_at(&bit_entries(entries), &[])
}

// ------------------------------------------------------------- proof walk

#[derive(Debug, Clone, Deserialize)]
#[serde(untagged)]
pub enum ProofNodeBody {
    Binary { left: String, right: String },
    Edge { path: String, length: u64, child: String },
}

#[derive(Debug, Clone, Deserialize)]
pub struct ProofNode {
    pub node_hash: String,
    pub node: ProofNodeBody,
}

/// Outcome of walking a storage proof for one key.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProofOutcome {
    /// The walk reached a leaf; this is its value.
    Member(Felt),
    /// The walk proved the key absent (value is zero).
    NonMember,
}

/// Verify a `contracts_storage_proofs` node set against `storage_root` for
/// `key`. Every visited node's claimed hash is recomputed from its contents;
/// any mismatch or missing node is an error (an incomplete proof proves
/// nothing).
pub fn verify_storage_proof(
    root: Felt,
    nodes: &[ProofNode],
    key: Felt,
) -> Result<ProofOutcome, FeedError> {
    use std::collections::HashMap;
    let mut by_hash: HashMap<[u8; 32], &ProofNode> = HashMap::with_capacity(nodes.len());
    for n in nodes {
        let h = crate::felt_from_hex(&n.node_hash)?;
        by_hash.insert(h.to_bytes_be(), n);
    }
    let key_bytes = key.to_bytes_be();
    let mut cur = root;
    let mut depth = 0usize;
    while depth < TREE_HEIGHT {
        if cur == Felt::ZERO {
            return Ok(ProofOutcome::NonMember);
        }
        let node = by_hash.get(&cur.to_bytes_be()).ok_or_else(|| {
            FeedError::Malformed(format!(
                "proof incomplete: node {} not present (depth {depth})",
                crate::felt_hex(&cur)
            ))
        })?;
        match &node.node {
            ProofNodeBody::Binary { left, right } => {
                let l = crate::felt_from_hex(left)?;
                let r = crate::felt_from_hex(right)?;
                let recomputed = pedersen_hash(&l, &r);
                if recomputed != cur {
                    return Err(FeedError::Malformed(format!(
                        "binary node hash mismatch at depth {depth}"
                    )));
                }
                cur = if key_bit(&key_bytes, depth) { r } else { l };
                depth += 1;
            }
            ProofNodeBody::Edge {
                path,
                length,
                child,
            } => {
                let path_felt = crate::felt_from_hex(path)?;
                let child_felt = crate::felt_from_hex(child)?;
                let recomputed =
                    pedersen_hash(&child_felt, &path_felt) + Felt::from(*length);
                if recomputed != cur {
                    return Err(FeedError::Malformed(format!(
                        "edge node hash mismatch at depth {depth}"
                    )));
                }
                let len = *length as usize;
                if depth + len > TREE_HEIGHT {
                    return Err(FeedError::Malformed("edge overruns tree height".into()));
                }
                // Compare the edge path bits against the key bits.
                let path_bytes = path_felt.to_bytes_be();
                let mut diverged = false;
                for i in 0..len {
                    // bit i of the path, MSB-first within `len` bits
                    let bit_from_lsb = len - 1 - i;
                    let path_bit =
                        (path_bytes[31 - bit_from_lsb / 8] >> (bit_from_lsb % 8)) & 1 == 1;
                    if path_bit != key_bit(&key_bytes, depth + i) {
                        diverged = true;
                        break;
                    }
                }
                if diverged {
                    return Ok(ProofOutcome::NonMember);
                }
                cur = child_felt;
                depth += len;
            }
        }
    }
    Ok(ProofOutcome::Member(cur))
}

// ------------------------------------------------- structural enumeration
//
// `verify_storage_proof` above answers "is THIS key in the chain's trie",
// which presupposes you can name the key. The hole class in sound-ingest.md §1
// is precisely the one where you cannot: a block with pool storage writes and
// zero pool events is invisible to `getEvents` and to `audit-coverage` alike,
// so the mirror has never heard of the slots it wrote and has no key to ask
// about.
//
// What the chain will still tell you is STRUCTURE. A storage proof returns
// nodes keyed by their own hash, and every node names its children by hash, so
// the child hash for a given bit-prefix is a commitment to the entire subtree
// under that prefix. The same quantity is computable from the mirror. Where
// the two agree the subtrees are identical and neither side needs to be
// fetched; where they disagree the walk descends, and it terminates on leaves
// the mirror does not have — the missing slots, named without ever having
// guessed them.
//
// These three functions are that comparison unit. The walk itself is
// `strk20_indexerd::trie_walk`.

/// The 251-bit MSB-first key path of `f`.
pub fn key_path(f: &Felt) -> Vec<bool> {
    key_bits(f)
}

/// Rebuild a felt from a bit path (MSB-first). Padding a shorter prefix out to
/// [`TREE_HEIGHT`] yields a key that routes into that prefix's subtree, which
/// is how the walk asks the chain to reveal a subtree it cannot yet explain.
pub fn path_to_key(bits: &[bool]) -> Felt {
    bits_to_felt(bits)
}

/// Sort a slot set into the bit-keyed, zero-free form the walk compares
/// against. [`storage_root`] is defined as `subtree_hash_at` over this at the
/// empty prefix, so the mirror's root and the quantity the walk compares
/// against chain child hashes cannot drift apart by construction.
pub fn bit_entries(entries: &[(Felt, Felt)]) -> Vec<(Vec<bool>, Felt)> {
    let mut with_bits: Vec<(Vec<bool>, Felt)> = entries
        .iter()
        .filter(|(_, v)| *v != Felt::ZERO)
        .map(|(k, v)| (key_bits(k), *v))
        .collect();
    with_bits.sort_by(|a, b| a.0.cmp(&b.0));
    with_bits.dedup_by(|a, b| a.0 == b.0);
    with_bits
}

/// The entries of `sorted` whose key begins with `prefix` (a contiguous run,
/// because `sorted` is in bit-lexicographic order).
pub fn entries_with_prefix<'a>(
    sorted: &'a [(Vec<bool>, Felt)],
    prefix: &[bool],
) -> &'a [(Vec<bool>, Felt)] {
    let d = prefix.len();
    let lo = sorted.partition_point(|(bits, _)| bits[..d] < *prefix);
    let hi = sorted.partition_point(|(bits, _)| bits[..d] <= *prefix);
    &sorted[lo..hi]
}

/// The hash a parent node stores for the child covering every key beginning
/// with `prefix` — `0` when this set holds no such key (the empty subtree).
///
/// Directly comparable with the child hash a chain proof node names for the
/// same prefix, which is the whole point: equality proves the two subtrees are
/// identical, so the walk can prune without a single extra request.
pub fn subtree_hash_at(sorted: &[(Vec<bool>, Felt)], prefix: &[bool]) -> Felt {
    let slice = entries_with_prefix(sorted, prefix);
    match subtree(slice, prefix.len()) {
        None => Felt::ZERO,
        Some(n) => resolve(&n),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::felt_from_hex;

    /// The archived live mainnet proof (fixtures/proof_lava.json): pool
    /// 0x040337b1…, slot auditor_public_key, value 0x1eed60b8… at the proof's
    /// block. Validates binary/edge/leaf hashing against reality.
    #[test]
    fn live_proof_walks_to_known_value() {
        let raw = include_str!("../../../fixtures/proof_lava.json");
        let v: serde_json::Value = serde_json::from_str(raw).unwrap();
        let r = &v["result"];
        let root = felt_from_hex(
            r["contracts_proof"]["contract_leaves_data"][0]["storage_root"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        let nodes: Vec<ProofNode> =
            serde_json::from_value(r["contracts_storage_proofs"][0].clone()).unwrap();
        let key = felt_from_hex(
            "0x18223681ac4182236a5f10794ec6fa3530a5cb1a18aff2005fbbed58772ec28",
        )
        .unwrap();
        let outcome = verify_storage_proof(root, &nodes, key).unwrap();
        let expected = felt_from_hex(
            "0x1eed60b8d483b3bede62d1cc0f32874aea30747e6943437c858359b41801bf7",
        )
        .unwrap();
        assert_eq!(outcome, ProofOutcome::Member(expected));
    }

    #[test]
    fn live_proof_nonmember_for_absent_key() {
        let raw = include_str!("../../../fixtures/proof_lava.json");
        let v: serde_json::Value = serde_json::from_str(raw).unwrap();
        let r = &v["result"];
        let root = felt_from_hex(
            r["contracts_proof"]["contract_leaves_data"][0]["storage_root"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
        let nodes: Vec<ProofNode> =
            serde_json::from_value(r["contracts_storage_proofs"][0].clone()).unwrap();
        // Flip a low bit of the proven key: the walk must either diverge on an
        // edge (NonMember) or fail on a missing node (incomplete proof) — it
        // must NOT return Member.
        let key = felt_from_hex(
            "0x18223681ac4182236a5f10794ec6fa3530a5cb1a18aff2005fbbed58772ec29",
        )
        .unwrap();
        match verify_storage_proof(root, &nodes, key) {
            Ok(ProofOutcome::Member(_)) => panic!("must not prove membership for absent key"),
            Ok(ProofOutcome::NonMember) | Err(_) => {}
        }
    }

    #[test]
    fn empty_root_is_zero() {
        assert_eq!(storage_root(&[]), Felt::ZERO);
    }

    #[test]
    fn single_entry_root_is_full_height_edge() {
        let k = felt_from_hex("0x18223681ac4182236a5f10794ec6fa3530a5cb1a18aff2005fbbed58772ec28")
            .unwrap();
        let val = Felt::from(7u64);
        let root = storage_root(&[(k, val)]);
        // A single leaf is one edge of length 251 whose path is the whole key.
        let expected = pedersen_hash(&val, &k) + Felt::from(TREE_HEIGHT as u64);
        assert_eq!(root, expected);
    }

    #[test]
    fn zero_values_are_excluded() {
        let k = Felt::from(5u64);
        assert_eq!(storage_root(&[(k, Felt::ZERO)]), Felt::ZERO);
    }

    /// Root over a set must be verifiable by hand-assembling the two-leaf
    /// case: keys 0b...00 and 0b...01 share 250 bits.
    #[test]
    fn two_adjacent_leaves() {
        let k0 = Felt::from(4u64); // ...100
        let k1 = Felt::from(5u64); // ...101
        let v0 = Felt::from(11u64);
        let v1 = Felt::from(22u64);
        let root = storage_root(&[(k1, v1), (k0, v0)]); // order-insensitive
        // depth 250 binary node over the two leaves:
        let bin = pedersen_hash(&v0, &v1);
        // edge of length 250 down to it, path = top 250 bits of k0 (= k0 >> 1)
        let path = Felt::from(2u64); // 4 >> 1
        let expected = pedersen_hash(&bin, &path) + Felt::from(250u64);
        assert_eq!(root, expected);
    }
}
