//! The CLOSED, whole-path URL allowlist a feed-mode client may emit
//! (consumer-path.md §2.8.1, extended by §11.2's anchors log).
//!
//! Whole-path is the point. A `starts_with("/feed/")` test would pass any
//! future artifact — including one carrying a user-derived selector — and that
//! is exactly how the address-blindness property erodes. Every pattern here is
//! matched against the ENTIRE path: a fixed literal, or a directory + an
//! 8-digit zero-padded index + a fixed suffix. Adding an artifact means adding
//! a pattern here, deliberately.
//!
//! Deltas from §2.8.1's eight patterns, both forced by §11:
//! - `/feed/anchors.ndjson` is IN: §11.2 makes the head-captured anchors log
//!   the artifact that grounds a snapshot (§11.3), so a client fetches it. It
//!   is parameterless and byte-identical for every user.
//! - `/feed/snapshots/{e:08}.anchor.json` is OUT: §11.1 measured that a proof
//!   at a snapshot's basis block cannot be obtained, so the per-snapshot proof
//!   sidecar does not exist. A client fetching one is a client following the
//!   superseded §1.3, and this list says so.

/// Human-readable form of the closed set, for assertion messages.
pub const PATTERNS: [&str; 8] = [
    "/feed/genesis.json",
    "/feed/manifest.json",
    "/feed/head.ndjson",
    "/feed/anchors.ndjson",
    "/feed/live",
    "/feed/epochs/{idx:08}.strk20e.zst",
    "/feed/epochs/{idx:08}.anchor.json",
    "/feed/snapshots/{e:08}.strk20s.zst",
];

/// Whole-path match against the closed set. `uri` is the request target,
/// query string included — a query string is never allowed on any of them.
pub fn is_allowed(uri: &str) -> bool {
    match uri {
        "/feed/genesis.json"
        | "/feed/manifest.json"
        | "/feed/head.ndjson"
        | "/feed/anchors.ndjson"
        | "/feed/live" => true,
        _ => {
            indexed(uri, "/feed/epochs/", ".strk20e.zst")
                || indexed(uri, "/feed/epochs/", ".anchor.json")
                || indexed(uri, "/feed/snapshots/", ".strk20s.zst")
        }
    }
}

/// `<dir><8 digits><suffix>` and nothing else.
fn indexed(uri: &str, dir: &str, suffix: &str) -> bool {
    let Some(rest) = uri.strip_prefix(dir) else {
        return false;
    };
    let Some(idx) = rest.strip_suffix(suffix) else {
        return false;
    };
    idx.len() == 8 && idx.bytes().all(|b| b.is_ascii_digit())
}

/// The epoch index of `/feed/epochs/{idx:08}.strk20e.zst`, if that is what
/// `uri` is.
pub fn epoch_index(uri: &str) -> Option<u64> {
    uri.strip_prefix("/feed/epochs/")
        .and_then(|r| r.strip_suffix(".strk20e.zst"))
        .filter(|i| i.len() == 8)
        .and_then(|i| i.parse().ok())
}

/// The epoch index of `/feed/snapshots/{e:08}.strk20s.zst`, if that is what
/// `uri` is.
pub fn snapshot_index(uri: &str) -> Option<u64> {
    uri.strip_prefix("/feed/snapshots/")
        .and_then(|r| r.strip_suffix(".strk20s.zst"))
        .filter(|i| i.len() == 8)
        .and_then(|i| i.parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn closed_set_is_whole_path() {
        assert!(is_allowed("/feed/manifest.json"));
        assert!(is_allowed("/feed/epochs/00000001.strk20e.zst"));
        assert!(is_allowed("/feed/snapshots/00001405.strk20s.zst"));
        // prefix-style matches are exactly what must NOT pass
        assert!(!is_allowed("/feed/"));
        assert!(!is_allowed("/feed/manifest.json?address=0x1"));
        assert!(!is_allowed("/feed/epochs/1.strk20e.zst"));
        assert!(!is_allowed("/feed/snapshots/00000001.anchor.json"));
        assert!(!is_allowed("/feed/notes/0xdeadbeef.json"));
        assert_eq!(epoch_index("/feed/epochs/00000007.strk20e.zst"), Some(7));
        assert_eq!(snapshot_index("/feed/snapshots/00000007.strk20s.zst"), Some(7));
    }
}
