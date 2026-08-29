//! `strk20` — the server binary (spec §8).

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use starknet_types_core::felt::Felt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use strk20_indexerd::config::{self, ChainConfig};
use strk20_indexerd::cutter::Cutter;
use strk20_indexerd::db::Db;
use strk20_indexerd::ingest::{init_checks, Ingestor};
use strk20_indexerd::rpc::RpcClient;

#[derive(Parser)]
#[command(name = "strk20", version, about = "STRK20 open note indexer")]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(clap::Args, Clone)]
struct CommonOpts {
    /// SQLite database path
    #[arg(long, default_value = "strk20.db", env = "STRK20_DB")]
    db: PathBuf,
    /// Feed output directory (the canonical product)
    #[arg(long, default_value = "feed", env = "STRK20_FEED_DIR")]
    feed_dir: PathBuf,
    /// Primary Starknet JSON-RPC URL
    #[arg(long, default_value = config::MAINNET_RPC_PRIMARY, env = "STRK20_RPC_URL")]
    rpc_url: String,
    /// Fallback RPC URL
    #[arg(long, default_value = config::MAINNET_RPC_FALLBACK, env = "STRK20_RPC_FALLBACK")]
    rpc_fallback: String,
    /// Pool contract address (defaults to the mainnet STRK20 pool)
    #[arg(long, env = "STRK20_POOL")]
    pool: Option<String>,
    /// Chain id (SN_MAIN unless overridden for tests)
    #[arg(long, env = "STRK20_CHAIN_ID")]
    chain_id: Option<String>,
    /// Pool deployment block
    #[arg(long, env = "STRK20_GENESIS_BLOCK")]
    genesis_block: Option<u64>,
    /// Epoch size in blocks (test configs only; mainnet value is frozen)
    #[arg(long, env = "STRK20_EPOCH_SIZE")]
    epoch_size: Option<u64>,
    /// getEvents page size
    #[arg(long, default_value_t = 1000)]
    chunk_size: u64,
    /// Additional known pool class hash(es) for the decoder map (recovery
    /// path after an upgrade; spec §5.7)
    #[arg(long = "allow-class")]
    allow_class: Vec<String>,
}

impl CommonOpts {
    fn chain_config(&self) -> ChainConfig {
        let mut cfg = ChainConfig::mainnet();
        if let Some(p) = &self.pool {
            cfg.pool = Felt::from_hex(p).expect("bad --pool");
        }
        if let Some(c) = &self.chain_id {
            cfg.chain_id = c.clone();
        }
        if let Some(g) = self.genesis_block {
            cfg.genesis_block = g;
        }
        if let Some(e) = self.epoch_size {
            cfg.epoch_size = e;
        }
        for c in &self.allow_class {
            cfg.decoder_map.insert(
                Felt::from_hex(c).expect("bad --allow-class"),
                "custom".to_owned(),
            );
        }
        cfg
    }

    fn rpc(&self) -> RpcClient {
        RpcClient::new(self.rpc_url.clone(), Some(self.rpc_fallback.clone()))
    }
}

