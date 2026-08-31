//! A byte-level recording proxy that can carry a long-lived response.
//!
//! `RecordingProxy` buffers whole responses, which is correct for static feed
//! files and impossible for `/feed/live`: an SSE response never ends, so a
//! buffering proxy would hang forever. This one splices bytes, so it can sit
//! in front of the stream while still capturing every request head for the
//! address-blindness assertions — and it can inject the two failures §2.5
//! requires a client to survive: a stream killed mid-flight, and a route that
//! answers 404 (a plain static-file mirror, which has no stream at all).

use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LivePolicy {
    /// Forward `/feed/live` transparently.
    Pass,
    /// Answer `/feed/live` with 404 (static-file mirror posture).
    NotFound,
    /// Forward, then drop the connection — a stream killed mid-flight.
    Kill,
}

#[derive(Debug, Clone)]
pub struct CapturedHead {
    pub method: String,
    pub uri: String,
    /// The verbatim request head, request line and headers included.
    pub bytes: Vec<u8>,
}

#[derive(Clone)]
pub struct TcpProxy {
    upstream: SocketAddr,
    policy: Arc<Mutex<LivePolicy>>,
    heads: Arc<Mutex<Vec<CapturedHead>>>,
    live_opens: Arc<AtomicUsize>,
}

impl TcpProxy {
    pub fn new(upstream: SocketAddr) -> Self {
        Self {
            upstream,
            policy: Arc::new(Mutex::new(LivePolicy::Pass)),
            heads: Arc::new(Mutex::new(Vec::new())),
            live_opens: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub fn set_live_policy(&self, policy: LivePolicy) {
        *self.policy.lock().expect("policy") = policy;
    }

    pub fn live_policy(&self) -> LivePolicy {
        *self.policy.lock().expect("policy")
    }

    /// Every request head seen so far (not drained).
    pub fn heads(&self) -> Vec<CapturedHead> {
        self.heads.lock().expect("heads").clone()
    }

    /// How many times a client opened `/feed/live`.
    pub fn live_opens(&self) -> usize {
        self.live_opens.load(Ordering::SeqCst)
    }

    pub async fn serve(&self) -> SocketAddr {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let this = self.clone();
        tokio::spawn(async move {
            loop {
                let Ok((client, _)) = listener.accept().await else {
                    return;
                };
                let this = this.clone();
                tokio::spawn(async move {
                    let _ = this.handle(client).await;
                });
            }
        });
        addr
    }

    async fn handle(&self, mut client: TcpStream) -> std::io::Result<()> {
        let mut head = Vec::new();
        let mut byte = [0u8; 1];
        // One request per connection: `Connection: close` is forced upstream,
        // so a plain copy terminates on a normal response and clients that
        // want keep-alive simply open another connection.
        while !head.windows(4).any(|w| w == b"\r\n\r\n") {
            match client.read(&mut byte).await {
                Ok(0) => return Ok(()),
                Ok(_) => head.push(byte[0]),
                Err(e) => return Err(e),
            }
            if head.len() > 64 * 1024 {
                return Ok(());
            }
        }
        let text = String::from_utf8_lossy(&head).into_owned();
        let mut parts = text.split_whitespace();
        let method = parts.next().unwrap_or_default().to_owned();
        let uri = parts.next().unwrap_or_default().to_owned();
        let is_live = uri.split('?').next() == Some("/feed/live");
        self.heads.lock().expect("heads").push(CapturedHead {
            method,
            uri,
            bytes: head.clone(),
        });
        if is_live {
            self.live_opens.fetch_add(1, Ordering::SeqCst);
            if self.live_policy() == LivePolicy::NotFound {
                let body = b"not found";
                let resp = format!(
                    "HTTP/1.1 404 Not Found\r\ncontent-type: text/plain\r\ncontent-length: {}\r\nconnection: close\r\n\r\n",
                    body.len()
                );
                client.write_all(resp.as_bytes()).await?;
                client.write_all(body).await?;
                return Ok(());
            }
        }

        let mut up = TcpStream::connect(self.upstream).await?;
        up.write_all(&force_close(&head)).await?;
        let mut buf = vec![0u8; 16 * 1024];
        loop {
            if is_live && self.live_policy() != LivePolicy::Pass {
                // dropping both sockets is the mid-flight kill
                return Ok(());
            }
            let n = match tokio::time::timeout(Duration::from_millis(200), up.read(&mut buf)).await {
                Ok(Ok(0)) => return Ok(()),
                Ok(Ok(n)) => n,
                Ok(Err(e)) => return Err(e),
                Err(_) => continue, // idle: re-check the policy
            };
            if client.write_all(&buf[..n]).await.is_err() {
                return Ok(());
            }
        }
    }
}

/// Replace any `Connection:` header with `close` (and add one if absent), so
/// a spliced response is terminated by EOF and needs no framing knowledge.
fn force_close(head: &[u8]) -> Vec<u8> {
    let text = String::from_utf8_lossy(head);
    let mut out = String::new();
    for line in text.split("\r\n") {
        if line.is_empty() {
            break;
        }
        if line.to_ascii_lowercase().starts_with("connection:") {
            continue;
        }
        out.push_str(line);
        out.push_str("\r\n");
    }
    out.push_str("connection: close\r\n\r\n");
    out.into_bytes()
}
