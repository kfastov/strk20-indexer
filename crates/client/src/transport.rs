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
    async fn fetch_anchor(&self, idx: u64) -> Result<Option<Vec<u8>>>;
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

    async fn fetch_anchor(&self, idx: u64) -> Result<Option<Vec<u8>>> {
        match self.get_bytes(&format!("epochs/{idx:08}.anchor.json")).await {
            Ok(b) => Ok(Some(b)),
            Err(_) => Ok(None),
        }
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

    async fn fetch_anchor(&self, idx: u64) -> Result<Option<Vec<u8>>> {
        match tokio::fs::read(
            self.dir.join("epochs").join(format!("{idx:08}.anchor.json")),
        )
        .await
        {
            Ok(b) => Ok(Some(b)),
            Err(_) => Ok(None),
        }
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
