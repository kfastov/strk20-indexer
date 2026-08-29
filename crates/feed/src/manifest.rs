//! Manifest and genesis documents (spec §4.2, §4.4). Not content-addressed;
//! plain serde. The manifest is the poll target that binds the epoch chain.

use crate::{FeedError, payload_sha256};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Genesis {
    pub format: String, // "strk20-feed"
    pub v: u32,
    pub chain_id: String,
    pub pool: String,
    pub genesis_block: u64,
    pub epoch_size: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestHead {
    pub number: u64,
    pub hash: String,
    pub l1_accepted: u64,
    pub class: String,
    pub decode_state: String, // "ok" | "degraded"
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EpochAnchor {
    pub block: u64,
    pub block_hash: String,
    pub storage_root: String,
    pub class: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestEpoch {
    pub e: u64,
    pub from: u64,
    pub to: u64,
    /// 64-hex sha256 of the uncompressed canonical payload.
    pub hash: String,
    /// 64-hex sha256 of the zstd file (transport checksum only).
    pub zst: String,
    pub bytes: u64,
    pub anchor: Option<EpochAnchor>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestSnapshot {
    pub block: u64,
    pub sha256: String,
    pub bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub v: u32,
    pub chain_id: String,
    pub pool: String,
    pub genesis_block: u64,
    pub epoch_size: u64,
    pub head: ManifestHead,
    pub latest_epoch: Option<u64>,
    pub epochs: Vec<ManifestEpoch>,
    pub snapshot: Option<ManifestSnapshot>,
}

impl Manifest {
    /// The manifest entry for epoch index `e`, if cut.
    pub fn epoch(&self, e: u64) -> Option<&ManifestEpoch> {
        self.epochs.iter().find(|m| m.e == e)
    }
}

/// Verify one epoch payload against its manifest entry and the previous
/// epoch's content hash (hash-chain link). `payload` is uncompressed bytes.
pub fn verify_epoch_against_manifest(
    payload: &[u8],
    entry: &ManifestEpoch,
    prev_hash: Option<[u8; 32]>,
) -> Result<crate::codec::Epoch, FeedError> {
    let actual = payload_sha256(payload);
    if hex::encode(actual) != entry.hash {
        return Err(FeedError::HashMismatch {
            epoch: entry.e,
            expected: entry.hash.clone(),
            actual: hex::encode(actual),
        });
    }
    let epoch = crate::codec::parse_epoch(payload)?;
    if epoch.header.epoch != entry.e || epoch.header.from != entry.from || epoch.header.to != entry.to
    {
        return Err(FeedError::Malformed(format!(
            "epoch {} header range disagrees with manifest",
            entry.e
        )));
    }
    if epoch.header.prev != prev_hash {
        return Err(FeedError::ChainBroken {
            epoch: entry.e,
            expected: prev_hash.map(hex::encode).unwrap_or_else(|| "null".into()),
            actual: epoch
                .header
                .prev
                .map(hex::encode)
                .unwrap_or_else(|| "null".into()),
        });
    }
    Ok(epoch)
}

/// Verify a full ordered set of (entry, payload) pairs as one chain.
pub fn verify_chain(
    items: &[(ManifestEpoch, Vec<u8>)],
) -> Result<Vec<crate::codec::Epoch>, FeedError> {
    let mut prev: Option<[u8; 32]> = None;
    let mut out = Vec::with_capacity(items.len());
    for (entry, payload) in items {
        let epoch = verify_epoch_against_manifest(payload, entry, prev)?;
        prev = Some(payload_sha256(payload));
        out.push(epoch);
    }
    Ok(out)
}
