//! One bounded recovery attempt per divergence — the §4.2 closure loop, and
//! the guard that keeps it from eating the poll loop.
//!
//! **What was wrong.** `run` cut epochs after every cycle that moved the head.
//! A `VERIFY-ROOT MISMATCH` sent it into a §5.6 rescan of a window derived
//! from the PROBE block, which is by construction near the frontier while the
//! divergence can sit five million blocks below it (sound-ingest.md §2.3). The
//! rescan therefore could not converge — measured: 4 rounds, 2.46 hours, 0
//! blocks repaired — and, worse, nothing remembered that it had already been
//! tried. The next cycle's head move re-entered the same rescan, so on the
//! hosted Sepolia instance ingest was starved in tens-of-minutes blocks, the
//! log went silent, and the operator saw a frozen head with a DEGRADED health
//! endpoint and no line explaining either.
//!
//! **What replaces it.** Two rules, and they are separable:
//!
//! 1. *At most one recovery attempt per unresolved divergence.* The identity
//!    of the divergence being handled is persisted in `meta`, so the guard
//!    survives a restart; a verify-root MATCH — auto-healed or repaired by an
//!    operator — clears it and re-arms the loop.
//! 2. *The attempt is the closure loop, not a window rescan.* Walk the chain's
//!    storage trie at the mismatch block, attribute the missing slots to the
//!    blocks that wrote them, re-ingest exactly those, retry the cut once.
//!    Cost is proportional to the size of the divergence rather than to the
//!    distance to the frontier, which is the property the window rescan never
//!    had.
//!
//! **Why the stored identity is not the decision.** The natural reading of
//! "the same divergence" is "the same three numbers", and on a quiet chain it
//! is. On a live one it is not: the probe block is `min(frontier, head)` and
//! moves every cycle, and both roots move with every pool write. Keying the
//! decision on the fingerprint would call every cycle a NEW divergence and
//! reproduce the starvation exactly. So the fingerprint is recorded, compared
//! and reported — it is what tells an operator whether the divergence moved —
//! while the decision is the coarser and provably terminating one: while an
//! unresolved divergence is on record, do not attempt again. A mirror that has
//! failed one repair is still missing a write at or below the previous
//! mismatch; a second identical attempt cannot learn anything the first did
//! not. This is also what makes the guard safe against §7.10, where a mutable
//! admin slot can make the bisection predicate non-monotone and the reported
//! block wander: a wandering block changes the fingerprint, and under this
//! rule a changed fingerprint still does not buy another attempt.

use crate::config::ChainConfig;
use crate::cutter::{Cutter, RootMismatch};
use crate::db::Db;
use crate::ingest::Ingestor;
use crate::rpc::RpcClient;
use anyhow::{Context, Result};
use starknet_types_core::felt::Felt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use strk20_feed::{felt_from_hex, felt_hex};

/// Fingerprint of the divergence currently on record: `block:local:chain`.
pub const DIVERGENCE_KEY: &str = "recovery_divergence";
/// Recovery attempts spent on the divergence on record. Reset with it.
pub const ATTEMPTS_KEY: &str = "recovery_attempts";
/// The short operator-facing sentence `/health` serves as `reason`.
pub const REASON_KEY: &str = "recovery_reason";
/// Unix seconds of the last "still diverged" warn, for the rate limit.
pub const LOGGED_AT_KEY: &str = "recovery_logged_at";

/// How often a divergence that has already had its attempt is re-announced.
/// A poll cycle is seconds; without this the one line that matters would be
/// buried under thousands of identical ones and the log would be as useless as
/// the silence it replaced.
pub const REPEAT_LOG_SECS: u64 = 600;

/// Wall-clock ceiling on one recovery attempt. The walk and the bisection are
/// each internally bounded (`trie_walk::MAX_ROUNDS`, ⌈log₂ range⌉ probes), so
/// this is the backstop for a provider that answers slowly rather than wrongly
/// — the property being defended is that ingest resumes, not that the repair
/// succeeds.
pub const ATTEMPT_DEADLINE: Duration = Duration::from_secs(900);

/// A verify-root divergence, as recorded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Divergence {
    pub block: u64,
    pub local_root: Felt,
    pub chain_root: Felt,
}

impl From<RootMismatch> for Divergence {
    fn from(m: RootMismatch) -> Self {
        Self {
            block: m.block,
            local_root: m.local_root,
            chain_root: m.chain_root,
        }
    }
}

