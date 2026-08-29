//! Canonical NDJSON codec for epoch and head files (spec §4.3, §4.4).
//!
//! Encoding is hand-built string emission so that byte identity is guaranteed
//! by construction: fixed field order, no whitespace, minimal lowercase hex,
//! `\n` after every line including the last. Decoding is serde-based and
//! order-insensitive; identity is always the raw bytes, never the parse.

use crate::{felt_from_hex, felt_hex, FeedError, Felt};
use serde_json::Value;
use std::fmt::Write as _;

pub const FORMAT_VERSION: u32 = 1;
pub const KIND_EPOCH: &str = "strk20-epoch";
pub const KIND_HEAD: &str = "strk20-head";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventLine {
    pub tx_index: u64,
    pub event_index: u64,
    pub tx_hash: Felt,
    pub keys: Vec<Felt>,
    pub data: Vec<Felt>,
}

/// Finality of a tail block (head file only).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Finality {
    L2,
    L1,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BlockLine {
    pub number: u64,
    pub hash: Felt,
    pub parent: Felt,
    pub timestamp: u64,
    /// Storage diffs sorted ascending by the 32-byte BE slot.
    pub diffs: Vec<(Felt, Felt)>,
    /// Events sorted ascending by event_index (emission order).
    pub events: Vec<EventLine>,
    /// Present only on blocks where the pool class changed.
    pub replaced_class: Option<Felt>,
    /// Present only in head files.
    pub finality: Option<Finality>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EpochHeader {
    pub chain_id: String,
    pub pool: Felt,
    pub epoch: u64,
    pub from: u64,
    pub to: u64,
    /// Content hash of the previous pool epoch's payload; None for the first.
    pub prev: Option<[u8; 32]>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadHeader {
    pub tail_from: u64,
    pub head: u64,
    pub head_hash: Felt,
    pub l1_accepted: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Footer {
    pub blocks: u64,
    pub diffs: u64,
    pub events: u64,
    pub class: Felt,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Epoch {
    pub header: EpochHeader,
    pub blocks: Vec<BlockLine>,
    pub footer: Footer,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Head {
    pub header: HeadHeader,
    pub blocks: Vec<BlockLine>,
    pub footer: Footer,
}

// ---------------------------------------------------------------- encoding

fn push_felt_array(out: &mut String, felts: &[Felt]) {
    out.push('[');
    for (i, f) in felts.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let _ = write!(out, "\"{}\"", felt_hex(f));
    }
    out.push(']');
}

fn encode_blk_line(out: &mut String, b: &BlockLine) {
    let _ = write!(
        out,
        "{{\"t\":\"blk\",\"b\":{},\"h\":\"{}\",\"p\":\"{}\",\"ts\":{},\"d\":[",
        b.number,
        felt_hex(&b.hash),
        felt_hex(&b.parent),
        b.timestamp
    );
    for (i, (slot, value)) in b.diffs.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let _ = write!(out, "[\"{}\",\"{}\"]", felt_hex(slot), felt_hex(value));
    }
    out.push_str("],\"e\":[");
    for (i, e) in b.events.iter().enumerate() {
        if i > 0 {
            out.push(',');
        }
        let _ = write!(
            out,
            "[{},{},\"{}\",",
            e.tx_index,
            e.event_index,
            felt_hex(&e.tx_hash)
        );
        push_felt_array(out, &e.keys);
        out.push(',');
        push_felt_array(out, &e.data);
        out.push(']');
    }
    out.push(']');
    if let Some(rc) = &b.replaced_class {
        let _ = write!(out, ",\"rc\":\"{}\"", felt_hex(rc));
    }
    if let Some(fin) = b.finality {
        let _ = write!(
            out,
            ",\"fin\":\"{}\"",
            match fin {
                Finality::L2 => "l2",
                Finality::L1 => "l1",
            }
        );
    }
    out.push_str("}\n");
}

fn encode_end_line(out: &mut String, f: &Footer) {
    let _ = write!(
        out,
        "{{\"t\":\"end\",\"blocks\":{},\"diffs\":{},\"events\":{},\"class\":\"{}\"}}\n",
        f.blocks,
        f.diffs,
        f.events,
        felt_hex(&f.class)
    );
}

/// Canonical epoch payload bytes. The caller is responsible for having sorted
/// diffs/events and ascending blocks; `encode_epoch` asserts it (a violated
/// invariant here would silently fork mirrors, so it is a hard panic).
pub fn encode_epoch(e: &Epoch) -> Vec<u8> {
    assert_invariants(&e.blocks);
    debug_assert!(e.blocks.iter().all(|b| b.finality.is_none()));
    let mut out = String::new();
    let prev = match &e.header.prev {
        Some(h) => format!("\"{}\"", hex::encode(h)),
        None => "null".to_owned(),
    };
    let _ = write!(
        out,
        "{{\"t\":\"hdr\",\"v\":{},\"kind\":\"{}\",\"chain_id\":\"{}\",\"pool\":\"{}\",\"epoch\":{},\"from\":{},\"to\":{},\"prev\":{}}}\n",
        FORMAT_VERSION,
        KIND_EPOCH,
        e.header.chain_id,
        felt_hex(&e.header.pool),
        e.header.epoch,
        e.header.from,
        e.header.to,
        prev
    );
    for b in &e.blocks {
        encode_blk_line(&mut out, b);
    }
    encode_end_line(&mut out, &e.footer);
    out.into_bytes()
}

/// Canonical head payload bytes.
pub fn encode_head(h: &Head) -> Vec<u8> {
    assert_invariants(&h.blocks);
    debug_assert!(h.blocks.iter().all(|b| b.finality.is_some()));
    let mut out = String::new();
    let _ = write!(
        out,
        "{{\"t\":\"hdr\",\"v\":{},\"kind\":\"{}\",\"tail_from\":{},\"head\":{},\"head_hash\":\"{}\",\"l1_accepted\":{}}}\n",
        FORMAT_VERSION,
        KIND_HEAD,
        h.header.tail_from,
        h.header.head,
        felt_hex(&h.header.head_hash),
        h.header.l1_accepted
    );
    for b in &h.blocks {
        encode_blk_line(&mut out, b);
    }
    encode_end_line(&mut out, &h.footer);
    out.into_bytes()
}

fn assert_invariants(blocks: &[BlockLine]) {
    for w in blocks.windows(2) {
        assert!(w[0].number < w[1].number, "blocks must ascend");
    }
    for b in blocks {
        for w in b.diffs.windows(2) {
            assert!(
                w[0].0.to_bytes_be() < w[1].0.to_bytes_be(),
                "diffs must be sorted by slot bytes, block {}",
                b.number
            );
        }
        for w in b.events.windows(2) {
            assert!(
                w[0].event_index < w[1].event_index,
                "events must ascend by event_index, block {}",
                b.number
            );
        }
    }
}

// ---------------------------------------------------------------- decoding

fn get_str<'a>(v: &'a Value, k: &str, line: usize) -> Result<&'a str, FeedError> {
    v.get(k)
        .and_then(Value::as_str)
        .ok_or_else(|| FeedError::Malformed(format!("line {line}: missing string field {k:?}")))
}

fn get_u64(v: &Value, k: &str, line: usize) -> Result<u64, FeedError> {
    v.get(k)
        .and_then(Value::as_u64)
        .ok_or_else(|| FeedError::Malformed(format!("line {line}: missing integer field {k:?}")))
}

fn get_felt(v: &Value, k: &str, line: usize) -> Result<Felt, FeedError> {
    felt_from_hex(get_str(v, k, line)?)
}

fn parse_blk(v: &Value, line: usize) -> Result<BlockLine, FeedError> {
    let diffs_v = v
        .get("d")
        .and_then(Value::as_array)
        .ok_or_else(|| FeedError::Malformed(format!("line {line}: missing d")))?;
    let mut diffs = Vec::with_capacity(diffs_v.len());
    for d in diffs_v {
        let pair = d
            .as_array()
            .filter(|a| a.len() == 2)
            .ok_or_else(|| FeedError::Malformed(format!("line {line}: bad diff pair")))?;
        let slot = felt_from_hex(pair[0].as_str().unwrap_or_default())?;
        let value = felt_from_hex(pair[1].as_str().unwrap_or_default())?;
        diffs.push((slot, value));
    }
    let events_v = v
        .get("e")
        .and_then(Value::as_array)
        .ok_or_else(|| FeedError::Malformed(format!("line {line}: missing e")))?;
    let mut events = Vec::with_capacity(events_v.len());
    for e in events_v {
        let tuple = e
            .as_array()
            .filter(|a| a.len() == 5)
            .ok_or_else(|| FeedError::Malformed(format!("line {line}: bad event tuple")))?;
        let felts = |val: &Value| -> Result<Vec<Felt>, FeedError> {
            val.as_array()
                .ok_or_else(|| FeedError::Malformed(format!("line {line}: bad felt array")))?
                .iter()
                .map(|x| felt_from_hex(x.as_str().unwrap_or_default()))
                .collect()
        };
        events.push(EventLine {
            tx_index: tuple[0]
                .as_u64()
                .ok_or_else(|| FeedError::Malformed(format!("line {line}: bad tx_index")))?,
            event_index: tuple[1]
                .as_u64()
                .ok_or_else(|| FeedError::Malformed(format!("line {line}: bad event_index")))?,
            tx_hash: felt_from_hex(tuple[2].as_str().unwrap_or_default())?,
            keys: felts(&tuple[3])?,
            data: felts(&tuple[4])?,
        });
    }
    let replaced_class = match v.get("rc") {
        Some(rc) => Some(felt_from_hex(rc.as_str().unwrap_or_default())?),
        None => None,
    };
    let finality = match v.get("fin").and_then(Value::as_str) {
        Some("l2") => Some(Finality::L2),
        Some("l1") => Some(Finality::L1),
        Some(other) => {
            return Err(FeedError::Malformed(format!(
                "line {line}: bad fin {other:?}"
            )))
        }
        None => None,
    };
    Ok(BlockLine {
        number: get_u64(v, "b", line)?,
        hash: get_felt(v, "h", line)?,
        parent: get_felt(v, "p", line)?,
        timestamp: get_u64(v, "ts", line)?,
        diffs,
        events,
        replaced_class,
        finality,
    })
}

fn parse_footer(v: &Value, line: usize) -> Result<Footer, FeedError> {
    Ok(Footer {
        blocks: get_u64(v, "blocks", line)?,
        diffs: get_u64(v, "diffs", line)?,
        events: get_u64(v, "events", line)?,
        class: get_felt(v, "class", line)?,
    })
}

struct Parsed {
    header: Value,
    blocks: Vec<BlockLine>,
    footer: Footer,
}

fn parse_lines(payload: &[u8], expected_kind: &str) -> Result<Parsed, FeedError> {
    let text = std::str::from_utf8(payload)
        .map_err(|_| FeedError::Malformed("payload is not utf-8".into()))?;
    if !text.ends_with('\n') {
        return Err(FeedError::Malformed(
            "payload must end with a newline".into(),
        ));
    }
    let mut header: Option<Value> = None;
    let mut footer: Option<Footer> = None;
    let mut blocks = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line_no = i + 1;
        let v: Value = serde_json::from_str(raw)?;
        let t = get_str(&v, "t", line_no)?;
        match t {
            "hdr" => {
                if line_no != 1 {
                    return Err(FeedError::Malformed(format!(
                        "hdr on line {line_no}, must be line 1"
                    )));
                }
                let kind = get_str(&v, "kind", line_no)?;
                if kind != expected_kind {
                    return Err(FeedError::Malformed(format!(
                        "kind {kind:?}, expected {expected_kind:?}"
                    )));
                }
                let version = get_u64(&v, "v", line_no)?;
                if version != FORMAT_VERSION as u64 {
                    return Err(FeedError::Malformed(format!(
                        "unsupported format version {version}"
                    )));
                }
                header = Some(v);
            }
            "blk" => {
                if footer.is_some() {
                    return Err(FeedError::Malformed("blk after end".into()));
                }
                blocks.push(parse_blk(&v, line_no)?);
            }
            "end" => {
                if footer.is_some() {
                    return Err(FeedError::Malformed("duplicate end".into()));
                }
                footer = Some(parse_footer(&v, line_no)?);
            }
            other => {
                return Err(FeedError::Malformed(format!(
                    "line {line_no}: unknown t {other:?}"
                )))
            }
        }
    }
    let header = header.ok_or_else(|| FeedError::Malformed("missing hdr".into()))?;
    let footer = footer.ok_or_else(|| FeedError::Malformed("missing end".into()))?;
    // structural checks
    if footer.blocks != blocks.len() as u64 {
        return Err(FeedError::Malformed(format!(
            "footer.blocks {} != actual {}",
            footer.blocks,
            blocks.len()
        )));
    }
    let n_diffs: u64 = blocks.iter().map(|b| b.diffs.len() as u64).sum();
    let n_events: u64 = blocks.iter().map(|b| b.events.len() as u64).sum();
    if footer.diffs != n_diffs || footer.events != n_events {
        return Err(FeedError::Malformed(format!(
            "footer counts diffs={} events={} != actual diffs={n_diffs} events={n_events}",
            footer.diffs, footer.events
        )));
    }
    for w in blocks.windows(2) {
        if w[0].number >= w[1].number {
            return Err(FeedError::Malformed(format!(
                "blocks not ascending at {}",
                w[1].number
            )));
        }
        // Parent linkage is checkable only for numerically adjacent blocks
        // (pool-active blocks are sparse).
        if w[1].number == w[0].number + 1 && w[1].parent != w[0].hash {
            return Err(FeedError::Malformed(format!(
                "parent linkage broken at block {}",
                w[1].number
            )));
        }
    }
    for b in &blocks {
        for w in b.diffs.windows(2) {
            if w[0].0.to_bytes_be() >= w[1].0.to_bytes_be() {
                return Err(FeedError::Malformed(format!(
                    "diffs not sorted in block {}",
                    b.number
                )));
            }
        }
        for w in b.events.windows(2) {
            if w[0].event_index >= w[1].event_index {
                return Err(FeedError::Malformed(format!(
                    "events not sorted in block {}",
                    b.number
                )));
            }
        }
    }
    Ok(Parsed {
        header,
        blocks,
        footer,
    })
}

