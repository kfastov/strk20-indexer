//! Golden byte vector for the canonical snapshot payload (consumer-path.md
//! §1.2). These bytes are FROZEN: the payload's sha256 is the snapshot's
//! content identity and two independent operators must publish the same file,
//! so changing them is a feed-format break.

use strk20_feed::felt_from_hex;
use strk20_feed::snapshot::{encode, parse, SnapSlot, Snapshot, SnapshotHeader, KIND_SNAPSHOT};

fn sample() -> Snapshot {
    let f = |s: &str| felt_from_hex(s).unwrap();
    Snapshot {
        header: SnapshotHeader {
            v: 1,
            kind: KIND_SNAPSHOT.to_owned(),
            chain_id: "SN_MAIN".to_owned(),
            pool: f("0x040337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a"),
            epoch: 1405,
            block: 14_059_999,
            epoch_hash: "a".repeat(64),
            storage_root: f("0x00beef"),
            class: f("0x67dd"),
        },
        slots: vec![
            SnapSlot {
                k: f("0x1"),
                v: f("0x2"),
                w: 9,
            },
            SnapSlot {
                k: f("0xff00"),
                v: f("0x0abc"),
                w: 14_059_999,
            },
        ],
    }
}

const GOLDEN: &str = "\
{\"t\":\"hdr\",\"v\":1,\"kind\":\"strk20-snapshot\",\"chain_id\":\"SN_MAIN\",\"pool\":\"0x40337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a\",\"epoch\":1405,\"block\":14059999,\"epoch_hash\":\"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\",\"storage_root\":\"0xbeef\",\"class\":\"0x67dd\"}\n\
{\"t\":\"s\",\"k\":\"0x1\",\"v\":\"0x2\",\"w\":9}\n\
{\"t\":\"s\",\"k\":\"0xff00\",\"v\":\"0xabc\",\"w\":14059999}\n\
{\"t\":\"end\",\"slots\":2}\n";

#[test]
fn golden_snapshot_bytes() {
    assert_eq!(std::str::from_utf8(&encode(&sample())).unwrap(), GOLDEN);
}

#[test]
fn round_trip_is_byte_stable() {
    let bytes = encode(&sample());
    let parsed = parse(&bytes).unwrap();
    assert_eq!(parsed, sample());
    assert_eq!(encode(&parsed), bytes);
}

#[test]
fn slot_lines_must_ascend_by_the_32_byte_be_key() {
    let mut doc = sample();
    doc.slots.reverse();
    let err = parse(&encode(&doc)).unwrap_err().to_string();
    assert!(err.contains("does not follow"), "{err}");
}

#[test]
fn the_footer_count_must_match() {
    let bad = GOLDEN.replace("\"slots\":2", "\"slots\":3");
    let err = parse(bad.as_bytes()).unwrap_err().to_string();
    assert!(err.contains("declares 3 slots"), "{err}");
}

/// Per-note `block_number` is derived from `w`, so a write above the basis is
/// a claim the snapshot cannot possibly support.
#[test]
fn a_write_block_above_the_basis_is_rejected() {
    let bad = GOLDEN.replace("\"w\":14059999", "\"w\":14060000");
    let err = parse(bad.as_bytes()).unwrap_err().to_string();
    assert!(err.contains("above the basis"), "{err}");
}

/// Cairo map semantics: absent IS zero, so an explicit zero slot would make
/// two encodings of the same state.
#[test]
fn zero_valued_slots_are_rejected() {
    let bad = GOLDEN.replace("\"v\":\"0x2\"", "\"v\":\"0x0\"");
    let err = parse(bad.as_bytes()).unwrap_err().to_string();
    assert!(err.contains("zero-valued"), "{err}");
}

#[test]
fn a_missing_trailing_newline_is_rejected() {
    let bad = GOLDEN.trim_end_matches('\n');
    let err = parse(bad.as_bytes()).unwrap_err().to_string();
    assert!(err.contains("newline"), "{err}");
}
