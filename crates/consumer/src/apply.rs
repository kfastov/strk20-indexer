//! Decode and fold the public feed. Hashes detect feed corruption; only an
//! independently authenticated checkpoint establishes chain completeness.
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
use anyhow::{bail, Result};
use starknet_types_core::felt::Felt;
use strk20_feed::codec::{self, BlockLine};
use strk20_feed::manifest::Manifest;
use strk20_feed::snapshot::{FeedIdentity, SnapSlot, Snapshot};

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
    apply_feed_once(store, transport, cold_start).await
}

async fn apply_feed_once<S: ConsumerStore>(
    store: &S,
    transport: &dyn FeedTransport,
    cold_start: ColdStart,
) -> Result<ApplyOutcome> {
    let mut out = ApplyOutcome::default();
    let genesis = transport.fetch_genesis().await?;
    match store.meta_get("pool")? {
        None => {
            store.meta_set("pool", &genesis.pool)?;
            store.meta_set("chain_id", &genesis.chain_id)?;
            store.meta_set("epoch_size", &genesis.epoch_size.to_string())?;
            store.meta_set("genesis_block", &genesis.genesis_block.to_string())?;
        }
        Some(stored) if stored != genesis.pool => {
            bail!(
                "feed pool {} does not match local mirror {}",
                genesis.pool,
                stored
            )
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
    if cold_start != ColdStart::Epochs && store.is_empty()? {
        match manifest.snapshot.clone() {
            Some(entry) => {
                cold_start_from_snapshot(store, transport, &manifest, &entry, &genesis, &feed_pool)
                    .await?;
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
        let blocks: Vec<(&BlockLine, bool)> = epoch.blocks.iter().map(|b| (b, true)).collect();
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
        let contradiction =
            stored
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
    Ok(strk20_feed::snapshot::decode_snapshot_payload(
        &payload,
        &zst_sha256,
        entry,
        basis_epoch_hash,
        expect,
    )?)
}

/// Fold snapshot slots, retaining publisher-supplied write metadata for
/// historical views. A final state root does not authenticate those times.
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
        ],
    )
}
