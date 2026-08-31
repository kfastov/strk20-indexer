//! `strk20-sync` — the keyless client binary (spec §8). Key input is
//! file/stdin only (never argv: process lists leak); the key buffer is
//! zeroized after parsing into `SecretFelt`.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use discovery_core::privacy_pool::types::SecretFelt;
use starknet_types_core::felt::Felt;
use std::io::Read;
use std::path::PathBuf;
use strk20_client::events::{events_for, LiveUnsupported};
use strk20_client::store::{ColdStart, FeedStore};
use strk20_client::sync::{sync_once, SyncOptions};
use strk20_client::transport::transport_for;
use zeroize::Zeroize;

/// §2.5 client behaviour for `/feed/live`: reconnect with jittered exponential
/// backoff 1 s -> 60 s, a 45 s watchdog on silence, and — the part that is a
/// deployment decision rather than an error — 404/405 permanently degrades this
/// session to polling with NOTHING surfaced. A plain static-file mirror has no
/// stream and is fully supported.
fn spawn_live_subscription(feed: &str) -> tokio::sync::mpsc::Receiver<()> {
    let (tx, rx) = tokio::sync::mpsc::channel::<()>(8);
    let Some(events) = events_for(feed) else {
        return rx;
    };
    tokio::spawn(async move {
        use futures::StreamExt;
        let mut backoff = 1u64;
        loop {
            match events.subscribe().await {
                Ok(mut stream) => {
                    backoff = 1;
                    tracing::info!("subscribed to the feed's live stream");
                    loop {
                        let next = tokio::time::timeout(
                            std::time::Duration::from_secs(45),
                            stream.next(),
                        )
                        .await;
                        match next {
                            Ok(Some(notice)) => {
                                if notice.is_poke() {
                                    // A full channel already means "go look",
                                    // so dropping the extra poke loses nothing.
                                    let _ = tx.try_send(());
                                }
                            }
                            // stream ended, or 45 s of total silence: either
                            // way the connection is no longer carrying pokes
                            Ok(None) | Err(_) => break,
                        }
                    }
                    tracing::debug!("live stream ended; reconnecting");
                }
                Err(e) if e.downcast_ref::<LiveUnsupported>().is_some() => {
                    tracing::info!("feed publishes no live stream; polling only");
                    return;
                }
                Err(e) => tracing::debug!(error = %e, "live stream unavailable; retrying"),
            }
            if tx.is_closed() {
                return;
            }
            // Fresh jitter per reconnect. It was derived from the process id,
            // which is CONSTANT for the process: every reconnect of a given
            // client landed at the same sub-second offset — ~9 bits of stable,
            // server-observable identity that survives reconnects, IP changes
            // and OHTTP, which is precisely the linkability §2.6's residual
            // paragraph assumed nothing would introduce.
            let jitter = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.subsec_nanos() as u64)
                .unwrap_or(0)
                % 500
                + 1;
            tokio::time::sleep(std::time::Duration::from_millis(
                backoff * 1000 + jitter,
            ))
            .await;
            backoff = (backoff * 2).min(60);
        }
    });
    rx
}

#[derive(Parser)]
#[command(name = "strk20-sync", version, about = "STRK20 keyless discovery client")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

/// The chain the client believes it is on. Given, a feed stamped with any
/// other chain id is refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq, clap::ValueEnum)]
enum Network {
    Mainnet,
    Sepolia,
}

impl Network {
    fn chain_id(self) -> &'static str {
        match self {
            Network::Mainnet => strk20_feed::CHAIN_ID_MAINNET,
            Network::Sepolia => strk20_feed::CHAIN_ID_SEPOLIA,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, clap::ValueEnum)]
enum ColdStartArg {
    #[default]
    Auto,
    Snapshot,
    Epochs,
}

impl From<ColdStartArg> for ColdStart {
    fn from(a: ColdStartArg) -> Self {
        match a {
            ColdStartArg::Auto => ColdStart::Auto,
            ColdStartArg::Snapshot => ColdStart::Snapshot,
            ColdStartArg::Epochs => ColdStart::Epochs,
        }
    }
}

