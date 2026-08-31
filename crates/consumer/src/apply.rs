//! Feed apply/verify — the fold half of Block B (spec §7.3, §1.5, §1.7, §11.3).
//!
//! This is the whole trust pipeline of a consumer: verify the epoch hash chain,
//! bind each epoch to the chain and pool it claims to be about, supersede
//! anything a reorg replaced, fold the head tail, and — for a cold start —
//! walk the snapshot verification ladder and ground the result. Nothing here
//! knows what a row is stored in; every write goes through [`ConsumerStore`].
//!
//! Reorg discipline (review findings): a newly applied epoch SUPERSEDES any
//! stored rows in its range (tail rows from a reorged-away chain must not
//! survive under the new floor — the "masked reorg"), and every tail
//! replacement bumps the persisted `tail_generation` in the SAME store write as
//! the rebuild, so per-owner cursor rewinds survive crashes and shared-store
//! multi-owner use.

use crate::store::{ApplyOutcome, ColdStart, ConsumerStore, Range};
use crate::transport::FeedTransport;
use anyhow::{bail, Context, Result};
use starknet_types_core::felt::Felt;
use strk20_feed::codec::{self, BlockLine};
use strk20_feed::manifest::Manifest;
use strk20_feed::snapshot::{FeedIdentity, SnapSlot, Snapshot};

/// Marker attached to every failure of the snapshot cold-start path, so the
/// `auto` fallback fires on exactly those and never swallows an unrelated
/// error.
#[derive(Debug)]
struct SnapshotRejected;

impl std::fmt::Display for SnapshotRejected {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("snapshot cold start rejected")
    }
}

/// §1.5.2 guard rail, shared by every host's `view()`: a bound below the
/// snapshot basis is refused rather than served. Pre-basis state does not exist
/// locally and must never be answered with zeros. Engine bounds are always
/// `last_epoch_to` or `head`, both at or above the basis — the rule exists so a
/// future refactor cannot introduce a silent zero-read.
pub fn check_bound_above_basis<S: ConsumerStore + ?Sized>(store: &S, block: u64) -> Result<()> {
    let basis: Option<u64> = store
        .meta_get("snapshot_basis")?
        .and_then(|s| s.parse().ok());
    if let Some(basis) = basis {
        if block < basis {
            bail!(
                "BOUND_BELOW_SNAPSHOT {{\"bound\": {block}, \"basis\": {basis}}}: this \
                 mirror was cold-started from a snapshot at block {basis} and holds no \
                 state below it"
            );
        }
    }
    Ok(())
}

/// Lowest block for which this mirror holds EVENTS (§1.1). 0 for a fully
/// epoch-replayed mirror.
pub fn history_floor<S: ConsumerStore + ?Sized>(store: &S) -> Result<u64> {
    Ok(store
        .meta_get("history_floor")?
        .and_then(|s| s.parse().ok())
        .unwrap_or(0))
}

/// Apply verified epochs + the head tail per the manifest. Any hash mismatch is
/// a hard error naming the epoch and both hashes (U5 divergence detection).
pub async fn apply_feed<S: ConsumerStore>(
    store: &S,
    transport: &dyn FeedTransport,
    cold_start: ColdStart,
) -> Result<ApplyOutcome> {
    // A mirror carrying snapshot rows that were never grounded is exactly
    // as trustworthy as one whose grounding failed, and it must not be
    // built on: without this, a rejected snapshot's rows survive the failed
    // run, the next run sees a non-empty mirror, skips the snapshot branch
    // and therefore skips the grounding, and the client ends up permanently
    // on a slot set it explicitly refused once.
    if store.meta_get("snapshot_pending_grounding")?.as_deref() == Some("1") {
        tracing::warn!(
            "this mirror holds a snapshot whose §11.3 grounding never completed; \
             discarding it rather than building on an unverified slot set"
        );
        store.reset_mirror()?;
    }
    match apply_feed_once(store, transport, cold_start, false).await {
        Ok(out) => Ok(out),
        Err(e) if e.downcast_ref::<SnapshotRejected>().is_some() => {
            // Whatever the mode, the refused snapshot's rows go: leaving
            // them is what turns one rejection into a permanently poisoned
            // store.
            store.reset_mirror()?;
            // C13: under `auto` a snapshot that cannot be verified or
            // grounded costs a full replay, never the sync.
            if cold_start == ColdStart::Auto {
                tracing::warn!(
                    error = %format!("{e:#}"),
                    "snapshot rejected; falling back to full epoch replay"
                );
                return apply_feed_once(store, transport, ColdStart::Epochs, true).await;
            }
            Err(e)
        }
        Err(e) => Err(e),
    }
}

