//! Snapshot legs (consumer-path.md §A1, as amended by §11).
//!
//! Why snapshots exist at all is a measurement, not a preference: a cold
//! client fold of full mainnet history takes 6.0 s natively over 515 epochs
//! (docs/research/live/live-run-findings.md) and WASM is slower, so a browser
//! client cannot replay history per page load.
//!
//! §11 supersedes §1.3/§1.4 step 4 and these legs are written to the amended
//! design, NOT to the superseded one:
//!
//! - There is no per-snapshot proof sidecar. A proof at a snapshot's basis
//!   block cannot be obtained from any public provider — the getStorageProof
//!   window is ~1024 blocks and a basis block is thousands of blocks old at
//!   cut time (0 of 515 epochs in a completed mainnet backfill carry an
//!   anchor). A design that requires one publishes no snapshots at all.
//! - Publication gate (§11.3): publish when the mirror's root matched the
//!   chain at the most recent `anchors.ndjson` capture at some block A >= the
//!   basis, with no verified mismatch since.
//! - Client grounding (§11.3): REACHABILITY. Fold snapshot(b), apply
//!   everything the feed carries for b+1..A, recompute the storage root and
//!   compare with the anchors.ndjson record at A. A match attests the
//!   snapshot AND the intervening epochs.
//! - Trust grade is stated honestly: an anchor is a SERVER ASSERTION until
//!   the client checks it against its own RPC (ring 6), which works for
//!   recent anchors because recent is what the proof window serves.
//!
//! S1 snapshot bytes are canonical and identical across independent backfills
//! S2 a snapshot-started client's discovery == a genesis-replayed client's
//! S3 reachability: snapshot + feed to an anchored block reproduces its root
//! S4 tampered / unreachable snapshots are rejected by name (negative of S3)
//! S5 the publication gate: no anchor >= basis, no snapshot; it resumes
//! S6 retention keeps the newest N without 404ing the previous manifest
//!
//! **§12 correction (2026-08-31, same day).** The premise above — that a proof
//! for the basis block cannot be obtained — was measured against an
//! aggregating endpoint with single attempts and is RETRACTED: deep proofs
//! answer for any block on retry (research/live/proof-window.md §1). §1.3's
//! per-snapshot anchor sidecar is reinstated as the PRIMARY grounding, and the
//! reachability check above is demoted to the fallback for snapshots whose
//! basis anchor could not be obtained — and kept, because it is the only thing
//! that catches an internally-consistent forgery (S4(ii)) and it validates the
//! intervening epochs as well. The legs above keep their meaning: their
//! fixture has a narrow proof window, which is exactly the fallback world.
//!
//! S8 the basis-block anchor is obtained (with retries) and grounds the
//!    snapshot: sidecar + manifest anchor, and the client uses it
//! S9 no basis anchor obtainable => publication still happens on reachability,
//!    and the manifest says which grounding was used

use discovery_core::privacy_pool::types::SecretFelt;
use discovery_core::storage_backend::MockBackend;
use e2e_tests::bins::{bin, ensure_built, pick_free_port, run_capture, spawn_with_logs, ChildGuard};
use e2e_tests::chain::{FixtureChain, FxEvent, ENC_NOTE_CREATED_SELECTOR, NOTE_USED_SELECTOR};
use e2e_tests::fixture::load_devnet_fixture;
use e2e_tests::oracle::{self, MintedNote};
use e2e_tests::proxy::RecordingProxy;
use e2e_tests::rpc_server::{FaultSpec, FixtureRpc};
use e2e_tests::{feed_urls, snapshot_fmt};
use serde_json::Value;
use starknet_types_core::felt::Felt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::Duration;
use strk20_feed::felt_hex;

const CHAIN_ID: &str = "SN_TEST";
const GENESIS_BLOCK: u64 = 10;
const EPOCH_SIZE: u64 = 16;

/// The snapshot the fixture publishes: epoch 1 = blocks [16, 31].
const BASIS_EPOCH: u64 = 1;
const BASIS_BLOCK: u64 = 31;
const SNAPSHOT_FILE: &str = "snapshots/00000001.strk20s.zst";

/// An endpoint whose trie retention really is narrow, scaled to the fixture.
/// It is deliberately NARROWER than head − basis (46 − 31 = 15), so no retry
/// count can obtain a proof at the basis and every anchor these legs rely on
/// comes from the head-side capture of §11.2. (The "~1024 blocks measured on
/// mainnet" reading of this number is retracted — proof-window.md §3 — but a
/// node that cannot serve deep proofs is still a real deployment.)
const PROOF_WINDOW: u64 = 4;

/// Pre-basis note blocks and the post-basis (head tail) note block.
const PRE_MINT_BLOCK: u64 = 28;
const PRE_SPEND_BLOCK: u64 = 30;
const TAIL_MINT_BLOCK: u64 = 46;
const TAIL_SPEND_BLOCK: u64 = 47;

// The fixture's whole point, checked at compile time: the spent note is
// created AND spent below the basis (so a snapshot-started client sees no
// NoteUsed for it), and one note lands above the basis (so the seam between
// snapshot slots and feed epochs is exercised rather than assumed).
const _: () = assert!(PRE_MINT_BLOCK <= BASIS_BLOCK && PRE_SPEND_BLOCK <= BASIS_BLOCK);
const _: () = assert!(TAIL_MINT_BLOCK > BASIS_BLOCK && TAIL_SPEND_BLOCK > BASIS_BLOCK);

// ------------------------------------------------------------------ harness

fn base_args(dir: &Path, url: &str, pool_hex: &str) -> Vec<String> {
    vec![
        "--db".into(),
        dir.join("strk20.db").display().to_string(),
        "--feed-dir".into(),
        dir.join("feed").display().to_string(),
        "--rpc-url".into(),
        url.to_owned(),
        "--rpc-fallback".into(),
        url.to_owned(),
        "--pool".into(),
        pool_hex.to_owned(),
        "--chain-id".into(),
        CHAIN_ID.into(),
        "--genesis-block".into(),
        GENESIS_BLOCK.to_string(),
        "--epoch-size".into(),
        EPOCH_SIZE.to_string(),
        "--chunk-size".into(),
        "5".into(),
    ]
}

fn backfill(dir: &Path, url: &str, pool_hex: &str) -> (String, String, bool) {
    let mut cmd = Command::new(bin("strk20"));
    cmd.arg("backfill").args(base_args(dir, url, pool_hex));
    run_capture(cmd, false)
}

fn feed_dir(dir: &Path) -> PathBuf {
    dir.join("feed")
}

fn read_manifest(dir: &Path) -> Value {
    let bytes = std::fs::read(feed_dir(dir).join("manifest.json")).expect("read manifest.json");
    serde_json::from_slice(&bytes).expect("manifest.json is JSON")
}

fn sha256_hex(bytes: &[u8]) -> String {
    hex::encode(strk20_feed::payload_sha256(bytes))
}

/// Run `strk20-sync sync`, returning the parsed report (or the stderr under
/// `"error"`) and whether the process succeeded.
fn sync_with(
    dir: &Path,
    feed: &str,
    address: &Felt,
    key_hex: &str,
    db: &str,
    extra: &[&str],
) -> (Value, bool) {
    let key_path = dir.join(format!("{db}.key"));
    std::fs::write(&key_path, key_hex).unwrap();
    let mut cmd = Command::new(bin("strk20-sync"));
    cmd.arg("sync")
        .args(["--feed", feed])
        .args(["--address", &felt_hex(address)])
        .args(["--key-file", &key_path.display().to_string()])
        .args(["--db", &dir.join(db).display().to_string()])
        .args(extra)
        .arg("--json");
    let (stdout, stderr, ok) = run_capture(cmd, false);
    assert!(
        !stderr.contains("unexpected argument") && !stderr.contains("unrecognized"),
        "strk20-sync must accept {extra:?} (spec §1.7 / §1.5):\n{stderr}"
    );
    let report = if ok {
        serde_json::from_str(&stdout).unwrap_or_else(|e| {
            panic!("sync --json must print a report ({e})\nstdout:\n{stdout}\nstderr:\n{stderr}")
        })
    } else {
        serde_json::json!({ "error": format!("{stdout}\n{stderr}") })
    };
    (report, ok)
}

fn verify_anchors(feed: &str, db: &Path) -> (String, String, bool) {
    let mut cmd = Command::new(bin("strk20-sync"));
    cmd.arg("verify-anchors")
        .args(["--feed", feed])
        .args(["--db", &db.display().to_string()])
        .arg("--json");
    run_capture(cmd, false)
}

/// Published anchor records, as `(block, storage_root)`.
fn published_anchors(dir: &Path) -> Vec<(u64, Felt)> {
    let path = feed_dir(dir).join("anchors.ndjson");
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    text.lines()
        .map(|l| {
            let v: Value = serde_json::from_str(l).expect("anchor line is JSON");
            (
                v["block"].as_u64().expect("anchor block"),
                Felt::from_hex(v["storage_root"].as_str().expect("anchor storage_root")).unwrap(),
            )
        })
        .collect()
}

/// The pool slot set as of `block`, zero values excluded (Cairo map
/// semantics), sorted the way the wire format requires.
fn expected_slots(chain: &FixtureChain, block: u64) -> Vec<(Felt, Felt)> {
    let mut set: Vec<(Felt, Felt)> = chain
        .state_at(block)
        .into_iter()
        .filter(|(_, v)| *v != Felt::ZERO)
        .collect();
    set.sort_by_key(|(k, _)| k.to_bytes_be());
    set
}

fn snapshot_payload(dir: &Path) -> Vec<u8> {
    let zst = std::fs::read(feed_dir(dir).join(SNAPSHOT_FILE))
        .unwrap_or_else(|e| panic!("read {SNAPSHOT_FILE}: {e}"));
    strk20_feed::decompress(&zst).expect("snapshot file is zstd")
}

