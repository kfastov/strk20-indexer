//! Engine bridge (spec §6.4, §10.2): the four discovery-core traits over the
//! local SQLite mirror. The unmodified upstream engine runs on top via the
//! blanket `impl<T: RawStorageAccess> IViews for T`.

use crate::db::Db;
use async_trait::async_trait;
use discovery_core::events_backend::RawEventAccess;
use discovery_core::storage_backend::{
    RawStorageAccess, StorageBackend, StorageError, StorageSnapshot,
};
use starknet_core::types::{BlockId, BlockTag, EmittedEvent, StorageResult};
use starknet_types_core::felt::Felt;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

fn backend_err(e: impl std::error::Error + Send + Sync + 'static) -> StorageError {
    StorageError::Backend(Box::new(e))
}

fn anyhow_err(e: anyhow::Error) -> StorageError {
    StorageError::Backend(e.into())
}

/// Factory: opens a fresh read connection per snapshot (SQLite WAL readers).
#[derive(Clone)]
pub struct DbBackend {
    pub db_path: PathBuf,
    pub pool: Felt,
}

pub struct DbSnapshot {
    inner: Arc<SnapInner>,
}

struct SnapInner {
    db: Mutex<Db>,
    pool: Felt,
    /// as-of block for every read
    block: u64,
    /// the id this snapshot reports (what handlers echo as block_ref)
    block_id: BlockId,
}

impl DbBackend {
    pub fn new(db_path: PathBuf, pool: Felt) -> Self {
        Self { db_path, pool }
    }

    fn resolve(db: &Db, id: Option<BlockId>) -> Result<(u64, BlockId), StorageError> {
        let head: u64 = db
            .meta_get("head_number")
            .map_err(anyhow_err)?
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let head_hash = db.meta_get("head_hash").map_err(anyhow_err)?;
        match id {
            None | Some(BlockId::Tag(BlockTag::Latest)) | Some(BlockId::Tag(BlockTag::PreConfirmed)) => {
                // pin to the current head; report its hash when known
                let reported = match &head_hash {
                    Some(h) => Felt::from_hex(h).map(BlockId::Hash).unwrap_or(BlockId::Number(head)),
                    None => BlockId::Number(head),
                };
                Ok((head, reported))
            }
            Some(BlockId::Tag(BlockTag::L1Accepted)) => {
                let l1: u64 = db
                    .meta_get("l1_accepted_number")
                    .map_err(anyhow_err)?
                    .and_then(|s| s.parse().ok())
                    .unwrap_or(0);
                Ok((l1, BlockId::Number(l1)))
            }
            Some(BlockId::Number(n)) => {
                if n > head {
                    return Err(anyhow_err(anyhow::anyhow!(
                        "block {n} beyond ingested head {head}"
                    )));
                }
                Ok((n, BlockId::Number(n)))
            }
            Some(BlockId::Hash(h)) => {
                if head_hash.as_deref() == Some(strk20_feed::felt_hex(&h).as_str()) {
                    return Ok((head, BlockId::Hash(h)));
                }
                match db.block_number_of_hash(&h).map_err(anyhow_err)? {
                    Some(n) => Ok((n, BlockId::Hash(h))),
                    None => Err(anyhow_err(anyhow::anyhow!(
                        "unknown block hash {}",
                        strk20_feed::felt_hex(&h)
                    ))),
                }
            }
        }
    }
}

#[async_trait]
impl StorageBackend for DbBackend {
    type Snapshot = DbSnapshot;

    async fn snapshot(
        &self,
        contract_address: Felt,
        block_id: Option<BlockId>,
    ) -> Result<Self::Snapshot, StorageError> {
        if contract_address != self.pool {
            return Err(StorageError::ContractNotFound);
        }
        let path = self.db_path.clone();
        let pool = self.pool;
        tokio::task::spawn_blocking(move || {
            let db = Db::open(&path).map_err(anyhow_err)?;
            let (block, reported) = DbBackend::resolve(&db, block_id)?;
            Ok(DbSnapshot {
                inner: Arc::new(SnapInner {
                    db: Mutex::new(db),
                    pool,
                    block,
                    block_id: reported,
                }),
            })
        })
        .await
        .map_err(backend_err)?
    }
}

impl DbSnapshot {
    fn with_db<T: Send + 'static>(
        &self,
        f: impl FnOnce(&Db, u64) -> anyhow::Result<T> + Send + 'static,
    ) -> impl std::future::Future<Output = Result<T, StorageError>> {
        let inner = self.inner.clone();
        async move {
            tokio::task::spawn_blocking(move || {
                let db = inner.db.lock().expect("db mutex");
                f(&db, inner.block).map_err(anyhow_err)
            })
            .await
            .map_err(backend_err)?
        }
    }

    pub fn bound_block(&self) -> u64 {
        self.inner.block
    }
}

#[async_trait]
impl RawStorageAccess for DbSnapshot {
    async fn read_slot(&self, slot: Felt) -> Result<Felt, StorageError> {
        self.with_db(move |db, block| Ok(db.read_slot_as_of(&slot, block)?.0))
            .await
    }

    async fn read_slots(&self, slots: Vec<Felt>) -> Result<Vec<Felt>, StorageError> {
        self.with_db(move |db, block| {
            slots
                .iter()
                .map(|s| Ok(db.read_slot_as_of(s, block)?.0))
                .collect()
        })
        .await
    }

    async fn read_slots_with_block(
        &self,
        slots: Vec<Felt>,
    ) -> Result<Vec<StorageResult>, StorageError> {
        self.with_db(move |db, block| {
            slots
                .iter()
                .map(|s| {
                    let (value, wb) = db.read_slot_as_of(s, block)?;
                    Ok(StorageResult {
                        value,
                        last_update_block: wb.unwrap_or(0),
                    })
                })
                .collect()
        })
        .await
    }
}

#[async_trait]
impl StorageSnapshot for DbSnapshot {
    fn contract_address(&self) -> Felt {
        self.inner.pool
    }

    fn block_id(&self) -> BlockId {
        self.inner.block_id
    }
}

#[async_trait]
impl RawEventAccess for DbSnapshot {
    async fn get_events(
        &self,
        keys: &[Vec<Felt>],
        from_block: BlockId,
        to_block: BlockId,
    ) -> Result<Vec<EmittedEvent>, StorageError> {
        let filters: Vec<Vec<Felt>> = keys.to_vec();
        let pool = self.inner.pool;
        let bound = self.inner.block;
        let resolve_num = |id: BlockId, default: u64| -> u64 {
            match id {
                BlockId::Number(n) => n,
                _ => default,
            }
        };
        let from = resolve_num(from_block, 0);
        let to = resolve_num(to_block, bound).min(bound);
        self.with_db(move |db, _| {
            let rows = db.events_filtered(from, to, &filters)?;
            let mut out = Vec::with_capacity(rows.len());
            for e in rows {
                let block = db.block(e.block)?;
                out.push(EmittedEvent {
                    from_address: pool,
                    keys: e.keys,
                    data: e.data,
                    block_hash: block.as_ref().map(|b| b.hash),
                    block_number: Some(e.block),
                    transaction_hash: e.tx_hash,
                    event_index: e.event_index,
                    transaction_index: e.tx_index,
                });
            }
            Ok(out)
        })
        .await
    }

    fn block_id(&self) -> BlockId {
        self.inner.block_id
    }

    fn block_number(&self) -> u64 {
        self.inner.block
    }
}
