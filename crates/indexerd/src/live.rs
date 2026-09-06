//! SSE transports the same canonical head and epoch payloads as HTTP.
//! Each head replaces the mutable tail, so reconnect and coalesced updates
//! need no per-client journal. A client missing immutable epochs catches up
//! over HTTP before applying the tail. Hash-based event IDs deduplicate repeats.
//! Published files are the source; a slow subscriber never blocks publishing.

use crate::db::Db;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::watch;

/// §2.2 connect padding: defeats buffering middleboxes that hold a response
/// until some minimum number of bytes has arrived.
pub const PADDING_BYTES: usize = 2048;
/// §2.2 `retry:` field, milliseconds.
pub const RETRY_MS: u64 = 15_000;
/// §2.2 keepalive cadence, and §2.5's watchdog budget on the client side.
pub const KEEPALIVE: Duration = Duration::from_secs(15);
/// Oversized artifacts use HTTP catch-up; SSE queues stay bounded.
const MAX_INLINE: usize = 2 * 1024 * 1024;

/// The state every subscriber is shown. Each field is the `data:` payload of
/// one event, already serialized, so every subscriber emits BYTE-IDENTICAL
/// bytes for the same state — there is nothing per-client to differ.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FeedState {
    pub head: Option<String>,
    pub epoch: Option<String>,
    pub snapshot: Option<String>,
    pub status: Option<String>,
}

pub struct LiveHub {
    tx: watch::Sender<Arc<FeedState>>,
    connections: AtomicUsize,
    source: Mutex<Source>,
}

struct Source {
    feed_dir: PathBuf,
    db: Arc<Mutex<Db>>,
    cache: HeadCache,
}

impl LiveHub {
    pub fn new(feed_dir: PathBuf, db: Arc<Mutex<Db>>) -> Self {
        let (tx, _rx) = watch::channel(Arc::new(FeedState::default()));
        Self {
            tx,
            connections: AtomicUsize::new(0),
            source: Mutex::new(Source {
                feed_dir,
                db,
                cache: HeadCache::default(),
            }),
        }
    }

    /// Re-read the published files and publish what they say.
    ///
    /// The writer calls this immediately after publication and after a change
    /// to verification status. Connecting clients also refresh their initial
    /// burst. No timer sits between a committed feed and its subscribers.
    pub fn refresh(&self) {
        let mut src = self.source.lock().expect("live source");
        let Source { feed_dir, db, cache } = &mut *src;
        let state = read_state(feed_dir, db, cache);
        // Keep ordering across concurrent connects and writer notifications.
        self.publish(state);
    }

    pub fn subscribe(&self) -> watch::Receiver<Arc<FeedState>> {
        self.tx.subscribe()
    }

    pub fn publish(&self, state: FeedState) {
        // send_if_modified keeps the change flag honest: a re-read of unchanged
        // files must not wake every subscriber.
        self.tx.send_if_modified(|cur| {
            if **cur == state {
                false
            } else {
                *cur = Arc::new(state);
                true
            }
        });
    }

    pub fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }

    pub fn opened(&self) -> ConnectionGuard<'_> {
        self.connections.fetch_add(1, Ordering::SeqCst);
        ConnectionGuard { hub: self }
    }
}

pub struct ConnectionGuard<'a> {
    hub: &'a LiveHub,
}

