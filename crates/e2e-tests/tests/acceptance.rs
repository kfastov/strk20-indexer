//! THE acceptance e2e (spec §10.3): real binaries, real HTTP, fully offline.
//! Topology: strk20-sync → recording proxy → strk20 → fixture RPC.
//!
//! Legs (run sequentially over one evolving fixture chain):
//!   a. pipeline liveness           g. reorg with cursor rewind
//!   b. keyless discovery equality  h. compat + 409 + detector self-test
//!   c. key-sensitivity control     i. upgrade/degraded mode
//!   d. mechanical no-key proof     j. mirror determinism
//!   e. tamper detection            k. spent-state
//!   f. O(delta) resume + server-side scan

use discovery_core::privacy_pool::types::SecretFelt;
use e2e_tests::bins::{bin, ensure_built, pick_free_port, run_capture, spawn_with_logs, ChildGuard};
use e2e_tests::chain::{FixtureChain, FxEvent};
use e2e_tests::fixture::load_devnet_fixture;
use e2e_tests::oracle;
use e2e_tests::proxy::RecordingProxy;
use e2e_tests::rpc_server::FixtureRpc;
use e2e_tests::scanner;
use serde_json::Value;
use starknet_types_core::felt::Felt;
use std::process::Command;
use std::time::Duration;

const CHAIN_ID: &str = "SN_TEST";
const GENESIS_BLOCK: u64 = 10;
const EPOCH_SIZE: u64 = 16;

struct Ctx {
    rpc_addr: std::net::SocketAddr,
    indexer_port: u16,
    proxy: RecordingProxy,
    proxy_addr: std::net::SocketAddr,
    dir: tempfile::TempDir,
    http: reqwest::Client,
    pool_hex: String,
}

