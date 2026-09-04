//! Ingest pipeline (spec §5). One sequential loop; BACKFILL is FOLLOW with a
//! target. Events-first: `getEvents(pool)` finds active blocks, then one
//! `getStateUpdate` + `getBlockWithTxHashes` per active block, one SQLite
//! transaction per block. Crash-safe: rescanning from the frontier is
//! idempotent (INSERT OR REPLACE everywhere).

use crate::config::ChainConfig;
use crate::db::{BlockRow, Db, EventRow};
use crate::rpc::{BlockHeader, BlockRef, RpcClient};
use anyhow::{bail, Context, Result};
use starknet_types_core::felt::Felt;
use std::collections::BTreeMap;

/// Largest block range the scan asks about in one request. Not a correctness
/// bound — correctness comes from the absence of a continuation token in the
/// answer — just a sane first probe so a 5M-block backfill does not open with
/// a request that can only be refused.
const MAX_SCAN_WINDOW: u64 = 100_000;

/// Blocks a single scan+ingest pass covers before the frontier is checkpointed
/// and the accumulated events are dropped. The scan buffers every event it
/// finds, so without a segment a genesis backfill holds ~120k `RpcEvent`s
/// before a single row lands, and ANY failure — an irreducible window, a
/// transport give-up, a kill — discards every call the scan made and restarts
/// at the old frontier. Segmenting bounds both to one segment.
const SCAN_SEGMENT: u64 = MAX_SCAN_WINDOW;

/// How far behind head a cycle may be and still afford the state-diff sweep
/// that catches pool writes emitting no pool event (`run_cycle` step 5).
///
/// The number is a price, not a policy: the sweep costs one `getStateUpdate`
/// per block, so it is affordable exactly while the mirror is following the
/// chain. A follower is 1–10 blocks behind per poll; 256 leaves room for a
/// restart after a short outage and still bounds one cycle at 256 calls. Above
/// it — a backfill, a long outage — the sweep is skipped and completeness for
/// that stretch is the auditor's job, not the poll loop's.
const TAIL_STATE_DIFF_SPAN: u64 = 256;

/// The only block status that means "irreversible without an L1 reorg". The
/// finality poll asks for this tier by name, so an answer that labels itself
/// anything else is not an answer to the question we asked.
const STATUS_ACCEPTED_ON_L1: &str = "ACCEPTED_ON_L1";

/// Why the finality poll's answer may not be believed, or `None` when it may.
///
/// `getBlockWithTxHashes("l1_accepted")` goes to the same endpoints as every
/// other call, and those endpoints are aggregators: LIVE-8 is precisely that a
/// request can be served by a backend that never saw the state we assume it is
/// answering from. Until this check existed the answer was believed verbatim —
/// `run_cycle` wrote `block_number` straight into `meta.l1_accepted_number`,
/// and `/health`, `/metrics`, `head.ndjson` and `manifest.json` all republish
/// that row, so one wrong answer became the height four public artifacts
/// asserted (#21).
///
/// Three things an L1-accepted height cannot be, each checkable against what
/// the same cycle already holds:
///
/// * a block the answer itself labels unfinalized — the tier is the question;
/// * a block above the chain's own `latest` — nothing is final above head;
/// * a block below a height already recorded as final — L1 finality does not
///   walk backwards, and epochs at or under the recorded height are already
///   published as immutable by construction.
///
/// The remaining window, `(persisted, latest]`, is exactly the range an honest
/// answer lives in; this function does not pretend to rank answers inside it.
fn l1_answer_rejection(
    answer: &BlockHeader,
    latest: u64,
    persisted: Option<u64>,
) -> Option<String> {
    let n = answer.block_number;
    match answer.status.as_deref() {
        Some(status) if status != STATUS_ACCEPTED_ON_L1 => {
            return Some(format!(
                "block {n} is labelled {status}, not {STATUS_ACCEPTED_ON_L1}"
            ))
        }
        _ => {}
    }
    if n > latest {
        return Some(format!("block {n} is above latest {latest}"));
    }
    match persisted {
        Some(p) if n < p => Some(format!(
            "block {n} is below {p}, already recorded as L1-accepted"
        )),
        _ => None,
    }
}

/// Next window length, predicted from what the last one answered: aim at three
/// quarters of a page. Prediction, not dogma — an overshoot comes back with a
/// continuation token and costs exactly one call to halve.
fn next_window_len(len: u64, events: u64, page_cap: u64) -> u64 {
    let ceiling = len.saturating_mul(8).max(1);
    let target = if events == 0 {
        ceiling
    } else {
        len.saturating_mul(page_cap).saturating_mul(3) / events.saturating_mul(4).max(1)
    };
    target.clamp(1, ceiling).min(MAX_SCAN_WINDOW)
}

pub struct Ingestor<'a> {
    pub db: &'a mut Db,
    pub rpc: &'a RpcClient,
    pub cfg: &'a ChainConfig,
    /// getEvents page size; production 1000, tests force small values.
    pub chunk_size: u64,
    /// Minimum seconds between scan progress lines; 0 = every page. A deep
    /// backfill spends hours inside one scan, and used to log nothing at all
    /// between start and the final summary (LIVE-2).
    pub progress_secs: u64,
}

