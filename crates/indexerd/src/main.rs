//! `strk20` — the server binary (spec §8).

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use starknet_types_core::felt::Felt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use strk20_indexerd::config::{self, ChainConfig};
use strk20_indexerd::cutter::{self, Cutter, VerifyOutcome};
use strk20_indexerd::db::Db;
use strk20_indexerd::ingest::{init_checks, Ingestor};
use strk20_indexerd::recovery;
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
    /// Chain profile: pool address, genesis block, chain id, decoder map and
    /// default RPC endpoints. Every flag below still overrides the profile.
    #[arg(long, value_enum, default_value_t = config::Network::Mainnet, env = "STRK20_NETWORK")]
    network: config::Network,
    /// Primary Starknet JSON-RPC URL
    #[arg(long, env = "STRK20_RPC_URL")]
    rpc_url: Option<String>,
    /// Fallback RPC URL
    #[arg(long, env = "STRK20_RPC_FALLBACK")]
    rpc_fallback: Option<String>,
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
    /// Seconds between scan progress lines; 0 reports every page
    #[arg(long, default_value_t = 15, env = "STRK20_PROGRESS_SECS")]
    progress_secs: u64,
    /// Additional known pool class hash(es) for the decoder map (recovery
    /// path after an upgrade; spec §5.7)
    #[arg(long = "allow-class")]
    allow_class: Vec<String>,
}

