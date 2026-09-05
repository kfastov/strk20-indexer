//! Spendable SDK data derived locally from one verified state, never by RPC
//! queries containing the user's address or viewing key.
use crate::{
    store::{ApplyOutcome, ConsumerStore},
    sync::{discover_state, full_resync},
};
use anyhow::{anyhow, ensure, Result};
use discovery_core::{
    discovery::DiscoveryCursor,
    privacy_pool::{
        decryption::{decrypt_packed_value, decrypt_subchannel_token},
        hashes::*,
        storage_slots,
        types::{EncSubchannelInfo, SecretFelt},
    },
};
use serde_json::{json, Value};
use starknet_types_core::felt::Felt;
use strk20_feed::felt_hex;

fn number<S: ConsumerStore>(store: &S, name: &str) -> Result<u64> {
    Ok(store
        .meta_get(name)?
        .and_then(|s| s.parse().ok())
        .unwrap_or(0))
}

pub fn checkpoint<S: ConsumerStore>(
    store: &S,
) -> Result<strk20_feed::checkpoint::TrustedCheckpoint> {
    ensure!(
        store.meta_get("verification_failed")?.as_deref() != Some("1"),
        "CHECKPOINT_FAILED: state verification failed"
    );
    serde_json::from_str(
        &store
            .meta_get("verified_checkpoint")?
            .ok_or_else(|| anyhow!("CHECKPOINT_REQUIRED: verify state before discovery"))?,
    )
    .map_err(Into::into)
}

fn cursor<S: ConsumerStore>(store: &S, kind: &str, owner: Felt) -> Result<DiscoveryCursor> {
    Ok(serde_json::from_str(
        &store
            .meta_get(&format!("cur_{kind}_{}", felt_hex(&owner)))?
            .unwrap_or_else(|| "{}".into()),
    )?)
}

pub async fn discover<S: ConsumerStore>(
    store: &S,
    owner: Felt,
    key: &SecretFelt,
) -> Result<String> {
    let cp = checkpoint(store)?;
    let stamp = format!(
        "{}:{}:{}",
        cp.block_hash,
        store.tail_generation()?,
        hex::encode(strk20_feed::payload_sha256(&key.to_bytes_be()))
    );
    let cache_key = format!("sdk_{}", felt_hex(&owner));
    if store.meta_get(&format!("{cache_key}_stamp"))?.as_deref() == Some(&stamp) {
        if let Some(report) = store.meta_get(&cache_key)? {
            return Ok(report);
        }
    }
    let last = number(store, &format!("{cache_key}_block"))?;
    if cp.block_number < last {
        full_resync(store, &owner)?;
    }
    let outcome = ApplyOutcome {
        head: cp.block_number,
        last_epoch_to: number(store, "last_epoch_to")?.min(cp.block_number),
        l1_accepted: number(store, "l1_accepted")?.min(cp.block_number),
        snapshot_basis: store
            .meta_get("snapshot_basis")?
            .and_then(|s| s.parse().ok()),
        history_floor: number(store, "history_floor")?,
        ..Default::default()
    };
    let report = discover_state(store, owner, key, outcome, "rpc-verified").await?;
    let incoming = cursor(store, "in", owner)?;
    let mut notes = Vec::new();
    for n in store.notes(&owner)? {
        let channel = incoming
            .channels
            .get(&n.sender)
            .ok_or_else(|| anyhow!("missing note channel"))?;
        let (packed, _) =
            store.read_slot_as_of(&storage_slots::notes(n.note_id), cp.block_number)?;
        let (amount, salt) = decrypt_packed_value(packed, &channel.channel_key, n.token, n.index);
        let known_key = format!("known_{}", felt_hex(&n.note_id));
        let value_key = format!("{known_key}_value");
        let old = number(store, &known_key)?;
        let same = store.meta_get(&value_key)?.as_deref() == Some(&felt_hex(&packed));
        let known_by = if same && old > 0 && old <= cp.block_number && !report.tail_rewound {
            old
        } else {
            cp.block_number
        };
        store.meta_set(&known_key, &known_by.to_string())?;
        store.meta_set(&value_key, &felt_hex(&packed))?;
        notes.push(json!({"token":felt_hex(&n.token),"id":felt_hex(&n.note_id),"amount":amount.to_string(),
            "sender":felt_hex(&n.sender),"spent":n.spent,"knownByBlock":known_by,
            "reportedWriteBlock":n.block,"nullifier":felt_hex(&n.nullifier),
            "witness":{"channelKey":felt_hex(&channel.channel_key),"index":n.index,"salt":salt.to_string()}}));
    }
    let result = json!({"block":cp.block_number,"blockHash":felt_hex(&cp.block_hash),"notes":notes,
        "incomingCursor":incoming,"report":{"incoming_complete":report.incoming_complete,"outgoing_complete":report.outgoing_complete,"history_from":report.history_from}})
    .to_string();
    store.meta_set(&cache_key, &result)?;
    store.meta_set(&format!("{cache_key}_stamp"), &stamp)?;
    store.meta_set(&format!("{cache_key}_block"), &cp.block_number.to_string())?;
    Ok(result)
}