#[derive(Debug, Default)]
pub struct CycleOutcome {
    pub head_changed: bool,
    pub reorged: bool,
    pub blocks_ingested: u64,
    pub head_number: u64,
    pub l1_accepted: u64,
}

impl<'a> Ingestor<'a> {
    /// One full cycle: finality poll, canonicity check, events-first scan,
    /// per-block ingest. Returns what changed so the caller can cut epochs
    /// and regenerate the tail.
    pub async fn run_cycle(&mut self) -> Result<CycleOutcome> {
        let mut out = CycleOutcome::default();

        // 1. finality poll
        let latest = self.rpc.get_block(BlockRef::Latest).await?;
        let l1 = self.rpc.get_block(BlockRef::L1Accepted).await?;
        out.head_number = latest.block_number;
        let persisted_l1: Option<u64> = self
            .db
            .meta_get("l1_accepted_number")?
            .and_then(|s| s.parse().ok());
        match l1_answer_rejection(&l1, latest.block_number, persisted_l1) {
            None => {
                out.l1_accepted = l1.block_number;
                self.db.promote_l1(l1.block_number)?;
                self.db
                    .meta_set("l1_accepted_number", &l1.block_number.to_string())?;
            }
            Some(reason) => {
                // Nothing is written and nothing is promoted: the cycle carries
                // the last height a cycle actually produced (0 on a mirror that
                // has never had one), so `/health` and the feed keep publishing
                // that instead of an answer we can see is not one.
                tracing::warn!(
                    answered = l1.block_number,
                    latest = latest.block_number,
                    persisted = persisted_l1,
                    endpoint = %self.rpc.active_endpoint(),
                    "rejected l1_accepted answer: {reason}; keeping the last persisted height"
                );
                out.l1_accepted = persisted_l1.unwrap_or(0);
            }
        }

        let prev_head_hash = self.db.meta_get("head_hash")?;
        let latest_hash_hex = normalize_hex(&latest.block_hash)?;
        if prev_head_hash.as_deref() != Some(latest_hash_hex.as_str()) {
            out.head_changed = true;
        }

        // 2. canonicity check on the stored frontier state
        if let Some(reorg_ancestor) = self.detect_reorg().await? {
            let removed = self.db.rollback_above(reorg_ancestor)?;
            tracing::warn!(ancestor = reorg_ancestor, removed, "reorg: rolled back");
            out.reorged = true;
            out.head_changed = true;
        }

        // 3+4. events-first scan and per-block ingest
        let frontier = match self.db.ingest_cursor()? {
            Some(f) => f,
            None => self.cfg.genesis_block.saturating_sub(1),
        };
        // Scanned in SEGMENTS, not in one pass over the whole remaining range:
        // the segment bounds both the memory the scan holds and what a failure
        // costs, since the frontier is checkpointed at each segment's end.
        let mut seg_from = frontier + 1;
        while seg_from <= latest.block_number {
            let seg_to = seg_from
                .saturating_add(SCAN_SEGMENT - 1)
                .min(latest.block_number);
            let active = self.scan_active_blocks(seg_from, seg_to).await?;
            let found = active.len();
            // The scan's own answer carries this block's events, so nothing
            // re-asks for them: a second getEvents per active block is both
            // 28,655 wasted calls on a full mainnet backfill and, once it has
            // to page, the LIVE-8 defect all over again.
            for (number, events) in active {
                self.ingest_block(number, Some(events)).await?;
                self.db.set_ingest_cursor(number)?;
                out.blocks_ingested += 1;
            }
            self.db.set_ingest_cursor(seg_to)?;
            tracing::info!(
                segment_from = seg_from,
                segment_to = seg_to,
                scan_to = latest.block_number,
                active_blocks = found,
                "scan segment ingested; frontier checkpointed"
            );
            seg_from = seg_to + 1;
        }

        // 5. The events-first scan is INCOMPLETE, and measurably so: a block can
        //    carry pool storage writes and emit no pool event, and `getEvents`
        //    cannot name such a block, so nothing above ever asks the chain about
        //    it. Measured 2026-09-01 on Sepolia — 23 blocks between 8,271,125 and
        //    14,358,219 write pool storage with zero pool events (8,472,101: 17
        //    writes; 12,715,446: 10; 13,702,347: 20), 221 slots the mirror had
        //    never seen — and reproduced on mainnet at 11,721,848 (7 writes, 0
        //    events). Every one of them is a permanent root divergence, which is
        //    what `verify-root` was reporting on both networks.
        //
        //    The tail closes the hole where it is affordable: one
        //    `getStateUpdate` per block, over the blocks this cycle just moved
        //    past. A live follower moves a handful of blocks per poll, so this is
        //    a handful of calls; a backfill moves millions and is skipped
        //    entirely, because one call per block over 6M blocks is not a poll
        //    interval's worth of work. History therefore still needs the
        //    out-of-band audit (`strk20 rescan`), and this only guarantees that a
        //    mirror already verified at its frontier stays verified.
        let tail_from = frontier + 1;
        if tail_from <= latest.block_number
            && latest.block_number - frontier <= TAIL_STATE_DIFF_SPAN
        {
            let recovered = self.rescan_range(tail_from, latest.block_number).await?;
            if recovered > 0 {
                tracing::info!(
                    from = tail_from,
                    to = latest.block_number,
                    recovered,
                    "tail state-diff sweep ingested block(s) that write pool storage \
                     without emitting a pool event"
                );
                out.blocks_ingested += recovered;
            }
        }

        // update head meta last (a crash before this point just rescans)
        self.db
            .meta_set("head_number", &latest.block_number.to_string())?;
        self.db.meta_set("head_hash", &latest_hash_hex)?;
        self.db.record_seen_head(
            &crate::rpc::parse_felt(&latest.block_hash)?,
            latest.block_number,
        )?;
        // Re-apply L1 promotion to rows ingested this cycle.
        self.db.promote_l1(l1.block_number)?;
        Ok(out)
    }