async fn apply_feed_once<S: ConsumerStore>(
    store: &S,
    transport: &dyn FeedTransport,
    cold_start: ColdStart,
    snapshot_rejected: bool,
) -> Result<ApplyOutcome> {
    let mut out = ApplyOutcome {
        snapshot_rejected,
        ..Default::default()
    };
    let genesis = transport.fetch_genesis().await?;
    match store.meta_get("pool")? {
        None => {
            store.meta_set("pool", &genesis.pool)?;
            store.meta_set("chain_id", &genesis.chain_id)?;
            store.meta_set("epoch_size", &genesis.epoch_size.to_string())?;
            store.meta_set("genesis_block", &genesis.genesis_block.to_string())?;
        }
        Some(stored) if stored != genesis.pool => {
            bail!("feed pool {} does not match local mirror {}", genesis.pool, stored)
        }
        Some(_) => {}
    }
    // The chain id was pinned on first sync but never compared again, so a
    // mirror could be re-pointed at a feed carrying the same pool address
    // on a fork or test chain and fold it in. Enforce it exactly like the
    // pool — this is the check that does not depend on the operator
    // remembering `--network`.
    if let Some(stored) = store.meta_get("chain_id")? {
        if stored != genesis.chain_id {
            bail!(
                "feed chain id {} does not match local mirror {stored}",
                genesis.chain_id
            );
        }
    }
    let manifest: Manifest = transport.fetch_manifest().await?;
    if manifest.chain_id != genesis.chain_id {
        bail!(
            "feed disagrees with itself: genesis chain id {} != manifest {}",
            genesis.chain_id,
            manifest.chain_id
        );
    }
    let feed_pool = strk20_feed::felt_from_hex(&genesis.pool)?;

    // 0. snapshot cold start (§1.7). Taken only on an empty mirror, so a
    // non-empty one never touches snapshots. Cold start is O(1) in history
    // length and identical for every user: genesis + manifest + snapshot +
    // anchors + (epochs above the basis, normally 0-1) + head.
    let mut fresh_basis: Option<u64> = None;
    if cold_start != ColdStart::Epochs && store.is_empty()? {
        match manifest.snapshot.clone() {
            Some(entry) => {
                let snap = cold_start_from_snapshot(
                    store, transport, &manifest, &entry, &genesis, &feed_pool,
                )
                .await
                .map_err(|e| e.context(SnapshotRejected))?;
                fresh_basis = Some(snap);
            }
            // `snapshot` REQUIRES one (§1.5.2: refuse loudly, never
            // degrade). Silently replaying every epoch from genesis is the
            // run the operator explicitly asked not to do, and on a metered
            // link the cost is theirs, not ours.
            None if cold_start == ColdStart::Snapshot => bail!(
                "SNAPSHOT_UNAVAILABLE: this feed publishes no snapshot \
                 (manifest.snapshot is null), so --cold-start snapshot cannot be \
                 honoured. Use --cold-start auto to fall back to epoch replay, or \
                 --cold-start epochs to ask for it explicitly."
            ),
            None => {}
        }
    }

    // 1. epochs
    let mut last_applied: Option<u64> = store
        .meta_get("last_epoch_applied")?
        .and_then(|s| s.parse().ok());
    let mut prev_hash: Option<[u8; 32]> = match store.meta_get("last_epoch_hash")? {
        Some(hexs) => Some(
            hex::decode(&hexs)?
                .try_into()
                .map_err(|_| anyhow::anyhow!("bad stored epoch hash"))?,
        ),
        None => None,
    };
    // A mirror switch to a divergent chain must not pass silently: the
    // manifest's entry for our last applied epoch must carry OUR hash.
    if let (Some(done), Some(local)) = (last_applied, prev_hash) {
        match manifest.epoch(done) {
            Some(entry) if entry.hash == hex::encode(local) => {}
            Some(entry) => bail!(
                "feed diverged: epoch {done} hash {} != locally applied {}",
                entry.hash,
                hex::encode(local)
            ),
            None => bail!("feed diverged: manifest no longer lists applied epoch {done}"),
        }
    }
    for entry in &manifest.epochs {
        if let Some(done) = last_applied {
            if entry.e <= done {
                continue;
            }
        }
        let compressed = transport.fetch_epoch(entry.e).await?;
        // R-I: the manifest that names this file's sha256 is authored by
        // the same server, so a passing transport hash says nothing about
        // how far the frame expands.
        let payload = transport.decompress(
            &compressed,
            strk20_feed::MAX_DECOMPRESSED,
            &format!("epoch {}", entry.e),
        )?;
        let epoch =
            strk20_feed::manifest::verify_epoch_against_manifest(&payload, entry, prev_hash)?;
        // The hash chain proves this is the epoch the manifest names; the
        // binding check proves the manifest is about the chain and pool we
        // believe we are mirroring.
        strk20_feed::manifest::verify_epoch_binding(&epoch, &genesis.chain_id, &feed_pool)?;
        let range = Range::Inclusive {
            from: entry.from,
            to: entry.to,
        };
        // Masked-reorg check: stored (tail) rows inside this epoch's range
        // that contradict the L1-final epoch content mean the old tail was
        // replaced while we were not looking.
        let stored = store.block_hashes(range)?;
        let contradiction = stored
            .iter()
            .any(|(n, h)| !epoch.blocks.iter().any(|b| b.number == *n && b.hash == *h));
        let hash_hex = hex::encode(strk20_feed::payload_sha256(&payload));
        // The epoch supersedes everything in its range.
        let blocks: Vec<(&BlockLine, bool)> =
            epoch.blocks.iter().map(|b| (b, true)).collect();
        store.replace_range(
            range,
            &blocks,
            &[
                ("last_epoch_applied", entry.e.to_string()),
                ("last_epoch_hash", hash_hex),
                ("last_epoch_to", entry.to.to_string()),
            ],
            contradiction,
        )?;
        if contradiction {
            out.tail_rewound = true;
        }
        prev_hash = Some(strk20_feed::payload_sha256(&payload));
        last_applied = Some(entry.e);
        out.epochs_applied += 1;
    }
    out.last_epoch_to = store
        .meta_get("last_epoch_to")?
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);

    // 2. head tail
    let etag = store.meta_get("head_etag")?;
    if let Some((head_bytes, new_etag)) = transport.fetch_head(etag.as_deref()).await? {
        let head = codec::parse_head(&head_bytes)?;
        // Manifest/head fetch race: a server-side epoch cut between our
        // two fetches leaves a gap the wholesale rebuild would turn into
        // a silent mirror hole — fail cleanly, the next sync heals.
        if head.header.tail_from > out.last_epoch_to + 1 {
            bail!(
                "feed advanced mid-sync (tail starts at {}, our epoch floor is {}); retry",
                head.header.tail_from,
                out.last_epoch_to
            );
        }
        let range = Range::Above {
            floor: out.last_epoch_to,
        };
        // Reorg rule (spec §7.5): stored tail rows that contradict the new
        // tail file mean the old tail was replaced.
        let stored = store.block_hashes(range)?;
        let contradiction = stored
            .iter()
            .any(|(n, h)| match head.blocks.iter().find(|b| b.number == *n) {
                Some(b) => b.hash != *h,
                None => *n >= head.header.tail_from,
            });
        // rebuild the tail wholesale above the epoch floor
        let blocks: Vec<(&BlockLine, bool)> = head
            .blocks
            .iter()
            .map(|b| (b, matches!(b.finality, Some(codec::Finality::L1))))
            .collect();
        store.replace_range(
            range,
            &blocks,
            &[
                ("head_etag", new_etag),
                ("head_number", head.header.head.to_string()),
                ("head_hash", strk20_feed::felt_hex(&head.header.head_hash)),
                ("l1_accepted", head.header.l1_accepted.to_string()),
            ],
            contradiction,
        )?;
        if contradiction {
            out.tail_rewound = true;
        }
        out.tail_changed = true;
        out.head = head.header.head;
        out.l1_accepted = head.header.l1_accepted;
    } else {
        out.head = store
            .meta_get("head_number")?
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        out.l1_accepted = store
            .meta_get("l1_accepted")?
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
    }

    // 3. grounding. Only meaningful for a mirror that was cold-started
    // from a snapshot in THIS call: the epochs above the basis and the
    // head tail had to land first, because reachability validates them
    // too.
    if let Some(basis) = fresh_basis {
        let anchor = check_reachability(store, transport, basis, out.head)
            .await
            .map_err(|e| e.context(SnapshotRejected))?;
        // Only a grounding that passed may clear the flag.
        store.meta_set("snapshot_pending_grounding", "0")?;
        tracing::info!(
            basis,
            anchor,
            "snapshot reachability verified against the published anchors log"
        );
    }
    out.snapshot_basis = store
        .meta_get("snapshot_basis")?
        .and_then(|s| s.parse().ok());
    out.history_floor = history_floor(store)?;
    Ok(out)
}