impl CommonOpts {
    fn chain_config(&self) -> ChainConfig {
        let mut cfg = self.network.profile();
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
        RpcClient::new(
            self.rpc_url
                .clone()
                .unwrap_or_else(|| self.network.rpc_primary().to_owned()),
            Some(
                self.rpc_fallback
                    .clone()
                    .unwrap_or_else(|| self.network.rpc_fallback().to_owned()),
            ),
        )
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
    /// Seeker pass: re-ask the chain for the whole block -> event-count map and
    /// report (or repair) blocks this mirror is missing or short of events
    AuditCoverage {
        #[command(flatten)]
        common: CommonOpts,
        /// First block to audit (default: the pool's genesis block)
        #[arg(long)]
        from: Option<u64>,
        /// Last block to audit (default: the ingest frontier — a mirror cannot
        /// be faulted for blocks it never claimed to have scanned)
        #[arg(long)]
        to: Option<u64>,
        /// Re-ingest the blocks the pass names, into the existing DB
        #[arg(long)]
        repair: bool,
        /// Also write the full report as JSON to this path
        #[arg(long)]
        json: Option<PathBuf>,
    },
    /// §5.6 slow path, as an operator command: re-ingest blocks straight from
    /// their state updates rather than events-first.
    ///
    /// This is the only way to recover a pool write that rode a block with NO
    /// pool event. `audit-coverage` cannot see those blocks — it compares
    /// event counts, and their event count is zero on both sides — and neither
    /// can the scanner, which asks `getEvents` what to ingest. Measured on
    /// Sepolia 2026-09-01: blocks 8,472,101 / 12,715,446 / 13,702,347 carry 17
    /// / 10 / 20 pool storage writes and zero pool events, and the mirror had
    /// no row for any of them, which is exactly what `verify-root` reported as
    /// a root divergence.
    Rescan {
        #[command(flatten)]
        common: CommonOpts,
        /// Walk [from..to] with one getStateUpdate per block. Complete, and
        /// priced accordingly — one call per block, so bound the range.
        #[arg(long, requires = "to")]
        from: Option<u64>,
        #[arg(long, requires = "from")]
        to: Option<u64>,
        /// Re-ingest exactly these blocks (comma-separated). Cheap, for when a
        /// diff has already named them.
        #[arg(long, value_delimiter = ',', conflicts_with = "from")]
        blocks: Vec<u64>,
    },
    /// Re-cut published epochs from one epoch upward after a repair changed
    /// blocks below the epoch floor (rewrites history; never automatic)
    RecutEpochs {
        #[command(flatten)]
        common: CommonOpts,
        /// Re-cut from the epoch containing this block (the repaired block)
        #[arg(long, conflicts_with = "from_epoch")]
        from_block: Option<u64>,
        /// Re-cut from this epoch index
        #[arg(long)]
        from_epoch: Option<u64>,
    },
    /// Import a feed directory from another instance (verified)
    MirrorPull {
        #[command(flatten)]
        common: CommonOpts,
        /// Source feed base URL (e.g. https://host/feed)
        url: String,
    },
    /// Enumerate the chain's pool storage trie by structure and name every
    /// slot this mirror is missing — the hole class nothing else can see.
    ///
    /// `audit-coverage` compares event counts, so a block with pool storage
    /// writes and ZERO pool events is invisible to it by construction, and
    /// `getEvents` cannot name that block either. `verify-root` proves such a
    /// hole exists but not where it is. This walks the chain's trie from a
    /// bound storage proof: nodes are keyed by their own hash and name their
    /// children by hash, so a child hash commits to a whole subtree and can be
    /// compared against the same quantity computed from the mirror. Equal
    /// prunes, unequal descends, and a full-depth disagreement is a missing
    /// slot. Cost scales with the divergence, not with the block range.
    ///
    /// Read-only. It names the blocks; `strk20 rescan --blocks` repairs them.
    EnumerateSlots {
        #[command(flatten)]
        common: CommonOpts,
        /// Block to walk at; must be provable. Default: the ingest frontier.
        #[arg(long)]
        block: Option<u64>,
        /// Also bisect each missing slot to the block that wrote it, so the
        /// output is a `rescan --blocks` list. Costs ~⌈log₂ range⌉
        /// `getStorageAt` calls per CLUSTER, not per slot.
        #[arg(long)]
        attribute: bool,
        /// Write the full diff as JSON to this path
        #[arg(long)]
        json: Option<PathBuf>,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Logs on stderr, results on stdout: `verify-root`/`status` output is
    // parsed by operators and tests, and must not be interleaved with tracing.
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info".into()),
        )
        .with_writer(std::io::stderr)
        // Colour only for a human terminal: piped logs are parsed by ops
        // tooling and must not carry escape sequences inside field values.
        .with_ansi(std::io::IsTerminal::is_terminal(&std::io::stderr()))
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
        Command::AuditCoverage {
            common,
            from,
            to,
            repair,
            json,
        } => audit_coverage(common, from, to, repair, json).await,
        Command::Rescan {
            common,
            from,
            to,
            blocks,
        } => rescan(common, from, to, blocks).await,
        Command::RecutEpochs {
            common,
            from_block,
            from_epoch,
        } => recut_epochs(common, from_block, from_epoch),
        Command::MirrorPull { common, url } => mirror_pull(common, url).await,
        Command::EnumerateSlots {
            common,
            block,
            attribute,
            json,
        } => enumerate_slots(common, block, attribute, json).await,
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
    let live = Arc::new(strk20_indexerd::live::LiveHub::new(
        common.feed_dir.clone(),
        server_db.clone(),
    ));
    tokio::spawn(strk20_indexerd::live::run_watcher(live.clone()));
    let state = strk20_indexerd::server::AppState {
        feed_dir: common.feed_dir.clone(),
        db: server_db.clone(),
        cfg: cfg.clone(),
        live,
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
                progress_secs: common.progress_secs,
            };
            ingestor.run_cycle().await
        };
        match outcome {
            Ok(o) => {
                // Publish the tail BEFORE cutting. `/health` already reports the
                // new head at this point, and the cut path can take a while
                // (verify-root, the anchor probe, or a §5.6 rescan) — a consumer
                // that polls /health and then fetches head.ndjson must not get a
                // tail from before the block it was just told about.
                {
                    let cutter = Cutter {
                        db: &db,
                        rpc: rpc_ref,
                        cfg: &cfg,
                        feed_dir: common.feed_dir.clone(),
                    };
                    if o.head_changed || o.blocks_ingested > 0 {
                        cutter.regen_head()?;
                    }
                    if o.reorged {
                        // The rollback dropped anchors above the ancestor;
                        // republish so the feed stops serving them.
                        cutter.write_anchors()?;
                    }
                }
                let cut =
                    cut_epochs_with_recovery(&mut db, &rpc, &cfg, &common, o.l1_accepted).await;
                if cut > 0 {
                    // The epoch floor moved: the tail starts higher now.
                    let cutter = Cutter {
                        db: &db,
                        rpc: rpc_ref,
                        cfg: &cfg,
                        feed_dir: common.feed_dir.clone(),
                    };
                    cutter.regen_head()?;
                }
            }
            Err(e) => tracing::error!(error = %e, "ingest cycle failed"),
        }
        tokio::time::sleep(std::time::Duration::from_millis(poll_ms)).await;
    }
}