    /// Highest stored block that is still canonical, if a reorg is detected.
    /// None = no reorg. Transport errors PROPAGATE — an RPC outage must never
    /// be mistaken for a reorg (review finding: detect_reorg on Err).
    async fn detect_reorg(&self) -> Result<Option<u64>> {
        let Some(head_num) = self.db.meta_get("head_number")?.and_then(|s| s.parse().ok())
        else {
            return Ok(None);
        };
        let Some(stored_hash) = self.db.meta_get("head_hash")? else {
            return Ok(None);
        };
        let gone = match self.rpc.get_block(BlockRef::Number(head_num)).await {
            Ok(h) => normalize_hex(&h.block_hash)? != stored_hash,
            Err(e) if RpcClient::is_block_not_found(&e) => true,
            Err(e) => return Err(e.context("canonicity check: head refetch failed")),
        };
        if !gone {
            return Ok(None);
        }
        // The stored head is gone. Walk stored active blocks from the top
        // until one is still canonical; the epoch floor is never crossed
        // because epochs are cut at l1-final blocks only.
        let floor = self.db.last_epoch()?.map(|(_, _, to)| to).unwrap_or(0);
        let stored = self.db.blocks_in_range(floor + 1, head_num)?;
        for b in stored.iter().rev() {
            if b.l1_accepted {
                return Ok(Some(b.number));
            }
            match self.rpc.get_block(BlockRef::Number(b.number)).await {
                Ok(h) if crate::rpc::parse_felt(&h.block_hash)? == b.hash => {
                    return Ok(Some(b.number));
                }
                Ok(_) => continue,
                Err(e) if RpcClient::is_block_not_found(&e) => continue,
                Err(e) => return Err(e.context("canonicity walkback failed")),
            }
        }
        Ok(Some(floor))
    }

