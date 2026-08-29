//! Honest explorer metrics (spec §6.2, Q18 policy). Only aggregates that a
//! public observer can verifiably derive; nothing that aids deanonymization.
//! Typed decoding freezes at the degraded boundary (spec §5.7).

use crate::db::Db;
use anyhow::Result;
use serde_json::{json, Value};
use starknet_types_core::felt::Felt;
use std::collections::BTreeMap;

/// Verified live selectors (docs/research/data/selector_map.json).
pub mod selectors {
    pub const DEPOSIT: &str = "0x9149d2123147c5f43d258257fef0b7b969db78269369ebcf5ebb9eef8592f2";
    pub const WITHDRAWAL: &str = "0x2eed7e29b3502a726faf503ac4316b7101f3da813654e8df02c13449e03da8";
    pub const ENC_NOTE_CREATED: &str =
        "0x23c20207be8b1ef4430c25eef8ce779c9745ebe04139555ae81bd4f8fdd6ec5";
    pub const NOTE_USED: &str =
        "0x247fc60d782e0094e7f98c47f277d92a3345d07a436f1f56b27a9b62be2322e";
    pub const OPEN_NOTE_CREATED: &str =
        "0x22330482fd296a27cf9096807b4a3622cd619d31cce42c1e55655914e8459ee";
    pub const OPEN_NOTE_DEPOSITED: &str =
        "0x25b6da03c4858d11cb0708d5cb6be79b190fb32eb7a7ce83804e07cbbb9bead";
    pub const VIEWING_KEY_SET: &str =
        "0x1321a492485b4f19851fb787ab3800a0030b595332cba93cd5fe40dfb5a4daf";
    pub const EXTERNAL_CONTRACT_INVOKED: &str =
        "0xa8fb36d0894f5e87797c38533a55c4486a1f35e9e9eced10f995b9639a8955";
}

fn sel(s: &str) -> Felt {
    Felt::from_hex(s).unwrap()
}

pub fn compute(db: &Db) -> Result<Value> {
    let head: u64 = db
        .meta_get("head_number")?
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    // Freeze typed decoding at the degraded boundary.
    let upto = match (
        db.meta_get("decode_state")?.as_deref(),
        db.meta_get("degraded_since_block")?
            .and_then(|s| s.parse::<u64>().ok()),
    ) {
        (Some("degraded"), Some(b)) => b.saturating_sub(1),
        _ => head,
    };

    let all = db.events_filtered(0, upto, &[])?;
    let mut deposits: BTreeMap<String, (u64, u128)> = BTreeMap::new();
    let mut withdrawals: BTreeMap<String, (u64, u128)> = BTreeMap::new();
    let mut open_note_deposits: BTreeMap<String, (u64, u128)> = BTreeMap::new();
    let mut external_calls: BTreeMap<String, u64> = BTreeMap::new();
    let (mut note_count, mut spend_count, mut registrations) = (0u64, 0u64, 0u64);

    let s_deposit = sel(selectors::DEPOSIT);
    let s_withdrawal = sel(selectors::WITHDRAWAL);
    let s_enc = sel(selectors::ENC_NOTE_CREATED);
    let s_used = sel(selectors::NOTE_USED);
    let s_open = sel(selectors::OPEN_NOTE_CREATED);
    let s_open_dep = sel(selectors::OPEN_NOTE_DEPOSITED);
    let s_vks = sel(selectors::VIEWING_KEY_SET);
    let s_ext = sel(selectors::EXTERNAL_CONTRACT_INVOKED);

    let amount_of = |f: Option<&Felt>| -> u128 {
        f.and_then(|v| u128::try_from(v.to_biguint()).ok()).unwrap_or(0)
    };

    for e in &all {
        let Some(k0) = e.keys.first() else { continue };
        if *k0 == s_deposit {
            // Deposit { #[key] user_addr, #[key] token, amount }
            let token = e.keys.get(2).map(strk20_feed::felt_hex).unwrap_or_default();
            let entry = deposits.entry(token).or_default();
            entry.0 += 1;
            entry.1 += amount_of(e.data.first());
        } else if *k0 == s_withdrawal {
            // Withdrawal { enc_user_addr(3 data), #[key] to_addr, #[key] token, amount@data[3] }
            let token = e.keys.get(2).map(strk20_feed::felt_hex).unwrap_or_default();
            let entry = withdrawals.entry(token).or_default();
            entry.0 += 1;
            entry.1 += amount_of(e.data.get(3));
        } else if *k0 == s_enc || *k0 == s_open {
            note_count += 1;
        } else if *k0 == s_used {
            spend_count += 1;
        } else if *k0 == s_open_dep {
            // OpenNoteDeposited { #[key] depositor, #[key] token, #[key] note_id, amount }
            let token = e.keys.get(2).map(strk20_feed::felt_hex).unwrap_or_default();
            let entry = open_note_deposits.entry(token).or_default();
            entry.0 += 1;
            entry.1 += amount_of(e.data.first());
        } else if *k0 == s_vks {
            registrations += 1;
        } else if *k0 == s_ext {
            // ExternalContractInvoked { #[key] contract_address, #[key] selector }
            let target = e.keys.get(1).map(strk20_feed::felt_hex).unwrap_or_default();
            *external_calls.entry(target).or_default() += 1;
        }
    }

    let map_json = |m: &BTreeMap<String, (u64, u128)>| -> Value {
        Value::Object(
            m.iter()
                .map(|(token, (count, amount))| {
                    (
                        token.clone(),
                        json!({ "count": count, "amount": amount.to_string() }),
                    )
                })
                .collect(),
        )
    };

    // TVL per token = deposits + open-note funding - withdrawals (saturating;
    // an explorer must never show negative TVL from partial history).
    let mut tvl: BTreeMap<String, u128> = BTreeMap::new();
    for (token, (_, amt)) in deposits.iter().chain(open_note_deposits.iter()) {
        *tvl.entry(token.clone()).or_default() += amt;
    }
    for (token, (_, amt)) in &withdrawals {
        let entry = tvl.entry(token.clone()).or_default();
        *entry = entry.saturating_sub(*amt);
    }

    let upgrades: Vec<Value> = {
        let mut out = Vec::new();
        let mut stmt = db
            .conn
            .prepare("SELECT block, class_hash FROM class_history ORDER BY block")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, i64>(0)? as u64, r.get::<_, Vec<u8>>(1)?))
        })?;
        for row in rows {
            let (block, class) = row?;
            out.push(json!({
                "block": block,
                "class": strk20_feed::felt_hex(&crate::db::blob_felt(&class)),
            }));
        }
        out
    };

    Ok(json!({
        "deposits": map_json(&deposits),
        "withdrawals": map_json(&withdrawals),
        "open_note_deposits": map_json(&open_note_deposits),
        "tvl": Value::Object(
            tvl.iter().map(|(t, v)| (t.clone(), json!(v.to_string()))).collect()
        ),
        "note_count": note_count,
        "spend_count": spend_count,
        "registrations": registrations,
        "external_calls": external_calls,
        "upgrades": upgrades,
        "stats_frozen_at": if upto == head { Value::Null } else { json!(upto) },
    }))
}