/// Rings 1-5 of §1.5, then fold the slot set. Reachability (§11.3) runs
/// later, once the feed above the basis has been applied.
async fn cold_start_from_snapshot<S: ConsumerStore>(
    store: &S,
    transport: &dyn FeedTransport,
    manifest: &Manifest,
    entry: &strk20_feed::manifest::ManifestSnapshot,
    genesis: &strk20_feed::manifest::Genesis,
    feed_pool: &Felt,
) -> Result<u64> {
    let basis_epoch_hash = manifest
        .epoch(entry.e)
        .map(|m| m.hash.clone())
        .ok_or_else(|| {
            anyhow::anyhow!(
                "FEED_CHAIN_BROKEN: the manifest carries a snapshot at epoch {} but does \
                 not list that epoch",
                entry.e
            )
        })?;
    let compressed = transport.fetch_snapshot(entry.e).await?;
    let snap = verify_snapshot(
        transport,
        &compressed,
        entry,
        &basis_epoch_hash,
        &FeedIdentity {
            chain_id: genesis.chain_id.clone(),
            pool: *feed_pool,
        },
    )?;
    // `entry.e` is manifest-supplied and therefore attacker-controlled:
    // unchecked, a large index wraps in release and can be made to equal
    // `header.block`, letting a snapshot claim an arbitrary epoch index
    // that is then written straight into `last_epoch_applied`.
    let expected_end = entry
        .e
        .checked_mul(genesis.epoch_size)
        .and_then(|start| start.checked_add(genesis.epoch_size))
        .and_then(|end| end.checked_sub(1))
        .ok_or_else(|| {
            anyhow::anyhow!(
                "FEED_MALFORMED: manifest snapshot epoch index {} overflows the epoch \
                 range arithmetic",
                entry.e
            )
        })?;
    if snap.header.block != expected_end {
        bail!(
            "FEED_MALFORMED: snapshot basis {} is not the end block of epoch {}",
            snap.header.block,
            entry.e
        );
    }
    check_basis_anchor(transport, entry, &snap).await?;
    install_snapshot(store, &snap)?;
    tracing::info!(
        epoch = snap.header.epoch,
        block = snap.header.block,
        slots = snap.slots.len(),
        grounding = %entry.grounding,
        "cold started from a snapshot; history floor is {}",
        snap.header.block + 1
    );
    Ok(snap.header.block)
}