impl Divergence {
    /// `block:local_root:chain_root`. Stable, greppable, and parseable back —
    /// the three numbers are the whole identity, so nothing is hashed away.
    pub fn fingerprint(&self) -> String {
        format!(
            "{}:{}:{}",
            self.block,
            felt_hex(&self.local_root),
            felt_hex(&self.chain_root)
        )
    }

    pub fn parse(s: &str) -> Option<Self> {
        let mut parts = s.split(':');
        let block = parts.next()?.parse().ok()?;
        let local_root = felt_from_hex(parts.next()?).ok()?;
        let chain_root = felt_from_hex(parts.next()?).ok()?;
        if parts.next().is_some() {
            return None;
        }
        Some(Self {
            block,
            local_root,
            chain_root,
        })
    }
}

/// What the poll loop should do about a mismatch it was just handed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    /// Nothing is on record: this divergence gets its one attempt.
    Attempt,
    /// A divergence is already on record and no verify-root has passed since.
    /// `same` distinguishes "the identical three numbers again" from "the
    /// divergence moved" — reported, never acted on differently.
    Skip { recorded: String, same: bool },
}

/// Read the fingerprint on record.
pub fn recorded(db: &Db) -> Result<Option<String>> {
    Ok(db.meta_get(DIVERGENCE_KEY)?.filter(|s| !s.is_empty()))
}

/// The block of the divergence on record, for `/health`.
pub fn mismatch_block(db: &Db) -> Result<Option<u64>> {
    Ok(recorded(db)?
        .as_deref()
        .and_then(Divergence::parse)
        .map(|d| d.block))
}

/// The operator-facing sentence on record, for `/health`.
pub fn reason(db: &Db) -> Result<Option<String>> {
    Ok(db.meta_get(REASON_KEY)?.filter(|s| !s.is_empty()))
}

/// Recovery attempts spent on the divergence on record.
pub fn attempts(db: &Db) -> Result<u64> {
    Ok(db
        .meta_get(ATTEMPTS_KEY)?
        .and_then(|s| s.parse().ok())
        .unwrap_or(0))
}

/// Attempt once, or skip — see the module header for why a changed
/// fingerprint does not buy a second attempt.
pub fn decide(db: &Db, d: &Divergence) -> Result<Decision> {
    match recorded(db)? {
        None => Ok(Decision::Attempt),
        Some(recorded) => {
            let same = recorded == d.fingerprint();
            Ok(Decision::Skip { recorded, same })
        }
    }
}

/// Put `d` on record before the attempt starts, so a crash mid-repair still
/// costs exactly one attempt.
pub fn begin_attempt(db: &Db, d: &Divergence) -> Result<()> {
    db.meta_set(DIVERGENCE_KEY, &d.fingerprint())?;
    db.meta_set(ATTEMPTS_KEY, "1")?;
    db.meta_set(
        REASON_KEY,
        &format!(
            "mismatch at {}, recovery in progress (storage-trie walk)",
            d.block
        ),
    )?;
    // Deliberately NOT stamping the repeat-log clock: the first cycle that
    // meets this divergence again should say so immediately — that line is
    // the one an operator reads — and only then fall silent for a while.
    db.meta_set(LOGGED_AT_KEY, "")?;
    Ok(())
}

/// Record how the one attempt ended. `detail` is a clause, not a sentence: it
/// is spliced into the operator-facing `reason` between the mismatch block and
/// the repair commands.
pub fn record_outcome(db: &Db, d: &Divergence, detail: &str) -> Result<()> {
    db.meta_set(
        REASON_KEY,
        &format!(
            "mismatch at {}, {detail}, operator repair required: \
             enumerate-slots --attribute / rescan --blocks / recut-epochs",
            d.block
        ),
    )?;
    Ok(())
}

/// Forget the divergence on record. Called from `verify_and_capture` the
/// moment a proof says the mirror is complete again.
pub fn clear(db: &Db) -> Result<()> {
    for key in [DIVERGENCE_KEY, ATTEMPTS_KEY, REASON_KEY, LOGGED_AT_KEY] {
        db.meta_set(key, "")?;
    }
    Ok(())
}

