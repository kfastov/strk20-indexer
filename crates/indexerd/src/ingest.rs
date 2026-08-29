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

pub struct Ingestor<'a> {
    pub db: &'a mut Db,
    pub rpc: &'a RpcClient,
    pub cfg: &'a ChainConfig,
    /// getEvents page size; production 1000, tests force small values.
    pub chunk_size: u64,
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
            Some((f, _)) => f,
            None => self.cfg.genesis_block.saturating_sub(1),
        };
        if latest.block_number > frontier {
            let active = self
                .scan_active_blocks(frontier + 1, latest.block_number)
                .await?;
            for (number, _) in &active {
                self.ingest_block(*number).await?;
                self.db.set_ingest_cursor(*number, None)?;
                out.blocks_ingested += 1;
            }
            self.db.set_ingest_cursor(latest.block_number, None)?;
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
    /// None = no reorg.
    async fn detect_reorg(&self) -> Result<Option<u64>> {
        let Some(head_num) = self.db.meta_get("head_number")?.and_then(|s| s.parse().ok())
        else {
            return Ok(None);
        };
        let Some(stored_hash) = self.db.meta_get("head_hash")? else {
            return Ok(None);
        };
        let current = self.rpc.get_block(BlockRef::Number(head_num)).await;
        match current {
            Ok(h) if normalize_hex(&h.block_hash)? == stored_hash => Ok(None),
            Ok(_) | Err(_) => {
                // The stored head is gone. Walk stored active blocks from the
                // top until one is still canonical; the epoch floor is never
                // crossed because epochs are cut at l1-final blocks only.
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
                        _ => continue,
                    }
                }
                Ok(Some(floor))
            }
        }
    }

    /// Pool-active block numbers in [from, to], with their pool events in
    /// emission order (getEvents order within a block is emission order).
    async fn scan_active_blocks(
        &mut self,
        from: u64,
        to: u64,
    ) -> Result<Vec<(u64, Vec<crate::rpc::RpcEvent>)>> {
        let mut by_block: BTreeMap<u64, Vec<crate::rpc::RpcEvent>> = BTreeMap::new();
        let mut continuation: Option<String> = None;
        loop {
            let page = self
                .rpc
                .get_events(
                    &self.cfg.pool,
                    from,
                    BlockRef::Number(to),
                    self.chunk_size,
                    continuation.as_deref(),
                )
                .await?;
            for ev in page.events {
                let Some(bn) = ev.block_number else {
                    continue; // pre-confirmed events carry no block number
                };
                if bn < from || bn > to {
                    continue;
                }
                by_block.entry(bn).or_default().push(ev);
            }
            match page.continuation_token {
                Some(token) => continuation = Some(token),
                None => break,
            }
        }
        Ok(by_block.into_iter().collect())
    }

    /// Fetch and store one pool-active block.
    async fn ingest_block(&mut self, number: u64) -> Result<()> {
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
        diffs.sort_by(|a, b| a.0.to_bytes_be().cmp(&b.0.to_bytes_be()));

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

        // events for this block, in emission order, with tx_index resolved
        // from the header's transaction list
        let tx_index_of = |tx_hash: &str| -> Option<u64> {
            let want = normalize_hex(tx_hash).ok()?;
            header
                .transactions
                .iter()
                .position(|t| normalize_hex(t).map(|h| h == want).unwrap_or(false))
                .map(|i| i as u64)
        };
        // Re-fetch this block's events via the same filter to keep a single
        // source of order (the scan already had them, but re-fetching per
        // block keeps ingest_block self-contained and idempotent).
        let page = self
            .rpc
            .get_events(
                &self.cfg.pool,
                number,
                BlockRef::Number(number),
                self.chunk_size.max(100),
                None,
            )
            .await?;
        let mut events = Vec::with_capacity(page.events.len());
        for (i, ev) in page.events.iter().enumerate() {
            if ev.block_number != Some(number) {
                continue;
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
            events.push(EventRow {
                block: number,
                event_index: i as u64,
                tx_index: tx_index_of(&ev.transaction_hash).unwrap_or(0),
                tx_hash: crate::rpc::parse_felt(&ev.transaction_hash)?,
                keys,
                data,
            });
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
            .insert_block_data(&row, &diffs, &events, replaced.as_ref(), number, None)?;
        Ok(())
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