async fn wait_for<F: Fn() -> bool>(what: &str, timeout: Duration, f: F) {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if f() {
            return;
        }
        if std::time::Instant::now() >= deadline {
            panic!("timed out waiting for {what}");
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

// ------------------------------------------------------------ fixture chain

struct Seeded {
    chain: FixtureChain,
    bob: Felt,
    bob_key: Felt,
    pool_hex: String,
    /// minted pre-basis, spent pre-basis: its NoteUsed event is below the
    /// snapshot's history floor, so spent-state can only come from the
    /// nullifier SLOT the snapshot carries.
    pre_spent: MintedNote,
    /// minted pre-basis, unspent at round 1
    pre_live: MintedNote,
    /// minted post-basis, in the head tail
    tail: MintedNote,
}

/// A chain whose note population straddles the snapshot basis in every way
/// that matters: created-and-spent below it, created below it, created above
/// it. The spend below the basis is the leg the whole item turns on — a
/// snapshot carries slots and no events, so a snapshot-started client sees no
/// `NoteUsed` for it and must reach the same conclusion from nullifier slots.
async fn seed_chain() -> Seeded {
    let fixture = load_devnet_fixture();
    let bob = fixture.constants.bob_address;
    let alice = fixture.constants.alice_address;
    let bob_key = fixture.constants.bob_viewing_key;
    let strk = fixture.constants.strk_token;

    let plain = MockBackend::new(fixture.slots.clone());
    let bob_plain = oracle::incoming(&plain, bob, &SecretFelt::new(bob_key)).await;
    let ck = oracle::channel_key_of(&bob_plain, &alice);
    let base_index = bob_plain
        .cursor
        .channels
        .get(&alice)
        .and_then(|c| c.subchannels.get(&strk))
        .and_then(|s| s.total_n_notes)
        .expect("fixture subchannel note total");

    let secret = SecretFelt::new(bob_key);
    let pre_spent = oracle::mint_note(&ck, strk, base_index, 111, &secret);
    let pre_live = oracle::mint_note(&ck, strk, base_index + 1, 222, &secret);
    let tail = oracle::mint_note(&ck, strk, base_index + 2, 333, &secret);

    let enc = Felt::from_hex(ENC_NOTE_CREATED_SELECTOR).unwrap();
    let used = Felt::from_hex(NOTE_USED_SELECTOR).unwrap();
    let mut chain = FixtureChain::build(&fixture);

    chain.add_note_block(
        PRE_MINT_BLOCK,
        pre_spent.slot,
        pre_spent.packed_value,
        FxEvent {
            keys: vec![enc, pre_spent.note_id],
            data: vec![pre_spent.packed_value],
        },
    );
    chain.add_note_block(
        PRE_SPEND_BLOCK,
        pre_live.slot,
        pre_live.packed_value,
        FxEvent {
            keys: vec![enc, pre_live.note_id],
            data: vec![pre_live.packed_value],
        },
    );
    chain.add_note_block(
        PRE_SPEND_BLOCK,
        discovery_core::privacy_pool::storage_slots::nullifiers(pre_spent.nullifier),
        Felt::ONE,
        FxEvent {
            keys: vec![used, pre_spent.nullifier],
            data: vec![],
        },
    );
    chain.add_note_block(
        TAIL_MINT_BLOCK,
        tail.slot,
        tail.packed_value,
        FxEvent {
            keys: vec![enc, tail.note_id],
            data: vec![tail.packed_value],
        },
    );
    chain.head = TAIL_MINT_BLOCK;

    Seeded {
        pool_hex: felt_hex(&fixture.constants.contract_address),
        chain,
        bob,
        bob_key,
        pre_spent,
        pre_live,
        tail,
    }
}

fn rpc_for(chain: &FixtureChain) -> FixtureRpc {
    FixtureRpc::with_faults(
        chain.clone(),
        CHAIN_ID,
        FaultSpec {
            proof_window: Some(PROOF_WINDOW),
            ..Default::default()
        },
    )
}

/// Backfill a fresh dir. Asserts only what §11.2 already delivers — the
/// head-captured anchors log — so a test can check its own fixture
/// preconditions before reaching for the snapshot.
async fn backfilled_feed(seed: &Seeded) -> (FixtureRpc, tempfile::TempDir) {
    let rpc = rpc_for(&seed.chain);
    let addr = rpc.serve().await;
    let url = format!("http://{addr}/");
    let dir = tempfile::tempdir().unwrap();
    let (out, err, ok) = backfill(dir.path(), &url, &seed.pool_hex);
    assert!(ok, "backfill failed\nstdout:\n{out}\nstderr:\n{err}");
    assert!(
        !published_anchors(dir.path()).is_empty(),
        "§11.2: the head-side capture must publish anchors.ndjson; without one the \
         snapshot gate can never be met"
    );
    (rpc, dir)
}

fn assert_snapshot_published(dir: &Path) {
    let manifest = read_manifest(dir);
    let anchors = published_anchors(dir);
    assert!(
        !anchors.is_empty(),
        "§11.2: the head-side capture must publish anchors.ndjson; without one the \
         snapshot gate can never be met"
    );
    assert!(
        !manifest["snapshot"].is_null() && manifest.get("snapshot").is_some(),
        "§A1 + §11.3: an anchor exists at block {:?} >= basis {BASIS_BLOCK} with no \
         mismatch since, so the gate is MET and the cutter must publish a snapshot for \
         the newest cut epoch. manifest.snapshot is {}\n\
         Note the gate is a CONDITION, not a step of one cut batch: the anchor that \
         satisfies it is captured at head, after the batch that cut epoch {BASIS_EPOCH}. \
         Publishing only inside the cut batch that produced the epoch means never \
         publishing at all.",
        anchors.iter().map(|(b, _)| *b).collect::<Vec<_>>(),
        manifest["snapshot"]
    );
    assert!(
        feed_dir(dir).join(SNAPSHOT_FILE).exists(),
        "manifest names a snapshot but {SNAPSHOT_FILE} is not in the feed dir"
    );
}

// ------------------------------------------------------------------- S1

/// S1 — the snapshot payload is canonical §1.2 bytes and a pure function of
/// chain data, so two independent operators publish the same file.
///
/// The canonicality oracle is an INDEPENDENT encoder in the test harness
/// (`snapshot_fmt`), not the product encoder: comparing the product against
/// itself would prove nothing about the frozen format.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s1_snapshot_bytes_are_canonical_and_reproducible() {
    ensure_built();
    let seed = seed_chain().await;
    let rpc = rpc_for(&seed.chain);
    let addr = rpc.serve().await;
    let url = format!("http://{addr}/");

    let dir = tempfile::tempdir().unwrap();
    let (out, err, ok) = backfill(dir.path(), &url, &seed.pool_hex);
    assert!(ok, "backfill failed\nstdout:\n{out}\nstderr:\n{err}");
    assert_snapshot_published(dir.path());

    let manifest = read_manifest(dir.path());
    let snap = manifest["snapshot"].clone();

    // ---- manifest entry (§1.8), minus the superseded `anchor` object
    assert_eq!(snap["e"].as_u64(), Some(BASIS_EPOCH), "snapshot.e: {snap}");
    assert_eq!(snap["block"].as_u64(), Some(BASIS_BLOCK), "snapshot.block: {snap}");
    assert_eq!(
        snap["file"].as_str(),
        Some(SNAPSHOT_FILE),
        "snapshot.file must name the published path: {snap}"
    );

    let zst = std::fs::read(feed_dir(dir.path()).join(SNAPSHOT_FILE)).unwrap();
    assert_eq!(
        snap["zst"].as_str(),
        Some(sha256_hex(&zst).as_str()),
        "manifest.snapshot.zst must be the sha256 of the .zst file (the transport \
         checksum verified BEFORE decompression, R-I)"
    );
    assert_eq!(
        snap["bytes"].as_u64(),
        Some(zst.len() as u64),
        "manifest.snapshot.bytes must be the compressed file size"
    );
    let payload = strk20_feed::decompress(&zst).expect("snapshot file is zstd");
    assert_eq!(
        snap["hash"].as_str(),
        Some(sha256_hex(&payload).as_str()),
        "content identity is sha256 over the UNCOMPRESSED payload (§1.2)"
    );

    // ---- canonical bytes
    let doc = snapshot_fmt::parse(&payload).unwrap_or_else(|e| {
        panic!("snapshot payload does not parse as §1.2 NDJSON: {e}");
    });
    let reencoded = snapshot_fmt::encode(&doc);
    assert_eq!(
        String::from_utf8_lossy(&payload),
        String::from_utf8_lossy(&reencoded),
        "snapshot payload is not canonical §1.2 (fixed field order, no whitespace, \
         minimal lowercase hex, slot lines ascending by the 32-byte BE key, \\n after \
         every line including the last)"
    );

    // ---- header identity
    assert_eq!(doc.header.v, 1);
    assert_eq!(doc.header.kind, "strk20-snapshot");
    assert_eq!(doc.header.chain_id, CHAIN_ID);
    assert_eq!(felt_hex(&doc.header.pool), seed.pool_hex);
    assert_eq!(doc.header.epoch, BASIS_EPOCH);
    assert_eq!(
        doc.header.block, BASIS_BLOCK,
        "§1.2: header.block == epoch_range(header.epoch).1 — snapshots exist only at \
         epoch boundaries, hence <= l1_accepted, hence immutable by construction"
    );
    let epoch_entry = manifest["epochs"]
        .as_array()
        .expect("manifest.epochs")
        .iter()
        .find(|e| e["e"].as_u64() == Some(BASIS_EPOCH))
        .expect("manifest lists the basis epoch")
        .clone();
    assert_eq!(
        doc.header.epoch_hash,
        epoch_entry["hash"].as_str().unwrap_or_default(),
        "§1.2: header.epoch_hash pins the snapshot to the ONE hash chain, so a \
         snapshot-started client continues it rather than starting a second one"
    );
    assert_eq!(
        felt_hex(&doc.header.class),
        felt_hex(&seed.chain.class_at(BASIS_BLOCK).unwrap_or(Felt::ZERO)),
        "header.class must be the pool class as of the basis block"
    );

    // ---- the slot set IS the chain's state at the basis
    let expected = expected_slots(&seed.chain, BASIS_BLOCK);
    let got: Vec<(Felt, Felt)> = doc.slots.iter().map(|s| (s.k, s.v)).collect();
    assert_eq!(
        got.iter().map(|(k, v)| (felt_hex(k), felt_hex(v))).collect::<Vec<_>>(),
        expected.iter().map(|(k, v)| (felt_hex(k), felt_hex(v))).collect::<Vec<_>>(),
        "the snapshot must carry EVERY nonzero pool slot as of the basis and nothing \
         else (zero-valued slots are never emitted — Cairo map semantics)"
    );
    for s in &doc.slots {
        assert!(
            s.w <= BASIS_BLOCK,
            "slot {} carries write block {} above the basis {BASIS_BLOCK}",
            felt_hex(&s.k),
            s.w
        );
        assert_eq!(
            Some(s.w),
            seed.chain.write_block_of(&s.k),
            "slot {}'s write block must be its committed partition block — per-note \
             block_number and the 10-block maturity rule are derived from it",
            felt_hex(&s.k)
        );
    }
    assert_eq!(
        doc.footer_slots,
        doc.slots.len() as u64,
        "footer count must match the slot lines"
    );
    assert_eq!(snap["slots"].as_u64(), Some(doc.slots.len() as u64));

    // ---- the declared root is the recomputed root
    let computed = strk20_feed::mpt::storage_root(&got);
    assert_eq!(
        felt_hex(&doc.header.storage_root),
        felt_hex(&computed),
        "header.storage_root must equal mpt::storage_root over the slot lines"
    );
    assert_eq!(
        snap["storage_root"].as_str(),
        Some(felt_hex(&computed).as_str()),
        "manifest.snapshot.storage_root must agree with the header"
    );

    // ---- determinism across independent operators
    let dir2 = tempfile::tempdir().unwrap();
    let (out, err, ok) = backfill(dir2.path(), &url, &seed.pool_hex);
    assert!(ok, "second backfill failed\nstdout:\n{out}\nstderr:\n{err}");
    assert_snapshot_published(dir2.path());
    let zst2 = std::fs::read(feed_dir(dir2.path()).join(SNAPSHOT_FILE)).unwrap();
    assert_eq!(
        sha256_hex(&zst),
        sha256_hex(&zst2),
        "the snapshot file must be byte-identical across two independent backfills at \
         the same tip — it is a pure function of DB rows as of the basis block"
    );
    assert_eq!(
        read_manifest(dir2.path())["snapshot"]["hash"],
        snap["hash"],
        "both operators must publish the same content hash"
    );
}

// ------------------------------------------------------------------- S2

/// S2 — THE equality that justifies the whole item: a client cold-started
/// from a snapshot produces the same discovery output, field for field, as a
/// client that folded every epoch from genesis.
///
/// A snapshot carries slots and no events (R-A), so this is not free:
/// per-note `block_number` must come from each slot's write block, and
/// spent-state must come from nullifier slots rather than from `NoteUsed`
/// events that live below the history floor. The fixture contains a note
/// created AND spent below the basis precisely so a lost nullifier slot
/// shows up as an inequality instead of as silence.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s2_snapshot_cold_start_equals_full_replay() {
    ensure_built();
    let seed = seed_chain().await;
    let rpc = rpc_for(&seed.chain);
    let rpc_addr = rpc.serve().await;
    let dir = tempfile::tempdir().unwrap();
    let indexer_port = pick_free_port();
    let proxy = RecordingProxy::new(&format!("http://127.0.0.1:{indexer_port}"));
    let proxy_addr = proxy.serve().await;
    let feed_url = format!("http://{proxy_addr}/feed");

    let mut cmd = Command::new(bin("strk20"));
    cmd.arg("run")
        .args(base_args(
            dir.path(),
            &format!("http://{rpc_addr}/"),
            &seed.pool_hex,
        ))
        .args(["--listen", &format!("127.0.0.1:{indexer_port}")])
        .args(["--poll-ms", "150"]);
    let _indexer: ChildGuard = spawn_with_logs(cmd, dir.path(), "indexer");

    // ---------------------------------------------------------- oracle O1
    let o1 = {
        let backend = oracle::backend_at(&seed.chain, TAIL_MINT_BLOCK);
        oracle::incoming(&backend, seed.bob, &SecretFelt::new(seed.bob_key)).await
    };
    // Non-vacuity of the pre-basis spend: WITHOUT the nullifier slot the same
    // engine reports the note. So a snapshot that dropped it would be caught
    // as a difference in notes, not merely in a flag nobody reads.
    let nullifier_slot =
        discovery_core::privacy_pool::storage_slots::nullifiers(seed.pre_spent.nullifier);
    let o1_without_nullifier = {
        let mut backend = MockBackend::empty();
        for (slot, value) in seed.chain.state_at(TAIL_MINT_BLOCK) {
            if slot == nullifier_slot {
                continue;
            }
            backend.insert_with_block(slot, value, seed.chain.write_block_of(&slot).unwrap_or(0));
        }
        oracle::incoming(&backend, seed.bob, &SecretFelt::new(seed.bob_key)).await
    };
    let has_note = |r: &oracle::OracleResult, id: &Felt| r.notes.iter().any(|n| n.note_id == *id);
    assert!(
        has_note(&o1_without_nullifier, &seed.pre_spent.note_id),
        "fixture sanity: the pre-basis note is discoverable when its nullifier slot is \
         absent, so its absence from the real result is caused by spent-state"
    );
    assert!(
        !has_note(&o1, &seed.pre_spent.note_id),
        "fixture sanity: the pre-basis note is spent and the engine filters it"
    );
    assert!(
        has_note(&o1, &seed.pre_live.note_id) && has_note(&o1, &seed.tail.note_id),
        "fixture sanity: the unspent pre-basis and tail notes are discoverable"
    );

    let path = feed_dir(dir.path()).join(SNAPSHOT_FILE);
    let deadline = std::time::Instant::now() + Duration::from_secs(30);
    while std::time::Instant::now() < deadline {
        if path.exists() && !read_manifest(dir.path())["snapshot"].is_null() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert_snapshot_published(dir.path());

    // ------------------------------------------------- round 1: cold starts
    proxy.take_captured();
    let (replay, ok) = sync_with(
        dir.path(),
        &feed_url,
        &seed.bob,
        "0xb0b",
        "replay.db",
        &["--cold-start", "epochs"],
    );
    assert!(ok, "epoch-replay cold start failed: {replay}");
    let replay_capture = proxy.take_captured();

    let (snapshot, ok) = sync_with(
        dir.path(),
        &feed_url,
        &seed.bob,
        "0xb0b",
        "snap.db",
        &["--cold-start", "snapshot"],
    );
    assert!(ok, "snapshot cold start failed: {snapshot}");
    let snapshot_capture = proxy.take_captured();

    let (auto, ok) = sync_with(
        dir.path(),
        &feed_url,
        &seed.bob,
        "0xb0b",
        "auto.db",
        &["--cold-start", "auto"],
    );
    assert!(ok, "auto cold start failed: {auto}");
    proxy.take_captured();
    assert_eq!(
        auto["snapshot_basis"].as_u64(),
        Some(BASIS_BLOCK),
        "§1.7: `auto` is the DEFAULT posture and takes the snapshot branch when the \
         mirror is empty and the manifest carries a snapshot: {auto}"
    );

    assert_reports_equal(&replay, &snapshot, "round 1 (cold start)");
    assert_grades(&replay, &snapshot);

    // notes equal the independent oracle on both paths
    for (label, report) in [("replay", &replay), ("snapshot", &snapshot)] {
        assert_eq!(
            client_notes(report),
            oracle::notes_canonical(&o1.notes),
            "{label} client notes != oracle O1"
        );
        // per-note creation block == the block that committed its slot
        for n in report["notes"].as_array().unwrap() {
            let note_id = Felt::from_hex(n["note_id"].as_str().unwrap()).unwrap();
            let slot = discovery_core::privacy_pool::storage_slots::notes(note_id);
            assert_eq!(
                n["block_number"].as_u64(),
                seed.chain.write_block_of(&slot),
                "{label}: note creation block must equal its committed partition block \
                 — for a snapshot-started client that value exists only because the \
                 snapshot carries each slot's write block"
            );
        }
        assert!(
            !report["notes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|n| n["note_id"].as_str() == Some(&felt_hex(&seed.pre_spent.note_id))),
            "{label}: the note spent BELOW the basis must not be reported"
        );
    }

    // the snapshot carries the material spent-state needs
    let doc = snapshot_fmt::parse(&snapshot_payload(dir.path())).expect("snapshot parses");
    let nf_line = doc
        .slots
        .iter()
        .find(|s| s.k == nullifier_slot)
        .unwrap_or_else(|| {
            panic!(
                "the snapshot must carry the nullifier slot {} written at block \
                 {PRE_SPEND_BLOCK}: it is the ONLY evidence of a pre-basis spend a \
                 snapshot-started client can ever see (its NoteUsed event is below the \
                 history floor)",
                felt_hex(&nullifier_slot)
            )
        });
    assert!(nf_line.w <= BASIS_BLOCK && nf_line.v != Felt::ZERO);

    // --------------------------------------------- capture (§1.7 / §2.8.1)
    let snapshot_urls = assert_capture_allowed(&snapshot_capture, "snapshot cold start");
    let replay_urls = assert_capture_allowed(&replay_capture, "epoch-replay cold start");
    assert!(
        snapshot_urls.iter().any(|u| feed_urls::snapshot_index(u) == Some(BASIS_EPOCH)),
        "the snapshot client must fetch {SNAPSHOT_FILE}; it fetched {snapshot_urls:?}"
    );
    assert!(
        snapshot_urls
            .iter()
            .all(|u| feed_urls::epoch_index(u).map(|e| e > BASIS_EPOCH).unwrap_or(true)),
        "§1.7: cold start is O(1) in history length — no epoch <= the basis may be \
         fetched. Fetched: {snapshot_urls:?}"
    );
    assert!(
        replay_urls.iter().any(|u| feed_urls::epoch_index(u) == Some(0)),
        "control: the epoch-replay client DOES fetch epoch 0, so the assertion above is \
         about the snapshot path and not about an empty capture: {replay_urls:?}"
    );
    assert!(
        !snapshot_urls.iter().any(|u| u == "/feed/live"),
        "the Rust sync path is polling-only; /feed/live belongs to --watch"
    );

    // -------------------------------------- round 2: a spend in the tail
    //
    // `l1_accepted` is advanced to the new head as well, so epoch 2 ([32, 47])
    // becomes cuttable. That is deliberate: without it no epoch above the basis
    // ever exists in this fixture, and the snapshot-started client crosses the
    // seam through the head tail alone — leaving the §1.7 branch that chains
    // epoch 2 out of `header.epoch_hash` (ring 4's whole purpose: one spine,
    // continued rather than restarted) executed by no leg at all.
    {
        let mut chain = rpc.chain.write().unwrap();
        chain.add_note_block(
            TAIL_SPEND_BLOCK,
            discovery_core::privacy_pool::storage_slots::nullifiers(seed.pre_live.nullifier),
            Felt::ONE,
            FxEvent {
                keys: vec![
                    Felt::from_hex(NOTE_USED_SELECTOR).unwrap(),
                    seed.pre_live.nullifier,
                ],
                data: vec![],
            },
        );
        chain.head = TAIL_SPEND_BLOCK;
        chain.l1_accepted = TAIL_SPEND_BLOCK;
    }
    let head_path = feed_dir(dir.path()).join("head.ndjson");
    wait_for("the feed head to reach the spend block", Duration::from_secs(60), || {
        std::fs::read_to_string(&head_path)
            .map(|t| t.contains(&format!("\"head\":{TAIL_SPEND_BLOCK}")))
            .unwrap_or(false)
    })
    .await;
    wait_for("epoch 2 to be cut above the snapshot basis", Duration::from_secs(60), || {
        read_manifest(dir.path())["latest_epoch"].as_u64() == Some(2)
    })
    .await;

    proxy.take_captured();
    let (replay2, ok) = sync_with(dir.path(), &feed_url, &seed.bob, "0xb0b", "replay.db", &[]);
    assert!(ok, "replay resync failed: {replay2}");
    proxy.take_captured();
    let (snapshot2, ok) = sync_with(dir.path(), &feed_url, &seed.bob, "0xb0b", "snap.db", &[]);
    assert!(ok, "snapshot resync failed: {snapshot2}");
    let seam_capture = proxy.take_captured();

    // The snapshot-started mirror really did apply an epoch ABOVE its basis,
    // verified against `prev_hash` taken from the snapshot header's epoch pin —
    // and still fetched no epoch at or below the basis.
    let seam_urls = assert_capture_allowed(&seam_capture, "snapshot resync across the seam");
    assert!(
        seam_urls.iter().any(|u| feed_urls::epoch_index(u) == Some(2)),
        "the snapshot client must fetch epoch 2, the first epoch above its basis; it \
         fetched {seam_urls:?}"
    );
    assert!(
        seam_urls
            .iter()
            .all(|u| feed_urls::epoch_index(u).map(|e| e > BASIS_EPOCH).unwrap_or(true)),
        "...without ever reaching below the basis: {seam_urls:?}"
    );
    assert_eq!(
        snapshot2["last_epoch_to"].as_u64(),
        Some(TAIL_SPEND_BLOCK),
        "the epoch floor must have advanced past the basis: {snapshot2}"
    );

    assert_reports_equal(&replay2, &snapshot2, "round 2 (after a tail spend)");
    let nf_hex = felt_hex(&seed.pre_live.nullifier);
    for (label, report) in [("replay", &replay2), ("snapshot", &snapshot2)] {
        let spent: Vec<&Value> = report["notes"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|n| n["spent"] == true)
            .collect();
        assert_eq!(
            spent.len(),
            1,
            "{label}: exactly the newly spent note must flip: {report}"
        );
        assert_eq!(spent[0]["nullifier"].as_str(), Some(nf_hex.as_str()));
        assert!(
            report["newly_spent"]
                .as_array()
                .unwrap()
                .contains(&Value::String(nf_hex.clone())),
            "{label}: the spend must be reported as newly_spent"
        );
    }
}

/// The four keys that MUST differ between the two paths (§8 leg l(i)). The
/// comparison deletes exactly these and compares everything else, so a field
/// added to the report later lands in the compared set by default and cannot
/// silently fall out of the equality.
const REPORT_EXEMPT: [&str; 4] = [
    "history_from",
    "snapshot_basis",
    "snapshot_rejected",
    "verified",
];

fn without_exempt(report: &Value) -> Value {
    let mut v = report.clone();
    let obj = v.as_object_mut().expect("report is a JSON object");
    for k in REPORT_EXEMPT {
        obj.remove(k);
    }
    Value::Object(obj.clone())
}

fn assert_reports_equal(replay: &Value, snapshot: &Value, phase: &str) {
    let a = without_exempt(replay);
    let b = without_exempt(snapshot);
    assert_eq!(
        serde_json::to_string_pretty(&a).unwrap(),
        serde_json::to_string_pretty(&b).unwrap(),
        "{phase}: a snapshot-started client must produce the SAME report as one that \
         folded from genesis, field for field, except {REPORT_EXEMPT:?}"
    );
}

fn assert_grades(replay: &Value, snapshot: &Value) {
    assert_eq!(
        replay["history_from"].as_u64(),
        Some(0),
        "a fully epoch-replayed mirror has history_floor 0: {replay}"
    );
    assert_eq!(
        replay["verified"].as_str(),
        Some("replayed"),
        "§1.5.1: an epoch-replayed mirror carries the base §9 epoch-chain guarantee: {replay}"
    );
    assert_eq!(
        snapshot["history_from"].as_u64(),
        Some(BASIS_BLOCK + 1),
        "§1.1: the store records history_floor = snapshot.block + 1, surfaced as \
         history_from: {snapshot}"
    );
    assert_eq!(
        snapshot["snapshot_basis"].as_u64(),
        Some(BASIS_BLOCK),
        "the snapshot-started client must name its basis: {snapshot}"
    );
    assert_eq!(
        snapshot["snapshot_rejected"], Value::Bool(false),
        "the snapshot was accepted here: {snapshot}"
    );
    assert_eq!(
        snapshot["verified"].as_str(),
        Some("server-asserted"),
        "§1.5.1 + §11.3: with no anchor RPC of the client's own, the grade is \
         server-asserted — reachability proves the snapshot is consistent with an \
         anchor the SERVER published, and an anchor is a server assertion until the \
         client checks it against its own RPC (ring 6): {snapshot}"
    );
}

fn client_notes(report: &Value) -> Vec<Value> {
    let mut v: Vec<Value> = report["notes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|n| {
            serde_json::json!({
                "sender": n["sender"],
                "token": n["token"],
                "index": n["index"],
                "note_id": n["note_id"],
                "amount": n["amount"],
                "block_number": n["block_number"],
            })
        })
        .collect();
    v.sort_by_key(|j| {
        (
            j["token"].as_str().unwrap_or("").to_owned(),
            j["index"].as_u64().unwrap_or(0),
        )
    });
    v
}

/// Every captured request is a parameterless GET whose whole path is in the
/// closed §2.8.1 set. Returns the URLs.
fn assert_capture_allowed(capture: &[e2e_tests::proxy::Captured], label: &str) -> Vec<String> {
    let mut urls = Vec::new();
    for req in capture {
        assert_eq!(req.method, "GET", "{label}: keyless clients only GET: {}", req.uri);
        assert!(req.body.is_empty(), "{label}: keyless GET must have no body");
        assert!(
            feed_urls::is_allowed(&req.uri),
            "{label}: {} is outside the closed whole-path allowlist {:?}. Widening it \
             is an amendment, never a prefix match.",
            req.uri,
            feed_urls::PATTERNS
        );
        urls.push(req.uri.clone());
    }
    urls
}

// ------------------------------------------------------------------- S3

/// S3 — reachability (§11.3): fold the snapshot, apply what the feed carries
/// above the basis, recompute the storage root at an anchored block A and
/// compare with the anchor. A match attests the snapshot AND the intervening
/// range — which is strictly stronger than a point-proof at the basis, and is
/// the only grounding obtainable, since no provider answers a proof for a
/// block thousands behind head.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s3_reachability_reproduces_the_anchor_root_across_the_snapshot_seam() {
    ensure_built();
    let seed = seed_chain().await;
    let (rpc, dir) = backfilled_feed(&seed).await;
    let feed = feed_dir(dir.path()).display().to_string();

    let anchors = published_anchors(dir.path());
    let newest = anchors.iter().map(|(b, _)| *b).max().expect("an anchor");
    assert!(
        newest >= BASIS_BLOCK,
        "§11.3 gate: an anchor at A >= basis {BASIS_BLOCK} must exist; newest is {newest}"
    );
    assert!(
        anchors.iter().any(|(b, _)| *b >= TAIL_MINT_BLOCK),
        "fixture requirement: an anchor must land at or above {TAIL_MINT_BLOCK}, the \
         post-basis note block, so the reachability check spans the seam instead of \
         re-checking the basis state; anchored blocks: {:?}",
        anchors.iter().map(|(b, _)| *b).collect::<Vec<_>>()
    );
    // The state genuinely moved between the basis and the anchor, so a check
    // that ignored everything above the basis would give a different root.
    let root_at_basis = strk20_feed::mpt::storage_root(&expected_slots(&seed.chain, BASIS_BLOCK));
    let (anchor_block, anchor_root) = *anchors
        .iter()
        .max_by_key(|(b, _)| *b)
        .expect("newest anchor");
    assert_ne!(
        felt_hex(&root_at_basis),
        felt_hex(&anchor_root),
        "fixture requirement: the root at the anchor must differ from the root at the \
         basis, or reachability could pass while ignoring the epochs in between"
    );
    assert_eq!(
        felt_hex(&anchor_root),
        felt_hex(&strk20_feed::mpt::storage_root(&expected_slots(
            &seed.chain,
            anchor_block
        ))),
        "the published anchor must be true to the chain at block {anchor_block}"
    );
    assert_snapshot_published(dir.path());

    // ---- cold start from the snapshot, then reach the anchor
    let (report, ok) = sync_with(
        dir.path(),
        &feed,
        &seed.bob,
        "0xb0b",
        "snap.db",
        &["--cold-start", "snapshot"],
    );
    assert!(ok, "snapshot cold start failed: {report}");
    assert_eq!(
        report["snapshot_basis"].as_u64(),
        Some(BASIS_BLOCK),
        "the snapshot path must actually have been taken: {report}"
    );
    assert_eq!(report["verified"].as_str(), Some("server-asserted"), "{report}");

    let (stdout, stderr, ok) = verify_anchors(&feed, &dir.path().join("snap.db"));
    assert!(
        ok,
        "§11.3: folding snapshot({BASIS_BLOCK}) + the feed above it must reproduce the \
         published root at block {anchor_block}\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let v: Value = serde_json::from_str(&stdout)
        .unwrap_or_else(|e| panic!("verify-anchors --json must print a report ({e}): {stdout}"));
    assert_eq!(v["all_ok"], Value::Bool(true), "{v}");
    let head = v["head"].as_u64().unwrap_or(0);
    let checkable = anchors.iter().filter(|(b, _)| *b <= head).count() as u64;
    assert!(checkable > 0, "no anchor is at or below the mirror head {head}");
    assert_eq!(
        v["anchors_checked"].as_u64(),
        Some(checkable),
        "every anchor the mirror can reach must be checked, snapshot seam included: {v}"
    );

    // ---- ring 6: the honest grade (§1.5 ring 6, §11.3)
    let rpc_url = {
        // the client's OWN endpoint; the request names only the public pool
        // and a public block, so it is identical for every user
        let addr = rpc.serve().await;
        format!("http://{addr}/")
    };
    let (grounded, ok) = sync_with(
        dir.path(),
        &feed,
        &seed.bob,
        "0xb0b",
        "grounded.db",
        &["--cold-start", "snapshot", "--verify-anchor", &rpc_url],
    );
    assert!(ok, "snapshot cold start with an anchor RPC failed: {grounded}");
    assert_eq!(
        grounded["verified"].as_str(),
        Some("anchored"),
        "§11.3: an anchor is a server assertion until the client checks it against an \
         RPC it trusts. Recent anchors are exactly what the ~1024-block window serves, \
         so with --verify-anchor the grade rises to \"anchored\": {grounded}"
    );
    assert_reports_equal(&report, &grounded, "ring 6 grounding");
}

// ------------------------------------------------------------------- S4

/// S4 — the negative of S3, so S3 cannot pass vacuously.
///
/// (i) corruption is caught by the transport hash, named;
/// (ii) the case rings 1-5 CANNOT catch — a server that alters a slot and
///      recomputes every root it publishes so the file is internally perfect
///      — is caught by reachability against the anchors log, because the
///      anchor's root is a claim about the CHAIN that the altered slot set
///      cannot reproduce. This is what §11.3 buys over the superseded
///      point-proof sidecar.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s4_tampered_and_unreachable_snapshots_are_rejected_by_name() {
    ensure_built();
    let seed = seed_chain().await;
    let (_rpc, dir) = backfilled_feed(&seed).await;
    assert_snapshot_published(dir.path());
    let feed = feed_dir(dir.path()).display().to_string();
    let file = feed_dir(dir.path()).join(SNAPSHOT_FILE);
    let manifest_path = feed_dir(dir.path()).join("manifest.json");
    let original_zst = std::fs::read(&file).unwrap();
    let original_manifest = std::fs::read(&manifest_path).unwrap();

    // control: the untouched snapshot is accepted, so every rejection below
    // is about the tamper and not about a client that refuses everything
    let (control, ok) = sync_with(
        dir.path(),
        &feed,
        &seed.bob,
        "0xb0b",
        "control.db",
        &["--cold-start", "snapshot"],
    );
    assert!(ok, "the untouched snapshot must be accepted: {control}");
    let good_notes = client_notes(&control);

    // ---------------------------------------------- (i) transport corruption
    let mut flipped = original_zst.clone();
    let mid = flipped.len() / 2;
    flipped[mid] ^= 0xff;
    std::fs::write(&file, &flipped).unwrap();
    let (err, ok) = sync_with(
        dir.path(),
        &feed,
        &seed.bob,
        "0xb0b",
        "flip.db",
        &["--cold-start", "snapshot"],
    );
    assert!(!ok, "a corrupted snapshot file must be rejected: {err}");
    let text = err["error"].as_str().unwrap_or_default().to_owned();
    assert!(
        text.contains("FEED_HASH_MISMATCH"),
        "§1.5 ring 1: the .zst sha256 is verified BEFORE decompression and the failure \
         is named FEED_HASH_MISMATCH (R-I). Got:\n{text}"
    );
    std::fs::write(&file, &original_zst).unwrap();

    // ------------------------------- (ii) the consistently-recomputed lie
    let payload = strk20_feed::decompress(&original_zst).unwrap();
    let mut doc = snapshot_fmt::parse(&payload).expect("snapshot parses");
    let victim = doc.slots[0];
    assert!(
        seed.chain.write_block_of(&victim.k).unwrap_or(0) <= BASIS_BLOCK,
        "the altered slot must not be rewritten above the basis, or folding the feed \
         would silently repair it"
    );
    doc.slots[0].v = victim.v + Felt::ONE;
    let slot_pairs: Vec<(Felt, Felt)> = doc.slots.iter().map(|s| (s.k, s.v)).collect();
    doc.header.storage_root = strk20_feed::mpt::storage_root(&slot_pairs);
    let forged_payload = snapshot_fmt::encode(&doc);
    let forged_zst = strk20_feed::compress(&forged_payload);
    std::fs::write(&file, &forged_zst).unwrap();
    {
        let mut manifest: Value = serde_json::from_slice(&original_manifest).unwrap();
        manifest["snapshot"]["hash"] = Value::String(sha256_hex(&forged_payload));
        manifest["snapshot"]["zst"] = Value::String(sha256_hex(&forged_zst));
        manifest["snapshot"]["bytes"] = Value::from(forged_zst.len() as u64);
        manifest["snapshot"]["storage_root"] = Value::String(felt_hex(&doc.header.storage_root));
        std::fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    }

    let (err, ok) = sync_with(
        dir.path(),
        &feed,
        &seed.bob,
        "0xb0b",
        "forged.db",
        &["--cold-start", "snapshot"],
    );
    assert!(
        !ok,
        "a snapshot whose slot set cannot reach the published anchor must be refused. \
         Rings 1-5 all PASS on this file — every root it can be compared against was \
         recomputed by the same forger — so only the §11.3 reachability check stands \
         between a client and an altered slot set: {err}"
    );
    let text = err["error"].as_str().unwrap_or_default().to_owned();
    assert!(
        text.contains("SNAPSHOT_UNREACHABLE"),
        "§11.3: the failure is a reachability failure and must be named \
         SNAPSHOT_UNREACHABLE, naming the anchor block it could not reproduce. Got:\n{text}"
    );
    assert!(
        !text.contains("FEED_HASH_MISMATCH"),
        "the forged file is internally consistent: a hash error here means the test \
         forged it wrong, not that the client caught the lie:\n{text}"
    );

    // ---- the refusal must not leave the refused rows behind
    //
    // `apply_snapshot` COMMITS the slot set long before §11.3 reachability can
    // run — the epochs above the basis and the head tail have to land first,
    // because reachability validates them too. So a rejected snapshot leaves a
    // populated mirror unless something clears it, and a populated mirror is
    // never empty again: the next sync skips the snapshot branch entirely and
    // therefore skips the grounding, leaving the client permanently on a slot
    // set it explicitly refused once. Re-running the same db is exactly what an
    // operator does after seeing a rejection.
    let (again, ok) = sync_with(
        dir.path(),
        &feed,
        &seed.bob,
        "0xb0b",
        "forged.db",
        &["--cold-start", "snapshot"],
    );
    assert!(
        !ok,
        "a second run against the same db must reject the same snapshot again. It \
         succeeded, which means the refused slot set survived the first rejection and \
         was then accepted with no grounding at all: {again}"
    );
    assert!(
        again["error"]
            .as_str()
            .unwrap_or_default()
            .contains("SNAPSHOT_UNREACHABLE"),
        "and for the same reason: {again}"
    );
    // The same db is also usable again once the operator asks for the honest
    // path — proof the rejection cleaned up rather than wedging the mirror.
    let (recovered, ok) = sync_with(
        dir.path(),
        &feed,
        &seed.bob,
        "0xb0b",
        "forged.db",
        &["--cold-start", "epochs"],
    );
    assert!(ok, "epoch replay on the cleaned-up db must work: {recovered}");
    assert_eq!(
        recovered["snapshot_basis"],
        Value::Null,
        "no trace of the refused snapshot may remain: {recovered}"
    );
    assert_eq!(recovered["history_from"].as_u64(), Some(0), "{recovered}");
    assert_eq!(recovered["verified"].as_str(), Some("replayed"), "{recovered}");
    assert_eq!(client_notes(&recovered), good_notes);

    // C13 fallback: `auto` degrades to epoch replay instead of failing
    let (fallback, ok) = sync_with(
        dir.path(),
        &feed,
        &seed.bob,
        "0xb0b",
        "fallback.db",
        &["--cold-start", "auto"],
    );
    assert!(
        ok,
        "§1.7 C13: under `auto` a rejected snapshot falls back to full epoch replay \
         rather than failing the sync: {fallback}"
    );
    assert_eq!(
        fallback["snapshot_rejected"],
        Value::Bool(true),
        "the fallback must be reported, not silent: {fallback}"
    );
    assert_eq!(
        fallback["verified"].as_str(),
        Some("replayed"),
        "after falling back the mirror carries the epoch-chain guarantee: {fallback}"
    );
    assert_eq!(fallback["history_from"].as_u64(), Some(0), "{fallback}");
    assert_eq!(
        client_notes(&fallback),
        good_notes,
        "the fallback must reach the same discovery result as the honest snapshot path"
    );

    // control again on restored bytes
    std::fs::write(&file, &original_zst).unwrap();
    std::fs::write(&manifest_path, &original_manifest).unwrap();
    let (restored, ok) = sync_with(
        dir.path(),
        &feed,
        &seed.bob,
        "0xb0b",
        "restored.db",
        &["--cold-start", "snapshot"],
    );
    assert!(ok, "the restored snapshot must verify again: {restored}");
    assert_eq!(client_notes(&restored), good_notes);
}

