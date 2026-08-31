//! Golden byte vector for the canonical anchors.ndjson encoding (spec §4.5).
//! These bytes are FROZEN: anchors.ndjson is not content-addressed, so its
//! only guarantee is that two independent operators emit identical bytes for
//! the same anchor set. Changing them is a feed-format break.

use strk20_feed::anchors::{encode_anchors, parse_anchors, AnchorRecord};
use strk20_feed::felt_from_hex;

fn sample() -> Vec<AnchorRecord> {
    let f = |s: &str| felt_from_hex(s).unwrap();
    vec![
        AnchorRecord {
            block: 15,
            block_hash: f("0x00aa"),
            storage_root: f("0x1234"),
            class: f("0x67dd"),
        },
        AnchorRecord {
            block: 14_128_517,
            block_hash: f("0xbeef"),
            storage_root: f("0x0"),
            class: f("0x67dddd89d80fedadc06b6f160798f94800a4a70164e5a24301cd0d6076b554d"),
        },
    ]
}

const GOLDEN: &str = "\
{\"block\":15,\"block_hash\":\"0xaa\",\"storage_root\":\"0x1234\",\"class\":\"0x67dd\"}\n\
{\"block\":14128517,\"block_hash\":\"0xbeef\",\"storage_root\":\"0x0\",\"class\":\"0x67dddd89d80fedadc06b6f160798f94800a4a70164e5a24301cd0d6076b554d\"}\n";

#[test]
fn golden_anchor_bytes() {
    let bytes = encode_anchors(&sample()).unwrap();
    assert_eq!(std::str::from_utf8(&bytes).unwrap(), GOLDEN);
}

#[test]
fn round_trip_is_byte_stable() {
    let bytes = encode_anchors(&sample()).unwrap();
    let parsed = parse_anchors(&bytes).unwrap();
    assert_eq!(parsed, sample());
    assert_eq!(encode_anchors(&parsed).unwrap(), bytes);
}

#[test]
fn empty_log_is_empty_bytes() {
    assert!(encode_anchors(&[]).unwrap().is_empty());
    assert!(parse_anchors(b"").unwrap().is_empty());
}

#[test]
fn non_ascending_blocks_are_rejected() {
    let bad = format!("{}{}", GOLDEN.lines().next().unwrap(), "\n").repeat(2);
    let err = parse_anchors(bad.as_bytes()).unwrap_err().to_string();
    assert!(err.contains("does not follow"), "{err}");
}

#[test]
fn a_missing_trailing_newline_is_rejected() {
    let bad = GOLDEN.trim_end_matches('\n');
    assert!(parse_anchors(bad.as_bytes()).is_err());
}

#[test]
fn a_blank_line_is_rejected() {
    let bad = format!("\n{GOLDEN}");
    assert!(parse_anchors(bad.as_bytes()).is_err());
}

/// The encoder runs inside the indexer's cut path: an unordered set must be an
/// error the daemon can log, never a panic that aborts it.
#[test]
fn encoding_an_unordered_set_errors_instead_of_panicking() {
    let mut bad = sample();
    bad.reverse();
    let err = encode_anchors(&bad).unwrap_err().to_string();
    assert!(err.contains("ascending"), "{err}");
    let dup = vec![sample()[0], sample()[0]];
    assert!(encode_anchors(&dup).is_err(), "duplicate blocks must be rejected");
}
