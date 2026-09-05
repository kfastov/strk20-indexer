//! Versioned folded-state container. Local storage is explicitly trusted;
//! SHA-256 catches corruption and does not authenticate a malicious cache.
use anyhow::{ensure, Result};
use strk20_consumer::{mem::MemStore, store::ConsumerStore};
use strk20_feed::manifest::{Genesis, Manifest};

const MAGIC: &[u8; 8] = b"S20FOLD2";
pub const ENGINE_VERSION: &str = env!("CARGO_PKG_VERSION");

pub fn encode(store: &MemStore, manifest: &Manifest) -> Result<Vec<u8>> {
    let mut bytes = MAGIC.to_vec();
    let meta = serde_json::to_vec(manifest)?;
    bytes.extend((meta.len() as u32).to_be_bytes());
    bytes.extend(meta);
    bytes.extend(store.encode_cache()?);
    bytes.extend(strk20_feed::payload_sha256(&bytes));
    Ok(bytes)
}

pub fn decode(bytes: &[u8], genesis: &Genesis) -> Result<(MemStore, Manifest)> {
    ensure!(
        bytes.len() >= 44 && bytes.len() <= 256 * 1024 * 1024,
        "STATE_CORRUPT: invalid size"
    );
    let (body, checksum) = bytes.split_at(bytes.len() - 32);
    ensure!(
        body.starts_with(MAGIC),
        "STATE_VERSION: cache format changed; cold start required"
    );
    ensure!(
        strk20_feed::payload_sha256(body).as_slice() == checksum,
        "STATE_CORRUPT: checksum"
    );
    let len = u32::from_be_bytes(body[8..12].try_into()?) as usize;
    ensure!(len <= body.len() - 12, "STATE_CORRUPT: header length");
    let manifest: Manifest = serde_json::from_slice(&body[12..12 + len])?;
    ensure!(
        manifest.chain_id == genesis.chain_id
            && strk20_feed::felt_from_hex(&manifest.pool)?
                == strk20_feed::felt_from_hex(&genesis.pool)?
            && manifest.epoch_size == genesis.epoch_size
            && manifest.genesis_block == genesis.genesis_block,
        "STATE_FOREIGN: cache belongs to another feed identity"
    );
    let store = MemStore::decode_cache(&body[12 + len..])?;
    ensure!(
        store.meta_get("chain_id")?.as_deref() == Some(&genesis.chain_id)
            && store
                .meta_get("pool")?
                .map(|p| strk20_feed::felt_from_hex(&p))
                .transpose()?
                == Some(strk20_feed::felt_from_hex(&genesis.pool)?),
        "STATE_FOREIGN: mirror identity"
    );
    Ok((store, manifest))
}