    /// Pool-active block numbers in [from, to], with their pool events in
    /// emission order (getEvents order within a block is emission order).
    ///
    /// LIVE-8, the critical one. A `getEvents` continuation token is NODE-LOCAL
    /// state, and the primary endpoint is an AGGREGATOR: the next request
    /// reaches a different backend, which does not reject the token — it
    /// resumes from somewhere else and the events in between are dropped with
    /// no error. Measured on one mainnet range: 13 pages found 2,628 blocks,
    /// 62 pages found 2,608; a full backfill lost 139 blocks and 489 events,
    /// which is what made verify-root report a genuine root mismatch.
    ///
    /// So the scan never presents a token. It subdivides the block range until
    /// EVERY window is answered in a single page with no continuation token and
    /// takes the union: one response carries no cross-request state, so it is
    /// sound under any routing, and a mid-scan endpoint change is harmless
    /// rather than a reason to restart. A window that still cannot be answered
    /// at single-block granularity is a hard error — keeping the first page
    /// would be this very defect, silently.
    async fn scan_active_blocks(
        &mut self,
        from: u64,
        to: u64,
    ) -> Result<Vec<(u64, Vec<crate::rpc::RpcEvent>)>> {
        let mut by_block: BTreeMap<u64, Vec<crate::rpc::RpcEvent>> = BTreeMap::new();
        let interval = std::time::Duration::from_secs(self.progress_secs);
        let mut last_report = std::time::Instant::now();
        let mut events_seen = 0u64;
        let mut calls = 0u64;
        let mut subdivisions = 0u64;
        let mut rerequests = 0u64;
        // How many events the endpoint puts in one FULL page, which can be
        // below what we asked for. Only a page that came back full teaches us
        // this: `chunk_size` is a maximum in the JSON-RPC spec, so a short page
        // carrying a token says the endpoint stopped for some other reason
        // (a scanned-block-range budget is the usual one) and says nothing
        // about how many events fit.
        let requested = self.chunk_size.max(1);
        let mut page_cap = requested;
        let mut cursor = from;
        let mut window = (to.saturating_sub(from) + 1).clamp(1, MAX_SCAN_WINDOW);
        // One re-request of the SAME window is allowed before a short-page
        // token is treated as a reason to subdivide; reset whenever the window
        // moves or changes.
        let mut rerequested = false;
        while cursor <= to {
            let end = cursor.saturating_add(window - 1).min(to);
            let page = self
                .rpc
                .get_events(&self.cfg.pool, cursor, BlockRef::Number(end), requested)
                .await?;
            calls += 1;
            if page.continuation_token.is_some() {
                let served = page.events.len() as u64;
                let full = served >= requested;
                if full {
                    page_cap = page_cap.min(served.max(1));
                } else if !rerequested {
                    // A SHORT page with a token is not evidence about event
                    // density, and letting it clamp `page_cap` would shrink
                    // every later window for the rest of a multi-hour scan
                    // (page_cap never recovers). A fresh single-page request
                    // carries no cross-request state, so asking the same
                    // window again is sound and costs one call.
                    rerequested = true;
                    rerequests += 1;
                    tracing::debug!(
                        from = cursor,
                        to = end,
                        served,
                        requested,
                        "short page carried a continuation token; re-requesting the same window"
                    );
                    continue;
                }
                if end == cursor {
                    // Two different endpoint defects, two different remedies:
                    // saying "raise --chunk-size" to an operator whose
                    // endpoint returned an empty page would be advice that
                    // cannot work.
                    let cause = if full {
                        "the page was filled to the endpoint's own limit, which is below this \
                         block's event count. Raise --chunk-size, or use an endpoint whose page \
                         limit is at least this block's event count."
                    } else {
                        "the page was NOT full and a re-request did not help, so this endpoint \
                         bounds getEvents by something other than event count and cannot answer \
                         a one-block window in one page at all. It must be replaced, not tuned."
                    };
                    bail!(
                        "block {cursor}: this endpoint answered a ONE-BLOCK window with a \
                         continuation token ({served} events returned for a requested \
                         chunk_size of {requested}), so no single-page request can cover this \
                         block and the window is IRREDUCIBLE. Following the token is unsound — \
                         it is node-local state and an aggregator's next backend resumes \
                         elsewhere, silently (LIVE-8) — and keeping the first page would \
                         truncate the block. {cause}"
                    );
                }
                subdivisions += 1;
                window = (end - cursor).div_ceil(2);
                rerequested = false;
                tracing::debug!(
                    from = cursor,
                    to = end,
                    next_window = window,
                    page_cap,
                    "scan window did not fit in one page; subdividing"
                );
                continue;
            }
            let len = end - cursor + 1;
            let mut in_window = 0u64;
            for ev in page.events {
                let Some(bn) = ev.block_number else {
                    continue; // pre-confirmed events carry no block number
                };
                if bn < cursor || bn > end {
                    continue;
                }
                in_window += 1;
                events_seen += 1;
                by_block.entry(bn).or_default().push(ev);
            }
            if last_report.elapsed() >= interval {
                tracing::info!(
                    cursor = end,
                    scan_to = to,
                    blocks_ingested = by_block.len(),
                    events = events_seen,
                    window = len,
                    calls,
                    subdivisions,
                    rerequests,
                    endpoint = %self.rpc.active_endpoint(),
                    "scan progress"
                );
                last_report = std::time::Instant::now();
            }
            cursor = end + 1;
            window = next_window_len(len, in_window, page_cap);
            rerequested = false;
        }
        tracing::info!(
            from,
            to,
            active_blocks = by_block.len(),
            events = events_seen,
            calls,
            subdivisions,
            rerequests,
            "scan complete (single-page windows only; no continuation token presented)"
        );
        Ok(by_block.into_iter().collect())
    }

