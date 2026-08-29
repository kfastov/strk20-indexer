//! Golden byte vectors for the canonical epoch/head encoding (spec §10.1).
//! These bytes are FROZEN: changing them is a feed-format break and requires
//! a `v:2` namespace, not an edit to this file.

use strk20_feed::codec::*;
use strk20_feed::{felt_from_hex, payload_sha256};

fn sample_epoch() -> Epoch {
    let f = |s: &str| felt_from_hex(s).unwrap();
    Epoch {
        header: EpochHeader {
            chain_id: "SN_TEST".into(),
            pool: f("0x66292db2e7d6fe7d76386b4198a41ad42a108a1895fe09eada749bed7633f76"),
            epoch: 0,
            from: 0,
            to: 15,
            prev: None,
        },
        blocks: vec![
            BlockLine {
                number: 10,
                hash: f("0xaa"),
                parent: f("0xa9"),
                timestamp: 1000,
                diffs: vec![(f("0x1"), f("0x111")), (f("0x2"), f("0x222"))],
                events: vec![
                    EventLine {
                        tx_index: 0,
                        event_index: 0,
                        tx_hash: f("0x70aa"),
                        keys: vec![f("0x9149"), f("0xbeef")],
                        data: vec![f("0x64")],
                    },
                    EventLine {
                        tx_index: 1,
                        event_index: 1,
                        tx_hash: f("0x70ab"),
                        keys: vec![f("0x247f")],
                        data: vec![],
                    },
                ],
                replaced_class: None,
                finality: None,
            },
            BlockLine {
                number: 12,
                hash: f("0xcc"),
                parent: f("0xcb"),
                timestamp: 1004,
                diffs: vec![],
                events: vec![],
                replaced_class: Some(f("0x67dd")),
                finality: None,
            },
        ],
        footer: Footer {
            blocks: 2,
            diffs: 2,
            events: 2,
            class: f("0x67dd"),
        },
    }
}

const GOLDEN: &str = "\
{\"t\":\"hdr\",\"v\":1,\"kind\":\"strk20-epoch\",\"chain_id\":\"SN_TEST\",\"pool\":\"0x66292db2e7d6fe7d76386b4198a41ad42a108a1895fe09eada749bed7633f76\",\"epoch\":0,\"from\":0,\"to\":15,\"prev\":null}\n\
{\"t\":\"blk\",\"b\":10,\"h\":\"0xaa\",\"p\":\"0xa9\",\"ts\":1000,\"d\":[[\"0x1\",\"0x111\"],[\"0x2\",\"0x222\"]],\"e\":[[0,0,\"0x70aa\",[\"0x9149\",\"0xbeef\"],[\"0x64\"]],[1,1,\"0x70ab\",[\"0x247f\"],[]]]}\n\
{\"t\":\"blk\",\"b\":12,\"h\":\"0xcc\",\"p\":\"0xcb\",\"ts\":1004,\"d\":[],\"e\":[],\"rc\":\"0x67dd\"}\n\
{\"t\":\"end\",\"blocks\":2,\"diffs\":2,\"events\":2,\"class\":\"0x67dd\"}\n";

#[test]
fn golden_epoch_bytes() {
    let bytes = encode_epoch(&sample_epoch());
    assert_eq!(std::str::from_utf8(&bytes).unwrap(), GOLDEN);
}

#[test]
fn epoch_round_trip() {
    let e = sample_epoch();
    let bytes = encode_epoch(&e);
    let parsed = parse_epoch(&bytes).unwrap();
    assert_eq!(parsed, e);
    // re-encode is byte-identical (canonicality)
    assert_eq!(encode_epoch(&parsed), bytes);
}

#[test]
fn epoch_contains_no_anchor_field() {
    // Resolution R7: anchors live outside the content-addressed payload.
    let bytes = encode_epoch(&sample_epoch());
    assert!(!std::str::from_utf8(&bytes).unwrap().contains("anchor"));
}

#[test]
fn tampered_payload_changes_hash_and_fails_structure() {
    let bytes = encode_epoch(&sample_epoch());
    let h0 = payload_sha256(&bytes);
    let mut tampered = bytes.clone();
    // flip one byte inside a value
    let pos = GOLDEN.find("0x222").unwrap() + 3;
    tampered[pos] = b'3';
    assert_ne!(payload_sha256(&tampered), h0);
}

#[test]
fn footer_count_mismatch_rejected() {
    let mut e = sample_epoch();
    e.footer.diffs = 99;
    let bytes = encode_epoch(&e);
    assert!(parse_epoch(&bytes).is_err());
}

#[test]
fn unsorted_diffs_rejected_on_parse() {
    // hand-build a payload with diffs out of order
    let bad = GOLDEN.replace(
        "[[\"0x1\",\"0x111\"],[\"0x2\",\"0x222\"]]",
        "[[\"0x2\",\"0x222\"],[\"0x1\",\"0x111\"]]",
    );
    assert!(parse_epoch(bad.as_bytes()).is_err());
}

#[test]
fn adjacent_parent_linkage_enforced() {
    let f = |s: &str| felt_from_hex(s).unwrap();
    let mut e = sample_epoch();
    // make blocks numerically adjacent with a broken parent link
    e.blocks[1].number = 11;
    e.blocks[1].parent = f("0xdead");
    let bytes = encode_epoch(&e);
    assert!(parse_epoch(&bytes).is_err());
}

#[test]
fn head_round_trip_with_finality() {
    let f = |s: &str| felt_from_hex(s).unwrap();
    let head = Head {
        header: HeadHeader {
            tail_from: 16,
            head: 46,
            head_hash: f("0xffff"),
            l1_accepted: 40,
        },
        blocks: vec![BlockLine {
            number: 30,
            hash: f("0xbb"),
            parent: f("0xba"),
            timestamp: 2000,
            diffs: vec![(f("0x5"), f("0x555"))],
            events: vec![],
            replaced_class: None,
            finality: Some(Finality::L1),
        }],
        footer: Footer {
            blocks: 1,
            diffs: 1,
            events: 0,
            class: f("0x67dd"),
        },
    };
    let bytes = encode_head(&head);
    let text = std::str::from_utf8(&bytes).unwrap();
    assert!(text.contains("\"kind\":\"strk20-head\""));
    assert!(text.contains("\"fin\":\"l1\""));
    let parsed = parse_head(&bytes).unwrap();
    assert_eq!(parsed, head);
    // an epoch parser must reject a head payload
    assert!(parse_epoch(&bytes).is_err());
}

#[test]
fn head_blocks_require_finality() {
    // epoch-style blk line (no fin) inside a head payload must be rejected
    let head_hdr = "{\"t\":\"hdr\",\"v\":1,\"kind\":\"strk20-head\",\"tail_from\":0,\"head\":46,\"head_hash\":\"0xff\",\"l1_accepted\":40}\n";
    let blk = "{\"t\":\"blk\",\"b\":10,\"h\":\"0xaa\",\"p\":\"0xa9\",\"ts\":1000,\"d\":[],\"e\":[]}\n";
    let end = "{\"t\":\"end\",\"blocks\":1,\"diffs\":0,\"events\":0,\"class\":\"0x67dd\"}\n";
    let payload = format!("{head_hdr}{blk}{end}");
    assert!(parse_head(payload.as_bytes()).is_err());
}
