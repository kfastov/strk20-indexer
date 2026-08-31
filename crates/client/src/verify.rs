//! U6 auditor path (spec §7.7): verify the client's own discovered state
//! against Starknet state roots via `starknet_getStorageProof` fetched from
//! the USER'S OWN RPC. The indexer plays no role here. Nullifier-slot
//! non-membership proves un-spent-ness.

use crate::store::FeedStore;
use strk20_consumer::store::ConsumerStore;
use anyhow::{anyhow, bail, Context, Result};
use serde_json::{json, Value};
use starknet_types_core::felt::Felt;
use strk20_feed::mpt::{verify_storage_proof, ProofNode, ProofOutcome};

async fn fetch_proof(rpc: &str, contract: &Felt, keys: &[Felt]) -> Result<Value> {
    let http = reqwest::Client::builder()
        .user_agent(concat!("strk20-sync/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let body = json!({
        "jsonrpc": "2.0", "id": 1, "method": "starknet_getStorageProof",
        "params": [
            "latest",
            Value::Null,
            [strk20_feed::felt_hex(contract)],
            [{
                "contract_address": strk20_feed::felt_hex(contract),
                "storage_keys": keys.iter().map(strk20_feed::felt_hex).collect::<Vec<_>>(),
            }]
        ]
    });
    let v: Value = http
        .post(rpc)
        .json(&body)
        .send()
        .await
        .with_context(|| format!("POST {rpc}"))?
        .error_for_status()?
        .json()
        .await?;
    if let Some(err) = v.get("error").filter(|e| !e.is_null()) {
        bail!("rpc error: {err}");
    }
    v.get("result")
        .cloned()
        .ok_or_else(|| anyhow!("no result in storage proof response"))
}

/// Verify every note of `owner` in the registry: note slot membership with
/// the mirrored value, and nullifier slot membership/non-membership matching
/// the recorded spent flag. Returns a JSON report.
pub async fn verify_owner(store: &FeedStore, rpc: &str, owner: &Felt) -> Result<Value> {
    let pool_hex = store
        .meta_get("pool")?
        .ok_or_else(|| anyhow!("mirror has no pool metadata; sync first"))?;
    let pool = Felt::from_hex(&pool_hex).map_err(|_| anyhow!("bad pool metadata"))?;
    let notes = store.notes(owner)?;
    if notes.is_empty() {
        bail!("no notes in the registry for this address; sync first");
    }

    let mut keys = Vec::new();
    for n in &notes {
        keys.push(discovery_core::privacy_pool::storage_slots::notes(n.note_id));
        keys.push(discovery_core::privacy_pool::storage_slots::nullifiers(
            n.nullifier,
        ));
    }
    let proof = fetch_proof(rpc, &pool, &keys).await?;
    let leaf = &proof["contracts_proof"]["contract_leaves_data"][0];
    let storage_root = Felt::from_hex(
        leaf["storage_root"]
            .as_str()
            .ok_or_else(|| anyhow!("proof has no storage_root"))?,
    )
    .map_err(|_| anyhow!("bad storage_root"))?;
    let class_hash = leaf["class_hash"].as_str().unwrap_or_default().to_owned();
    let nodes: Vec<ProofNode> =
        serde_json::from_value(proof["contracts_storage_proofs"][0].clone())?;

    let mut results = Vec::new();
    let mut all_ok = true;
    let head: u64 = store
        .meta_get("head_number")?
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let view = store.view(head)?;
    for n in &notes {
        let note_slot = discovery_core::privacy_pool::storage_slots::notes(n.note_id);
        let nullifier_slot =
            discovery_core::privacy_pool::storage_slots::nullifiers(n.nullifier);
        let mirror_note = view_read(&view, &note_slot)?;
        let note_check = match verify_storage_proof(storage_root, &nodes, note_slot) {
            Ok(ProofOutcome::Member(v)) => {
                if v == mirror_note {
                    "proven"
                } else {
                    all_ok = false;
                    "MISMATCH: chain value differs from mirror"
                }
            }
            Ok(ProofOutcome::NonMember) => {
                all_ok = false;
                "MISSING: note slot empty on chain"
            }
            Err(_) => {
                all_ok = false;
                "UNPROVABLE: note slot not covered by the proof"
            }
        };
        let spent_check = match verify_storage_proof(storage_root, &nodes, nullifier_slot) {
            Ok(ProofOutcome::Member(_)) => {
                if n.spent {
                    "spent-proven"
                } else {
                    all_ok = false;
                    "MISMATCH: chain says spent, registry says unspent"
                }
            }
            Ok(ProofOutcome::NonMember) => {
                if n.spent {
                    all_ok = false;
                    "MISMATCH: chain says unspent, registry says spent"
                } else {
                    "unspent-proven"
                }
            }
            Err(_) => {
                all_ok = false;
                "UNPROVABLE: nullifier slot not covered by the proof"
            }
        };
        results.push(json!({
            "note_id": strk20_feed::felt_hex(&n.note_id),
            "token": strk20_feed::felt_hex(&n.token),
            "amount": n.amount.to_string(),
            "note": note_check,
            "spent_state": spent_check,
        }));
    }
    Ok(json!({
        "address": strk20_feed::felt_hex(owner),
        "pool": pool_hex,
        "storage_root": strk20_feed::felt_hex(&storage_root),
        "pool_class_hash": class_hash,
        "all_ok": all_ok,
        "notes": results,
    }))
}

fn view_read(view: &crate::store::ClientView, slot: &Felt) -> Result<Felt> {
    // synchronous helper over the async trait for CLI use
    let view = view.clone();
    let slot = *slot;
    let rt = tokio::runtime::Handle::current();
    let v = std::thread::spawn(move || {
        rt.block_on(async move {
            use discovery_core::storage_backend::RawStorageAccess;
            view.read_slot(slot).await
        })
    })
    .join()
    .map_err(|_| anyhow!("read thread panicked"))?
    .map_err(|e| anyhow!("mirror read failed: {e}"))?;
    Ok(v)
}
