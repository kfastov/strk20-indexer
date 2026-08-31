//! Ingest pipeline (spec §5). One sequential loop; BACKFILL is FOLLOW with a
//! target. Events-first: `getEvents(pool)` finds active blocks, then one
//! `getStateUpdate` + `getBlockWithTxHashes` per active block, one SQLite
//! transaction per block. Crash-safe: rescanning from the frontier is
//! idempotent (INSERT OR REPLACE everywhere).

use crate::config::ChainConfig;
use crate::db::{BlockRow, Db, EventRow};
use crate::rpc::{BlockRef, RpcClient};
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
        out.l1_accepted = l1.block_number;
        self.db.promote_l1(l1.block_number)?;
        self.db
            .meta_set("l1_accepted_number", &l1.block_number.to_string())?;

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

impl<'a> Ingestor<'a> {
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
