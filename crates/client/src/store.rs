//! FeedStore (spec §7.3): the client's verified local mirror in sync.db.
//! Applies content-addressed epochs (full hash-chain verification) and the
//! head tail; exposes the two raw discovery-core traits so the UNMODIFIED
//! engine runs on top. Contains SecretFelt-derived cursor material — the DB
//! file is chmod 0600 and never leaves the machine.

use anyhow::{bail, Context, Result};
use async_trait::async_trait;
use discovery_core::events_backend::RawEventAccess;
use discovery_core::storage_backend::{RawStorageAccess, StorageError};
use rusqlite::{params, Connection, OptionalExtension};
#[allow(unused_imports)]
use anyhow::Context as _;
use starknet_core::types::{BlockId, EmittedEvent, StorageResult};
use starknet_types_core::felt::Felt;
use std::path::Path;
use std::sync::{Arc, Mutex};
use strk20_feed::codec::{self, BlockLine};
use strk20_feed::manifest::Manifest;

const DDL: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
  key TEXT PRIMARY KEY, value TEXT NOT NULL
) WITHOUT ROWID;
CREATE TABLE IF NOT EXISTS blocks (
  number INTEGER PRIMARY KEY, hash BLOB NOT NULL, parent_hash BLOB NOT NULL,
  timestamp INTEGER NOT NULL, finality INTEGER NOT NULL
);
CREATE TABLE IF NOT EXISTS storage_log (
  slot BLOB NOT NULL, block INTEGER NOT NULL, value BLOB NOT NULL,
  PRIMARY KEY (slot, block)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS sl_block ON storage_log(block);
CREATE TABLE IF NOT EXISTS events (
  block INTEGER NOT NULL, event_index INTEGER NOT NULL, tx_index INTEGER NOT NULL,
  tx_hash BLOB NOT NULL, key0 BLOB NOT NULL, key1 BLOB,
  keys BLOB NOT NULL, data BLOB NOT NULL,
  PRIMARY KEY (block, event_index)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS ev_key1 ON events(key1) WHERE key1 IS NOT NULL;
CREATE TABLE IF NOT EXISTS notes_registry (
  note_id BLOB PRIMARY KEY,
  owner BLOB NOT NULL, sender BLOB NOT NULL, token BLOB NOT NULL,
  idx INTEGER NOT NULL, nullifier BLOB NOT NULL, amount TEXT NOT NULL,
  block INTEGER NOT NULL, spent INTEGER NOT NULL DEFAULT 0
) WITHOUT ROWID;
"#;

fn fb(f: &Felt) -> [u8; 32] {
    f.to_bytes_be()
}

fn bf(b: &[u8]) -> Felt {
    Felt::from_bytes_be(&b.try_into().expect("32-byte felt"))
}

fn felts_blob(fs: &[Felt]) -> Vec<u8> {
    let mut out = Vec::with_capacity(fs.len() * 32);
    for f in fs {
        out.extend_from_slice(&f.to_bytes_be());
    }
    out
}

fn blob_felts(b: &[u8]) -> Vec<Felt> {
    b.chunks_exact(32).map(bf).collect()
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NoteRow {
    pub note_id: Felt,
    pub owner: Felt,
    pub sender: Felt,
    pub token: Felt,
    pub index: u64,
    pub nullifier: Felt,
    pub amount: u128,
    pub block: u64,
    pub spent: bool,
}

pub struct FeedStore {
    conn: Arc<Mutex<Connection>>,
}

#[derive(Debug, Default, Clone, Copy)]
pub struct ApplyOutcome {
    pub epochs_applied: u64,
    pub tail_rewound: bool,
    pub tail_changed: bool,
    pub head: u64,
    pub l1_accepted: u64,
    /// end block of the newest applied epoch (L1-final checkpoint)
    pub last_epoch_to: u64,
    /// basis block of the snapshot this mirror was cold-started from
    pub snapshot_basis: Option<u64>,
    /// a snapshot was offered, failed verification, and `auto` fell back
    pub snapshot_rejected: bool,
    /// lowest block for which this mirror holds EVENTS (§1.1)
    pub history_floor: u64,
}

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

/// How an empty mirror is populated (§1.7). `auto` is the default: the
/// snapshot branch, with the C13 fallback to full epoch replay when the
/// snapshot cannot be grounded.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ColdStart {
    #[default]
    Auto,
    Snapshot,
    Epochs,
}

impl FeedStore {
    pub fn open(path: &Path) -> Result<Self> {
        // 0600 must be in place BEFORE SQLite creates -wal/-shm: those files
        // inherit the main db's mode, and cursor material (SecretFelt-derived
        // channel keys) lands in the WAL (review finding: sync.db-wal 0644).
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if !path.exists() {
                std::fs::File::create(path)
                    .with_context(|| format!("create {}", path.display()))?;
            }
            for suffix in ["", "-wal", "-shm"] {
                let target = if suffix.is_empty() {
                    path.to_path_buf()
                } else {
                    let mut os = path.as_os_str().to_owned();
                    os.push(suffix);
                    std::path::PathBuf::from(os)
                };
                if target.exists() {
                    std::fs::set_permissions(&target, std::fs::Permissions::from_mode(0o600))
                        .with_context(|| format!("chmod {}", target.display()))?;
                }
            }
        }
        let conn = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.execute_batch(DDL)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for suffix in ["-wal", "-shm"] {
                let mut os = path.as_os_str().to_owned();
                os.push(suffix);
                let target = std::path::PathBuf::from(os);
                if target.exists() {
                    let _ = std::fs::set_permissions(
                        &target,
                        std::fs::Permissions::from_mode(0o600),
                    );
                }
            }
        }
        Ok(Self {
            conn: Arc::new(Mutex::new(conn)),
        })
    }

    pub fn meta_get(&self, key: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().expect("conn");
        Ok(conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    pub fn meta_set(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.conn.lock().expect("conn");
        conn.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    fn apply_block_line(conn: &Connection, b: &BlockLine, finality: i64) -> Result<()> {
        conn.execute(
            "INSERT OR REPLACE INTO blocks(number, hash, parent_hash, timestamp, finality)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                b.number as i64,
                fb(&b.hash).as_slice(),
                fb(&b.parent).as_slice(),
                b.timestamp as i64,
                finality
            ],
        )?;
        for (slot, value) in &b.diffs {
            conn.execute(
                "INSERT OR REPLACE INTO storage_log(slot, block, value) VALUES (?1, ?2, ?3)",
                params![fb(slot).as_slice(), b.number as i64, fb(value).as_slice()],
            )?;
        }
        for e in &b.events {
            let key0 = e.keys.first().map(fb).unwrap_or([0u8; 32]);
            let key1 = e.keys.get(1).map(fb);
            conn.execute(
                "INSERT OR REPLACE INTO events(block, event_index, tx_index, tx_hash, key0, key1, keys, data)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    b.number as i64,
                    e.event_index as i64,
                    e.tx_index as i64,
                    fb(&e.tx_hash).as_slice(),
                    key0.as_slice(),
                    key1.as_ref().map(|k| k.as_slice()),
                    felts_blob(&e.keys),
                    felts_blob(&e.data)
                ],
            )?;
        }
        Ok(())
    }

    /// Is this mirror still unpopulated? Only then may a snapshot be applied:
    /// a snapshot is a floor, and laying one under existing rows would create
    /// a mirror whose history floor contradicts what it already holds.
    pub fn is_empty(&self) -> Result<bool> {
        if self.meta_get("last_epoch_applied")?.is_some() {
            return Ok(false);
        }
        let conn = self.conn.lock().expect("conn");
        let rows: i64 = conn.query_row(
            "SELECT (SELECT COUNT(*) FROM blocks) + (SELECT COUNT(*) FROM storage_log)",
            [],
            |r| r.get(0),
        )?;
        Ok(rows == 0)
    }

    /// Drop everything the feed put here and return to the pre-sync state, so
    /// the C13 fallback replays epochs into a mirror with no snapshot rows
    /// left under it. Identity metadata (pool, chain id, epoch size) survives:
    /// it is what a re-sync is checked against.
    pub fn reset_mirror(&self) -> Result<()> {
        let mut guard = self.conn.lock().expect("conn");
        let tx = guard.transaction()?;
        for table in ["storage_log", "events", "blocks", "notes_registry"] {
            tx.execute(&format!("DELETE FROM {table}"), [])?;
        }
        tx.execute(
            "DELETE FROM meta WHERE key IN ('last_epoch_applied','last_epoch_hash',
             'last_epoch_to','head_etag','head_number','head_hash','l1_accepted',
             'snapshot_basis','history_floor','snapshot_pending_grounding')",
            [],
        )?;
        tx.commit()?;
        Ok(())
    }

    /// Fold a verified snapshot into the mirror (§1.7). No new tables: slot
    /// rows land in `storage_log` at their REAL write blocks, so the shipped
    /// as-of query serves them and `read_slots_with_block` returns the exact
    /// `last_update_block` a note's `block_number` is derived from.
    fn apply_snapshot(&self, snap: &strk20_feed::snapshot::Snapshot) -> Result<()> {
        let mut guard = self.conn.lock().expect("conn");
        let tx = guard.transaction()?;
        {
            let mut ins = tx.prepare(
                "INSERT OR REPLACE INTO storage_log(slot, block, value) VALUES (?1, ?2, ?3)",
            )?;
            for s in &snap.slots {
                ins.execute(params![
                    fb(&s.k).as_slice(),
                    s.w as i64,
                    fb(&s.v).as_slice()
                ])?;
            }
        }
        let h = &snap.header;
        meta_set_tx(&tx, "last_epoch_applied", &h.epoch.to_string())?;
        meta_set_tx(&tx, "last_epoch_hash", &h.epoch_hash)?;
        meta_set_tx(&tx, "last_epoch_to", &h.block.to_string())?;
        // Made loud (§1.1): a snapshot carries slots and no events, so no
        // event below this block exists locally and none ever will.
        meta_set_tx(&tx, "history_floor", &(h.block + 1).to_string())?;
        meta_set_tx(&tx, "snapshot_basis", &h.block.to_string())?;
        // Committed BEFORE the §11.3 grounding can run — the epochs above the
        // basis and the head tail have to land first, because reachability
        // validates them too. Anything that ends the process inside that
        // window (a rejection the operator re-runs past, Ctrl-C, an OOM kill)
        // would otherwise leave a mirror that is never checked again: the next
        // sync sees a non-empty mirror, skips the snapshot branch entirely and
        // never calls the grounding at all. The flag makes that state
        // recognisable, and it is cleared only by a grounding that passed.
        meta_set_tx(&tx, "snapshot_pending_grounding", "1")?;
        tx.commit()?;
        Ok(())
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
    async fn check_reachability(
        &self,
        transport: &dyn crate::transport::FeedTransport,
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
            let local =
                strk20_feed::mpt::storage_root(&self.full_slot_set_as_of(anchor.block)?);
            if local != anchor.storage_root {
                failures.push(format!(
                    "block {}: folded root {} != published {}",
                    anchor.block,
                    strk20_feed::felt_hex(&local),
                    strk20_feed::felt_hex(&anchor.storage_root)
                ));
                continue;
            }
            if let Some(stored) = self.block_hash(anchor.block)? {
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

    /// Apply verified epochs + the head tail per the manifest. This is the
    /// whole trust pipeline of the client: any hash mismatch is a hard error
    /// naming the epoch and both hashes (U5 divergence detection).
    ///
    /// Reorg discipline (review findings): a newly applied epoch SUPERSEDES
    /// any stored rows in its range (tail rows from a reorged-away chain must
    /// not survive under the new floor — the "masked reorg"), and every tail
    /// replacement bumps a persisted `tail_generation` in the SAME
    /// transaction as the rebuild, so per-owner cursor rewinds survive
    /// crashes and shared-db multi-owner use.
    pub async fn apply_feed(
        &self,
        transport: &dyn crate::transport::FeedTransport,
        cold_start: ColdStart,
    ) -> Result<ApplyOutcome> {
        // A mirror carrying snapshot rows that were never grounded is exactly
        // as trustworthy as one whose grounding failed, and it must not be
        // built on: without this, a rejected snapshot's rows survive the failed
        // run, the next run sees a non-empty mirror, skips the snapshot branch
        // and therefore skips the grounding, and the client ends up permanently
        // on a slot set it explicitly refused once.
        if self.meta_get("snapshot_pending_grounding")?.as_deref() == Some("1") {
            tracing::warn!(
                "this mirror holds a snapshot whose §11.3 grounding never completed; \
                 discarding it rather than building on an unverified slot set"
            );
            self.reset_mirror()?;
        }
        match self.apply_feed_once(transport, cold_start, false).await {
            Ok(out) => Ok(out),
            Err(e) if e.downcast_ref::<SnapshotRejected>().is_some() => {
                // Whatever the mode, the refused snapshot's rows go: leaving
                // them is what turns one rejection into a permanently poisoned
                // db.
                self.reset_mirror()?;
                // C13: under `auto` a snapshot that cannot be verified or
                // grounded costs a full replay, never the sync.
                if cold_start == ColdStart::Auto {
                    tracing::warn!(
                        error = %format!("{e:#}"),
                        "snapshot rejected; falling back to full epoch replay"
                    );
                    return self.apply_feed_once(transport, ColdStart::Epochs, true).await;
                }
                Err(e)
            }
            Err(e) => Err(e),
        }
    }

    async fn apply_feed_once(
        &self,
        transport: &dyn crate::transport::FeedTransport,
        cold_start: ColdStart,
        snapshot_rejected: bool,
    ) -> Result<ApplyOutcome> {
        let mut out = ApplyOutcome {
            snapshot_rejected,
            ..Default::default()
        };
        let genesis = transport.fetch_genesis().await?;
        match self.meta_get("pool")? {
            None => {
                self.meta_set("pool", &genesis.pool)?;
                self.meta_set("chain_id", &genesis.chain_id)?;
                self.meta_set("epoch_size", &genesis.epoch_size.to_string())?;
                self.meta_set("genesis_block", &genesis.genesis_block.to_string())?;
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
        if let Some(stored) = self.meta_get("chain_id")? {
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
        if cold_start != ColdStart::Epochs && self.is_empty()? {
            match manifest.snapshot.clone() {
                Some(entry) => {
                    let snap = self
                        .cold_start_from_snapshot(
                            transport, &manifest, &entry, &genesis, &feed_pool,
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
        let mut last_applied: Option<u64> = self
            .meta_get("last_epoch_applied")?
            .and_then(|s| s.parse().ok());
        let mut prev_hash: Option<[u8; 32]> = match self.meta_get("last_epoch_hash")? {
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
            let payload = strk20_feed::decompress_capped(
                &compressed,
                strk20_feed::MAX_DECOMPRESSED,
                &format!("epoch {}", entry.e),
            )?;
            let epoch =
                strk20_feed::manifest::verify_epoch_against_manifest(&payload, entry, prev_hash)?;
            // The hash chain proves this is the epoch the manifest names; the
            // binding check proves the manifest is about the chain and pool we
            // believe we are mirroring.
            strk20_feed::manifest::verify_epoch_binding(
                &epoch,
                &genesis.chain_id,
                &feed_pool,
            )?;
            {
                let mut guard = self.conn.lock().expect("conn");
                let tx = guard.transaction()?;
                // Masked-reorg check: stored (tail) rows inside this epoch's
                // range that contradict the L1-final epoch content mean the
                // old tail was replaced while we were not looking.
                let contradiction = {
                    let mut stmt = tx.prepare(
                        "SELECT number, hash FROM blocks WHERE number BETWEEN ?1 AND ?2",
                    )?;
                    let stored: Vec<(u64, Felt)> = stmt
                        .query_map(params![entry.from as i64, entry.to as i64], |r| {
                            Ok((r.get::<_, i64>(0)? as u64, bf(&r.get::<_, Vec<u8>>(1)?)))
                        })?
                        .collect::<std::result::Result<Vec<_>, _>>()?;
                    stored.iter().any(|(n, h)| {
                        !epoch.blocks.iter().any(|b| b.number == *n && b.hash == *h)
                    })
                };
                // The epoch supersedes everything in its range.
                tx.execute(
                    "DELETE FROM storage_log WHERE block BETWEEN ?1 AND ?2",
                    params![entry.from as i64, entry.to as i64],
                )?;
                tx.execute(
                    "DELETE FROM events WHERE block BETWEEN ?1 AND ?2",
                    params![entry.from as i64, entry.to as i64],
                )?;
                tx.execute(
                    "DELETE FROM blocks WHERE number BETWEEN ?1 AND ?2",
                    params![entry.from as i64, entry.to as i64],
                )?;
                for b in &epoch.blocks {
                    Self::apply_block_line(&tx, b, 1)?;
                }
                let hash_hex = hex::encode(strk20_feed::payload_sha256(&payload));
                meta_set_tx(&tx, "last_epoch_applied", &entry.e.to_string())?;
                meta_set_tx(&tx, "last_epoch_hash", &hash_hex)?;
                meta_set_tx(&tx, "last_epoch_to", &entry.to.to_string())?;
                if contradiction {
                    bump_generation_tx(&tx)?;
                    out.tail_rewound = true;
                }
                tx.commit()?;
            }
            prev_hash = Some(strk20_feed::payload_sha256(&payload));
            last_applied = Some(entry.e);
            out.epochs_applied += 1;
        }
        out.last_epoch_to = self
            .meta_get("last_epoch_to")?
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);

        // 2. head tail
        let etag = self.meta_get("head_etag")?;
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
            {
                let mut guard = self.conn.lock().expect("conn");
                let tx = guard.transaction()?;
                // Reorg rule (spec §7.5): stored tail rows that contradict
                // the new tail file mean the old tail was replaced.
                let contradiction = {
                    let mut stmt = tx.prepare(
                        "SELECT number, hash FROM blocks WHERE number > ?1 ORDER BY number",
                    )?;
                    let stored: Vec<(u64, Felt)> = stmt
                        .query_map([out.last_epoch_to as i64], |r| {
                            Ok((r.get::<_, i64>(0)? as u64, bf(&r.get::<_, Vec<u8>>(1)?)))
                        })?
                        .collect::<std::result::Result<Vec<_>, _>>()?;
                    stored.iter().any(|(n, h)| {
                        match head.blocks.iter().find(|b| b.number == *n) {
                            Some(b) => b.hash != *h,
                            None => *n >= head.header.tail_from,
                        }
                    })
                };
                // rebuild the tail wholesale above the epoch floor
                tx.execute(
                    "DELETE FROM storage_log WHERE block > ?1",
                    [out.last_epoch_to as i64],
                )?;
                tx.execute(
                    "DELETE FROM events WHERE block > ?1",
                    [out.last_epoch_to as i64],
                )?;
                tx.execute(
                    "DELETE FROM blocks WHERE number > ?1",
                    [out.last_epoch_to as i64],
                )?;
                for b in &head.blocks {
                    let fin = match b.finality {
                        Some(codec::Finality::L1) => 1,
                        _ => 0,
                    };
                    Self::apply_block_line(&tx, b, fin)?;
                }
                meta_set_tx(&tx, "head_etag", &new_etag)?;
                meta_set_tx(&tx, "head_number", &head.header.head.to_string())?;
                meta_set_tx(
                    &tx,
                    "head_hash",
                    &strk20_feed::felt_hex(&head.header.head_hash),
                )?;
                meta_set_tx(&tx, "l1_accepted", &head.header.l1_accepted.to_string())?;
                if contradiction {
                    bump_generation_tx(&tx)?;
                    out.tail_rewound = true;
                }
                tx.commit()?;
            }
            out.tail_changed = true;
            out.head = head.header.head;
            out.l1_accepted = head.header.l1_accepted;
        } else {
            out.head = self
                .meta_get("head_number")?
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
            out.l1_accepted = self
                .meta_get("l1_accepted")?
                .and_then(|s| s.parse().ok())
                .unwrap_or(0);
        }

        // 3. grounding. Only meaningful for a mirror that was cold-started
        // from a snapshot in THIS call: the epochs above the basis and the
        // head tail had to land first, because reachability validates them
        // too.
        if let Some(basis) = fresh_basis {
            let anchor = self
                .check_reachability(transport, basis, out.head)
                .await
                .map_err(|e| e.context(SnapshotRejected))?;
            // Only a grounding that passed may clear the flag.
            self.meta_set("snapshot_pending_grounding", "0")?;
            tracing::info!(
                basis,
                anchor,
                "snapshot reachability verified against the published anchors log"
            );
        }
        out.snapshot_basis = self
            .meta_get("snapshot_basis")?
            .and_then(|s| s.parse().ok());
        out.history_floor = self
            .meta_get("history_floor")?
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        Ok(out)
    }

    /// Rings 1-5 of §1.5, then fold the slot set. Reachability (§11.3) runs
    /// later, once the feed above the basis has been applied.
    async fn cold_start_from_snapshot(
        &self,
        transport: &dyn crate::transport::FeedTransport,
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
        let snap = strk20_feed::snapshot::verify_snapshot(
            &compressed,
            entry,
            &basis_epoch_hash,
            &strk20_feed::snapshot::FeedIdentity {
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
        self.apply_snapshot(&snap)?;
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

    /// Complete pool slot set as of `block` (latest value per slot, zero-value
    /// rows excluded) — the input the client folds into a storage root when
    /// checking a published anchor.
    pub fn full_slot_set_as_of(&self, block: u64) -> Result<Vec<(Felt, Felt)>> {
        let conn = self.conn.lock().expect("conn");
        let mut stmt = conn.prepare(
            "SELECT slot, value FROM storage_log s
             WHERE block = (SELECT MAX(block) FROM storage_log
                            WHERE slot = s.slot AND block <= ?1)",
        )?;
        let rows = stmt
            .query_map([block as i64], |r| {
                Ok((bf(&r.get::<_, Vec<u8>>(0)?), bf(&r.get::<_, Vec<u8>>(1)?)))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows.into_iter().filter(|(_, v)| *v != Felt::ZERO).collect())
    }

    /// Hash of the mirrored block `number`, if the mirror holds it.
    pub fn block_hash(&self, number: u64) -> Result<Option<Felt>> {
        let conn = self.conn.lock().expect("conn");
        Ok(conn
            .query_row(
                "SELECT hash FROM blocks WHERE number = ?1",
                [number as i64],
                |r| r.get::<_, Vec<u8>>(0),
            )
            .optional()?
            .map(|b| bf(&b)))
    }

    /// Persisted tail-replacement counter (crash-safe, shared-db-safe).
    pub fn tail_generation(&self) -> Result<u64> {
        Ok(self
            .meta_get("tail_generation")?
            .and_then(|s| s.parse().ok())
            .unwrap_or(0))
    }

    /// Lowest block for which this mirror holds EVENTS (§1.1). 0 for a fully
    /// epoch-replayed mirror.
    pub fn history_floor(&self) -> Result<u64> {
        Ok(self
            .meta_get("history_floor")?
            .and_then(|s| s.parse().ok())
            .unwrap_or(0))
    }

    /// A read view bound to `block` for the discovery engine.
    ///
    /// §1.5.2 guard rail: a bound below the snapshot basis is refused rather
    /// than served. Pre-basis state does not exist locally and must never be
    /// answered with zeros. Engine bounds are always `last_epoch_to` or `head`,
    /// both at or above the basis — the rule exists so a future refactor cannot
    /// introduce a silent zero-read.
    pub fn view(&self, block: u64) -> Result<ClientView> {
        let basis: Option<u64> = self
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
        Ok(ClientView {
            conn: self.conn.clone(),
            bound: block,
            history_floor: self.history_floor()?,
        })
    }

    // -------------------------------------------------------- notes registry

    pub fn upsert_note(&self, n: &NoteRow) -> Result<()> {
        let conn = self.conn.lock().expect("conn");
        conn.execute(
            "INSERT INTO notes_registry(note_id, owner, sender, token, idx, nullifier, amount, block, spent)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)
             ON CONFLICT(note_id) DO UPDATE SET
               sender = excluded.sender, amount = excluded.amount,
               block = excluded.block",
            params![
                fb(&n.note_id).as_slice(),
                fb(&n.owner).as_slice(),
                fb(&n.sender).as_slice(),
                fb(&n.token).as_slice(),
                n.index as i64,
                fb(&n.nullifier).as_slice(),
                n.amount.to_string(),
                n.block as i64,
                n.spent as i64
            ],
        )?;
        Ok(())
    }

    /// Drop registry rows whose note slot no longer exists in the mirror —
    /// the precise reorg cleanup (covers both direct and masked tail
    /// replacements; a canonical note re-added by the new tail/epoch is
    /// rediscovered by the next engine pass).
    pub fn prune_missing_notes(&self, owner: &Felt, as_of: u64) -> Result<usize> {
        let notes = self.notes(owner)?;
        let conn = self.conn.lock().expect("conn");
        let mut pruned = 0;
        for n in notes {
            let slot = discovery_core::privacy_pool::storage_slots::notes(n.note_id);
            let value: Option<Vec<u8>> = conn
                .query_row(
                    "SELECT value FROM storage_log WHERE slot = ?1 AND block <= ?2
                     ORDER BY block DESC LIMIT 1",
                    params![fb(&slot).as_slice(), as_of as i64],
                    |r| r.get(0),
                )
                .optional()?;
            let exists = value.map(|v| bf(&v) != Felt::ZERO).unwrap_or(false);
            if !exists {
                conn.execute(
                    "DELETE FROM notes_registry WHERE note_id = ?1",
                    [fb(&n.note_id).as_slice()],
                )?;
                pruned += 1;
            }
        }
        Ok(pruned)
    }

    pub fn delete_owner_notes(&self, owner: &Felt) -> Result<usize> {
        let conn = self.conn.lock().expect("conn");
        Ok(conn.execute(
            "DELETE FROM notes_registry WHERE owner = ?1",
            [fb(owner).as_slice()],
        )?)
    }

    pub fn notes(&self, owner: &Felt) -> Result<Vec<NoteRow>> {
        let conn = self.conn.lock().expect("conn");
        let mut stmt = conn.prepare(
            "SELECT note_id, owner, sender, token, idx, nullifier, amount, block, spent
             FROM notes_registry WHERE owner = ?1 ORDER BY token, idx",
        )?;
        let rows = stmt
            .query_map([fb(owner).as_slice()], |r| {
                Ok(NoteRow {
                    note_id: bf(&r.get::<_, Vec<u8>>(0)?),
                    owner: bf(&r.get::<_, Vec<u8>>(1)?),
                    sender: bf(&r.get::<_, Vec<u8>>(2)?),
                    token: bf(&r.get::<_, Vec<u8>>(3)?),
                    index: r.get::<_, i64>(4)? as u64,
                    nullifier: bf(&r.get::<_, Vec<u8>>(5)?),
                    amount: r.get::<_, String>(6)?.parse().unwrap_or(0),
                    block: r.get::<_, i64>(7)? as u64,
                    spent: r.get::<_, i64>(8)? != 0,
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Re-evaluate spent-state from the mirror (nullifier slot != 0 as of
    /// `block`). Returns nullifiers that flipped to spent.
    pub fn refresh_spent(&self, owner: &Felt, block: u64) -> Result<Vec<Felt>> {
        let notes = self.notes(owner)?;
        let mut flipped = Vec::new();
        let conn = self.conn.lock().expect("conn");
        for n in notes {
            let slot = discovery_core::privacy_pool::storage_slots::nullifiers(n.nullifier);
            let value: Option<Vec<u8>> = conn
                .query_row(
                    "SELECT value FROM storage_log WHERE slot = ?1 AND block <= ?2
                     ORDER BY block DESC LIMIT 1",
                    params![fb(&slot).as_slice(), block as i64],
                    |r| r.get(0),
                )
                .optional()?;
            let is_spent = value.map(|v| bf(&v) != Felt::ZERO).unwrap_or(false);
            if is_spent != n.spent {
                conn.execute(
                    "UPDATE notes_registry SET spent = ?1 WHERE note_id = ?2",
                    params![is_spent as i64, fb(&n.note_id).as_slice()],
                )?;
                if is_spent {
                    flipped.push(n.nullifier);
                }
            }
        }
        Ok(flipped)
    }
}

/// Engine-facing read view bound to one block.
#[derive(Clone, Debug)]
pub struct ClientView {
    conn: Arc<Mutex<Connection>>,
    bound: u64,
    /// Lowest block for which the `events` table can answer (§1.1). Below it
    /// the table is not empty-because-nothing-happened, it is empty because a
    /// snapshot carries slots and no events.
    history_floor: u64,
}

impl ClientView {
    pub fn bound(&self) -> u64 {
        self.bound
    }

    pub fn history_floor(&self) -> u64 {
        self.history_floor
    }

    fn read_one(&self, slot: &Felt) -> anyhow::Result<(Felt, u64)> {
        let conn = self.conn.lock().expect("conn");
        let row: Option<(Vec<u8>, i64)> = conn
            .query_row(
                "SELECT value, block FROM storage_log WHERE slot = ?1 AND block <= ?2
                 ORDER BY block DESC LIMIT 1",
                params![fb(slot).as_slice(), self.bound as i64],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(match row {
            Some((v, b)) => (bf(&v), b as u64),
            None => (Felt::ZERO, 0),
        })
    }
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
    transport: &dyn crate::transport::FeedTransport,
    entry: &strk20_feed::manifest::ManifestSnapshot,
    snap: &strk20_feed::snapshot::Snapshot,
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

fn meta_set_tx(tx: &rusqlite::Transaction<'_>, key: &str, value: &str) -> Result<()> {
    tx.execute(
        "INSERT INTO meta(key, value) VALUES (?1, ?2)
         ON CONFLICT(key) DO UPDATE SET value = excluded.value",
        params![key, value],
    )?;
    Ok(())
}

fn bump_generation_tx(tx: &rusqlite::Transaction<'_>) -> Result<()> {
    tx.execute(
        "INSERT INTO meta(key, value) VALUES ('tail_generation', '1')
         ON CONFLICT(key) DO UPDATE SET value = CAST(CAST(value AS INTEGER) + 1 AS TEXT)",
        [],
    )?;
    Ok(())
}

fn client_err(e: anyhow::Error) -> StorageError {
    StorageError::Backend(e.into())
}

#[async_trait]
impl RawStorageAccess for ClientView {
    async fn read_slot(&self, slot: Felt) -> Result<Felt, StorageError> {
        let this = self.clone();
        tokio::task::spawn_blocking(move || Ok(this.read_one(&slot).map_err(client_err)?.0))
            .await
            .map_err(|e| StorageError::Backend(Box::new(e)))?
    }

    async fn read_slots(&self, slots: Vec<Felt>) -> Result<Vec<Felt>, StorageError> {
        let this = self.clone();
        tokio::task::spawn_blocking(move || {
            slots
                .iter()
                .map(|s| Ok(this.read_one(s).map_err(client_err)?.0))
                .collect()
        })
        .await
        .map_err(|e| StorageError::Backend(Box::new(e)))?
    }

    async fn read_slots_with_block(
        &self,
        slots: Vec<Felt>,
    ) -> Result<Vec<StorageResult>, StorageError> {
        let this = self.clone();
        tokio::task::spawn_blocking(move || {
            slots
                .iter()
                .map(|s| {
                    let (value, wb) = this.read_one(s).map_err(client_err)?;
                    Ok(StorageResult {
                        value,
                        last_update_block: wb,
                    })
                })
                .collect()
        })
        .await
        .map_err(|e| StorageError::Backend(Box::new(e)))?
    }
}

#[async_trait]
impl RawEventAccess for ClientView {
    async fn get_events(
        &self,
        keys: &[Vec<Felt>],
        from_block: BlockId,
        to_block: BlockId,
    ) -> Result<Vec<EmittedEvent>, StorageError> {
        let filters: Vec<Vec<Felt>> = keys.to_vec();
        let this = self.clone();
        let num = |id: BlockId, default: u64| match id {
            BlockId::Number(n) => n,
            _ => default,
        };
        let from = num(from_block, 0);
        let to = num(to_block, self.bound).min(self.bound);
        // R-L: a range reaching below the floor is a HARD ERROR, never a
        // clamped or silently truncated answer. On a snapshot-started mirror
        // the `events` table is simply absent below `basis + 1`, so returning
        // the above-floor part with a success status is indistinguishable from
        // "nothing happened down there" — the masked incompleteness R-L exists
        // to forbid. The caller is told the floor so it can ask again above it.
        if from < self.history_floor {
            let floor = self.history_floor;
            return Err(StorageError::Backend(
                anyhow::anyhow!(
                    "HISTORY_UNAVAILABLE {{\"floor\": {floor}}}: this mirror was \
                     cold-started from a snapshot and holds no events below block \
                     {floor}; the requested range starts at {from}. Full history requires \
                     --cold-start epochs."
                )
                .into(),
            ));
        }
        tokio::task::spawn_blocking(move || {
            let conn = this.conn.lock().expect("conn");
            let mut stmt = conn
                .prepare(
                    "SELECT e.block, e.event_index, e.tx_index, e.tx_hash, e.keys, e.data, b.hash
                     FROM events e JOIN blocks b ON b.number = e.block
                     WHERE e.block BETWEEN ?1 AND ?2 ORDER BY e.block, e.event_index",
                )
                .map_err(|e| client_err(e.into()))?;
            let rows = stmt
                .query_map(params![from as i64, to as i64], |r| {
                    Ok((
                        r.get::<_, i64>(0)? as u64,
                        r.get::<_, i64>(1)? as u64,
                        r.get::<_, i64>(2)? as u64,
                        r.get::<_, Vec<u8>>(3)?,
                        r.get::<_, Vec<u8>>(4)?,
                        r.get::<_, Vec<u8>>(5)?,
                        r.get::<_, Vec<u8>>(6)?,
                    ))
                })
                .map_err(|e| client_err(e.into()))?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(|e| client_err(e.into()))?;
            let mut out = Vec::new();
            for (block, event_index, tx_index, tx_hash, keys_b, data_b, bhash) in rows {
                let keys = blob_felts(&keys_b);
                let matched = filters.iter().enumerate().all(|(i, allowed)| {
                    allowed.is_empty()
                        || keys.get(i).map(|k| allowed.contains(k)).unwrap_or(false)
                });
                if !matched {
                    continue;
                }
                out.push(EmittedEvent {
                    from_address: Felt::ZERO, // single-contract feed: the pool
                    keys,
                    data: blob_felts(&data_b),
                    block_hash: Some(bf(&bhash)),
                    block_number: Some(block),
                    transaction_hash: bf(&tx_hash),
                    event_index,
                    transaction_index: tx_index,
                });
            }
            Ok(out)
        })
        .await
        .map_err(|e| StorageError::Backend(Box::new(e)))?
    }

    fn block_id(&self) -> BlockId {
        BlockId::Number(self.bound)
    }

    fn block_number(&self) -> u64 {
        self.bound
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot_started(basis: u64) -> (tempfile::TempDir, FeedStore) {
        let dir = tempfile::tempdir().expect("tempdir");
        let store = FeedStore::open(&dir.path().join("sync.db")).expect("open");
        store.meta_set("snapshot_basis", &basis.to_string()).unwrap();
        store.meta_set("history_floor", &(basis + 1).to_string()).unwrap();
        (dir, store)
    }

    /// §1.1 / R-L: below the floor the `events` table is empty because a
    /// snapshot carries no events, not because nothing happened. Answering
    /// such a range with a truncated set and a success status is the masked
    /// incompleteness R-L exists to forbid — before this guard, a query over
    /// `[0, head]` returned only the above-floor events and reported success,
    /// which is indistinguishable from "there were no events down there".
    #[tokio::test]
    async fn a_history_read_below_the_floor_is_a_hard_error_naming_the_floor() {
        let (_dir, store) = snapshot_started(31);
        let view = store.view(46).expect("bound above the basis");
        assert_eq!(view.history_floor(), 32);

        let err = view
            .get_events(&[], BlockId::Number(0), BlockId::Number(46))
            .await
            .expect_err("a range starting below the floor must not succeed");
        let text = format!("{err}");
        assert!(
            text.contains("HISTORY_UNAVAILABLE") && text.contains("32"),
            "the error must be HISTORY_UNAVAILABLE and name the floor: {text}"
        );

        // The defaulted lower bound counts too: a non-numeric `from` means 0,
        // which is below the floor.
        let err = view
            .get_events(
                &[],
                BlockId::Tag(starknet_core::types::BlockTag::Latest),
                BlockId::Number(46),
            )
            .await
            .expect_err("a defaulted lower bound must not slip past the floor");
        assert!(format!("{err}").contains("HISTORY_UNAVAILABLE"));

        // At or above the floor the read is served normally.
        let ok = view
            .get_events(&[], BlockId::Number(32), BlockId::Number(46))
            .await
            .expect("a range fully above the floor must be served");
        assert!(ok.is_empty(), "no events were inserted in this fixture");
    }

    /// A fully epoch-replayed mirror has floor 0, so nothing is refused.
    #[tokio::test]
    async fn a_replayed_mirror_has_no_floor_and_answers_from_zero() {
        let dir = tempfile::tempdir().unwrap();
        let store = FeedStore::open(&dir.path().join("sync.db")).unwrap();
        let view = store.view(46).expect("no basis, no bound rule");
        assert_eq!(view.history_floor(), 0);
        assert!(view
            .get_events(&[], BlockId::Number(0), BlockId::Number(46))
            .await
            .is_ok());
    }

    /// §1.5.2 guard rail: a view bound below the basis is refused rather than
    /// answered with zeros. Engine bounds are always `last_epoch_to` or `head`,
    /// both at or above the basis — the rule is here so a future refactor
    /// cannot introduce a silent zero-read.
    #[test]
    fn a_view_bound_below_the_snapshot_basis_is_refused() {
        let (_dir, store) = snapshot_started(31);
        let err = store.view(30).expect_err("bound below the basis");
        let text = format!("{err}");
        assert!(
            text.contains("BOUND_BELOW_SNAPSHOT") && text.contains("30") && text.contains("31"),
            "the refusal must name both the bound and the basis: {text}"
        );
        assert!(store.view(31).is_ok(), "the basis itself is in range");
    }
}
