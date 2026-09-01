//! `MemStore` — the in-memory [`ConsumerStore`], and the reason the seam is
//! more than a comment.
//!
//! Two jobs:
//!
//! 1. It is the store the **browser** host folds into (§3.2): no SQLite, no
//!    filesystem, no tokio blocking pool, nothing that cannot exist in a
//!    `wasm32-unknown-unknown` module.
//! 2. It is the **second implementation** the conformance leg needs. The
//!    existing suite runs Block B over SQLite only, and a suite with one impl
//!    cannot detect a missing abstraction — a `ConsumerStore` that had quietly
//!    kept a SQL assumption would stay green forever. Running the same feed
//!    bytes through the same state machine over both stores and demanding
//!    identical notes, balances, spent-state and report is what makes the
//!    extraction checkable rather than asserted.
//!
//! Behaviour is intentionally identical to the SQLite store, down to the
//! error names: `HISTORY_UNAVAILABLE` below a snapshot floor and
//! `BOUND_BELOW_SNAPSHOT` for a view under the basis both come from the shared
//! helpers, not from a re-derivation here.

use crate::apply::{check_bound_above_basis, history_floor};
use crate::store::{ConsumerStore, NoteRow, Range};
use anyhow::Result;
use async_trait::async_trait;
use discovery_core::events_backend::RawEventAccess;
use discovery_core::storage_backend::{RawStorageAccess, StorageError};
use starknet_core::types::{BlockId, EmittedEvent, StorageResult};
use starknet_types_core::felt::Felt;
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};
use strk20_feed::codec::BlockLine;
use strk20_feed::snapshot::SnapSlot;

type Key = [u8; 32];

fn k(f: &Felt) -> Key {
    f.to_bytes_be()
}

#[derive(Clone, Debug)]
struct BlockRec {
    hash: Felt,
    #[allow(dead_code)]
    parent: Felt,
    #[allow(dead_code)]
    timestamp: u64,
    #[allow(dead_code)]
    l1_final: bool,
}

#[derive(Clone, Debug)]
struct EventRec {
    tx_index: u64,
    tx_hash: Felt,
    keys: Vec<Felt>,
    data: Vec<Felt>,
}

#[derive(Default, Debug)]
struct Inner {
    meta: BTreeMap<String, String>,
    blocks: BTreeMap<u64, BlockRec>,
    /// `(slot, write_block) -> value`, exactly the shape of the SQLite
    /// `storage_log` table: history is kept, reads are as-of.
    storage: BTreeMap<(Key, u64), Felt>,
    /// Highest write block any `storage` row carries, ever. Only ever raised,
    /// so it is an upper bound and never lets a clear skip work it owed.
    ///
    /// `storage` is keyed `(slot, block)`, so a clear BY BLOCK cannot be a
    /// range query and has to scan. During a from-genesis replay every epoch's
    /// range lies strictly above everything already folded, and that scan found
    /// nothing 607 times over a map that was growing under it. This is the
    /// cheap proof that there is nothing to find.
    storage_max_block: u64,
    /// `(block, event_index) -> event`
    events: BTreeMap<(u64, u64), EventRec>,
    notes: BTreeMap<Key, NoteRow>,
}

/// `Range` as a pair of inclusive block bounds, so the `BTreeMap`s keyed by
/// block can answer with a range query instead of a full scan.
fn bounds(range: Range) -> (u64, u64) {
    match range {
        Range::Inclusive { from, to } => (from, to),
        Range::Above { floor } => (floor.saturating_add(1), u64::MAX),
    }
}

impl Inner {
    fn read_slot_as_of(&self, slot: &Felt, bound: u64) -> (Felt, u64) {
        let sk = k(slot);
        self.storage
            .range((sk, 0)..=(sk, bound))
            .next_back()
            .map(|((_, b), v)| (*v, *b))
            .unwrap_or((Felt::ZERO, 0))
    }

    fn slots(&self) -> Vec<Key> {
        let mut out: Vec<Key> = self.storage.keys().map(|(s, _)| *s).collect();
        out.dedup();
        out
    }

    fn clear_range(&mut self, range: Range) {
        let (lo, hi) = bounds(range);
        let victims: Vec<u64> = self.blocks.range(lo..=hi).map(|(n, _)| *n).collect();
        for n in &victims {
            self.blocks.remove(n);
        }
        // Keyed by block, so this is a range query.
        let dead: Vec<(u64, u64)> = self
            .events
            .range((lo, 0)..=(hi, u64::MAX))
            .map(|(k, _)| *k)
            .collect();
        for k in &dead {
            self.events.remove(k);
        }
        // Keyed by (slot, block), so this one cannot be. Scan only when a row
        // in range can actually exist.
        if lo <= self.storage_max_block {
            self.storage.retain(|(_, b), _| !range.contains(*b));
        }
    }