    /// Fetch and store one pool-active block. `events` is the block's pool
    /// events as the scan already saw them; `None` (the §5.6 rescan path) asks
    /// the endpoint for them in ONE page.
    async fn ingest_block(
        &mut self,
        number: u64,
        events: Option<Vec<crate::rpc::RpcEvent>>,
    ) -> Result<()> {
        let header = self
            .rpc
            .get_block(BlockRef::Number(number))
            .await
            .with_context(|| format!("header of block {number}"))?;
        let update = self
            .rpc
            .get_state_update(number)
            .await
            .with_context(|| format!("state update of block {number}"))?;

        let pool_hex = strk20_feed::felt_hex(&self.cfg.pool);
        let mut diffs: Vec<(Felt, Felt)> = Vec::new();
        for cd in &update.state_diff.storage_diffs {
            if normalize_hex(&cd.address)? == pool_hex {
                for e in &cd.storage_entries {
                    diffs.push((
                        crate::rpc::parse_felt(&e.key)?,
                        crate::rpc::parse_felt(&e.value)?,
                    ));
                }
            }
        }
        diffs.sort_by_key(|a| a.0.to_bytes_be());

        let mut replaced: Option<Felt> = None;
        for rc in &update.state_diff.replaced_classes {
            if normalize_hex(&rc.contract_address)? == pool_hex {
                replaced = Some(crate::rpc::parse_felt(&rc.class_hash)?);
            }
        }
        for dc in &update.state_diff.deployed_contracts {
            if normalize_hex(&dc.address)? == pool_hex {
                replaced = Some(crate::rpc::parse_felt(&dc.class_hash)?);
            }
        }
        if let Some(class) = &replaced {
            if !self.cfg.decoder_map.contains_key(class) {
                tracing::error!(
                    class = %strk20_feed::felt_hex(class),
                    block = number,
                    "UNKNOWN pool class hash: typed decoding degraded from this block; raw ingest continues"
                );
                self.db.meta_set("decode_state", "degraded")?;
                self.db
                    .meta_set("degraded_since_block", &number.to_string())?;
            }
        }

        // Fork-consistency (review finding: three non-atomic RPC calls can
        // straddle a reorg/failover): every artifact of this block must carry
        // the SAME block hash as the header, or the whole ingest is retried.
        let header_hash_hex = normalize_hex(&header.block_hash)?;
        if let Some(su_hash) = &update.block_hash {
            if normalize_hex(su_hash)? != header_hash_hex {
                bail!(
                    "block {number}: state update hash {su_hash} disagrees with header                      {header_hash_hex} (reorg/failover mid-fetch); retrying next cycle"
                );
            }
        }

        // events for this block, in emission order, with tx_index resolved
        // from the header's transaction list.
        let tx_index_of = |tx_hash: &str| -> Option<u64> {
            let want = normalize_hex(tx_hash).ok()?;
            header
                .transactions
                .iter()
                .position(|t| normalize_hex(t).map(|h| h == want).unwrap_or(false))
                .map(|i| i as u64)
        };
        let raw_events = match events {
            Some(evs) => evs,
            None => {
                // LIVE-8 applies to a one-block window exactly as it does to
                // the scan's: a page 2 for this block would be answered by a
                // backend that never issued the token. Ask once, with the
                // largest page we are willing to request, and treat a token as
                // the irreducible case.
                let requested = self.chunk_size.max(1000);
                let page = self
                    .rpc
                    .get_events(&self.cfg.pool, number, BlockRef::Number(number), requested)
                    .await?;
                if page.continuation_token.is_some() {
                    bail!(
                        "block {number}: this endpoint answered a ONE-BLOCK window with a \
                         continuation token ({} events returned for a requested chunk_size \
                         of {requested}), so no single-page request can cover this block and \
                         the window is IRREDUCIBLE. Following the token is unsound (LIVE-8) \
                         and keeping the first page would truncate the block.",
                        page.events.len()
                    );
                }
                page.events
            }
        };
        let mut events = Vec::with_capacity(raw_events.len());
        let mut event_index = 0u64;
        for ev in &raw_events {
            if ev.block_number != Some(number) {
                continue;
            }
            if let Some(eh) = &ev.block_hash {
                if normalize_hex(eh)? != header_hash_hex {
                    bail!(
                        "block {number}: event block hash {eh} disagrees with header                          (reorg/failover mid-fetch); retrying next cycle"
                    );
                }
            }
            let keys = ev
                .keys
                .iter()
                .map(|k| crate::rpc::parse_felt(k))
                .collect::<Result<Vec<_>>>()?;
            let data = ev
                .data
                .iter()
                .map(|d| crate::rpc::parse_felt(d))
                .collect::<Result<Vec<_>>>()?;
            let tx_index = tx_index_of(&ev.transaction_hash).ok_or_else(|| {
                anyhow::anyhow!(
                    "block {number}: event tx {} not in header transactions                      (cross-fork fetch); retrying next cycle",
                    ev.transaction_hash
                )
            })?;
            events.push(EventRow {
                block: number,
                event_index,
                tx_index,
                tx_hash: crate::rpc::parse_felt(&ev.transaction_hash)?,
                keys,
                data,
            });
            event_index += 1;
        }

        let l1_accepted = header.status.as_deref() == Some("ACCEPTED_ON_L1");
        let row = BlockRow {
            number,
            hash: crate::rpc::parse_felt(&header.block_hash)?,
            parent_hash: crate::rpc::parse_felt(&header.parent_hash)?,
            timestamp: header.timestamp,
            l1_accepted,
        };
        self.db
            .insert_block_data(&row, &diffs, &events, replaced.as_ref(), number)?;
        Ok(())
    }
}

/// One block the seeker found the mirror and the chain disagree about.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct CoverageGap {
    pub block: u64,
    /// Pool events the chain has in this block.
    pub chain_events: u64,
    /// Pool events the mirror has stored for it.
    pub mirror_events: u64,
    /// Whether the mirror holds a `blocks` row for it at all.
    pub in_mirror: bool,
}

