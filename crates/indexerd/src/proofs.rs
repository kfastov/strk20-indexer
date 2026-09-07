//! Shared public proof acquisition. One task per block, independent of ingest
//! and of subscriber lifetimes; completed proofs are also pushed through SSE.
use crate::{
    live::LiveHub,
    rpc::{BlockRef, RpcClient},
};
use serde_json::json;
use starknet_types_core::felt::Felt;
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, Weak,
    },
    time::Duration,
};
use tokio::sync::{watch, Semaphore};

type Result = std::result::Result<String, String>;
type Pending = watch::Receiver<Option<Result>>;

pub struct Proofs {
    rpc: Arc<RpcClient>,
    pool: Felt,
    live: Weak<LiveHub>,
    entries: Mutex<BTreeMap<u64, Pending>>,
    generation: AtomicU64,
    slots: Arc<Semaphore>,
}

impl Proofs {
    pub fn new(rpc: Arc<RpcClient>, pool: Felt, live: Weak<LiveHub>) -> Arc<Self> {
        Arc::new(Self {
            rpc,
            pool,
            live,
            entries: Mutex::new(BTreeMap::new()),
            generation: AtomicU64::new(0),
            slots: Arc::new(Semaphore::new(4)),
        })
    }

    pub fn request(self: &Arc<Self>, block: u64) -> Option<Pending> {
        let mut entries = self.entries.lock().expect("proof entries");
        if let Some(entry) = entries.get(&block) {
            return Some(entry.clone());
        }
        // Both upstream concurrency and retained memory are bounded globally,
        // rather than once per subscriber. HTTP demand cannot flood the RPC.
        let permit = self.slots.clone().try_acquire_owned().ok()?;
        while entries.len() >= 128 {
            let oldest = entries
                .iter()
                .find(|(_, entry)| entry.borrow().is_some())
                .map(|(b, _)| *b)?;
            entries.remove(&oldest);
        }
        let (tx, rx) = watch::channel(None);
        entries.insert(block, rx.clone());
        let this = self.clone();
        let own = rx.clone();
        let generation = self.generation.load(Ordering::SeqCst);
        tokio::spawn(async move {
            let _permit = permit;
            let result = tokio::time::timeout(Duration::from_secs(20), async {
                // New-head notification can precede availability on the proof
                // backend. Absorb this once for all clients, never retry a bad
                // proof on a client's behalf.
                let mut last = String::new();
                for attempt in 0..3 {
                    match this
                        .rpc
                        .get_storage_proof(BlockRef::Number(block), &this.pool, &[])
                        .await
                    {
                        Ok((_, proof)) => {
                            let hash = proof["global_roots"]["block_hash"]
                                .as_str()
                                .ok_or_else(|| "proof has no block hash".to_owned())?;
                            crate::rpc::parse_felt(hash).map_err(|e| e.to_string())?;
                            let text = json!({"block": block, "block_hash": hash, "proof": proof})
                                .to_string();
                            if text.len() > 128 * 1024 {
                                return Err("proof exceeds size limit".into());
                            }
                            return Ok(text);
                        }
                        Err(error) => {
                            last = format!("{error:#}");
                            if !last.contains("\"code\":24") {
                                break;
                            }
                            if attempt < 2 {
                                tokio::time::sleep(Duration::from_millis(200 << attempt)).await;
                            }
                        }
                    }
                }
                Err(last)
            })
            .await
            .unwrap_or_else(|_| Err("proof acquisition timed out".into()));
            if this.generation.load(Ordering::SeqCst) != generation {
                tx.send_replace(Some(Err(
                    "chain reorganized during proof acquisition".into()
                )));
                return;
            }
            if result.is_err() {
                let mut entries = this.entries.lock().expect("proof entries");
                if entries
                    .get(&block)
                    .is_some_and(|entry| entry.same_channel(&own))
                {
                    entries.remove(&block);
                }
            }
            if let Ok(text) = &result {
                if let Some(live) = this.live.upgrade() {
                    live.publish_proof(text.clone());
                }
            }
            tx.send_replace(Some(result));
        });
        Some(rx)
    }

    pub fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
        self.entries.lock().expect("proof entries").clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{routing::post, Json, Router};
    use std::sync::atomic::AtomicUsize;

    #[tokio::test]
    async fn concurrent_readers_share_one_request_and_reorg_discards_inflight_result() {
        let calls = Arc::new(AtomicUsize::new(0));
        let entered = Arc::new(tokio::sync::Notify::new());
        let release = Arc::new(Semaphore::new(0));
        let app = Router::new().route(
            "/",
            post({
                let (calls, entered, release) = (calls.clone(), entered.clone(), release.clone());
                move || {
                    let (calls, entered, release) =
                        (calls.clone(), entered.clone(), release.clone());
                    async move {
                        calls.fetch_add(1, Ordering::SeqCst);
                        entered.notify_one();
                        release.acquire().await.unwrap().forget();
                        Json(json!({"jsonrpc":"2.0", "id":1, "result":{
                            "contracts_proof":{"contract_leaves_data":[]},
                            "global_roots":{"block_hash":"0x123"}
                        }}))
                    }
                }
            }),
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            axum::serve(listener, app).await.unwrap();
        });
        let proofs = Proofs::new(Arc::new(RpcClient::new(url, None)), Felt::ONE, Weak::new());
        let mut first = proofs.request(10).unwrap();
        let second = proofs.request(10).unwrap();
        tokio::time::timeout(Duration::from_secs(2), entered.notified())
            .await
            .unwrap();
        assert_eq!(calls.load(Ordering::SeqCst), 1);
        release.add_permits(1);
        first.changed().await.unwrap();
        assert!(second.borrow().as_ref().unwrap().is_ok());
        assert!(proofs
            .request(10)
            .unwrap()
            .borrow()
            .as_ref()
            .unwrap()
            .is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 1);

        let mut old = proofs.request(11).unwrap();
        entered.notified().await;
        proofs.invalidate();
        release.add_permits(1);
        old.changed().await.unwrap();
        assert!(old
            .borrow()
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap_err()
            .contains("reorganized"));
        let mut new = proofs.request(11).unwrap();
        entered.notified().await;
        release.add_permits(1);
        new.changed().await.unwrap();
        assert!(new.borrow().as_ref().unwrap().is_ok());
        assert_eq!(calls.load(Ordering::SeqCst), 3);
        server.abort();
    }
}
