use serde_json::{json, Value};
use strk20_feed::{
    checkpoint::{verify_checkpoint, TrustedCheckpoint, MAX_PROOF_BYTES},
    Felt,
};

fn fixture() -> (TrustedCheckpoint, Value) {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/checkpoint-mainnet.json")).unwrap();
    (
        serde_json::from_value(fixture["checkpoint"].clone()).unwrap(),
        fixture["proof"].clone(),
    )
}

#[test]
fn real_mainnet_contract_path_reaches_independent_checkpoint() {
    let (cp, proof) = fixture();
    assert_eq!(
        verify_checkpoint(&cp, &proof.to_string()).unwrap(),
        Felt::from_hex("0x16d76033a65fa79272e8326dbbb543c4e3bcce6e8395531286347a9f216c270")
            .unwrap()
    );
    assert!(verify_checkpoint(&cp, &json!({"result":proof}).to_string()).is_ok());
}

#[test]
fn tampering_each_link_is_rejected() {
    let (cp, proof) = fixture();
    for pointer in [
        "/global_roots/block_hash",
        "/global_roots/contracts_tree_root",
        "/global_roots/classes_tree_root",
        "/contracts_proof/contract_leaves_data/0/class_hash",
        "/contracts_proof/contract_leaves_data/0/nonce",
        "/contracts_proof/contract_leaves_data/0/storage_root",
        "/contracts_proof/nodes/0/node_hash",
    ] {
        let mut bad = proof.clone();
        *bad.pointer_mut(pointer).unwrap() = json!("0x123");
        assert!(
            verify_checkpoint(&cp, &bad.to_string()).is_err(),
            "{pointer}"
        );
    }
    let mut bad = proof.clone();
    bad["contracts_proof"]["nodes"] = json!([]);
    assert!(verify_checkpoint(&cp, &bad.to_string()).is_err());
    let mut bad = cp.clone();
    bad.pool += Felt::ONE;
    assert!(verify_checkpoint(&bad, &proof.to_string()).is_err());
    let mut bad = cp.clone();
    bad.state_root += Felt::ONE;
    assert!(verify_checkpoint(&bad, &proof.to_string()).is_err());
    assert!(verify_checkpoint(&cp, &" ".repeat(MAX_PROOF_BYTES + 1)).is_err());
}

#[test]
fn malformed_paths_are_bounded_and_unambiguous() {
    use strk20_feed::mpt::{edge_hash, verify_storage_proof, ProofNode};
    for (length, path) in [(0, Felt::ZERO), (252, Felt::ZERO), (1, Felt::from(2u64))] {
        let root = edge_hash(&Felt::ONE, &path, length);
        let n: ProofNode = serde_json::from_value(
            json!({"node_hash":root,"node":{"child":"0x1","path":path,"length":length}}),
        )
        .unwrap();
        assert!(verify_storage_proof(root, &[n], Felt::ZERO).is_err());
    }
    let (cp, mut proof) = fixture();
    let first = proof["contracts_proof"]["nodes"][0].clone();
    proof["contracts_proof"]["nodes"]
        .as_array_mut()
        .unwrap()
        .push(first);
    assert!(verify_checkpoint(&cp, &proof.to_string()).is_err());
}