#[derive(Subcommand)]
enum Command {
    /// Ingest continuously and serve the HTTP API
    Run {
        #[command(flatten)]
        common: CommonOpts,
        #[arg(long, default_value = "127.0.0.1:8080", env = "STRK20_LISTEN")]
        listen: String,
        /// Enable targeted raw endpoints (leaks queried slots — labeled)
        #[arg(long)]
        enable_raw: bool,
        /// Enable the reference-compatible keyed API (receives viewing keys!)
        #[arg(long)]
        enable_compat: bool,
        /// Poll interval in milliseconds
        #[arg(long, default_value_t = 2000)]
        poll_ms: u64,
    },
    /// Ingest to l1_accepted, cut epochs, exit
    Backfill {
        #[command(flatten)]
        common: CommonOpts,
    },
    /// Print health, cursor and epoch inventory
    Status {
        #[command(flatten)]
        common: CommonOpts,
    },
    /// Verify epoch files against the DB chain (hash chain + content hashes)
    EpochVerify {
        #[command(flatten)]
        common: CommonOpts,
        #[arg(long)]
        epoch: Option<u64>,
    },
    /// Recompute the pool storage MPT root and compare with the chain
    VerifyRoot {
        #[command(flatten)]
        common: CommonOpts,
        #[arg(long)]
        block: Option<u64>,
    },
    /// Import a feed directory from another instance (verified)
    MirrorPull {
        #[command(flatten)]
        common: CommonOpts,
        /// Source feed base URL (e.g. https://host/feed)
        url: String,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .init();
    let cli = Cli::parse();
    match cli.command {
        Command::Run {
            common,
            listen,
            enable_raw,
            enable_compat,
            poll_ms,
        } => run(common, listen, enable_raw, enable_compat, poll_ms).await,
        Command::Backfill { common } => backfill(common).await,
        Command::Status { common } => status(common),
        Command::EpochVerify { common, epoch } => epoch_verify(common, epoch),
        Command::VerifyRoot { common, block } => verify_root(common, block).await,
        Command::MirrorPull { common, url } => mirror_pull(common, url).await,
    }
}

async fn run(
    common: CommonOpts,
    listen: String,
    enable_raw: bool,
    enable_compat: bool,
    poll_ms: u64,
) -> Result<()> {
    let cfg = common.chain_config();
    let rpc = common.rpc();
    let mut db = Db::open(&common.db)?;
    init_checks(&db, &rpc, &cfg).await?;

    if enable_compat {
        tracing::warn!(
            "COMPAT MODE ENABLED: /v1/sync/* accepts raw viewing keys. \
             This mode is for self-hosted deployments; the flagship keyless \
             feed does not need it. Bodies are never logged."
        );
    }

    // HTTP server on a shared read connection
    let server_db = Arc::new(Mutex::new(db.reopen()?));
    let state = strk20_indexerd::server::AppState {
        feed_dir: common.feed_dir.clone(),
        db: server_db.clone(),
        cfg: cfg.clone(),
    };
    let compat_state = enable_compat.then(|| strk20_indexerd::compat::CompatState {
        backend: strk20_indexerd::bridge::DbBackend::new(common.db.clone(), cfg.pool),
        db: server_db,
        pool: cfg.pool,
    });
    let router = strk20_indexerd::server::build_router(state, enable_raw, compat_state);
    let listener = tokio::net::TcpListener::bind(&listen)
        .await
        .with_context(|| format!("bind {listen}"))?;
    tracing::info!(addr = %listener.local_addr()?, "http server listening");
    tokio::spawn(async move {
        if let Err(e) = axum::serve(listener, router).await {
            tracing::error!(error = %e, "http server exited");
        }
    });

    // ingest loop
    let rpc_ref = &rpc;
    loop {
        let outcome = {
            let mut ingestor = Ingestor {
                db: &mut db,
                rpc: rpc_ref,
                cfg: &cfg,
                chunk_size: common.chunk_size,
            };
            ingestor.run_cycle().await
        };
        match outcome {
            Ok(o) => {
                let frontier = db.ingest_cursor()?.map(|(f, _)| f).unwrap_or(0);
                let cutter = Cutter {
                    db: &db,
                    rpc: rpc_ref,
                    cfg: &cfg,
                    feed_dir: common.feed_dir.clone(),
                };
                if let Err(e) = cutter.cut_ready_epochs(o.l1_accepted, frontier).await {
                    tracing::error!(error = %e, "epoch cutting halted");
                }
                if o.head_changed || o.blocks_ingested > 0 {
                    cutter.regen_head()?;
                }
            }
            Err(e) => tracing::error!(error = %e, "ingest cycle failed"),
        }
        tokio::time::sleep(std::time::Duration::from_millis(poll_ms)).await;
    }
}

async fn backfill(common: CommonOpts) -> Result<()> {
    let cfg = common.chain_config();
    let rpc = common.rpc();
    let mut db = Db::open(&common.db)?;
    init_checks(&db, &rpc, &cfg).await?;
    let started = std::time::Instant::now();
    loop {
        let outcome = {
            let mut ingestor = Ingestor {
                db: &mut db,
                rpc: &rpc,
                cfg: &cfg,
                chunk_size: common.chunk_size,
            };
            ingestor.run_cycle().await?
        };
        let frontier = db.ingest_cursor()?.map(|(f, _)| f).unwrap_or(0);
        let cutter = Cutter {
            db: &db,
            rpc: &rpc,
            cfg: &cfg,
            feed_dir: common.feed_dir.clone(),
        };
        cutter.cut_ready_epochs(outcome.l1_accepted, frontier).await?;
        cutter.regen_head()?;
        if outcome.blocks_ingested == 0 && frontier >= outcome.head_number {
            tracing::info!(
                elapsed_secs = started.elapsed().as_secs(),
                head = outcome.head_number,
                "backfill complete"
            );
            return Ok(());
        }
    }
}

fn status(common: CommonOpts) -> Result<()> {
    let db = Db::open(&common.db)?;
    let head = db.meta_get("head_number")?;
    let l1 = db.meta_get("l1_accepted_number")?;
    let decode = db.meta_get("decode_state")?;
    let cursor = db.ingest_cursor()?;
    let epochs = db.epoch_rows()?;
    println!("head:          {}", head.unwrap_or_else(|| "-".into()));
    println!("l1_accepted:   {}", l1.unwrap_or_else(|| "-".into()));
    println!("decode_state:  {}", decode.unwrap_or_else(|| "ok".into()));
    println!("ingest cursor: {cursor:?}");
    println!("epochs cut:    {}", epochs.len());
    for e in epochs.iter().rev().take(5) {
        println!(
            "  epoch {} [{}..{}] hash {} anchor {}",
            e.idx,
            e.from,
            e.to,
            hex::encode(e.content_hash),
            e.anchor_block.map(|b| b.to_string()).unwrap_or_else(|| "-".into())
        );
    }
    Ok(())
}

fn epoch_verify(common: CommonOpts, only: Option<u64>) -> Result<()> {
    let db = Db::open(&common.db)?;
    let rows = db.epoch_rows()?;
    let mut prev: Option<[u8; 32]> = None;
    let mut checked = 0usize;
    for row in &rows {
        let path = common
            .feed_dir
            .join("epochs")
            .join(format!("{:08}.strk20e.zst", row.idx));
        let compressed = std::fs::read(&path)
            .with_context(|| format!("read {}", path.display()))?;
        let payload = strk20_feed::decompress(&compressed)?;
        let entry = strk20_feed::manifest::ManifestEpoch {
            e: row.idx,
            from: row.from,
            to: row.to,
            hash: hex::encode(row.content_hash),
            zst: hex::encode(row.zst_sha256),
            bytes: row.file_size,
            anchor: None,
        };
        if only.is_none() || only == Some(row.idx) {
            strk20_feed::manifest::verify_epoch_against_manifest(&payload, &entry, prev)?;
            checked += 1;
        }
        prev = Some(strk20_feed::payload_sha256(&payload));
    }
    println!("verified {checked} epoch(s): hash chain OK");
    Ok(())
}

async fn verify_root(common: CommonOpts, block: Option<u64>) -> Result<()> {
    let cfg = common.chain_config();
    let rpc = common.rpc();
    let db = Db::open(&common.db)?;
    let target = match block {
        Some(b) => b,
        None => db
            .meta_get("l1_accepted_number")?
            .and_then(|s| s.parse().ok())
            .ok_or_else(|| anyhow::anyhow!("no l1_accepted in db; run ingest first"))?,
    };
    let cutter = Cutter {
        db: &db,
        rpc: &rpc,
        cfg: &cfg,
        feed_dir: common.feed_dir.clone(),
    };
    let anchor = cutter.verify_root(target).await?;
    println!(
        "verify-root OK at block {}: storage_root {} class {}",
        anchor.block,
        strk20_feed::felt_hex(&anchor.storage_root),
        strk20_feed::felt_hex(&anchor.class_hash)
    );
    Ok(())
}

async fn mirror_pull(common: CommonOpts, url: String) -> Result<()> {
    let base = url.trim_end_matches('/').to_owned();
    let http = reqwest::Client::builder()
        .user_agent(concat!("strk20-indexer/", env!("CARGO_PKG_VERSION")))
        .build()?;
    let manifest: strk20_feed::manifest::Manifest = http
        .get(format!("{base}/manifest.json"))
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let dir = common.feed_dir.join("epochs");
    std::fs::create_dir_all(&dir)?;
    let mut prev: Option<[u8; 32]> = None;
    for entry in &manifest.epochs {
        let name = format!("{:08}.strk20e.zst", entry.e);
        let bytes = http
            .get(format!("{base}/epochs/{name}"))
            .send()
            .await?
            .error_for_status()?
            .bytes()
            .await?;
        let payload = strk20_feed::decompress(&bytes)?;
        strk20_feed::manifest::verify_epoch_against_manifest(&payload, entry, prev)?;
        prev = Some(strk20_feed::payload_sha256(&payload));
        std::fs::write(dir.join(&name), &bytes)?;
        println!("epoch {}: verified + stored", entry.e);
    }
    // store the manifest and genesis as-is for onward serving
    let genesis = http
        .get(format!("{base}/genesis.json"))
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?;
    std::fs::write(common.feed_dir.join("genesis.json"), &genesis)?;
    std::fs::write(
        common.feed_dir.join("manifest.json"),
        serde_json::to_vec_pretty(&manifest)?,
    )?;
    println!(
        "mirror pull complete: {} epochs, chain verified",
        manifest.epochs.len()
    );
    Ok(())
}