impl Drop for ConnectionGuard<'_> {
    fn drop(&mut self) {
        self.hub.connections.fetch_sub(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct HeadCache {
    etag: String,
    data: Option<String>,
    epoch_entry: Option<Value>,
    epoch_data: Option<String>,
}

fn read_state(feed_dir: &Path, db: &Arc<Mutex<Db>>, cache: &mut HeadCache) -> FeedState {
    let head = std::fs::read(feed_dir.join("head.ndjson"))
        .ok()
        .and_then(|bytes| head_event(&bytes, cache));
    let manifest: Option<Value> = std::fs::read(feed_dir.join("manifest.json"))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok());
    let epoch = manifest.as_ref().and_then(|m| {
        let entry=m["epochs"].as_array()?.iter().find(|row|row["e"]==m["latest_epoch"])?;
        if cache.epoch_entry.as_ref()!=Some(entry) {
            cache.epoch_data=epoch_event(m,feed_dir);
            cache.epoch_entry=Some(entry.clone());
        }
        cache.epoch_data.clone()
    });
    let snapshot = manifest.as_ref().and_then(snapshot_event);
    let decode_state = manifest
        .as_ref()
        .and_then(|m| m["head"]["decode_state"].as_str())
        .unwrap_or("ok")
        .to_owned();
    let verify_root_failed = db
        .lock()
        .ok()
        .and_then(|db| db.meta_get("verify_root_failed").ok().flatten())
        .map(|v| v == "1")
        .unwrap_or(false);
    FeedState {
        head,
        epoch,
        snapshot,
        status: Some(
            json!({"decode_state": decode_state, "verify_root_failed": verify_root_failed})
                .to_string(),
        ),
    }
}

fn head_event(bytes: &[u8], cache: &mut HeadCache) -> Option<String> {
    let etag = format!("\"{}\"", hex::encode(strk20_feed::payload_sha256(bytes)));
    if etag == cache.etag {
        return cache.data.clone();
    }
    let head = strk20_feed::codec::parse_head(bytes).ok()?;
    let payload = (bytes.len() <= MAX_INLINE)
        .then(|| std::str::from_utf8(bytes).ok()).flatten();
    let data = json!({
        "head": head.header.head,
        "head_hash": strk20_feed::felt_hex(&head.header.head_hash),
        "l1_accepted": head.header.l1_accepted,
        "tail_from": head.header.tail_from,
        "etag": etag,
        "payload": payload,
        "resync": payload.is_none(),
    })
    .to_string();
    cache.etag = etag;
    cache.data = Some(data.clone());
    Some(data)
}

/// Review finding 14d: the epoch index key is `"e"` on BOTH events that name an
/// epoch, because the manifest — the identity source the client
/// cross-references — uses `"e"`.
fn epoch_event(manifest: &Value, feed_dir: &Path) -> Option<String> {
    let latest = manifest["latest_epoch"].as_u64()?;
    let entry = manifest["epochs"]
        .as_array()?
        .iter()
        .find(|e| e["e"].as_u64() == Some(latest))?;
    let file = feed_dir.join(format!("epochs/{latest:08}.strk20e.zst"));
    let payload = std::fs::read(file).ok()
        .filter(|bytes| bytes.len() <= MAX_INLINE)
        .and_then(|bytes| strk20_feed::decompress_capped(&bytes, MAX_INLINE as u64, "SSE epoch").ok())
        .and_then(|bytes| String::from_utf8(bytes).ok());
    Some(
        json!({
            "entry": entry,
            "payload": payload,
            "resync": payload.is_none(),
            "e": latest,
            "from": entry["from"],
            "to": entry["to"],
            "hash": entry["hash"],
            "zst": entry["zst"],
            "bytes": entry["bytes"],
        })
        .to_string(),
    )
}

fn snapshot_event(manifest: &Value) -> Option<String> {
    let s = manifest.get("snapshot")?;
    if s.is_null() {
        return None;
    }
    Some(json!({"e": s["e"], "block": s["block"], "hash": s["hash"]}).to_string())
}

/// The connect burst plus the delta loop, as one byte stream.
///
/// `hello` first, so a proxy pointed at the wrong network dies before any
/// refetch or state mutation.
pub async fn stream_to(
    hub: Arc<LiveHub>,
    hello: String,
    tx: tokio::sync::mpsc::Sender<std::io::Result<axum::body::Bytes>>,
) {
    let _guard = hub.opened();
    let mut rx = hub.subscribe();
    let mut sent = FeedState::default();

    let mut opening = String::with_capacity(PADDING_BYTES + 64);
    opening.push(':');
    opening.extend(std::iter::repeat_n(' ', PADDING_BYTES));
    opening.push_str("\n\n");
    opening.push_str(&format!("retry: {RETRY_MS}\n\n"));
    opening.push_str(&event("hello", &hello));
    if send(&tx, opening).await.is_err() {
        return;
    }

    loop {
        let current = rx.borrow_and_update().clone();
        let mut out = String::new();
        for (name, next, prev) in [
            ("epoch", &current.epoch, &sent.epoch),
            ("head", &current.head, &sent.head),
            ("snapshot", &current.snapshot, &sent.snapshot),
            ("status", &current.status, &sent.status),
        ] {
            if let Some(data) = next {
                if Some(data) != prev.as_ref() {
                    out.push_str(&event(name, data));
                }
            }
        }
        sent = (*current).clone();
        if !out.is_empty() && send(&tx, out).await.is_err() {
            return;
        }
        match tokio::time::timeout(KEEPALIVE, rx.changed()).await {
            Ok(Ok(())) => {}
            // the publisher is gone: nothing more can ever be announced
            Ok(Err(_)) => return,
            Err(_) => {
                if send(&tx, ": ka\n\n".to_owned()).await.is_err() {
                    return;
                }
            }
        }
    }
}

fn event(name: &str, data: &str) -> String {
    let id = hex::encode(strk20_feed::payload_sha256(data.as_bytes()));
    format!("event: {name}\nid: {name}:{id}\ndata: {data}\n\n")
}

async fn send(
    tx: &tokio::sync::mpsc::Sender<std::io::Result<axum::body::Bytes>>,
    text: String,
) -> Result<(), ()> {
    tokio::time::timeout(KEEPALIVE, tx.send(Ok(axum::body::Bytes::from(text))))
        .await
        .map_err(|_| ())?
        .map_err(|_| ())
}
