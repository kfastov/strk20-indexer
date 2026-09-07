//! SSE bootstraps the canonical head, then sends only appended records and
//! the new header/trailer. Deltas name the previously sent head's ETag; a
//! reconnect, epoch rollover or changed prefix sends a full replacement.
//! Coalesced updates diff against the last sent state, with no replay journal.
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
    pub proof: Option<String>,
}

pub struct LiveHub {
    tx: watch::Sender<Arc<FeedState>>,
    connections: AtomicUsize,
    catchup: watch::Sender<()>,
    source: Mutex<Source>,
    proofs: Mutex<Option<Arc<crate::proofs::Proofs>>>,
}

struct Source {
    feed_dir: PathBuf,
    db: Arc<Mutex<Db>>,
    cache: HeadCache,
}

impl LiveHub {
    pub fn new(feed_dir: PathBuf, db: Arc<Mutex<Db>>) -> Self {
        let (tx, _rx) = watch::channel(Arc::new(FeedState::default()));
        let (catchup, _) = watch::channel(());
        Self {
            tx,
            proofs: Mutex::new(None),
            catchup,
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
        let mut state = read_state(feed_dir, db, cache);
        state.proof = self.tx.borrow().proof.clone();
        if let Some(head) = state.head.as_ref().and_then(|h| serde_json::from_str::<Value>(h).ok()) {
            if let Some(block) = head["head"].as_u64() {
                if let Some(proofs) = self.proofs() { proofs.request(block); }
            }
        }
        // Keep ordering across concurrent connects and writer notifications.
        self.publish(state);
    }

    pub fn start_proofs(self: &Arc<Self>, rpc: Arc<crate::rpc::RpcClient>, pool: starknet_types_core::felt::Felt) {
        *self.proofs.lock().expect("proof broker") = Some(crate::proofs::Proofs::new(rpc, pool, Arc::downgrade(self)));
    }

    pub fn proofs(&self) -> Option<Arc<crate::proofs::Proofs>> {
        self.proofs.lock().expect("proof broker").clone()
    }

    pub fn invalidate_proofs(&self) {
        if let Some(proofs) = self.proofs() { proofs.invalidate(); }
        self.publish_proof(json!({"reset": true}).to_string());
    }

    pub fn publish_proof(&self, text: String) {
        // Use the same lock as refresh to preserve proof updates across reads.
        let _source = self.source.lock().expect("live source");
        self.tx.send_modify(|state| Arc::make_mut(state).proof = Some(text));
    }

    pub fn catchup_requests(&self) -> watch::Receiver<()> {
        self.catchup.subscribe()
    }

    /// A public head read is also demand for fresh data. Coalesce requests
    /// once per publication, so slow upstream notifications need not stall a
    /// waiting client and a burst of readers cannot create a burst of RPCs.
    pub fn request_catchup(&self) {
        let mut src = self.source.lock().expect("live source");
        if !src.cache.catchup_requested {
            src.cache.catchup_requested = true;
            self.catchup.send_replace(());
        }
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
    catchup_requested: bool,
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
        proof: None,
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
    cache.catchup_requested = false;
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

/// NDJSON parts retain their newlines, so applying a delta reproduces the
/// canonical HTTP artifact byte-for-byte (including its counts/class trailer).
fn head_parts(payload: &str) -> Option<(&str, &str, &str)> {
    let first = payload.find('\n')? + 1;
    let last = payload.strip_suffix('\n')?.rfind('\n')? + 1;
    (first <= last).then(|| (&payload[..first], &payload[first..last], &payload[last..]))
}

fn head_update(next: &str, prev: Option<&str>) -> String {
    let delta = (|| -> Option<String> {
        let previous: Value = serde_json::from_str(prev?).ok()?;
        let mut current: Value = serde_json::from_str(next).ok()?;
        if previous["tail_from"] != current["tail_from"] {
            return None;
        }
        let (_, old_records, _) = head_parts(previous["payload"].as_str()?)?;
        let (header, records, end) = head_parts(current["payload"].as_str()?)?;
        let append = records.strip_prefix(old_records)?;
        let change = json!({
            "base_etag": previous["etag"].as_str()?,
            "header": header,
            "append": append,
            "end": end,
        });
        current["payload"] = Value::Null;
        current["delta"] = change;
        Some(current.to_string())
    })();
    // Tiny tails need no delta overhead. Old clients see payload=null and
    // safely use the existing HTTP catch-up path; the feed URL is unchanged.
    delta.filter(|data| data.len() < next.len()).unwrap_or_else(|| next.to_owned())
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
            ("proof", &current.proof, &sent.proof),
            ("snapshot", &current.snapshot, &sent.snapshot),
            ("status", &current.status, &sent.status),
        ] {
            if let Some(data) = next {
                if Some(data) != prev.as_ref() {
                    if name == "head" {
                        out.push_str(&event(name, &head_update(data, prev.as_deref())));
                    } else {
                        out.push_str(&event(name, data));
                    }
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

#[cfg(test)]
mod tests {
    use super::*;

    fn state(block: u64, from: u64, records: &str) -> String {
        let payload = format!("{{\"t\":\"hdr\",\"head\":{block}}}\n{records}{{\"t\":\"end\"}}\n");
        json!({"head":block,"tail_from":from,"etag":format!("head-{block}"),
            "payload":payload,"resync":false}).to_string()
    }

    #[test]
    fn live_delta_sends_only_new_records_and_reconstructs_exact_bytes() {
        let old_records = format!("{{\"t\":\"blk\",\"data\":\"{}\"}}\n", "a".repeat(4096));
        let first = state(10, 1, &old_records);
        for append in ["", "{\"t\":\"blk\",\"number\":11}\n{\"t\":\"blk\",\"number\":12}\n"] {
            // Direct 10 -> 12 covers watch-channel coalescing: no intermediate
            // published head is required by the subscriber.
            let next = state(12, 1, &format!("{old_records}{append}"));
            let wire = head_update(&next, Some(&first));
            assert!(wire.len() < next.len() / 4);
            let data: Value = serde_json::from_str(&wire).unwrap();
            assert!(data["payload"].is_null());
            assert_eq!(data["delta"]["base_etag"], "head-10");
            assert_eq!(data["delta"]["append"], append);
            let rebuilt = format!("{}{}{}{}", data["delta"]["header"].as_str().unwrap(),
                old_records, data["delta"]["append"].as_str().unwrap(), data["delta"]["end"].as_str().unwrap());
            let expected: Value = serde_json::from_str(&next).unwrap();
            assert_eq!(rebuilt, expected["payload"].as_str().unwrap());
        }
    }

    #[test]
    fn live_delta_reconnect_rollover_and_reorg_replace_the_tail() {
        let records = format!("{{\"t\":\"blk\",\"data\":\"{}\"}}\n", "a".repeat(4096));
        let first = state(10, 1, &records);
        assert_eq!(head_update(&first, None), first);
        for next in [state(11, 11, &records), state(9, 1, ""), state(10, 1, &records.replace('a', "b"))] {
            assert_eq!(head_update(&next, Some(&first)), next);
        }
    }
}
