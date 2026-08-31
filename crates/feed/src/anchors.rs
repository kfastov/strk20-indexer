//! `anchors.ndjson` — the append-only chain-anchor log (spec §4.5).
//!
//! Per-epoch anchors are absent in production by construction: an epoch's end
//! block is thousands of blocks old by the time the epoch is cut, and the
//! storage-proof window is ~1024 blocks wide (docs/research/live/proof-window.md).
//! The log instead records anchors captured opportunistically while a block was
//! still provable, so a client can recompute the pool storage root from its own
//! folded mirror and compare.
//!
//! The file is NOT content-addressed — recomputation against the mirror is the
//! only thing standing behind it. Encoding follows the same canonical rules as
//! the epoch codec (fixed field order, no whitespace, minimal lowercase hex,
//! `\n` after every line), so the BYTES are a pure function of the anchor SET.
//! The set itself is operator-specific: captures are opportunistic, so two
//! honest operators with different cut timings, endpoints or backfill starts
//! legitimately publish different anchor sets. Comparing two mirrors' logs for
//! byte equality is therefore only meaningful when their anchor sets are known
//! to be the same.
//!
//! `parse_anchors` validates structure (utf-8, one record per line, trailing
//! newline, no blanks, strictly ascending blocks) but deliberately does NOT
//! reject non-canonical spellings — `0x00aa`, reordered fields or unknown extra
//! fields all parse. Canonicality is a WRITE-side property here; a reader
//! cannot detect a non-canonical publisher and must not rely on being able to.

use crate::{felt_from_hex, felt_hex, FeedError, Felt};
use serde_json::Value;
use std::fmt::Write as _;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnchorRecord {
    pub block: u64,
    pub block_hash: Felt,
    pub storage_root: Felt,
    pub class: Felt,
}

/// Canonical bytes for an ascending, deduplicated anchor set. Errors rather
/// than panics on an unordered set: this runs inside the indexer's cut path,
/// and a library encoder must not abort the daemon.
pub fn encode_anchors(records: &[AnchorRecord]) -> Result<Vec<u8>, FeedError> {
    for w in records.windows(2) {
        if w[0].block >= w[1].block {
            return Err(FeedError::Malformed(format!(
                "anchors must be strictly ascending in block: {} then {}",
                w[0].block, w[1].block
            )));
        }
    }
    let mut out = String::new();
    for r in records {
        let _ = writeln!(
            out,
            "{{\"block\":{},\"block_hash\":\"{}\",\"storage_root\":\"{}\",\"class\":\"{}\"}}",
            r.block,
            felt_hex(&r.block_hash),
            felt_hex(&r.storage_root),
            felt_hex(&r.class)
        );
    }
    Ok(out.into_bytes())
}

/// Upper bound on a served anchors log. The file is not content-addressed and
/// grows with every capture, so a client must bound what it is willing to
/// materialise from it (~220 bytes per record: 32 MiB is ~150 000 anchors,
/// far past any retention window an honest publisher keeps).
pub const MAX_ANCHORS_BYTES: usize = 32 * 1024 * 1024;

/// Parse and structurally validate an anchors log.
pub fn parse_anchors(payload: &[u8]) -> Result<Vec<AnchorRecord>, FeedError> {
    if payload.len() > MAX_ANCHORS_BYTES {
        return Err(FeedError::Malformed(format!(
            "anchors.ndjson is {} bytes, past the {MAX_ANCHORS_BYTES}-byte cap",
            payload.len()
        )));
    }
    let text = std::str::from_utf8(payload)
        .map_err(|_| FeedError::Malformed("anchors.ndjson is not utf-8".into()))?;
    if !text.is_empty() && !text.ends_with('\n') {
        return Err(FeedError::Malformed(
            "anchors.ndjson must end with a newline".into(),
        ));
    }
    let mut out: Vec<AnchorRecord> = Vec::new();
    for (i, raw) in text.lines().enumerate() {
        let line = i + 1;
        if raw.is_empty() {
            return Err(FeedError::Malformed(format!(
                "anchors.ndjson line {line} is blank"
            )));
        }
        let v: Value = serde_json::from_str(raw)?;
        let field = |k: &str| -> Result<Felt, FeedError> {
            felt_from_hex(v.get(k).and_then(Value::as_str).ok_or_else(|| {
                FeedError::Malformed(format!("anchors.ndjson line {line}: missing {k:?}"))
            })?)
        };
        let record = AnchorRecord {
            block: v.get("block").and_then(Value::as_u64).ok_or_else(|| {
                FeedError::Malformed(format!("anchors.ndjson line {line}: missing \"block\""))
            })?,
            block_hash: field("block_hash")?,
            storage_root: field("storage_root")?,
            class: field("class")?,
        };
        if let Some(prev) = out.last() {
            if record.block <= prev.block {
                return Err(FeedError::Malformed(format!(
                    "anchors.ndjson line {line}: block {} does not follow {}",
                    record.block, prev.block
                )));
            }
        }
        out.push(record);
    }
    Ok(out)
}
