//! HTTP server (spec §6): feed static files (§6.1), ops (§6.2), raw
//! targeted endpoints behind --enable-raw (§6.3), compat behind
//! --enable-compat (§6.4). No feed route takes any user-derived parameter —
//! that absence is the privacy mechanism.

use crate::config::ChainConfig;
use crate::db::Db;
use axum::extract::{Path as AxPath, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::json;
use starknet_types_core::felt::Felt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

pub const PRIVACY_HEADER: &str = "x-strk20-privacy";
pub const PRIVACY_VALUE: &str = "targeted-mode-leaks-queried-slots";

#[derive(Clone)]
pub struct AppState {
    pub feed_dir: PathBuf,
    pub db: Arc<Mutex<Db>>,
    pub cfg: ChainConfig,
}

pub fn build_router(
    state: AppState,
    enable_raw: bool,
    compat: Option<crate::compat::CompatState>,
) -> Router {
    let mut router = Router::new()
        .route("/feed/genesis.json", get(feed_genesis))
        .route("/feed/manifest.json", get(feed_manifest))
        .route("/feed/head.ndjson", get(feed_head))
        .route("/feed/epochs/{name}", get(feed_epoch_file))
        .route("/feed/snapshots/{name}", get(feed_snapshot_file))
        .route("/health", get(health))
        .route("/v1/stats", get(stats))
        .route("/metrics", get(metrics))
        .with_state(state.clone());
    if enable_raw {
        let raw = Router::new()
            .route("/v1/raw/read_slots", post(raw_read_slots))
            .route("/v1/raw/events", get(raw_events))
            .with_state(state);
        router = router.merge(raw);
    }
    if let Some(compat_state) = compat {
        router = router.merge(crate::compat::router(compat_state));
    }
    router
}

fn with_db<T>(state: &AppState, f: impl FnOnce(&Db) -> anyhow::Result<T>) -> Result<T, Response> {
    let db = state.db.lock().expect("db mutex");
    f(&db).map_err(|e| {
        tracing::warn!(error = %e, "db error");
        (StatusCode::SERVICE_UNAVAILABLE, "storage error").into_response()
    })
}

async fn serve_file(path: PathBuf, cache: &'static str, extra: Option<(&'static str, String)>) -> Response {
    match tokio::fs::read(&path).await {
        Ok(bytes) => {
            let mut headers = HeaderMap::new();
            headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
            let ct = if path.extension().map(|e| e == "zst").unwrap_or(false) {
                "application/zstd"
            } else if path.extension().map(|e| e == "json").unwrap_or(false) {
                "application/json"
            } else {
                "application/x-ndjson"
            };
            headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(ct));
            if let Some((name, value)) = extra {
                if let Ok(v) = HeaderValue::from_str(&value) {
                    headers.insert(name, v);
                }
            }
            (headers, bytes).into_response()
        }
        Err(_) => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

async fn feed_genesis(State(s): State<AppState>) -> Response {
    serve_file(
        s.feed_dir.join("genesis.json"),
        "public, max-age=31536000, immutable",
        None,
    )
    .await
}

async fn feed_manifest(State(s): State<AppState>) -> Response {
    serve_file(s.feed_dir.join("manifest.json"), "public, max-age=30", None).await
}

async fn feed_head(State(s): State<AppState>, headers: HeaderMap) -> Response {
    let path = s.feed_dir.join("head.ndjson");
    let bytes = match tokio::fs::read(&path).await {
        Ok(b) => b,
        Err(_) => return (StatusCode::NOT_FOUND, "not found").into_response(),
    };
    let etag = format!("\"{}\"", hex::encode(strk20_feed::payload_sha256(&bytes)));
    if let Some(inm) = headers.get(header::IF_NONE_MATCH).and_then(|v| v.to_str().ok()) {
        if inm == etag {
            return StatusCode::NOT_MODIFIED.into_response();
        }
    }
    let mut h = HeaderMap::new();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    h.insert(header::ETAG, HeaderValue::from_str(&etag).unwrap());
    h.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/x-ndjson"),
    );
    (h, bytes).into_response()
}

fn valid_epoch_name(name: &str) -> Option<(u64, bool)> {
    // {idx:08}.strk20e.zst or {idx:08}.anchor.json
    let (idx_part, rest) = name.split_once('.')?;
    if idx_part.len() != 8 || !idx_part.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let idx: u64 = idx_part.parse().ok()?;
    match rest {
        "strk20e.zst" => Some((idx, true)),
        "anchor.json" => Some((idx, false)),
        _ => None,
    }
}

