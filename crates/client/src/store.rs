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
    ) -> Result<ApplyOutcome> {
        let mut out = ApplyOutcome::default();
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
        let manifest: Manifest = transport.fetch_manifest().await?;

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
            let payload = strk20_feed::decompress(&compressed)?;
            let epoch =
                strk20_feed::manifest::verify_epoch_against_manifest(&payload, entry, prev_hash)?;
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
        Ok(out)
    }

    /// Persisted tail-replacement counter (crash-safe, shared-db-safe).
    pub fn tail_generation(&self) -> Result<u64> {
        Ok(self
            .meta_get("tail_generation")?
            .and_then(|s| s.parse().ok())
            .unwrap_or(0))
    }

    /// A read view bound to `block` for the discovery engine.
    pub fn view(&self, block: u64) -> ClientView {
        ClientView {
            conn: self.conn.clone(),
            bound: block,
        }
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
#[derive(Clone)]
pub struct ClientView {
    conn: Arc<Mutex<Connection>>,
    bound: u64,
}

impl ClientView {
    pub fn bound(&self) -> u64 {
        self.bound
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