/// Cut ready epochs. A verify-root mismatch gets ONE bounded recovery attempt
/// — the sound-ingest.md §4.2 closure loop — and then the divergence is on
/// record; every later cycle that meets it again returns immediately so ingest
/// keeps running, with `verify_root_failed` latched and `/health` DEGRADED.
///
/// The two properties this function exists to hold, in order of importance:
///
/// 1. **Ingest is never blocked for longer than one attempt.** The old path
///    re-entered a non-convergent window rescan on every cycle whose head had
///    moved — tens of minutes each, forever. Recovery now happens at most once
///    per divergence and is additionally capped by `ATTEMPT_DEADLINE`.
/// 2. **The attempt localises the divergence instead of guessing at it.** The
///    window rescan was derived from the PROBE block and provably could not
///    reach a divergence below it (§2.3). The walk asks the chain's trie which
///    slots are missing and bisects them to their writing blocks, so its cost
///    tracks the size of the hole rather than the distance to the frontier.
async fn cut_epochs_with_recovery(
    db: &mut Db,
    rpc: &RpcClient,
    cfg: &ChainConfig,
    common: &CommonOpts,
    l1_accepted: u64,
) -> u64 {
    for attempt in 0..2 {
        let frontier = db.ingest_cursor().ok().flatten().unwrap_or(0);
        let cutter = Cutter {
            db,
            rpc,
            cfg,
            feed_dir: common.feed_dir.clone(),
        };
        let err = match cutter.cut_ready_epochs(l1_accepted, frontier).await {
            Ok(n) => return n,
            Err(e) => e,
        };
        let Some(mismatch) = cutter::root_mismatch_of(&err) else {
            tracing::error!(error = %err, "epoch cutting halted");
            // A retry that dies for an unrelated reason must not leave
            // `/health` serving "recovery in progress" forever.
            if attempt > 0 {
                if let Some(d) = recovery::recorded(db)
                    .ok()
                    .flatten()
                    .as_deref()
                    .and_then(recovery::Divergence::parse)
                {
                    log_and_record(
                        db,
                        &d,
                        "recovery attempted once; the retried cut then failed for an \
                         unrelated reason",
                        &err,
                    );
                }
            }
            return 0;
        };
        let divergence = recovery::Divergence::from(mismatch);
        if attempt > 0 {
            // The one attempt has been spent inside this very call: the walk
            // and the re-ingest ran and the chain still disagrees.
            let detail = "recovery attempted once (storage-trie walk + targeted rescan) and \
                          the mirror still disagrees";
            log_and_record(db, &divergence, detail, &err);
            return 0;
        }
        // A mismatch during a reorg is the mirror holding the abandoned
        // branch, not a hole. The next cycle rolls it back; recovery would
        // burn its one attempt on a branch that is about to be discarded.
        match recovery::reorg_in_flight(db, rpc).await {
            Ok(true) => {
                tracing::warn!(
                    block = divergence.block,
                    "verify-root mismatched while a reorg is in flight; the canonicity \
                     rollback owns this, so no recovery attempt is spent on it"
                );
                return 0;
            }
            Ok(false) => {}
            Err(e) => {
                // Not knowing is not the same as knowing there is no reorg.
                tracing::error!(error = %e, "cannot tell a reorg from a hole; not attempting");
                return 0;
            }
        }
        match recovery::decide(db, &divergence) {
            Ok(recovery::Decision::Skip { recorded, same }) => {
                // The whole fix, in one branch: no work, no starvation, and
                // one line only when a line is due.
                if recovery::should_log_repeat(db, recovery::now_secs(), recovery::REPEAT_LOG_SECS)
                    .unwrap_or(true)
                {
                    tracing::warn!(
                        block = divergence.block,
                        recorded = %recorded,
                        identical = same,
                        reason = %recovery::reason(db).ok().flatten().unwrap_or_default(),
                        "verify-root still MISMATCHes and this divergence has already had its \
                         one recovery attempt; skipping recovery and continuing to ingest. \
                         Publication stays blocked until verify-root passes. Repair by hand: \
                         strk20 enumerate-slots --attribute, strk20 rescan --blocks <list>, \
                         strk20 recut-epochs --from-block <lowest>."
                    );
                }
                return 0;
            }
            Ok(recovery::Decision::Attempt) => {}
            Err(e) => {
                tracing::error!(error = %e, "cannot read the recovery record; not attempting");
                return 0;
            }
        }

        tracing::error!(
            error = %err,
            block = divergence.block,
            "verify-root mismatch: entering the §4.2 closure loop, once"
        );
        if let Err(e) = recovery::begin_attempt(db, &divergence) {
            tracing::error!(error = %e, "cannot record the divergence; not attempting recovery");
            return 0;
        }
        let closure = recovery::run_closure(
            db,
            rpc,
            cfg,
            common.feed_dir.clone(),
            common.chunk_size,
            common.progress_secs,
            &divergence,
        );
        match tokio::time::timeout(recovery::ATTEMPT_DEADLINE, closure).await {
            Ok(Ok(report)) => {
                tracing::warn!(
                    missing_slots = report.missing_slots,
                    divergent_slots = report.divergent_slots,
                    extra_slots = report.extra_slots,
                    proof_calls = report.proof_calls,
                    blocks = ?report.blocks,
                    reingested = report.reingested,
                    elapsed_secs = report.elapsed_secs,
                    "closure loop finished; retrying the cut once"
                );
                if let Some(lowest) = report.blocks.first() {
                    if let Ok(Some((_, _, epoch_to))) = db.last_epoch() {
                        if *lowest <= epoch_to {
                            tracing::warn!(
                                block = lowest,
                                epoch = cfg.epoch_of(*lowest),
                                "the repair landed BELOW the epoch floor, so already-published \
                                 epochs still carry their pre-repair bytes: run \
                                 `strk20 recut-epochs --from-block {lowest}`, then \
                                 `strk20 epoch-verify`. Consumers see the content hashes change."
                            );
                        }
                    }
                }
            }
            Ok(Err(e)) if strk20_indexerd::rpc::is_proof_unavailable(&e) => {
                // A capability gap, not a mirror defect: say so, keep the
                // latch (verify-root DID mismatch), keep ingesting.
                log_and_record(
                    db,
                    &divergence,
                    "recovery attempted once and no endpoint served a storage proof for the \
                     walk (UNAVAILABLE)",
                    &e,
                );
                return 0;
            }
            Ok(Err(e)) => {
                log_and_record(db, &divergence, "recovery attempted once and failed", &e);
                return 0;
            }
            Err(_) => {
                log_and_record(
                    db,
                    &divergence,
                    &format!(
                        "recovery attempted once and hit its {}s deadline",
                        recovery::ATTEMPT_DEADLINE.as_secs()
                    ),
                    &anyhow::anyhow!("attempt deadline"),
                );
                return 0;
            }
        }
    }
    0
}

