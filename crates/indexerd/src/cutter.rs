//! Epoch cutter, verify-root, manifest and head-tail generation (spec §5.5,
//! §5.6, §4.2–§4.4). Epochs are cut only when their whole range is
//! ≤ l1_accepted — immutable by construction.

use crate::config::ChainConfig;
use crate::db::Db;
use crate::rpc::{BlockRef, RpcClient};
use anyhow::{bail, Context, Result};
use starknet_types_core::felt::Felt;
use std::fs;
use std::path::{Path, PathBuf};
use strk20_feed::codec::{self, BlockLine, Epoch, EpochHeader, EventLine, Finality, Footer, Head, HeadHeader};
use strk20_feed::manifest::{
    EpochAnchor, Genesis, Manifest, ManifestEpoch, ManifestHead, ManifestSnapshot,
};
use strk20_feed::snapshot::{self, SnapSlot, Snapshot, SnapshotHeader};
use strk20_feed::{felt_hex, payload_sha256};

/// Retention (§1.4 step 6): snapshots are derived artifacts, deletable and
/// never in the hash chain, but never pruned out from under a client that read
/// the previous manifest moments ago.
pub const SNAPSHOT_KEEP: usize = 2;

#[derive(Debug, Clone, Copy)]
pub struct Anchor {
    pub block: u64,
    pub block_hash: Felt,
    pub storage_root: Felt,
    pub class_hash: Felt,
}

/// Result of a completeness check that was allowed to not happen.
/// `Unavailable` is a statement about the PROVIDER, never about the mirror:
/// it must never latch `verify_root_failed` or degrade health (LIVE-4/6).
/// A real divergence is an `Err` carrying `VERIFY-ROOT MISMATCH`.
#[derive(Debug)]
pub enum VerifyOutcome {
    Verified(Anchor),
    Unavailable(String),
}

pub struct Cutter<'a> {
    pub db: &'a Db,
    pub rpc: &'a RpcClient,
    pub cfg: &'a ChainConfig,
    pub feed_dir: PathBuf,
}

fn atomic_write(path: &Path, bytes: &[u8]) -> Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes).with_context(|| format!("write {}", tmp.display()))?;
    fs::rename(&tmp, path).with_context(|| format!("rename into {}", path.display()))?;
    Ok(())
}

impl<'a> Cutter<'a> {
    pub fn epochs_dir(&self) -> PathBuf {
        self.feed_dir.join("epochs")
    }

    pub fn ensure_layout(&self) -> Result<()> {
        fs::create_dir_all(self.epochs_dir())?;
        fs::create_dir_all(self.feed_dir.join("snapshots"))?;
        let genesis_path = self.feed_dir.join("genesis.json");
        if !genesis_path.exists() {
            let g = Genesis {
                format: "strk20-feed".into(),
                v: codec::FORMAT_VERSION,
                chain_id: self.cfg.chain_id.clone(),
                pool: felt_hex(&self.cfg.pool),
                genesis_block: self.cfg.genesis_block,
                epoch_size: self.cfg.epoch_size,
            };
            atomic_write(&genesis_path, serde_json::to_vec_pretty(&g)?.as_slice())?;
        }
        Ok(())
    }

    /// Build the canonical Epoch struct for `idx` from DB rows — a pure
    /// function of chain data (spec §5.3 determinism guarantee).
    pub fn build_epoch(&self, idx: u64, prev: Option<[u8; 32]>) -> Result<Epoch> {
        let (from, to) = self.cfg.epoch_range(idx);
        let blocks = self.blocks_as_lines(from, to, None)?;
        let footer = footer_for(&blocks, self.db.class_as_of(to)?.unwrap_or(Felt::ZERO));
        Ok(Epoch {
            header: EpochHeader {
                chain_id: self.cfg.chain_id.clone(),
                pool: self.cfg.pool,
                epoch: idx,
                from,
                to,
                prev,
            },
            blocks,
            footer,
        })
    }