/// Rings 1–5 of the §1.5 ladder over a compressed snapshot file, with
/// decompression delegated to the host (see [`FeedTransport::decompress`]) so
/// Block B does not link zstd.
///
/// The ladder itself is NOT reimplemented here. This function owns exactly the
/// one thing the feed crate's `compress`-gated wrapper cannot own for a
/// wasm-clean client — where the bytes get inflated — and delegates every ring
/// to `strk20_feed::snapshot::verify_snapshot_payload`, the single
/// implementation that the feed crate's `snapshot_rings` tests pin. Ring 1 is
/// run here, before the transport sees the bytes, and re-asserted by the
/// shared ladder.
fn verify_snapshot(
    transport: &dyn FeedTransport,
    compressed: &[u8],
    entry: &strk20_feed::manifest::ManifestSnapshot,
    basis_epoch_hash: &str,
    expect: &FeedIdentity,
) -> Result<Snapshot> {
    // ring 1 — transport, before decompression
    let zst_sha256 = hex::encode(strk20_feed::payload_sha256(compressed));
    strk20_feed::snapshot::check_zst_hash(&zst_sha256, entry)?;
    let payload = transport.decompress(compressed, strk20_feed::MAX_DECOMPRESSED, &entry.file)?;
    Ok(strk20_feed::snapshot::verify_snapshot_payload(
        &payload,
        &zst_sha256,
        entry,
        basis_epoch_hash,
        expect,
    )?)
}

