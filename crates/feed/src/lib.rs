//! STRK20 feed format v1 (docs/spec/architecture.md §4).
//!
//! The canonical product of the indexer is a directory of content-addressed
//! static files. This crate owns their byte format: canonical NDJSON epoch
//! encoding, sha256 content addressing, hash-chain and manifest verification,
//! and the head-tail grammar. Pure bytes-in/bytes-out; no IO, no async.

pub mod codec;
pub mod manifest;
#[cfg(feature = "mpt")]
pub mod mpt;

use sha2::{Digest, Sha256};
pub use starknet_types_core::felt::Felt;

/// sha256 over the UNCOMPRESSED canonical payload bytes — the content identity
/// of an epoch file. zstd output is compressor-version-unstable and is never
/// used for identity.
pub fn payload_sha256(payload: &[u8]) -> [u8; 32] {
    let mut h = Sha256::new();
    h.update(payload);
    h.finalize().into()
}

/// Canonical felt rendering: lowercase `0x`-prefixed minimal hex, zero = `0x0`.
pub fn felt_hex(f: &Felt) -> String {
    let bytes = f.to_bytes_be();
    let hex = hex::encode(bytes);
    let trimmed = hex.trim_start_matches('0');
    if trimmed.is_empty() {
        "0x0".to_owned()
    } else {
        format!("0x{trimmed}")
    }
}

/// Parse a felt from canonical (or any 0x-prefixed) hex.
pub fn felt_from_hex(s: &str) -> Result<Felt, FeedError> {
    Felt::from_hex(s).map_err(|_| FeedError::BadFelt(s.to_owned()))
}

#[derive(Debug, thiserror::Error)]
pub enum FeedError {
    #[error("invalid felt hex: {0}")]
    BadFelt(String),
    #[error("malformed feed file: {0}")]
    Malformed(String),
    #[error("json: {0}")]
    Json(#[from] serde_json::Error),
    #[error("content hash mismatch for epoch {epoch}: expected {expected}, got {actual}")]
    HashMismatch {
        epoch: u64,
        expected: String,
        actual: String,
    },
    #[error("hash chain broken at epoch {epoch}: prev is {actual}, expected {expected}")]
    ChainBroken {
        epoch: u64,
        expected: String,
        actual: String,
    },
    #[error("decompression: {0}")]
    Decompress(String),
}

#[cfg(feature = "compress")]
pub fn compress(payload: &[u8]) -> Vec<u8> {
    zstd::encode_all(payload, 19).expect("in-memory zstd cannot fail")
}

#[cfg(feature = "compress")]
pub fn decompress(bytes: &[u8]) -> Result<Vec<u8>, FeedError> {
    zstd::decode_all(bytes).map_err(|e| FeedError::Decompress(e.to_string()))
}