// ------------------------------------------------------------------- S5

/// S5 — the publication gate (§11.3). An endpoint that cannot serve storage
/// proofs (publicnode answers code 42 at every height) yields no anchors, so
/// nothing grounds a snapshot and none may be published. When proofs become
/// available again the gate is met and publication RESUMES — without any new
/// epoch being cut, because the anchor that satisfies the gate is captured at
/// head, long after the batch that cut the basis epoch.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s5_no_snapshot_until_the_publication_gate_is_met() {
    ensure_built();
    let seed = seed_chain().await;
    let rpc = FixtureRpc::with_faults(
        seed.chain.clone(),
        CHAIN_ID,
        FaultSpec {
            proof_window: Some(PROOF_WINDOW),
            proofs_unsupported: true,
            ..Default::default()
        },
    );
    let addr = rpc.serve().await;
    let url = format!("http://{addr}/");
    let dir = tempfile::tempdir().unwrap();

    let (out, err, ok) = backfill(dir.path(), &url, &seed.pool_hex);
    assert!(ok, "backfill failed\nstdout:\n{out}\nstderr:\n{err}");
    assert!(
        rpc.proofs_denied() > 0,
        "vacuous test: the fixture endpoint never denied a storage proof"
    );

    let manifest = read_manifest(dir.path());
    assert_eq!(
        manifest["latest_epoch"].as_u64(),
        Some(BASIS_EPOCH),
        "non-vacuity: epochs must have been cut, so the absence of a snapshot is the \
         gate and not an empty feed: {manifest}"
    );
    assert!(
        published_anchors(dir.path()).is_empty(),
        "a proof-less endpoint can produce no anchors"
    );
    assert!(
        manifest["snapshot"].is_null(),
        "§11.3: with no anchor at A >= basis nothing grounds a snapshot, so none may \
         be published: {manifest}"
    );
    let snap_dir = feed_dir(dir.path()).join("snapshots");
    let published: Vec<_> = std::fs::read_dir(&snap_dir)
        .map(|rd| rd.flatten().map(|e| e.file_name()).collect())
        .unwrap_or_default();
    assert!(
        published.is_empty(),
        "no snapshot file may exist while the gate is unmet: {published:?}"
    );
    // ---- §1.5.2: `--cold-start snapshot` REFUSES rather than degrades
    //
    // This feed is the real shape of an unmet gate, so it is the right place to
    // pin it. Falling through to a full epoch replay would be the run the
    // operator explicitly asked not to do — on a metered or slow link, silently,
    // and reported as `verified: "replayed"` with no diagnostic naming the
    // reason. "Refuse loudly" is the guard rail; "degrade quietly" is what
    // §1.5.2 forbids.
    let (refused, ok) = sync_with(
        dir.path(),
        &feed_dir(dir.path()).display().to_string(),
        &seed.bob,
        "0xb0b",
        "no-snapshot.db",
        &["--cold-start", "snapshot"],
    );
    assert!(
        !ok,
        "a feed with manifest.snapshot == null cannot honour --cold-start snapshot, and \
         must say so instead of quietly replaying every epoch from genesis: {refused}"
    );
    assert!(
        refused["error"]
            .as_str()
            .unwrap_or_default()
            .contains("SNAPSHOT_UNAVAILABLE"),
        "the refusal must be named: {refused}"
    );
    // ...while `auto` is exactly the mode that IS allowed to fall back.
    let (fell_back, ok) = sync_with(
        dir.path(),
        &feed_dir(dir.path()).display().to_string(),
        &seed.bob,
        "0xb0b",
        "auto-no-snapshot.db",
        &["--cold-start", "auto"],
    );
    assert!(ok, "`auto` against a snapshot-less feed must simply replay: {fell_back}");
    assert_eq!(fell_back["verified"].as_str(), Some("replayed"), "{fell_back}");

    // ---- the endpoint regains the capability and the head moves on
    rpc.set_proofs_supported(true);
    {
        let mut chain = rpc.chain.write().unwrap();
        chain.add_note_block(
            50,
            Felt::from(0x5000_0000u64),
            Felt::from(0x51u64),
            FxEvent {
                keys: vec![
                    Felt::from_hex(ENC_NOTE_CREATED_SELECTOR).unwrap(),
                    Felt::from(0x5050u64),
                ],
                data: vec![Felt::from(0u64)],
            },
        );
        chain.head = 50;
        // l1_accepted deliberately unchanged: NO new epoch becomes cuttable,
        // so publication must resume on the gate alone.
        assert!(chain.l1_accepted < 47);
    }
    let (out, err, ok) = backfill(dir.path(), &url, &seed.pool_hex);
    assert!(ok, "second backfill failed\nstdout:\n{out}\nstderr:\n{err}");

    let anchors = published_anchors(dir.path());
    assert!(
        anchors.iter().any(|(b, _)| *b >= BASIS_BLOCK),
        "§11.2: with proofs available the head-side capture must record an anchor at \
         A >= basis {BASIS_BLOCK}; got {:?}",
        anchors.iter().map(|(b, _)| *b).collect::<Vec<_>>()
    );
    let manifest = read_manifest(dir.path());
    assert_eq!(
        manifest["latest_epoch"].as_u64(),
        Some(BASIS_EPOCH),
        "sanity: no new epoch was cut in this phase: {manifest}"
    );
    assert_snapshot_published(dir.path());
    assert_eq!(
        read_manifest(dir.path())["snapshot"]["e"].as_u64(),
        Some(BASIS_EPOCH),
        "publication resumes for the newest cut epoch once the gate is met"
    );
}