/// Rate limit for the "still diverged" line: true at most once per
/// `interval` seconds, and it records the tick it just spent.
pub fn should_log_repeat(db: &Db, now: u64, interval: u64) -> Result<bool> {
    let last: Option<u64> = db
        .meta_get(LOGGED_AT_KEY)?
        .filter(|s| !s.is_empty())
        .and_then(|s| s.parse().ok());
    let due = match last {
        Some(t) => now.saturating_sub(t) >= interval,
        None => true,
    };
    if due {
        db.meta_set(LOGGED_AT_KEY, &now.to_string())?;
    }
    Ok(due)
}

pub fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Is the mirror's own view of the chain head still canonical?
///
/// A verify-root MISMATCH means "the mirror's slot set does not fold to the
/// chain's root". During a reorg that is true and says nothing about
/// completeness: the mirror holds the abandoned branch and the ingest loop is
/// about to roll it back. Spending the one recovery attempt there is worse
/// than useless — the walk enumerates against the NEW branch and re-ingests
/// blocks from it into a mirror that has not rolled back, which makes the
/// canonicity walkback stop at a block that only looks canonical because we
/// just fetched it, and the reorg is then under-rewound. (Seen: acceptance
/// leg g, where a fork landed between the probe and the walk.)
///
/// One header refetch, the same question `Ingestor::detect_reorg` asks, so the
/// two cannot disagree about what a reorg is. Errors are NOT reorgs: an RPC
/// outage must never be read as one.
pub async fn reorg_in_flight(db: &Db, rpc: &RpcClient) -> Result<bool> {
    let (Some(head), Some(stored)) = (
        db.meta_get("head_number")?.and_then(|s| s.parse::<u64>().ok()),
        db.meta_get("head_hash")?,
    ) else {
        return Ok(false);
    };
    match rpc.get_block(crate::rpc::BlockRef::Number(head)).await {
        Ok(h) => Ok(crate::ingest::normalize_hex(&h.block_hash)? != stored),
        Err(e) if RpcClient::is_block_not_found(&e) => Ok(true),
        Err(e) => Err(e.context("checking whether a mismatch is a reorg in flight")),
    }
}

/// What one closure-loop attempt found and did.
#[derive(Debug, Default, Clone)]
pub struct RecoveryReport {
    pub missing_slots: usize,
    pub divergent_slots: usize,
    pub extra_slots: usize,
    pub proof_calls: usize,
    /// Blocks the walk attributed the missing slots to.
    pub blocks: Vec<u64>,
    pub reingested: u64,
    pub elapsed_secs: u64,
}

/// A line every `secs` for as long as the attempt runs, so the log is never
/// silent while ingest is paused. `--progress-secs` sets the cadence; the scan
/// path reads it the same way, and 0 there means "every page", which for a
/// heartbeat has to become a floor of one second rather than a spin.
struct Heartbeat {
    handle: tokio::task::JoinHandle<()>,
}

impl Heartbeat {
    fn start(block: u64, secs: u64, phase: Arc<AtomicU64>) -> Self {
        let period = Duration::from_secs(secs.max(1));
        let handle = tokio::spawn(async move {
            let started = Instant::now();
            loop {
                tokio::time::sleep(period).await;
                tracing::info!(
                    block,
                    elapsed_secs = started.elapsed().as_secs(),
                    phase = PHASES[(phase.load(Ordering::Relaxed) as usize).min(PHASES.len() - 1)],
                    "recovery in progress; ingest resumes as soon as this one attempt ends"
                );
            }
        });
        Self { handle }
    }
}