/// What a seeker pass found: the chain's own block→event-count map for a range,
/// compared against the mirror, block by block.
///
/// The three categories are kept apart on purpose. `missing` is the LIVE-8
/// shape (block 11,263,135 had 6 pool events and 4 slot writes on chain and no
/// row in any of our tables); `undercounted` is what a lost PAGE rather than a
/// lost window looks like, and a check that only compared block presence would
/// certify it as healthy; `overcounted` should never happen and is reported
/// rather than swallowed, because a mirror holding more events than the chain
/// is a different defect that the same walk can see for free.
#[derive(Debug, Default, Clone, serde::Serialize, serde::Deserialize)]
pub struct CoverageReport {
    pub from: u64,
    pub to: u64,
    /// Pool-active blocks the chain has in [from, to].
    pub chain_blocks: u64,
    pub chain_events: u64,
    /// How many of those the mirror holds, and how many of their events.
    pub mirror_blocks: u64,
    pub mirror_events: u64,
    pub missing: Vec<CoverageGap>,
    pub undercounted: Vec<CoverageGap>,
    pub overcounted: Vec<CoverageGap>,
}

impl CoverageReport {
    pub fn is_complete(&self) -> bool {
        self.missing.is_empty() && self.undercounted.is_empty() && self.overcounted.is_empty()
    }

    /// The same report re-read against the mirror after a repair, so what is
    /// printed and written is the state the operator is left in rather than the
    /// state that prompted the repair.
    ///
    /// Only the blocks that WERE gaps are re-read: every other block already
    /// agreed with the chain and nothing in a targeted re-ingest can have
    /// changed it. The chain-side numbers are the ones the seeker measured and
    /// are not re-fetched — this must not become a second scan.
    pub fn refreshed_after_repair(&self, db: &crate::db::Db) -> Result<CoverageReport> {
        let mut out = self.clone();
        let gapped: Vec<CoverageGap> = self
            .missing
            .iter()
            .chain(&self.undercounted)
            .chain(&self.overcounted)
            .copied()
            .collect();
        out.missing.clear();
        out.undercounted.clear();
        out.overcounted.clear();
        let old_blocks = gapped.iter().filter(|g| g.in_mirror).count() as u64;
        let old_events: u64 = gapped.iter().map(|g| g.mirror_events).sum();
        let (mut new_blocks, mut new_events) = (0u64, 0u64);
        for g in &gapped {
            let in_mirror = db.block(g.block)?.is_some();
            let mirror_events = db.events_of_block(g.block)?.len() as u64;
            if in_mirror {
                new_blocks += 1;
            }
            new_events += mirror_events;
            let now = CoverageGap {
                block: g.block,
                chain_events: g.chain_events,
                mirror_events,
                in_mirror,
            };
            if !in_mirror {
                out.missing.push(now);
            } else if mirror_events < g.chain_events {
                out.undercounted.push(now);
            } else if mirror_events > g.chain_events {
                out.overcounted.push(now);
            }
        }
        out.mirror_blocks = out.mirror_blocks.saturating_sub(old_blocks) + new_blocks;
        out.mirror_events = out.mirror_events.saturating_sub(old_events) + new_events;
        Ok(out)
    }

    /// Every block that needs re-ingesting, ascending and deduplicated.
    pub fn repair_blocks(&self) -> Vec<u64> {
        let mut blocks: Vec<u64> = self
            .missing
            .iter()
            .chain(&self.undercounted)
            .chain(&self.overcounted)
            .map(|g| g.block)
            .collect();
        blocks.sort_unstable();
        blocks.dedup();
        blocks
    }
}

impl<'a> Ingestor<'a> {
    /// The SEEKER PASS: walk `[from, to]` with the same sound subdivision scan
    /// the ingest uses, `getEvents` only, and report where the mirror disagrees
    /// with the chain.
    ///
    /// This is the cheap half of a backfill — the expensive half is one
    /// `getStateUpdate` plus one `getBlockWithTxHashes` per active block, and
    /// none of that happens here — which is the whole point: a full mainnet
    /// re-backfill costs ~70 minutes to repair 139 blocks, and this pass is the
    /// part of it that can find them.
    ///
    /// It MUST be the subdivision scan and not a paging one. A continuation
    /// token is node-local state and every endpoint we use is an aggregator
    /// (LIVE-8), so a paging seeker would skip blocks exactly the way the
    /// backfill did — and then report the resulting hole as "no gaps found",
    /// certifying the very loss it was run to find. Scanning in segments
    /// bounds the events held in memory the same way the ingest loop does.
    pub async fn audit_coverage(&mut self, from: u64, to: u64) -> Result<CoverageReport> {
        let mut report = CoverageReport {
            from,
            to,
            ..Default::default()
        };
        if from > to {
            return Ok(report);
        }
        let mut seg_from = from;
        while seg_from <= to {
            let seg_to = seg_from.saturating_add(SCAN_SEGMENT - 1).min(to);
            let active = self.scan_active_blocks(seg_from, seg_to).await?;
            for (number, events) in active {
                let chain_events = events.len() as u64;
                report.chain_blocks += 1;
                report.chain_events += chain_events;
                let in_mirror = self.db.block(number)?.is_some();
                let mirror_events = self.db.events_of_block(number)?.len() as u64;
                if in_mirror {
                    report.mirror_blocks += 1;
                }
                report.mirror_events += mirror_events;
                let gap = CoverageGap {
                    block: number,
                    chain_events,
                    mirror_events,
                    in_mirror,
                };
                if !in_mirror {
                    report.missing.push(gap);
                } else if mirror_events < chain_events {
                    report.undercounted.push(gap);
                } else if mirror_events > chain_events {
                    report.overcounted.push(gap);
                }
            }
            tracing::info!(
                segment_from = seg_from,
                segment_to = seg_to,
                audit_to = to,
                chain_blocks = report.chain_blocks,
                chain_events = report.chain_events,
                missing = report.missing.len(),
                undercounted = report.undercounted.len(),
                "coverage audit segment complete"
            );
            seg_from = seg_to + 1;
        }
        Ok(report)
    }