/// Fold a verified snapshot into the mirror (§1.7). Slot rows land at their
/// REAL write blocks, so the shipped as-of query serves them and
/// `read_slots_with_block` returns the exact `last_update_block` a note's
/// `block_number` is derived from.
fn install_snapshot<S: ConsumerStore>(store: &S, snap: &Snapshot) -> Result<()> {
    let h = &snap.header;
    let slots: &[SnapSlot] = &snap.slots;
    store.install_snapshot(
        slots,
        &[
            ("last_epoch_applied", h.epoch.to_string()),
            ("last_epoch_hash", h.epoch_hash.clone()),
            ("last_epoch_to", h.block.to_string()),
            // Made loud (§1.1): a snapshot carries slots and no events, so no
            // event below this block exists locally and none ever will.
            ("history_floor", (h.block + 1).to_string()),
            ("snapshot_basis", h.block.to_string()),
            // Committed BEFORE the §11.3 grounding can run — the epochs above
            // the basis and the head tail have to land first, because
            // reachability validates them too. Anything that ends the process
            // inside that window (a rejection the operator re-runs past,
            // Ctrl-C, an OOM kill) would otherwise leave a mirror that is never
            // checked again: the next sync sees a non-empty mirror, skips the
            // snapshot branch entirely and never calls the grounding at all.
            // The flag makes that state recognisable, and it is cleared only by
            // a grounding that passed.
            ("snapshot_pending_grounding", "1".to_owned()),
        ],
    )
}

