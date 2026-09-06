//! Block headers wake the ingest loop and bind subsequently acquired block data.
//! Coalescing is safe: ingestion resumes from its persisted cursor, including
//! after reconnects and reorgs. No per-block queue or timer-driven head fetch.

use anyhow::{bail, Context, Result};
use crate::rpc::BlockHeader;
use futures::{SinkExt, StreamExt};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::sync::watch;
use tokio::time::Instant;
use tokio_tungstenite::{connect_async, tungstenite::Message};

pub fn subscribe(url: String) -> watch::Receiver<Option<BlockHeader>> {
    let (tx, rx) = watch::channel(None);
    tokio::spawn(async move {
        while !tx.is_closed() {
            let result = connection(&url, &tx).await;
            if tx.is_closed() {
                return;
            }
            tracing::warn!(error = ?result.err(), "head subscription disconnected; reconnecting");
            // Backoff only after transport failure, never between new blocks.
            tokio::select! {
                _ = tx.closed() => return,
                _ = tokio::time::sleep(Duration::from_secs(1)) => {}
            }
        }
    });
    rx
}

async fn connection(url: &str, tx: &watch::Sender<Option<BlockHeader>>) -> Result<()> {
    let (mut socket, _) = tokio::time::timeout(Duration::from_secs(3), connect_async(url))
        .await
        .context("connect head subscription timeout")??;
    socket
        .send(Message::Text(
            json!({
                "jsonrpc": "2.0", "id": 1,
                "method": "starknet_subscribeNewHeads", "params": {"block_id": "latest"}
            })
            .to_string()
            .into(),
        ))
        .await?;
    let mut subscription = None;
    let mut deadline = Instant::now() + Duration::from_secs(2);
    loop {
        let frame = tokio::select! {
            _ = tx.closed() => return Ok(()),
            frame = tokio::time::timeout_at(deadline, socket.next()) =>
                frame.context(if subscription.is_none() { "head subscription ACK timeout" }
                    else { "head subscription header timeout" })?
                    .context("head subscription closed")??,
        };
        match frame {
            Message::Text(text) => {
                let message: Value = serde_json::from_str(&text)?;
                if message.get("id") == Some(&json!(1)) {
                    if message.get("error").is_some() {
                        bail!("head subscription rejected: {}", message["error"]);
                    }
                    let id = &message["result"];
                    if !id.is_string() && !id.is_u64() {
                        bail!("invalid head subscription id");
                    }
                    subscription = Some(id.clone());
                    deadline = Instant::now() + Duration::from_secs(10);
                    tracing::info!("head subscription connected");
                    // Always catch up, even if no fresh block follows the ACK.
                    tx.send_replace(None);
                } else if is_notification(&message, subscription.as_ref()) {
                    deadline = Instant::now() + Duration::from_secs(10);
                    let header = if message["method"] == "starknet_subscriptionNewHeads" {
                        let header: BlockHeader = serde_json::from_value(message["params"]["result"].clone())
                            .context("invalid announced header")?;
                        crate::rpc::parse_felt(&header.block_hash)?;
                        crate::rpc::parse_felt(header.new_root.as_deref().context("announced state root missing")?)?;
                        Some(header)
                    } else { None };
                    tx.send_replace(header);
                }
            }
            Message::Ping(bytes) => socket.send(Message::Pong(bytes)).await?,
            Message::Close(_) => bail!("head subscription closed"),
            _ => {}
        }
    }
}

fn is_notification(message: &Value, subscription: Option<&Value>) -> bool {
    subscription.is_some()
        && message["params"].get("subscription_id") == subscription
        && matches!(
            message["method"].as_str(),
            Some("starknet_subscriptionNewHeads" | "starknet_subscriptionReorg")
        )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn reconnect_ack_and_frames_wake_without_polling() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            for round in 0..2 {
                let (tcp, _) = listener.accept().await.unwrap();
                let mut socket = tokio_tungstenite::accept_async(tcp).await.unwrap();
                let request = socket.next().await.unwrap().unwrap().into_text().unwrap();
                assert_eq!(
                    serde_json::from_str::<Value>(&request).unwrap()["method"],
                    "starknet_subscribeNewHeads"
                );
                socket
                    .send(Message::Text(
                        json!({"id": 1, "result": "7"}).to_string().into(),
                    ))
                    .await
                    .unwrap();
                let event = if round == 0 {
                    json!({"method": "starknet_subscriptionNewHeads", "params": {"subscription_id": "7", "result": {"block_number": 42, "block_hash":"0x42", "parent_hash":"0x41", "new_root":"0x123", "timestamp":42}}})
                } else {
                    json!({"method": "starknet_subscriptionReorg", "params": {"subscription_id": "7"}})
                };
                socket
                    .send(Message::Text(event.to_string().into()))
                    .await
                    .unwrap();
                socket.close(None).await.unwrap();
            }
        });
        let (tx, mut rx) = watch::channel(None);
        // Drive two connections without a backoff clock: the reconnect ACK
        // must cause catch-up even if no head was produced while disconnected.
        for expected in [Some(42), None] {
            assert!(connection(&url, &tx).await.is_err());
            assert!(rx.has_changed().unwrap());
            assert_eq!(rx.borrow_and_update().as_ref().map(|h| h.block_number), expected);
        }
        server.await.unwrap();
    }

    #[tokio::test]
    async fn an_open_socket_without_subscription_ack_does_not_stall_for_a_minute() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("ws://{}", listener.local_addr().unwrap());
        let server = tokio::spawn(async move {
            let (tcp, _) = listener.accept().await.unwrap();
            let mut socket = tokio_tungstenite::accept_async(tcp).await.unwrap();
            socket.next().await.unwrap().unwrap();
            // Answering transport pings cannot substitute for a subscription ACK.
            for _ in 0..20 {
                if socket.send(Message::Ping(vec![1].into())).await.is_err() { break; }
                tokio::time::sleep(Duration::from_millis(150)).await;
            }
        });
        let (tx, _rx) = watch::channel(None);
        let result = tokio::time::timeout(Duration::from_secs(4), connection(&url, &tx)).await;
        server.abort();
        assert!(format!("{:#}", result.expect("missing ACK must not wait for header idle timeout").unwrap_err())
            .contains("ACK timeout"));
    }

    #[test]
    fn only_our_heads_and_reorgs_wake_ingestion() {
        let id = json!("7");
        for method in [
            "starknet_subscriptionNewHeads",
            "starknet_subscriptionReorg",
        ] {
            let event = json!({"method": method, "params": {"subscription_id": "7"}});
            assert!(is_notification(&event, Some(&id)));
            assert!(!is_notification(&event, Some(&json!("8"))));
            assert!(!is_notification(&event, None));
        }
        assert!(!is_notification(
            &json!({"method": "other", "params": {"subscription_id": "7"}}),
            Some(&id)
        ));
    }
}
