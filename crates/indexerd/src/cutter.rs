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
    EpochAnchor, Genesis, Manifest, ManifestEpoch, ManifestHead,
};
use strk20_feed::{felt_hex, payload_sha256};

#[derive(Debug, Clone, Copy)]
pub struct Anchor {
    pub block: u64,
    pub block_hash: Felt,
    pub storage_root: Felt,
    pub class_hash: Felt,
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
        let block_hash = proof
            .global_roots
            .get("block_hash")
            .and_then(|v| v.as_str())
            .map(crate::rpc::parse_felt)
            .transpose()?
            .unwrap_or(Felt::ZERO);
        Ok(Anchor {
            block,
            block_hash,
            storage_root: remote_root,
            class_hash,
        })
    }

    /// Cut every epoch whose range is fully ≤ `l1_accepted`. `frontier` is
    /// the last fully-ingested block. Returns the number of epochs cut.
    pub async fn cut_ready_epochs(&self, l1_accepted: u64, frontier: u64) -> Result<u64> {
        self.ensure_layout()?;
        let mut cut = 0u64;
        let mut next_idx = match self.db.last_epoch()? {
            Some((idx, _, _)) => idx + 1,
            None => self.cfg.first_epoch(),
        };
        let mut verified = false;
        loop {
            let (from, to) = self.cfg.epoch_range(next_idx);
            if to > l1_accepted || to > frontier {
                break;
            }
            // Mandatory completeness check once per cut batch, against the
            // newest block we can both prove and have fully ingested.
            if !verified {
                let vb = l1_accepted.min(frontier);
                match self.verify_root(vb).await {
                    Ok(_) => {}
                    Err(e) if e.to_string().contains("VERIFY-ROOT MISMATCH") => return Err(e),
                    Err(e) => {
                        // Proof unavailable (window, provider) — log and
                        // continue; the anchor below stays best-effort.
                        tracing::warn!(error = %e, "verify-root unavailable; cutting without it");
                    }
                }
                verified = true;
            }
            let prev = self.db.last_epoch()?.map(|(_, h, _)| h);
            let epoch = self.build_epoch(next_idx, prev)?;
            let payload = codec::encode_epoch(&epoch);
            let content_hash = payload_sha256(&payload);
            let compressed = strk20_feed::compress(&payload);
            let zst_hash = payload_sha256(&compressed);
            let file = self.epochs_dir().join(format!("{next_idx:08}.strk20e.zst"));
            atomic_write(&file, &compressed)?;

            // best-effort anchor at the epoch's end block
            let anchor = match self.rpc.get_storage_proof(BlockRef::Number(to), &self.cfg.pool, &[]).await {
                Ok((proof, raw)) => {
                    let leaf = proof.contracts_proof.contract_leaves_data.first().cloned();
                    if let Some(leaf) = leaf {
                        let sidecar = self.epochs_dir().join(format!("{next_idx:08}.anchor.json"));
                        atomic_write(&sidecar, serde_json::to_vec_pretty(&raw)?.as_slice())?;
                        Some(Anchor {
                            block: to,
                            block_hash: proof
                                .global_roots
                                .get("block_hash")
                                .and_then(|v| v.as_str())
                                .map(crate::rpc::parse_felt)
                                .transpose()?
                                .unwrap_or(Felt::ZERO),
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
        Ok(cut)
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
            snapshot: None,
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
