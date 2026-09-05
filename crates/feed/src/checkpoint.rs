//! Bind an untrusted contract proof to an independently trusted block header.
//! This returns the authenticated storage root; the caller must still compare
//! its complete state at that block. No intermediate history is authenticated.

use crate::mpt::{is_trie_key, verify_storage_proof, ProofNode, ProofOutcome};
use crate::{felt_from_hex, felt_hex, FeedError, Felt};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use starknet_crypto::{pedersen_hash, poseidon_hash_many};

/// Upper bound on the proof JSON, in bytes, checked before parsing. A pool
/// contract proof is a few KiB (tests/fixtures/checkpoint-mainnet.json is under
/// 7 KiB); the cap leaves room for verbose providers, not for a flood.
pub const MAX_PROOF_BYTES: usize = 256 * 1024;

/// Upper bound on `contracts_proof.nodes`, checked before any node is hashed.
/// One contract path is at most 251 nodes.
pub const MAX_PROOF_NODES: usize = 512;

/// The state-commitment domain tag, as the ASCII short string Starknet uses.
pub const STATE_COMMITMENT_TAG: &[u8] = b"STARKNET_STATE_V0";

/// What the host independently trusts about one block. Everything the proof
/// says is checked against this and never the other way round.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TrustedCheckpoint {
    pub chain_id: String,
    pub pool: Felt,
    pub block_number: u64,
    pub block_hash: Felt,
    pub state_root: Felt,
}

/// `"STARKNET_STATE_V0"` as a felt (big-endian ASCII bytes).
pub fn state_commitment_tag() -> Felt {
    let mut bytes = [0u8; 32];
    bytes[32 - STATE_COMMITMENT_TAG.len()..].copy_from_slice(STATE_COMMITMENT_TAG);
    Felt::from_bytes_be(&bytes)
}

/// The Starknet state commitment a block header carries as `new_root`.
pub fn state_commitment(contracts_tree_root: &Felt, classes_tree_root: &Felt) -> Felt {
    poseidon_hash_many(&[
        state_commitment_tag(),
        *contracts_tree_root,
        *classes_tree_root,
    ])
}

/// The contracts-trie leaf for a deployed contract:
/// `pedersen(pedersen(pedersen(class_hash, storage_root), nonce), 0)`.
pub fn contract_leaf_hash(class_hash: &Felt, storage_root: &Felt, nonce: &Felt) -> Felt {
    pedersen_hash(
        &pedersen_hash(&pedersen_hash(class_hash, storage_root), nonce),
        &Felt::ZERO,
    )
}

// ------------------------------------------------------------ proof shape

#[derive(Deserialize)]
struct ProofResult {
    contracts_proof: ContractsProof,
    global_roots: GlobalRoots,
}

#[derive(Deserialize)]
struct ContractsProof {
    nodes: Vec<ProofNode>,
    contract_leaves_data: Vec<ContractLeaf>,
}

#[derive(Deserialize)]
struct ContractLeaf {
    class_hash: String,
    nonce: String,
    storage_root: String,
    /// Not in the RPC schema; an endpoint that names the contract must not be
    /// naming a different one.
    #[serde(default)]
    contract_address: Option<String>,
    #[serde(default)]
    address: Option<String>,
}

#[derive(Deserialize)]
struct GlobalRoots {
    block_hash: String,
    classes_tree_root: String,
    contracts_tree_root: String,
}

fn malformed(msg: impl std::fmt::Display) -> FeedError {
    FeedError::Malformed(format!("CHECKPOINT_PROOF_MALFORMED: {msg}"))
}

fn felt(hex: &str, what: &str) -> Result<Felt, FeedError> {
    felt_from_hex(hex).map_err(|_| malformed(format!("{what} {hex:?} is not a felt")))
}

/// Unwrap a bare `result` object or a JSON-RPC envelope; an `error` envelope
/// is refused rather than read as a proof with missing fields.
fn unwrap_envelope(v: Value) -> Result<Value, FeedError> {
    if !v.is_object() {
        return Err(malformed("proof is not a JSON object"));
    }
    if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
        return Err(FeedError::Malformed(format!(
            "CHECKPOINT_RPC_ERROR: the endpoint returned a JSON-RPC error, not a proof: {err}"
        )));
    }
    Ok(match v.get("result") {
        Some(r) if !r.is_null() => r.clone(),
        _ => v,
    })
}