// ------------------------------------------------------------------- S6

/// S6 — retention. Snapshots are derived artifacts and are pruned, but never
/// out from under a client: a client that read the previous manifest moments
/// earlier must still be able to download the file that manifest names.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s6_retention_keeps_the_newest_snapshots_without_404ing_the_previous_manifest() {
    ensure_built();
    let seed = seed_chain().await;
    let rpc = rpc_for(&seed.chain);
    let addr = rpc.serve().await;
    let url = format!("http://{addr}/");
    let dir = tempfile::tempdir().unwrap();

    // phase 1 — epochs 0,1 cut; snapshot at epoch 1
    let (out, err, ok) = backfill(dir.path(), &url, &seed.pool_hex);
    assert!(ok, "backfill failed\nstdout:\n{out}\nstderr:\n{err}");
    assert_snapshot_published(dir.path());
    let manifest_v1 = read_manifest(dir.path());
    let file_v1 = manifest_v1["snapshot"]["file"].as_str().unwrap().to_owned();
    let bytes_v1 = std::fs::read(feed_dir(dir.path()).join(&file_v1)).unwrap();

    // phase 2 — epoch 2 [32,47] becomes cuttable; snapshot at epoch 2
    grow(&rpc, 60, 47);
    let (out, err, ok) = backfill(dir.path(), &url, &seed.pool_hex);
    assert!(ok, "second backfill failed\nstdout:\n{out}\nstderr:\n{err}");
    assert_snapshot_published(dir.path());
    assert_eq!(
        read_manifest(dir.path())["snapshot"]["e"].as_u64(),
        Some(2),
        "cutting epoch 2 with the gate met must publish a snapshot at its basis"
    );

    // the file the PREVIOUS manifest named is still there, byte-identical
    let still = feed_dir(dir.path()).join(&file_v1);
    assert!(
        still.exists(),
        "§1.4 step 6: keeping the newest 2 exists so a client that read the previous \
         manifest never 404s mid-download; {file_v1} was deleted immediately"
    );
    assert_eq!(
        std::fs::read(&still).unwrap(),
        bytes_v1,
        "a retained snapshot must never be rewritten"
    );

    // ... and it is still SERVED, not merely present on disk
    let port = pick_free_port();
    let mut cmd = Command::new(bin("strk20"));
    cmd.arg("run")
        .args(base_args(dir.path(), &url, &seed.pool_hex))
        .args(["--listen", &format!("127.0.0.1:{port}")])
        .args(["--poll-ms", "5000"]);
    let _server = spawn_with_logs(cmd, dir.path(), "retention-server");
    let http = reqwest::Client::new();
    let mut served = None;
    for _ in 0..100 {
        if let Ok(resp) = http
            .get(format!("http://127.0.0.1:{port}/feed/{file_v1}"))
            .send()
            .await
        {
            served = Some(resp);
            break;
        }
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
    let served = served.expect("server did not come up");
    assert_eq!(
        served.status().as_u16(),
        200,
        "the previous manifest's snapshot must still be downloadable"
    );
    assert_eq!(served.bytes().await.unwrap().to_vec(), bytes_v1);
    drop(_server);

    // phase 3 — epoch 3 [48,63]; now the oldest of three is pruned
    grow(&rpc, 80, 63);
    let (out, err, ok) = backfill(dir.path(), &url, &seed.pool_hex);
    assert!(ok, "third backfill failed\nstdout:\n{out}\nstderr:\n{err}");
    assert_eq!(
        read_manifest(dir.path())["snapshot"]["e"].as_u64(),
        Some(3),
        "the manifest must name the newest snapshot"
    );
    let mut kept: Vec<String> = std::fs::read_dir(feed_dir(dir.path()).join("snapshots"))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .filter(|n| n.ends_with(".strk20s.zst"))
        .collect();
    kept.sort();
    assert_eq!(
        kept,
        vec![
            "00000002.strk20s.zst".to_owned(),
            "00000003.strk20s.zst".to_owned()
        ],
        "§1.4 step 6: retention keeps the newest 2 snapshots and deletes older ones"
    );
}