    fn blocks_as_lines(
        &self,
        from: u64,
        to: u64,
        finality_from_status: Option<()>,
    ) -> Result<Vec<BlockLine>> {
        let rows = self.db.blocks_in_range(from, to)?;
        let mut out = Vec::with_capacity(rows.len());
        for b in rows {
            let diffs = self.db.diffs_of_block(b.number)?;
            let events = self
                .db
                .events_of_block(b.number)?
                .into_iter()
                .map(|e| EventLine {
                    tx_index: e.tx_index,
                    event_index: e.event_index,
                    tx_hash: e.tx_hash,
                    keys: e.keys,
                    data: e.data,
                })
                .collect();
            out.push(BlockLine {
                number: b.number,
                hash: b.hash,
                parent: b.parent_hash,
                timestamp: b.timestamp,
                diffs,
                events,
                replaced_class: self.db.replaced_class_of_block(b.number)?,
                finality: finality_from_status.map(|_| {
                    if b.l1_accepted {
                        Finality::L1
                    } else {
                        Finality::L2
                    }
                }),
            });
        }
        Ok(out)
    }

    /// Verify-root (spec §5.6): recompute the pool storage MPT root from the
    /// full mirrored slot set as of `block` and compare with the proof served
    /// by the RPC for that block. Returns the anchor on success.
    pub async fn verify_root(&self, block: u64) -> Result<Anchor> {
        let set = self.db.full_slot_set_as_of(block)?;
        let local_root = strk20_feed::mpt::storage_root(&set);
        let (proof, _raw) = self
            .rpc
            .get_storage_proof(BlockRef::Number(block), &self.cfg.pool, &[])
            .await
            .context("getStorageProof for verify-root")?;
        let leaf = proof
            .contracts_proof
            .contract_leaves_data
            .first()
            .ok_or_else(|| anyhow::anyhow!("proof has no contract leaf"))?;
        let remote_root = crate::rpc::parse_felt(&leaf.storage_root)?;
        let class_hash = crate::rpc::parse_felt(&leaf.class_hash)?;
        if local_root != remote_root {
            bail!(
                "VERIFY-ROOT MISMATCH at block {block}: local {} != chain {} — \
                 the mirror is missing writes; refusing to publish. \
                 Recover with a full-range rescan of recent epochs.",
                felt_hex(&local_root),
                felt_hex(&remote_root)
            );
        }
        Ok(Anchor {
            block,
            block_hash: self.anchor_block_hash(&proof.global_roots, block).await,
            storage_root: remote_root,
            class_hash,
        })
    }

    /// The chain block hash to stamp on an anchor. Not every provider puts
    /// `block_hash` in `global_roots` (proxies and non-pathfinder
    /// implementations drop it), and substituting zero would publish an anchor
    /// that every client holding that block reports as a mismatch — a false
    /// divergence alarm manufactured by the publisher. Fall back to the block
    /// header; `Felt::ZERO` means "unknown", and `record_anchor` refuses to
    /// publish it.
    async fn anchor_block_hash(&self, global_roots: &serde_json::Value, block: u64) -> Felt {
        if let Some(h) = global_roots
            .get("block_hash")
            .and_then(|v| v.as_str())
            .and_then(|s| crate::rpc::parse_felt(s).ok())
        {
            return h;
        }
        match self.rpc.get_block(BlockRef::Number(block)).await {
            Ok(header) => crate::rpc::parse_felt(&header.block_hash).unwrap_or(Felt::ZERO),
            Err(e) => {
                tracing::debug!(block, error = %e, "anchor block hash unavailable");
                Felt::ZERO
            }
        }
    }

