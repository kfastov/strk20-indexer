//! In-memory transport for public bytes staged by the Worker. Rust validates
//! payload identity and hashes; checkpoint verification establishes chain state.

use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use strk20_consumer::transport::FeedTransport;
use strk20_feed::manifest::{Genesis, Manifest};

/// A staged payload. `Arc` because every epoch is reachable under two keys —
/// its index and its content hash — and holding two `Vec`s meant the module
/// carried the whole feed twice (20 MB of wasm heap for the 10 MB Sepolia
/// history) and paid a full copy at stage time and another at export time.
type Bytes = Arc<Vec<u8>>;

#[derive(Default)]
struct Staged {
    genesis: Option<Genesis>,
    manifest: Option<Manifest>,
    /// epoch index -> raw payload
    epochs: BTreeMap<u64, Bytes>,
    /// epoch index -> compressed snapshot bytes (ring 1 is checked over these)
    snapshots: BTreeMap<u64, Bytes>,
    head: Option<(Vec<u8>, String)>,
    /// `sha256(bytes as Block B will see them) -> inflated payload`
    inflated: BTreeMap<[u8; 32], Bytes>,
}

/// Push-side handle. Cheap to share; every method locks for the duration of one
/// map operation only.
#[derive(Default)]
pub struct StagedFeed {
    inner: Mutex<Staged>,
}

fn h(bytes: &[u8]) -> [u8; 32] {
    strk20_feed::payload_sha256(bytes)
}

impl StagedFeed {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Staged> {
        self.inner.lock().expect("staged feed poisoned")
    }

    pub fn set_genesis(&self, json: &str) -> Result<Genesis> {
        let g: Genesis = serde_json::from_str(json)
            .map_err(|e| anyhow!("FEED_MALFORMED: genesis.json is not a genesis document: {e}"))?;
        self.lock().genesis = Some(g.clone());
        Ok(g)
    }

    pub fn genesis(&self) -> Option<Genesis> {
        self.lock().genesis.clone()
    }

    pub fn set_manifest(&self, json: &str) -> Result<()> {
        let m: Manifest = serde_json::from_str(json)
            .map_err(|e| anyhow!("FEED_MALFORMED: manifest.json is not a manifest: {e}"))?;
        self.lock().manifest = Some(m);
        Ok(())
    }

    pub fn manifest(&self) -> Option<Manifest> {
        self.lock().manifest.clone()
    }

    /// Raw (already-inflated) epoch payload. Block B checks its sha256 against
    /// the manifest entry, so staging the wrong bytes fails loudly.
    pub fn put_epoch(&self, e: u64, payload: Vec<u8>) {
        let payload: Bytes = Arc::new(payload);
        let mut g = self.lock();
        g.inflated.insert(h(&payload), Arc::clone(&payload));
        g.epochs.insert(e, payload);
    }

    /// Compressed snapshot bytes **and** what TypeScript inflated from them.
    /// Both are needed: ring 1 hashes the former, rings 2-5 parse the latter.
    pub fn put_snapshot(&self, e: u64, zst: Vec<u8>, payload: Vec<u8>) {
        let mut g = self.lock();
        g.inflated.insert(h(&zst), Arc::new(payload));
        g.snapshots.insert(e, Arc::new(zst));
    }

    /// `head.ndjson` plus the ETag it was served with. Block B skips the tail
    /// rebuild when the ETag it already holds matches, exactly as over HTTP.
    pub fn put_head(&self, payload: Vec<u8>, etag: String) {
        self.lock().head = Some((payload, etag));
    }

    /// Staged artifacts are transient; folded state owns persistence.
    pub fn clear_applied(&self) {
        let mut g = self.lock();
        g.epochs.clear();
        g.snapshots.clear();
        g.inflated.clear();
    }
}

#[async_trait]
impl FeedTransport for StagedFeed {
    async fn fetch_genesis(&self) -> Result<Genesis> {
        self.lock()
            .genesis
            .clone()
            .ok_or_else(|| anyhow!("NOT_STAGED: genesis.json was never staged"))
    }

    async fn fetch_manifest(&self) -> Result<Manifest> {
        self.lock()
            .manifest
            .clone()
            .ok_or_else(|| anyhow!("NOT_STAGED: manifest.json was never staged"))
    }

    async fn fetch_epoch(&self, idx: u64) -> Result<Vec<u8>> {
        self.lock()
            .epochs
            .get(&idx)
            .map(|b| b.as_ref().clone())
            .ok_or_else(|| anyhow!("NOT_STAGED: epoch {idx} was never staged"))
    }

    async fn fetch_snapshot(&self, e: u64) -> Result<Vec<u8>> {
        self.lock()
            .snapshots
            .get(&e)
            .map(|b| b.as_ref().clone())
            .ok_or_else(|| anyhow!("NOT_STAGED: snapshot for epoch {e} was never staged"))
    }

    async fn fetch_anchor(&self, _idx: u64) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }

    async fn fetch_snapshot_anchor(&self, _e: u64) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }
    async fn fetch_anchors(&self) -> Result<Option<Vec<u8>>> {
        Ok(None)
    }

    async fn fetch_head(&self, etag: Option<&str>) -> Result<Option<(Vec<u8>, String)>> {
        let g = self.lock();
        match &g.head {
            // `None` means "unchanged" — the ETag path, honoured here so a
            // re-apply with no new head does not rebuild the tail.
            Some((_, staged_etag)) if etag == Some(staged_etag.as_str()) => Ok(None),
            Some((bytes, staged_etag)) => Ok(Some((bytes.clone(), staged_etag.clone()))),
            None => Ok(None),
        }
    }

    fn decompress(&self, bytes: &[u8], cap: u64, artifact: &str) -> Result<Vec<u8>> {
        let g = self.lock();
        let Some(raw) = g.inflated.get(&h(bytes)) else {
            bail!(
                "NOT_STAGED: no inflated payload was staged for {artifact}. This module \
                 does not link zstd (§3.4); the caller must inflate and stage the result."
            );
        };
        // R-I still applies on this side of the seam: the caller inflated, but
        // the cap is Block B's rule and is enforced where Block B asks for it.
        if raw.len() as u64 > cap {
            bail!(
                "DECOMPRESS_LIMIT: inflated {artifact} is {} bytes, past the {cap}-byte cap",
                raw.len()
            );
        }
        Ok(raw.as_ref().clone())
    }
}
