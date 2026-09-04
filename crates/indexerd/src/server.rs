//! HTTP server (spec §6): feed static files (§6.1), ops (§6.2), raw
//! targeted endpoints behind --enable-raw (§6.3), compat behind
//! --enable-compat (§6.4). No feed route takes any user-derived parameter —
//! that absence is the privacy mechanism.

use crate::config::ChainConfig;
use crate::db::Db;
use axum::extract::{Path as AxPath, Query, RawQuery, Request, State};
use axum::handler::Handler;
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, MethodRouter};
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
    pub live: Arc<crate::live::LiveHub>,
}

/// CORS is a property of the PUBLIC surface only.
///
/// `/feed/*`, `/health` and `/v1/stats` take no user-derived parameter, so a
/// browser page on any origin may read them; that is the whole point of the
/// keyless feed. `/v1/raw/*` and `/v1/sync/*` are the leaky modes — what you
/// ask for is what you disclose — and they get no CORS headers at all, so a
/// hostile page cannot make a visitor's browser query them and read the
/// answer. `/metrics` is operator data and stays off the browser surface for
/// the same reason.
///
/// `Access-Control-Allow-Origin` is the literal `*`, never a reflected
/// `Origin`, so responses stay cacheable by a shared CDN without a
/// `Vary: Origin` that would shatter the cache per-origin.
const CORS_ALLOW_ORIGIN: &str = "*";
const CORS_ALLOW_METHODS: &str = "GET, HEAD, OPTIONS";
/// `ETag` is exposed because the conditional-GET path on `head.ndjson`,
/// `anchors.ndjson` and `manifest.json` is useless to a browser client that
/// cannot read the validator back out of the response.
const CORS_EXPOSE_HEADERS: &str = "ETag";
/// `If-None-Match` is not a CORS-safelisted request header, so a client that
/// revalidates by hand (rather than leaving it to the browser HTTP cache)
/// triggers a preflight that must allow it by name.
const CORS_ALLOW_HEADERS: &str =
    "If-None-Match, If-Modified-Since, Accept, Cache-Control, Last-Event-ID";
const CORS_MAX_AGE: &str = "600";

/// Adds the CORS response headers. Applied with `route_layer`, so it runs for
/// matched public routes only — a 404 elsewhere in the tree stays un-labelled.
async fn cors_public(req: Request, next: Next) -> Response {
    let mut resp = next.run(req).await;
    let h = resp.headers_mut();
    h.insert(
        header::ACCESS_CONTROL_ALLOW_ORIGIN,
        HeaderValue::from_static(CORS_ALLOW_ORIGIN),
    );
    h.insert(
        header::ACCESS_CONTROL_ALLOW_METHODS,
        HeaderValue::from_static(CORS_ALLOW_METHODS),
    );
    h.insert(
        header::ACCESS_CONTROL_EXPOSE_HEADERS,
        HeaderValue::from_static(CORS_EXPOSE_HEADERS),
    );
    resp
}

/// The preflight answer. Registered as a real `OPTIONS` handler on every
/// public route rather than left to a middleware that intercepts the method:
/// without it axum answers `405` for `OPTIONS` on a GET-only route and the
/// browser reports a CORS failure.
async fn cors_preflight() -> Response {
    let mut h = HeaderMap::new();
    h.insert(
        header::ACCESS_CONTROL_ALLOW_HEADERS,
        HeaderValue::from_static(CORS_ALLOW_HEADERS),
    );
    h.insert(
        header::ACCESS_CONTROL_MAX_AGE,
        HeaderValue::from_static(CORS_MAX_AGE),
    );
    (StatusCode::NO_CONTENT, h).into_response()
}

/// `GET` (+ automatic `HEAD`) plus the preflight answer, for a public route.
fn public_get<H, T>(handler: H) -> MethodRouter<AppState>
where
    H: Handler<T, AppState>,
    T: 'static,
{
    get(handler).options(cors_preflight)
}

