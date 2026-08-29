//! SQLite store (spec §4.1). One writer (the ingest loop); readers open their
//! own connections (WAL). Felts are 32-byte big-endian BLOBs.

use anyhow::{Context, Result};
use rusqlite::{params, Connection, OptionalExtension};
use starknet_types_core::felt::Felt;
use std::path::{Path, PathBuf};

pub const SCHEMA_VERSION: i64 = 1;

const DDL: &str = r#"
CREATE TABLE IF NOT EXISTS meta (
  key   TEXT PRIMARY KEY,
  value TEXT NOT NULL
) WITHOUT ROWID;

CREATE TABLE IF NOT EXISTS blocks (
  number      INTEGER PRIMARY KEY,
  hash        BLOB NOT NULL,
  parent_hash BLOB NOT NULL,
  timestamp   INTEGER NOT NULL,
  status      INTEGER NOT NULL
);
CREATE UNIQUE INDEX IF NOT EXISTS blocks_hash ON blocks(hash);

CREATE TABLE IF NOT EXISTS storage_log (
  slot  BLOB NOT NULL,
  block INTEGER NOT NULL REFERENCES blocks(number) ON DELETE CASCADE,
  value BLOB NOT NULL,
  PRIMARY KEY (slot, block)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS storage_log_block ON storage_log(block);

CREATE TABLE IF NOT EXISTS events (
  block       INTEGER NOT NULL REFERENCES blocks(number) ON DELETE CASCADE,
  event_index INTEGER NOT NULL,
  tx_index    INTEGER NOT NULL,
  tx_hash     BLOB NOT NULL,
  key0        BLOB NOT NULL,
  key1        BLOB,
  keys        BLOB NOT NULL,
  data        BLOB NOT NULL,
  PRIMARY KEY (block, event_index)
) WITHOUT ROWID;
CREATE INDEX IF NOT EXISTS ev_key0 ON events(key0, block);
CREATE INDEX IF NOT EXISTS ev_key1 ON events(key1) WHERE key1 IS NOT NULL;

CREATE TABLE IF NOT EXISTS class_history (
  block      INTEGER PRIMARY KEY,
  class_hash BLOB NOT NULL,
  decoder    TEXT
);

CREATE TABLE IF NOT EXISTS epochs (
  idx           INTEGER PRIMARY KEY,
  from_block    INTEGER NOT NULL,
  to_block      INTEGER NOT NULL,
  content_hash  BLOB NOT NULL,
  zst_sha256    BLOB NOT NULL,
  file_size     INTEGER NOT NULL,
  prev_hash     BLOB,
  anchor_block        INTEGER,
  anchor_block_hash   BLOB,
  anchor_storage_root BLOB,
  anchor_class_hash   BLOB,
  cut_at        INTEGER NOT NULL
);

CREATE TABLE IF NOT EXISTS ingest_cursor (
  id                  INTEGER PRIMARY KEY CHECK (id = 1),
  scan_frontier       INTEGER NOT NULL,
  events_continuation TEXT
);

-- Every canonical head hash ever observed (incl. non-pool-active blocks), so
-- compat-mode canonicity checks on last_known_block work for block hashes
-- that never carried pool activity. Pruned on reorg rollback.
CREATE TABLE IF NOT EXISTS seen_heads (
  hash    BLOB PRIMARY KEY,
  number  INTEGER NOT NULL,
  reorged INTEGER NOT NULL DEFAULT 0
) WITHOUT ROWID;
"#;

pub fn felt_blob(f: &Felt) -> [u8; 32] {
    f.to_bytes_be()
}

pub fn blob_felt(b: &[u8]) -> Felt {
    let arr: [u8; 32] = b.try_into().expect("felt blob must be 32 bytes");
    Felt::from_bytes_be(&arr)
}

/// Concatenated 32-byte felts <-> Vec<Felt>.
pub fn felts_blob(fs: &[Felt]) -> Vec<u8> {
    let mut out = Vec::with_capacity(fs.len() * 32);
    for f in fs {
        out.extend_from_slice(&f.to_bytes_be());
    }
    out
}

pub fn blob_felts(b: &[u8]) -> Vec<Felt> {
    assert!(b.len() % 32 == 0, "felt list blob must be a multiple of 32");
    b.chunks_exact(32).map(blob_felt).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BlockRow {
    pub number: u64,
    pub hash: Felt,
    pub parent_hash: Felt,
    pub timestamp: u64,
    pub l1_accepted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EventRow {
    pub block: u64,
    pub event_index: u64,
    pub tx_index: u64,
    pub tx_hash: Felt,
    pub keys: Vec<Felt>,
    pub data: Vec<Felt>,
}

pub struct Db {
    pub conn: Connection,
    path: PathBuf,
}

impl Db {
    pub fn open(path: &Path) -> Result<Self> {
        let conn = Connection::open(path).with_context(|| format!("open {}", path.display()))?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        conn.pragma_update(None, "synchronous", "NORMAL")?;
        conn.pragma_update(None, "foreign_keys", "ON")?;
        conn.execute_batch(DDL)?;
        Ok(Self {
            conn,
            path: path.to_owned(),
        })
    }

    /// A second connection to the same file (readers, snapshots).
    pub fn reopen(&self) -> Result<Self> {
        Self::open(&self.path)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    // ------------------------------------------------------------- meta

    pub fn meta_get(&self, key: &str) -> Result<Option<String>> {
        Ok(self
            .conn
            .query_row("SELECT value FROM meta WHERE key = ?1", [key], |r| {
                r.get(0)
            })
            .optional()?)
    }

    pub fn meta_set(&self, key: &str, value: &str) -> Result<()> {
        self.conn.execute(
            "INSERT INTO meta(key, value) VALUES (?1, ?2)
             ON CONFLICT(key) DO UPDATE SET value = excluded.value",
            params![key, value],
        )?;
        Ok(())
    }

    // ----------------------------------------------------------- blocks

    pub fn insert_block(&self, b: &BlockRow) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO blocks(number, hash, parent_hash, timestamp, status)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                b.number as i64,
                felt_blob(&b.hash).as_slice(),
                felt_blob(&b.parent_hash).as_slice(),
                b.timestamp as i64,
                b.l1_accepted as i64
            ],
        )?;
        Ok(())
    }

    pub fn block(&self, number: u64) -> Result<Option<BlockRow>> {
        Ok(self
            .conn
            .query_row(
                "SELECT number, hash, parent_hash, timestamp, status FROM blocks WHERE number = ?1",
                [number as i64],
                row_to_block,
            )
            .optional()?)
    }

    pub fn blocks_in_range(&self, from: u64, to: u64) -> Result<Vec<BlockRow>> {
        let mut stmt = self.conn.prepare(
            "SELECT number, hash, parent_hash, timestamp, status FROM blocks
             WHERE number BETWEEN ?1 AND ?2 ORDER BY number",
        )?;
        let rows = stmt
            .query_map(params![from as i64, to as i64], row_to_block)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    pub fn max_block(&self) -> Result<Option<u64>> {
        Ok(self
            .conn
            .query_row("SELECT MAX(number) FROM blocks", [], |r| r.get::<_, Option<i64>>(0).map(|o| o.map(|v| v as u64)))?)
    }

    /// Canonicity of a block hash for compat's `last_known_block` gate.
    /// Three-valued knowledge collapsed honestly: a hash we hold as a live
    /// block or head is canonical; a hash we TOMBSTONED during a rollback is
    /// known-reorged (409); a hash this instance never observed (fresh DB,
    /// pre-history block) is treated as canonical — a rebuilt indexer must
    /// not 409 every existing client (review finding: db.rs is_canonical).
    pub fn is_canonical(&self, hash: &Felt) -> Result<bool> {
        let in_blocks = self
            .conn
            .query_row(
                "SELECT 1 FROM blocks WHERE hash = ?1",
                [felt_blob(hash).as_slice()],
                |_| Ok(()),
            )
            .optional()?
            .is_some();
        if in_blocks {
            return Ok(true);
        }
        let seen: Option<i64> = self
            .conn
            .query_row(
                "SELECT reorged FROM seen_heads WHERE hash = ?1",
                [felt_blob(hash).as_slice()],
                |r| r.get(0),
            )
            .optional()?;
        Ok(match seen {
            Some(reorged) => reorged == 0,
            None => true, // unknown: this instance cannot testify against it
        })
    }

    pub fn record_seen_head(&self, hash: &Felt, number: u64) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO seen_heads(hash, number, reorged) VALUES (?1, ?2, 0)",
            params![felt_blob(hash).as_slice(), number as i64],
        )?;
        Ok(())
    }

    pub fn block_number_of_hash(&self, hash: &Felt) -> Result<Option<u64>> {
        let in_blocks: Option<i64> = self
            .conn
            .query_row(
                "SELECT number FROM blocks WHERE hash = ?1",
                [felt_blob(hash).as_slice()],
                |r| r.get(0),
            )
            .optional()?;
        if let Some(n) = in_blocks {
            return Ok(Some(n as u64));
        }
        Ok(self
            .conn
            .query_row(
                "SELECT number FROM seen_heads WHERE hash = ?1",
                [felt_blob(hash).as_slice()],
                |r| r.get::<_, i64>(0),
            )
            .optional()?
            .map(|v| v as u64))
    }

    /// Mark all blocks <= `upto` as ACCEPTED_ON_L1.
    pub fn promote_l1(&self, upto: u64) -> Result<usize> {
        Ok(self.conn.execute(
            "UPDATE blocks SET status = 1 WHERE number <= ?1 AND status = 0",
            [upto as i64],
        )?)
    }

    /// Delete everything above `ancestor` (reorg rollback). Cascades to
    /// storage_log and events. Returns rows removed from blocks.
    pub fn rollback_above(&mut self, ancestor: u64) -> Result<usize> {
        let tx = self.conn.transaction()?;
        // Tombstone every hash we are about to forget, so compat can answer
        // 409 (known-reorged) instead of guessing.
        tx.execute(
            "INSERT OR REPLACE INTO seen_heads(hash, number, reorged)
             SELECT hash, number, 1 FROM blocks WHERE number > ?1",
            [ancestor as i64],
        )?;
        tx.execute(
            "UPDATE seen_heads SET reorged = 1 WHERE number > ?1",
            [ancestor as i64],
        )?;
        let n = tx.execute("DELETE FROM blocks WHERE number > ?1", [ancestor as i64])?;
        tx.execute(
            "UPDATE ingest_cursor SET scan_frontier = MIN(scan_frontier, ?1),
             events_continuation = NULL WHERE id = 1",
            [ancestor as i64],
        )?;
        tx.commit()?;
        Ok(n)
    }

    // ------------------------------------------------------ storage/events

    /// One ingest transaction for a fully-fetched block.
    pub fn insert_block_data(
        &mut self,
        block: &BlockRow,
        diffs: &[(Felt, Felt)],
        events: &[EventRow],
        replaced_class: Option<&Felt>,
        new_frontier: u64,
        continuation: Option<&str>,
    ) -> Result<()> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT OR REPLACE INTO blocks(number, hash, parent_hash, timestamp, status)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            params![
                block.number as i64,
                felt_blob(&block.hash).as_slice(),
                felt_blob(&block.parent_hash).as_slice(),
                block.timestamp as i64,
                block.l1_accepted as i64
            ],
        )?;
        {
            let mut ins = tx.prepare_cached(
                "INSERT OR REPLACE INTO storage_log(slot, block, value) VALUES (?1, ?2, ?3)",
            )?;
            for (slot, value) in diffs {
                ins.execute(params![
                    felt_blob(slot).as_slice(),
                    block.number as i64,
                    felt_blob(value).as_slice()
                ])?;
            }
        }
        {
            let mut ins = tx.prepare_cached(
                "INSERT OR REPLACE INTO events(block, event_index, tx_index, tx_hash, key0, key1, keys, data)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
            )?;
            for e in events {
                let key0 = e.keys.first().map(felt_blob).unwrap_or([0u8; 32]);
                let key1 = e.keys.get(1).map(felt_blob);
                ins.execute(params![
                    e.block as i64,
                    e.event_index as i64,
                    e.tx_index as i64,
                    felt_blob(&e.tx_hash).as_slice(),
                    key0.as_slice(),
                    key1.as_ref().map(|k| k.as_slice()),
                    felts_blob(&e.keys),
                    felts_blob(&e.data)
                ])?;
            }
        }
        if let Some(class) = replaced_class {
            tx.execute(
                "INSERT OR REPLACE INTO class_history(block, class_hash, decoder) VALUES (?1, ?2, NULL)",
                params![block.number as i64, felt_blob(class).as_slice()],
            )?;
        }
        tx.execute(
            "INSERT INTO ingest_cursor(id, scan_frontier, events_continuation)
             VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET scan_frontier = excluded.scan_frontier,
                                           events_continuation = excluded.events_continuation",
            params![new_frontier as i64, continuation],
        )?;
        tx.commit()?;
        Ok(())
    }

    pub fn ingest_cursor(&self) -> Result<Option<(u64, Option<String>)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT scan_frontier, events_continuation FROM ingest_cursor WHERE id = 1",
                [],
                |r| Ok((r.get::<_, i64>(0)? as u64, r.get::<_, Option<String>>(1)?)),
            )
            .optional()?)
    }

    pub fn set_ingest_cursor(&self, frontier: u64, continuation: Option<&str>) -> Result<()> {
        self.conn.execute(
            "INSERT INTO ingest_cursor(id, scan_frontier, events_continuation)
             VALUES (1, ?1, ?2)
             ON CONFLICT(id) DO UPDATE SET scan_frontier = excluded.scan_frontier,
                                           events_continuation = excluded.events_continuation",
            params![frontier as i64, continuation],
        )?;
        Ok(())
    }

    /// Value of `slot` as of `block` (inclusive); Cairo map semantics: absent
    /// = zero. Also returns the write block.
    pub fn read_slot_as_of(&self, slot: &Felt, block: u64) -> Result<(Felt, Option<u64>)> {
        let row = self
            .conn
            .query_row(
                "SELECT value, block FROM storage_log
                 WHERE slot = ?1 AND block <= ?2 ORDER BY block DESC LIMIT 1",
                params![felt_blob(slot).as_slice(), block as i64],
                |r| {
                    Ok((
                        blob_felt(&r.get::<_, Vec<u8>>(0)?),
                        r.get::<_, i64>(1)? as u64,
                    ))
                },
            )
            .optional()?;
        Ok(match row {
            Some((v, b)) => (v, Some(b)),
            None => (Felt::ZERO, None),
        })
    }

    /// All diffs of one block, sorted by slot bytes.
    pub fn diffs_of_block(&self, block: u64) -> Result<Vec<(Felt, Felt)>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT slot, value FROM storage_log WHERE block = ?1 ORDER BY slot",
        )?;
        let rows = stmt
            .query_map([block as i64], |r| {
                Ok((
                    blob_felt(&r.get::<_, Vec<u8>>(0)?),
                    blob_felt(&r.get::<_, Vec<u8>>(1)?),
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Complete current slot set as of `block` (latest value per slot),
    /// zero-value rows excluded — input for verify-root.
    pub fn full_slot_set_as_of(&self, block: u64) -> Result<Vec<(Felt, Felt)>> {
        let mut stmt = self.conn.prepare(
            "SELECT slot, value FROM storage_log s
             WHERE block = (SELECT MAX(block) FROM storage_log
                            WHERE slot = s.slot AND block <= ?1)",
        )?;
        let rows = stmt
            .query_map([block as i64], |r| {
                Ok((
                    blob_felt(&r.get::<_, Vec<u8>>(0)?),
                    blob_felt(&r.get::<_, Vec<u8>>(1)?),
                ))
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows.into_iter().filter(|(_, v)| *v != Felt::ZERO).collect())
    }

    pub fn events_of_block(&self, block: u64) -> Result<Vec<EventRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT block, event_index, tx_index, tx_hash, keys, data FROM events
             WHERE block = ?1 ORDER BY event_index",
        )?;
        let rows = stmt
            .query_map([block as i64], row_to_event)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Events in [from, to] filtered per key position (upstream RawEventAccess
    /// semantics: for every non-empty filter set at position i, event.keys[i]
    /// must be in the set).
    pub fn events_filtered(
        &self,
        from: u64,
        to: u64,
        key_filters: &[Vec<Felt>],
    ) -> Result<Vec<EventRow>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT block, event_index, tx_index, tx_hash, keys, data FROM events
             WHERE block BETWEEN ?1 AND ?2 ORDER BY block, event_index",
        )?;
        let rows = stmt
            .query_map(params![from as i64, to as i64], row_to_event)?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows
            .into_iter()
            .filter(|e| {
                key_filters.iter().enumerate().all(|(i, allowed)| {
                    allowed.is_empty()
                        || e.keys.get(i).map(|k| allowed.contains(k)).unwrap_or(false)
                })
            })
            .collect())
    }

    pub fn replaced_class_of_block(&self, block: u64) -> Result<Option<Felt>> {
        Ok(self
            .conn
            .query_row(
                "SELECT class_hash FROM class_history WHERE block = ?1",
                [block as i64],
                |r| Ok(blob_felt(&r.get::<_, Vec<u8>>(0)?)),
            )
            .optional()?)
    }

    /// Pool class hash as of `block` (latest class_history row <= block).
    pub fn class_as_of(&self, block: u64) -> Result<Option<Felt>> {
        Ok(self
            .conn
            .query_row(
                "SELECT class_hash FROM class_history WHERE block <= ?1
                 ORDER BY block DESC LIMIT 1",
                [block as i64],
                |r| Ok(blob_felt(&r.get::<_, Vec<u8>>(0)?)),
            )
            .optional()?)
    }

    // ------------------------------------------------------------ epochs

    pub fn last_epoch(&self) -> Result<Option<(u64, [u8; 32], u64)>> {
        Ok(self
            .conn
            .query_row(
                "SELECT idx, content_hash, to_block FROM epochs ORDER BY idx DESC LIMIT 1",
                [],
                |r| {
                    let h: Vec<u8> = r.get(1)?;
                    Ok((
                        r.get::<_, i64>(0)? as u64,
                        h.try_into().expect("32-byte hash"),
                        r.get::<_, i64>(2)? as u64,
                    ))
                },
            )
            .optional()?)
    }

    #[allow(clippy::too_many_arguments)]
    pub fn insert_epoch(
        &self,
        idx: u64,
        from: u64,
        to: u64,
        content_hash: &[u8; 32],
        zst_sha256: &[u8; 32],
        file_size: u64,
        prev_hash: Option<&[u8; 32]>,
        anchor: Option<&crate::cutter::Anchor>,
        cut_at: u64,
    ) -> Result<()> {
        self.conn.execute(
            "INSERT OR REPLACE INTO epochs(idx, from_block, to_block, content_hash, zst_sha256,
                file_size, prev_hash, anchor_block, anchor_block_hash, anchor_storage_root,
                anchor_class_hash, cut_at)
             VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12)",
            params![
                idx as i64,
                from as i64,
                to as i64,
                content_hash.as_slice(),
                zst_sha256.as_slice(),
                file_size as i64,
                prev_hash.map(|h| h.as_slice()),
                anchor.map(|a| a.block as i64),
                anchor.map(|a| felt_blob(&a.block_hash).to_vec()),
                anchor.map(|a| felt_blob(&a.storage_root).to_vec()),
                anchor.map(|a| felt_blob(&a.class_hash).to_vec()),
                cut_at as i64
            ],
        )?;
        Ok(())
    }

    pub fn epoch_rows(&self) -> Result<Vec<EpochRowFull>> {
        let mut stmt = self.conn.prepare(
            "SELECT idx, from_block, to_block, content_hash, zst_sha256, file_size,
                    anchor_block, anchor_block_hash, anchor_storage_root, anchor_class_hash
             FROM epochs ORDER BY idx",
        )?;
        let rows = stmt
            .query_map([], |r| {
                let ch: Vec<u8> = r.get(3)?;
                let zh: Vec<u8> = r.get(4)?;
                Ok(EpochRowFull {
                    idx: r.get::<_, i64>(0)? as u64,
                    from: r.get::<_, i64>(1)? as u64,
                    to: r.get::<_, i64>(2)? as u64,
                    content_hash: ch.try_into().expect("32"),
                    zst_sha256: zh.try_into().expect("32"),
                    file_size: r.get::<_, i64>(5)? as u64,
                    anchor_block: r.get::<_, Option<i64>>(6)?.map(|v| v as u64),
                    anchor_block_hash: r.get::<_, Option<Vec<u8>>>(7)?.map(|b| blob_felt(&b)),
                    anchor_storage_root: r.get::<_, Option<Vec<u8>>>(8)?.map(|b| blob_felt(&b)),
                    anchor_class_hash: r.get::<_, Option<Vec<u8>>>(9)?.map(|b| blob_felt(&b)),
                })
            })?
            .collect::<std::result::Result<Vec<_>, _>>()?;
        Ok(rows)
    }
}