/// §11.3 reachability — the only grounding a cold-started client can
/// obtain. Fold `snapshot(b)`, apply everything the feed carries above it,
/// recompute the storage root at the newest anchored block `A` the mirror
/// reaches, and compare with the published record. A match attests the
/// snapshot AND every epoch between `b` and `A`, which is strictly stronger
/// than the point-proof at `b` that §1.3 asked for and §11.1 measured to be
/// unobtainable.
///
/// What it is NOT: an anchor is a SERVER ASSERTION until the client checks
/// it against an RPC it trusts (§1.5 ring 6). Hence the grade
/// `server-asserted` when no anchor RPC is configured.
async fn check_reachability<S: ConsumerStore>(
    store: &S,
    transport: &dyn FeedTransport,
    basis: u64,
    head: u64,
) -> Result<u64> {
    let bytes = transport.fetch_anchors().await?.ok_or_else(|| {
        anyhow::anyhow!(
            "SNAPSHOT_UNREACHABLE: the feed publishes no anchors.ndjson, so nothing \
             grounds the snapshot at block {basis}"
        )
    })?;
    let anchors = strk20_feed::anchors::parse_anchors(&bytes)?;
    // Newest first. Anchors are captured at HEAD (§11.2), i.e. on
    // reorg-able blocks far above `l1_accepted`, and the client fetches
    // head.ndjson and anchors.ndjson in two separate requests — so a server
    // reorg between them makes the newest anchor disagree with a tail the
    // client folded from the pre-reorg file. That is a benign race, not
    // tampering, and under `auto` treating it as tampering costs a full
    // history replay (the exact cost §11 says snapshots exist to avoid).
    //
    // Reaching ANY anchor at or above the basis attests the snapshot, since
    // pool slots are write-once and a root match at A subsumes every write
    // below A. So a lower anchor — untouched by a tail reorg — is a valid
    // fallback, while a forged slot set still fails every one of them.
    let mut candidates: Vec<&strk20_feed::anchors::AnchorRecord> = anchors
        .iter()
        .filter(|a| a.block >= basis && a.block <= head)
        .collect();
    candidates.sort_by_key(|a| std::cmp::Reverse(a.block));
    if candidates.is_empty() {
        bail!(
            "SNAPSHOT_UNREACHABLE: no published anchor lies between the snapshot \
             basis {basis} and the mirror head {head}, so nothing grounds the \
             snapshot"
        );
    }
    let mut failures: Vec<String> = Vec::new();
    for anchor in &candidates {
        let local = strk20_feed::mpt::storage_root(&store.full_slot_set_as_of(anchor.block)?);
        if local != anchor.storage_root {
            failures.push(format!(
                "block {}: folded root {} != published {}",
                anchor.block,
                strk20_feed::felt_hex(&local),
                strk20_feed::felt_hex(&anchor.storage_root)
            ));
            continue;
        }
        if let Some(stored) = store.block_hash(anchor.block)? {
            if stored != anchor.block_hash {
                failures.push(format!(
                    "block {}: mirrored block hash {} != published {}",
                    anchor.block,
                    strk20_feed::felt_hex(&stored),
                    strk20_feed::felt_hex(&anchor.block_hash)
                ));
                continue;
            }
        }
        return Ok(anchor.block);
    }
    bail!(
        "SNAPSHOT_UNREACHABLE: folding the snapshot at block {basis} plus everything \
         the feed carries above it reproduces NO published anchor between {basis} and \
         {head}. Tried {} anchor(s): {}",
        candidates.len(),
        failures.join("; ")
    )
}