/// Extend the fixture chain: a pool-active block at the new head (so the
/// head-side anchor lands on a block a mirror holds) and a higher
/// `l1_accepted` so further epochs become cuttable.
fn grow(rpc: &FixtureRpc, head: u64, l1_accepted: u64) {
    let mut chain = rpc.chain.write().unwrap();
    chain.add_note_block(
        head,
        Felt::from(0x9000_0000u64 + head),
        Felt::from(head),
        FxEvent {
            keys: vec![
                Felt::from_hex(ENC_NOTE_CREATED_SELECTOR).unwrap(),
                Felt::from(0xa000u64 + head),
            ],
            data: vec![Felt::from(head)],
        },
    );
    chain.head = head;
    chain.l1_accepted = l1_accepted;
}

/// A feed server that answers `anchors.ndjson` DIFFERENTLY on its first
/// request and every request after it, serving every other file straight from
/// the mirror directory.
///
/// This exists to make one specific composition failure observable. Two
/// separate checks used to read the anchors log through two separate GETs —
/// §11.3 reachability compared the MIRROR against a record from its own fetch,
/// and ring 6 compared a record from ITS own fetch against the chain — and
/// those compose into "the mirror is the chain's" only if both fetches returned
/// the same record. Nothing made them. Both are byte-identical parameterless
/// GETs, so a server can simply answer them differently; an honest server
/// breaks the same composition for free by appending an anchor in between.
struct TwoFacedFeed {
    dir: PathBuf,
    first: Vec<u8>,
    rest: Vec<u8>,
    requests: std::sync::Arc<std::sync::atomic::AtomicUsize>,
}

