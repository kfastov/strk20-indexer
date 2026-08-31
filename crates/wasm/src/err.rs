//! §3.7's error model, at the strength this crate can actually deliver.
//!
//! Every throw is a `JsError` whose message is one canonical JSON object:
//!
//! ```json
//! {"code":"FEED_HASH_MISMATCH","message":"…","retryable":false}
//! ```
//!
//! # Two ways a code is found, and why
//!
//! `strk20-feed` has a typed error enum, so the errors it raises are
//! **downcast** out of the `anyhow` chain and projected onto codes *and*
//! `details` structurally — no string matching, no drift. §3.7 claims
//! `FeedError` "maps 1:1 onto the `FEED_*` and `SNAPSHOT_*` codes"; it does
//! not, because its `Display` strings spell none of them (only
//! `DECOMPRESS_LIMIT`). This module is where that mapping actually exists.
//!
//! `strk20-consumer` has no error enum at all — it raises `anyhow!` with the
//! code spelled at the head of a prose message. Those are matched on the code
//! token, which is stable and machine-checkable, and their operands stay in the
//! prose because there is no struct to read them from. Giving Block B a typed
//! error enum would let `details` be populated everywhere; that is a
//! `strk20-consumer` change, listed in the return notes.
//!
//! # Scrubbing
//!
//! No message reaching here can contain key material: the only type holding a
//! viewing key is `SecretFelt`, whose `Debug` is `[REDACTED]` and which is
//! never formatted into an error anywhere in Block B. Channel keys live inside
//! cursors, which are never formatted into errors either. The `INTERNAL` path
//! (panics) is the one place a raw string arrives, and it carries a Rust panic
//! location and message, not user data.

use anyhow::Result;
use wasm_bindgen::JsError;

/// The closed §3.7 code set, plus the two this crate raises for staging. A code
/// is recognised when the message begins with it followed by `:` or ` `, which
/// is exactly how `strk20-consumer` and `strk20-feed` spell them.
const CODES: &[&str] = &[
    "FEED_HASH_MISMATCH",
    "FEED_CHAIN_BROKEN",
    "FEED_MALFORMED",
    "FEED_EPOCH_GAP",
    "FEED_ADVANCED_MIDSYNC",
    "DECOMPRESS_LIMIT",
    "SNAPSHOT_ROOT_MISMATCH",
    "SNAPSHOT_ANCHOR_MISSING",
    "SNAPSHOT_NOT_EMPTY",
    "SNAPSHOT_UNAVAILABLE",
    "SNAPSHOT_UNREACHABLE",
    "BOUND_BELOW_SNAPSHOT",
    "CHAIN_MISMATCH",
    "STATE_CORRUPT",
    "STATE_VERSION",
    "STATE_FOREIGN",
    "KEY_INVALID",
    "HISTORY_UNAVAILABLE",
    "CONFIG_INVALID",
    // this crate's own: the caller pushed nothing for an artifact Block B asked
    // for, which is a wrapper bug, not a feed problem
    "NOT_STAGED",
    // §1.5 ring 6. `ANCHOR_NOT_ON_CHAIN` is Block B's verdict — the user's own
    // endpoint refutes this mirror; the `PROOF_*` codes are this crate's, and
    // every one of them is a REFUSAL to report a grade rather than a downgrade
    // of one.
    "ANCHOR_NOT_ON_CHAIN",
    "PROOF_MALFORMED",
    "PROOF_NOT_STAGED",
    "PROOF_UNUSED",
];

/// Retryable per §3.7. Only the manifest/head race heals on its own.
const RETRYABLE: &[&str] = &["FEED_ADVANCED_MIDSYNC"];

pub struct ErrJson {
    code: String,
    message: String,
    details: serde_json::Value,
}

impl ErrJson {
    pub fn internal(message: &str) -> Self {
        Self {
            code: "INTERNAL".into(),
            message: message.to_owned(),
            details: serde_json::json!({}),
        }
    }

    /// The structural half: project a typed `FeedError` onto its code and
    /// operands. This is the mapping §3.7 assumed already existed.
    fn from_feed_error(e: &strk20_feed::FeedError, rendered: &str) -> Self {
        use strk20_feed::FeedError as F;
        let (code, details) = match e {
            F::HashMismatch {
                epoch,
                expected,
                actual,
            } => (
                "FEED_HASH_MISMATCH",
                serde_json::json!({"artifact": format!("epoch {epoch}"), "epoch": epoch,
                                   "expected": expected, "actual": actual}),
            ),
            F::ChainBroken {
                epoch,
                expected,
                actual,
            } => (
                "FEED_CHAIN_BROKEN",
                serde_json::json!({"epoch": epoch, "expected_prev": expected,
                                   "actual_prev": actual}),
            ),
            F::DecompressLimit { artifact, cap } => (
                "DECOMPRESS_LIMIT",
                serde_json::json!({"artifact": artifact, "cap": cap}),
            ),
            F::Malformed(detail) => (
                "FEED_MALFORMED",
                serde_json::json!({"detail": detail}),
            ),
            F::Json(err) => (
                "FEED_MALFORMED",
                serde_json::json!({"detail": err.to_string(), "line": err.line()}),
            ),
            F::BadFelt(s) => (
                "FEED_MALFORMED",
                serde_json::json!({"detail": format!("not a felt: {s}")}),
            ),
            F::Decompress(s) => (
                "FEED_MALFORMED",
                serde_json::json!({"detail": s}),
            ),
        };
        Self {
            code: code.to_owned(),
            message: rendered.to_owned(),
            details,
        }
    }

    /// Structural first, then textual. A `FeedError` anywhere in the chain wins
    /// — it carries operands a string match could only guess at.
    pub fn classify(e: &anyhow::Error) -> Self {
        let rendered = format!("{e:#}");
        for cause in e.chain() {
            if let Some(fe) = cause.downcast_ref::<strk20_feed::FeedError>() {
                return Self::from_feed_error(fe, &rendered);
            }
        }
        Self::classify_text(&rendered)
    }

    /// Lift a code from an `anyhow` chain's rendering, if it spells one.
    /// `anyhow`'s `{:#}` walks the whole context chain, so a code attached
    /// deeper than the outermost context is still found.
    fn classify_text(rendered: &str) -> Self {
        for code in CODES {
            let mut hay = rendered;
            while let Some(at) = hay.find(code) {
                let rest = &hay[at + code.len()..];
                let boundary = rest.starts_with(':') || rest.starts_with(' ') || rest.is_empty();
                let start_ok = at == 0 || !hay.as_bytes()[at - 1].is_ascii_alphanumeric();
                if boundary && start_ok {
                    return Self {
                        code: (*code).to_owned(),
                        message: rendered.to_owned(),
                        details: serde_json::json!({}),
                    };
                }
                hay = &hay[at + code.len()..];
            }
        }
        Self::internal(rendered)
    }

    #[allow(clippy::inherent_to_string)]
    pub fn to_string(&self) -> String {
        let retryable = RETRYABLE.contains(&self.code.as_str());
        serde_json::json!({
            "code": self.code,
            "message": self.message,
            "details": self.details,
            "retryable": retryable,
        })
        .to_string()
    }
}

/// Convert a Block B result into the ABI's result type.
pub fn to_js<T>(r: Result<T>) -> Result<T, JsError> {
    r.map_err(|e| JsError::new(&ErrJson::classify(&e).to_string()))
}