    fn apply_block_line(&mut self, b: &BlockLine, l1_final: bool) {
        self.blocks.insert(
            b.number,
            BlockRec {
                hash: b.hash,
                parent: b.parent,
                timestamp: b.timestamp,
                l1_final,
            },
        );
        for (slot, value) in &b.diffs {
            self.storage.insert((k(slot), b.number), *value);
        }
        if !b.diffs.is_empty() {
            self.storage_max_block = self.storage_max_block.max(b.number);
        }
        for e in &b.events {
            self.events.insert(
                (b.number, e.event_index),
                EventRec {
                    tx_index: e.tx_index,
                    tx_hash: e.tx_hash,
                    keys: e.keys.clone(),
                    data: e.data.clone(),
                },
            );
        }
    }
}

/// An in-memory mirror. Cloning shares the same state (an `Arc` handle), the
/// way opening the same SQLite file twice does.
#[derive(Clone, Default, Debug)]
pub struct MemStore {
    inner: Arc<Mutex<Inner>>,
}

impl MemStore {
    pub fn new() -> Self {
        Self::default()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Inner> {
        self.inner.lock().expect("mem store poisoned")
    }
}

impl ConsumerStore for MemStore {
    type View = MemView;

    fn meta_get(&self, key: &str) -> Result<Option<String>> {
        Ok(self.lock().meta.get(key).cloned())
    }

    fn meta_set(&self, key: &str, value: &str) -> Result<()> {
        self.lock().meta.insert(key.to_owned(), value.to_owned());
        Ok(())
    }

    fn is_empty(&self) -> Result<bool> {
        let g = self.lock();
        if g.meta.contains_key("last_epoch_applied") {
            return Ok(false);
        }
        Ok(g.blocks.is_empty() && g.storage.is_empty())
    }

    fn block_hash(&self, number: u64) -> Result<Option<Felt>> {
        Ok(self.lock().blocks.get(&number).map(|b| b.hash))
    }

    fn block_hashes(&self, range: Range) -> Result<Vec<(u64, Felt)>> {
        let (lo, hi) = bounds(range);
        Ok(self
            .lock()
            .blocks
            .range(lo..=hi)
            .map(|(n, b)| (*n, b.hash))
            .collect())
    }

    fn read_slot_as_of(&self, slot: &Felt, bound: u64) -> Result<(Felt, u64)> {
        Ok(self.lock().read_slot_as_of(slot, bound))
    }

    fn full_slot_set_as_of(&self, block: u64) -> Result<Vec<(Felt, Felt)>> {
        let g = self.lock();
        let mut out = Vec::new();
        for sk in g.slots() {
            let slot = Felt::from_bytes_be(&sk);
            let (value, _) = g.read_slot_as_of(&slot, block);
            if value != Felt::ZERO {
                out.push((slot, value));
            }
        }
        Ok(out)
    }

    fn view(&self, block: u64) -> Result<MemView> {
        check_bound_above_basis(self, block)?;
        Ok(MemView {
            inner: self.inner.clone(),
            bound: block,
            history_floor: history_floor(self)?,
        })
    }

    fn reset_mirror(&self) -> Result<()> {
        let mut g = self.lock();
        g.blocks.clear();
        g.storage.clear();
        g.storage_max_block = 0;
        g.events.clear();
        g.notes.clear();
        for key in [
            "last_epoch_applied",
            "last_epoch_hash",
            "last_epoch_to",
            "head_etag",
            "head_number",
            "head_hash",
            "l1_accepted",
            "snapshot_basis",
            "history_floor",
            "snapshot_pending_grounding",
        ] {
            g.meta.remove(key);
        }
        Ok(())
    }

    fn install_snapshot(&self, slots: &[SnapSlot], meta: &[(&str, String)]) -> Result<()> {
        let mut g = self.lock();
        for s in slots {
            g.storage.insert((k(&s.k), s.w), s.v);
            g.storage_max_block = g.storage_max_block.max(s.w);
        }
        for (key, value) in meta {
            g.meta.insert((*key).to_owned(), value.clone());
        }
        Ok(())
    }

    fn replace_range(
        &self,
        range: Range,
        blocks: &[(&BlockLine, bool)],
        meta: &[(&str, String)],
        bump_generation: bool,
    ) -> Result<()> {
        let mut g = self.lock();
        g.clear_range(range);
        for (b, l1_final) in blocks {
            g.apply_block_line(b, *l1_final);
        }
        for (key, value) in meta {
            g.meta.insert((*key).to_owned(), value.clone());
        }
        if bump_generation {
            let next = g
                .meta
                .get("tail_generation")
                .and_then(|s| s.parse::<u64>().ok())
                .unwrap_or(0)
                + 1;
            g.meta.insert("tail_generation".to_owned(), next.to_string());
        }
        Ok(())
    }

    fn tail_generation(&self) -> Result<u64> {
        Ok(self
            .lock()
            .meta
            .get("tail_generation")
            .and_then(|s| s.parse().ok())
            .unwrap_or(0))
    }

