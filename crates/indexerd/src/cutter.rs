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

/// Cycles a snapshot's basis-block proof is attempted over before the snapshot
/// settles for the §11.3 fallback grounding. Each attempt already spends the
/// per-endpoint refusal budget inside `get_storage_proof`; this is the OUTER
/// budget, and it is bounded because an endpoint that implements no proofs at
/// any height would otherwise be asked forever.
pub const BASIS_PROBE_ATTEMPTS: u64 = 5;

/// Epoch the basis-probe attempt counter belongs to. Stored separately so a new
/// epoch's budget starts fresh without a migration.
const PROBE_EPOCH_KEY: &str = "snapshot_basis_probe_epoch";
const PROBE_ATTEMPTS_KEY: &str = "snapshot_basis_probe_attempts";

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

    /// A storage proof BOUND to the chain (consumer-path.md §12 B2), together
    /// with the raw response and the block hash it was bound to.
    ///
    /// The proof pool is anonymous and load-balanced, and §12 B1 answers a
    /// refusal by asking again. Retry-until-success is only distinguishable
    /// from "accept whichever answer we liked" if the answer is tied to the
    /// block we asked about, so every accepted proof's
    /// `global_roots.block_hash` is compared with the block header's own hash
    /// BEFORE any `storage_root` from it is believed. A proof that carries no
    /// block hash at all cannot be bound, and that is a hard error on the first
    /// answer — no re-read of the chain can supply a field the proof does not
    /// have.
    ///
    /// A DISAGREEMENT is different, and the difference matters because the
    /// target is deliberately near head (see `verify_root_at_target`), where
    /// the block hash for a number legitimately changes: two hashes for one
    /// block number are ordinary reorg behaviour, not evidence that the pool
    /// lied. The two calls are independent and independently routed, so a
    /// single disagreement is re-tested — proof and header both re-fetched —
    /// and only a disagreement that SURVIVES that is `PROOF_NOT_BOUND`. This
    /// channel has to stay quiet to be believed: it is the one alarm that
    /// means "the endpoint answered with a proof about something else".
    pub async fn bound_proof(
        &self,
        block: u64,
    ) -> Result<(crate::rpc::StorageProof, serde_json::Value)> {
        const BINDING_ATTEMPTS: usize = 2;
        let mut last: Option<(Felt, Felt)> = None;
        for attempt in 0..BINDING_ATTEMPTS {
            // Proof first: an endpoint that cannot serve one answers fast, and
            // there is nothing to bind, so the header fetch is never spent
            // on it.
            let (proof, raw) = self
                .rpc
                .get_storage_proof(BlockRef::Number(block), &self.cfg.pool, &[])
                .await?;
            let header = self
                .rpc
                .get_block(BlockRef::Number(block))
                .await
                .with_context(|| format!("chain binding: header of block {block}"))?;
            let chain_hash = crate::rpc::parse_felt(&header.block_hash)?;
            let claimed = proof
                .global_roots
                .get("block_hash")
                .and_then(|v| v.as_str())
                .map(crate::rpc::parse_felt)
                .transpose()?;
            match claimed {
                Some(h) if h == chain_hash => return Ok((proof, raw)),
                Some(h) => {
                    last = Some((h, chain_hash));
                    if attempt + 1 < BINDING_ATTEMPTS {
                        tracing::warn!(
                            block,
                            proof_block_hash = %felt_hex(&h),
                            header_block_hash = %felt_hex(&chain_hash),
                            "storage proof and block header disagree on this block's hash; \
                             re-fetching both before calling it a lie (a reorg between two \
                             independent calls looks identical at this depth)"
                        );
                    }
                }
                None => bail!(
                    "{}: the proof for block {block} carries no global_roots.block_hash, so it \
                     cannot be bound to the chain and its storage_root must not be believed \
                     (§12 B2).",
                    crate::rpc::PROOF_NOT_BOUND
                ),
            }
        }
        let (claimed, chain_hash) = last.expect("a disagreement was recorded");
        bail!(
            "{}: the proof's global_roots.block_hash {} is not this block's hash {} \
             (starknet_getBlockWithTxHashes), on {BINDING_ATTEMPTS} independent fetches of \
             both. It is not a proof about block {block} at all, so its storage_root must \
             not be believed (§12 B2).",
            crate::rpc::PROOF_NOT_BOUND,
            felt_hex(&claimed),
            felt_hex(&chain_hash)
        )
    }

    /// Verify-root (spec §5.6): recompute the pool storage MPT root from the
    /// full mirrored slot set as of `block` and compare with the proof served
    /// by the RPC for that block. Returns the anchor on success.
    pub async fn verify_root(&self, block: u64) -> Result<Anchor> {
        let set = self.db.full_slot_set_as_of(block)?;
        let local_root = strk20_feed::mpt::storage_root(&set);
        let (proof, _raw) = self
            .bound_proof(block)
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
            // §12 B2 already proved this equals the chain's header hash.
            block_hash: anchor_block_hash(&proof)?,
            storage_root: remote_root,
            class_hash,
        })
    }

    /// Verify-root at `min(frontier, rpc_head)`.
    ///
    /// The target is NOT chosen to dodge a proof window: proof-window.md §3
    /// retracts that window — it was a bisection over a nondeterministic
    /// predicate, and deep proofs answer for any block once `get_storage_proof`
    /// retries (§12 B1). The target is chosen because pool slots are
    /// write-once, so a root match at block B attests every write at or below
    /// B, and the newest block we hold is therefore the strongest single check
    /// available. Finality is a separate concern, already handled by the epoch
    /// floor. Going ABOVE the frontier would be unsound: the chain root there
    /// covers writes we have not ingested.
    ///
    /// UNAVAILABLE survives the retraction, with a narrower meaning: not "this
    /// block is too old to prove" but "every endpoint we hold spent its whole
    /// retry budget refusing", which on a proof-less provider (publicnode
    /// implements none at any height) is the permanent answer. It is a
    /// statement about the PROVIDER and never about the mirror.
    pub async fn verify_root_at_target(&self, frontier: u64) -> Result<VerifyOutcome> {
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
    /// record anchors OPPORTUNISTICALLY, at whatever block the mirror has just
    /// reached. Skipped when the frontier has not
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
        match self.verify_root_at_target(frontier).await {
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
                    "verify-root UNAVAILABLE: every endpoint spent its proof retry budget"
                );
            }
            Err(e) if e.to_string().contains("VERIFY-ROOT MISMATCH") => {
                // Surfaced in /health; the caller runs the §5.6 rescan slow
                // path and retries.
                self.db.meta_set("verify_root_failed", "1")?;
                return Err(e);
            }
            // §12 B2: an endpoint that ANSWERS with a proof belonging to some
            // other block has not had a capability gap. Swallowing it as one
            // would hide a lie behind LIVE-6, so it halts the batch loudly —
            // but it says nothing about the mirror, so it does not latch
            // verify_root_failed either.
            Err(e) if crate::rpc::is_proof_unbound(&e) => return Err(e),
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

            // Anchor at the epoch's end block. §12 point 2: a block of our
            // choosing IS provable — the "0 of 515 epochs carry one" result
            // came from single attempts against an aggregator, and
            // `get_storage_proof` now retries — so this is no longer a
            // rehearsal-only path. Still tolerant of an endpoint that cannot
            // serve proofs at all (a capability gap is never a data defect),
            // but a proof that cannot be BOUND to the block halts the batch.
            let anchor = match self.bound_proof(to).await {
                Ok((proof, raw)) => {
                    let leaf = proof.contracts_proof.contract_leaves_data.first().cloned();
                    if let Some(leaf) = leaf {
                        let sidecar = self.epochs_dir().join(format!("{next_idx:08}.anchor.json"));
                        atomic_write(&sidecar, serde_json::to_vec_pretty(&raw)?.as_slice())?;
                        Some(Anchor {
                            block: to,
                            block_hash: anchor_block_hash(&proof)?,
                            storage_root: crate::rpc::parse_felt(&leaf.storage_root)?,
                            class_hash: crate::rpc::parse_felt(&leaf.class_hash)?,
                        })
                    } else {
                        None
                    }
                }
                Err(e) if crate::rpc::is_proof_unbound(&e) => return Err(e),
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
        self.maybe_publish_snapshot().await?;
        Ok(cut)
    }

    /// Publish a snapshot at the newest cut epoch's end block.
    ///
    /// §12 B4 gives this two groundings, in order of strength:
    ///
    /// 1. **basis anchor** (§1.3, §1.4 step 4, reinstated) — a chain-bound
    ///    storage proof at the basis block ITSELF, published as the sidecar
    ///    `snapshots/{e:08}.anchor.json`. §11.1 declared this unobtainable on a
    ///    bisection over a nondeterministic predicate; retried, deep proofs
    ///    answer for any block, so it is the primary grounding again. Primary
    ///    against a mirror that is WRONG — it is the only check that speaks
    ///    about the basis block itself — and not against a publisher that is
    ///    dishonest, which is what (2) is for.
    /// 2. **reachability** (§11.3) — `anchors.ndjson` carries a record at some
    ///    `A >= basis` with no verified mismatch since. Pool slots are
    ///    write-once, so a root match at `A` attests every write at or below
    ///    `A`, the basis included. Kept, not deleted: it also validates the
    ///    intervening epochs and it is the only check that catches an
    ///    internally consistent forged snapshot.
    ///
    /// Which one was used is published in the manifest rather than left for a
    /// client to infer from a missing field.
    pub async fn maybe_publish_snapshot(&self) -> Result<()> {
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
        // Grounding 1 (§12 point 1): the proof at the basis block itself.
        //
        // The probe is BUDGETED PER EPOCH, and the budget is spent only by an
        // attempt that actually happened and actually failed. Two separate
        // reasons for that shape:
        //
        //  - It must be more than one attempt. A refusal is per-call routing
        //    luck, not a property of the block (§12 B1), so a single
        //    unsuccessful group of retries is a coin the snapshot's primary
        //    grounding should not be lost on. `PROOF_RETRIES` covers the
        //    within-call odds; this covers the rest across cycles.
        //  - The counter must be written AFTER the answer. Written before, a
        //    mismatch bail would leave the probe already recorded as spent, and
        //    the very next call would fall back to reachability and publish the
        //    slot set the chain had just contradicted — the error would BE the
        //    skip it was written to prevent.
        let probe_epoch = self
            .db
            .meta_get(PROBE_EPOCH_KEY)?
            .and_then(|s| s.parse::<u64>().ok());
        let spent = if probe_epoch == Some(epoch) {
            self.db
                .meta_get(PROBE_ATTEMPTS_KEY)?
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0)
        } else {
            0
        };
        let basis_proof = if spent >= BASIS_PROBE_ATTEMPTS {
            None
        } else {
            let outcome = match self.bound_proof(basis).await {
                // A bound proof with no contract leaf is an answer we cannot
                // use, and it counts against the budget exactly like a refusal:
                // an endpoint that keeps answering that way must not be asked
                // once per poll forever.
                Ok((proof, raw)) => proof
                    .contracts_proof
                    .contract_leaves_data
                    .first()
                    .cloned()
                    .map(|leaf| (leaf, proof, raw))
                    .ok_or_else(|| {
                        anyhow::anyhow!("the proof for block {basis} carries no contract leaf")
                    }),
                Err(e) if crate::rpc::is_proof_unbound(&e) => return Err(e),
                Err(e) => Err(e),
            };
            match outcome {
                Ok(got) => Some(got),
                Err(e) => {
                    self.db.meta_set(PROBE_EPOCH_KEY, &epoch.to_string())?;
                    self.db
                        .meta_set(PROBE_ATTEMPTS_KEY, &(spent + 1).to_string())?;
                    tracing::info!(
                        epoch, block = basis, attempt = spent + 1,
                        budget = BASIS_PROBE_ATTEMPTS, error = %format!("{e:#}"),
                        "no basis-block proof for this snapshot yet; falling back to the \
                         §11.3 reachability grounding for now"
                    );
                    None
                }
            }
        };

        let mut anchor: Option<Anchor> = None;
        if let Some((leaf, proof, _raw)) = &basis_proof {
            let chain_root = crate::rpc::parse_felt(&leaf.storage_root)?;
            if chain_root != snap.header.storage_root {
                // The chain disagrees with the slot set this snapshot would
                // carry. Two things have to happen, and neither is enough
                // alone: the failure is LATCHED, so the §11.3 fallback cannot
                // publish this slot set on a later cycle while the divergence
                // stands, and it is an ERROR rather than a skip, so the §5.6
                // recovery path is entered and /health goes DEGRADED. The latch
                // is cleared only by a verify-root that passes.
                self.db.meta_set("verify_root_failed", "1")?;
                bail!(
                    "VERIFY-ROOT MISMATCH at block {basis}: the snapshot's slot set folds \
                     to {} but the chain's proof for that block says {} — refusing to \
                     publish a snapshot for epoch {epoch}. \
                     Recover with a full-range rescan of recent epochs.",
                    felt_hex(&snap.header.storage_root),
                    felt_hex(&chain_root)
                );
            }
            anchor = Some(Anchor {
                block: basis,
                block_hash: anchor_block_hash(proof)?,
                storage_root: chain_root,
                class_hash: crate::rpc::parse_felt(&leaf.class_hash)?,
            });
        }

        // Grounding 2 (§11.3), required only when grounding 1 was unobtainable.
        if anchor.is_none() {
            let Some(anchor_block) = self.db.newest_anchor_block()? else {
                return Ok(());
            };
            if anchor_block < basis {
                return Ok(());
            }
        }

        let payload = snapshot::encode(&snap);
        let compressed = strk20_feed::compress(&payload);
        let file = snapshot::snapshot_file_name(epoch);
        let grounding = match &anchor {
            Some(_) => strk20_feed::manifest::GROUNDING_BASIS_ANCHOR,
            None => strk20_feed::manifest::GROUNDING_REACHABILITY,
        };
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
            anchor: anchor.as_ref().map(|a| EpochAnchor {
                block: a.block,
                block_hash: felt_hex(&a.block_hash),
                storage_root: felt_hex(&a.storage_root),
                class: felt_hex(&a.class_hash),
            }),
            grounding: grounding.to_owned(),
        };
        self.ensure_layout()?;
        atomic_write(&self.feed_dir.join(&file), &compressed)?;
        // The sidecar is the provider's stored response, published verbatim.
        // What that buys, exactly: a client can check the manifest's anchor
        // against the proof it claims to come from, and against the slot set
        // the snapshot carries (client/store.rs `check_basis_anchor`), so the
        // three cannot disagree unnoticed. What it does NOT buy: any offline
        // strength against the publisher itself. Nothing in the feed binds
        // `global_roots` to a chain a client independently knows, so a
        // publisher that forges the slot set and the sidecar together is
        // consistent — that adversary is caught by the §11.3 reachability walk
        // (still run, on every cold start) and by ring 6 against the user's own
        // RPC, for which this file is the audit material.
        if let Some((_, _, raw)) = &basis_proof {
            atomic_write(
                &self
                    .feed_dir
                    .join(strk20_feed::manifest::snapshot_anchor_file_name(epoch)),
                serde_json::to_vec_pretty(raw)?.as_slice(),
            )?;
        }
        self.db.insert_snapshot(&entry)?;
        self.prune_snapshots()?;
        self.rewrite_manifest()?;
        tracing::info!(
            epoch,
            block = basis,
            grounding,
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
            for name in [
                old.file.clone(),
                strk20_feed::manifest::snapshot_anchor_file_name(old.e),
            ] {
                let path = self.feed_dir.join(&name);
                if path.exists() {
                    fs::remove_file(&path)
                        .with_context(|| format!("remove {}", path.display()))?;
                }
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

/// Where the §5.6 recovery rescan must START to have any chance of repairing
/// the divergence named in `mismatch`.
///
/// The obvious lower bound — one block above the newest cut epoch — is wrong
/// for a mismatch reported AT a basis block, which is that epoch's end block:
/// `last_epoch.to + 1` is one block ABOVE the divergence, so the rescan cannot
/// touch it and reports "recovered 0 blocks" for a range that never contained
/// the problem. Pool slots are write-once, so a root mismatch at B means a
/// write at or below B was never learned; the rescan therefore starts at the
/// beginning of the epoch containing B whenever B is below the default bound,
/// and keeps the default (the un-cut tail) otherwise.
///
/// It is a bounded widening, not a proof of repair: the missing write can be
/// older than B's own epoch, and a mismatch that survives the rescan needs a
/// full resync. The caller says so when the retry fails.
pub fn rescan_lower_bound(mismatch: &str, last_epoch_to: Option<u64>, cfg: &ChainConfig) -> u64 {
    let default_from = last_epoch_to
        .map(|to| to.saturating_add(1))
        .unwrap_or(cfg.genesis_block);
    let named = mismatch
        .split("MISMATCH at block ")
        .nth(1)
        .and_then(|rest| {
            let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
            digits.parse::<u64>().ok()
        });
    match named {
        Some(b) if b < default_from => cfg
            .epoch_range(cfg.epoch_of(b))
            .0
            .max(cfg.genesis_block)
            .min(default_from),
        _ => default_from,
    }
}

/// The block hash a §12 B2-bound proof was verified against. Only reachable
/// after `bound_proof` has already established that it is present and equal to
/// the chain's.
fn anchor_block_hash(proof: &crate::rpc::StorageProof) -> Result<Felt> {
    proof
        .global_roots
        .get("block_hash")
        .and_then(|v| v.as_str())
        .map(crate::rpc::parse_felt)
        .transpose()?
        .ok_or_else(|| anyhow::anyhow!("bound proof lost its global_roots.block_hash"))
}

fn footer_for(blocks: &[BlockLine], class: Felt) -> Footer {
    Footer {
        blocks: blocks.len() as u64,
        diffs: blocks.iter().map(|b| b.diffs.len() as u64).sum(),
        events: blocks.iter().map(|b| b.events.len() as u64).sum(),
        class,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_cfg(genesis: u64, epoch_size: u64) -> ChainConfig {
        let mut c = ChainConfig::mainnet();
        c.genesis_block = genesis;
        c.epoch_size = epoch_size;
        c
    }

    /// The §5.6 recovery rescan must be able to REACH the block the mismatch
    /// names. A basis-block mismatch is reported AT the newest cut epoch's end
    /// block, and the obvious lower bound (`last_epoch.to + 1`) is one block
    /// above it: the rescan then covers a range that provably cannot contain
    /// the divergence, recovers nothing, and the operator sees "rescan
    /// complete" for a repair that never had a chance to happen.
    #[test]
    fn the_rescan_range_covers_the_block_the_mismatch_names() {
        let cfg = test_cfg(0, 16);
        // Epoch 1 = [16, 31]; a snapshot's basis is 31, the same block
        // `last_epoch.to` names.
        let basis_mismatch = "VERIFY-ROOT MISMATCH at block 31: the snapshot's slot set folds \
                              to 0x1 but the chain's proof for that block says 0x2";
        let from = rescan_lower_bound(basis_mismatch, Some(31), &cfg);
        assert!(
            from <= 31,
            "a rescan starting at {from} cannot re-ingest block 31, which is the block the \
             mismatch is about"
        );
        assert_eq!(from, 16, "the widening is to the containing epoch, not to genesis");

        // A pool write below the basis is the actual cause of such a mismatch
        // (slots are write-once), so the range has to include the whole epoch.
        assert!(from <= 20, "block 20 is in the same epoch and must be rescanned too");
    }

    /// The head-side case is unchanged: the mismatch is reported ABOVE the
    /// newest cut epoch, and rescanning the un-cut tail is both sufficient and
    /// the cheapest thing that can work.
    #[test]
    fn a_mismatch_above_the_epoch_floor_still_rescans_only_the_tail() {
        let cfg = test_cfg(0, 16);
        let from = rescan_lower_bound("VERIFY-ROOT MISMATCH at block 40: ...", Some(31), &cfg);
        assert_eq!(from, 32, "the tail starts one block above the newest cut epoch");
    }

    /// Degenerate inputs must not widen the range by accident: no epoch cut
    /// yet means genesis, and an error that names no block leaves the default
    /// bound alone.
    #[test]
    fn an_unparseable_or_absent_bound_falls_back_to_the_default() {
        let cfg = test_cfg(1000, 16);
        assert_eq!(
            rescan_lower_bound("VERIFY-ROOT MISMATCH at block 1008: ...", None, &cfg),
            1000,
            "with no epoch cut the rescan starts at the pool's genesis block"
        );
        assert_eq!(
            rescan_lower_bound("some other failure entirely", Some(1031), &cfg),
            1032
        );
        // Never below the pool's genesis: there is nothing to ingest there.
        assert_eq!(
            rescan_lower_bound("VERIFY-ROOT MISMATCH at block 1000: ...", Some(1000), &cfg),
            1000
        );
    }
}