#[derive(Subcommand)]
enum Command {
    /// Discover notes for an address; the viewing key never leaves this process
    Sync {
        /// Feed source: http(s) URL of a /feed endpoint, or a local mirror dir
        #[arg(long, env = "STRK20_FEED")]
        feed: String,
        /// The wallet's public address
        #[arg(long)]
        address: String,
        /// Path to a file containing the hex viewing key, or "-" for stdin
        #[arg(long)]
        key_file: String,
        /// Local mirror database
        #[arg(long, default_value = "sync.db")]
        db: PathBuf,
        /// Emit the full report as JSON on stdout
        #[arg(long)]
        json: bool,
        /// Keep polling the feed head and report new notes/spends
        #[arg(long)]
        watch: bool,
        /// Poll interval for --watch, seconds
        #[arg(long, default_value_t = 30)]
        interval: u64,
        /// Drop cursors and the notes registry for this address and
        /// rediscover from scratch (mirror data is kept)
        #[arg(long)]
        full_resync: bool,
        /// Refuse the feed unless its chain id is this network's
        #[arg(long, value_enum)]
        network: Option<Network>,
        /// How to populate an EMPTY mirror: `auto` takes the published
        /// snapshot and falls back to full replay if it cannot be grounded,
        /// `snapshot` requires it, `epochs` replays every epoch from genesis
        /// (the only way to get complete transaction history)
        #[arg(long, value_enum, default_value_t = ColdStartArg::Auto)]
        cold_start: ColdStartArg,
        /// Ground the feed's anchor in the chain through YOUR OWN RPC
        /// (§1.5 ring 6). Configured means mandatory: if it cannot pass, the
        /// sync fails rather than reporting a grade it did not earn.
        #[arg(long)]
        verify_anchor: Option<String>,
    },
    /// Check the local mirror against the feed's published chain anchors
    VerifyAnchors {
        #[arg(long, env = "STRK20_FEED")]
        feed: String,
        #[arg(long, default_value = "sync.db")]
        db: PathBuf,
        #[arg(long)]
        json: bool,
    },
    /// Verify discovered notes against Starknet state roots via your OWN RPC
    Verify {
        #[arg(long)]
        rpc: String,
        #[arg(long)]
        address: String,
        #[arg(long, default_value = "sync.db")]
        db: PathBuf,
    },
}

