//! The §1.5 verification ladder, ring by ring, each with a negative that only
//! that ring can catch.
//!
//! Why this file exists: before it, `verify_snapshot` was reachable from no
//! test in this crate — the e2e legs exercised ring 1 (a byte flip) and the
//! §11.3 reachability check, and rings 3, 4 and 5 were exercised by nothing at
//! all. Ring 5 in particular could be DELETED outright with the whole suite
//! still green, because the only adversarial fixture recomputed the root
//! consistently and so passed ring 5 on its way to being caught by
//! reachability. A ladder whose rungs are unfalsifiable is prose.
//!
//! Each test alters exactly one thing and asserts the error names the ring
//! that owns it, so a short-circuited or reordered ladder fails here.

use strk20_feed::manifest::ManifestSnapshot;
use strk20_feed::snapshot::{
    encode, snapshot_file_name, verify_snapshot, FeedIdentity, SnapSlot, Snapshot, SnapshotHeader,
    KIND_SNAPSHOT,
};
use strk20_feed::{compress, felt_from_hex, felt_hex, payload_sha256, Felt};

const CHAIN: &str = "SN_TEST";
const EPOCH: u64 = 3;
const BASIS: u64 = 63;
const EPOCH_HASH: &str = "b7c5f1a2d3e4f5061728394a5b6c7d8e9f0011223344556677889900aabbccdd";

fn f(s: &str) -> Felt {
    felt_from_hex(s).unwrap()
}

fn pool() -> Felt {
    f("0x40337b1af3c663e86e333bab5a4b28da8d4652a15a69beee2b677776ffe812a")
}

fn identity() -> FeedIdentity {
    FeedIdentity {
        chain_id: CHAIN.to_owned(),
        pool: pool(),
    }
}

fn slots() -> Vec<SnapSlot> {
    vec![
        SnapSlot { k: f("0x1"), v: f("0x2"), w: 9 },
        SnapSlot { k: f("0x2a"), v: f("0x3b"), w: 40 },
        SnapSlot { k: f("0xff00"), v: f("0xabc"), w: BASIS },
    ]
}

/// A snapshot whose declared root really is the root of its slot lines.
fn honest() -> Snapshot {
    let s = slots();
    let pairs: Vec<(Felt, Felt)> = s.iter().map(|x| (x.k, x.v)).collect();
    Snapshot {
        header: SnapshotHeader {
            v: 1,
            kind: KIND_SNAPSHOT.to_owned(),
            chain_id: CHAIN.to_owned(),
            pool: pool(),
            epoch: EPOCH,
            block: BASIS,
            epoch_hash: EPOCH_HASH.to_owned(),
            storage_root: strk20_feed::mpt::storage_root(&pairs),
            class: f("0x67dd"),
        },
        slots: s,
    }
}

/// The manifest entry an honest publisher writes for `snap`.
fn entry_for(snap: &Snapshot) -> (Vec<u8>, ManifestSnapshot) {
    let payload = encode(snap);
    let zst = compress(&payload);
    let entry = ManifestSnapshot {
        e: snap.header.epoch,
        block: snap.header.block,
        epoch_hash: snap.header.epoch_hash.clone(),
        file: snapshot_file_name(snap.header.epoch),
        hash: hex::encode(payload_sha256(&payload)),
        zst: hex::encode(payload_sha256(&zst)),
        bytes: zst.len() as u64,
        slots: snap.slots.len() as u64,
        storage_root: felt_hex(&snap.header.storage_root),
        // Offline rings 1-5 are about the file itself; the basis-block anchor
        // (§12 point 1) is checked by the client against the published sidecar,
        // which no offline ring can see.
        anchor: None,
        grounding: strk20_feed::manifest::GROUNDING_REACHABILITY.to_owned(),
    };
    (zst, entry)
}

