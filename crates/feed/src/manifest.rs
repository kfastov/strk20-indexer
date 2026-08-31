//! Manifest and genesis documents (spec §4.2, §4.4). Not content-addressed;
//! plain serde. The manifest is the poll target that binds the epoch chain.

use crate::{FeedError, Felt, payload_sha256};
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

/// Snapshot entry (consumer-path.md §1.8, as amended by §11.1).
///
/// There is deliberately NO `anchor` object: a storage proof at a snapshot's
/// basis block cannot be obtained from any public provider (the window is
/// ~1024 blocks, a basis block is thousands of blocks old at cut time), so a
/// schema that required one would publish no snapshots at all. Grounding is
/// the §11.3 reachability check against `anchors.ndjson` instead.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ManifestSnapshot {
    pub e: u64,
    pub block: u64,
    /// Content hash of the basis epoch — the pin onto the one hash chain.
    pub epoch_hash: String,
    /// Feed-relative path, `snapshots/{e:08}.strk20s.zst`.
    pub file: String,
    /// 64-hex sha256 of the UNCOMPRESSED payload (content identity).
    pub hash: String,
    /// 64-hex sha256 of the `.zst` file (transport checksum only).
    pub zst: String,
    pub bytes: u64,
    pub slots: u64,
    pub storage_root: String,
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

/// Bind a verified epoch to the chain and pool it claims to be about.
///
/// The hash chain proves an epoch is the one the manifest names; it says
/// nothing about WHICH chain that manifest describes. Without this, a feed
/// carrying the same pool address on a fork or test chain folds cleanly into a
/// mirror built from the real one (review finding F8: chain id was stamped
/// everywhere and compared nowhere).
pub fn verify_epoch_binding(
    epoch: &crate::codec::Epoch,
    chain_id: &str,
    pool: &Felt,
) -> Result<(), FeedError> {
    if epoch.header.chain_id != chain_id {
        return Err(FeedError::Malformed(format!(
            "epoch {} is stamped chain {} but the feed declares {chain_id}",
            epoch.header.epoch, epoch.header.chain_id
        )));
    }
    if epoch.header.pool != *pool {
        return Err(FeedError::Malformed(format!(
            "epoch {} is stamped pool {} but the feed declares {}",
            epoch.header.epoch,
            crate::felt_hex(&epoch.header.pool),
            crate::felt_hex(pool)
        )));
    }
    Ok(())
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
