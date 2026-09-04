//! Compat mode (spec §6.4): the exact reference `/v1/sync/*` + `/v1/history`
//! wire over the unmodified discovery-core engine via the SQLite bridge.
//! OFF by default; key-visible and labeled. Hard rules: request/response
//! bodies and cursors are NEVER logged (they carry raw viewing keys and
//! key-derived channel keys).

pub mod block_id_serde;
pub mod wire;

use crate::bridge::DbBackend;
use crate::db::Db;
use axum::extract::State;
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use axum::{Json, Router};
use serde::de::DeserializeOwned;
use discovery_core::discovery::CursorLimits;
use discovery_core::io_budget::IoBudget;
use discovery_core::privacy_pool::types::SecretFelt;
use discovery_core::privacy_pool::views::IViews;
use discovery_core::storage_backend::{StorageBackend, StorageSnapshot};
use serde::{Deserialize, Serialize};
use starknet_core::types::{BlockId, Felt};
use std::sync::{Arc, Mutex};
use wire::{error_codes, ApiErrorResponse};

/// Reference server budget default (spec §14.1 limits).
const SERVER_BUDGET: usize = 10_000;
pub const MODE_HEADER: &str = "x-strk20-mode";
pub const MODE_VALUE: &str = "compat-keyed";

/// Reference /health chain head shape.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct ChainHead {
    pub block_number: u64,
    pub block_hash: Felt,
    pub timestamp: u64,
}

#[derive(Clone)]
pub struct CompatState {
    pub backend: DbBackend,
    pub db: Arc<Mutex<Db>>,
    pub pool: Felt,
}

pub fn router(state: CompatState) -> Router {
    // NOTE: /health is served by the ops router (server.rs) for both modes;
    // its body carries the same `status` field the SDK's isHealthy() reads.
    Router::new()
        .route("/v1/sync/incoming_state", post(incoming))
        .route("/v1/sync/outgoing_state", post(outgoing))
        .route("/v1/sync/preflight_check", post(preflight))
        .route("/v1/history", post(history))
        // Unconditional labeling: axum-generated rejections (405, 415, …)
        // must carry the mode header too (review finding).
        .layer(axum::middleware::map_response(label_response))
        .with_state(state)
}

async fn label_response(mut resp: Response) -> Response {
    resp.headers_mut()
        .insert(MODE_HEADER, HeaderValue::from_static(MODE_VALUE));
    resp
}

/// Manual body parse: an axum `Json` rejection echoes fragments of the body
/// in its error text — a request body here contains a raw viewing key, so
/// parse failures must produce a generic error that echoes NOTHING.
fn parse_body<T: DeserializeOwned>(body: &axum::body::Bytes) -> Result<T, Response> {
    serde_json::from_slice(body).map_err(|_| {
        err(
            StatusCode::BAD_REQUEST,
            ApiErrorResponse::new(error_codes::INVALID_REQUEST, "malformed request body"),
        )
    })
}

fn labeled(mut resp: Response) -> Response {
    resp.headers_mut()
        .insert(MODE_HEADER, HeaderValue::from_static(MODE_VALUE));
    resp
}

fn err(status: StatusCode, body: ApiErrorResponse) -> Response {
    labeled((status, Json(body)).into_response())
}

type HandlerResult = Result<Response, Response>;

fn with_db<T>(state: &CompatState, f: impl FnOnce(&Db) -> anyhow::Result<T>) -> Result<T, Response> {
    let db = state.db.lock().expect("db mutex");
    f(&db).map_err(|e| {
        tracing::warn!(error = %e, "compat db error");
        err(
            StatusCode::SERVICE_UNAVAILABLE,
            ApiErrorResponse::new(error_codes::STORAGE_ERROR, "Storage backend error"),
        )
    })
}

/// Degraded-mode gate (spec §5.7): once an unknown class hash appears at
/// block b, compat answers SERVICE_UNAVAILABLE for any read at/after b.
fn check_degraded(state: &CompatState, resolved_block: u64) -> Result<(), Response> {
    let (decode_state, since) = with_db(state, |db| {
        Ok((
            db.meta_get("decode_state")?.unwrap_or_else(|| "ok".into()),
            db.meta_get("degraded_since_block")?
                .and_then(|s| s.parse::<u64>().ok()),
        ))
    })?;
    if decode_state == "degraded" {
        if let Some(boundary) = since {
            if resolved_block >= boundary {
                return Err(err(
                    StatusCode::SERVICE_UNAVAILABLE,
                    ApiErrorResponse::new(
                        error_codes::SERVICE_UNAVAILABLE,
                        "typed decoding degraded: unknown pool class hash",
                    ),
                ));
            }
        }
    }
    Ok(())
}

