//! FeedStore (spec §7.3): the client's verified local mirror in sync.db.
//!
//! This is the **SQLite host** for Block B and nothing else. Everything that
//! decides *what* to fold — the epoch hash chain, the snapshot verification
//! ladder, reorg supersession, the discovery passes, the report — lives in
//! `strk20-consumer` and runs identically over the browser's in-memory store.
//! What lives here is only the part that knows about rows: DDL, transactions,
//! blob encoding, file permissions, and the `spawn_blocking` bridge that lets
//! the unmodified upstream engine read from a synchronous database.
//!
//! Contains SecretFelt-derived cursor material — the DB file is chmod 0600 and
//! never leaves the machine.

use anyhow::{Context, Result};
use async_trait::async_trait;
use discovery_core::events_backend::RawEventAccess;
use discovery_core::storage_backend::{RawStorageAccess, StorageError};
use rusqlite::{params, Connection, OptionalExtension};
use starknet_core::types::{BlockId, EmittedEvent, StorageResult};
use starknet_types_core::felt::Felt;
use std::path::Path;
use std::sync::{Arc, Mutex};
use strk20_consumer::apply::{check_bound_above_basis, history_floor};
use strk20_consumer::store::{ConsumerStore, Range};
use strk20_feed::codec::BlockLine;
use strk20_feed::snapshot::SnapSlot;

/// Re-exported so callers of this crate keep one import path for the value
/// types Block B and the SQLite host share.
pub use strk20_consumer::store::{ApplyOutcome, ColdStart, NoteRow};

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

pub struct FeedStore {
    conn: Arc<Mutex<Connection>>,
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

    /// Lowest block for which this mirror holds EVENTS (§1.1). 0 for a fully
    /// epoch-replayed mirror.
    pub fn history_floor(&self) -> Result<u64> {
        history_floor(self)
    }

    /// Inherent metadata accessors, so a caller that wants one value out of a
    /// mirror does not have to import `ConsumerStore` to get it. They are the
    /// trait methods, called by their one true name.
    pub fn meta_get(&self, key: &str) -> Result<Option<String>> {
        <Self as ConsumerStore>::meta_get(self, key)
    }

    pub fn meta_set(&self, key: &str, value: &str) -> Result<()> {
        <Self as ConsumerStore>::meta_set(self, key, value)
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
}

/// The SQLite implementation of the one seam. Nothing below decides anything:
/// each method is a row operation whose meaning is fixed by
/// `strk20_consumer::store::ConsumerStore`.
impl ConsumerStore for FeedStore {
    type View = ClientView;

    fn meta_get(&self, key: &str) -> Result<Option<String>> {
        let conn = self.conn.lock().expect("conn");
        Ok(conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| r.get(0))
            .optional()?)
    }

