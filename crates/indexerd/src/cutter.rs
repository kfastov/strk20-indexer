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
/// A real divergence is an `Err` carrying [`RootMismatch`].
#[derive(Debug)]
pub enum VerifyOutcome {
    Verified(Anchor),
    Unavailable(String),
}

/// A verify-root divergence as DATA rather than as a sentence.
///
/// The shipped build re-derived the mismatch block by parsing it back out of
/// the error message (`rescan_lower_bound`, deleted with this type), which is
/// how "recover with a full-range rescan of recent epochs" — advice that was
/// wrong in every case ever observed — became load-bearing in code
/// (sound-ingest.md §2.3 and §8.1). The three numbers the recovery path
/// actually needs now travel with the error, and the sentence is only a
/// sentence.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RootMismatch {
    /// The block the PROBE asked about. Not where the divergence is.
    pub block: u64,
    pub local_root: Felt,
    pub chain_root: Felt,
}

impl std::fmt::Display for RootMismatch {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "VERIFY-ROOT MISMATCH at block {}: local {} != chain {} — the mirror is \
             missing writes and publication stays blocked until a verify-root passes. \
             Block {} is where we LOOKED, not where the divergence is: pool slots are \
             write-once, so the missing write may sit arbitrarily far below it \
             (sound-ingest.md §2.3). Recovery localises it by walking the storage trie \
             (`strk20 enumerate-slots --attribute`), never by rescanning a recent window.",
            self.block,
            felt_hex(&self.local_root),
            felt_hex(&self.chain_root),
            self.block
        )
    }
}

impl std::error::Error for RootMismatch {}

/// The structured divergence `err` carries, if it carries one. Looks through
/// the whole context chain, so a mismatch stays recognisable however many
/// `.context()` layers it picked up on the way up.
pub fn root_mismatch_of(err: &anyhow::Error) -> Option<RootMismatch> {
    err.chain()
        .find_map(|cause| cause.downcast_ref::<RootMismatch>())
        .copied()
}

/// What a backward re-cut rewrote.
#[derive(Debug)]
pub struct RecutOutcome {
    pub first_epoch: u64,
    /// (epoch, old content hash, new content hash), ascending.
    pub rewritten: Vec<(u64, [u8; 32], [u8; 32])>,
    /// Epochs at the bottom of the named range whose published bytes already
    /// described this database, so they were left alone. Non-empty exactly when
    /// an earlier re-cut of this range died part-way and this one resumed it.
    pub already_current: Vec<u64>,
    /// Snapshots withdrawn because the epoch they name was re-cut.
    pub snapshots_dropped: Vec<u64>,
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

