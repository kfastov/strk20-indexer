//! Closed, whole-path allowlist for public feed requests in the native tests.
//! Each entry identifies public data, never a wallet selector. Match the whole
//! path so a new user-derived route cannot pass through a broad prefix check.
//! Keep changes consistent with docs/spec/architecture.md#http-interfaces.

/// Human-readable form of the closed set, for assertion messages.
pub const PATTERNS: [&str; 10] = [
    "/feed/genesis.json",
    "/feed/manifest.json",
    "/feed/head.ndjson",
    "/feed/anchors.ndjson",
    "/feed/live",
    "/feed/proofs/{block}",
    "/feed/epochs/{idx:08}.strk20e.zst",
    "/feed/epochs/{idx:08}.anchor.json",
    "/feed/snapshots/{e:08}.strk20s.zst",
    "/feed/snapshots/{e:08}.anchor.json",
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
            uri.strip_prefix("/feed/proofs/").is_some_and(|b| !b.is_empty() && b.len() <= 20 && b.bytes().all(|c| c.is_ascii_digit()))
                || indexed(uri, "/feed/epochs/", ".strk20e.zst")
                || indexed(uri, "/feed/epochs/", ".anchor.json")
                || indexed(uri, "/feed/snapshots/", ".strk20s.zst")
                || indexed(uri, "/feed/snapshots/", ".anchor.json")
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
        // A snapshot proof sidecar is a public, epoch-indexed artifact.
        assert!(is_allowed("/feed/snapshots/00000001.anchor.json"));
        // prefix-style matches are exactly what must NOT pass
        assert!(!is_allowed("/feed/"));
        assert!(!is_allowed("/feed/manifest.json?address=0x1"));
        assert!(!is_allowed("/feed/epochs/1.strk20e.zst"));
        assert!(!is_allowed("/feed/snapshots/1.anchor.json"));
        assert!(!is_allowed("/feed/snapshots/00000001.anchor.json?key=0x1"));
        assert!(!is_allowed("/feed/notes/0xdeadbeef.json"));
        assert_eq!(epoch_index("/feed/epochs/00000007.strk20e.zst"), Some(7));
        assert_eq!(snapshot_index("/feed/snapshots/00000007.strk20s.zst"), Some(7));
    }
}