/// Verify `proof_json` (a `starknet_getStorageProof` result or its JSON-RPC
/// envelope) against `checkpoint` and return the pool's authenticated
/// `storage_root`. Any failure returns an error and no root.
pub fn verify_checkpoint(
    checkpoint: &TrustedCheckpoint,
    proof_json: &str,
) -> Result<Felt, FeedError> {
    // --- the checkpoint itself must be usable as a trust anchor
    if checkpoint.chain_id.is_empty() {
        return Err(FeedError::Malformed(
            "CHECKPOINT_INVALID: checkpoint has an empty chain_id".into(),
        ));
    }
    if checkpoint.pool == Felt::ZERO || !is_trie_key(&checkpoint.pool) {
        return Err(FeedError::Malformed(format!(
            "CHECKPOINT_INVALID: pool {} is not a contract address (must be non-zero and \
             below 2^251; a larger felt would alias the address of its low 251 bits)",
            felt_hex(&checkpoint.pool)
        )));
    }
    if checkpoint.block_hash == Felt::ZERO || checkpoint.state_root == Felt::ZERO {
        return Err(FeedError::Malformed(format!(
            "CHECKPOINT_INVALID: block {} has a zero block_hash or state_root",
            checkpoint.block_number
        )));
    }

    // --- bounds before any work
    if proof_json.len() > MAX_PROOF_BYTES {
        return Err(FeedError::Malformed(format!(
            "CHECKPOINT_PROOF_TOO_LARGE: {} bytes, past the {MAX_PROOF_BYTES}-byte cap",
            proof_json.len()
        )));
    }
    let raw: Value = serde_json::from_str(proof_json)
        .map_err(|e| malformed(format!("proof is not JSON: {e}")))?;
    let result = unwrap_envelope(raw)?;
    let node_count = result
        .get("contracts_proof")
        .and_then(|c| c.get("nodes"))
        .and_then(Value::as_array)
        .map_or(0, Vec::len);
    if node_count > MAX_PROOF_NODES {
        return Err(FeedError::Malformed(format!(
            "CHECKPOINT_PROOF_TOO_LARGE: {node_count} contracts_proof nodes, past the \
             {MAX_PROOF_NODES}-node cap"
        )));
    }
    let proof: ProofResult = serde_json::from_value(result)
        .map_err(|e| malformed(format!("not a storage proof result: {e}")))?;
    let leaf = match proof.contracts_proof.contract_leaves_data.as_slice() {
        [one] => one,
        other => {
            return Err(malformed(format!(
                "expected exactly one contract_leaves_data entry (the pool), got {}",
                other.len()
            )))
        }
    };

    // --- 1. the proof is about the trusted block
    let proof_block_hash = felt(&proof.global_roots.block_hash, "global_roots.block_hash")?;
    if proof_block_hash != checkpoint.block_hash {
        return Err(FeedError::Malformed(format!(
            "CHECKPOINT_BLOCK_HASH_MISMATCH: proof is about block hash {}, checkpoint block \
             {} is {}",
            felt_hex(&proof_block_hash),
            checkpoint.block_number,
            felt_hex(&checkpoint.block_hash)
        )));
    }

    // --- 2. the global roots are the ones the trusted state root commits to
    let contracts_tree_root = felt(
        &proof.global_roots.contracts_tree_root,
        "global_roots.contracts_tree_root",
    )?;
    let classes_tree_root = felt(
        &proof.global_roots.classes_tree_root,
        "global_roots.classes_tree_root",
    )?;
    let commitment = state_commitment(&contracts_tree_root, &classes_tree_root);
    if commitment != checkpoint.state_root {
        return Err(FeedError::Malformed(format!(
            "CHECKPOINT_STATE_ROOT_MISMATCH: global roots commit to {}, checkpoint block {} \
             has state root {}",
            felt_hex(&commitment),
            checkpoint.block_number,
            felt_hex(&checkpoint.state_root)
        )));
    }

    // --- 3. the leaf is about the pool and is reachable from the contracts root
    for named in [&leaf.contract_address, &leaf.address]
        .into_iter()
        .flatten()
    {
        let addr = felt(named, "contract_leaves_data[0] address")?;
        if addr != checkpoint.pool {
            return Err(FeedError::Malformed(format!(
                "CHECKPOINT_ADDRESS_MISMATCH: proof leaf is about contract {}, not the pool {}",
                felt_hex(&addr),
                felt_hex(&checkpoint.pool)
            )));
        }
    }
    let class_hash = felt(&leaf.class_hash, "contract_leaves_data[0].class_hash")?;
    let nonce = felt(&leaf.nonce, "contract_leaves_data[0].nonce")?;
    let storage_root = felt(&leaf.storage_root, "contract_leaves_data[0].storage_root")?;
    if class_hash == Felt::ZERO {
        return Err(FeedError::Malformed(format!(
            "CHECKPOINT_NOT_DEPLOYED: proof leaf for {} carries class_hash 0x0",
            felt_hex(&checkpoint.pool)
        )));
    }
    let outcome = verify_storage_proof(
        contracts_tree_root,
        &proof.contracts_proof.nodes,
        checkpoint.pool,
    )
    .map_err(|e| FeedError::Malformed(format!("CHECKPOINT_PROOF_INVALID: {e}")))?;
    let leaf_hash = match outcome {
        ProofOutcome::Member(h) => h,
        ProofOutcome::NonMember => {
            return Err(FeedError::Malformed(format!(
                "CHECKPOINT_NOT_DEPLOYED: contracts trie at block {} has no leaf for {}",
                checkpoint.block_number,
                felt_hex(&checkpoint.pool)
            )))
        }
    };

    // --- 4. the leaf commits to exactly this class, root and nonce
    let expected = contract_leaf_hash(&class_hash, &storage_root, &nonce);
    if leaf_hash != expected {
        return Err(FeedError::Malformed(format!(
            "CHECKPOINT_LEAF_MISMATCH: contracts trie leaf for {} is {}, but class_hash {} / \
             storage_root {} / nonce {} hash to {}",
            felt_hex(&checkpoint.pool),
            felt_hex(&leaf_hash),
            felt_hex(&class_hash),
            felt_hex(&storage_root),
            felt_hex(&nonce),
            felt_hex(&expected)
        )));
    }
    Ok(storage_root)
}