    /// Re-ingest exactly the blocks a seeker pass named, into the existing DB.
    ///
    /// Each block goes through the ordinary per-block ingest path — header,
    /// state update, one single-page `getEvents` — so a repaired block is
    /// byte-for-byte what a backfill would have stored, storage writes
    /// included. The mainnet hole was 6 events AND 4 slot writes per block; a
    /// repair that recovered only events would leave the storage root wrong and
    /// verify-root would still mismatch.
    ///
    /// The ingest cursor is never pulled backwards by this: `insert_block_data`
    /// only ever advances it.
    pub async fn reingest_blocks(&mut self, blocks: &[u64]) -> Result<u64> {
        let mut repaired = 0u64;
        for (i, number) in blocks.iter().enumerate() {
            self.ingest_block(*number, None)
                .await
                .with_context(|| format!("re-ingest of block {number}"))?;
            repaired += 1;
            tracing::info!(
                block = number,
                done = i + 1,
                total = blocks.len(),
                "block re-ingested"
            );
        }
        Ok(repaired)
    }

    /// Verify-root recovery slow path (spec §5.6): re-ingest EVERY block in
    /// [from, to] straight from per-block state updates — not events-first —
    /// so a pool write that rode a block with no pool event is recovered.
    pub async fn rescan_range(&mut self, from: u64, to: u64) -> Result<u64> {
        let mut recovered = 0u64;
        for number in from..=to {
            let update = self.rpc.get_state_update(number).await?;
            let pool_hex = strk20_feed::felt_hex(&self.cfg.pool);
            let touches_pool = update
                .state_diff
                .storage_diffs
                .iter()
                .any(|cd| normalize_hex(&cd.address).map(|a| a == pool_hex).unwrap_or(false))
                || update
                    .state_diff
                    .replaced_classes
                    .iter()
                    .any(|rc| normalize_hex(&rc.contract_address).map(|a| a == pool_hex).unwrap_or(false))
                || update
                    .state_diff
                    .deployed_contracts
                    .iter()
                    .any(|dc| normalize_hex(&dc.address).map(|a| a == pool_hex).unwrap_or(false));
            if touches_pool {
                self.ingest_block(number, None).await?;
                recovered += 1;
            }
        }
        Ok(recovered)
    }
}

/// Canonical minimal-hex form for address comparison.
pub fn normalize_hex(s: &str) -> Result<String> {
    let f = Felt::from_hex(s).map_err(|_| anyhow::anyhow!("bad felt {s:?}"))?;
    Ok(strk20_feed::felt_hex(&f))
}

