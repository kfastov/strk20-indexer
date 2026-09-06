//! One coherent accepted block, receipts and state diff from the sequencer.
//! This is input data, not a proof: normal mirror/root verification still applies.
use crate::rpc::{BlockHeader, RpcEvent, StateUpdate};
use anyhow::{ensure, Context, Result};
use serde_json::Value;
use starknet_types_core::felt::Felt;
use std::time::Duration;

pub struct BlockData {
    pub header: BlockHeader,
    pub update: StateUpdate,
    pub events: Vec<RpcEvent>,
}

pub async fn fetch(
    http: &reqwest::Client,
    base: &str,
    number: u64,
    pool: &Felt,
) -> Result<Option<BlockData>> {
    let request = async {
        let mut response = http
            .get(format!(
                "{}/get_state_update?blockNumber={number}&includeBlock=true",
                base.trim_end_matches('/')
            ))
            .timeout(Duration::from_secs(2))
            .send()
            .await?
            .error_for_status()?;
        let mut bytes = Vec::new();
        while let Some(chunk) = response.chunk().await? {
            ensure!(
                bytes.len() + chunk.len() <= 32 * 1024 * 1024,
                "feeder response exceeds 32 MiB"
            );
            bytes.extend_from_slice(&chunk);
        }
        Ok::<_, anyhow::Error>(bytes)
    }
    .await;
    match request {
        Ok(bytes) => decode(
            serde_json::from_slice(&bytes).context("decode feeder block")?,
            number,
            pool,
        )
        .map(Some),
        Err(error) => {
            // One fallback, no timer or availability polling. RPC remains usable
            // when the optional feeder is down; malformed data is never hidden.
            tracing::warn!(number, %error, "feeder unavailable; using RPC");
            Ok(None)
        }
    }
}

