//! STRK20 feed format v1 (docs/spec/architecture.md §4).
//!
//! The canonical product of the indexer is a directory of content-addressed
//! static files. This crate owns their byte format: canonical NDJSON epoch
//! encoding, sha256 content addressing, hash-chain and manifest verification,
//! and the head-tail grammar. Pure bytes-in/bytes-out; no IO, no async.

pub mod anchors;
#[cfg(feature = "mpt")]
pub mod checkpoint;
pub mod codec;
pub mod manifest;
#[cfg(feature = "mpt")]
pub mod mpt;
pub mod snapshot;

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

/// Chain ids of the supported networks. Both the indexer's `--network`
/// profile and the client's `--network` expectation resolve through here so
/// the two can never disagree about what a name means.
pub const CHAIN_ID_MAINNET: &str = "SN_MAIN";
pub const CHAIN_ID_SEPOLIA: &str = "SN_SEPOLIA";

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
    #[error("DECOMPRESS_LIMIT: {artifact} expands past the {cap}-byte output cap")]
    DecompressLimit { artifact: String, cap: u64 },
}

#[cfg(feature = "compress")]
pub fn compress(payload: &[u8]) -> Vec<u8> {
    zstd::encode_all(payload, 19).expect("in-memory zstd cannot fail")
}

/// §1.5 ring 1 output cap (R-I). The transport hash is authored by the same
/// server as the file it names, so a passing hash says nothing about how far
/// the frame expands: without a cap a ~100 KB `.zst` can be made to allocate
/// tens of GB, which on the browser target this format exists to serve is a
/// tab crash rather than a recoverable error.
pub const MAX_DECOMPRESSED: u64 = 256 * 1024 * 1024;

#[cfg(feature = "compress")]
pub fn decompress(bytes: &[u8]) -> Result<Vec<u8>, FeedError> {
    decompress_capped(bytes, MAX_DECOMPRESSED, "feed artifact")
}

/// Decompress with a hard output cap. The reader is bounded BEFORE any
/// allocation grows past `cap`, so an over-long frame costs `cap` bytes and
/// not the frame's true size.
#[cfg(feature = "compress")]
pub fn decompress_capped(bytes: &[u8], cap: u64, artifact: &str) -> Result<Vec<u8>, FeedError> {
    use std::io::Read;
    let decoder = zstd::Decoder::new(bytes).map_err(|e| FeedError::Decompress(e.to_string()))?;
    // Read one byte past the cap: a stream that yields cap+1 bytes is over the
    // limit, one that ends at exactly cap is not.
    let mut limited = decoder.take(cap + 1);
    let mut out = Vec::new();
    limited
        .read_to_end(&mut out)
        .map_err(|e| FeedError::Decompress(e.to_string()))?;
    if out.len() as u64 > cap {
        return Err(FeedError::DecompressLimit {
            artifact: artifact.to_owned(),
            cap,
        });
    }
    Ok(out)
}

#[cfg(feature = "mpt")]
pub mod trie;