pub fn channels<S: ConsumerStore>(
    store: &S,
    owner: Felt,
    key: &SecretFelt,
    recipients: Option<Vec<Felt>>,
) -> Result<Value> {
    let block = checkpoint(store)?.block_number;
    let outgoing = cursor(store, "out", owner)?;
    let total = outgoing
        .total_n_channels
        .unwrap_or(outgoing.channels.len() as u64);
    let mut recipients = recipients.unwrap_or_else(|| outgoing.channels.keys().copied().collect());
    recipients.sort();
    recipients.dedup();
    let mut channels = Vec::new();
    for recipient in recipients {
        let public = store
            .read_slot_as_of(&storage_slots::public_key(recipient), block)?
            .0;
        let channel_key = compute_channel_key(owner, key, recipient, public);
        let marker = compute_channel_marker(&channel_key, owner, recipient, public);
        let exists = store
            .read_slot_as_of(&storage_slots::channel_exists(marker), block)?
            .0
            != Felt::ZERO;
        let mut tokens = Vec::new();
        if let Some(channel) = outgoing.channels.get(&recipient) {
            for index in 0..channel.last_subchannel_index.map_or(0, |n| n + 1) {
                let slots =
                    storage_slots::subchannel_tokens(compute_subchannel_id(&channel_key, index));
                let salt = store.read_slot_as_of(&slots.salt, block)?.0;
                let encrypted = store.read_slot_as_of(&slots.enc_token, block)?.0;
                let token = decrypt_subchannel_token(
                    &EncSubchannelInfo {
                        salt,
                        enc_token: encrypted,
                    },
                    &channel_key,
                    index,
                );
                let sub = channel
                    .subchannels
                    .get(&token)
                    .ok_or_else(|| anyhow!("subchannel cursor mismatch"))?;
                tokens.push(json!({"token":felt_hex(&token),"tokenIndex":index,"noteNonce":sub.total_n_notes.unwrap_or(0)}));
            }
        }
        channels.push(
            json!({"recipient":felt_hex(&recipient),"publicKey":felt_hex(&public),
            "key":exists.then(||felt_hex(&channel_key)),"tokens":tokens}),
        );
    }
    Ok(json!({"block":block,"total":total,"channels":channels}))
}

pub fn requirement<S: ConsumerStore>(
    store: &S,
    owner: Felt,
    key: &SecretFelt,
    recipient: Felt,
    token: Felt,
) -> Result<u8> {
    let block = checkpoint(store)?.block_number;
    let read = |slot| -> Result<Felt> { Ok(store.read_slot_as_of(&slot, block)?.0) };
    if read(storage_slots::public_key(owner))? == Felt::ZERO {
        return Ok(0);
    }
    let public = read(storage_slots::public_key(recipient))?;
    let channel = compute_channel_key(owner, key, recipient, public);
    if read(storage_slots::channel_exists(compute_channel_marker(
        &channel, owner, recipient, public,
    )))? == Felt::ZERO
    {
        return Ok(1);
    }
    if read(storage_slots::subchannel_exists(compute_subchannel_marker(
        &channel, recipient, public, token,
    )))? == Felt::ZERO
    {
        return Ok(2);
    }
    Ok(3)
}
