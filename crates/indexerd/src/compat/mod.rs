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
use axum::routing::{get, post};
use axum::{Json, Router};
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
    Router::new()
        .route("/health", get(health))
        .route("/v1/sync/incoming_state", post(incoming))
        .route("/v1/sync/outgoing_state", post(outgoing))
        .route("/v1/sync/preflight_check", post(preflight))
        .route("/v1/history", post(history))
        .with_state(state)
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

async fn health(State(state): State<CompatState>) -> Response {
    let head = with_db(&state, |db| {
        let number: Option<u64> = db.meta_get("head_number")?.and_then(|s| s.parse().ok());
        let hash = db.meta_get("head_hash")?;
        let ts = number
            .and_then(|n| db.block(n).ok().flatten())
            .map(|b| b.timestamp);
        Ok(number.zip(hash).map(|(n, h)| ChainHead {
            block_number: n,
            block_hash: Felt::from_hex(&h).unwrap_or(Felt::ZERO),
            timestamp: ts.unwrap_or(0),
        }))
    });
    match head {
        Ok(chain_head) => labeled(
            Json(wire::HealthResponse {
                status: "OK".into(),
                chain_head,
                lag_secs: 0,
            })
            .into_response(),
        ),
        Err(resp) => resp,
    }
}

async fn incoming(
    State(state): State<CompatState>,
    Json(req): Json<wire::IncomingSyncRequest>,
) -> HandlerResult {
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
    Json(req): Json<wire::OutgoingSyncRequest>,
) -> HandlerResult {
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
    Json(req): Json<wire::PreflightCheckRequest>,
) -> HandlerResult {
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
    Json(req): Json<wire::HistoryRequest>,
) -> HandlerResult {
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