    fn notes(&self, owner: &Felt) -> Result<Vec<NoteRow>> {
        let g = self.lock();
        let mut rows: Vec<NoteRow> = g
            .notes
            .values()
            .filter(|n| n.owner == *owner)
            .cloned()
            .collect();
        // The SQLite store orders by `(token, idx)` with `token` a 32-byte BE
        // blob; matching that byte order keeps the two reports identical.
        rows.sort_by_key(|n| (k(&n.token), n.index));
        Ok(rows)
    }

    fn upsert_note(&self, n: &NoteRow) -> Result<()> {
        let mut g = self.lock();
        match g.notes.get_mut(&k(&n.note_id)) {
            // Same conflict clause as the SQLite upsert: sender, amount and
            // block are refreshed, `spent` is left to `refresh_spent`.
            Some(existing) => {
                existing.sender = n.sender;
                existing.amount = n.amount;
                existing.block = n.block;
            }
            None => {
                g.notes.insert(k(&n.note_id), n.clone());
            }
        }
        Ok(())
    }

    fn set_note_spent(&self, note_id: &Felt, spent: bool) -> Result<()> {
        if let Some(n) = self.lock().notes.get_mut(&k(note_id)) {
            n.spent = spent;
        }
        Ok(())
    }

    fn delete_note(&self, note_id: &Felt) -> Result<()> {
        self.lock().notes.remove(&k(note_id));
        Ok(())
    }

    fn delete_owner_notes(&self, owner: &Felt) -> Result<usize> {
        let mut g = self.lock();
        let before = g.notes.len();
        g.notes.retain(|_, n| n.owner != *owner);
        Ok(before - g.notes.len())
    }
}

/// Engine-facing read view bound to one block — the in-memory twin of
/// `ClientView`.
#[derive(Clone, Debug)]
pub struct MemView {
    inner: Arc<Mutex<Inner>>,
    bound: u64,
    /// Lowest block for which the event index can answer (§1.1). Below it it
    /// is not empty-because-nothing-happened, it is empty because a snapshot
    /// carries slots and no events.
    history_floor: u64,
}

impl MemView {
    pub fn bound(&self) -> u64 {
        self.bound
    }

    pub fn history_floor(&self) -> u64 {
        self.history_floor
    }

    fn read_one(&self, slot: &Felt) -> (Felt, u64) {
        self.inner
            .lock()
            .expect("mem store poisoned")
            .read_slot_as_of(slot, self.bound)
    }
}

#[async_trait]
impl RawStorageAccess for MemView {
    async fn read_slot(&self, slot: Felt) -> Result<Felt, StorageError> {
        Ok(self.read_one(&slot).0)
    }

    async fn read_slots(&self, slots: Vec<Felt>) -> Result<Vec<Felt>, StorageError> {
        Ok(slots.iter().map(|s| self.read_one(s).0).collect())
    }

    async fn read_slots_with_block(
        &self,
        slots: Vec<Felt>,
    ) -> Result<Vec<StorageResult>, StorageError> {
        Ok(slots
            .iter()
            .map(|s| {
                let (value, wb) = self.read_one(s);
                StorageResult {
                    value,
                    last_update_block: wb,
                }
            })
            .collect())
    }
}

#[async_trait]
impl RawEventAccess for MemView {
    async fn get_events(
        &self,
        keys: &[Vec<Felt>],
        from_block: BlockId,
        to_block: BlockId,
    ) -> Result<Vec<EmittedEvent>, StorageError> {
        let num = |id: BlockId, default: u64| match id {
            BlockId::Number(n) => n,
            _ => default,
        };
        let from = num(from_block, 0);
        let to = num(to_block, self.bound).min(self.bound);
        // R-L: a range reaching below the floor is a HARD ERROR, never a
        // clamped or silently truncated answer (see the SQLite twin).
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
        let g = self.inner.lock().expect("mem store poisoned");
        let mut out = Vec::new();
        for ((block, event_index), e) in g.events.range((from, 0)..=(to, u64::MAX)) {
            let matched = keys.iter().enumerate().all(|(i, allowed)| {
                allowed.is_empty() || e.keys.get(i).map(|x| allowed.contains(x)).unwrap_or(false)
            });
            if !matched {
                continue;
            }
            let Some(b) = g.blocks.get(block) else {
                // The SQLite twin joins events to blocks, so an event whose
                // block is gone is invisible there too.
                continue;
            };
            out.push(EmittedEvent {
                from_address: Felt::ZERO, // single-contract feed: the pool
                keys: e.keys.clone(),
                data: e.data.clone(),
                block_hash: Some(b.hash),
                block_number: Some(*block),
                transaction_hash: e.tx_hash,
                event_index: *event_index,
                transaction_index: e.tx_index,
            });
        }
        Ok(out)
    }

    fn block_id(&self) -> BlockId {
        BlockId::Number(self.bound)
    }

    fn block_number(&self) -> u64 {
        self.bound
    }
}