/// One error line and one persisted `reason`, so the operator reads the same
/// sentence in the log and on `/health`.
fn log_and_record(db: &Db, d: &recovery::Divergence, detail: &str, err: &anyhow::Error) {
    if let Err(e) = recovery::record_outcome(db, d, detail) {
        tracing::error!(error = %e, "cannot record the recovery outcome");
    }
    tracing::error!(
        error = %format!("{err:#}"),
        block = d.block,
        reason = %recovery::reason(db).ok().flatten().unwrap_or_default(),
        "epoch cutting halted and publication stays blocked, but INGEST CONTINUES. \
         This divergence will not be retried automatically."
    );
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
                progress_secs: common.progress_secs,
            };
            ingestor.run_cycle().await?
        };
        let _ = cut_epochs_with_recovery(&mut db, &rpc, &cfg, &common, outcome.l1_accepted).await;
        let frontier = db.ingest_cursor()?.unwrap_or(0);
        let cutter = Cutter {
            db: &db,
            rpc: &rpc,
            cfg: &cfg,
            feed_dir: common.feed_dir.clone(),
        };
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
    let cutter = Cutter {
        db: &db,
        rpc: &rpc,
        cfg: &cfg,
        feed_dir: common.feed_dir.clone(),
    };
    let outcome = match block {
        Some(b) => VerifyOutcome::Verified(cutter.verify_root(b).await?),
        None => {
            let frontier = db
                .ingest_cursor()?
                .ok_or_else(|| anyhow::anyhow!("no ingest cursor in db; run ingest first"))?;
            cutter.verify_root_at_target(frontier).await?
        }
    };
    match outcome {
        VerifyOutcome::Verified(anchor) => println!(
            "verify-root OK at block {}: storage_root {} class {}",
            anchor.block,
            strk20_feed::felt_hex(&anchor.storage_root),
            strk20_feed::felt_hex(&anchor.class_hash)
        ),
        // "We could not check" is not "the mirror is wrong": a capability gap
        // is reported, and the exit status stays zero.
        VerifyOutcome::Unavailable(why) => println!(
            "verify-root UNAVAILABLE: every configured endpoint spent its whole storage-proof \
             retry budget refusing ({why}). This is a statement about the providers and says \
             nothing about mirror correctness."
        ),
    }
    Ok(())
}

