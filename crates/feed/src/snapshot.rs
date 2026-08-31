//! Snapshot wire format v1 (consumer-path.md §1.2, frozen).
//!
//! A snapshot is the folded SLOT STATE of the pool at one epoch boundary: every
//! slot with a nonzero value as of that block, carrying its value and its last
//! write block. It carries no events and no block index — a snapshot-started
//! client therefore has discovery, balances, spent-state and per-note block
//! metadata, and no transaction history below the basis.
//!
//! Content identity is sha256 over the UNCOMPRESSED payload, exactly as for
//! epochs (§4.3); the `.zst` hash is a transport checksum only. Encoding is
//! hand-built string emission so byte identity holds by construction: fixed
//! field order, no whitespace, minimal lowercase hex, slot lines ascending by
//! the 32-byte BE key, `\n` after every line including the last.
//!
//! `parse` is strict where a client's verification ladder (§1.5 ring 3) needs
//! it to be — ordering, the footer count, `w <= header.block` — but is
//! order-insensitive about JSON fields, because identity is always the raw
//! bytes and never the parse.

use crate::{felt_from_hex, felt_hex, FeedError, Felt};
use serde_json::Value;
use std::fmt::Write as _;

pub const SNAPSHOT_VERSION: u64 = 1;
pub const KIND_SNAPSHOT: &str = "strk20-snapshot";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotHeader {
    pub v: u64,
    pub kind: String,
    pub chain_id: String,
    pub pool: Felt,
    pub epoch: u64,
    pub block: u64,
    /// 64-hex content hash of the basis epoch's payload — the pin that keeps a
    /// snapshot-started client on the ONE hash chain.
    pub epoch_hash: String,
    pub storage_root: Felt,
    /// Pool class as of `block`. INFORMATIONAL under §11: ring 5 used to pin it
    /// to the anchor sidecar's `contract_leaves_data[0].class_hash`, and §11.1
    /// deleted the sidecar. No value a snapshot-started client can obtain is
    /// comparable to it — the basis epoch (whose footer carries the class) is
    /// exactly what such a client never fetches, and the anchors log records
    /// the class at a HEAD block, which may legitimately differ after an
    /// upgrade. The field is inside the content hash and useful to an auditor;
    /// spec leg m(vi) is not implementable without the sidecar (see the
    /// implementation-notes delta log).
    pub class: Felt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SnapSlot {
    pub k: Felt,
    pub v: Felt,
    /// Last write block, `<= header.block`. Per-note `block_number` and the
    /// maturity rule are derived from it, so it is part of the format.
    pub w: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Snapshot {
    pub header: SnapshotHeader,
    pub slots: Vec<SnapSlot>,
}

/// Feed-relative path of the snapshot file for epoch `e`.
pub fn snapshot_file_name(e: u64) -> String {
    format!("snapshots/{e:08}.strk20s.zst")
}

pub fn encode(s: &Snapshot) -> Vec<u8> {
    let h = &s.header;
    let mut out = String::new();
    let _ = writeln!(
        out,
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
    );
    for slot in &s.slots {
        let _ = writeln!(
            out,
            "{{\"t\":\"s\",\"k\":\"{}\",\"v\":\"{}\",\"w\":{}}}",
            felt_hex(&slot.k),
            felt_hex(&slot.v),
            slot.w
        );
    }
    let _ = writeln!(out, "{{\"t\":\"end\",\"slots\":{}}}", s.slots.len());
    out.into_bytes()
}

fn str_field(v: &Value, key: &str, ctx: &str) -> Result<String, FeedError> {
    Ok(v.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| FeedError::Malformed(format!("{ctx}: missing string field {key:?}")))?
        .to_owned())
}

fn u64_field(v: &Value, key: &str, ctx: &str) -> Result<u64, FeedError> {
    v.get(key)
        .and_then(Value::as_u64)
        .ok_or_else(|| FeedError::Malformed(format!("{ctx}: missing integer field {key:?}")))
}

fn felt_field(v: &Value, key: &str, ctx: &str) -> Result<Felt, FeedError> {
    felt_from_hex(&str_field(v, key, ctx)?)
}

pub fn parse(payload: &[u8]) -> Result<Snapshot, FeedError> {
    let text = std::str::from_utf8(payload)
        .map_err(|_| FeedError::Malformed("snapshot payload is not utf-8".into()))?;
    if !text.ends_with('\n') {
        return Err(FeedError::Malformed(
            "snapshot payload must end with a newline".into(),
        ));
    }
    let mut lines = text.lines();
    let first = lines
        .next()
        .ok_or_else(|| FeedError::Malformed("snapshot payload is empty".into()))?;
    let hv: Value = serde_json::from_str(first)?;
    if hv.get("t").and_then(Value::as_str) != Some("hdr") {
        return Err(FeedError::Malformed(
            "snapshot line 1 must be the \"hdr\" record".into(),
        ));
    }
    let header = SnapshotHeader {
        v: u64_field(&hv, "v", "snapshot header")?,
        kind: str_field(&hv, "kind", "snapshot header")?,
        chain_id: str_field(&hv, "chain_id", "snapshot header")?,
        pool: felt_field(&hv, "pool", "snapshot header")?,
        epoch: u64_field(&hv, "epoch", "snapshot header")?,
        block: u64_field(&hv, "block", "snapshot header")?,
        epoch_hash: str_field(&hv, "epoch_hash", "snapshot header")?,
        storage_root: felt_field(&hv, "storage_root", "snapshot header")?,
        class: felt_field(&hv, "class", "snapshot header")?,
    };
    if header.v != SNAPSHOT_VERSION || header.kind != KIND_SNAPSHOT {
        return Err(FeedError::Malformed(format!(
            "snapshot is {:?} v{}, expected {KIND_SNAPSHOT} v{SNAPSHOT_VERSION}",
            header.kind, header.v
        )));
    }

    let mut slots: Vec<SnapSlot> = Vec::new();
    let mut footer: Option<u64> = None;
    for (i, raw) in lines.enumerate() {
        let n = i + 2;
        let v: Value = serde_json::from_str(raw)?;
        match v.get("t").and_then(Value::as_str) {
            Some("s") => {
                if footer.is_some() {
                    return Err(FeedError::Malformed(format!(
                        "snapshot line {n}: slot record after the \"end\" record"
                    )));
                }
                let ctx = format!("snapshot line {n}");
                let slot = SnapSlot {
                    k: felt_field(&v, "k", &ctx)?,
                    v: felt_field(&v, "v", &ctx)?,
                    w: u64_field(&v, "w", &ctx)?,
                };
                if slot.v == Felt::ZERO {
                    return Err(FeedError::Malformed(format!(
                        "snapshot line {n}: zero-valued slots are never emitted (Cairo map semantics)"
                    )));
                }
                if slot.w > header.block {
                    return Err(FeedError::Malformed(format!(
                        "snapshot line {n}: write block {} is above the basis {}",
                        slot.w, header.block
                    )));
                }
                if let Some(prev) = slots.last() {
                    if prev.k.to_bytes_be() >= slot.k.to_bytes_be() {
                        return Err(FeedError::Malformed(format!(
                            "snapshot line {n}: slot {} does not follow {}",
                            felt_hex(&slot.k),
                            felt_hex(&prev.k)
                        )));
                    }
                }
                slots.push(slot);
            }
            Some("end") => {
                footer = Some(u64_field(&v, "slots", &format!("snapshot line {n}"))?);
            }
            other => {
                return Err(FeedError::Malformed(format!(
                    "snapshot line {n}: unknown record type {other:?}"
                )));
            }
        }
    }
    let declared = footer
        .ok_or_else(|| FeedError::Malformed("snapshot payload has no \"end\" record".into()))?;
    if declared != slots.len() as u64 {
        return Err(FeedError::Malformed(format!(
            "snapshot footer declares {declared} slots but {} were emitted",
            slots.len()
        )));
    }
    Ok(Snapshot { header, slots })
}

/// The slot set as `(key, value)` pairs — the MPT input.
pub fn slot_pairs(s: &Snapshot) -> Vec<(Felt, Felt)> {
    s.slots.iter().map(|x| (x.k, x.v)).collect()
}

/// Recompute the storage root over the slot lines (§1.5 ring 5).
#[cfg(feature = "mpt")]
pub fn storage_root_of(s: &Snapshot) -> Felt {
    crate::mpt::storage_root(&slot_pairs(s))
}

/// Feed identity a snapshot must be bound to before a single slot is applied.
#[derive(Debug, Clone)]
pub struct FeedIdentity {
    pub chain_id: String,
    pub pool: Felt,
}

/// Ring 1 of the §1.5 ladder on its own: the transport checksum, expressed
/// over an already-computed digest so it can be run at the one moment that
/// makes it worth anything — BEFORE the bytes reach a decompressor (R-I). A
/// checksum that only runs after a decompressor has eaten the bytes protects
/// nothing, and the host that owns the decompressor is not always the crate
/// that owns the ladder (Block B inflates through
/// `FeedTransport::decompress`, because linking zstd would put `zstd-sys` —
/// a C build with no `wasm32-unknown-unknown` backend — in its graph).
///
/// `zst_sha256` is the lowercase hex sha256 of the COMPRESSED bytes.
pub fn check_zst_hash(
    zst_sha256: &str,
    entry: &crate::manifest::ManifestSnapshot,
) -> Result<(), FeedError> {
    if zst_sha256 != entry.zst {
        return Err(FeedError::Malformed(format!(
            "FEED_HASH_MISMATCH: snapshot {} has sha256 {zst_sha256}, manifest says {}",
            entry.file, entry.zst
        )));
    }
    Ok(())
}

/// Rings 1–5 of the §1.5 ladder over an ALREADY-DECOMPRESSED payload, minus
/// ring 6 (which needs an RPC) and minus reachability (§11.3, which needs the
/// folded mirror).
///
/// This is the single implementation of the snapshot ladder. It is split out
/// from [`verify_snapshot`] on the decompression boundary and nowhere else,
/// because that boundary is the only thing a host can legitimately need to
/// vary: the feed crate inflates with `zstd`, Block B hands the job to its
/// host so it stays wasm-clean. Everything security-bearing — every ring,
/// every ordering, every failure string — lives here once. A second copy of
/// these checks anywhere in the workspace is a bug: the copy the tests pin
/// stops being the copy a client executes.
///
/// `zst_sha256` (the digest of the compressed bytes) is re-checked here so a
/// caller that inflated first cannot skip ring 1 altogether; callers that can
/// run it earlier must, via [`check_zst_hash`].
#[cfg(feature = "mpt")]
pub fn verify_snapshot_payload(
    payload: &[u8],
    zst_sha256: &str,
    entry: &crate::manifest::ManifestSnapshot,
    basis_epoch_hash: &str,
    expect: &FeedIdentity,
) -> Result<Snapshot, FeedError> {
    // ring 1 — transport. Already run before decompression by any caller that
    // owns the decompressor; re-asserted so it can never be skipped entirely.
    check_zst_hash(zst_sha256, entry)?;

    // ring 2 — content identity
    let content_hash = hex::encode(crate::payload_sha256(payload));
    if content_hash != entry.hash {
        return Err(FeedError::Malformed(format!(
            "FEED_HASH_MISMATCH: snapshot {} payload has sha256 {content_hash}, manifest says {}",
            entry.file, entry.hash
        )));
    }

    // ring 3 — structure and identity
    let snap = parse(payload)?;
    if snap.header.chain_id != expect.chain_id || snap.header.pool != expect.pool {
        return Err(FeedError::Malformed(format!(
            "CHAIN_MISMATCH: snapshot is stamped chain {} pool {} but the feed declares {} {}",
            snap.header.chain_id,
            felt_hex(&snap.header.pool),
            expect.chain_id,
            felt_hex(&expect.pool)
        )));
    }
    if snap.header.epoch != entry.e || snap.header.block != entry.block {
        return Err(FeedError::Malformed(format!(
            "FEED_MALFORMED: snapshot header names epoch {} block {} but the manifest says {} {}",
            snap.header.epoch, snap.header.block, entry.e, entry.block
        )));
    }
    if entry.file != snapshot_file_name(entry.e) {
        return Err(FeedError::Malformed(format!(
            "FEED_MALFORMED: manifest snapshot file {:?} is not {:?}",
            entry.file,
            snapshot_file_name(entry.e)
        )));
    }
    if snap.slots.len() as u64 != entry.slots {
        return Err(FeedError::Malformed(format!(
            "FEED_MALFORMED: snapshot carries {} slots, manifest says {}",
            snap.slots.len(),
            entry.slots
        )));
    }

    // ring 4 — chain pin: one spine, derived leaves
    if snap.header.epoch_hash != basis_epoch_hash || entry.epoch_hash != basis_epoch_hash {
        return Err(FeedError::ChainBroken {
            epoch: entry.e,
            expected: basis_epoch_hash.to_owned(),
            actual: snap.header.epoch_hash.clone(),
        });
    }

    // ring 5 — self-consistency of the slot set against the declared root.
    // Every value compared here is produced by the same server, so this buys
    // integrity of the file and nothing at all against the publisher; the
    // canonicity claim comes from §11.3 reachability and §1.5 ring 6.
    let computed = storage_root_of(&snap);
    let declared = felt_from_hex(&entry.storage_root)?;
    if computed != snap.header.storage_root || computed != declared {
        return Err(FeedError::Malformed(format!(
            "SNAPSHOT_ROOT_MISMATCH: recomputed {} vs header {} vs manifest {}",
            felt_hex(&computed),
            felt_hex(&snap.header.storage_root),
            felt_hex(&declared)
        )));
    }
    Ok(snap)
}

/// Rings 1–5 over a COMPRESSED snapshot file, for hosts that inflate with
/// `zstd`. A thin wrapper: ring 1 before the decompressor touches the bytes,
/// then the shared ladder in [`verify_snapshot_payload`]. It holds no checks
/// of its own.
#[cfg(all(feature = "compress", feature = "mpt"))]
pub fn verify_snapshot(
    compressed: &[u8],
    entry: &crate::manifest::ManifestSnapshot,
    basis_epoch_hash: &str,
    expect: &FeedIdentity,
) -> Result<Snapshot, FeedError> {
    let zst_sha256 = hex::encode(crate::payload_sha256(compressed));
    check_zst_hash(&zst_sha256, entry)?;
    let payload = crate::decompress_capped(compressed, crate::MAX_DECOMPRESSED, &entry.file)?;
    verify_snapshot_payload(&payload, &zst_sha256, entry, basis_epoch_hash, expect)
}