async fn feed_epoch_file(State(s): State<AppState>, AxPath(name): AxPath<String>) -> Response {
    let Some((idx, is_epoch)) = valid_epoch_name(&name) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    let extra = if is_epoch {
        // content hash from the epochs table, exposed for mirrors/clients
        let db = s.db.lock().expect("db mutex");
        db.epoch_rows()
            .ok()
            .and_then(|rows| rows.into_iter().find(|r| r.idx == idx))
            .map(|r| ("x-content-sha256-raw", hex::encode(r.content_hash)))
    } else {
        None
    };
    serve_file(
        s.feed_dir.join("epochs").join(&name),
        "public, max-age=31536000, immutable",
        extra,
    )
    .await
}

async fn feed_snapshot_file(State(s): State<AppState>, AxPath(name): AxPath<String>) -> Response {
    if name.contains('/') || name.contains("..") {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    serve_file(
        s.feed_dir.join("snapshots").join(&name),
        "public, max-age=31536000, immutable",
        None,
    )
    .await
}

async fn health(State(s): State<AppState>) -> Response {
    let result = with_db(&s, |db| {
        let head_number: Option<u64> = db.meta_get("head_number")?.and_then(|x| x.parse().ok());
        let head_hash = db.meta_get("head_hash")?;
        let l1: Option<u64> = db.meta_get("l1_accepted_number")?.and_then(|x| x.parse().ok());
        let decode_state = db.meta_get("decode_state")?.unwrap_or_else(|| "ok".into());
        let verify_root_failed = db
            .meta_get("verify_root_failed")?
            .map(|v| v == "1")
            .unwrap_or(false);
        let latest_epoch = db.last_epoch()?.map(|(i, _, _)| i);
        let ts = head_number
            .and_then(|n| db.block(n).ok().flatten())
            .map(|b| b.timestamp);
        let class = head_number
            .and_then(|n| db.class_as_of(n).ok().flatten())
            .map(|c| strk20_feed::felt_hex(&c));
        Ok(json!({
            "status": if head_number.is_none() { "UNHEALTHY" }
                      else if decode_state == "degraded" || verify_root_failed { "DEGRADED" }
                      else { "OK" },
            "head": head_number.map(|n| json!({
                "number": n, "hash": head_hash, "timestamp": ts
            })),
            "l1_accepted": l1,
            "lag_secs": 0,
            "latest_epoch": latest_epoch,
            "class_hash": class,
            "decode_state": decode_state,
            "verify_root_failed": verify_root_failed,
        }))
    });
    match result {
        Ok(body) => {
            let unhealthy = body["status"] == "UNHEALTHY";
            let code = if unhealthy {
                StatusCode::SERVICE_UNAVAILABLE
            } else {
                StatusCode::OK
            };
            (code, Json(body)).into_response()
        }
        Err(resp) => resp,
    }
}

async fn stats(State(s): State<AppState>) -> Response {
    match with_db(&s, crate::stats::compute) {
        Ok(body) => Json(body).into_response(),
        Err(resp) => resp,
    }
}

async fn metrics(State(s): State<AppState>) -> Response {
    let result = with_db(&s, |db| {
        let head: u64 = db
            .meta_get("head_number")?
            .and_then(|x| x.parse().ok())
            .unwrap_or(0);
        let l1: u64 = db
            .meta_get("l1_accepted_number")?
            .and_then(|x| x.parse().ok())
            .unwrap_or(0);
        let epochs = db.epoch_rows()?.len();
        let degraded = db.meta_get("decode_state")?.as_deref() == Some("degraded");
        Ok(format!(
            "# TYPE strk20_head_block gauge\nstrk20_head_block {head}\n\
             # TYPE strk20_l1_accepted_block gauge\nstrk20_l1_accepted_block {l1}\n\
             # TYPE strk20_epochs_cut gauge\nstrk20_epochs_cut {epochs}\n\
             # TYPE strk20_decode_degraded gauge\nstrk20_decode_degraded {}\n",
            degraded as u8
        ))
    });
    match result {
        Ok(text) => (
            [(header::CONTENT_TYPE, "text/plain; version=0.0.4")],
            text,
        )
            .into_response(),
        Err(resp) => resp,
    }
}

// ------------------------------------------------------------------- raw

#[derive(Deserialize)]
struct ReadSlotsRequest {
    block: serde_json::Value, // "head" | number
    slots: Vec<String>,
}

fn privacy_labeled(mut resp: Response) -> Response {
    resp.headers_mut()
        .insert(PRIVACY_HEADER, HeaderValue::from_static(PRIVACY_VALUE));
    resp
}

async fn raw_read_slots(
    State(s): State<AppState>,
    body: axum::body::Bytes,
) -> Response {
    // Manual parse: an axum Json rejection would echo body fragments and
    // skip the privacy label (review finding).
    let req: ReadSlotsRequest = match serde_json::from_slice(&body) {
        Ok(r) => r,
        Err(_) => {
            return privacy_labeled(
                (StatusCode::BAD_REQUEST, "invalid request body").into_response(),
            )
        }
    };
    if req.slots.len() > 1000 {
        return privacy_labeled(
            (StatusCode::BAD_REQUEST, "at most 1000 slots").into_response(),
        );
    }
    let result = with_db(&s, |db| {
        let head: u64 = db
            .meta_get("head_number")?
            .and_then(|x| x.parse().ok())
            .unwrap_or(0);
        let block = match &req.block {
            v if v.as_str() == Some("head") => head,
            v => v.as_u64().unwrap_or(head).min(head),
        };
        let head_hash = db.meta_get("head_hash")?;
        let mut values = Vec::with_capacity(req.slots.len());
        for slot_hex in &req.slots {
            let slot = Felt::from_hex(slot_hex)
                .map_err(|_| anyhow::anyhow!("bad slot {slot_hex:?}"))?;
            let (value, wb) = db.read_slot_as_of(&slot, block)?;
            values.push(json!({
                "slot": strk20_feed::felt_hex(&slot),
                "value": strk20_feed::felt_hex(&value),
                "write_block": wb,
            }));
        }
        Ok(json!({ "block": block, "block_hash": head_hash, "values": values }))
    });
    privacy_labeled(match result {
        Ok(body) => Json(body).into_response(),
        Err(resp) => resp,
    })
}

#[derive(Deserialize)]
struct RawEventsQuery {
    from: u64,
    to: u64,
    key0: Option<String>,
    key1: Option<String>,
    limit: Option<usize>,
    cursor: Option<String>,
}

async fn raw_events(State(s): State<AppState>, Query(q): Query<RawEventsQuery>) -> Response {
    let result = with_db(&s, |db| {
        let mut filters: Vec<Vec<Felt>> = Vec::new();
        if let Some(k0) = &q.key0 {
            filters.push(vec![Felt::from_hex(k0)
                .map_err(|_| anyhow::anyhow!("bad key0"))?]);
        }
        if let Some(k1) = &q.key1 {
            if filters.is_empty() {
                filters.push(vec![]);
            }
            filters.push(vec![Felt::from_hex(k1)
                .map_err(|_| anyhow::anyhow!("bad key1"))?]);
        }
        let mut rows = db.events_filtered(q.from, q.to, &filters)?;
        // cursor: "<block>-<event_index>", exclusive resume point
        if let Some(cur) = &q.cursor {
            if let Some((b, i)) = cur.split_once('-') {
                let (b, i): (u64, u64) = (b.parse().unwrap_or(0), i.parse().unwrap_or(0));
                rows.retain(|e| (e.block, e.event_index) > (b, i));
            }
        }
        let limit = q.limit.unwrap_or(1000).min(1000);
        let next = if rows.len() > limit {
            rows.truncate(limit);
            rows.last().map(|e| format!("{}-{}", e.block, e.event_index))
        } else {
            None
        };
        let events: Vec<_> = rows
            .iter()
            .map(|e| {
                json!({
                    "block": e.block,
                    "tx_index": e.tx_index,
                    "event_index": e.event_index,
                    "tx_hash": strk20_feed::felt_hex(&e.tx_hash),
                    "keys": e.keys.iter().map(strk20_feed::felt_hex).collect::<Vec<_>>(),
                    "data": e.data.iter().map(strk20_feed::felt_hex).collect::<Vec<_>>(),
                })
            })
            .collect();
        Ok(json!({ "events": events, "cursor": next }))
    });
    privacy_labeled(match result {
        Ok(body) => Json(body).into_response(),
        Err(resp) => resp,
    })
}