fn decode(mut data: Value, number: u64, pool: &Felt) -> Result<BlockData> {
    let block = data.get_mut("block").context("feeder block missing")?;
    ensure!(
        block["block_number"].as_u64() == Some(number),
        "feeder returned another block"
    );
    ensure!(
        matches!(
            block["status"].as_str(),
            Some("ACCEPTED_ON_L1" | "ACCEPTED_ON_L2")
        ),
        "feeder block is not accepted"
    );
    let transactions = block["transactions"]
        .as_array()
        .context("feeder transactions missing")?
        .iter()
        .map(|tx| {
            tx["transaction_hash"]
                .as_str()
                .map(str::to_owned)
                .context("feeder transaction hash missing")
        })
        .collect::<Result<Vec<_>>>()?;
    let receipts = block["transaction_receipts"]
        .as_array()
        .context("feeder receipts missing")?;
    ensure!(
        receipts.len() == transactions.len(),
        "feeder receipt count differs from transactions"
    );
    let hash = block["block_hash"]
        .as_str()
        .context("feeder block hash missing")?
        .to_owned();
    let mut events = Vec::new();
    for (index, (receipt, transaction)) in receipts.iter().zip(&transactions).enumerate() {
        ensure!(
            receipt["transaction_index"].as_u64() == Some(index as u64),
            "feeder receipt order mismatch"
        );
        ensure!(
            receipt["transaction_hash"]
                .as_str()
                .map(crate::rpc::parse_felt)
                .transpose()?
                == Some(crate::rpc::parse_felt(transaction)?),
            "feeder receipt transaction mismatch"
        );
        for event in receipt["events"]
            .as_array()
            .context("feeder receipt events missing")?
        {
            let address = event["from_address"]
                .as_str()
                .context("feeder event address missing")?;
            if crate::rpc::parse_felt(address)? != *pool {
                continue;
            }
            events.push(RpcEvent {
                from_address: address.into(),
                keys: serde_json::from_value(event["keys"].clone())?,
                data: serde_json::from_value(event["data"].clone())?,
                block_number: Some(number),
                block_hash: Some(hash.clone()),
                transaction_hash: transaction.clone(),
            });
        }
    }
    block["transactions"] = serde_json::to_value(transactions)?;
    block["parent_hash"] = block["parent_block_hash"].take();
    block["new_root"] = block["state_root"].take();
    let header: BlockHeader = serde_json::from_value(block.take())?;
    let update = data
        .get_mut("state_update")
        .context("feeder state update missing")?;
    // RPC uses an array; feeder uses an address-keyed map. Keep one ingest model.
    let storage = update["state_diff"]["storage_diffs"]
        .as_object()
        .context("feeder storage diffs missing")?
        .iter()
        .map(|(address, entries)| serde_json::json!({"address":address,"storage_entries":entries}))
        .collect();
    update["state_diff"]["storage_diffs"] = Value::Array(storage);
    ensure!(
        update["state_diff"]["deployed_contracts"].is_array(),
        "feeder deployments missing"
    );
    for replacement in update["state_diff"]["replaced_classes"]
        .as_array_mut()
        .context("feeder replacements missing")?
    {
        replacement["contract_address"] = replacement["address"].take();
    }
    let update: StateUpdate = serde_json::from_value(update.take())?;
    let felt = |v: Option<&str>| crate::rpc::parse_felt(v.context("feeder commitment missing")?);
    ensure!(
        felt(update.block_hash.as_deref())? == crate::rpc::parse_felt(&header.block_hash)?,
        "feeder state update block mismatch"
    );
    ensure!(
        felt(update.new_root.as_deref())? == felt(header.new_root.as_deref())?,
        "feeder state root mismatch"
    );
    Ok(BlockData {
        header,
        update,
        events,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn bundle() -> Value {
        serde_json::json!({"block": {
            "block_number":10,"block_hash":"0xa","parent_block_hash":"0x9","state_root":"0xb",
            "timestamp":10,"status":"ACCEPTED_ON_L2","transactions":[{"transaction_hash":"0xc"}],
            "transaction_receipts":[{"transaction_hash":"0xc","transaction_index":0,"events":[
                {"from_address":"0x1","keys":["0x2"],"data":["0x3"]},
                {"from_address":"0x4","keys":[],"data":[]}
            ]}]
        },"state_update":{"block_hash":"0xa","new_root":"0xb","state_diff":{
            "storage_diffs":{"0x1":[{"key":"0x5","value":"0x6"}]},
            "deployed_contracts":[],"replaced_classes":[{"address":"0x1","class_hash":"0x7"}]
        }}})
    }
    #[test]
    fn complete_bundle_preserves_pool_events_silent_storage_and_class_changes() {
        let result = decode(bundle(), 10, &Felt::ONE).unwrap();
        assert_eq!(result.events.len(), 1);
        assert_eq!(result.events[0].transaction_hash, "0xc");
        assert_eq!(
            result.update.state_diff.storage_diffs[0].storage_entries[0].value,
            "0x6"
        );
        assert_eq!(
            result.update.state_diff.replaced_classes[0].contract_address,
            "0x1"
        );
        assert_eq!(result.header.new_root.as_deref(), Some("0xb"));
        let mut silent = bundle();
        silent["block"]["transaction_receipts"][0]["events"] = serde_json::json!([]);
        assert_eq!(
            decode(silent, 10, &Felt::ONE)
                .unwrap()
                .update
                .state_diff
                .storage_diffs
                .len(),
            1
        );
    }
    #[test]
    fn incomplete_wrong_fork_and_misordered_bundles_are_rejected() {
        for (path, value) in [
            ("/block/block_number", serde_json::json!(11)),
            ("/block/status", serde_json::json!("PRE_CONFIRMED")),
            ("/block/transaction_receipts", serde_json::json!([])),
            (
                "/block/transaction_receipts/0/transaction_hash",
                serde_json::json!("0xd"),
            ),
            (
                "/block/transaction_receipts/0/transaction_index",
                serde_json::json!(1),
            ),
            ("/state_update/block_hash", serde_json::json!("0xd")),
            ("/state_update/new_root", serde_json::json!("0xd")),
            ("/state_update/state_diff/storage_diffs", Value::Null),
            ("/state_update/state_diff/replaced_classes", Value::Null),
        ] {
            let mut data = bundle();
            *data.pointer_mut(path).unwrap() = value;
            assert!(decode(data, 10, &Felt::ONE).is_err(), "accepted {path}");
        }
    }
}