/// Structural enumeration of the chain's storage trie (sound-ingest.md §1, the
/// eventless-write hole class). Read-only: it names slots and, with
/// `--attribute`, the blocks that wrote them. Repair is `rescan --blocks`.
async fn enumerate_slots(
    common: CommonOpts,
    block: Option<u64>,
    attribute: bool,
    json: Option<PathBuf>,
) -> Result<()> {
    let cfg = common.chain_config();
    let rpc = common.rpc();
    let db = Db::open(&common.db)?;
    let cutter = Cutter {
        db: &db,
        rpc: &rpc,
        cfg: &cfg,
        feed_dir: common.feed_dir.clone(),
    };
    let target = match block {
        Some(b) => b,
        None => db
            .ingest_cursor()?
            .ok_or_else(|| anyhow::anyhow!("no ingest cursor in db; run ingest first"))?,
    };

    let diff = strk20_indexerd::trie_walk::enumerate_missing_slots(&cutter, target).await?;

    println!("enumerate-slots at block {}", diff.block);
    println!("  chain storage_root {}", strk20_feed::felt_hex(&diff.chain_root));
    println!("  local storage_root {}", strk20_feed::felt_hex(&diff.local_root));
    println!("  mirror slots       {}", diff.local_leaves);
    println!(
        "  proof calls        {} over {} round(s)",
        diff.proof_calls, diff.rounds
    );
    println!("  missing slots      {}", diff.missing.len());
    println!("  divergent values   {}", diff.divergent.len());
    println!("  slots not on chain {}", diff.extra.len());

    let mut blocks: Vec<u64> = Vec::new();
    if attribute && !diff.missing.is_empty() {
        let slots: Vec<Felt> = diff.missing.iter().map(|(k, _)| *k).collect();
        blocks = strk20_indexerd::trie_walk::attribute_to_blocks(
            &cutter,
            &slots,
            cfg.genesis_block,
            diff.block,
        )
        .await?;
        println!(
            "  blocks to rescan   {}: {}",
            blocks.len(),
            blocks
                .iter()
                .map(|b| b.to_string())
                .collect::<Vec<_>>()
                .join(",")
        );
    }

    if let Some(path) = json {
        let report = serde_json::json!({
            "block": diff.block,
            "chain_root": strk20_feed::felt_hex(&diff.chain_root),
            "local_root": strk20_feed::felt_hex(&diff.local_root),
            "mirror_slots": diff.local_leaves,
            "proof_calls": diff.proof_calls,
            "rounds": diff.rounds,
            "missing": diff.missing.iter().map(|(k, v)| serde_json::json!({
                "slot": strk20_feed::felt_hex(k),
                "chain_value": strk20_feed::felt_hex(v),
            })).collect::<Vec<_>>(),
            "divergent": diff.divergent.iter().map(|(k, ours, theirs)| serde_json::json!({
                "slot": strk20_feed::felt_hex(k),
                "mirror_value": strk20_feed::felt_hex(ours),
                "chain_value": strk20_feed::felt_hex(theirs),
            })).collect::<Vec<_>>(),
            "extra": diff.extra.iter().map(strk20_feed::felt_hex).collect::<Vec<_>>(),
            "blocks": blocks,
        });
        std::fs::write(&path, serde_json::to_vec_pretty(&report)?)
            .with_context(|| format!("write {}", path.display()))?;
        println!("  report written to  {}", path.display());
    }

    if diff.is_clean() {
        println!(
            "enumerate-slots CLEAN: the chain's trie at block {} holds no pool slot this \
             mirror is missing.",
            diff.block
        );
    } else {
        println!(
            "enumerate-slots INCOMPLETE: repair with `strk20 rescan --blocks <list>`, then \
             re-run this command until it reports CLEAN."
        );
    }
    Ok(())
}