    /// Verify-root against a block chosen INSIDE the live storage-proof window
    /// (spec addendum A1). Measured on mainnet the proof window is ~1024 blocks
    /// wide while `l1_accepted` lags head by ~5000, so the old
    /// `min(l1_accepted, frontier)` target was outside the window by
    /// construction and the check had never once run. The target is instead
    /// `min(frontier, rpc_head)`: pool slots are write-once, so a root match at
    /// block B subsumes every write below B, which makes a head-side check
    /// strictly stronger for completeness. Finality is a separate concern,
    /// already handled by the epoch floor. Going ABOVE the frontier would be
    /// unsound — the chain root there covers writes we have not ingested.
    ///
    /// There is exactly ONE candidate block, so this is a single attempt and
    /// not a search: the target is pinned from below by what we have ingested
    /// (never above `frontier`, or the chain root covers writes we do not have)
    /// and from above by `rpc_head`. When `head - frontier` exceeds the proof
    /// window — the normal state through a multi-hour backfill — no block
    /// satisfies both and the answer is UNAVAILABLE until the mirror catches
    /// up. Retrying the same block within one batch cannot change that, so it
    /// is left to the next ingest cycle, when the frontier has moved.
    pub async fn verify_root_in_window(&self, frontier: u64) -> Result<VerifyOutcome> {
        let head = match self.rpc.get_block(BlockRef::Latest).await {
            Ok(h) => h.block_number,
            Err(_) => frontier,
        };
        let target = frontier.min(head);
        match self.verify_root(target).await {
            Ok(anchor) => Ok(VerifyOutcome::Verified(anchor)),
            Err(e) if crate::rpc::is_proof_unavailable(&e) => {
                let why = format!("{e:#}");
                tracing::debug!(block = target, head, error = %why, "no proof at target");
                Ok(VerifyOutcome::Unavailable(why))
            }
            Err(e) => Err(e),
        }
    }

    /// Append `anchor` to the published log. The file is rewritten from the DB
    /// so its bytes are a pure function of the anchor set, not of the order
    /// captures happened in.
    pub fn record_anchor(&self, anchor: &Anchor) -> Result<()> {
        if anchor.block_hash == Felt::ZERO {
            // Publishing a zero block hash would make every client that holds
            // this block report a mismatch it cannot possibly resolve.
            tracing::warn!(
                block = anchor.block,
                "anchor has no chain block hash; not publishing it"
            );
            return Ok(());
        }
        self.db.insert_anchor(anchor)?;
        self.db.prune_anchors()?;
        self.write_anchors()
    }

    pub fn write_anchors(&self) -> Result<()> {
        let records = self.db.anchors()?;
        let path = self.feed_dir.join("anchors.ndjson");
        if records.is_empty() {
            // A reorg can empty the table; leaving the old file published would
            // keep serving anchors for a chain that no longer exists.
            if path.exists() {
                fs::remove_file(&path)?;
            }
            return Ok(());
        }
        self.ensure_layout()?;
        atomic_write(&path, &strk20_feed::anchors::encode_anchors(&records)?)
    }

    /// Verify the mirror against the chain at the newest provable block and,
    /// on success, capture the anchor into the published log.
    ///
    /// Runs once per ingest cycle rather than once per cut batch: on mainnet an
    /// epoch is 10 000 blocks, so tying capture to a cut would yield roughly one
    /// anchor per 10 000 blocks, while the whole point of the log (LIVE-5) is to
    /// record anchors OPPORTUNISTICALLY, whenever a block we hold is still
    /// inside the ~1024-block proof window. Skipped when the frontier has not
    /// moved since the last completed probe — the answer cannot change and a
    /// proof call per poll interval is pure waste. A MISMATCH deliberately does
    /// NOT record the probe frontier: the caller's §5.6 rescan must be able to
    /// re-verify at the same frontier before any epoch is cut.
    pub async fn verify_and_capture(&self, frontier: u64) -> Result<()> {
        if frontier == 0 {
            return Ok(());
        }
        let last_probe: Option<u64> = self
            .db
            .meta_get("anchor_probe_frontier")?
            .and_then(|s| s.parse().ok());
        if last_probe == Some(frontier) {
            return Ok(());
        }
        match self.verify_root_in_window(frontier).await {
            Ok(VerifyOutcome::Verified(anchor)) => {
                self.db.meta_set("verify_root_failed", "")?;
                self.record_anchor(&anchor)?;
            }
            Ok(VerifyOutcome::Unavailable(why)) => {
                // A provider capability gap. Never latch verify_root_failed
                // for it: /health would report DEGRADED for a reason unrelated
                // to the mirror.
                tracing::warn!(
                    reason = %why,
                    "verify-root UNAVAILABLE: no provable block inside the window"
                );
            }
            Err(e) if e.to_string().contains("VERIFY-ROOT MISMATCH") => {
                // Surfaced in /health; the caller runs the §5.6 rescan slow
                // path and retries.
                self.db.meta_set("verify_root_failed", "1")?;
                return Err(e);
            }
            Err(e) => {
                // Not an answer about the mirror and not a capability gap
                // either: leave the probe frontier unrecorded so the mandatory
                // check is retried next cycle rather than silently skipped.
                tracing::warn!(error = %e, "verify-root could not run");
                return Ok(());
            }
        }
        self.db
            .meta_set("anchor_probe_frontier", &frontier.to_string())?;
        Ok(())
    }