impl TwoFacedFeed {
    async fn serve(self) -> (String, std::sync::Arc<std::sync::atomic::AtomicUsize>) {
        use axum::extract::State;
        use axum::http::{StatusCode, Uri};
        use axum::response::{IntoResponse, Response};
        use std::sync::atomic::Ordering;

        let requests = self.requests.clone();
        let state = std::sync::Arc::new(self);

        async fn handler(
            State(s): State<std::sync::Arc<TwoFacedFeed>>,
            uri: Uri,
        ) -> Response {
            let path = uri.path().trim_start_matches('/');
            let rel = path.strip_prefix("feed/").unwrap_or(path);
            if rel == "anchors.ndjson" {
                let n = s.requests.fetch_add(1, Ordering::SeqCst);
                let body = if n == 0 { s.first.clone() } else { s.rest.clone() };
                return (StatusCode::OK, body).into_response();
            }
            match std::fs::read(s.dir.join(rel)) {
                Ok(b) => (StatusCode::OK, b).into_response(),
                Err(_) => (StatusCode::NOT_FOUND, "not found").into_response(),
            }
        }

        let app = axum::Router::new()
            .fallback(axum::routing::get(handler))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        (format!("http://{addr}/feed"), requests)
    }
}

// ------------------------------------------------------------------- S7