    /// `finality_upto` is the L1-accepted height the FILE these lines go into
    /// asserts in its own header, or `None` for epoch files, whose lines carry
    /// no `finality` at all (an epoch is cut at an l1-final block, so every
    /// line in it is final by construction).
    ///
    /// Passing the height rather than a "yes, stamp finality" marker is the
    /// point: one head.ndjson must not say `l1_accepted: N` in its header and
    /// stamp `"fin":"l1"` on a block above N in its tail. `blocks.status`
    /// and `meta.l1_accepted_number` are two different writers — the column is
    /// also set from each block's own header label in `ingest_block` — so the
    /// two halves can disagree, and the tail is the half that reaches wallets
    /// per block (consumer/src/apply.rs maps `Finality::L1` into the flag
    /// `replace_range` persists). The header wins: a line is L1 only if the
    /// row says so AND the header's height covers it.
    fn blocks_as_lines(
        &self,
        from: u64,
        to: u64,
        finality_upto: Option<u64>,
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
                finality: finality_upto.map(|upto| {
                    if b.l1_accepted && b.number <= upto {
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
            return Err(anyhow::Error::new(RootMismatch {
                block,
                local_root,
                chain_root: remote_root,
            }));
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
                // The proof says the mirror holds every pool slot as of this
                // block, so whatever divergence was being tracked is over —
                // whether the closure loop repaired it or an operator did.
                // Clearing both together is what keeps /health's reason from
                // outliving the condition it describes.
                self.db.meta_set("verify_root_failed", "")?;
                crate::recovery::clear(self.db)?;
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
            Err(e) if root_mismatch_of(&e).is_some() => {
                // Surfaced in /health; the caller decides whether this
                // divergence has already had its one recovery attempt
                // (`recovery::decide`) and, if not, runs the §4.2 closure loop.
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

    /// Re-cut every published epoch from `first_idx` upward, from the DB as it
    /// stands now. Local work only — DB → NDJSON → zstd → manifest, not one RPC
    /// call.
    ///
    /// Why this has to exist at all: `cut_ready_epochs` starts at
    /// `last_epoch().idx + 1` and can only append. Repairing a block deep in
    /// history changes its epoch's content, therefore that epoch's content
    /// hash, therefore — through the `prev` field in every header above it —
    /// every epoch hash in the chain above. Without a backward re-cut a
    /// repaired database can never reach the published bytes, and an operator
    /// who finds a hole in production has no path from "the DB is fixed" to
    /// "the feed is fixed" short of deleting the feed and re-cutting 515
    /// epochs.
    ///
    /// The GUARD is the other half. Rewriting published history is the most
    /// dangerous thing this binary does, so it is refused unless SOME epoch at
    /// or above the one named actually rebuilds to something different from
    /// what was published. That makes the operation impossible to trigger by
    /// accident (nothing calls it automatically, and a stray invocation on a
    /// healthy DB fails) while leaving the legitimate case — content really did
    /// change underneath — free to proceed. Epochs above the first rewritten
    /// one are rewritten unconditionally and correctly: their content changed
    /// by definition, because their `prev` did.
    ///
    /// The guard deliberately asks about the whole range and not just its first
    /// epoch. Asking only about the first makes a re-cut that died part-way
    /// UNRESUMABLE: the epochs it already rewrote now rebuild identically, so
    /// re-running the documented command hits the refusal — with a message
    /// ("nothing below it changed") that is false in exactly that state, while
    /// every epoch above is still stale. Whole-range means a resumed re-cut
    /// skips the finished prefix and carries on from the first epoch that is
    /// genuinely stale.
    ///
    /// CRASH SAFETY: each epoch's `.zst`, its DB row, AND the manifest are
    /// committed together, inside the loop. The manifest is what every client
    /// hashes against (ring 1), so leaving it for the end means any abort
    /// part-way publishes post-repair bytes under a pre-repair manifest —
    /// fail-closed for every client at once, i.e. a total feed outage. Rewriting
    /// it per epoch costs one small file write per epoch and makes every
    /// intermediate state a consistent, servable feed that is simply not yet
    /// fully repaired.
    pub fn recut_epochs_from(&self, first_idx: u64) -> Result<RecutOutcome> {
        self.ensure_layout()?;
        let rows = self.db.epoch_rows()?;
        let Some(pos) = rows.iter().position(|r| r.idx == first_idx) else {
            let cut = match (rows.first(), rows.last()) {
                (Some(f), Some(l)) => format!("{}..{}", f.idx, l.idx),
                _ => "none".to_owned(),
            };
            bail!(
                "epoch {first_idx} has not been cut in this database (cut epochs: {cut}), so \
                 there is nothing published to re-cut there. Epochs that were never cut are \
                 produced by the ordinary forward cut, not by this command."
            );
        };
        // The chain below `first_idx` is untouched, so the re-cut inherits its
        // last link rather than recomputing it — but only after checking that
        // it IS untouched. Naming an epoch above the first affected one is the
        // easy mistake (a repair that touched two epochs, an off-by-one on the
        // block), and it fails silently in the worst possible way: the epochs
        // below keep publishing bytes that no longer describe the database,
        // while everything above is rewritten and looks freshly repaired.
        //
        // The scan runs all the way to epoch 0 rather than stopping at the
        // first epoch that still matches. Stopping there only ever reports the
        // CONTIGUOUS stale run directly beneath `first_idx`, and a repair that
        // touched two distant blocks produces a non-contiguous one: the
        // operator follows the guard's advice once, and a lower stale epoch
        // stays published indefinitely under a hash chain `epoch-verify` calls
        // OK, because nothing else in the system compares feed bytes against
        // the database. The cost is one `build_epoch` per epoch below the named
        // one — the same work the re-cut itself does, on a command that is run
        // by hand after a repair.
        let mut stale_below: Option<u64> = None;
        for below in (0..pos).rev() {
            if self.rebuilt_hash(&rows, below)? != rows[below].content_hash {
                stale_below = Some(rows[below].idx);
            }
        }
        if let Some(lowest) = stale_below {
            bail!(
                "REFUSING TO RE-CUT: epoch {lowest} is BELOW {first_idx} and no longer matches \
                 this database either, so re-cutting from {first_idx} would leave every epoch \
                 from {lowest} up publishing bytes that describe the pre-repair mirror. \
                 Re-cut from epoch {lowest} instead."
            );
        }
        let mut prev: Option<[u8; 32]> = if pos == 0 {
            None
        } else {
            Some(rows[pos - 1].content_hash)
        };
        let cut_at: u64 = self
            .db
            .meta_get("head_number")?
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let mut out = RecutOutcome {
            first_epoch: first_idx,
            rewritten: Vec::new(),
            already_current: Vec::new(),
            snapshots_dropped: Vec::new(),
        };
        // Nothing is written until an epoch is found that genuinely disagrees
        // with the database, so the refusal below can still abort with the feed
        // untouched. Once one has been found, every epoch above it MUST be
        // rewritten: its `prev` changed, therefore its content did.
        let mut diverged = false;
        for row in rows[pos..].iter() {
            let (from, to) = self.cfg.epoch_range(row.idx);
            let epoch = self.build_epoch(row.idx, prev)?;
            let payload = codec::encode_epoch(&epoch);
            let content_hash = payload_sha256(&payload);
            if !diverged {
                if content_hash == row.content_hash {
                    // Published bytes still describe this database, and the
                    // chain below is intact (checked above), so this epoch is
                    // already what a re-cut would produce. Skipping it keeps a
                    // resumed re-cut from re-emitting bytes clients already
                    // hold, and keeps the WARN log honest about what changed.
                    out.already_current.push(row.idx);
                    prev = Some(content_hash);
                    continue;
                }
                diverged = true;
                // BEFORE the first byte is replaced, for the same reason the
                // manifest is written inside the loop: a snapshot describes a
                // slot set folded from the pre-repair mirror, and it is stale
                // from the moment the first re-cut epoch lands, not from the
                // moment the batch finishes.
                self.withdraw_snapshots_from(first_idx, &mut out)?;
            }
            let compressed = strk20_feed::compress(&payload);
            let zst_hash = payload_sha256(&compressed);
            let file = self.epochs_dir().join(format!("{:08}.strk20e.zst", row.idx));
            atomic_write(&file, &compressed)?;
            // The anchor is a CHAIN fact about the epoch's end block — a proof
            // this instance obtained and published — and a re-cut of our own
            // bytes says nothing about it, so it is carried over verbatim
            // rather than dropped or re-fetched.
            let anchor = row.anchor_block.map(|block| Anchor {
                block,
                block_hash: row.anchor_block_hash.unwrap_or(Felt::ZERO),
                storage_root: row.anchor_storage_root.unwrap_or(Felt::ZERO),
                class_hash: row.anchor_class_hash.unwrap_or(Felt::ZERO),
            });
            self.db.insert_epoch(
                row.idx,
                from,
                to,
                &content_hash,
                &zst_hash,
                compressed.len() as u64,
                prev.as_ref(),
                anchor.as_ref(),
                cut_at,
            )?;
            tracing::warn!(
                epoch = row.idx,
                from,
                to,
                blocks = epoch.blocks.len(),
                old_hash = hex::encode(row.content_hash),
                hash = hex::encode(content_hash),
                "epoch RE-CUT (published bytes replaced)"
            );
            out.rewritten
                .push((row.idx, row.content_hash, content_hash));
            prev = Some(content_hash);
            // Committed WITH the epoch, not after the batch. The manifest is
            // ring 1 for every client, so a manifest that lags the files it
            // names is a feed outage; rewriting it here means an abort at any
            // point leaves a consistent feed whose repair is simply
            // incomplete, and re-running the same command finishes the job.
            self.rewrite_manifest()?;
        }
        if !diverged {
            bail!(
                "REFUSING TO RE-CUT: every published epoch from {first_idx} up ({} epoch(s), \
                 through {}) rebuilds from this database byte-for-byte, so the feed already \
                 describes it. A re-cut would rewrite all of them — every client's hash chain — \
                 for no reason. Re-cut only after a repair that actually changed an epoch's \
                 blocks (`strk20 audit-coverage --repair`), and name the epoch the repair \
                 touched. (If an earlier re-cut of this range was interrupted, this message \
                 means it finished: confirm with `strk20 epoch-verify`.)",
                rows.len() - pos,
                rows.last().map(|r| r.idx).unwrap_or(first_idx)
            );
        }
        self.rewrite_manifest()?;
        Ok(out)
    }

    /// Withdraw every published snapshot at or above `first_idx`.
    ///
    /// A published snapshot names its epoch's content hash and carries the slot
    /// set as of that epoch's end block. Both are stale for every re-cut epoch,
    /// and a client that fetched the manifest would reject the snapshot (ring
    /// 4) or, worse, fold a slot set built from the holed mirror — so the
    /// affected ones are dropped here and republished by the next cut, once the
    /// §11.3 gate is met again.
    ///
    /// The bound is `first_idx` and not the first epoch actually rewritten: an
    /// interrupted earlier re-cut may already have replaced an epoch that this
    /// run now skips as current, and the snapshot it grounds is stale all the
    /// same.
    fn withdraw_snapshots_from(&self, first_idx: u64, out: &mut RecutOutcome) -> Result<()> {
        for snap in self.db.snapshot_rows()? {
            if snap.e < first_idx {
                continue;
            }
            for name in [
                snap.file.clone(),
                strk20_feed::manifest::snapshot_anchor_file_name(snap.e),
            ] {
                let path = self.feed_dir.join(&name);
                if path.exists() {
                    fs::remove_file(&path)
                        .with_context(|| format!("remove {}", path.display()))?;
                }
            }
            self.db.delete_snapshot(snap.e)?;
            out.snapshots_dropped.push(snap.e);
        }
        Ok(())
    }

    /// The content hash epoch `rows[pos]` would have if it were cut from the
    /// database as it stands now, keeping its PUBLISHED `prev`. Equal to the
    /// stored hash exactly when the published bytes still describe the DB.
    fn rebuilt_hash(&self, rows: &[crate::db::EpochRowFull], pos: usize) -> Result<[u8; 32]> {
        let prev = if pos == 0 {
            None
        } else {
            Some(rows[pos - 1].content_hash)
        };
        let epoch = self.build_epoch(rows[pos].idx, prev)?;
        Ok(payload_sha256(&codec::encode_epoch(&epoch)))
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
                tracing::error!(
                    epoch,
                    block = basis,
                    "the slot set this snapshot would carry does not fold to the chain's \
                     root at its basis block; refusing to publish the snapshot"
                );
                return Err(anyhow::Error::new(RootMismatch {
                    block: basis,
                    local_root: snap.header.storage_root,
                    chain_root,
                }));
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
        let blocks = self.blocks_as_lines(tail_from, head_number, Some(l1_accepted))?;
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
    use crate::db::BlockRow;

    /// One head.ndjson must not contradict itself. Its header's `l1_accepted`
    /// comes from `meta`; each tail line's `fin` comes from `blocks.status`,
    /// and the two are written by different code — `promote_l1` and the
    /// finality poll write the first, while `ingest_block` also sets the column
    /// straight from each block's OWN header label, which no cycle ever
    /// compared against `meta`. So the column can run ahead, and a reader
    /// handed `l1_accepted: 102` in the header and `"fin":"l1"` on block 104
    /// has no way to tell which half to believe. The tail is the half that
    /// reaches wallets per block (consumer/src/apply.rs turns `Finality::L1`
    /// into the flag `replace_range` persists), so the header is what caps it.
    #[test]
    fn no_head_line_claims_more_finality_than_the_header_it_ships_with() {
        let dir = tempfile::tempdir().unwrap();
        let mut cfg = ChainConfig::sepolia();
        cfg.genesis_block = 100;
        cfg.epoch_size = 16;
        let mut db = Db::open(&dir.path().join("t.db")).unwrap();
        // 100..=104 all carry status=1 — the state a stale `meta` plus honest
        // per-block labels leaves behind — while meta stops at 102.
        for n in 100..=104u64 {
            db.insert_block_data(
                &BlockRow {
                    number: n,
                    hash: Felt::from(n + 0x1000),
                    parent_hash: Felt::from(n + 0xfff),
                    timestamp: 1_700_000_000 + n,
                    l1_accepted: true,
                },
                &[],
                &[],
                None,
                n,
            )
            .unwrap();
        }
        db.meta_set("head_number", "104").unwrap();
        db.meta_set("head_hash", &felt_hex(&Felt::from(0x1068u64)))
            .unwrap();
        db.meta_set("l1_accepted_number", "102").unwrap();

        let rpc = RpcClient::new("http://127.0.0.1:1/".to_owned(), None);
        let feed_dir = dir.path().join("feed");
        Cutter {
            db: &db,
            rpc: &rpc,
            cfg: &cfg,
            feed_dir: feed_dir.clone(),
        }
        .regen_head()
        .unwrap();

        let text = fs::read_to_string(feed_dir.join("head.ndjson")).unwrap();
        assert!(text.contains("\"l1_accepted\":102"), "{text}");
        for line in text.lines() {
            let Some(rest) = line.strip_prefix("{\"t\":\"blk\",\"b\":") else {
                continue;
            };
            let number: u64 = rest[..rest.find(',').unwrap()].parse().unwrap();
            let l1 = line.contains("\"fin\":\"l1\"");
            assert_eq!(
                l1,
                number <= 102,
                "block {number} is stamped {} in a file whose header says \
                 l1_accepted 102\n{text}",
                if l1 { "l1" } else { "l2" }
            );
        }
    }

    fn mismatch() -> RootMismatch {
        RootMismatch {
            block: 14_448_522,
            local_root: Felt::from(0xabcu64),
            chain_root: Felt::from(0xdefu64),
        }
    }

    /// The recovery path reads the divergence off the error by DOWNCAST, and
    /// the error reaches it through `cut_ready_epochs`, which is free to add
    /// context on the way. If a context layer hid the type, recovery would
    /// silently degrade into "epoch cutting halted" and the closure loop would
    /// never run — the same silence, differently caused.
    #[test]
    fn a_mismatch_survives_the_context_layers_it_collects_on_the_way_up() {
        let m = mismatch();
        let err = anyhow::Error::new(m)
            .context("getStorageProof for verify-root")
            .context("cutting epoch 1172");
        assert_eq!(root_mismatch_of(&err), Some(m));
        assert_eq!(
            root_mismatch_of(&anyhow::anyhow!("some other failure entirely")),
            None
        );
    }

    /// §8.1: the mismatch text used to end with "recover with a full-range
    /// rescan of recent epochs", and `rescan_lower_bound` parsed the block
    /// number back out of it to build exactly that window. Both are gone. What
    /// the sentence must now say is that the probe block is where we LOOKED,
    /// because a reader who believes otherwise reaches for the wrong tool.
    #[test]
    fn the_mismatch_text_no_longer_advises_a_recent_window_rescan() {
        let text = mismatch().to_string();
        assert!(text.starts_with("VERIFY-ROOT MISMATCH at block 14448522:"), "{text}");
        assert!(!text.to_lowercase().contains("recent epochs"), "{text}");
        assert!(text.contains("arbitrarily far below"), "{text}");
        assert!(text.contains("enumerate-slots --attribute"), "{text}");
    }
}