/// The seeker pass, and with `--repair` the targeted re-ingest that follows it
/// (docs/pre-submission-corrections.md plan A steps 1–2).
///
/// A hole below the ingest frontier is invisible to every forward mechanism
/// this binary has: the scan starts at `cursor + 1`, the §5.6 rescan only
/// widens to the epoch of a mismatch it was handed, and `verify-root` can say
/// a root diverged but not which blocks are absent. Re-asking the chain for the
/// whole block → event-count map is the only thing that names them, and doing
/// it with `getEvents` alone costs a fraction of the re-backfill that would
/// otherwise be the answer.
async fn audit_coverage(
    common: CommonOpts,
    from: Option<u64>,
    to: Option<u64>,
    repair: bool,
    json: Option<PathBuf>,
) -> Result<()> {
    let cfg = common.chain_config();
    let rpc = common.rpc();
    let mut db = Db::open(&common.db)?;
    init_checks(&db, &rpc, &cfg).await?;
    let frontier = db.ingest_cursor()?;
    let from = from.unwrap_or(cfg.genesis_block);
    let to = match to {
        Some(t) => t,
        // Auditing above the frontier would report blocks the mirror never
        // claimed to have scanned as losses.
        None => frontier.ok_or_else(|| {
            anyhow::anyhow!(
                "this db has no ingest cursor, so nothing has been scanned and there is no \
                 coverage to audit. Run `strk20 backfill` first, or pass --to explicitly."
            )
        })?,
    };
    anyhow::ensure!(
        from <= to,
        "empty audit range [{from}..{to}]: --from must not be above --to"
    );

    let mut ingestor = Ingestor {
        db: &mut db,
        rpc: &rpc,
        cfg: &cfg,
        chunk_size: common.chunk_size,
        progress_secs: common.progress_secs,
    };
    let report = ingestor.audit_coverage(from, to).await?;
    print_coverage(&report);

    let mut final_report = report.clone();
    if repair && !report.is_complete() {
        let blocks = report.repair_blocks();
        println!("\nre-ingesting {} block(s)...", blocks.len());
        let repaired = ingestor.reingest_blocks(&blocks).await?;
        final_report = report.refreshed_after_repair(ingestor.db)?;
        println!("re-ingested {repaired} block(s); rechecking them:");
        print_coverage(&final_report);
    }

    if let Some(path) = &json {
        std::fs::write(path, serde_json::to_vec_pretty(&final_report)?)
            .with_context(|| format!("write {}", path.display()))?;
    }

    // Whether the feed still carries pre-repair bytes is a question about the
    // blocks the REPAIR touched, not about whatever is left unrepaired.
    let repaired_lowest = repair.then(|| report.repair_blocks().first().copied()).flatten();
    if let Some(lowest) = repaired_lowest {
        match db.last_epoch()? {
            // A repaired block below the epoch floor sits inside bytes that are
            // already published, and nothing rewrites those automatically.
            Some((_, _, epoch_to)) if lowest <= epoch_to => println!(
                "\nBlock {lowest} is inside already-cut epoch {}, so the published feed still \
                 carries the pre-repair bytes. Republish with \
                 `strk20 recut-epochs --from-block {lowest}`, then `strk20 epoch-verify` (the \
                 re-cut rewrites published history one epoch at a time; epoch-verify is what \
                 says the whole chain landed), then re-check with `verify-root`.",
                cfg.epoch_of(lowest)
            ),
            _ => println!(
                "\nEvery repaired block is above the epoch floor, so no published epoch \
                 changed; the next `strk20 run` cycle regenerates head.ndjson from the \
                 repaired database."
            ),
        }
    }

    if final_report.is_complete() {
        println!(
            "\naudit-coverage OK: every pool-active block the chain has in [{from}..{to}] is \
             in this mirror, with the chain's event count."
        );
        return Ok(());
    }
    let lowest = final_report.repair_blocks().first().copied().unwrap_or(from);
    if repair {
        println!(
            "\naudit-coverage INCOMPLETE: {} block(s) still disagree with the chain after the \
             re-ingest. That is not a scan problem — re-run and, if it persists, treat it as a \
             provider or ingest defect rather than a hole.",
            final_report.repair_blocks().len()
        );
    } else {
        println!(
            "\naudit-coverage INCOMPLETE: {} block(s) need re-ingest. Repair with \
             `strk20 audit-coverage --repair`.",
            final_report.repair_blocks().len()
        );
    }
    if !repair {
        if let Some((_, _, epoch_to)) = db.last_epoch()? {
            if lowest <= epoch_to {
                println!(
                    "Block {lowest} is inside already-cut epoch {}, so repairing it also \
                     changes bytes that are already published: the repair is followed by \
                     `strk20 recut-epochs --from-block {lowest}` and then \
                     `strk20 epoch-verify`.",
                    cfg.epoch_of(lowest)
                );
            }
        }
    }
    Ok(())
}