/// Verify chain identity at startup (spec §5.1 INIT).
pub async fn init_checks(db: &Db, rpc: &RpcClient, cfg: &ChainConfig) -> Result<()> {
    let chain_id = rpc.chain_id().await?;
    if chain_id != cfg.chain_id {
        bail!(
            "rpc chain id {chain_id:?} does not match configured {:?}",
            cfg.chain_id
        );
    }
    match db.meta_get("chain_id")? {
        Some(stored) if stored != cfg.chain_id => {
            bail!("db was built for chain {stored:?}, configured {:?}", cfg.chain_id)
        }
        None => {
            db.meta_set("chain_id", &cfg.chain_id)?;
            db.meta_set("pool_address", &strk20_feed::felt_hex(&cfg.pool))?;
            db.meta_set("genesis_block", &cfg.genesis_block.to_string())?;
            db.meta_set("epoch_size", &cfg.epoch_size.to_string())?;
            db.meta_set("schema_version", &crate::db::SCHEMA_VERSION.to_string())?;
            db.meta_set("decode_state", "ok")?;
        }
        Some(_) => {
            let stored_pool = db.meta_get("pool_address")?.unwrap_or_default();
            if stored_pool != strk20_feed::felt_hex(&cfg.pool) {
                bail!("db was built for pool {stored_pool}, configured {}", strk20_feed::felt_hex(&cfg.pool));
            }
        }
    }
    // Recompute decode_state from class_history against the CURRENT decoder
    // map: this is the recovery path after an operator adds a class via
    // --allow-class (spec §5.7).
    {
        let mut stmt = db
            .conn
            .prepare("SELECT block, class_hash FROM class_history ORDER BY block")?;
        let rows = stmt
            .query_map([], |r| {
                Ok((r.get::<_, i64>(0)? as u64, r.get::<_, Vec<u8>>(1)?))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        let mut degraded_since: Option<u64> = None;
        for (block, class_blob) in rows {
            let class = crate::db::blob_felt(&class_blob);
            if !cfg.decoder_map.contains_key(&class) {
                degraded_since = Some(degraded_since.map_or(block, |b| b.min(block)));
            }
        }
        match degraded_since {
            Some(b) => {
                db.meta_set("decode_state", "degraded")?;
                db.meta_set("degraded_since_block", &b.to_string())?;
            }
            None => {
                db.meta_set("decode_state", "ok")?;
            }
        }
    }
    // current class sanity: warn (not fail) when the live class is unknown
    if let Ok(class) = rpc.get_class_hash_at(BlockRef::Latest, &cfg.pool).await {
        if !cfg.decoder_map.contains_key(&class) {
            tracing::warn!(
                class = %strk20_feed::felt_hex(&class),
                "live pool class is not in the decoder map"
            );
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ tests

#[cfg(test)]
mod tests {
    use super::*;

    /// An answer shaped like the one `getBlockWithTxHashes` returns. `status`
    /// is `Option` on the wire and stays optional here: an endpoint that omits
    /// it has told us nothing about the tier, which is not the same as telling
    /// us the wrong tier.
    fn answer(number: u64, status: Option<&str>) -> BlockHeader {
        BlockHeader {
            block_number: number,
            block_hash: "0x1".into(),
            parent_hash: "0x0".into(),
            timestamp: 1_700_000_000,
            status: status.map(str::to_owned),
            new_root: None,
            transactions: vec![],
        }
    }

    /// #21, as observed: `/mainnet/health` published 7,457,223 while the feed
    /// head was 14,262,623 — a height ~6.8M blocks under one already recorded
    /// as final, and under the pool's own genesis block. No mirror state can
    /// produce that number; an endpoint's answer can, and used to be written
    /// into `meta.l1_accepted_number` unread.
    #[test]
    fn an_answer_below_the_recorded_height_is_rejected() {
        let reason = l1_answer_rejection(
            &answer(7_457_223, Some(STATUS_ACCEPTED_ON_L1)),
            14_262_623,
            Some(14_258_250),
        )
        .expect("an L1-accepted height that walks backwards must be rejected");
        assert!(reason.contains("7457223"), "{reason}");
        assert!(reason.contains("14258250"), "{reason}");
    }

    /// The dangerous direction: a height above `latest` would let
    /// `cut_ready_epochs` publish an epoch as immutable over blocks that are
    /// still revocable en masse (docs/research-answers.md Q12).
    #[test]
    fn an_answer_above_latest_is_rejected() {
        assert!(l1_answer_rejection(
            &answer(14_300_000, Some(STATUS_ACCEPTED_ON_L1)),
            14_262_623,
            Some(14_258_250),
        )
        .is_some());
    }

    /// The tag names a finality tier, so the tier the answer reports IS the
    /// binding check: an aggregator that resolves `"l1_accepted"` as `latest`
    /// hands back a real, current, wrong block, and only `status` shows it.
    #[test]
    fn an_answer_that_is_not_l1_accepted_is_rejected() {
        assert!(l1_answer_rejection(
            &answer(14_262_623, Some("ACCEPTED_ON_L2")),
            14_262_623,
            Some(14_258_250),
        )
        .is_some());
        assert!(l1_answer_rejection(
            &answer(14_262_623, Some("PRE_CONFIRMED")),
            14_262_623,
            None
        )
        .is_some());
    }

    /// A mirror with no recorded height has nothing to compare against, so the
    /// only bounds left are the tier and `latest`. A plausible answer is still
    /// accepted — the guard must not deadlock a first cycle.
    #[test]
    fn a_first_answer_is_accepted_on_its_own_terms() {
        assert_eq!(
            l1_answer_rejection(
                &answer(14_258_250, Some(STATUS_ACCEPTED_ON_L1)),
                14_262_623,
                None
            ),
            None
        );
        // Below genesis is NOT a rejection reason on its own: a fresh mirror
        // holds no evidence about L1 that would rank one height over another,
        // and inventing one here would be the same class of claim as #21.
        assert_eq!(
            l1_answer_rejection(&answer(1, Some(STATUS_ACCEPTED_ON_L1)), 14_262_623, None),
            None
        );
    }

    /// Normal operation: finality advances, and standing still is normal too —
    /// `l1_accepted` moves once every few hours while the poll runs every cycle.
    #[test]
    fn an_advancing_or_unchanged_height_is_accepted() {
        for n in [14_258_250, 14_258_400] {
            assert_eq!(
                l1_answer_rejection(
                    &answer(n, Some(STATUS_ACCEPTED_ON_L1)),
                    14_262_623,
                    Some(14_258_250)
                ),
                None,
                "height {n}"
            );
        }
    }

    /// An endpoint that sends no `status` is not evidence of a wrong tier, so
    /// the range checks carry the whole decision on their own.
    #[test]
    fn a_missing_status_leaves_the_range_checks_deciding() {
        assert_eq!(
            l1_answer_rejection(&answer(14_258_400, None), 14_262_623, Some(14_258_250)),
            None
        );
        assert!(
            l1_answer_rejection(&answer(7_457_223, None), 14_262_623, Some(14_258_250)).is_some()
        );
    }
}