/// Reorg gate: 409 BLOCK_REORGED when last_known_block is no longer
/// canonical (reference spec 11; SDK treats HTTP 409 exclusively as reorg).
fn check_last_known(state: &CompatState, last_known: Option<Felt>) -> Result<(), Response> {
    let Some(hash) = last_known else {
        return Ok(());
    };
    let canonical = with_db(state, |db| db.is_canonical(&hash))?;
    if !canonical {
        return Err(err(
            StatusCode::CONFLICT,
            ApiErrorResponse::new(
                error_codes::BLOCK_REORGED,
                "last_known_block is no longer canonical",
            ),
        ));
    }
    Ok(())
}

async fn snapshot_for(
    state: &CompatState,
    contract: Felt,
    block_ref: Option<BlockId>,
) -> Result<crate::bridge::DbSnapshot, Response> {
    state
        .backend
        .snapshot(contract, block_ref)
        .await
        .map_err(|e| {
            let (status, body) = wire::storage_error_to_response(e);
            err(status, body)
        })
}

async fn validate_viewing_key(
    snapshot: &crate::bridge::DbSnapshot,
    user_address: Felt,
    viewing_key: &SecretFelt,
) -> Result<(), Response> {
    let registered = snapshot.get_public_key(user_address).await.map_err(|e| {
        let (status, body) = wire::storage_error_to_response(e);
        err(status, body)
    })?;
    if registered == Felt::ZERO {
        return Ok(()); // unregistered: skip (reference behavior)
    }
    let derived = starknet_crypto::get_public_key(viewing_key);
    if derived != registered {
        return Err(err(
            StatusCode::BAD_REQUEST,
            ApiErrorResponse::new(
                error_codes::INVALID_REQUEST,
                "viewing_key does not match the registered public key for the given address",
            ),
        ));
    }
    Ok(())
}

async fn incoming(
    State(state): State<CompatState>,
    body: axum::body::Bytes,
) -> HandlerResult {
    let req: wire::IncomingSyncRequest = parse_body(&body)?;
    check_last_known(&state, req.base.last_known_block)?;
    let snapshot = snapshot_for(&state, req.base.contract_address, req.base.block_ref).await?;
    check_degraded(&state, snapshot.bound_block())?;
    validate_viewing_key(&snapshot, req.recipient_address, &req.base.viewing_key).await?;
    let budget = IoBudget::new(SERVER_BUDGET);
    let result = discovery_core::sync::incoming_state::sync_incoming_state(
        &snapshot,
        req.recipient_address,
        &req.base.viewing_key,
        req.base.cursor,
        CursorLimits::default(),
        &budget,
    )
    .await
    .map_err(|e| {
        let (status, body) = wire::discovery_error_to_response(e);
        err(status, body)
    })?;
    Ok(labeled(
        Json(wire::IncomingSyncResponse {
            block_ref: StorageSnapshot::block_id(&snapshot),
            channels: result.channels,
            subchannels: result.subchannels,
            notes: result.notes,
            cursor: result.cursor,
        })
        .into_response(),
    ))
}

async fn outgoing(
    State(state): State<CompatState>,
    body: axum::body::Bytes,
) -> HandlerResult {
    let req: wire::OutgoingSyncRequest = parse_body(&body)?;
    check_last_known(&state, req.base.last_known_block)?;
    let snapshot = snapshot_for(&state, req.base.contract_address, req.base.block_ref).await?;
    check_degraded(&state, snapshot.bound_block())?;
    validate_viewing_key(&snapshot, req.sender_address, &req.base.viewing_key).await?;
    let budget = IoBudget::new(SERVER_BUDGET);
    let result = discovery_core::sync::outgoing_state::sync_outgoing_state(
        &snapshot,
        req.sender_address,
        &req.base.viewing_key,
        req.base.cursor,
        CursorLimits::default(),
        &budget,
        req.recipients.as_ref(),
    )
    .await
    .map_err(|e| {
        let (status, body) = wire::discovery_error_to_response(e);
        err(status, body)
    })?;
    Ok(labeled(
        Json(wire::OutgoingSyncResponse {
            block_ref: StorageSnapshot::block_id(&snapshot),
            channels: result.channels,
            subchannels: result.subchannels,
            cursor: result.cursor,
        })
        .into_response(),
    ))
}

