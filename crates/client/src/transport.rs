//! FeedTransport (spec §7.2) — the type-system privacy boundary.
//!
//! NO method accepts an address, key, slot, or any user-derived value: a
//! feed-mode client physically cannot ask the server anything about itself.
//! The compile-fail suite in e2e-tests locks this signature.

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use std::path::PathBuf;
use strk20_feed::manifest::{Genesis, Manifest};

#[async_trait]
pub trait FeedTransport: Send + Sync {
    async fn fetch_genesis(&self) -> Result<Genesis>;
    async fn fetch_manifest(&self) -> Result<Manifest>;
    /// Compressed epoch bytes.
    async fn fetch_epoch(&self, idx: u64) -> Result<Vec<u8>>;
    /// Compressed snapshot bytes for epoch `e`. `e` comes from the manifest —
    /// feed progress, never anything derived from a user.
    async fn fetch_snapshot(&self, e: u64) -> Result<Vec<u8>>;
    async fn fetch_anchor(&self, idx: u64) -> Result<Option<Vec<u8>>>;
    /// The stored `getStorageProof` response for snapshot `e`'s basis block
    /// (§1.3, reinstated by §12 point 1); `None` when the feed publishes none.
    /// `e` is a manifest-supplied epoch index — feed progress, never anything
    /// derived from a user.
    async fn fetch_snapshot_anchor(&self, e: u64) -> Result<Option<Vec<u8>>>;
    /// The append-only anchor log; `None` when the feed publishes none.
    async fn fetch_anchors(&self) -> Result<Option<Vec<u8>>>;
    /// `None` = unchanged (ETag matched). Returns (payload, new_etag).
    async fn fetch_head(&self, etag: Option<&str>) -> Result<Option<(Vec<u8>, String)>>;
}

/// HTTP transport against a `/feed` base URL. Emits only parameterless GETs.
pub struct HttpTransport {
    base: String,
    http: reqwest::Client,
}

impl HttpTransport {
    pub fn new(base: &str) -> Self {
        Self {
            base: base.trim_end_matches('/').to_owned(),
            http: reqwest::Client::builder()
                .user_agent(concat!("strk20-sync/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("reqwest client"),
        }
    }

    async fn get_bytes(&self, path: &str) -> Result<Vec<u8>> {
        let url = format!("{}/{}", self.base, path);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        if !resp.status().is_success() {
            bail!("GET {url}: HTTP {}", resp.status());
        }
        Ok(resp.bytes().await?.to_vec())
    }

    /// Fetch an OPTIONAL artifact. Only a 404 means "the feed does not publish
    /// this"; every other outcome (5xx, connection refused, a truncated body)
    /// is an error the caller must see. Collapsing them into `None` is how a
    /// verification command comes back green against a feed it never reached.
    async fn get_optional(&self, path: &str) -> Result<Option<Vec<u8>>> {
        let url = format!("{}/{}", self.base, path);
        let resp = self
            .http
            .get(&url)
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !resp.status().is_success() {
            bail!("GET {url}: HTTP {}", resp.status());
        }
        Ok(Some(resp.bytes().await?.to_vec()))
    }
}

/// Same rule for a local mirror directory: absent is `None`, unreadable is an
/// error.
async fn read_optional(path: std::path::PathBuf) -> Result<Option<Vec<u8>>> {
    match tokio::fs::read(&path).await {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(anyhow::Error::new(e).context(format!("read {}", path.display()))),
    }
}

#[async_trait]
impl FeedTransport for HttpTransport {
    async fn fetch_genesis(&self) -> Result<Genesis> {
        Ok(serde_json::from_slice(&self.get_bytes("genesis.json").await?)?)
    }

    async fn fetch_manifest(&self) -> Result<Manifest> {
        Ok(serde_json::from_slice(
            &self.get_bytes("manifest.json").await?,
        )?)
    }

    async fn fetch_epoch(&self, idx: u64) -> Result<Vec<u8>> {
        self.get_bytes(&format!("epochs/{idx:08}.strk20e.zst")).await
    }

    async fn fetch_snapshot(&self, e: u64) -> Result<Vec<u8>> {
        self.get_bytes(&format!("snapshots/{e:08}.strk20s.zst")).await
    }

    async fn fetch_anchor(&self, idx: u64) -> Result<Option<Vec<u8>>> {
        self.get_optional(&format!("epochs/{idx:08}.anchor.json"))
            .await
    }

    async fn fetch_snapshot_anchor(&self, e: u64) -> Result<Option<Vec<u8>>> {
        self.get_optional(&strk20_feed::manifest::snapshot_anchor_file_name(e))
            .await
    }

    async fn fetch_anchors(&self) -> Result<Option<Vec<u8>>> {
        self.get_optional("anchors.ndjson").await
    }

    async fn fetch_head(&self, etag: Option<&str>) -> Result<Option<(Vec<u8>, String)>> {
        let url = format!("{}/head.ndjson", self.base);
        let mut req = self.http.get(&url);
        if let Some(tag) = etag {
            req = req.header(reqwest::header::IF_NONE_MATCH, tag);
        }
        let resp = req.send().await.with_context(|| format!("GET {url}"))?;
        if resp.status() == reqwest::StatusCode::NOT_MODIFIED {
            return Ok(None);
        }
        if !resp.status().is_success() {
            bail!("GET {url}: HTTP {}", resp.status());
        }
        let new_etag = resp
            .headers()
            .get(reqwest::header::ETAG)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned();
        let bytes = resp.bytes().await?.to_vec();
        let etag = if new_etag.is_empty() {
            format!(
                "\"{}\"",
                hex::encode(strk20_feed::payload_sha256(&bytes))
            )
        } else {
            new_etag
        };
        Ok(Some((bytes, etag)))
    }
}

/// Local directory transport (mirror dir, air-gap, tests).
pub struct DirTransport {
    dir: PathBuf,
}

impl DirTransport {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }
}

#[async_trait]
impl FeedTransport for DirTransport {
    async fn fetch_genesis(&self) -> Result<Genesis> {
        Ok(serde_json::from_slice(&tokio::fs::read(
            self.dir.join("genesis.json"),
        )
        .await?)?)
    }

