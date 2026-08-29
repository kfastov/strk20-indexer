//! `strk20-sync` — the keyless client binary (spec §8). Key input is
//! file/stdin only (never argv: process lists leak); the key buffer is
//! zeroized after parsing into `SecretFelt`.

use anyhow::{bail, Context, Result};
use clap::{Parser, Subcommand};
use discovery_core::privacy_pool::types::SecretFelt;
use starknet_types_core::felt::Felt;
use std::io::Read;
use std::path::PathBuf;
use strk20_client::store::FeedStore;
use strk20_client::sync::sync_once;
use strk20_client::transport::transport_for;
use zeroize::Zeroize;

#[derive(Parser)]
#[command(name = "strk20-sync", version, about = "STRK20 keyless discovery client")]
struct Cli {
    #[command(subcommand)]
    command: Command,
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
        } => {
            let owner =
                Felt::from_hex(&address).map_err(|_| anyhow::anyhow!("bad --address"))?;
            let key = read_key(&key_file)?;
            let store = FeedStore::open(&db)?;
            if full_resync {
                strk20_client::sync::full_resync(&store, &owner)?;
            }
            let transport = transport_for(&feed);
            let report = sync_once(&store, transport.as_ref(), owner, &key).await?;
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
                loop {
                    tokio::time::sleep(std::time::Duration::from_secs(interval)).await;
                    let r = match sync_once(&store, transport.as_ref(), owner, &key).await {
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