/// §12 point 1 — the basis-block anchor, when the publisher obtained one.
///
/// **What this check is worth, stated exactly.** Every value compared here is
/// produced by the same server: the manifest's anchor, the published proof
/// sidecar, and the snapshot's own slot set. The comparison is still worth
/// making — it is proof-against-data rather than claim-against-claim, since
/// `snap.header.storage_root` was already proved equal to the fold of the slot
/// set by ring 5 of `verify_snapshot`, so a publisher whose proof and data
/// disagree is caught, as is one that publishes an anchor for another block or
/// one that names a root no proof backs. But nothing here binds
/// `global_roots` to a chain this client independently knows, so a publisher
/// that forges the slot set and the sidecar TOGETHER is internally consistent
/// and passes. That adversary is the §11.3 reachability walk's (it runs on
/// every cold start regardless of grounding) and ring 6's, against the user's
/// own RPC. The sidecar is the audit material for the latter.
async fn check_basis_anchor(
    transport: &dyn FeedTransport,
    entry: &strk20_feed::manifest::ManifestSnapshot,
    snap: &Snapshot,
) -> Result<()> {
    let Some(anchor) = &entry.anchor else {
        // Server-side the two are produced from one Option, so they can only
        // disagree through corruption or design: a manifest that ADVERTISES
        // the stronger grounding while carrying nothing to check would have
        // every client silently downgrade to the fallback while both the
        // manifest and the client's own log claimed otherwise.
        if entry.grounding == strk20_feed::manifest::GROUNDING_BASIS_ANCHOR {
            bail!(
                "FEED_MALFORMED: manifest.snapshot for epoch {} declares grounding \"{}\" but \
                 carries no anchor object, so there is nothing to check and the claim cannot \
                 be honoured",
                entry.e,
                entry.grounding
            );
        }
        return Ok(());
    };
    let file = strk20_feed::manifest::snapshot_anchor_file_name(entry.e);
    let bytes = transport
        .fetch_snapshot_anchor(entry.e)
        .await?
        .ok_or_else(|| {
            anyhow::anyhow!(
                "SNAPSHOT_ROOT_MISMATCH: the manifest claims a basis-block anchor for \
                 snapshot {} but the feed does not publish {file}, so there is no proof \
                 behind the claim",
                entry.e
            )
        })?;
    let sidecar: serde_json::Value = serde_json::from_slice(&bytes)
        .with_context(|| format!("SNAPSHOT_ROOT_MISMATCH: {file} is not JSON"))?;
    let proof_root = sidecar["contracts_proof"]["contract_leaves_data"][0]["storage_root"]
        .as_str()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "SNAPSHOT_ROOT_MISMATCH: {file} carries no \
                 contracts_proof.contract_leaves_data[0].storage_root"
            )
        })?;
    let proof_root = strk20_feed::felt_from_hex(proof_root)?;
    // `snap.header.storage_root` was already proved equal to the root of the
    // slot set the snapshot carries (ring 5 of verify_snapshot), so this
    // compares the proof against the data, not against another claim.
    if proof_root != snap.header.storage_root {
        bail!(
            "SNAPSHOT_ROOT_MISMATCH: the basis-block proof published at {file} attests \
             storage_root {} at block {}, but this snapshot's slot set folds to {}",
            strk20_feed::felt_hex(&proof_root),
            snap.header.block,
            strk20_feed::felt_hex(&snap.header.storage_root)
        );
    }
    if anchor.block != snap.header.block {
        bail!(
            "SNAPSHOT_ROOT_MISMATCH: manifest.snapshot.anchor is for block {} but the \
             snapshot's basis is block {}; an anchor for some other block attests \
             nothing about this one",
            anchor.block,
            snap.header.block
        );
    }
    if strk20_feed::felt_from_hex(&anchor.storage_root)? != proof_root {
        bail!(
            "SNAPSHOT_ROOT_MISMATCH: manifest.snapshot.anchor.storage_root {} is not the \
             root in the proof it points at ({})",
            anchor.storage_root,
            strk20_feed::felt_hex(&proof_root)
        );
    }
    let proof_block_hash = sidecar["global_roots"]["block_hash"]
        .as_str()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "SNAPSHOT_ROOT_MISMATCH: {file} carries no global_roots.block_hash, so the \
                 proof cannot be bound to a block at all"
            )
        })?;
    if strk20_feed::felt_from_hex(proof_block_hash)?
        != strk20_feed::felt_from_hex(&anchor.block_hash)?
    {
        bail!(
            "SNAPSHOT_ROOT_MISMATCH: manifest.snapshot.anchor.block_hash {} is not the \
             block hash the published proof is bound to ({proof_block_hash})",
            anchor.block_hash
        );
    }
    tracing::info!(
        block = snap.header.block,
        "the published basis-block proof agrees with this snapshot's slot set (§12 point 1); \
         the §11.3 reachability walk still runs"
    );
    Ok(())
}