/// Control: the honest file passes all five offline rings. Every negative
/// below is therefore about the alteration and not about a verifier that
/// refuses everything.
#[test]
fn an_honest_snapshot_passes_rings_one_through_five() {
    let snap = honest();
    let (zst, entry) = entry_for(&snap);
    let got = verify_snapshot(&zst, &entry, EPOCH_HASH, &identity()).expect("honest snapshot");
    assert_eq!(got, snap);
}

/// Ring 1 — the transport checksum is verified BEFORE decompression, so a
/// `.zst` whose bytes do not match `manifest.snapshot.zst` never reaches the
/// decompressor.
#[test]
fn ring1_transport_hash_mismatch() {
    let snap = honest();
    let (zst, mut entry) = entry_for(&snap);
    entry.zst = "0".repeat(64);
    let err = verify_snapshot(&zst, &entry, EPOCH_HASH, &identity()).unwrap_err().to_string();
    assert!(err.contains("FEED_HASH_MISMATCH"), "{err}");
    assert!(err.contains("sha256"), "the error must name both hashes: {err}");
}

/// Ring 2 — content identity. The `.zst` hash can be made to agree while the
/// payload inside is not the one the manifest names.
#[test]
fn ring2_content_hash_mismatch() {
    let snap = honest();
    let (zst, mut entry) = entry_for(&snap);
    entry.hash = "1".repeat(64);
    let err = verify_snapshot(&zst, &entry, EPOCH_HASH, &identity()).unwrap_err().to_string();
    assert!(err.contains("FEED_HASH_MISMATCH"), "{err}");
    assert!(err.contains("payload"), "ring 2 is about the payload, not the file: {err}");
}

/// Ring 3 — identity. A snapshot stamped with another chain or another pool is
/// refused before a single slot is applied, whatever its hashes say.
#[test]
fn ring3_chain_mismatch_is_named() {
    let mut snap = honest();
    snap.header.chain_id = "SN_MAIN".to_owned();
    let (zst, entry) = entry_for(&snap);
    let err = verify_snapshot(&zst, &entry, EPOCH_HASH, &identity()).unwrap_err().to_string();
    assert!(
        err.contains("CHAIN_MISMATCH") && err.contains("SN_MAIN") && err.contains(CHAIN),
        "the refusal must name BOTH chains (§8 leg t(i)): {err}"
    );
}

#[test]
fn ring3_pool_mismatch_is_named() {
    let mut snap = honest();
    snap.header.pool = f("0xdead");
    let (zst, entry) = entry_for(&snap);
    let err = verify_snapshot(&zst, &entry, EPOCH_HASH, &identity()).unwrap_err().to_string();
    assert!(err.contains("CHAIN_MISMATCH"), "{err}");
}

/// Ring 3 — the manifest and the header must agree about which epoch and block
/// this snapshot is of.
#[test]
fn ring3_header_and_manifest_must_agree_on_epoch_and_block() {
    let snap = honest();
    let (zst, mut entry) = entry_for(&snap);
    entry.block = BASIS - 1;
    let err = verify_snapshot(&zst, &entry, EPOCH_HASH, &identity()).unwrap_err().to_string();
    assert!(err.contains("FEED_MALFORMED"), "{err}");
}

/// Ring 3 — the declared slot count is part of the manifest, so a truncated
/// file cannot be passed off as a complete one by fixing the hashes.
#[test]
fn ring3_slot_count_must_match_the_manifest() {
    let snap = honest();
    let (zst, mut entry) = entry_for(&snap);
    entry.slots = 99;
    let err = verify_snapshot(&zst, &entry, EPOCH_HASH, &identity()).unwrap_err().to_string();
    assert!(err.contains("FEED_MALFORMED") && err.contains("99"), "{err}");
}

