//! Test-side reader/writer for the snapshot wire format (consumer-path.md
//! §1.2), written INDEPENDENTLY of the product encoder on purpose: the byte
//! canonicality leg compares the served payload against bytes this module
//! produces, so a shared implementation would make that comparison vacuous.
//!
//! `parse` is deliberately tolerant (it accepts any JSON object order and any
//! felt spelling) so that a non-canonical publisher produces an informative
//! byte diff rather than a parse error; `encode` is strict canonical §1.2.

use serde_json::Value;
use starknet_types_core::felt::Felt;
use strk20_feed::felt_hex;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapHeader {
    pub v: u64,
    pub kind: String,
    pub chain_id: String,
    pub pool: Felt,
    pub epoch: u64,
    pub block: u64,
    pub epoch_hash: String,
    pub storage_root: Felt,
    pub class: Felt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapSlot {
    pub k: Felt,
    pub v: Felt,
    pub w: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapDoc {
    pub header: SnapHeader,
    pub slots: Vec<SnapSlot>,
    pub footer_slots: u64,
}

fn felt_of(v: &Value, key: &str, ctx: &str) -> Result<Felt, String> {
    let s = v
        .get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{ctx}: missing string field {key:?}"))?;
    Felt::from_hex(s).map_err(|_| format!("{ctx}: field {key:?} is not a felt: {s}"))
}

fn u64_of(v: &Value, key: &str, ctx: &str) -> Result<u64, String> {
    v.get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| format!("{ctx}: missing integer field {key:?}"))
}

fn str_of(v: &Value, key: &str, ctx: &str) -> Result<String, String> {
    Ok(v.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("{ctx}: missing string field {key:?}"))?
        .to_owned())
}

/// Parse an uncompressed snapshot payload.
pub fn parse(payload: &[u8]) -> Result<SnapDoc, String> {
    let text = std::str::from_utf8(payload).map_err(|_| "snapshot payload is not utf-8".to_owned())?;
    if !text.ends_with('\n') {
        return Err("snapshot payload must end with a newline".into());
    }
    let mut lines = text.lines().enumerate();
    let (_, first) = lines.next().ok_or_else(|| "snapshot payload is empty".to_owned())?;
    let hv: Value =
        serde_json::from_str(first).map_err(|e| format!("header line is not JSON ({e}): {first}"))?;
    if hv.get("t").and_then(Value::as_str) != Some("hdr") {
        return Err(format!("first line must be the \"hdr\" line: {first}"));
    }
    let header = SnapHeader {
        v: u64_of(&hv, "v", "header")?,
        kind: str_of(&hv, "kind", "header")?,
        chain_id: str_of(&hv, "chain_id", "header")?,
        pool: felt_of(&hv, "pool", "header")?,
        epoch: u64_of(&hv, "epoch", "header")?,
        block: u64_of(&hv, "block", "header")?,
        epoch_hash: str_of(&hv, "epoch_hash", "header")?,
        storage_root: felt_of(&hv, "storage_root", "header")?,
        class: felt_of(&hv, "class", "header")?,
    };

    let mut slots = Vec::new();
    let mut footer_slots = None;
    for (i, raw) in lines {
        let n = i + 1;
        let v: Value = serde_json::from_str(raw)
            .map_err(|e| format!("line {n} is not JSON ({e}): {raw}"))?;
        match v.get("t").and_then(Value::as_str) {
            Some("s") => {
                if footer_slots.is_some() {
                    return Err(format!("line {n}: slot line after the \"end\" line"));
                }
                slots.push(SnapSlot {
                    k: felt_of(&v, "k", &format!("line {n}"))?,
                    v: felt_of(&v, "v", &format!("line {n}"))?,
                    w: u64_of(&v, "w", &format!("line {n}"))?,
                });
            }
            Some("end") => {
                footer_slots = Some(u64_of(&v, "slots", &format!("line {n}"))?);
            }
            other => {
                return Err(format!("line {n}: unknown record type {other:?}: {raw}"));
            }
        }
    }
    Ok(SnapDoc {
        header,
        slots,
        footer_slots: footer_slots.ok_or_else(|| "snapshot payload has no \"end\" line".to_owned())?,
    })
}

pub fn header_line(h: &SnapHeader) -> String {
    format!(
        "{{\"t\":\"hdr\",\"v\":{},\"kind\":\"{}\",\"chain_id\":\"{}\",\"pool\":\"{}\",\"epoch\":{},\"block\":{},\"epoch_hash\":\"{}\",\"storage_root\":\"{}\",\"class\":\"{}\"}}",
        h.v,
        h.kind,
        h.chain_id,
        felt_hex(&h.pool),
        h.epoch,
        h.block,
        h.epoch_hash,
        felt_hex(&h.storage_root),
        felt_hex(&h.class)
    )
}

pub fn slot_line(s: &SnapSlot) -> String {
    format!(
        "{{\"t\":\"s\",\"k\":\"{}\",\"v\":\"{}\",\"w\":{}}}",
        felt_hex(&s.k),
        felt_hex(&s.v),
        s.w
    )
}

pub fn footer_line(slots: u64) -> String {
    format!("{{\"t\":\"end\",\"slots\":{slots}}}")
}

/// Canonical §1.2 bytes: fixed field order, no whitespace, minimal lowercase
/// hex, slot lines ascending by the 32-byte BE key, `\n` after every line.
pub fn encode(doc: &SnapDoc) -> Vec<u8> {
    let mut out = String::new();
    out.push_str(&header_line(&doc.header));
    out.push('\n');
    let mut slots = doc.slots.clone();
    slots.sort_by_key(|s| s.k.to_bytes_be());
    for s in &slots {
        out.push_str(&slot_line(s));
        out.push('\n');
    }
    out.push_str(&footer_line(doc.footer_slots));
    out.push('\n');
    out.into_bytes()
}