/// §5.6 slow path on demand. Two shapes, same ingest path: a bounded range
/// walked one `getStateUpdate` at a time, or an explicit block list.
///
/// Why this exists as a command: the mismatch recovery inside `run` rescans
/// only `[last_epoch.to + 1 .. frontier]`, and when the missing write is older
/// than that it prints "re-run with --full-resync" — a flag that does not
/// exist, for a rebuild that costs a full backfill. A divergence localized to
/// a handful of blocks deserves a repair the size of the divergence.
async fn rescan(
    common: CommonOpts,
    from: Option<u64>,
    to: Option<u64>,
    blocks: Vec<u64>,
) -> Result<()> {
    let cfg = common.chain_config();
    let rpc = common.rpc();
    let mut db = Db::open(&common.db)?;
    init_checks(&db, &rpc, &cfg).await?;
    let mut ingestor = Ingestor {
        db: &mut db,
        rpc: &rpc,
        cfg: &cfg,
        chunk_size: common.chunk_size,
        progress_secs: common.progress_secs,
    };
    let (touched, lowest) = match (from, to) {
        (Some(f), Some(t)) => {
            anyhow::ensure!(f <= t, "empty range [{f}..{t}]");
            println!("rescanning [{f}..{t}] from per-block state updates...");
            (ingestor.rescan_range(f, t).await?, f)
        }
        _ => {
            anyhow::ensure!(
                !blocks.is_empty(),
                "nothing to do: pass --from/--to for a range, or --blocks for a list"
            );
            let mut list = blocks;
            list.sort_unstable();
            list.dedup();
            let lowest = list[0];
            println!("re-ingesting {} named block(s)...", list.len());
            (ingestor.reingest_blocks(&list).await?, lowest)
        }
    };
    println!("rescan touched {touched} block(s) that write pool storage");
    if touched == 0 {
        return Ok(());
    }
    match db.last_epoch()? {
        Some((_, _, epoch_to)) if lowest <= epoch_to => println!(
            "\nBlock {lowest} is inside already-cut epoch {}, so the published feed still \
             carries the pre-repair bytes. Republish with \
             `strk20 recut-epochs --from-block {lowest}`, then `strk20 epoch-verify`, then \
             re-check with `verify-root`.",
            cfg.epoch_of(lowest)
        ),
        _ => println!(
            "\nEvery touched block is above the epoch floor, so no published epoch changed; \
             the next `strk20 run` cycle regenerates head.ndjson from the repaired database."
        ),
    }
    Ok(())
}

/// Cap on how many gap lines one section prints; the JSON report is complete.
const GAP_PRINT_LIMIT: usize = 50;

fn print_coverage(r: &strk20_indexerd::ingest::CoverageReport) {
    println!("audit-coverage [{}..{}]", r.from, r.to);
    println!(
        "  chain:  {} pool-active blocks, {} events",
        r.chain_blocks, r.chain_events
    );
    println!(
        "  mirror: {} of those blocks, {} events",
        r.mirror_blocks, r.mirror_events
    );
    println!("  missing blocks:      {}", r.missing.len());
    println!("  undercounted blocks: {}", r.undercounted.len());
    println!("  overcounted blocks:  {}", r.overcounted.len());
    for (label, gaps) in [
        ("MISSING", &r.missing),
        ("UNDERCOUNT", &r.undercounted),
        ("OVERCOUNT", &r.overcounted),
    ] {
        for g in gaps.iter().take(GAP_PRINT_LIMIT) {
            println!(
                "    {label:<10} block {}: chain {} event(s), mirror {}",
                g.block, g.chain_events, g.mirror_events
            );
        }
        if gaps.len() > GAP_PRINT_LIMIT {
            println!(
                "    {label:<10} ... and {} more (see --json for the full list)",
                gaps.len() - GAP_PRINT_LIMIT
            );
        }
    }
}

