//! `ConsumerStore` — the one seam between Block B and its host.
//!
//! Block B (feed apply/verify, the discovery passes, the note registry, the
//! report) is host-independent; everything that knows *where the bytes live* is
//! behind this trait. The native client implements it over SQLite
//! (`strk20_client::store::FeedStore`); the browser implements it over the
//! in-memory view (`crate::mem::MemStore`, which also serves the conformance
//! leg that proves the seam is real).
//!
//! Two rules keep the trait honest:
//!
//! 1. **No SQL, no async, no IO vocabulary leaks in.** Every method is a
//!    synchronous read or write of mirrored state; the only async in Block B is
//!    the engine's own and the feed transport's.
//! 2. **Writes that must not tear are single calls.** `install_snapshot` and
//!    `replace_range` each carry their rows, their metadata and (for the
//!    latter) the tail-generation bump, because the reorg discipline depends on
//!    a tail replacement and its generation bump landing together — a crash
//!    between them is what makes a per-owner cursor rewind silently not happen.

use anyhow::Result;
use discovery_core::events_backend::RawEventAccess;
use discovery_core::storage_backend::RawStorageAccess;
use starknet_types_core::felt::Felt;
use strk20_feed::codec::BlockLine;
use strk20_feed::snapshot::SnapSlot;

/// One row of the owner-scoped note registry.
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

/// A contiguous span of mirrored blocks addressed by a write.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Range {
    /// `from ..= to` — an epoch's own range.
    Inclusive { from: u64, to: u64 },
    /// Every block strictly above `floor` — the head tail.
    Above { floor: u64 },
}

impl Range {
    /// Does `block` fall inside this range?
    pub fn contains(&self, block: u64) -> bool {
        match *self {
            Range::Inclusive { from, to } => block >= from && block <= to,
            Range::Above { floor } => block > floor,
        }
    }
}

/// What `apply_feed` did and what the rest of the sync needs to know about it.
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

/// The mirror Block B folds into and reads back.
///
/// `Send + Sync` is required of the store and its view because `sync_once` is
/// an async function holding `&self` across awaits; a host whose state is not
/// shareable would have to wrap it, not weaken this.
pub trait ConsumerStore: Send + Sync {
    /// The engine-facing read view bound to one block. This is the *only*
    /// place the unmodified upstream engine touches the host.
    type View: RawStorageAccess + RawEventAccess + Send + Sync;

    // ------------------------------------------------------------------ meta

    fn meta_get(&self, key: &str) -> Result<Option<String>>;
    fn meta_set(&self, key: &str, value: &str) -> Result<()>;

    // ------------------------------------------------------- mirror, reading

    /// Is this mirror still unpopulated? Only then may a snapshot be applied:
    /// a snapshot is a floor, and laying one under existing rows would create
    /// a mirror whose history floor contradicts what it already holds.
    fn is_empty(&self) -> Result<bool>;

    /// Hash of the mirrored block `number`, if the mirror holds it.
    fn block_hash(&self, number: u64) -> Result<Option<Felt>>;

    /// Every mirrored `(number, hash)` inside `range`, ascending. This is what
    /// the reorg contradiction checks read before a range is replaced.
    fn block_hashes(&self, range: Range) -> Result<Vec<(u64, Felt)>>;

    /// Latest `(value, write_block)` for `slot` at or below `bound`.
    /// `(Felt::ZERO, 0)` when the mirror holds no write for it.
    fn read_slot_as_of(&self, slot: &Felt, bound: u64) -> Result<(Felt, u64)>;

    /// Complete pool slot set as of `block` (latest value per slot,
    /// zero-value rows excluded) — the input a client folds into a storage
    /// root when checking an anchor.
    fn full_slot_set_as_of(&self, block: u64) -> Result<Vec<(Felt, Felt)>>;

    /// A read view bound to `block` for the discovery engine. Implementations
    /// must refuse a bound below the snapshot basis; call
    /// [`crate::apply::check_bound_above_basis`] rather than re-deriving that
    /// rule.
    fn view(&self, block: u64) -> Result<Self::View>;

    // ------------------------------------------------------- mirror, writing

    /// Drop everything the feed put here and return to the pre-sync state.
    /// Identity metadata (pool, chain id, epoch size) survives: it is what a
    /// re-sync is checked against.
    fn reset_mirror(&self) -> Result<()>;

    /// Fold a verified snapshot's slot set in at the slots' real write blocks
    /// and set `meta`, atomically.
    fn install_snapshot(&self, slots: &[SnapSlot], meta: &[(&str, String)]) -> Result<()>;

    /// Replace every mirrored row in `range` with `blocks` (each paired with
    /// its L1-finality flag), set `meta`, and optionally bump the tail
    /// generation — atomically. The generation bump rides along precisely so a
    /// crash cannot separate a tail replacement from the counter that makes
    /// every owner rewind.
    fn replace_range(
        &self,
        range: Range,
        blocks: &[(&BlockLine, bool)],
        meta: &[(&str, String)],
        bump_generation: bool,
    ) -> Result<()>;

    /// Persisted tail-replacement counter (crash-safe, shared-store-safe).
    fn tail_generation(&self) -> Result<u64>;

    // -------------------------------------------------------- notes registry

    /// Registry rows for `owner`, ordered by `(token, index)`.
    fn notes(&self, owner: &Felt) -> Result<Vec<NoteRow>>;
    fn upsert_note(&self, n: &NoteRow) -> Result<()>;
    fn set_note_spent(&self, note_id: &Felt, spent: bool) -> Result<()>;
    fn delete_note(&self, note_id: &Felt) -> Result<()>;
    fn delete_owner_notes(&self, owner: &Felt) -> Result<usize>;
}
