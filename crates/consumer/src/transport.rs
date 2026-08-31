//! `FeedTransport` (spec §7.2) — the type-system privacy boundary.
//!
//! NO method accepts an address, key, slot, or any user-derived value: a
//! feed-mode client physically cannot ask the server anything about itself.
//! The compile-fail suite in e2e-tests locks this signature.
//!
//! The trait lives here rather than in the native client because Block B is
//! written against it: an HTTP transport, a directory transport and a browser
//! `fetch` transport are three hosts for one state machine.

use anyhow::Result;
use async_trait::async_trait;
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

    /// Inflate a feed artifact with a hard output cap (R-I), naming `artifact`
    /// in the failure.
    ///
    /// This is a *transport* obligation and not Block B's for one concrete
    /// reason: `zstd-sys` compiles C and has no wasm32-unknown-unknown
    /// backend, so a consumer core that linked it could not run in a browser
    /// at all. Nothing verification-bearing moves out with it — the `.zst`
    /// sha256 is checked by Block B BEFORE this is called and the payload
    /// sha256 immediately after, so a transport that inflates the wrong bytes
    /// is caught by the same two hashes as before. The browser transport hands
    /// back what TypeScript already inflated (§3.4).
    fn decompress(&self, bytes: &[u8], cap: u64, artifact: &str) -> Result<Vec<u8>>;
}