    /// Cut every epoch whose range is fully ≤ `l1_accepted`. `frontier` is
    /// the last fully-ingested block. Returns the number of epochs cut.
    pub async fn cut_ready_epochs(&self, l1_accepted: u64, frontier: u64) -> Result<u64> {
        self.ensure_layout()?;
        // Mandatory completeness check before anything is published, and the
        // opportunistic anchor capture in the same pass.
        self.verify_and_capture(frontier).await?;
        let mut cut = 0u64;
        let mut next_idx = match self.db.last_epoch()? {
            Some((idx, _, _)) => idx + 1,
            None => self.cfg.first_epoch(),
        };
        loop {
            let (from, to) = self.cfg.epoch_range(next_idx);
            if to > l1_accepted || to > frontier {
                break;
            }
            let prev = self.db.last_epoch()?.map(|(_, h, _)| h);
            let epoch = self.build_epoch(next_idx, prev)?;
            let payload = codec::encode_epoch(&epoch);
            let content_hash = payload_sha256(&payload);
            let compressed = strk20_feed::compress(&payload);
            let zst_hash = payload_sha256(&compressed);
            let file = self.epochs_dir().join(format!("{next_idx:08}.strk20e.zst"));
            atomic_write(&file, &compressed)?;

            // Best-effort anchor at the epoch's end block. In production this
            // is essentially never capturable (the end block is thousands of
            // blocks old by cut time) — it is kept because it still works on
            // short-epoch and rehearsal configs; anchors.ndjson is the real
            // artifact.
            let anchor = match self.rpc.get_storage_proof(BlockRef::Number(to), &self.cfg.pool, &[]).await {
                Ok((proof, raw)) => {
                    let leaf = proof.contracts_proof.contract_leaves_data.first().cloned();
                    if let Some(leaf) = leaf {
                        let sidecar = self.epochs_dir().join(format!("{next_idx:08}.anchor.json"));
                        atomic_write(&sidecar, serde_json::to_vec_pretty(&raw)?.as_slice())?;
                        Some(Anchor {
                            block: to,
                            block_hash: self.anchor_block_hash(&proof.global_roots, to).await,
                            storage_root: crate::rpc::parse_felt(&leaf.storage_root)?,
                            class_hash: crate::rpc::parse_felt(&leaf.class_hash)?,
                        })
                    } else {
                        None
                    }
                }
                Err(e) => {
                    tracing::debug!(epoch = next_idx, error = %e, "anchor unavailable");
                    None
                }
            };
            if let Some(a) = &anchor {
                self.record_anchor(a)?;
            }

            self.db.insert_epoch(
                next_idx,
                from,
                to,
                &content_hash,
                &zst_hash,
                compressed.len() as u64,
                prev.as_ref(),
                anchor.as_ref(),
                self.db
                    .meta_get("head_number")?
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0),
            )?;
            tracing::info!(
                epoch = next_idx,
                from,
                to,
                blocks = epoch.blocks.len(),
                hash = hex::encode(content_hash),
                "epoch cut"
            );
            cut += 1;
            next_idx += 1;
        }
        if cut > 0 {
            self.rewrite_manifest()?;
        }
        // Publication is a CONDITION, not a step of a successful cut batch: the
        // anchor that satisfies the §11.3 gate is captured at head, long after
        // the batch that cut the epoch it grounds. Trying only inside a batch
        // that cut something would publish nothing, ever.
        self.maybe_publish_snapshot()?;
        Ok(cut)
    }

    /// Publish a snapshot at the newest cut epoch's end block when the §11.3
    /// gate is met: `anchors.ndjson` carries a record at some `A >= basis` with
    /// no verified mismatch since.
    ///
    /// This REPLACES §1.4 step 4's basis-block root check, which §11.1 measured
    /// to be unobtainable — the storage-proof window is ~1024 blocks and a
    /// basis block is thousands of blocks old at cut time (0 of 515 epochs in a
    /// completed mainnet backfill carry an anchor). Pool slots are write-once,
    /// so a root match at `A` attests every write at or below `A`, the basis
    /// included.
    pub fn maybe_publish_snapshot(&self) -> Result<()> {
        let Some((epoch, content_hash, basis)) = self.db.last_epoch()? else {
            return Ok(());
        };
        // A retained snapshot is never rewritten, so an already-published epoch
        // is done regardless of what has happened since.
        if self.db.snapshot_rows()?.iter().any(|s| s.e == epoch) {
            return Ok(());
        }
        if self.db.meta_get("verify_root_failed")?.as_deref() == Some("1") {
            return Ok(());
        }
        let Some(anchor_block) = self.db.newest_anchor_block()? else {
            return Ok(());
        };
        if anchor_block < basis {
            return Ok(());
        }

        let rows = self.db.full_slot_set_with_blocks_as_of(basis)?;
        let slots: Vec<SnapSlot> = rows
            .iter()
            .map(|(k, v, w)| SnapSlot {
                k: *k,
                v: *v,
                w: *w,
            })
            .collect();
        let pairs: Vec<(Felt, Felt)> = rows.iter().map(|(k, v, _)| (*k, *v)).collect();
        let snap = Snapshot {
            header: SnapshotHeader {
                v: snapshot::SNAPSHOT_VERSION,
                kind: snapshot::KIND_SNAPSHOT.to_owned(),
                chain_id: self.cfg.chain_id.clone(),
                pool: self.cfg.pool,
                epoch,
                block: basis,
                epoch_hash: hex::encode(content_hash),
                storage_root: strk20_feed::mpt::storage_root(&pairs),
                class: self.db.class_as_of(basis)?.unwrap_or(Felt::ZERO),
            },
            slots,
        };
        let payload = snapshot::encode(&snap);
        let compressed = strk20_feed::compress(&payload);
        let file = snapshot::snapshot_file_name(epoch);
        let entry = ManifestSnapshot {
            e: epoch,
            block: basis,
            epoch_hash: snap.header.epoch_hash.clone(),
            file: file.clone(),
            hash: hex::encode(payload_sha256(&payload)),
            zst: hex::encode(payload_sha256(&compressed)),
            bytes: compressed.len() as u64,
            slots: snap.slots.len() as u64,
            storage_root: felt_hex(&snap.header.storage_root),
        };
        self.ensure_layout()?;
        atomic_write(&self.feed_dir.join(&file), &compressed)?;
        self.db.insert_snapshot(&entry)?;
        self.prune_snapshots()?;
        self.rewrite_manifest()?;
        tracing::info!(
            epoch,
            block = basis,
            anchor_block,
            slots = entry.slots,
            bytes = entry.bytes,
            "snapshot published"
        );
        Ok(())
    }

    /// Keep the newest `SNAPSHOT_KEEP`; delete the rest. A client that read the
    /// previous manifest moments earlier must still be able to download the
    /// file it named, which is the whole reason the number is not 1.
    fn prune_snapshots(&self) -> Result<()> {
        let rows = self.db.snapshot_rows()?;
        if rows.len() <= SNAPSHOT_KEEP {
            return Ok(());
        }
        for old in &rows[..rows.len() - SNAPSHOT_KEEP] {
            let path = self.feed_dir.join(&old.file);
            if path.exists() {
                fs::remove_file(&path)
                    .with_context(|| format!("remove {}", path.display()))?;
            }
            self.db.delete_snapshot(old.e)?;
        }
        Ok(())
    }

    /// Regenerate head.ndjson wholesale (spec §4.4).
    pub fn regen_head(&self) -> Result<()> {
        self.ensure_layout()?;
        let head_number: u64 = self
            .db
            .meta_get("head_number")?
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let head_hash = self
            .db
            .meta_get("head_hash")?
            .map(|s| crate::rpc::parse_felt(&s))
            .transpose()?
            .unwrap_or(Felt::ZERO);
        let l1_accepted: u64 = self
            .db
            .meta_get("l1_accepted_number")?
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let tail_from = match self.db.last_epoch()? {
            Some((_, _, to)) => to + 1,
            None => self.cfg.epoch_range(self.cfg.first_epoch()).0,
        };
        let blocks = self.blocks_as_lines(tail_from, head_number, Some(()))?;
        let footer = footer_for(
            &blocks,
            self.db.class_as_of(head_number)?.unwrap_or(Felt::ZERO),
        );
        let head = Head {
            header: HeadHeader {
                tail_from,
                head: head_number,
                head_hash,
                l1_accepted,
            },
            blocks,
            footer,
        };
        let payload = codec::encode_head(&head);
        atomic_write(&self.feed_dir.join("head.ndjson"), &payload)?;
        self.rewrite_manifest()?;
        Ok(())
    }

    pub fn rewrite_manifest(&self) -> Result<()> {
        let rows = self.db.epoch_rows()?;
        let epochs: Vec<ManifestEpoch> = rows
            .iter()
            .map(|r| ManifestEpoch {
                e: r.idx,
                from: r.from,
                to: r.to,
                hash: hex::encode(r.content_hash),
                zst: hex::encode(r.zst_sha256),
                bytes: r.file_size,
                anchor: r.anchor_block.map(|b| EpochAnchor {
                    block: b,
                    block_hash: felt_hex(&r.anchor_block_hash.unwrap_or(Felt::ZERO)),
                    storage_root: felt_hex(&r.anchor_storage_root.unwrap_or(Felt::ZERO)),
                    class: felt_hex(&r.anchor_class_hash.unwrap_or(Felt::ZERO)),
                }),
            })
            .collect();
        let head_number: u64 = self
            .db
            .meta_get("head_number")?
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let manifest = Manifest {
            v: codec::FORMAT_VERSION,
            chain_id: self.cfg.chain_id.clone(),
            pool: felt_hex(&self.cfg.pool),
            genesis_block: self.cfg.genesis_block,
            epoch_size: self.cfg.epoch_size,
            head: ManifestHead {
                number: head_number,
                hash: self.db.meta_get("head_hash")?.unwrap_or_else(|| "0x0".into()),
                l1_accepted: self
                    .db
                    .meta_get("l1_accepted_number")?
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0),
                class: felt_hex(&self.db.class_as_of(head_number)?.unwrap_or(Felt::ZERO)),
                decode_state: self
                    .db
                    .meta_get("decode_state")?
                    .unwrap_or_else(|| "ok".into()),
            },
            latest_epoch: rows.last().map(|r| r.idx),
            epochs,
            // The newest retained snapshot; `null` until the §11.3 gate has
            // been met once.
            snapshot: self.db.snapshot_rows()?.pop(),
        };
        atomic_write(
            &self.feed_dir.join("manifest.json"),
            serde_json::to_vec_pretty(&manifest)?.as_slice(),
        )?;
        Ok(())
    }
}

fn footer_for(blocks: &[BlockLine], class: Felt) -> Footer {
    Footer {
        blocks: blocks.len() as u64,
        diffs: blocks.iter().map(|b| b.diffs.len() as u64).sum(),
        events: blocks.iter().map(|b| b.events.len() as u64).sum(),
        class,
    }
}