#[derive(Debug, Clone)]
pub struct EpochRowFull {
    pub idx: u64,
    pub from: u64,
    pub to: u64,
    pub content_hash: [u8; 32],
    pub zst_sha256: [u8; 32],
    pub file_size: u64,
    pub anchor_block: Option<u64>,
    pub anchor_block_hash: Option<Felt>,
    pub anchor_storage_root: Option<Felt>,
    pub anchor_class_hash: Option<Felt>,
}

fn row_to_block(r: &rusqlite::Row<'_>) -> rusqlite::Result<BlockRow> {
    Ok(BlockRow {
        number: r.get::<_, i64>(0)? as u64,
        hash: blob_felt(&r.get::<_, Vec<u8>>(1)?),
        parent_hash: blob_felt(&r.get::<_, Vec<u8>>(2)?),
        timestamp: r.get::<_, i64>(3)? as u64,
        l1_accepted: r.get::<_, i64>(4)? == 1,
    })
}

fn row_to_event(r: &rusqlite::Row<'_>) -> rusqlite::Result<EventRow> {
    Ok(EventRow {
        block: r.get::<_, i64>(0)? as u64,
        event_index: r.get::<_, i64>(1)? as u64,
        tx_index: r.get::<_, i64>(2)? as u64,
        tx_hash: blob_felt(&r.get::<_, Vec<u8>>(3)?),
        keys: blob_felts(&r.get::<_, Vec<u8>>(4)?),
        data: blob_felts(&r.get::<_, Vec<u8>>(5)?),
    })
}