impl Ctx {
    fn indexer_url(&self) -> String {
        format!("http://127.0.0.1:{}", self.indexer_port)
    }
    fn feed_via_proxy(&self) -> String {
        format!("http://{}/feed", self.proxy_addr)
    }
    fn common_args(&self) -> Vec<String> {
        vec![
            "--db".into(),
            self.dir.path().join("strk20.db").display().to_string(),
            "--feed-dir".into(),
            self.dir.path().join("feed").display().to_string(),
            "--rpc-url".into(),
            format!("http://{}/", self.rpc_addr),
            "--rpc-fallback".into(),
            format!("http://{}/", self.rpc_addr),
            "--pool".into(),
            self.pool_hex.clone(),
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

    fn spawn_indexer(&self, tag: &str, extra: &[&str]) -> ChildGuard {
        let mut cmd = Command::new(bin("strk20"));
        cmd.arg("run")
            .args(self.common_args())
            .args(["--listen", &format!("127.0.0.1:{}", self.indexer_port)])
            .args(["--poll-ms", "150"])
            .args(extra);
        spawn_with_logs(cmd, self.dir.path(), tag)
    }

    async fn wait_health(&self, want_epoch: u64) {
        for _ in 0..300 {
            if let Ok(resp) = self
                .http
                .get(format!("{}/health", self.indexer_url()))
                .send()
                .await
            {
                if let Ok(v) = resp.json::<Value>().await {
                    if v["status"] == "OK"
                        && v["latest_epoch"].as_u64() == Some(want_epoch)
                    {
                        return;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        self.dump_logs();
        panic!("indexer did not become healthy with epoch {want_epoch}");
    }

    async fn wait_head(&self, want_head: u64) {
        for _ in 0..300 {
            if let Ok(resp) = self
                .http
                .get(format!("{}/health", self.indexer_url()))
                .send()
                .await
            {
                if let Ok(v) = resp.json::<Value>().await {
                    if v["head"]["number"].as_u64() == Some(want_head) {
                        return;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        self.dump_logs();
        panic!("indexer did not reach head {want_head}");
    }

    fn dump_logs(&self) {
        for entry in std::fs::read_dir(self.dir.path()).into_iter().flatten().flatten() {
            let p = entry.path();
            if p.extension().map(|e| e == "log").unwrap_or(false) {
                let content = std::fs::read_to_string(&p).unwrap_or_default();
                let tail: Vec<&str> = content.lines().rev().take(25).collect();
                eprintln!("==== {} (last 25 lines) ====", p.display());
                for line in tail.iter().rev() {
                    eprintln!("{line}");
                }
            }
        }
    }

    fn sync_client(&self, tag: &str, address: &Felt, key_hex: &str, db_name: &str) -> (Value, bool) {
        self.sync_client_with(tag, address, key_hex, db_name, &[])
    }

    fn sync_client_with(
        &self,
        tag: &str,
        address: &Felt,
        key_hex: &str,
        db_name: &str,
        extra: &[&str],
    ) -> (Value, bool) {
        let key_path = self.dir.path().join(format!("{tag}.key"));
        std::fs::write(&key_path, key_hex).unwrap();
        let mut cmd = Command::new(bin("strk20-sync"));
        cmd.arg("sync")
            .args(["--feed", &self.feed_via_proxy()])
            .args(["--address", &strk20_feed::felt_hex(address)])
            .args(["--key-file", &key_path.display().to_string()])
            .args(["--db", &self.dir.path().join(db_name).display().to_string()])
            .args(extra)
            .arg("--json");
        let (stdout, stderr, success) = run_capture(cmd, false);
        let report = if success {
            serde_json::from_str(&stdout).unwrap_or_else(|e| {
                panic!("bad report json ({e})\nstdout:\n{stdout}\nstderr:\n{stderr}")
            })
        } else {
            serde_json::json!({ "error": stderr })
        };
        (report, success)
    }
}

/// The report's UNSPENT notes, in the oracle's shape. The filter is the whole
/// point of the comparison: `oracle::incoming` is the raw engine, which drops
/// every spent index before it reads the note slot, whereas the client's
/// report also carries the spent ones (flagged) so that it does not depend on
/// when the client started — see `conformance.rs` leg 5. Comparing the two
/// therefore compares like with like.
fn client_notes_canonical(report: &Value) -> Vec<Value> {
    let mut v: Vec<Value> = report["notes"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .filter(|n| n["spent"] != true)
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

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn acceptance() {
    ensure_built();
    let fixture = load_devnet_fixture();
    let alice = fixture.constants.alice_address;
    let bob = fixture.constants.bob_address;
    let alice_key = fixture.constants.alice_viewing_key;
    let bob_key = fixture.constants.bob_viewing_key;
    let pool = fixture.constants.contract_address;

    // The fixture's only bob note is SPENT (upstream's own tests assert the
    // engine filters it), so the chain gets two freshly minted UNSPENT notes
    // for bob at block 31 (inside epoch 1) — valid ciphertexts built with
    // the engine's own crypto.
    let plain_backend = discovery_core::storage_backend::MockBackend::new(fixture.slots.clone());
    let bob_plain = oracle::incoming(&plain_backend, bob, &SecretFelt::new(bob_key)).await;
    let bob_ck = oracle::channel_key_of(&bob_plain, &alice);
    let strk = fixture.constants.strk_token;
    let base_index = bob_plain
        .cursor
        .channels
        .get(&alice)
        .and_then(|c| c.subchannels.get(&strk))
        .and_then(|s| s.total_n_notes)
        .expect("fixture subchannel note total");
    let enc_sel = Felt::from_hex(e2e_tests::chain::ENC_NOTE_CREATED_SELECTOR).unwrap();
    let m1 = oracle::mint_note(&bob_ck, strk, base_index, 500, &SecretFelt::new(bob_key));
    let m2 = oracle::mint_note(&bob_ck, strk, base_index + 1, 600, &SecretFelt::new(bob_key));
    let mut chain = FixtureChain::build(&fixture);
    for m in [&m1, &m2] {
        chain.add_note_block(
            31,
            m.slot,
            m.packed_value,
            FxEvent {
                keys: vec![enc_sel, m.note_id],
                data: vec![m.packed_value],
            },
        );
    }
    chain.head = 46;
    let rpc = FixtureRpc::new(chain, CHAIN_ID);
    let rpc_addr = rpc.serve().await;

    let indexer_port = pick_free_port();
    let dir = tempfile::tempdir().unwrap();
    let proxy = RecordingProxy::new(&format!("http://127.0.0.1:{indexer_port}"));
    let proxy_addr = proxy.serve().await;
    let ctx = Ctx {
        rpc_addr,
        indexer_port,
        proxy,
        proxy_addr,
        dir,
        http: reqwest::Client::new(),
        pool_hex: strk20_feed::felt_hex(&pool),
    };

    // ---------------------------------------------------------- leg a
    let indexer = ctx.spawn_indexer("indexer", &[]);
    ctx.wait_health(1).await; // epochs 0 [0,15] and 1 [16,31] cut
    println!("leg a OK: pipeline live, epoch 1 cut");

    // ---------------------------------------------------------- leg b
    // Oracle O1 over the same 48 slots + committed write blocks.
    let o1_backend = {
        let chain = ctx_chain(&rpc);
        oracle::backend_at(&chain, 46)
    };
    let o1_bob = oracle::incoming(&o1_backend, bob, &SecretFelt::new(bob_key)).await;
    let o1_alice = oracle::incoming(&o1_backend, alice, &SecretFelt::new(alice_key)).await;
    assert_eq!(
        o1_bob.notes.len(),
        2,
        "sanity: bob has exactly the two minted unspent notes"
    );
    assert!(
        o1_bob.notes.iter().all(|n| n.block_number == 31),
        "minted notes were committed at block 31"
    );

    ctx.proxy.take_captured();
    let (bob_report, ok) = ctx.sync_client("bob", &bob, "0xb0b", "bob.db");
    assert!(ok, "bob sync failed: {bob_report}");
    let bob_capture = ctx.proxy.take_captured();
    assert_eq!(
        client_notes_canonical(&bob_report),
        oracle::notes_canonical(&o1_bob.notes),
        "bob keyless notes != oracle"
    );
    assert_eq!(bob_report["incoming_complete"], true);
    assert_eq!(bob_report["outgoing_complete"], true);
    // per-note creation block == committed partition block (write_block capability)
    {
        let chain = ctx_chain(&rpc);
        for n in bob_report["notes"].as_array().unwrap() {
            let note_id = Felt::from_hex(n["note_id"].as_str().unwrap()).unwrap();
            let slot = discovery_core::privacy_pool::storage_slots::notes(note_id);
            assert_eq!(
                n["block_number"].as_u64(),
                chain.write_block_of(&slot),
                "note creation block must equal its committed partition block"
            );
        }
    }

    let (alice_report, ok) = ctx.sync_client("alice", &alice, "0xa11ce", "alice.db");
    assert!(ok, "alice sync failed: {alice_report}");
    let alice_capture = ctx.proxy.take_captured();
    assert_eq!(
        client_notes_canonical(&alice_report),
        oracle::notes_canonical(&o1_alice.notes),
        "alice keyless notes != oracle"
    );

    let unused = Felt::from_hex("0x777777").unwrap();
    let (unused_report, ok) = ctx.sync_client("unused", &unused, "0x123", "unused.db");
    assert!(ok, "unused-address sync failed: {unused_report}");
    let unused_capture = ctx.proxy.take_captured();
    assert!(unused_report["notes"].as_array().unwrap().is_empty());
    assert_eq!(unused_report["incoming_complete"], true);
    // typed stats must count EVERY event, incl. the 3-event block that spans
    // two getEvents pages (review regression: per-block event truncation)
    let stats: Value = ctx
        .http
        .get(format!("{}/v1/stats", ctx.indexer_url()))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(
        stats["note_count"].as_u64(),
        Some(6),
        "EncNoteCreated count must survive per-block event pagination: {stats}"
    );
    assert_eq!(stats["registrations"].as_u64(), Some(1));
    println!("leg b OK: keyless discovery equals oracle for alice, bob, unused; stats complete");

    // ---------------------------------------------------------- leg c
    let (wrong_report, ok) = ctx.sync_client("wrongkey", &alice, "0xdead", "wrong.db");
    assert!(ok, "wrong-key sync errored: {wrong_report}");
    let wrong_capture = ctx.proxy.take_captured();
    assert!(
        wrong_report["notes"].as_array().unwrap().is_empty(),
        "wrong key must discover zero notes"
    );
    assert!(wrong_report["balances"].as_object().unwrap().is_empty());
    println!("leg c OK: results depend on the key");

    // ---------------------------------------------------------- leg d
    let mut secrets: Vec<(Felt, String)> = vec![
        (alice_key, "alice-key".into()),
        (bob_key, "bob-key".into()),
        (alice, "alice-address".into()),
        (bob, "bob-address".into()),
    ];
    for (sender, cur) in o1_bob.cursor.channels.iter() {
        secrets.push((
            *cur.channel_key,
            format!("bob-channel-key-from-{}", strk20_feed::felt_hex(sender)),
        ));
    }
    for (sender, cur) in o1_alice.cursor.channels.iter() {
        secrets.push((
            *cur.channel_key,
            format!("alice-channel-key-from-{}", strk20_feed::felt_hex(sender)),
        ));
    }
    let all_captures = [&bob_capture, &alice_capture, &unused_capture, &wrong_capture];
    let mut url_multisets: Vec<Vec<String>> = Vec::new();
    for capture in all_captures {
        let mut urls = Vec::new();
        let mut haystack = Vec::new();
        for req in capture.iter() {
            assert_eq!(req.method, "GET", "keyless client must only GET: {}", req.uri);
            assert!(req.body.is_empty(), "keyless GET must have no body");
            assert!(
                !req.uri.contains('?'),
                "keyless URL must carry no query: {}",
                req.uri
            );
            // §2.8.1: the allowlist is CLOSED and matched WHOLE-PATH, never by
            // prefix and never by a startsWith('/feed/') test — that is how the
            // property erodes the first time a new artifact turns this red.
            assert!(
                e2e_tests::feed_urls::is_allowed(&req.uri),
                "keyless request path {} is outside the closed allowlist {:?}",
                req.uri,
                e2e_tests::feed_urls::PATTERNS
            );
            // The Rust sync path is polling-only: /feed/live is in the closed
            // set for the npm client, and must not appear here (§2.5).
            assert_ne!(req.uri, "/feed/live", "the sync path must not subscribe");
            urls.push(req.uri.clone());
            haystack.extend_from_slice(&req.all_bytes());
        }
        let hits = scanner::scan(&haystack, &secrets);
        assert!(hits.is_empty(), "secret material on the wire: {hits:?}");
        urls.sort();
        url_multisets.push(urls);
    }
    // address-blindness: alice's, bob's and unused's request sets identical
    assert_eq!(url_multisets[0], url_multisets[1]);
    assert_eq!(url_multisets[0], url_multisets[2]);
    println!("leg d OK: no key/address/channel-key bytes on the wire; request sets address-blind");

    // ---------------------------------------------------------- leg e
    let epoch0 = ctx.dir.path().join("feed/epochs/00000000.strk20e.zst");
    let original = std::fs::read(&epoch0).unwrap();
    let mut tampered = original.clone();
    let mid = tampered.len() / 2;
    tampered[mid] ^= 0xff;
    std::fs::write(&epoch0, &tampered).unwrap();
    // Explicitly the EPOCH path. Since A1 the default `auto` cold start folds
    // the published snapshot and never reads an epoch at or below its basis,
    // so a tampered epoch 0 is simply not on that client's fetch list — this
    // leg is about epoch integrity, and the snapshot path has its own tamper
    // leg (snapshots.rs S4, including the case only reachability catches).
    let (tamper_report, ok) =
        ctx.sync_client_with("tamper", &bob, "0xb0b", "tamper.db", &["--cold-start", "epochs"]);
    assert!(!ok, "client must reject a tampered epoch");
    let err = tamper_report["error"].as_str().unwrap_or_default();
    assert!(
        err.contains("mismatch") || err.contains("ecompress") || err.contains("alformed"),
        "tamper error must name the failure: {err}"
    );
    std::fs::write(&epoch0, &original).unwrap();
    let mut cmd = Command::new(bin("strk20"));
    cmd.arg("epoch-verify").args(ctx.common_args());
    let (out, _, ok) = run_capture(cmd, true);
    assert!(ok && out.contains("hash chain OK"));
    println!("leg e OK: tampered epoch detected by the client; chain verifies after restore");

    // ---------------------------------------------------------- leg f (resume)
    ctx.proxy.take_captured();
    let (resumed, ok) = ctx.sync_client("bob2", &bob, "0xb0b", "bob.db");
    assert!(ok, "resumed sync failed: {resumed}");
    let resume_capture = ctx.proxy.take_captured();
    for req in &resume_capture {
        assert!(
            !req.uri.starts_with("/feed/epochs/"),
            "resume must not refetch epochs: {}",
            req.uri
        );
    }
    assert_eq!(
        client_notes_canonical(&resumed),
        oracle::notes_canonical(&o1_bob.notes),
        "resumed sync must reproduce the same notes"
    );
    println!("leg f OK: O(delta) resume — no epoch refetch, same result");

    // ---------------------------------------------------------- leg g (reorg)
    // Pre-fork: a NEW tail block 47 carries a freshly minted note for bob
    // (real chains only append; data never lands in already-sealed heights).
    let next_index = base_index + 2;
    let n1 = oracle::mint_note(&bob_ck, strk, next_index, 777, &SecretFelt::new(bob_key));
    let etag_before = ctx.head_hash().await;
    {
        let mut chain = rpc.chain.write().unwrap();
        chain.add_note_block(
            47,
            n1.slot,
            n1.packed_value,
            FxEvent {
                keys: vec![enc_sel, n1.note_id],
                data: vec![n1.packed_value],
            },
        );
    }
    ctx.wait_head_etag_change(&etag_before).await;
    let (pre_fork, ok) = ctx.sync_client("bob-prefork", &bob, "0xb0b", "bob.db");
    assert!(ok, "pre-fork sync failed: {pre_fork}");
    let pre_fork_notes = client_notes_canonical(&pre_fork);
    assert!(
        pre_fork_notes
            .iter()
            .any(|n| n["block_number"].as_u64() == Some(47) && n["amount"] == "777"),
        "pre-fork tail note must be discovered: {pre_fork_notes:?}"
    );
    let pre_fork_head_hash = {
        let chain = ctx_chain(&rpc);
        chain.block_hash(47)
    };

    // Fork blocks >= 45: N1 moves to 45', a new N2 appears at 46', head 47'.
    let n2 = oracle::mint_note(
        &bob_ck,
        strk,
        next_index + 1,
        888,
        &SecretFelt::new(bob_key),
    );
    let etag_before = ctx.head_hash().await;
    {
        let mut chain = rpc.chain.write().unwrap();
        chain.fork_tail(45);
        chain.add_note_block(
            45,
            n1.slot,
            n1.packed_value,
            FxEvent {
                keys: vec![enc_sel, n1.note_id],
                data: vec![n1.packed_value],
            },
        );
        chain.add_note_block(
            46,
            n2.slot,
            n2.packed_value,
            FxEvent {
                keys: vec![enc_sel, n2.note_id],
                data: vec![n2.packed_value],
            },
        );
        chain.head = 47;
    }
    ctx.wait_head_etag_change(&etag_before).await;
    // epoch files must be byte-untouched by the reorg
    assert_eq!(std::fs::read(&epoch0).unwrap(), original);

    let (post_fork, ok) = ctx.sync_client("bob-postfork", &bob, "0xb0b", "bob.db");
    assert!(ok, "post-fork sync failed: {post_fork}");
    assert_eq!(post_fork["tail_rewound"], true, "client must detect the reorg");
    let o1_post = {
        let chain = ctx_chain(&rpc);
        let backend = oracle::backend_at(&chain, 47);
        oracle::incoming(&backend, bob, &SecretFelt::new(bob_key)).await
    };
    assert_eq!(
        o1_post.notes.len(),
        o1_bob.notes.len() + 2,
        "post-fork oracle sanity: both minted notes present"
    );
    assert_eq!(
        client_notes_canonical(&post_fork),
        oracle::notes_canonical(&o1_post.notes),
        "post-fork notes must equal the re-seeded oracle"
    );
    // a further sync is a no-op with identical results
    let (noop, ok) = ctx.sync_client("bob-noop", &bob, "0xb0b", "bob.db");
    assert!(ok);
    assert_eq!(noop["tail_rewound"], false);
    assert_eq!(client_notes_canonical(&noop), client_notes_canonical(&post_fork));
    println!("leg g OK: reorg detected, cursor rewound to L1 checkpoint, epochs untouched");

    // ---------------------------------------------------------- leg h (compat)
    drop(indexer);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let indexer = ctx.spawn_indexer("indexer-compat", &["--enable-compat"]);
    ctx.wait_head(47).await;

    let compat_body = serde_json::json!({
        "contract_address": ctx.pool_hex,
        "viewing_key": "0xb0b",
        "recipient_address": strk20_feed::felt_hex(&bob),
    });
    let compat_bytes = serde_json::to_vec(&compat_body).unwrap();
    let resp = ctx
        .http
        .post(format!("{}/v1/sync/incoming_state", ctx.indexer_url()))
        .header("content-type", "application/json")
        .body(compat_bytes.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(
        resp.headers().get("x-strk20-mode").and_then(|v| v.to_str().ok()),
        Some("compat-keyed")
    );
    assert!(resp.status().is_success(), "compat sync failed: {}", resp.status());
    let compat: Value = resp.json().await.unwrap();
    // drive pagination to completion like the SDK would
    let mut compat_notes: Vec<Value> = compat["notes"].as_array().cloned().unwrap_or_default();
    let mut cursor = compat["cursor"].clone();
    for _ in 0..50 {
        let complete = cursor["channel_discovery_complete"] == true;
        let _ = complete;
        let body = serde_json::json!({
            "contract_address": ctx.pool_hex,
            "viewing_key": "0xb0b",
            "recipient_address": strk20_feed::felt_hex(&bob),
            "cursor": cursor,
        });
        let resp = ctx
            .http
            .post(format!("{}/v1/sync/incoming_state", ctx.indexer_url()))
            .json(&body)
            .send()
            .await
            .unwrap();
        assert!(resp.status().is_success());
        let page: Value = resp.json().await.unwrap();
        compat_notes.extend(page["notes"].as_array().cloned().unwrap_or_default());
        let new_cursor = page["cursor"].clone();
        if new_cursor == cursor {
            break;
        }
        cursor = new_cursor;
    }
    // cursor round-trips into the client's persisted type (type identity)
    let _: discovery_core::discovery::DiscoveryCursor =
        serde_json::from_value(cursor.clone()).expect("compat cursor round-trips");
    let mut compat_canonical: Vec<Value> = compat_notes
        .iter()
        .map(|n| {
            serde_json::json!({
                "sender": n["sender_addr"],
                "token": n["token"],
                "index": n["index"],
                "note_id": n["note_id"],
                "amount": n["amount"],
                "block_number": n["block_number"],
            })
        })
        .collect();
    compat_canonical.sort_by_key(|j| {
        (
            j["token"].as_str().unwrap_or("").to_owned(),
            j["index"].as_u64().unwrap_or(0),
        )
    });
    assert_eq!(
        compat_canonical,
        oracle::notes_canonical(&o1_post.notes),
        "compat notes must equal the oracle"
    );

    // detector self-test: the same scanner MUST find the key in compat bytes
    let self_test = scanner::scan(&compat_bytes, &[(bob_key, "bob-key".into())]);
    assert!(
        !self_test.is_empty(),
        "detector self-test failed: scanner blind to the key in a compat body"
    );

    // 409 on the reorged-out pre-fork head hash
    let resp = ctx
        .http
        .post(format!("{}/v1/sync/incoming_state", ctx.indexer_url()))
        .json(&serde_json::json!({
            "contract_address": ctx.pool_hex,
            "viewing_key": "0xb0b",
            "recipient_address": strk20_feed::felt_hex(&bob),
            "last_known_block": strk20_feed::felt_hex(&pre_fork_head_hash),
        }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 409, "reorged block_ref must 409");
    let body: Value = resp.json().await.unwrap();
    assert_eq!(body["error"]["code"], "BLOCK_REORGED");
    println!("leg h OK: compat wire equals oracle, cursor interops, 409 on reorg, detector sees keys");

    // ---------------------------------------------------- leg f(iii) server scan
    let mut server_side = Vec::new();
    // main db AND its live WAL/SHM (review finding: the un-checkpointed WAL
    // is where fresh writes actually live while the server runs)
    for name in ["strk20.db", "strk20.db-wal", "strk20.db-shm"] {
        server_side.extend(std::fs::read(ctx.dir.path().join(name)).unwrap_or_default());
    }
    for entry in walk(ctx.dir.path().join("feed")) {
        let bytes = std::fs::read(&entry).unwrap();
        // scan zstd payloads decompressed (review finding: compressed epochs
        // were opaque to the scanner)
        if entry.extension().map(|e| e == "zst").unwrap_or(false) {
            if let Ok(raw) = strk20_feed::decompress(&bytes) {
                server_side.extend(raw);
            }
        }
        server_side.extend(bytes);
    }
    server_side.extend(std::fs::read(&indexer.stdout_path).unwrap_or_default());
    server_side.extend(std::fs::read(&indexer.stderr_path).unwrap_or_default());
    // NOTE: the compat leg intentionally sent the key to the server; compat
    // hardening forbids LOGGING it, and the DB/feed must never contain it.
    let key_only: Vec<(Felt, String)> = secrets
        .iter()
        .filter(|(_, l)| l.contains("key"))
        .cloned()
        .collect();
    let hits = scanner::scan(&server_side, &key_only);
    assert!(
        hits.is_empty(),
        "key material persisted server-side: {hits:?}"
    );
    println!("leg f OK: server-side scan clean (db, feed, logs)");

    // ---------------------------------------------------------- leg i (degraded)
    let unknown_class = Felt::from_hex("0xbadc1a55badc1a55").unwrap();
    {
        let mut chain = rpc.chain.write().unwrap();
        let blk = chain.active.entry(48).or_default();
        blk.replaced_class = Some(unknown_class);
        blk.events.push(FxEvent {
            keys: vec![
                Felt::from_hex(e2e_tests::chain::VIEWING_KEY_SET_SELECTOR).unwrap(),
                Felt::from_hex("0x1234").unwrap(),
            ],
            data: vec![],
        });
        chain.head = 49;
    }
    ctx.wait_head(49).await;
    let health: Value = ctx
        .http
        .get(format!("{}/health", ctx.indexer_url()))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(health["decode_state"], "degraded");
    assert_eq!(health["status"], "DEGRADED");
    // feed continues: head.ndjson carries the rc line
    let head_txt = ctx
        .http
        .get(format!("{}/feed/head.ndjson", ctx.indexer_url()))
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(head_txt.contains("\"rc\":"), "tail must carry the class change");
    // compat is gated
    let resp = ctx
        .http
        .post(format!("{}/v1/sync/incoming_state", ctx.indexer_url()))
        .json(&compat_body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 503, "degraded compat must 503");
    // recovery: restart with the class allowed
    drop(indexer);
    tokio::time::sleep(Duration::from_millis(300)).await;
    let allow = strk20_feed::felt_hex(&unknown_class).to_string();
    let indexer = ctx.spawn_indexer(
        "indexer-recovered",
        &["--enable-compat", "--allow-class", &allow],
    );
    ctx.wait_head(49).await;
    let health: Value = ctx
        .http
        .get(format!("{}/health", ctx.indexer_url()))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(health["decode_state"], "ok");
    assert_eq!(health["status"], "OK");
    println!("leg i OK: unknown class degrades typed serving, feed continues, recovery via --allow-class");

    // ---------------------------------------------------------- leg j (determinism)
    let dir2 = tempfile::tempdir().unwrap();
    let mut cmd = Command::new(bin("strk20"));
    cmd.arg("backfill")
        .args([
            "--db",
            &dir2.path().join("strk20.db").display().to_string(),
            "--feed-dir",
            &dir2.path().join("feed").display().to_string(),
            "--rpc-url",
            &format!("http://{}/", ctx.rpc_addr),
            "--rpc-fallback",
            &format!("http://{}/", ctx.rpc_addr),
            "--pool",
            &ctx.pool_hex,
            "--chain-id",
            CHAIN_ID,
            "--genesis-block",
            &GENESIS_BLOCK.to_string(),
            "--epoch-size",
            &EPOCH_SIZE.to_string(),
            "--chunk-size",
            "3",
            "--allow-class",
            &allow,
        ]);
    run_capture(cmd, true);
    for name in ["00000000.strk20e.zst", "00000001.strk20e.zst"] {
        let a = std::fs::read(ctx.dir.path().join("feed/epochs").join(name)).unwrap();
        let b = std::fs::read(dir2.path().join("feed/epochs").join(name)).unwrap();
        assert_eq!(a, b, "epoch {name} must be byte-identical across operators");
    }
    println!("leg j OK: independent backfill produces byte-identical epochs");

    // ---------------------------------------------------------- leg k (spent)
    let spent_note = &o1_post.notes[0];
    let spent_ck = oracle::channel_key_of(&o1_post, &spent_note.sender_addr);
    let nf = discovery_core::privacy_pool::hashes::compute_nullifier(
        &spent_ck,
        spent_note.token,
        spent_note.index,
        &SecretFelt::new(bob_key),
    );
    {
        let mut chain = rpc.chain.write().unwrap();
        let slot = discovery_core::privacy_pool::storage_slots::nullifiers(nf);
        chain.add_note_block(
            50,
            slot,
            Felt::ONE,
            FxEvent {
                keys: vec![
                    Felt::from_hex(e2e_tests::chain::NOTE_USED_SELECTOR).unwrap(),
                    nf,
                ],
                data: vec![],
            },
        );
        chain.head = 50;
    }
    ctx.wait_head(50).await;
    let (after_spend, ok) = ctx.sync_client("bob-spend", &bob, "0xb0b", "bob.db");
    assert!(ok, "post-spend sync failed: {after_spend}");
    // `newly_spent` is the delta and carries the claim: EXACTLY the nullifier
    // we just wrote flipped in this sync. The count of spent notes is not the
    // same statement — the devnet seed already contains a spent note for bob,
    // which this client reported (flagged) on its very first sync.
    assert_eq!(
        after_spend["newly_spent"].as_array().unwrap(),
        &vec![Value::String(strk20_feed::felt_hex(&nf))],
        "exactly one nullifier must flip in this sync: {after_spend}"
    );
    let spent: Vec<&Value> = after_spend["notes"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|n| n["spent"] == true)
        .collect();
    assert!(
        spent
            .iter()
            .any(|n| n["nullifier"].as_str() == Some(strk20_feed::felt_hex(&nf).as_str())),
        "and the note it belongs to must be reported spent: {after_spend}"
    );
    assert!(
        !after_spend["balances"]
            .as_object()
            .unwrap()
            .is_empty(),
        "sanity: bob's other note is untouched, so a balance remains: {after_spend}"
    );
    println!("leg k OK: spent-state flips exactly the spent note");

    drop(indexer);
}

/// Snapshot of the current fixture chain (clone under the lock).
fn ctx_chain(rpc: &FixtureRpc) -> FixtureChain {
    rpc.chain.read().unwrap().clone()
}

fn walk(dir: std::path::PathBuf) -> Vec<std::path::PathBuf> {
    let mut out = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                out.extend(walk(p));
            } else {
                out.push(p);
            }
        }
    }
    out
}

impl Ctx {
    /// Wait until the served head ETag differs from `initial`.
    async fn wait_head_etag_change(&self, initial: &str) {
        for _ in 0..150 {
            tokio::time::sleep(Duration::from_millis(200)).await;
            let now = self.head_hash().await;
            if !now.is_empty() && now != initial {
                return;
            }
        }
        panic!("head etag did not change");
    }

    async fn head_hash(&self) -> String {
        self.http
            .get(format!("{}/feed/head.ndjson", self.indexer_url()))
            .send()
            .await
            .ok()
            .map(|r| r.headers().get("etag").and_then(|v| v.to_str().ok()).unwrap_or("").to_owned())
            .unwrap_or_default()
    }
}
