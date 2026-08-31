//! Raw SSE subscriber for the acceptance harness (consumer-path.md §A2).
//!
//! The stream is drained into a shared buffer by a background task so several
//! subscribers can be compared against each other while the fixture chain
//! moves underneath them. The buffer holds the DE-CHUNKED body — i.e. exactly
//! the bytes §2.2 specifies — so framing assertions (`retry:`, the 2 KB
//! padding comment, `: ka` keepalives) are made against the real wire form
//! rather than against a parsed abstraction.

use anyhow::{Context, Result};
use futures::StreamExt;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub id: Option<String>,
    pub name: Option<String>,
    /// Concatenated `data:` field values (SSE joins multiple with `\n`).
    pub data: String,
}

impl SseEvent {
    pub fn json(&self) -> serde_json::Value {
        serde_json::from_str(&self.data).unwrap_or(serde_json::Value::Null)
    }
}

pub struct SseStream {
    pub status: u16,
    headers: BTreeMap<String, String>,
    body: Arc<Mutex<Vec<u8>>>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for SseStream {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl SseStream {
    /// Connect and start draining. Returns as soon as the response head is in.
    pub async fn connect(url: &str) -> Result<Self> {
        Self::connect_with(url, &[]).await
    }

    /// Connect carrying extra request headers — used to prove that
    /// `Last-Event-ID` is DELIBERATELY IGNORED (§2.3).
    pub async fn connect_with(url: &str, headers: &[(&str, &str)]) -> Result<Self> {
        let mut req = reqwest::Client::new()
            .get(url)
            .header("accept", "text/event-stream");
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let resp = req
            .send()
            .await
            .with_context(|| format!("GET {url}"))?;
        let status = resp.status().as_u16();
        let headers = resp
            .headers()
            .iter()
            .map(|(k, v)| {
                (
                    k.as_str().to_ascii_lowercase(),
                    v.to_str().unwrap_or_default().to_owned(),
                )
            })
            .collect();
        let body = Arc::new(Mutex::new(Vec::new()));
        let sink = body.clone();
        let task = tokio::spawn(async move {
            let mut stream = resp.bytes_stream();
            while let Some(chunk) = stream.next().await {
                match chunk {
                    Ok(bytes) => sink.lock().expect("sse buffer").extend_from_slice(&bytes),
                    Err(_) => break,
                }
            }
        });
        Ok(Self {
            status,
            headers,
            body,
            task,
        })
    }

    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers.get(&name.to_ascii_lowercase()).map(String::as_str)
    }

    pub fn bytes(&self) -> Vec<u8> {
        self.body.lock().expect("sse buffer").clone()
    }

    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes()).into_owned()
    }

    /// Wait until `pred` holds over the accumulated body, or `timeout` passes.
    pub async fn wait_for(&self, timeout: Duration, pred: impl Fn(&str) -> bool) -> bool {
        let deadline = std::time::Instant::now() + timeout;
        loop {
            if pred(&self.text()) {
                return true;
            }
            if std::time::Instant::now() >= deadline {
                return false;
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    /// Wait until at least `n` events (not comments) have arrived.
    pub async fn wait_for_events(&self, timeout: Duration, n: usize) -> bool {
        self.wait_for(timeout, |t| parse_events(t).len() >= n).await
    }

    pub fn events(&self) -> Vec<SseEvent> {
        parse_events(&self.text())
    }

    /// Comment lines (`:` prefixed), in order, without the leading colon.
    pub fn comments(&self) -> Vec<String> {
        self.text()
            .lines()
            .filter_map(|l| l.strip_prefix(':').map(str::to_owned))
            .collect()
    }
}

/// Parse the SSE grammar: blocks separated by a blank line; `field: value`
/// lines; `:` comment lines. Only complete blocks (terminated by a blank
/// line) are returned, so a half-arrived event never looks like a whole one.
pub fn parse_events(text: &str) -> Vec<SseEvent> {
    let mut blocks: Vec<&str> = text.split("\n\n").collect();
    // The trailing piece is only a complete block if the text ended on a
    // blank line; otherwise it is a half-arrived event and must not count.
    if !text.ends_with("\n\n") {
        blocks.pop();
    }
    let mut out = Vec::new();
    for block in blocks {
        let mut id = None;
        let mut name = None;
        let mut data: Vec<String> = Vec::new();
        let mut saw_field = false;
        for line in block.lines() {
            if line.starts_with(':') || line.is_empty() {
                continue;
            }
            let (field, value) = match line.split_once(':') {
                Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
                None => (line, ""),
            };
            saw_field = true;
            match field {
                "id" => id = Some(value.to_owned()),
                "event" => name = Some(value.to_owned()),
                "data" => data.push(value.to_owned()),
                _ => {}
            }
        }
        if saw_field && name.is_some() {
            out.push(SseEvent {
                id,
                name,
                data: data.join("\n"),
            });
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Instrument self-test: a chunked `text/event-stream` written by hand is
    /// read back de-chunked, with headers, incremental arrival and framing
    /// intact. Without this, a red SSE leg could be the reader's fault.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn reader_sees_headers_framing_and_late_events() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let (mut sock, _) = listener.accept().await.unwrap();
            let mut req = Vec::new();
            let mut b = [0u8; 1];
            while !req.windows(4).any(|w| w == b"\r\n\r\n") {
                if sock.read(&mut b).await.unwrap_or(0) == 0 {
                    return;
                }
                req.push(b[0]);
            }
            let head = "HTTP/1.1 200 OK\r\ncontent-type: text/event-stream\r\n\
                        cache-control: no-cache\r\nx-accel-buffering: no\r\n\
                        transfer-encoding: chunked\r\n\r\n";
            sock.write_all(head.as_bytes()).await.unwrap();
            let chunk = |s: &str| format!("{:x}\r\n{s}\r\n", s.len());
            for part in [
                ":pad\n\n".to_owned(),
                "retry: 15000\n\n".to_owned(),
                "event: hello\nid: 1\ndata: {\"v\":1}\n\n".to_owned(),
            ] {
                sock.write_all(chunk(&part).as_bytes()).await.unwrap();
                sock.flush().await.unwrap();
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
            sock.write_all(chunk("event: head\nid: 2\ndata: {\"head\":7}\n\n").as_bytes())
                .await
                .unwrap();
            sock.flush().await.unwrap();
            tokio::time::sleep(Duration::from_secs(2)).await;
        });

        let s = SseStream::connect(&format!("http://{addr}/feed/live"))
            .await
            .expect("connect");
        assert_eq!(s.status, 200);
        assert_eq!(s.header("cache-control"), Some("no-cache"));
        assert_eq!(s.header("x-accel-buffering"), Some("no"));
        assert!(s.wait_for_events(Duration::from_secs(5), 2).await, "{}", s.text());
        let events = s.events();
        assert_eq!(events[0].name.as_deref(), Some("hello"));
        assert_eq!(events[1].json()["head"], serde_json::json!(7));
        assert!(s.text().starts_with(":pad"), "raw framing is preserved");
        assert!(s.text().contains("retry: 15000"));
        assert_eq!(s.comments(), vec!["pad".to_owned()]);
    }

    #[test]
    fn only_complete_blocks_parse() {
        let text = ":pad\n\nretry: 15000\n\nevent: hello\nid: 1\ndata: {\"v\":1}\n\nevent: head\nid: 2\ndata: {\"he";
        let evs = parse_events(text);
        assert_eq!(evs.len(), 1, "the half-arrived head event must not count");
        assert_eq!(evs[0].name.as_deref(), Some("hello"));
        assert_eq!(evs[0].id.as_deref(), Some("1"));
        assert_eq!(evs[0].json()["v"], serde_json::json!(1));
    }
}