/// S7 — spec leg m(ii-b), in its §11 form: the malicious-server case that S4
/// deliberately cannot reach.
///
/// S4's forgery rewrites the snapshot and every root INSIDE the content-addressed
/// artifacts, and is caught by §11.3 reachability against `anchors.ndjson`. But
/// the anchors log is NOT content-addressed and is not in the epoch hash chain —
/// `crates/client/src/anchors.rs` says so in its own header: "a hostile feed can
/// write whatever it likes there". A server that forges the log CONSISTENTLY
/// with its forged snapshot therefore defeats reachability too, and only ring 6
/// — the user's OWN RPC — is left.
///
/// The leg asserts both directions, because the reduced grade of §1.5.2/§11.3 is
/// a claim about what is NOT caught as much as about what is:
///
///   - with no ring 6, NOTHING catches it: rings 1-5 pass, reachability passes,
///     the client accepts the tampered slot set and reports
///     `verified: "server-asserted"`. Asserted POSITIVELY, so any future claim
///     that the offline ladder is proof-grade against the server turns this red.
///   - with ring 6 configured, the user's own RPC catches it and `verified`
///     never reaches `"anchored"`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s7_a_server_that_forges_the_anchors_log_too_is_caught_only_by_ring_6() {
    ensure_built();
    let seed = seed_chain().await;
    let (rpc, dir) = backfilled_feed(&seed).await;
    assert_snapshot_published(dir.path());
    let feed = feed_dir(dir.path()).display().to_string();
    let file = feed_dir(dir.path()).join(SNAPSHOT_FILE);
    let manifest_path = feed_dir(dir.path()).join("manifest.json");
    let anchors_path = feed_dir(dir.path()).join("anchors.ndjson");
    let original_zst = std::fs::read(&file).unwrap();
    let original_manifest = std::fs::read(&manifest_path).unwrap();
    let original_anchors = std::fs::read(&anchors_path).unwrap();

    // control: the honest feed is accepted at BOTH grades
    let (control, ok) = sync_with(
        dir.path(), &feed, &seed.bob, "0xb0b", "s7-control.db",
        &["--cold-start", "snapshot"],
    );
    assert!(ok, "the untouched snapshot must be accepted: {control}");
    let honest_notes = client_notes(&control);
    let rpc_url = format!("http://{}/", rpc.serve().await);
    let (control6, ok) = sync_with(
        dir.path(), &feed, &seed.bob, "0xb0b", "s7-control6.db",
        &["--cold-start", "snapshot", "--verify-anchor", &rpc_url],
    );
    assert!(ok, "the untouched snapshot must ground against an honest RPC: {control6}");
    assert_eq!(control6["verified"].as_str(), Some("anchored"), "{control6}");

    // ---- forge the snapshot, then forge the anchors log to MATCH it
    let payload = strk20_feed::decompress(&original_zst).unwrap();
    let mut doc = snapshot_fmt::parse(&payload).expect("snapshot parses");
    let victim = doc.slots[0];
    assert!(
        seed.chain.write_block_of(&victim.k).unwrap_or(0) <= BASIS_BLOCK,
        "the altered slot must not be rewritten above the basis, or folding the feed \
         would silently repair it"
    );
    let forged_value = victim.v + Felt::ONE;
    doc.slots[0].v = forged_value;
    let slot_pairs: Vec<(Felt, Felt)> = doc.slots.iter().map(|s| (s.k, s.v)).collect();
    doc.header.storage_root = strk20_feed::mpt::storage_root(&slot_pairs);
    let forged_payload = snapshot_fmt::encode(&doc);
    let forged_zst = strk20_feed::compress(&forged_payload);
    std::fs::write(&file, &forged_zst).unwrap();
    {
        let mut manifest: Value = serde_json::from_slice(&original_manifest).unwrap();
        manifest["snapshot"]["hash"] = Value::String(sha256_hex(&forged_payload));
        manifest["snapshot"]["zst"] = Value::String(sha256_hex(&forged_zst));
        manifest["snapshot"]["bytes"] = Value::from(forged_zst.len() as u64);
        manifest["snapshot"]["storage_root"] = Value::String(felt_hex(&doc.header.storage_root));
        std::fs::write(&manifest_path, serde_json::to_vec_pretty(&manifest).unwrap()).unwrap();
    }
    // Every anchor's storage_root becomes the root the FORGED slot set folds to
    // at that block — block hashes and classes left honest, because the forger
    // has no reason to touch what the mirror can check another way.
    let forged_anchor_blocks: Vec<u64> = {
        let text = std::fs::read_to_string(&anchors_path).unwrap();
        let mut out = String::new();
        let mut blocks = Vec::new();
        for line in text.lines() {
            let mut v: Value = serde_json::from_str(line).unwrap();
            let block = v["block"].as_u64().unwrap();
            let mut set: Vec<(Felt, Felt)> = expected_slots(&seed.chain, block);
            for (k, value) in set.iter_mut() {
                if *k == victim.k {
                    *value = forged_value;
                }
            }
            v["storage_root"] = Value::String(felt_hex(&strk20_feed::mpt::storage_root(&set)));
            out.push_str(&serde_json::to_string(&v).unwrap());
            out.push('\n');
            blocks.push(block);
        }
        std::fs::write(&anchors_path, out).unwrap();
        blocks
    };
    assert!(
        !forged_anchor_blocks.is_empty(),
        "non-vacuity: there must be anchors to forge, or reachability had nothing to \
         check in the first place"
    );

    // ---- (a) with NO ring 6, nothing catches it. This is the honest statement
    // of the grade, asserted as an outcome rather than described in a comment.
    let (accepted, ok) = sync_with(
        dir.path(), &feed, &seed.bob, "0xb0b", "s7-nolodge.db",
        &["--cold-start", "snapshot"],
    );
    assert!(
        ok,
        "§1.5.2 / §11.3: rings 1-5 are self-consistency checks over values the SAME \
         server produced, and reachability compares the mirror against a log that same \
         server writes and that is outside content addressing. A server willing to forge \
         both is not caught by any of them. If this now fails, the trust grade below is \
         stale and §1.5.2 must be re-stated — do not 'fix' the test.\n{accepted}"
    );
    assert_eq!(
        accepted["verified"].as_str(),
        Some("server-asserted"),
        "the grade must say exactly what was and was not established: {accepted}"
    );
    assert_eq!(
        accepted["snapshot_basis"].as_u64(),
        Some(BASIS_BLOCK),
        "the snapshot path really was taken: {accepted}"
    );
    assert_eq!(
        accepted["snapshot_rejected"], Value::Bool(false),
        "nothing rejected it — that is the point of this half: {accepted}"
    );

    // ---- (b) with ring 6, the user's own RPC catches it
    let (caught, ok) = sync_with(
        dir.path(), &feed, &seed.bob, "0xb0b", "s7-grounded.db",
        &["--cold-start", "snapshot", "--verify-anchor", &rpc_url],
    );
    assert!(
        !ok,
        "§1.5 ring 6 is the ONLY ring that grounds this mirror in the chain, and it is \
         what must catch a server that forged the snapshot and the anchors log \
         together: {caught}"
    );
    let text = caught["error"].as_str().unwrap_or_default().to_owned();
    assert!(
        text.contains("ANCHOR_NOT_ON_CHAIN"),
        "the failure must be named ANCHOR_NOT_ON_CHAIN: it is the client's own RPC, not \
         the feed, that disagrees. Got:\n{text}"
    );
    assert!(
        !text.contains("SNAPSHOT_UNREACHABLE") && !text.contains("FEED_HASH_MISMATCH"),
        "the forgery is internally consistent AND reachable against the forged log — an \
         earlier-ring error here means the test forged it wrong, not that the offline \
         ladder caught the lie:\n{text}"
    );
    assert!(
        !text.contains("\"verified\": \"anchored\""),
        "`anchored` must never be reported for a mirror the RPC refuted:\n{text}"
    );

    // ---- (c) and the refuted mirror does not survive to be reused ungrounded
    let (rerun, ok) = sync_with(
        dir.path(), &feed, &seed.bob, "0xb0b", "s7-grounded.db",
        &["--cold-start", "snapshot"],
    );
    assert_eq!(
        rerun["verified"].as_str().unwrap_or("<failed>"),
        if ok { "server-asserted" } else { "<failed>" },
        "sanity on the re-run's shape"
    );
    assert_ne!(
        rerun["verified"].as_str(),
        Some("anchored"),
        "a mirror the RPC refuted must never come back graded anchored: {rerun}"
    );

    // ---- (d) the two-faced server: forged anchors to the reachability fetch,
    // HONEST anchors to ring 6's. Ring 6 must still catch the mirror, because
    // what it grounds is the client's OWN recomputed root and not a record it
    // re-downloaded. A ring 6 that compared anchor-against-chain would see the
    // honest log agree with the honest chain and report `"anchored"` for a slot
    // set that was never compared to anything.
    let forged_anchors = std::fs::read(&anchors_path).unwrap();
    let (two_faced, anchor_hits) = TwoFacedFeed {
        dir: feed_dir(dir.path()),
        first: forged_anchors.clone(),
        rest: original_anchors.clone(),
        requests: std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0)),
    }
    .serve()
    .await;
    let (two_faced_out, ok) = sync_with(
        dir.path(), &two_faced, &seed.bob, "0xb0b", "s7-twofaced.db",
        &["--cold-start", "snapshot", "--verify-anchor", &rpc_url],
    );
    assert!(
        anchor_hits.load(std::sync::atomic::Ordering::SeqCst) >= 2,
        "non-vacuity: the anchors log must have been fetched at least twice, or the two \
         faces never both showed"
    );
    assert!(
        !ok,
        "ring 6 must ground the MIRROR, not a re-downloaded anchor record: with the \
         honest log served to its fetch, an anchor-against-chain comparison passes and \
         `anchored` is reported for a slot set nothing ever checked: {two_faced_out}"
    );
    let text = two_faced_out["error"].as_str().unwrap_or_default().to_owned();
    assert!(
        text.contains("ANCHOR_NOT_ON_CHAIN"),
        "and the disagreement is between this mirror and the user's own RPC:\n{text}"
    );

    // restore and confirm the fixture was the tamper, not the client
    std::fs::write(&file, &original_zst).unwrap();
    std::fs::write(&manifest_path, &original_manifest).unwrap();
    std::fs::write(&anchors_path, &original_anchors).unwrap();
    let (restored, ok) = sync_with(
        dir.path(), &feed, &seed.bob, "0xb0b", "s7-restored.db",
        &["--cold-start", "snapshot", "--verify-anchor", &rpc_url],
    );
    assert!(ok, "the restored feed must verify again: {restored}");
    assert_eq!(restored["verified"].as_str(), Some("anchored"), "{restored}");
    assert_eq!(client_notes(&restored), honest_notes);
}

// ------------------------------------------------------------------- S8

/// The per-snapshot proof sidecar, `snapshots/{e:08}.anchor.json`.
const SNAPSHOT_ANCHOR_FILE: &str = "snapshots/00000001.anchor.json";

/// Serve a feed directory over HTTP: `GET /feed/<path>` → the file, 404 when
/// it is absent. A keyless client only GETs static artifacts, so this is a
/// complete server for it — and unlike `strk20 run` it publishes a FIXED feed,
/// so a leg about which artifacts the client fetches has nothing racing it.
async fn serve_feed_dir(root: PathBuf) -> std::net::SocketAddr {
    use axum::extract::{Path as AxPath, State};
    use axum::http::StatusCode;
    let app = axum::Router::new()
        .route(
            "/feed/{*path}",
            axum::routing::get(
                |State(root): State<PathBuf>, AxPath(path): AxPath<String>| async move {
                    match std::fs::read(root.join(&path)) {
                        Ok(bytes) => (StatusCode::OK, bytes),
                        Err(_) => (StatusCode::NOT_FOUND, Vec::new()),
                    }
                },
            ),
        )
        .with_state(root);
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    addr
}