/// Parse and structurally validate an epoch payload (uncompressed bytes).
pub fn parse_epoch(payload: &[u8]) -> Result<Epoch, FeedError> {
    let parsed = parse_lines(payload, KIND_EPOCH)?;
    let h = &parsed.header;
    let prev = match h.get("prev") {
        Some(Value::Null) | None => None,
        Some(Value::String(s)) => {
            let bytes = hex::decode(s)
                .map_err(|_| FeedError::Malformed(format!("bad prev hex {s:?}")))?;
            let arr: [u8; 32] = bytes
                .try_into()
                .map_err(|_| FeedError::Malformed("prev must be 32 bytes".into()))?;
            Some(arr)
        }
        _ => return Err(FeedError::Malformed("bad prev".into())),
    };
    let header = EpochHeader {
        chain_id: get_str(h, "chain_id", 1)?.to_owned(),
        pool: get_felt(h, "pool", 1)?,
        epoch: get_u64(h, "epoch", 1)?,
        from: get_u64(h, "from", 1)?,
        to: get_u64(h, "to", 1)?,
        prev,
    };
    for b in &parsed.blocks {
        if b.number < header.from || b.number > header.to {
            return Err(FeedError::Malformed(format!(
                "block {} outside epoch range [{}, {}]",
                b.number, header.from, header.to
            )));
        }
        if b.finality.is_some() {
            return Err(FeedError::Malformed(format!(
                "epoch block {} carries fin",
                b.number
            )));
        }
    }
    Ok(Epoch {
        header,
        blocks: parsed.blocks,
        footer: parsed.footer,
    })
}

/// Parse and structurally validate a head payload.
pub fn parse_head(payload: &[u8]) -> Result<Head, FeedError> {
    let parsed = parse_lines(payload, KIND_HEAD)?;
    let h = &parsed.header;
    let header = HeadHeader {
        tail_from: get_u64(h, "tail_from", 1)?,
        head: get_u64(h, "head", 1)?,
        head_hash: get_felt(h, "head_hash", 1)?,
        l1_accepted: get_u64(h, "l1_accepted", 1)?,
    };
    for b in &parsed.blocks {
        if b.finality.is_none() {
            return Err(FeedError::Malformed(format!(
                "head block {} missing fin",
                b.number
            )));
        }
        if b.number < header.tail_from {
            return Err(FeedError::Malformed(format!(
                "head block {} before tail_from {}",
                b.number, header.tail_from
            )));
        }
    }
    Ok(Head {
        header,
        blocks: parsed.blocks,
        footer: parsed.footer,
    })
}