/// Ring 4 — the chain pin. `header.epoch_hash` is what keeps a
/// snapshot-started client on the ONE hash chain instead of starting a second
/// one, so a snapshot pinned to an epoch content hash we did not verify is a
/// broken chain, not a malformed file.
#[test]
fn ring4_epoch_hash_pin_is_enforced_against_the_manifest_epoch() {
    let mut snap = honest();
    snap.header.epoch_hash = "c".repeat(64);
    let (zst, entry) = entry_for(&snap);
    // `entry.epoch_hash` follows the header here, exactly as a forger would
    // write it: the value that decides is the hash of the epoch the client
    // ITSELF verified, passed in as `basis_epoch_hash`.
    let err = verify_snapshot(&zst, &entry, EPOCH_HASH, &identity()).unwrap_err().to_string();
    assert!(
        err.contains("hash chain broken") && err.contains(EPOCH_HASH),
        "FEED_CHAIN_BROKEN must name the epoch hash the client verified: {err}"
    );
}

/// Ring 5 — self-consistency of the slot set against the declared root.
///
/// This is the rung that could be deleted with the whole suite still green:
/// the adversarial e2e fixture recomputes `header.storage_root` consistently
/// and is caught by §11.3 reachability instead. Here the slot set and the
/// declared root genuinely disagree, so ONLY ring 5 stands between the client
/// and a slot set that is not the one the publisher declared.
#[test]
fn ring5_slot_set_must_reproduce_the_declared_root() {
    let mut snap = honest();
    let honest_root = snap.header.storage_root;
    snap.slots[1].v = f("0x3c"); // one value altered, root left as declared
    let (zst, entry) = entry_for(&snap);
    assert_eq!(
        entry.storage_root,
        felt_hex(&honest_root),
        "fixture: the manifest still carries the honest root, so the file is the thing \
         that disagrees"
    );
    let err = verify_snapshot(&zst, &entry, EPOCH_HASH, &identity()).unwrap_err().to_string();
    assert!(
        err.contains("SNAPSHOT_ROOT_MISMATCH"),
        "§1.5 ring 5: recomputing mpt::storage_root over the slot lines and comparing \
         with header.storage_root and manifest.snapshot.storage_root is the ONLY check \
         that catches a slot set which does not match its own declared root: {err}"
    );
}

/// Ring 5, other half — the header and the manifest must agree with each other
/// as well as with the recomputation, so a forger cannot fix one and leave the
/// other.
#[test]
fn ring5_manifest_root_must_agree_with_the_header() {
    let snap = honest();
    let (zst, mut entry) = entry_for(&snap);
    entry.storage_root = felt_hex(&f("0xbad"));
    let err = verify_snapshot(&zst, &entry, EPOCH_HASH, &identity()).unwrap_err().to_string();
    assert!(err.contains("SNAPSHOT_ROOT_MISMATCH"), "{err}");
}

// ------------------------------------------------------------ ring 1's cap

/// §1.5 ring 1's output cap (R-I). A passing transport hash proves nothing
/// about how far the frame expands — the same server authors both the file and
/// the manifest that names its sha256 — so a ~100 KB `.zst` that expands to
/// tens of GB passes ring 1 and then allocates until the process dies. On the
/// browser target A1 exists to serve that is a tab crash, not an error.
#[test]
fn decompression_is_capped_and_the_failure_is_named() {
    let bomb = compress(&vec![0u8; 4096]);
    assert!(
        bomb.len() < 256,
        "fixture: the compressed form must be far smaller than its output ({} bytes)",
        bomb.len()
    );
    let err = strk20_feed::decompress_capped(&bomb, 1024, "snapshot")
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("DECOMPRESS_LIMIT") && err.contains("snapshot") && err.contains("1024"),
        "the cap failure must be named DECOMPRESS_LIMIT and carry {{artifact, cap}}: {err}"
    );
    // Exactly at the cap is not over it.
    let exact = compress(&vec![7u8; 1024]);
    assert_eq!(
        strk20_feed::decompress_capped(&exact, 1024, "snapshot").unwrap(),
        vec![7u8; 1024]
    );
    // And the default path still decompresses an ordinary artifact.
    assert_eq!(strk20_feed::decompress(&exact).unwrap(), vec![7u8; 1024]);
}