fn read_key(key_file: &str) -> Result<SecretFelt> {
    // Pre-reserved buffer: no reallocation may strand key fragments in freed
    // heap memory (review finding); a hex felt is at most 66 chars.
    let mut raw = String::with_capacity(256);
    if key_file == "-" {
        std::io::stdin()
            .read_to_string(&mut raw)
            .context("read key from stdin")?;
    } else {
        let mut bytes = std::fs::read(key_file)
            .with_context(|| format!("read key file {key_file}"))?;
        raw.push_str(std::str::from_utf8(&bytes).context("key file is not utf-8")?);
        bytes.zeroize();
    }
    if raw.len() > 200 {
        raw.zeroize();
        anyhow::bail!("key input implausibly large");
    }
    let trimmed = raw.trim();
    let felt = Felt::from_hex(trimmed).map_err(|_| anyhow::anyhow!("key is not valid hex"));
    raw.zeroize();
    Ok(SecretFelt::new(felt?))
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
        .init();
    let cli = Cli::parse();
    match cli.command {
        Command::Sync {
            feed,
            address,
            key_file,
            db,
            json,
            watch,
            interval,
            full_resync,
            network,
            cold_start,
            verify_anchor,
        } => {
            let owner =
                Felt::from_hex(&address).map_err(|_| anyhow::anyhow!("bad --address"))?;
            let key = read_key(&key_file)?;
            let store = FeedStore::open(&db)?;
            if full_resync {
                strk20_client::sync::full_resync(&store, &owner)?;
            }
            let transport = transport_for(&feed);
            if let Some(net) = network {
                strk20_client::sync::check_chain_id(transport.as_ref(), net.chain_id()).await?;
            }
            let opts = SyncOptions {
                cold_start: cold_start.into(),
                verify_anchor_rpc: verify_anchor,
            };
            let report = sync_once(&store, transport.as_ref(), owner, &key, &opts).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                print_human(&report);
            }
            let complete = report.incoming_complete && report.outgoing_complete;
            if watch {
                // Advancing baseline + emitted-set: each note is reported
                // once; a transient transport error is logged, not fatal
                // (review finding: --watch re-emitted forever and died on
                // any hiccup).
                let mut emitted: std::collections::HashSet<String> =
                    report.notes.iter().map(|n| n.note_id.clone()).collect();
                // §2.5: subscribe when the feed offers a stream, poll always.
                // Both paths converge on identical bytes, so a poke only ever
                // moves work earlier.
                let mut pokes = spawn_live_subscription(&feed);
                loop {
                    let tick =
                        tokio::time::sleep(std::time::Duration::from_secs(interval));
                    tokio::select! {
                        _ = tick => {}
                        Some(()) = pokes.recv() => {}
                    }
                    let r = match sync_once(&store, transport.as_ref(), owner, &key, &opts).await {
                        Ok(r) => r,
                        Err(e) => {
                            tracing::warn!(error = %e, "watch sync failed; retrying");
                            continue;
                        }
                    };
                    let fresh: Vec<_> = r
                        .notes
                        .iter()
                        .filter(|n| !emitted.contains(&n.note_id))
                        .collect();
                    for n in fresh {
                        println!("{}", serde_json::json!({"event": "note", "note": n}));
                        emitted.insert(n.note_id.clone());
                    }
                    for nf in &r.newly_spent {
                        println!(
                            "{}",
                            serde_json::json!({"event": "spent", "nullifier": nf})
                        );
                    }
                }
            }
            if !complete {
                bail!("discovery incomplete");
            }
            Ok(())
        }
        Command::VerifyAnchors { feed, db, json } => {
            let store = FeedStore::open(&db)?;
            let transport = transport_for(&feed);
            let report =
                strk20_client::anchors::verify_anchors(&store, transport.as_ref()).await?;
            if json {
                println!("{}", serde_json::to_string_pretty(&report)?);
            } else {
                println!(
                    "anchors: {} published, {} checked, status={}",
                    report["anchors_published"],
                    report["anchors_checked"],
                    report["status"]
                );
                for p in report["problems"].as_array().into_iter().flatten() {
                    println!("  {}", p.as_str().unwrap_or_default());
                }
            }
            // A run that checked nothing has verified nothing: it must be
            // distinguishable from a successful verification at the exit
            // status, not just in the report text.
            if report["all_ok"] != serde_json::Value::Bool(true) {
                let problems = report["problems"].as_array().map(Vec::len).unwrap_or(0);
                if problems > 0 {
                    bail!("anchor verification failed: {problems} anchor mismatch(es)");
                }
                bail!(
                    "anchor verification did not verify anything ({}): \
                     no anchor could be checked against this mirror",
                    report["status"].as_str().unwrap_or("unknown")
                );
            }
            Ok(())
        }
        Command::Verify { rpc, address, db } => {
            let owner =
                Felt::from_hex(&address).map_err(|_| anyhow::anyhow!("bad --address"))?;
            let store = FeedStore::open(&db)?;
            let report = strk20_client::verify::verify_owner(&store, &rpc, &owner).await?;
            println!("{}", serde_json::to_string_pretty(&report)?);
            if report["all_ok"] != serde_json::Value::Bool(true) {
                bail!("verification found mismatches");
            }
            Ok(())
        }
    }
}

fn print_human(r: &strk20_client::sync::SyncReport) {
    eprintln!(
        "synced {} @ head {} (l1_accepted {}, epoch floor {})",
        r.address, r.head, r.l1_accepted, r.last_epoch_to
    );
    if r.tail_rewound {
        eprintln!("  tail reorg detected: rewound to L1-final checkpoint");
    }
    // §1.1/§1.5.1: the grade and the history floor are SURFACED, never implied.
    match r.snapshot_basis {
        Some(b) => eprintln!(
            "  integrity: {} (snapshot basis {b}; transaction history starts at block {})",
            r.verified, r.history_from
        ),
        None => eprintln!("  integrity: {} (full history)", r.verified),
    }
    if r.snapshot_rejected {
        eprintln!("  the published snapshot was refused; this mirror was replayed instead");
    }
    eprintln!(
        "  incoming: {} sender(s), complete={}",
        r.incoming_senders.len(),
        r.incoming_complete
    );
    eprintln!(
        "  outgoing: {} recipient(s), complete={}",
        r.outgoing_recipients.len(),
        r.outgoing_complete
    );
    for n in &r.notes {
        eprintln!(
            "  note token={} index={} amount={} block={} spent={}",
            n.token, n.index, n.amount, n.block_number, n.spent
        );
    }
    for (token, bal) in &r.balances {
        eprintln!("  balance {token} = {bal}");
    }
}