pub fn build_router(
    state: AppState,
    enable_raw: bool,
    compat: Option<crate::compat::CompatState>,
) -> Router {
    let public = Router::new()
        .route("/feed/genesis.json", public_get(feed_genesis))
        .route("/feed/manifest.json", public_get(feed_manifest))
        .route("/feed/head.ndjson", public_get(feed_head))
        .route("/feed/anchors.ndjson", public_get(feed_anchors))
        .route("/feed/live", public_get(feed_live))
        .route("/feed/epochs/{name}", public_get(feed_epoch_file))
        .route("/feed/snapshots/{name}", public_get(feed_snapshot_file))
        .route("/health", public_get(health))
        .route("/v1/stats", public_get(stats))
        // route_layer, not layer: only matched public routes are labelled, so
        // merging the raw/compat trees below cannot inherit CORS through a
        // shared fallback.
        .route_layer(middleware::from_fn(cors_public));
    let mut router = public
        // Operator surface: no CORS, deliberately.
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

/// A strong validator: `"<sha256 hex>"`, no `W/` prefix, quoted per RFC 9110.
fn strong_etag(digest: &[u8]) -> String {
    format!("\"{}\"", hex::encode(digest))
}

/// `If-None-Match` per RFC 9110 §13.1.2: a comma-separated list, or `*`.
/// Weak comparison is used, which for our always-strong tags is exact match
/// after stripping any `W/` a proxy may have added.
fn if_none_match(headers: &HeaderMap, etag: &str) -> bool {
    let Some(raw) = headers
        .get(header::IF_NONE_MATCH)
        .and_then(|v| v.to_str().ok())
    else {
        return false;
    };
    raw.split(',').any(|candidate| {
        let c = candidate.trim();
        c == "*" || c.strip_prefix("W/").unwrap_or(c) == etag
    })
}

/// `etag` is the validator for the bytes at `path`, when it is known without
/// reading them (the epochs table already stores the hash of the compressed
/// file). Supplying it lets a revalidating cache take the 304 branch without
/// the server touching the disk at all.
async fn serve_file(
    path: PathBuf,
    cache: &'static str,
    etag: Option<String>,
    extra: Option<(&'static str, String)>,
    req: &HeaderMap,
) -> Response {
    if let Some(tag) = &etag {
        if if_none_match(req, tag) {
            let mut h = HeaderMap::new();
            h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
            if let Ok(v) = HeaderValue::from_str(tag) {
                h.insert(header::ETAG, v);
            }
            return (StatusCode::NOT_MODIFIED, h).into_response();
        }
    }
    match tokio::fs::read(&path).await {
        Ok(bytes) => {
            let mut headers = HeaderMap::new();
            headers.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
            if let Some(tag) = &etag {
                if let Ok(v) = HeaderValue::from_str(tag) {
                    headers.insert(header::ETAG, v);
                }
            }
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

async fn feed_genesis(State(s): State<AppState>, headers: HeaderMap) -> Response {
    serve_file(
        s.feed_dir.join("genesis.json"),
        "public, max-age=31536000, immutable",
        None,
        None,
        &headers,
    )
    .await
}

/// The manifest is the one MUTABLE index every client polls, and it grows
/// without bound: 146 KB / 519 epochs on mainnet today. `max-age=30` alone
/// meant every poll past 30 s was a full re-transfer of a file that usually
/// gained one entry, so it gets the same sha256 validator and 304 path as the
/// head. The short `max-age` stays: it is what lets a CDN absorb a burst.
async fn feed_manifest(State(s): State<AppState>, headers: HeaderMap) -> Response {
    revalidated(
        s.feed_dir.join("manifest.json"),
        "public, max-age=30",
        "application/json",
        headers,
    )
    .await
}

/// The append-only anchor log. Append-only means the tail grows, so it is
/// revalidated like the head rather than cached immutably — and, like the head,
/// it gets a conditional-GET path: a grounded client refetches it on EVERY
/// sync, and without an ETag every one of those is a full transfer of a file
/// that only ever gains a line.
async fn feed_anchors(State(s): State<AppState>, headers: HeaderMap) -> Response {
    revalidated_ndjson(s.feed_dir.join("anchors.ndjson"), headers).await
}

async fn feed_head(State(s): State<AppState>, headers: HeaderMap) -> Response {
    revalidated_ndjson(s.feed_dir.join("head.ndjson"), headers).await
}

/// A mutable NDJSON artifact served `no-cache` with a sha256 ETag and a 304
/// path.
async fn revalidated_ndjson(path: std::path::PathBuf, headers: HeaderMap) -> Response {
    revalidated(path, "no-cache", "application/x-ndjson", headers).await
}

/// A mutable artifact served with a strong sha256 ETag and a 304 path.
async fn revalidated(
    path: std::path::PathBuf,
    cache: &'static str,
    content_type: &'static str,
    headers: HeaderMap,
) -> Response {
    let bytes = match tokio::fs::read(&path).await {
        Ok(b) => b,
        Err(_) => return (StatusCode::NOT_FOUND, "not found").into_response(),
    };
    let etag = strong_etag(&strk20_feed::payload_sha256(&bytes));
    let mut h = HeaderMap::new();
    h.insert(header::CACHE_CONTROL, HeaderValue::from_static(cache));
    h.insert(header::ETAG, HeaderValue::from_str(&etag).unwrap());
    if if_none_match(&headers, &etag) {
        // A 304 must carry the validator and the caching metadata it is
        // refreshing (RFC 9110 §15.4.5); the previous version sent a bare 304,
        // which left a cache with no ETag to reuse on its next revalidation.
        return (StatusCode::NOT_MODIFIED, h).into_response();
    }
    h.insert(header::CONTENT_TYPE, HeaderValue::from_static(content_type));
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

async fn feed_epoch_file(
    State(s): State<AppState>,
    AxPath(name): AxPath<String>,
    headers: HeaderMap,
) -> Response {
    let Some((idx, is_epoch)) = valid_epoch_name(&name) else {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    };
    // Two different hashes, deliberately: `x-content-sha256-raw` is the
    // DECOMPRESSED payload hash a mirror checks against the manifest chain,
    // while the ETag is the hash of the exact `.zst` bytes on the wire, which
    // is what an HTTP validator has to identify. Both come from the row we
    // already read, so neither costs a hash over the file.
    let (extra, etag) = if is_epoch {
        let db = s.db.lock().expect("db mutex");
        match db
            .epoch_rows()
            .ok()
            .and_then(|rows| rows.into_iter().find(|r| r.idx == idx))
        {
            Some(r) => (
                Some(("x-content-sha256-raw", hex::encode(r.content_hash))),
                Some(strong_etag(&r.zst_sha256)),
            ),
            None => (None, None),
        }
    } else {
        (None, None)
    };
    serve_file(
        s.feed_dir.join("epochs").join(&name),
        "public, max-age=31536000, immutable",
        etag,
        extra,
        &headers,
    )
    .await
}

async fn feed_snapshot_file(
    State(s): State<AppState>,
    AxPath(name): AxPath<String>,
    headers: HeaderMap,
) -> Response {
    if name.contains('/') || name.contains("..") {
        return (StatusCode::NOT_FOUND, "not found").into_response();
    }
    serve_file(
        s.feed_dir.join("snapshots").join(&name),
        "public, max-age=31536000, immutable",
        None,
        None,
        &headers,
    )
    .await
}

/// `GET /feed/live` (§2.1) — always on, no flag.
///
/// Any query string is 400 `INVALID_QUERY` rather than ignored: that turns the
/// address-blindness property into a SERVER-enforced guarantee instead of a
/// client-side convention. Query-appending `EventSource` polyfills are
/// documented as unsupported.
async fn feed_live(State(s): State<AppState>, RawQuery(query): RawQuery) -> Response {
    if query.is_some() {
        return (
            StatusCode::BAD_REQUEST,
            "INVALID_QUERY: /feed/live takes no parameters",
        )
            .into_response();
    }
    let hello = json!({
        "v": strk20_feed::codec::FORMAT_VERSION,
        "chain_id": s.cfg.chain_id,
        "pool": strk20_feed::felt_hex(&s.cfg.pool),
        "module": concat!("strk20/", env!("CARGO_PKG_VERSION")),
    })
    .to_string();

    // Read the published files now, so the connect burst is the present and
    // not the last tick's past (§2.2: connect always replays CURRENT state).
    s.live.refresh();
    let (tx, rx) = tokio::sync::mpsc::channel::<std::io::Result<axum::body::Bytes>>(32);
    let hub = s.live.clone();
    tokio::spawn(async move { crate::live::stream_to(hub, hello, tx).await });
    let stream = futures::stream::unfold(rx, |mut rx| async move {
        rx.recv().await.map(|item| (item, rx))
    });

    let mut headers = HeaderMap::new();
    headers.insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("text/event-stream"),
    );
    headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-cache"));
    // Proxies must not buffer the stream; without this a poke can sit in a
    // reverse proxy until enough bytes accumulate.
    headers.insert("x-accel-buffering", HeaderValue::from_static("no"));
    (headers, axum::body::Body::from_stream(stream)).into_response()
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
        // DEGRADED on its own tells an operator that something is wrong and
        // nothing about what to do, which is how a frozen head with a silent
        // log stayed unexplained for tens of minutes at a time. These two say
        // WHERE the divergence was seen and what the next action is.
        let mismatch_block = crate::recovery::mismatch_block(db)?;
        let reason = crate::recovery::reason(db)?;
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
            "mismatch_block": mismatch_block,
            "reason": reason,
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
             # TYPE strk20_decode_degraded gauge\nstrk20_decode_degraded {}\n\
             # TYPE strk20_sse_connections gauge\nstrk20_sse_connections {}\n",
            degraded as u8,
            s.live.connections()
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

// ------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;

    /// Serve `build_router` on a loopback port and return its base URL.
    /// A real socket, not a synthetic `oneshot`: the properties under test are
    /// what a browser sees on the wire, including the `OPTIONS`-vs-405
    /// behaviour of the method router.
    async fn serve(dir: &std::path::Path, enable_raw: bool) -> String {
        let db = Arc::new(Mutex::new(Db::open(&dir.join("t.db")).unwrap()));
        let feed_dir = dir.join("feed");
        std::fs::create_dir_all(feed_dir.join("epochs")).unwrap();
        std::fs::write(feed_dir.join("manifest.json"), br#"{"epochs":[]}"#).unwrap();
        std::fs::write(feed_dir.join("head.ndjson"), b"{}\n").unwrap();
        let live = Arc::new(crate::live::LiveHub::new(feed_dir.clone(), db.clone()));
        let state = AppState {
            feed_dir,
            db,
            cfg: crate::config::ChainConfig::mainnet(),
            live,
        };
        let router = build_router(state, enable_raw, None);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        format!("http://{addr}")
    }

    fn acao(r: &reqwest::Response) -> Option<String> {
        r.headers()
            .get(header::ACCESS_CONTROL_ALLOW_ORIGIN)
            .and_then(|v| v.to_str().ok())
            .map(str::to_owned)
    }

    /// #21: the container answers `/health` from the moment it binds, which is
    /// before the first ingest cycle has produced anything. Every height it
    /// publishes comes from `meta`, and `meta.l1_accepted_number` is written by
    /// exactly one place — `Ingestor::run_cycle` — so a mirror that has not
    /// completed a cycle must publish NO L1-accepted height at all. `null` and
    /// the `0` gauge are the two shapes of "not produced yet"; a block number
    /// here is a claim the server has no basis for.
    #[tokio::test]
    async fn a_mirror_that_has_run_no_cycle_publishes_no_l1_accepted_height() {
        let dir = tempfile::tempdir().unwrap();
        let base = serve(dir.path(), false).await;

        let r = reqwest::get(format!("{base}/health")).await.unwrap();
        assert_eq!(r.status(), 503, "no head yet is UNHEALTHY");
        let body: serde_json::Value = r.json().await.unwrap();
        assert_eq!(body["status"], "UNHEALTHY", "{body}");
        assert!(body["head"].is_null(), "{body}");
        assert!(
            body["l1_accepted"].is_null(),
            "a fresh mirror must publish no L1-accepted height, got {}",
            body["l1_accepted"]
        );

        // The gauge cannot carry `null`, so it carries the unset value; what it
        // must never carry is a height nothing measured.
        let text = reqwest::get(format!("{base}/metrics"))
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(
            text.contains("strk20_l1_accepted_block 0\n"),
            "expected the unset gauge, got:\n{text}"
        );
    }

    #[tokio::test]
    async fn cors_covers_the_public_surface_and_nothing_else() {
        let dir = tempfile::tempdir().unwrap();
        let base = serve(dir.path(), true).await;
        let http = reqwest::Client::new();

        // Every public route, including the ones whose GET streams
        // (`/feed/live`) or 404s on an empty feed dir: the preflight is what a
        // browser blocks on, and it must be answered on all of them.
        for path in [
            "/feed/genesis.json",
            "/feed/manifest.json",
            "/feed/head.ndjson",
            "/feed/anchors.ndjson",
            "/feed/live",
            "/feed/epochs/00000001.strk20e.zst",
            "/feed/snapshots/anything",
            "/health",
            "/v1/stats",
        ] {
            // Preflight must be ANSWERED, not 405'd.
            let pre = http
                .request(reqwest::Method::OPTIONS, format!("{base}{path}"))
                .send()
                .await
                .unwrap();
            assert_eq!(pre.status(), 204, "preflight {path}");
            assert_eq!(acao(&pre).as_deref(), Some("*"), "preflight ACAO {path}");
            assert_eq!(
                pre.headers()
                    .get(header::ACCESS_CONTROL_ALLOW_METHODS)
                    .and_then(|v| v.to_str().ok()),
                Some("GET, HEAD, OPTIONS"),
            );
            assert!(pre.headers().contains_key(header::ACCESS_CONTROL_ALLOW_HEADERS));
        }

        // The labels must also be on the real answers, including the 404 a
        // missing artifact produces — a browser that cannot read a 404 sees a
        // CORS error instead and reports the wrong failure.
        for path in [
            "/feed/manifest.json",
            "/feed/head.ndjson",
            "/feed/epochs/00000001.strk20e.zst",
            "/health",
            "/v1/stats",
        ] {
            let get = http.get(format!("{base}{path}")).send().await.unwrap();
            assert_eq!(acao(&get).as_deref(), Some("*"), "GET ACAO {path}");
            assert_eq!(
                get.headers()
                    .get(header::ACCESS_CONTROL_EXPOSE_HEADERS)
                    .and_then(|v| v.to_str().ok()),
                Some("ETag"),
                "ETag must be readable from script on {path}",
            );
        }

        // The leaky modes and the operator surface stay un-embeddable.
        for path in ["/v1/raw/events?from=0&to=1", "/metrics"] {
            let pre = http
                .request(reqwest::Method::OPTIONS, format!("{base}{path}"))
                .send()
                .await
                .unwrap();
            assert!(acao(&pre).is_none(), "no CORS on preflight {path}");
            let get = http.get(format!("{base}{path}")).send().await.unwrap();
            assert!(acao(&get).is_none(), "no CORS on {path}");
        }

        // Existing response labelling survives the router restructure.
        let raw = http
            .get(format!("{base}/v1/raw/events?from=0&to=1"))
            .send()
            .await
            .unwrap();
        assert_eq!(
            raw.headers().get(PRIVACY_HEADER).and_then(|v| v.to_str().ok()),
            Some(PRIVACY_VALUE),
        );
    }

    #[tokio::test]
    async fn manifest_has_a_strong_etag_and_a_304_path() {
        let dir = tempfile::tempdir().unwrap();
        let base = serve(dir.path(), false).await;
        let http = reqwest::Client::new();

        let first = http
            .get(format!("{base}/feed/manifest.json"))
            .send()
            .await
            .unwrap();
        assert_eq!(first.status(), 200);
        let etag = first
            .headers()
            .get(header::ETAG)
            .unwrap()
            .to_str()
            .unwrap()
            .to_owned();
        assert!(etag.starts_with('"') && !etag.starts_with("W/"), "strong: {etag}");
        assert_eq!(
            first
                .headers()
                .get(header::CACHE_CONTROL)
                .and_then(|v| v.to_str().ok()),
            Some("public, max-age=30"),
        );

        let second = http
            .get(format!("{base}/feed/manifest.json"))
            .header(header::IF_NONE_MATCH, &etag)
            .send()
            .await
            .unwrap();
        assert_eq!(second.status(), 304);
        // A 304 without the validator forces the next revalidation to be a
        // full transfer.
        assert_eq!(
            second.headers().get(header::ETAG).and_then(|v| v.to_str().ok()),
            Some(etag.as_str()),
        );
    }
}
