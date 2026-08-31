//! SSE legs (consumer-path.md §A2).
//!
//! `/feed/live` is a NOTIFICATION plane, never a data plane (R-C): every
//! event is state-carrying and idempotent, and on any event the client
//! fetches the same files it would have polled, through the one existing
//! verified path. A lost, duplicated, reordered or buffered event costs
//! latency only — which is why polling remains the reference semantics and a
//! plain static-file mirror (no stream at all) stays a fully supported
//! deployment.
//!
//! E1 framing + a poked client reaches the state a polling client reaches
//! E2 the stream is identical for every subscriber, and nothing user-derived
//!    can enter the subscription
//! E3 a killed and then absent stream degrades to polling and converges

use e2e_tests::bins::{bin, ensure_built, pick_free_port, spawn_with_logs, ChildGuard};
use e2e_tests::chain::{FixtureChain, FxEvent, ENC_NOTE_CREATED_SELECTOR};
use e2e_tests::fixture::load_devnet_fixture;
use e2e_tests::oracle::{self, MintedNote};
use e2e_tests::rpc_server::FixtureRpc;
use e2e_tests::scanner;
use e2e_tests::sse::{SseEvent, SseStream};
use e2e_tests::tcp_proxy::{LivePolicy, TcpProxy};
use e2e_tests::{feed_urls, sse};
use discovery_core::privacy_pool::types::SecretFelt;
use serde_json::Value;
use starknet_types_core::felt::Felt;
use std::path::Path;
use std::process::Command;
use std::time::Duration;
use strk20_feed::felt_hex;

const CHAIN_ID: &str = "SN_TEST";
const GENESIS_BLOCK: u64 = 10;
const EPOCH_SIZE: u64 = 16;
/// §2.2: the connect-time padding comment that defeats buffering middleboxes.
const PADDING_BYTES: usize = 2048;

struct Fx {
    rpc: FixtureRpc,
    dir: tempfile::TempDir,
    port: u16,
    http: reqwest::Client,
    bob: Felt,
    bob_key: Felt,
    alice: Felt,
    alice_key: Felt,
    pool_hex: String,
    channel_key: discovery_core::privacy_pool::types::SecretFelt,
    strk: Felt,
    next_index: u64,
    _indexer: ChildGuard,
}

