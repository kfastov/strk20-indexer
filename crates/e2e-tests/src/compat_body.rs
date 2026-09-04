//! The reference-shaped `POST /v1/sync/incoming_state` request body, in one
//! place because three carriers must agree on its shape, and two of them on
//! the exact bytes:
//!
//! * `acceptance.rs` leg d(iv) scans these bytes to prove the leak detector
//!   is not blind (the body DOES carry a real viewing key);
//! * `acceptance.rs` leg h POSTs them at a live `--enable-compat` server, so
//!   that the key leg f(iii) greps the server's db/feed/logs for is a key the
//!   server was actually handed;
//! * `conformance.rs::compat_incoming_wire_equals_oracle` builds the same
//!   shape (over its own seeded mirror) and POSTs it at the compat router
//!   bound in-process, comparing the decoded response to the oracle.
//!
//! Before #17 leg d built the bytes and leg h consumed them ~165 lines later,
//! so an edit to the self-test body silently changed what the live server was
//! sent — and with it whether f(iii)'s scan was vacuous. Shared here the
//! coupling is structural instead of comment-enforced.

use starknet_types_core::felt::Felt;

/// Reference `IncomingSyncRequest` for a fresh (cursorless) sync.
pub fn incoming_state_body(pool_hex: &str, recipient: Felt, viewing_key_hex: &str) -> Vec<u8> {
    serde_json::to_vec(&serde_json::json!({
        "contract_address": pool_hex,
        "viewing_key": viewing_key_hex,
        "recipient_address": strk20_feed::felt_hex(&recipient),
    }))
    .expect("serialize compat request body")
}