/// Backward epoch re-cut (plan A step 2). Explicit by construction: its own
/// subcommand, never called from the ingest loop, and refused outright unless
/// the first epoch named actually rebuilds to different bytes.
fn recut_epochs(
    common: CommonOpts,
    from_block: Option<u64>,
    from_epoch: Option<u64>,
) -> Result<()> {
    let cfg = common.chain_config();
    let db = Db::open(&common.db)?;
    let idx = match (from_epoch, from_block) {
        (Some(e), _) => e,
        (None, Some(b)) => cfg.epoch_of(b),
        (None, None) => anyhow::bail!(
            "name where to re-cut from: --from-block <block> (usually the lowest block the \
             repair touched) or --from-epoch <idx>"
        ),
    };
    // A Cutter is constructed with an RpcClient, but a re-cut makes no calls:
    // DB → NDJSON → zstd → manifest, all local.
    let rpc = common.rpc();
    let cutter = Cutter {
        db: &db,
        rpc: &rpc,
        cfg: &cfg,
        feed_dir: common.feed_dir.clone(),
    };
    let out = cutter.recut_epochs_from(idx)?;
    println!(
        "re-cut {} epoch(s) from epoch {}:",
        out.rewritten.len(),
        out.first_epoch
    );
    if !out.already_current.is_empty() {
        // Only an interrupted earlier re-cut produces this, so say what it
        // means rather than leaving the operator to wonder why the range they
        // named was not fully rewritten.
        println!(
            "  {} epoch(s) already matched this database and were left alone ({:?}) — an \
             earlier re-cut of this range got that far before it stopped.",
            out.already_current.len(),
            out.already_current
        );
    }
    for (e, old, new) in &out.rewritten {
        println!("  epoch {e}: {} -> {}", hex::encode(old), hex::encode(new));
    }
    if !out.snapshots_dropped.is_empty() {
        println!(
            "withdrew {} snapshot(s) whose epoch was re-cut: {:?} — the next cut republishes \
             from the repaired database.",
            out.snapshots_dropped.len(),
            out.snapshots_dropped
        );
    }
    println!(
        "manifest rewritten; every client re-verifies the chain from epoch {idx} up.\n\
         Next: `strk20 epoch-verify` — it re-reads every published file and walks the hash \
         chain, which is the only confirmation that the re-cut landed in full. If this \
         command ever stops part-way, re-run it unchanged: it resumes from the first epoch \
         that is still stale."
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
    let cfg = common.chain_config();
    if manifest.chain_id != cfg.chain_id {
        anyhow::bail!(
            "mirror feed is for chain {} but this instance is configured for {}",
            manifest.chain_id,
            cfg.chain_id
        );
    }
    let mut db = Db::open(&common.db)?;
    let mut prev: Option<[u8; 32]> = None;
    let mut last_to = 0u64;
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
        let epoch =
            strk20_feed::manifest::verify_epoch_against_manifest(&payload, entry, prev)?;
        // The hash chain proves the epoch is the one the manifest names; it
        // does not say which chain the manifest is about. Refuse a feed whose
        // epochs are stamped for another chain or pool before ingesting it.
        strk20_feed::manifest::verify_epoch_binding(
            &epoch,
            &manifest.chain_id,
            &strk20_feed::felt_from_hex(&manifest.pool)?,
        )?;
        // Ingest the verified payload into the DB: a later `strk20 run`
        // continues from the feed head instead of clobbering the manifest
        // (review finding: mirror_pull never populated the DB).
        for b in &epoch.blocks {
            let row = strk20_indexerd::db::BlockRow {
                number: b.number,
                hash: b.hash,
                parent_hash: b.parent,
                timestamp: b.timestamp,
                l1_accepted: true, // epochs are cut ≤ l1_accepted by construction
            };
            let events: Vec<strk20_indexerd::db::EventRow> = b
                .events
                .iter()
                .map(|e| strk20_indexerd::db::EventRow {
                    block: b.number,
                    event_index: e.event_index,
                    tx_index: e.tx_index,
                    tx_hash: e.tx_hash,
                    keys: e.keys.clone(),
                    data: e.data.clone(),
                })
                .collect();
            db.insert_block_data(
                &row,
                &b.diffs,
                &events,
                b.replaced_class.as_ref(),
                b.number,
            )?;
            db.record_seen_head(&b.hash, b.number)?;
        }
        let content_hash = strk20_feed::payload_sha256(&payload);
        let zst_hash = strk20_feed::payload_sha256(&bytes);
        db.insert_epoch(
            entry.e,
            entry.from,
            entry.to,
            &content_hash,
            &zst_hash,
            bytes.len() as u64,
            prev.as_ref(),
            None,
            0,
        )?;
        prev = Some(content_hash);
        last_to = entry.to;
        std::fs::write(dir.join(&name), &bytes)?;
        println!("epoch {}: verified + stored + ingested", entry.e);
    }
    if last_to > 0 {
        db.set_ingest_cursor(last_to)?;
        db.meta_set("chain_id", &cfg.chain_id)?;
        db.meta_set("pool_address", &strk20_feed::felt_hex(&cfg.pool))?;
        db.meta_set("genesis_block", &cfg.genesis_block.to_string())?;
        db.meta_set("epoch_size", &cfg.epoch_size.to_string())?;
        db.meta_set("schema_version", &strk20_indexerd::db::SCHEMA_VERSION.to_string())?;
        db.meta_set("decode_state", "ok")?;
    }
    // Store the manifest and genesis for onward serving, MINUS the origin's
    // snapshot: mirror-pull ingests epochs only (§1.9 — a server needs events
    // to cut future epochs and can never bootstrap from a slots-only file), so
    // advertising a snapshot file this mirror does not hold would 404 every
    // client that believed the manifest. This mirror publishes its own after
    // its first cut batch, byte-identical to the origin's.
    let mut manifest = manifest;
    manifest.snapshot = None;
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