/// S8 — §12 point 1: a snapshot's basis block CAN be proved, so the snapshot
/// carries the anchor sidecar §1.3 always required, and the client grounds on
/// it.
///
/// This leg exists because this repo talked itself out of the sidecar on a
/// measurement error. `getStorageProof` refuses often — a fifth to a half of
/// attempts succeed — and a bisection over that nondeterministic predicate
/// produced the "~1024-block window" that §11 was built on. Retried, proofs
/// come back from 5.15M blocks behind head. So the fixture here refuses the
/// first attempts at every block and then answers, and the anchor must be
/// obtained anyway: the retry is the whole mechanism, and
/// `proofs_denied() > 0` below is what stops this leg passing on an endpoint
/// that never refused anything.
///
/// "The client grounds on it" is pinned two ways: the sidecar is actually
/// FETCHED (and its URL is inside the closed address-blind allowlist), and a
/// sidecar that disagrees with the snapshot's own slot set is REFUSED. Without
/// the second half, publishing the file and ignoring it would pass.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s8_the_snapshot_carries_a_basis_block_anchor_and_the_client_uses_it() {
    ensure_built();
    let seed = seed_chain().await;
    // No window — §12 retracts it. Proofs answer for any block, but only after
    // the aggregator has routed a few attempts to backends without tries.
    const FLAKY: usize = 2;
    let rpc = FixtureRpc::with_faults(
        seed.chain.clone(),
        CHAIN_ID,
        FaultSpec {
            proof_flaky_attempts: FLAKY,
            ..Default::default()
        },
    );
    let addr = rpc.serve().await;
    let url = format!("http://{addr}/");
    let dir = tempfile::tempdir().unwrap();
    let (out, err, ok) = backfill(dir.path(), &url, &seed.pool_hex);
    assert!(ok, "backfill failed\nstdout:\n{out}\nstderr:\n{err}");
    assert!(
        !read_manifest(dir.path())["snapshot"].is_null(),
        "§12 B1 first: this endpoint refuses the first {FLAKY} proof attempts at each \
         block and answers afterwards, exactly as lava does. Without a bounded retry on \
         error 42 nothing downstream happens at all — no proof, no anchor, no snapshot — \
         which is where a single-attempt implementation stands.\nstderr:\n{err}"
    );
    assert_snapshot_published(dir.path());

    let chain = &seed.chain;
    let basis_root = felt_hex(&strk20_feed::mpt::storage_root(&expected_slots(chain, BASIS_BLOCK)));
    let manifest = read_manifest(dir.path());
    let snapshot = &manifest["snapshot"];
    let anchor = &snapshot["anchor"];
    assert!(
        !anchor.is_null(),
        "§12 point 1: the basis block IS provable (with a bounded retry on error 42), so \
         §1.3's required anchor is obtainable and must be published. manifest.snapshot = \
         {snapshot}"
    );
    assert_eq!(
        anchor["block"].as_u64(),
        Some(BASIS_BLOCK),
        "the anchor must be for the snapshot's OWN basis block, not for some later block \
         that happens to be easier to prove: {anchor}"
    );
    assert_eq!(
        anchor["storage_root"].as_str(),
        Some(basis_root.as_str()),
        "the anchor's root must be the chain's root at the basis: {anchor}"
    );
    assert_eq!(
        anchor["block_hash"].as_str(),
        Some(felt_hex(&chain.block_hash(BASIS_BLOCK)).as_str()),
        "§12 B2: the anchor must carry the block hash the proof was BOUND to: {anchor}"
    );
    assert_eq!(
        anchor["class"].as_str(),
        Some(felt_hex(&chain.class_at(BASIS_BLOCK).unwrap_or(Felt::ZERO)).as_str()),
        "§1.5 ring 5 compares header.class with the anchor's class: {anchor}"
    );
    assert_eq!(
        snapshot["grounding"].as_str(),
        Some("basis-anchor"),
        "§12 B4: the manifest is the only published record of HOW this snapshot is \
         grounded, and a client needs it to know whether the reachability walk is its \
         primary check or a fallback: {snapshot}"
    );
    assert_eq!(
        snapshot["storage_root"].as_str(),
        Some(basis_root.as_str()),
        "manifest, snapshot header and anchor must all name one root: {snapshot}"
    );

    let sidecar_path = feed_dir(dir.path()).join(SNAPSHOT_ANCHOR_FILE);
    assert!(
        sidecar_path.exists(),
        "§1.3: the full stored getStorageProof response for the basis block is published \
         as {SNAPSHOT_ANCHOR_FILE}, so the manifest's anchor can be checked against the \
         proof it claims to come from (and, through the snapshot's own root, against the \
         slot set) rather than taken on the manifest's word. It is not offline-strong \
         against the publisher itself — that is reachability's job, and ring 6's, for \
         which this file is the audit material."
    );
    let sidecar: Value = serde_json::from_slice(&std::fs::read(&sidecar_path).unwrap())
        .expect("the anchor sidecar is JSON");
    assert_eq!(
        sidecar["contracts_proof"]["contract_leaves_data"][0]["storage_root"].as_str(),
        Some(basis_root.as_str()),
        "the sidecar must be the proof for the basis block: {sidecar}"
    );
    assert_eq!(
        sidecar["global_roots"]["block_hash"].as_str(),
        Some(felt_hex(&chain.block_hash(BASIS_BLOCK)).as_str()),
        "§12 B2: a stored proof whose block hash is not the block's must never have been \
         accepted, let alone published: {sidecar}"
    );
    assert!(
        rpc.proofs_denied() > 0,
        "vacuity guard: this endpoint never refused a proof, so the leg says nothing \
         about the retry that §12 B1 is entirely about"
    );

    // ---- the client fetches it, inside the closed allowlist
    let static_addr = serve_feed_dir(feed_dir(dir.path())).await;
    let proxy = RecordingProxy::new(&format!("http://{static_addr}"));
    let proxy_addr = proxy.serve().await;
    let feed_url = format!("http://{proxy_addr}/feed");
    proxy.take_captured();
    let (report, ok) = sync_with(
        dir.path(),
        &feed_url,
        &seed.bob,
        "0xb0b",
        "s8-snap.db",
        &["--cold-start", "snapshot"],
    );
    assert!(ok, "snapshot cold start failed: {report}");
    assert_eq!(report["snapshot_basis"].as_u64(), Some(BASIS_BLOCK), "{report}");
    let urls = assert_capture_allowed(&proxy.take_captured(), "snapshot cold start");
    assert!(
        urls.iter().any(|u| u == "/feed/snapshots/00000001.anchor.json"),
        "§1.5 ring 5: the client must fetch the basis-block proof sidecar. It fetched \
         {urls:?}"
    );

    // ---- and it is load-bearing: a sidecar that disagrees is refused
    let honest_notes = client_notes(&report);
    let original = std::fs::read(&sidecar_path).unwrap();
    let mut forged: Value = serde_json::from_slice(&original).unwrap();
    forged["contracts_proof"]["contract_leaves_data"][0]["storage_root"] =
        Value::String(felt_hex(&(strk20_feed::mpt::storage_root(&expected_slots(
            chain,
            BASIS_BLOCK,
        )) + Felt::ONE)));
    std::fs::write(&sidecar_path, serde_json::to_vec_pretty(&forged).unwrap()).unwrap();
    let (err, ok) = sync_with(
        dir.path(),
        &feed_url,
        &seed.bob,
        "0xb0b",
        "s8-forged.db",
        &["--cold-start", "snapshot"],
    );
    assert!(
        !ok,
        "a snapshot whose anchor sidecar does not agree with its own slot set must be \
         refused — otherwise the sidecar is decoration and §1.3 buys nothing: {err}"
    );
    let text = err["error"].as_str().unwrap_or_default().to_owned();
    assert!(
        text.contains("SNAPSHOT_ROOT_MISMATCH"),
        "§1.5 ring 5 names this failure SNAPSHOT_ROOT_MISMATCH. Got:\n{text}"
    );

    // control: the honest bytes still verify, so the refusal was about the lie
    std::fs::write(&sidecar_path, &original).unwrap();
    let (restored, ok) = sync_with(
        dir.path(),
        &feed_url,
        &seed.bob,
        "0xb0b",
        "s8-restored.db",
        &["--cold-start", "snapshot"],
    );
    assert!(ok, "the restored sidecar must verify again: {restored}");
    assert_eq!(client_notes(&restored), honest_notes);

    // ---- §12 B4: the manifest's grounding field is a CLAIM, and a claim with
    // nothing behind it must be refused rather than silently downgraded.
    // Server-side the anchor object and the grounding string come from one
    // Option, so they can only disagree through corruption or malice; a client
    // that accepts the disagreement reports "basis-anchor" in its own log while
    // checking nothing of the kind.
    let manifest_path = feed_dir(dir.path()).join("manifest.json");
    let manifest_bytes = std::fs::read(&manifest_path).unwrap();
    let mut tampered: Value = serde_json::from_slice(&manifest_bytes).unwrap();
    tampered["snapshot"]["anchor"] = Value::Null;
    assert_eq!(
        tampered["snapshot"]["grounding"].as_str(),
        Some("basis-anchor"),
        "the point of this case is a manifest that still CLAIMS the stronger grounding"
    );
    std::fs::write(&manifest_path, serde_json::to_vec_pretty(&tampered).unwrap()).unwrap();
    let (err, ok) = sync_with(
        dir.path(),
        &feed_url,
        &seed.bob,
        "0xb0b",
        "s8-nogrounding.db",
        &["--cold-start", "snapshot"],
    );
    assert!(
        !ok,
        "a manifest declaring grounding \"basis-anchor\" with no anchor to check must be \
         refused: accepting it downgrades every consumer to the fallback while both the \
         manifest and the client log claim otherwise: {err}"
    );
    let text = err["error"].as_str().unwrap_or_default().to_owned();
    assert!(
        text.contains("FEED_MALFORMED"),
        "the two fields are produced together server-side, so a disagreement is a \
         malformed feed and must be named as one. Got:\n{text}"
    );
    std::fs::write(&manifest_path, &manifest_bytes).unwrap();
}

// ------------------------------------------------------------------- S9

/// S9 — §12 B4's other half: when the basis-block anchor cannot be obtained,
/// the snapshot is still PUBLISHED, grounded by the §11.3 reachability check,
/// and the manifest says so.
///
/// Both halves matter. A design that requires the sidecar unconditionally
/// publishes nothing whenever an operator's endpoint cannot serve a deep proof
/// (Juno-backed providers serve head only, by design) — that is the mistake
/// §11 was over-correcting. A design that never publishes the sidecar throws
/// away the one grounding that is bound to the chain at the basis itself.
/// So the outcome is per-snapshot, and it is reported rather than inferred.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn s9_an_unobtainable_basis_anchor_falls_back_to_reachability() {
    ensure_built();
    let seed = seed_chain().await;
    // The fixture's window is narrower than head - basis, so no retry count
    // can obtain a proof at the basis: this endpoint really cannot serve it.
    let (rpc, dir) = backfilled_feed(&seed).await;
    assert_snapshot_published(dir.path());

    let manifest = read_manifest(dir.path());
    let snapshot = &manifest["snapshot"];
    assert!(
        snapshot["anchor"].is_null(),
        "the basis {BASIS_BLOCK} is more than {PROOF_WINDOW} blocks behind head here, so \
         no proof for it exists to publish: {snapshot}"
    );
    assert!(
        !feed_dir(dir.path()).join(SNAPSHOT_ANCHOR_FILE).exists(),
        "no proof was obtained, so no sidecar may be published — an empty or fabricated \
         one would be worse than none"
    );
    assert_eq!(
        snapshot["grounding"].as_str(),
        Some("reachability"),
        "§12 B4: the snapshot is published on the fallback grounding, and the manifest \
         must SAY that rather than leaving a client to infer it from a missing field: \
         {snapshot}"
    );
    assert!(
        rpc.proof_attempts(BASIS_BLOCK) > 0,
        "vacuity guard: no storage proof was ever requested AT THE BASIS BLOCK, so this \
         leg would pass just as well against a build that never tries for a basis anchor \
         at all. `proofs_denied` is not enough on its own — the head-side probe and the \
         per-epoch anchors both feed it."
    );
    assert!(
        rpc.proofs_denied() > 0,
        "vacuity guard: the fixture never refused a proof, so nothing forced the fallback"
    );
    let anchors = published_anchors(dir.path());
    assert!(
        anchors.iter().any(|(b, _)| *b >= BASIS_BLOCK),
        "the fallback gate's input must exist: an anchors.ndjson record at A >= basis \
         {BASIS_BLOCK}; got {:?}",
        anchors.iter().map(|(b, _)| *b).collect::<Vec<_>>()
    );

    // The client still cold-starts, and reachability still grounds it.
    let feed = feed_dir(dir.path()).display().to_string();
    let (report, ok) = sync_with(
        dir.path(),
        &feed,
        &seed.bob,
        "0xb0b",
        "s9-snap.db",
        &["--cold-start", "snapshot"],
    );
    assert!(
        ok,
        "§12 B4: a snapshot without a basis anchor is publishable and consumable — the \
         reachability gate is a FALLBACK, not a removal: {report}"
    );
    assert_eq!(report["snapshot_basis"].as_u64(), Some(BASIS_BLOCK), "{report}");
    let (stdout, stderr, ok) = verify_anchors(&feed, &dir.path().join("s9-snap.db"));
    assert!(
        ok,
        "reachability must still verify the mirror across the snapshot seam\
         \nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
    let v: Value = serde_json::from_str(&stdout).expect("verify-anchors --json report");
    assert_eq!(v["all_ok"], Value::Bool(true), "{v}");
}