    fn meta_set(&self, key: &str, value: &str) -> Result<()> {
        let conn = self.conn.lock().expect("conn");
        conn.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    /// Is this mirror still unpopulated? Only then may a snapshot be applied:
    /// a snapshot is a floor, and laying one under existing rows would create
    /// a mirror whose history floor contradicts what it already holds.
    fn is_empty(&self) -> Result<bool> {
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

    /// Hash of the mirrored block `number`, if the mirror holds it.
    fn block_hash(&self, number: u64) -> Result<Option<Felt>> {
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

    fn block_hashes(&self, range: Range) -> Result<Vec<(u64, Felt)>> {
        let conn = self.conn.lock().expect("conn");
        let row = |r: &rusqlite::Row<'_>| {
            Ok((r.get::<_, i64>(0)? as u64, bf(&r.get::<_, Vec<u8>>(1)?)))
        };
        let rows = match range {
            Range::Inclusive { from, to } => {
                let mut stmt = conn.prepare(
                    "SELECT number, hash FROM blocks WHERE number BETWEEN ?1 AND ?2
                     ORDER BY number",
                )?;
                let out = stmt
                    .query_map(params![from as i64, to as i64], row)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                out
            }
            Range::Above { floor } => {
                let mut stmt = conn
                    .prepare("SELECT number, hash FROM blocks WHERE number > ?1 ORDER BY number")?;
                let out = stmt
                    .query_map([floor as i64], row)?
                    .collect::<std::result::Result<Vec<_>, _>>()?;
                out
            }
        };
        Ok(rows)
    }

    fn read_slot_as_of(&self, slot: &Felt, bound: u64) -> Result<(Felt, u64)> {
        let conn = self.conn.lock().expect("conn");
        let row: Option<(Vec<u8>, i64)> = conn
            .query_row(
                "SELECT value, block FROM storage_log WHERE slot = ?1 AND block <= ?2
                 ORDER BY block DESC LIMIT 1",
                params![fb(slot).as_slice(), bound as i64],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        Ok(match row {
            Some((v, b)) => (bf(&v), b as u64),
            None => (Felt::ZERO, 0),
        })
    }

    /// Complete pool slot set as of `block` (latest value per slot, zero-value
    /// rows excluded) — the input the client folds into a storage root when
    /// checking a published anchor.
    fn full_slot_set_as_of(&self, block: u64) -> Result<Vec<(Felt, Felt)>> {
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

    /// A read view bound to `block` for the discovery engine.
    fn view(&self, block: u64) -> Result<ClientView> {
        check_bound_above_basis(self, block)?;
        Ok(ClientView {
            conn: self.conn.clone(),
            bound: block,
            history_floor: history_floor(self)?,
        })
    }

    /// Drop everything the feed put here and return to the pre-sync state, so
    /// the C13 fallback replays epochs into a mirror with no snapshot rows
    /// left under it. Identity metadata (pool, chain id, epoch size) survives:
    /// it is what a re-sync is checked against.
    fn reset_mirror(&self) -> Result<()> {
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
    fn install_snapshot(&self, slots: &[SnapSlot], meta: &[(&str, String)]) -> Result<()> {
        let mut guard = self.conn.lock().expect("conn");
        let tx = guard.transaction()?;
        {
            let mut ins = tx.prepare(
                "INSERT OR REPLACE INTO storage_log(slot, block, value) VALUES (?1, ?2, ?3)",
            )?;
            for s in slots {
                ins.execute(params![
                    fb(&s.k).as_slice(),
                    s.w as i64,
                    fb(&s.v).as_slice()
                ])?;
            }
        }
        for (key, value) in meta {
            meta_set_tx(&tx, key, value)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// One transaction: supersede the range, lay down the new blocks, write the
    /// metadata, and (on a contradiction) bump the tail generation. They ride
    /// together because a crash between the rebuild and the bump is exactly the
    /// state in which every owner's cursor silently fails to rewind.
    fn replace_range(
        &self,
        range: Range,
        blocks: &[(&BlockLine, bool)],
        meta: &[(&str, String)],
        bump_generation: bool,
    ) -> Result<()> {
        let mut guard = self.conn.lock().expect("conn");
        let tx = guard.transaction()?;
        match range {
            Range::Inclusive { from, to } => {
                for table in ["storage_log", "events"] {
                    tx.execute(
                        &format!("DELETE FROM {table} WHERE block BETWEEN ?1 AND ?2"),
                        params![from as i64, to as i64],
                    )?;
                }
                tx.execute(
                    "DELETE FROM blocks WHERE number BETWEEN ?1 AND ?2",
                    params![from as i64, to as i64],
                )?;
            }
            Range::Above { floor } => {
                for table in ["storage_log", "events"] {
                    tx.execute(
                        &format!("DELETE FROM {table} WHERE block > ?1"),
                        [floor as i64],
                    )?;
                }
                tx.execute("DELETE FROM blocks WHERE number > ?1", [floor as i64])?;
            }
        }
        for (b, l1_final) in blocks {
            Self::apply_block_line(&tx, b, i64::from(*l1_final))?;
        }
        for (key, value) in meta {
            meta_set_tx(&tx, key, value)?;
        }
        if bump_generation {
            bump_generation_tx(&tx)?;
        }
        tx.commit()?;
        Ok(())
    }

    /// Persisted tail-replacement counter (crash-safe, shared-db-safe).
    fn tail_generation(&self) -> Result<u64> {
        Ok(self
            .meta_get("tail_generation")?
            .and_then(|s| s.parse().ok())
            .unwrap_or(0))
    }

    // -------------------------------------------------------- notes registry

    fn notes(&self, owner: &Felt) -> Result<Vec<NoteRow>> {
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

    fn upsert_note(&self, n: &NoteRow) -> Result<()> {
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

    fn set_note_spent(&self, note_id: &Felt, spent: bool) -> Result<()> {
        let conn = self.conn.lock().expect("conn");
        conn.execute(
            "UPDATE notes_registry SET spent = ?1 WHERE note_id = ?2",
            params![spent as i64, fb(note_id).as_slice()],
        )?;
        Ok(())
    }

    fn delete_note(&self, note_id: &Felt) -> Result<()> {
        let conn = self.conn.lock().expect("conn");
        conn.execute(
            "DELETE FROM notes_registry WHERE note_id = ?1",
            [fb(note_id).as_slice()],
        )?;
        Ok(())
    }

    fn delete_owner_notes(&self, owner: &Felt) -> Result<usize> {
        let conn = self.conn.lock().expect("conn");
        Ok(conn.execute(
            "DELETE FROM notes_registry WHERE owner = ?1",
            [fb(owner).as_slice()],
        )?)
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