    async fn fetch_manifest(&self) -> Result<Manifest> {
        Ok(serde_json::from_slice(&tokio::fs::read(
            self.dir.join("manifest.json"),
        )
        .await?)?)
    }

    async fn fetch_epoch(&self, idx: u64) -> Result<Vec<u8>> {
        Ok(tokio::fs::read(
            self.dir.join("epochs").join(format!("{idx:08}.strk20e.zst")),
        )
        .await?)
    }

    async fn fetch_snapshot(&self, e: u64) -> Result<Vec<u8>> {
        Ok(tokio::fs::read(
            self.dir
                .join("snapshots")
                .join(format!("{e:08}.strk20s.zst")),
        )
        .await?)
    }

    async fn fetch_anchor(&self, idx: u64) -> Result<Option<Vec<u8>>> {
        read_optional(self.dir.join("epochs").join(format!("{idx:08}.anchor.json"))).await
    }

    async fn fetch_snapshot_anchor(&self, e: u64) -> Result<Option<Vec<u8>>> {
        read_optional(
            self.dir
                .join(strk20_feed::manifest::snapshot_anchor_file_name(e)),
        )
        .await
    }

    async fn fetch_anchors(&self) -> Result<Option<Vec<u8>>> {
        read_optional(self.dir.join("anchors.ndjson")).await
    }

    async fn fetch_head(&self, etag: Option<&str>) -> Result<Option<(Vec<u8>, String)>> {
        let bytes = tokio::fs::read(self.dir.join("head.ndjson")).await?;
        let tag = format!(
            "\"{}\"",
            hex::encode(strk20_feed::payload_sha256(&bytes))
        );
        if etag == Some(tag.as_str()) {
            return Ok(None);
        }
        Ok(Some((bytes, tag)))
    }
}

/// Parse `--feed` argument: an http(s) URL or a local directory.
pub fn transport_for(feed: &str) -> Box<dyn FeedTransport> {
    if feed.starts_with("http://") || feed.starts_with("https://") {
        Box::new(HttpTransport::new(feed))
    } else {
        Box::new(DirTransport::new(PathBuf::from(feed)))
    }
}