async fn preflight(
    State(state): State<CompatState>,
    body: axum::body::Bytes,
) -> HandlerResult {
    let req: wire::PreflightCheckRequest = parse_body(&body)?;
    let snapshot = snapshot_for(&state, req.contract_address, None).await?;
    check_degraded(&state, snapshot.bound_block())?;
    let result = discovery_core::sync::preflight_check::preflight_check(
        &snapshot,
        req.sender_address,
        &req.viewing_key,
        req.recipient,
        req.token,
    )
    .await
    .map_err(|e| {
        let (status, body) = wire::discovery_error_to_response(e);
        err(status, body)
    })?;
    Ok(labeled(
        Json(wire::PreflightCheckResponse {
            block_ref: StorageSnapshot::block_id(&snapshot),
            sender_registered: result.sender_registered,
            channel_exists: result.channel_exists,
            subchannel_exists: result.subchannel_exists,
        })
        .into_response(),
    ))
}

async fn history(
    State(state): State<CompatState>,
    body: axum::body::Bytes,
) -> HandlerResult {
    let req: wire::HistoryRequest = parse_body(&body)?;
    check_last_known(&state, req.last_known_block)?;
    let snapshot = snapshot_for(&state, req.contract_address, req.block_ref).await?;
    check_degraded(&state, snapshot.bound_block())?;
    let budget = IoBudget::new(SERVER_BUDGET);
    let mut cursor = req.cursor;
    let transactions = discovery_core::history::transactions::fetch_transactions(
        &snapshot,
        req.user_address,
        &mut cursor,
        req.max_transactions as usize,
        &budget,
    )
    .await
    .map_err(|e| {
        let (status, body) = wire::discovery_error_to_response(e);
        err(status, body)
    })?;
    Ok(labeled(
        Json(wire::HistoryResponse {
            block_ref: StorageSnapshot::block_id(&snapshot),
            transactions,
            cursor,
        })
        .into_response(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::BlockRow;

    /// The 409 gate, moved down from the acceptance leg it used to ride on
    /// (#17). The claim is narrow and so is the fixture: a hash the reorg
    /// rollback tombstoned in `seen_heads` must produce HTTP 409 with code
    /// BLOCK_REORGED, while a hash still in `blocks` must pass. The
    /// unknown-hash case is a THIRD outcome and belongs here too — this
    /// instance cannot testify against a hash it never saw, so it must NOT
    /// 409; that false 409 is what broke every client after a DB rebuild.
    #[tokio::test]
    async fn reorged_last_known_block_409s() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("compat.db");
        let canonical = Felt::from(0xc0ffee_u64);
        let orphan = Felt::from(0xdead_u64);
        let never_seen = Felt::from(0x1234_u64);
        {
            let mut db = Db::open(&path).unwrap();
            for (number, hash) in [(44_u64, canonical), (45, orphan)] {
                db.insert_block(&BlockRow {
                    number,
                    hash,
                    parent_hash: Felt::ZERO,
                    timestamp: 1000 + number,
                    l1_accepted: false,
                })
                .unwrap();
            }
            // the reorg: everything above 44 is forgotten and tombstoned
            db.rollback_above(44).unwrap();
        }
        let state = CompatState {
            backend: DbBackend::new(path.clone(), Felt::ONE),
            db: Arc::new(Mutex::new(Db::open(&path).unwrap())),
            pool: Felt::ONE,
        };

        assert!(check_last_known(&state, None).is_ok(), "no hash, no gate");
        assert!(
            check_last_known(&state, Some(canonical)).is_ok(),
            "a block still in `blocks` is canonical"
        );
        assert!(
            check_last_known(&state, Some(never_seen)).is_ok(),
            "an unseen hash must not 409: this instance cannot testify against it"
        );

        let resp = check_last_known(&state, Some(orphan)).expect_err("orphan must be rejected");
        assert_eq!(resp.status(), StatusCode::CONFLICT);
        let body = axum::body::to_bytes(resp.into_body(), 64 * 1024)
            .await
            .unwrap();
        let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(json["error"]["code"], error_codes::BLOCK_REORGED);
    }
}