impl Drop for Heartbeat {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

const PHASES: [&str; 3] = ["trie-walk", "attribute-to-blocks", "re-ingest"];

/// One bounded run of the §4.2 closure loop at `d.block`.
///
/// Identical to what `strk20 enumerate-slots --attribute` does by hand —
/// enumerate the chain's pool slots the mirror does not hold, bisect each
/// cluster to the block that wrote it — followed by `strk20 rescan --blocks`
/// over exactly those blocks. Nothing here widens a window or guesses a range.
pub async fn run_closure(
    db: &mut Db,
    rpc: &RpcClient,
    cfg: &ChainConfig,
    feed_dir: PathBuf,
    chunk_size: u64,
    progress_secs: u64,
    d: &Divergence,
) -> Result<RecoveryReport> {
    let started = Instant::now();
    let phase = Arc::new(AtomicU64::new(0));
    let _beat = Heartbeat::start(d.block, progress_secs, phase.clone());

    let mut report = RecoveryReport::default();
    let blocks = {
        let cutter = Cutter {
            db,
            rpc,
            cfg,
            feed_dir,
        };
        let diff = crate::trie_walk::enumerate_missing_slots(&cutter, d.block)
            .await
            .with_context(|| format!("storage-trie walk at block {}", d.block))?;
        report.missing_slots = diff.missing.len();
        report.divergent_slots = diff.divergent.len();
        report.extra_slots = diff.extra.len();
        report.proof_calls = diff.proof_calls;
        tracing::warn!(
            block = d.block,
            missing_slots = diff.missing.len(),
            divergent_slots = diff.divergent.len(),
            extra_slots = diff.extra.len(),
            proof_calls = diff.proof_calls,
            "storage-trie walk finished: the divergence is enumerated, not guessed at"
        );
        if diff.missing.is_empty() {
            // Nothing to attribute. Either the walk closed clean (the root
            // moved under us between probe and walk) or the divergence is of a
            // shape re-ingesting blocks cannot fix — a slot the mirror holds
            // at the wrong value, or one the chain does not hold at all.
            Vec::new()
        } else {
            phase.store(1, Ordering::Relaxed);
            let slots: Vec<Felt> = diff.missing.iter().map(|(k, _)| *k).collect();
            crate::trie_walk::attribute_to_blocks(&cutter, &slots, cfg.genesis_block, d.block)
                .await
                .context("attributing the missing slots to the blocks that wrote them")?
        }
    };

    report.blocks = blocks.clone();
    if !blocks.is_empty() {
        phase.store(2, Ordering::Relaxed);
        tracing::warn!(
            blocks = ?blocks,
            "re-ingesting exactly the blocks that wrote the missing slots"
        );
        let mut ingestor = Ingestor {
            db,
            rpc,
            cfg,
            chunk_size,
            progress_secs,
        };
        report.reingested = ingestor
            .reingest_blocks(&blocks)
            .await
            .context("re-ingesting the attributed blocks")?;
    }
    report.elapsed_secs = started.elapsed().as_secs();
    Ok(report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn db() -> (tempfile::TempDir, Db) {
        let dir = tempfile::tempdir().unwrap();
        let db = Db::open(&dir.path().join("strk20.db")).unwrap();
        (dir, db)
    }

    fn divergence(block: u64, local: u64, chain: u64) -> Divergence {
        Divergence {
            block,
            local_root: Felt::from(local),
            chain_root: Felt::from(chain),
        }
    }

    /// The identity has to survive a restart, because the whole point of the
    /// guard is that a crash mid-repair does not buy a second attempt. That
    /// means it round-trips through `meta` exactly.
    #[test]
    fn the_divergence_round_trips_through_meta() {
        let (_dir, db) = db();
        let d = divergence(14_448_522, 0xabc, 0xdef);
        begin_attempt(&db, &d).unwrap();
        assert_eq!(recorded(&db).unwrap().as_deref(), Some(d.fingerprint().as_str()));
        assert_eq!(Divergence::parse(&d.fingerprint()), Some(d));
        assert_eq!(mismatch_block(&db).unwrap(), Some(14_448_522));
        assert_eq!(attempts(&db).unwrap(), 1);
    }

    #[test]
    fn a_malformed_fingerprint_is_no_divergence_at_all() {
        for s in ["", "abc", "12", "12:0x1", "12:0x1:0x2:0x3", "12:zz:0x2"] {
            assert_eq!(Divergence::parse(s), None, "{s:?} must not parse");
        }
    }

    /// The defect, as a unit: the same divergence coming back must not buy a
    /// second recovery. On the hosted instance this was every poll cycle, each
    /// one costing tens of minutes of starved ingest.
    #[test]
    fn the_same_divergence_is_skipped_not_re_attempted() {
        let (_dir, db) = db();
        let d = divergence(100, 0x1, 0x2);
        assert_eq!(decide(&db, &d).unwrap(), Decision::Attempt);
        begin_attempt(&db, &d).unwrap();
        record_outcome(&db, &d, "recovery attempted once").unwrap();
        match decide(&db, &d).unwrap() {
            Decision::Skip { same, .. } => assert!(same, "the fingerprint is unchanged"),
            other => panic!("a second attempt on the same divergence: {other:?}"),
        }
        assert_eq!(attempts(&db).unwrap(), 1, "still exactly one attempt");
    }

    /// §7.10: a mutable admin slot can make the bisection predicate
    /// non-monotone, so the reported mismatch block — and with it the
    /// fingerprint — can wander while the underlying hole never moves. A
    /// wandering fingerprint must not be mistaken for progress, or the guard
    /// loops on exactly the case it exists to bound. It is REPORTED as moved
    /// and still skipped.
    #[test]
    fn a_divergence_that_moves_while_unresolved_is_still_skipped() {
        let (_dir, db) = db();
        let first = divergence(100, 0x1, 0x2);
        begin_attempt(&db, &first).unwrap();
        // Next cycle: the frontier advanced, so the probe block and both roots
        // are different numbers for the same unrepaired hole.
        let moved = divergence(137, 0x11, 0x22);
        match decide(&db, &moved).unwrap() {
            Decision::Skip { same, recorded } => {
                assert!(!same, "the identity moved and is reported as moved");
                assert_eq!(recorded, first.fingerprint());
            }
            other => panic!("a moved divergence must not buy another attempt: {other:?}"),
        }
    }

    /// The clear path: a verify-root MATCH — auto-healed or repaired by an
    /// operator — retires the record, and only then is the loop re-armed.
    #[test]
    fn a_match_clears_the_record_and_re_arms_the_loop() {
        let (_dir, db) = db();
        let d = divergence(100, 0x1, 0x2);
        begin_attempt(&db, &d).unwrap();
        record_outcome(&db, &d, "recovery attempted once").unwrap();
        assert!(reason(&db).unwrap().is_some());

        clear(&db).unwrap();
        assert_eq!(recorded(&db).unwrap(), None);
        assert_eq!(mismatch_block(&db).unwrap(), None);
        assert_eq!(reason(&db).unwrap(), None);
        assert_eq!(attempts(&db).unwrap(), 0);

        let later = divergence(9_000, 0x7, 0x8);
        assert_eq!(
            decide(&db, &later).unwrap(),
            Decision::Attempt,
            "a divergence found after a clean verify-root is a new one and gets its attempt"
        );
    }

    /// The line an operator needs has to be findable, which means it is said
    /// once when the divergence first comes back and then not once per poll
    /// cycle for the next hour.
    #[test]
    fn the_repeat_warning_is_rate_limited() {
        let (_dir, db) = db();
        let d = divergence(100, 0x1, 0x2);
        begin_attempt(&db, &d).unwrap();
        let t0 = 1_000_000u64;
        assert!(
            should_log_repeat(&db, t0, REPEAT_LOG_SECS).unwrap(),
            "the first cycle that meets the divergence again must say so"
        );
        assert!(!should_log_repeat(&db, t0 + 1, REPEAT_LOG_SECS).unwrap());
        assert!(!should_log_repeat(&db, t0 + REPEAT_LOG_SECS - 1, REPEAT_LOG_SECS).unwrap());
        assert!(should_log_repeat(&db, t0 + REPEAT_LOG_SECS, REPEAT_LOG_SECS).unwrap());
        // ...and the tick it just spent moves the window forward.
        assert!(!should_log_repeat(&db, t0 + REPEAT_LOG_SECS + 1, REPEAT_LOG_SECS).unwrap());

        // A cleared record re-arms the line too: the next divergence is new
        // information and must not be swallowed by the previous one's window.
        clear(&db).unwrap();
        assert!(should_log_repeat(&db, t0 + REPEAT_LOG_SECS + 2, REPEAT_LOG_SECS).unwrap());
    }

    /// `reason` is what an operator reads on `/health`, so it must name the
    /// block and the three commands that repair it by hand.
    #[test]
    fn the_reason_names_the_block_and_the_repair_commands() {
        let (_dir, db) = db();
        let d = divergence(14_448_522, 0x1, 0x2);
        begin_attempt(&db, &d).unwrap();
        record_outcome(&db, &d, "recovery attempted once, 0 block(s) repaired").unwrap();
        let reason = reason(&db).unwrap().unwrap();
        assert!(reason.contains("mismatch at 14448522"), "{reason}");
        assert!(reason.contains("recovery attempted once"), "{reason}");
        for cmd in ["enumerate-slots --attribute", "rescan --blocks", "recut-epochs"] {
            assert!(reason.contains(cmd), "{reason} must name {cmd}");
        }
    }
}
