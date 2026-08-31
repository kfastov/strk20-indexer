//! `FeedEvents` — SSE consumption (consumer-path.md §A2, §2.5).
//!
//! Deliberately a SEPARATE trait from `FeedTransport`. The privacy seam is
//! compile-locked: no `FeedTransport` method may ever accept a user-derived
//! value, and a compile-fail suite pins that signature. Adding a streaming
//! method there would change the locked surface for a capability that has
//! nothing to do with fetching verified bytes, so notification lives here and
//! the seam stays byte-stable.
//!
//! The stream is a NOTIFICATION plane only. Nothing here is trusted, parsed
//! into state, or applied: an event pokes the client into running the same
//! verified fetch it would have run on its poll cadence. A lost, duplicated,
//! reordered or forged event therefore costs latency at worst, which is why
//! polling remains the reference semantics.

use anyhow::{Context, Result};
use async_trait::async_trait;
use futures::stream::{BoxStream, StreamExt};

/// One received event. The payload is carried verbatim and NEVER folded into
/// the mirror.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FeedNotice {
    Hello(String),
    Head(String),
    Epoch(String),
    Snapshot(String),
    Status(String),
    Other(String),
}

impl FeedNotice {
    /// Does this event mean "there may be new bytes to fetch"?
    pub fn is_poke(&self) -> bool {
        matches!(
            self,
            FeedNotice::Head(_) | FeedNotice::Epoch(_) | FeedNotice::Snapshot(_)
        )
    }
}

/// The feed publishes no stream at all — a plain static-file mirror. Not an
/// error condition: §2.5 makes it a fully supported deployment that degrades
/// the session to polling with nothing surfaced.
#[derive(Debug)]
pub struct LiveUnsupported;

impl std::fmt::Display for LiveUnsupported {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("this feed publishes no /feed/live stream")
    }
}

impl std::error::Error for LiveUnsupported {}

#[async_trait]
pub trait FeedEvents: Send + Sync {
    async fn subscribe(&self) -> Result<BoxStream<'static, FeedNotice>>;
}

/// SSE over the same `/feed` base URL. The subscription is parameterless, so
/// two clients with different keys and addresses emit byte-identical request
/// heads (§2.6).
pub struct HttpEvents {
    url: String,
    http: reqwest::Client,
}

impl HttpEvents {
    pub fn new(base: &str) -> Self {
        Self {
            url: format!("{}/live", base.trim_end_matches('/')),
            http: reqwest::Client::builder()
                .user_agent(concat!("strk20-sync/", env!("CARGO_PKG_VERSION")))
                .build()
                .expect("reqwest client"),
        }
    }
}

#[async_trait]
impl FeedEvents for HttpEvents {
    async fn subscribe(&self) -> Result<BoxStream<'static, FeedNotice>> {
        let resp = self
            .http
            .get(&self.url)
            .header(reqwest::header::ACCEPT, "text/event-stream")
            .send()
            .await
            .with_context(|| format!("GET {}", self.url))?;
        let status = resp.status();
        if status == reqwest::StatusCode::NOT_FOUND
            || status == reqwest::StatusCode::METHOD_NOT_ALLOWED
        {
            return Err(LiveUnsupported.into());
        }
        if !status.is_success() {
            anyhow::bail!("GET {}: HTTP {status}", self.url);
        }
        // Raw BYTES, not lossy text: TCP chunk boundaries fall anywhere, and
        // decoding each chunk on its own turns a multi-byte character split
        // across two chunks into U+FFFD plus a stray continuation. Every
        // payload we emit today happens to be ASCII, but the framing layer must
        // not depend on that.
        let mut buf: Vec<u8> = Vec::new();
        let stream = resp.bytes_stream().flat_map(move |chunk| {
            let events = match chunk {
                Ok(bytes) => {
                    buf.extend_from_slice(&bytes);
                    drain_events(&mut buf)
                }
                Err(_) => Vec::new(),
            };
            futures::stream::iter(events)
        });
        Ok(stream.boxed())
    }
}

/// Pull every COMPLETE event block out of `buf`, leaving any partial tail. A
/// half-arrived event must never look like a whole one.
fn drain_events(buf: &mut Vec<u8>) -> Vec<FeedNotice> {
    let mut out = Vec::new();
    while let Some(end) = buf.windows(2).position(|w| w == b"\n\n") {
        let block: Vec<u8> = buf.drain(..end + 2).collect();
        // A whole block is reassembled before decoding, so no character can be
        // split across the boundary. An event that is genuinely not utf-8 is
        // dropped rather than mangled.
        let Ok(block) = std::str::from_utf8(&block) else {
            continue;
        };
        let mut name = None;
        let mut data: Vec<&str> = Vec::new();
        for line in block.lines() {
            if line.starts_with(':') || line.is_empty() {
                continue;
            }
            let (field, value) = match line.split_once(':') {
                Some((f, v)) => (f, v.strip_prefix(' ').unwrap_or(v)),
                None => (line, ""),
            };
            match field {
                "event" => name = Some(value.to_owned()),
                "data" => data.push(value),
                _ => {}
            }
        }
        let Some(name) = name else { continue };
        let payload = data.join("\n");
        out.push(match name.as_str() {
            "hello" => FeedNotice::Hello(payload),
            "head" => FeedNotice::Head(payload),
            "epoch" => FeedNotice::Epoch(payload),
            "snapshot" => FeedNotice::Snapshot(payload),
            "status" => FeedNotice::Status(payload),
            _ => FeedNotice::Other(payload),
        });
    }
    out
}

/// A stream source for `--feed`, when there can be one. A local mirror
/// directory has no stream and never will; that is polling-only by nature, not
/// a degraded deployment.
pub fn events_for(feed: &str) -> Option<Box<dyn FeedEvents>> {
    (feed.starts_with("http://") || feed.starts_with("https://"))
        .then(|| Box::new(HttpEvents::new(feed)) as Box<dyn FeedEvents>)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_complete_blocks_are_yielded() {
        let mut buf: Vec<u8> = Vec::from(
            &b":pad\n\nretry: 15000\n\nevent: hello\ndata: {\"v\":1}\n\nevent: head\ndata: {\"he"[..],
        );
        let events = drain_events(&mut buf);
        assert_eq!(events, vec![FeedNotice::Hello("{\"v\":1}".to_owned())]);
        buf.extend_from_slice(b"ad\":7}\n\n");
        assert_eq!(
            drain_events(&mut buf),
            vec![FeedNotice::Head("{\"head\":7}".to_owned())]
        );
        assert!(buf.is_empty());
    }

    /// A multi-byte character split across two arrivals must be REASSEMBLED,
    /// not decoded twice into a replacement character and a stray
    /// continuation byte.
    #[test]
    fn a_character_split_across_chunk_boundaries_survives() {
        let payload = "{\"module\":\"strk20/ünïcødé\"}";
        let whole = format!("event: hello\ndata: {payload}\n\n").into_bytes();
        // Split inside the two-byte 'ü'.
        let cut = whole.iter().position(|b| *b == 0xc3).expect("multi-byte lead") + 1;
        let mut buf: Vec<u8> = whole[..cut].to_vec();
        assert!(drain_events(&mut buf).is_empty(), "half an event is not an event");
        buf.extend_from_slice(&whole[cut..]);
        assert_eq!(
            drain_events(&mut buf),
            vec![FeedNotice::Hello(payload.to_owned())]
        );
    }
}