impl Fx {
    fn base(&self) -> String {
        format!("http://127.0.0.1:{}", self.port)
    }
    fn live_url(&self) -> String {
        format!("{}/feed/live", self.base())
    }
    async fn manifest(&self) -> Value {
        self.http
            .get(format!("{}/feed/manifest.json", self.base()))
            .send()
            .await
            .unwrap()
            .json()
            .await
            .unwrap()
    }
    async fn head_etag(&self) -> String {
        let resp = self
            .http
            .get(format!("{}/feed/head.ndjson", self.base()))
            .send()
            .await
            .unwrap();
        resp.headers()
            .get("etag")
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_owned()
    }
    /// The connect burst is about PUBLISHED FILES, and `wait_head` only proves
    /// the indexer ingested a block. A fresh indexer legitimately publishes a
    /// head tail before it has cut anything — §2.2 announces the current epoch
    /// and snapshot "if any" — so a test that asserts they are announced must
    /// first wait for the feed itself to settle: the epoch cut, the snapshot
    /// published (the §11.3 gate is met as soon as the head-side anchor lands),
    /// and head.ndjson regenerated above the new epoch floor.
    async fn wait_feed_settled(&self) {
        for _ in 0..300 {
            if let Some(want_tail) = self.settled_tail_from().await {
                if let Ok(resp) = self
                    .http
                    .get(format!("{}/feed/head.ndjson", self.base()))
                    .send()
                    .await
                {
                    if let Ok(text) = resp.text().await {
                        if text.contains(&format!("\"tail_from\":{want_tail}")) {
                            return;
                        }
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        panic!("the feed never published an epoch + snapshot");
    }

    /// `to + 1` of the newest cut epoch, once a snapshot has been published.
    async fn settled_tail_from(&self) -> Option<u64> {
        let resp = self
            .http
            .get(format!("{}/feed/manifest.json", self.base()))
            .send()
            .await
            .ok()?;
        let v: Value = resp.json().await.ok()?;
        if v["snapshot"].is_null() {
            return None;
        }
        let latest = v["latest_epoch"].as_u64()?;
        let entry = v["epochs"]
            .as_array()?
            .iter()
            .find(|e| e["e"].as_u64() == Some(latest))?;
        Some(entry["to"].as_u64()? + 1)
    }

    async fn wait_head(&self, want: u64) {
        for _ in 0..300 {
            if let Ok(resp) = self.http.get(format!("{}/health", self.base())).send().await {
                if let Ok(v) = resp.json::<Value>().await {
                    if v["head"]["number"].as_u64() == Some(want) {
                        return;
                    }
                }
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
        panic!("indexer did not reach head {want}");
    }

    /// Append an unspent note for bob at `block` and advance the head.
    fn mint_at(&mut self, block: u64, amount: u128) -> MintedNote {
        let note = oracle::mint_note(
            &self.channel_key,
            self.strk,
            self.next_index,
            amount,
            &SecretFelt::new(self.bob_key),
        );
        self.next_index += 1;
        let mut chain = self.rpc.chain.write().unwrap();
        chain.add_note_block(
            block,
            note.slot,
            note.packed_value,
            FxEvent {
                keys: vec![
                    Felt::from_hex(ENC_NOTE_CREATED_SELECTOR).unwrap(),
                    note.note_id,
                ],
                data: vec![note.packed_value],
            },
        );
        chain.head = block;
        note
    }

    /// Spawn `strk20-sync sync --watch` and return its stdout log path.
    fn spawn_watcher(
        &self,
        tag: &str,
        feed: &str,
        address: &Felt,
        key_hex: &str,
        interval_secs: u64,
    ) -> ChildGuard {
        let key_path = self.dir.path().join(format!("{tag}.key"));
        std::fs::write(&key_path, key_hex).unwrap();
        let mut cmd = Command::new(bin("strk20-sync"));
        cmd.arg("sync")
            .args(["--feed", feed])
            .args(["--address", &felt_hex(address)])
            .args(["--key-file", &key_path.display().to_string()])
            .args(["--db", &self.dir.path().join(format!("{tag}.db")).display().to_string()])
            .arg("--watch")
            .args(["--interval", &interval_secs.to_string()])
            .arg("--json");
        spawn_with_logs(cmd, self.dir.path(), tag)
    }
}

async fn setup() -> Fx {
    ensure_built();
    let fixture = load_devnet_fixture();
    let bob = fixture.constants.bob_address;
    let alice = fixture.constants.alice_address;
    let bob_key = fixture.constants.bob_viewing_key;
    let strk = fixture.constants.strk_token;

    let plain = discovery_core::storage_backend::MockBackend::new(fixture.slots.clone());
    let bob_plain = oracle::incoming(&plain, bob, &SecretFelt::new(bob_key)).await;
    let channel_key = oracle::channel_key_of(&bob_plain, &alice);
    let next_index = bob_plain
        .cursor
        .channels
        .get(&alice)
        .and_then(|c| c.subchannels.get(&strk))
        .and_then(|s| s.total_n_notes)
        .expect("fixture subchannel note total");

    let rpc = FixtureRpc::new(FixtureChain::build(&fixture), CHAIN_ID);
    let rpc_addr = rpc.serve().await;
    let dir = tempfile::tempdir().unwrap();
    let port = pick_free_port();

    let mut cmd = Command::new(bin("strk20"));
    cmd.arg("run")
        .args([
            "--db",
            &dir.path().join("strk20.db").display().to_string(),
            "--feed-dir",
            &dir.path().join("feed").display().to_string(),
            "--rpc-url",
            &format!("http://{rpc_addr}/"),
            "--rpc-fallback",
            &format!("http://{rpc_addr}/"),
            "--pool",
            &felt_hex(&fixture.constants.contract_address),
            "--chain-id",
            CHAIN_ID,
            "--genesis-block",
            &GENESIS_BLOCK.to_string(),
            "--epoch-size",
            &EPOCH_SIZE.to_string(),
            "--chunk-size",
            "5",
        ])
        .args(["--listen", &format!("127.0.0.1:{port}")])
        .args(["--poll-ms", "150"]);
    let indexer = spawn_with_logs(cmd, dir.path(), "indexer");

    let fx = Fx {
        rpc,
        dir,
        port,
        http: reqwest::Client::new(),
        bob,
        bob_key,
        alice,
        alice_key: fixture.constants.alice_viewing_key,
        pool_hex: felt_hex(&fixture.constants.contract_address),
        channel_key,
        strk,
        next_index,
        _indexer: indexer,
    };
    fx.wait_head(46).await;
    fx.wait_feed_settled().await;
    fx
}

/// `status` closes the connect burst (§2.2), so its arrival means the whole
/// burst is in.
async fn burst_complete(s: &SseStream) -> bool {
    s.wait_for(Duration::from_secs(20), |t| {
        sse::parse_events(t)
            .iter()
            .any(|e| e.name.as_deref() == Some("status"))
    })
    .await
}

fn named<'a>(events: &'a [SseEvent], name: &str) -> Option<&'a SseEvent> {
    events.iter().find(|e| e.name.as_deref() == Some(name))
}

fn names(events: &[SseEvent]) -> Vec<String> {
    events
        .iter()
        .map(|e| e.name.clone().unwrap_or_default())
        .collect()
}

fn tail_of_log(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_default()
}

/// Poll `pred` until it holds or `timeout` elapses.
async fn wait_until(timeout: Duration, pred: impl Fn() -> bool) -> bool {
    let deadline = std::time::Instant::now() + timeout;
    loop {
        if pred() {
            return true;
        }
        if std::time::Instant::now() >= deadline {
            return false;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
}

// ------------------------------------------------------------------- E1

/// E1 — the exact §2.2 framing, and a client driven by the stream reaching
/// the state a polling client reaches.
///
/// The convergence half is made non-vacuous by the poll interval: the watcher
/// is given an hour-long cadence, so anything it reports inside the test came
/// from the stream and from nothing else.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e1_sse_framing_and_poked_client_converges() {
    let mut fx = setup().await;

    let stream = SseStream::connect(&fx.live_url())
        .await
        .expect("GET /feed/live must be served (always on, no flag — §2.1)");
    assert_eq!(stream.status, 200, "/feed/live must answer 200");
    assert_eq!(
        stream.header("content-type").map(|c| c.split(';').next().unwrap_or("").trim().to_owned()),
        Some("text/event-stream".to_owned()),
        "§2.4 response headers"
    );
    assert_eq!(stream.header("cache-control"), Some("no-cache"));
    assert_eq!(
        stream.header("x-accel-buffering"),
        Some("no"),
        "§2.4: proxies must not buffer the stream"
    );

    // `status` closes the connect burst, so waiting for it means the whole
    // burst has arrived — without assuming how many events it contains.
    assert!(
        stream
            .wait_for(Duration::from_secs(20), |t| sse::parse_events(t)
                .iter()
                .any(|e| e.name.as_deref() == Some("status")))
            .await,
        "§2.2: on connect the server sends, in order: hello, the current head, the \
         current epoch and snapshot, and status. Got:\n{}",
        stream.text()
    );
    let raw = stream.text();

    // ---- padding + retry, before anything else
    let first_line = raw.lines().next().unwrap_or_default();
    assert!(
        first_line.starts_with(':') && first_line.len() > PADDING_BYTES,
        "§2.2: the stream opens with a {PADDING_BYTES}-byte `:` padding comment that \
         defeats buffering middleboxes; first line was {} bytes",
        first_line.len()
    );
    let line_of = |pred: &dyn Fn(&str) -> bool| raw.lines().position(pred);
    let retry_at = line_of(&|l: &str| {
        l.strip_prefix("retry:").map(str::trim) == Some("15000")
    })
    .expect("§2.2: a `retry: 15000` field is required so EventSource reconnects on its own");
    let hello_at = line_of(&|l: &str| l.strip_prefix("event:").map(str::trim) == Some("hello"))
        .expect("§2.2: a `hello` event is required");
    assert!(retry_at < hello_at, "`retry:` must precede the first event");

    // ---- the connect burst
    let events = stream.events();
    assert_eq!(
        names(&events).first().map(String::as_str),
        Some("hello"),
        "the first event must be hello: {:?}",
        names(&events)
    );
    for e in &events {
        assert!(
            e.id.is_some(),
            "§2.2: every event carries an `id:` (client-side dedup and debuggability): {e:?}"
        );
    }

    let manifest = fx.manifest().await;
    let hello = named(&events, "hello").unwrap().json();
    assert_eq!(hello["v"], Value::from(1));
    assert_eq!(
        hello["chain_id"].as_str(),
        Some(CHAIN_ID),
        "§2.2: hello carries chain identity so a proxy pointed at the wrong network \
         dies before any refetch or state mutation: {hello}"
    );
    assert_eq!(hello["pool"].as_str(), Some(fx.pool_hex.as_str()), "{hello}");
    assert!(
        hello["module"].as_str().unwrap_or_default().starts_with("strk20/"),
        "hello.module names the build: {hello}"
    );

    let head = named(&events, "head")
        .unwrap_or_else(|| panic!("§2.2: a current `head` event is required: {:?}", names(&events)))
        .json();
    assert_eq!(head["head"].as_u64(), Some(46), "{head}");
    assert_eq!(head["l1_accepted"].as_u64(), Some(40), "{head}");
    assert!(head["head_hash"].as_str().is_some(), "{head}");
    assert!(head["tail_from"].as_u64().is_some(), "{head}");
    assert_eq!(
        head["etag"].as_str().map(str::to_owned),
        Some(fx.head_etag().await),
        "§2.2: the head event's etag lets a client skip a conditional GET it has \
         already applied, so it must be the ETag the file is served with: {head}"
    );

    let epoch = named(&events, "epoch")
        .unwrap_or_else(|| panic!("§2.2: a current `epoch` event is required: {:?}", names(&events)))
        .json();
    assert!(
        epoch.get("epoch").is_none(),
        "review finding 14d: the epoch index key is \"e\" on BOTH events that name an \
         epoch, because the manifest — the identity source the client cross-references \
         — uses \"e\": {epoch}"
    );
    assert_eq!(epoch["e"].as_u64(), manifest["latest_epoch"].as_u64(), "{epoch}");
    let entry = manifest["epochs"]
        .as_array()
        .unwrap()
        .iter()
        .find(|e| e["e"] == epoch["e"])
        .expect("manifest lists the announced epoch");
    for k in ["from", "to", "hash", "zst", "bytes"] {
        assert_eq!(epoch[k], entry[k], "epoch event field {k} must match the manifest");
    }

    let status = named(&events, "status")
        .unwrap_or_else(|| panic!("§2.2: a `status` event is required: {:?}", names(&events)))
        .json();
    assert_eq!(status["decode_state"].as_str(), Some("ok"), "{status}");
    assert_eq!(status["verify_root_failed"], Value::Bool(false), "{status}");

    let snapshot = named(&events, "snapshot").map(|e| e.json());
    if !manifest["snapshot"].is_null() {
        let snapshot = snapshot.unwrap_or_else(|| {
            panic!(
                "§2.2: the manifest carries a snapshot, so connect must announce it: {:?}",
                names(&events)
            )
        });
        assert_eq!(snapshot["e"], manifest["snapshot"]["e"], "{snapshot}");
        assert_eq!(snapshot["block"], manifest["snapshot"]["block"], "{snapshot}");
        assert_eq!(snapshot["hash"], manifest["snapshot"]["hash"], "{snapshot}");
    } else {
        panic!(
            "this fixture publishes a snapshot (§A1 + §11.3 gate), so the connect burst \
             must include a `snapshot` event; manifest.snapshot was null"
        );
    }

    // ---- a head change pokes, and the announcement is fetchable
    let note = fx.mint_at(47, 4242);
    assert!(
        stream
            .wait_for(Duration::from_secs(30), |t| {
                sse::parse_events(t)
                    .iter()
                    .any(|e| e.name.as_deref() == Some("head") && e.json()["head"] == 47)
            })
            .await,
        "§2.2: `head` fires on any change of head.ndjson's bytes. Stream:\n{}",
        stream.text()
    );
    let poked = stream
        .events()
        .into_iter()
        .rfind(|e| e.name.as_deref() == Some("head"))
        .unwrap()
        .json();
    assert_eq!(
        poked["etag"].as_str().map(str::to_owned),
        Some(fx.head_etag().await),
        "§2.4: the emitter watches the PUBLISHED files, so it can only announce what \
         is already renamed into place and fetchable: {poked}"
    );

    // ---- keepalive
    assert!(
        stream
            .wait_for(Duration::from_secs(25), |t| t
                .lines()
                .any(|l| l.starts_with(':') && l.trim_start_matches(':').trim() == "ka"))
            .await,
        "§2.2: a `: ka` keepalive comment every 15 s of silence. Stream:\n{}",
        stream.text()
    );

    // ---- convergence: a stream-driven client vs a polling client
    //
    // ORDERING IS THE WHOLE TEST. `--watch` runs a full `sync_once` and prints
    // the entire report to stdout BEFORE it enters the poke/tick loop, so a
    // note that pre-dates the process is reported by that initial pass and
    // proves nothing about the stream — with the note minted first, removing
    // SSE from the client entirely would not turn this red. The note must
    // therefore be minted only AFTER the initial report has landed.
    let feed = format!("{}/feed", fx.base());
    let watcher = fx.spawn_watcher("live-watcher", &feed, &fx.bob, "0xb0b", 3600);
    let mut initial = false;
    for _ in 0..300 {
        // the initial report is the pretty-printed SyncReport, not a
        // {"event":"note"} line
        if tail_of_log(&watcher.stdout_path).contains("\"incoming_complete\"") {
            initial = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(
        initial,
        "the watcher never printed its initial report, so the ordering this leg depends \
         on cannot be established.\nstdout:\n{}\nstderr:\n{}",
        tail_of_log(&watcher.stdout_path),
        tail_of_log(&watcher.stderr_path)
    );
    let baseline = tail_of_log(&watcher.stdout_path);

    // Now mint. From here the ONLY way this note reaches the watcher inside the
    // test is a poke: its next tick is 3600 s away.
    let live_note = fx.mint_at(48, 777);
    let live_id = felt_hex(&live_note.note_id);
    assert!(
        !baseline.contains(&live_id),
        "the note must not exist at the time of the initial report, or the assertion \
         below is about that report and not about the stream"
    );
    let mut seen = false;
    for _ in 0..300 {
        if tail_of_log(&watcher.stdout_path).contains(&live_id) {
            seen = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(
        seen,
        "§2.5: a client subscribed to /feed/live must learn of a note minted AFTER its \
         initial sync without waiting for its poll cadence — the interval here is \
         3600 s, so only the stream can have delivered it. Watcher stdout:\n{}\n\
         stderr:\n{}",
        tail_of_log(&watcher.stdout_path),
        tail_of_log(&watcher.stderr_path)
    );

    // the polling reference reaches the same note
    let key_path = fx.dir.path().join("poller.key");
    std::fs::write(&key_path, "0xb0b").unwrap();
    let mut cmd = Command::new(bin("strk20-sync"));
    cmd.arg("sync")
        .args(["--feed", &feed])
        .args(["--address", &felt_hex(&fx.bob)])
        .args(["--key-file", &key_path.display().to_string()])
        .args(["--db", &fx.dir.path().join("poller.db").display().to_string()])
        .arg("--json");
    let (stdout, stderr, ok) = e2e_tests::bins::run_capture(cmd, false);
    assert!(ok, "polling client failed: {stderr}");
    let report: Value = serde_json::from_str(&stdout).expect("report json");
    let ids: Vec<&str> = report["notes"]
        .as_array()
        .unwrap()
        .iter()
        .filter_map(|n| n["note_id"].as_str())
        .collect();
    for want in [felt_hex(&note.note_id), live_id.clone()] {
        assert!(
            ids.contains(&want.as_str()),
            "the polling client must reach the same state the stream-driven one reached; \
             {want} is missing from {ids:?}"
        );
    }
}

// ------------------------------------------------------------------- E2

/// E2 — one global stream, never per-user (base R3). Two subscribers receive
/// the same events, the subscription cannot carry a parameter, and two real
/// clients with different keys and addresses emit byte-identical requests.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2_stream_is_identical_for_every_subscriber() {
    let mut fx = setup().await;

    // ---- any query string is refused, not ignored (§2.1)
    for q in ["?", "?x=1", "?address=0xb0b", "?last_event_id=3"] {
        let url = format!("{}{q}", fx.live_url());
        let resp = fx.http.get(&url).send().await.expect("request");
        assert_eq!(
            resp.status().as_u16(),
            400,
            "§2.1: any query string on /feed/live is 400 INVALID_QUERY — stronger than \
             ignoring it, because the address-blindness leg then has a SERVER-enforced \
             guarantee. {url} answered {}",
            resp.status()
        );
        let body = resp.text().await.unwrap_or_default();
        assert!(
            body.contains("INVALID_QUERY"),
            "the refusal must name INVALID_QUERY: {body}"
        );
    }

    // ---- two subscribers, same bytes
    let a = SseStream::connect(&fx.live_url()).await.expect("subscriber A");
    let b = SseStream::connect(&fx.live_url()).await.expect("subscriber B");
    assert!(burst_complete(&a).await, "A: {}", a.text());
    assert!(burst_complete(&b).await, "B: {}", b.text());
    assert_same_stream(&a, &b, "the connect burst");

    fx.mint_at(47, 5151);
    for (label, s) in [("A", &a), ("B", &b)] {
        assert!(
            s.wait_for(Duration::from_secs(30), |t| sse::parse_events(t)
                .iter()
                .any(|e| e.name.as_deref() == Some("head") && e.json()["head"] == 47))
                .await,
            "subscriber {label} never saw the poke:\n{}",
            s.text()
        );
    }
    assert_same_stream(&a, &b, "after a poke");

    // ---- §2.3 resume is the empty program: `Last-Event-ID` is ignored
    //
    // Note this is a different mechanism from the query-string refusal above:
    // §2.1 rejects a `?last_event_id=` PARAMETER outright, while §2.3 is about
    // the HEADER an EventSource sends on its own, which the server must accept
    // and then take no notice of. There is no replay buffer to resume from —
    // every event carries full current state — and "no per-client cursor" is
    // itself the privacy property: at the protocol layer the server cannot be
    // made to remember a client because the protocol gives it nothing to
    // remember. A server that grew a journal would pass §2.1 and fail here.
    let resumed = SseStream::connect_with(
        &fx.live_url(),
        &[("last-event-id", "3"), ("Last-Event-ID", "9999")],
    )
    .await
    .expect("a stale Last-Event-ID must be accepted, not refused");
    assert_eq!(resumed.status, 200, "the header is ignored, never an error");
    assert!(burst_complete(&resumed).await, "resumed: {}", resumed.text());
    let resumed_events = resumed.events();
    assert_eq!(
        names(&resumed_events).first().map(String::as_str),
        Some("hello"),
        "a reconnect carrying a stale id receives CURRENT STATE from the top, starting \
         with hello: {:?}",
        names(&resumed_events)
    );
    assert_eq!(
        resumed_events.first().and_then(|e| e.id.clone()),
        Some("1".to_owned()),
        "ids are per-connection and start at 1 — a server that resumed from the \
         supplied id would number differently: {:?}",
        resumed_events.iter().map(|e| e.id.clone()).collect::<Vec<_>>()
    );
    // ...and a late joiner gets the same current-state burst the two
    // already-connected subscribers got, including the head they were poked
    // with. This is also the leg that proves connect replays state rather than
    // only forwarding future deltas.
    assert_eq!(
        named(&resumed_events, "head").map(|e| e.json()["head"].clone()),
        Some(Value::from(47)),
        "the late joiner must be handed the CURRENT head, not the head that was \
         current when the other subscribers connected: {:?}",
        names(&resumed_events)
    );

    // ---- the scanner over the whole response capture
    let secrets: Vec<(Felt, String)> = vec![
        (fx.bob_key, "bob-key".into()),
        (fx.alice_key, "alice-key".into()),
        (fx.bob, "bob-address".into()),
        (fx.alice, "alice-address".into()),
    ];
    let hits = scanner::scan(a.bytes().as_slice(), &secrets);
    assert!(hits.is_empty(), "key/address material inside the SSE stream: {hits:?}");

    // ---- two real clients, byte-identical subscriptions
    let proxy = TcpProxy::new(format!("127.0.0.1:{}", fx.port).parse().unwrap());
    let proxy_addr = proxy.serve().await;
    let feed = format!("http://{proxy_addr}/feed");
    let w1 = fx.spawn_watcher("watch-bob", &feed, &fx.bob, "0xb0b", 3600);
    let w2 = fx.spawn_watcher("watch-alice", &feed, &fx.alice, "0xa11ce", 3600);
    for _ in 0..150 {
        if proxy.live_opens() >= 2 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(
        proxy.live_opens() >= 2,
        "both --watch clients must subscribe to /feed/live (§2.5 client behaviour); \
         opens = {}\nbob:\n{}\nalice:\n{}",
        proxy.live_opens(),
        tail_of_log(&w1.stderr_path),
        tail_of_log(&w2.stderr_path)
    );

    let heads = proxy.heads();
    let mut haystack = Vec::new();
    for h in &heads {
        assert_eq!(h.method, "GET", "keyless clients only GET: {}", h.uri);
        assert!(
            feed_urls::is_allowed(&h.uri),
            "{} is outside the closed whole-path allowlist {:?}",
            h.uri,
            feed_urls::PATTERNS
        );
        haystack.extend_from_slice(&h.bytes);
    }
    let hits = scanner::scan(&haystack, &secrets);
    assert!(hits.is_empty(), "key/address material in a request head: {hits:?}");

    let live: Vec<Vec<u8>> = heads
        .iter()
        .filter(|h| h.uri == "/feed/live")
        .map(|h| h.bytes.clone())
        .collect();
    assert!(live.len() >= 2, "expected a subscription from each client");
    // EVERY captured subscription head, not just the first two: two heads
    // could both belong to one client reconnecting, which would satisfy a
    // pairwise check trivially.
    for (i, bytes) in live.iter().enumerate() {
        assert_eq!(
            String::from_utf8_lossy(bytes),
            String::from_utf8_lossy(&live[0]),
            "§2.6: the subscription request is parameterless and carries nothing derived \
             from a user, so two clients with different keys and addresses must emit \
             BYTE-IDENTICAL request heads. Head {i} of {} differs.",
            live.len()
        );
    }
}

/// §2.6: the emitted bytes are identical for every subscriber, modulo connect
/// ordering and `id` numbering. Two live subscribers can legitimately be a
/// few bytes apart in flight, so the requirement is that the shorter run is
/// EXACTLY a prefix of the longer: any per-client difference at all — a
/// different payload, a reordering, an extra event for one of them — breaks
/// it, while a mid-flight event does not.
fn assert_same_stream(a: &SseStream, b: &SseStream, phase: &str) {
    let pairs = |s: &SseStream| -> Vec<(String, String)> {
        s.events()
            .into_iter()
            .map(|e| (e.name.unwrap_or_default(), e.data))
            .collect()
    };
    let (pa, pb) = (pairs(a), pairs(b));
    let n = pa.len().min(pb.len());
    assert!(
        n >= 4,
        "{phase}: too few events to compare ({} vs {})",
        pa.len(),
        pb.len()
    );
    assert_eq!(
        pa[..n],
        pb[..n],
        "{phase}: one global stream, never per-user (base R3) — every event is \
         state-carrying and idempotent, so there is nothing per-client to differ.\n\
         A:\n{}\nB:\n{}",
        a.text(),
        b.text()
    );
}

// ------------------------------------------------------------------- E3

/// E3 — degrade and restore (§2.5). The stream is killed mid-flight and then
/// the route disappears entirely, which is what a plain static-file mirror
/// looks like. The client must degrade to polling with no error surfaced and
/// still converge on the same bytes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e3_sse_disconnect_falls_back_to_polling_and_converges() {
    let mut fx = setup().await;
    let proxy = TcpProxy::new(format!("127.0.0.1:{}", fx.port).parse().unwrap());
    let proxy_addr = proxy.serve().await;
    let feed = format!("http://{proxy_addr}/feed");

    let watcher = fx.spawn_watcher("degrading-watcher", &feed, &fx.bob, "0xb0b", 2);
    for _ in 0..150 {
        if proxy.live_opens() >= 1 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(
        proxy.live_opens() >= 1,
        "§2.5 client behaviour: `strk20-sync sync --watch` subscribes to /feed/live and \
         falls back to polling when it cannot. Nothing ever opened the stream, so the \
         fallback below would be vacuous.\nstdout:\n{}\nstderr:\n{}",
        tail_of_log(&watcher.stdout_path),
        tail_of_log(&watcher.stderr_path)
    );

    // ---- (1) a stream killed mid-flight is TRANSIENT: reconnect, don't quit
    let before_kill = proxy.live_opens();
    proxy.set_live_policy(LivePolicy::Kill);
    let reconnected = wait_until(Duration::from_secs(40), || {
        proxy.live_opens() > before_kill
    })
    .await;
    assert!(
        reconnected,
        "§2.5: a dropped connection is a transient failure — the client must reconnect \
         with backoff rather than give up. Opens stayed at {before_kill}.\nstderr:\n{}",
        tail_of_log(&watcher.stderr_path)
    );

    // ---- (2) RESTORE: when the route works again the client returns to live
    proxy.set_live_policy(LivePolicy::Pass);
    let at_restore = proxy.live_opens();
    let restored = wait_until(Duration::from_secs(40), || {
        proxy.live_opens() > at_restore
    })
    .await;
    assert!(
        restored,
        "the client must re-subscribe once /feed/live answers again; opens stuck at \
         {at_restore}\nstderr:\n{}",
        tail_of_log(&watcher.stderr_path)
    );
    let after_restore = proxy.live_opens();
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(
        proxy.live_opens(),
        after_restore,
        "a RESTORED stream stays up: further opens in a quiet window mean the client is \
         still churning rather than back on live.\nstderr:\n{}",
        tail_of_log(&watcher.stderr_path)
    );

    // ---- (3) 404 is a DEPLOYMENT FACT, not a failure: degrade permanently
    proxy.set_live_policy(LivePolicy::NotFound);
    let degraded = wait_until(Duration::from_secs(60), || {
        tail_of_log(&watcher.stderr_path).contains("polling only")
    })
    .await;
    assert!(
        degraded,
        "§2.5: 404/405 permanently degrades the session to polling, and the degrade is \
         SIGNALLED rather than inferred from silence.\nstderr:\n{}",
        tail_of_log(&watcher.stderr_path)
    );
    let after_404 = proxy.live_opens();
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(
        proxy.live_opens(),
        after_404,
        "\"permanently\" means the client stops trying: a 404 that produced a reconnect \
         storm would be a retry loop against a plain static-file mirror, which is a fully \
         supported deployment"
    );

    let before = proxy
        .heads()
        .iter()
        .filter(|h| h.uri == "/feed/head.ndjson")
        .count();
    let note = fx.mint_at(47, 6363);
    fx.wait_head(47).await;

    let note_id = felt_hex(&note.note_id);
    let mut seen = false;
    for _ in 0..200 {
        if tail_of_log(&watcher.stdout_path).contains(&note_id) {
            seen = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }
    assert!(
        seen,
        "§2.5: 404/405 on /feed/live permanently degrades the session to polling with \
         no error surfaced — a plain static-file mirror has no stream and is a fully \
         supported deployment. The watcher must still converge on the new note.\n\
         stdout:\n{}\nstderr:\n{}",
        tail_of_log(&watcher.stdout_path),
        tail_of_log(&watcher.stderr_path)
    );

    let after = proxy
        .heads()
        .iter()
        .filter(|h| h.uri == "/feed/head.ndjson")
        .count();
    assert!(
        after > before,
        "the convergence must have come from the polling fallback: head.ndjson was \
         polled {before} times before the stream died and {after} after"
    );
    assert!(
        !tail_of_log(&watcher.stderr_path).contains("ERROR"),
        "a missing stream is a supported deployment, not an error:\n{}",
        tail_of_log(&watcher.stderr_path)
    );
}
