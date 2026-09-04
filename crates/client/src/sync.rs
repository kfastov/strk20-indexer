//! Sync orchestration — **moved**.
//!
//! Everything that was here (the checkpoint/live pass split, cursor re-open,
//! note registration and nullifier derivation, spent-state refresh, the reorg
//! rewind, the report) is Block B and now lives in `strk20_consumer::sync`,
//! generic over `ConsumerStore`, so the browser runs the same code rather than
//! a second implementation of it. This module is the native crate's stable
//! import path onto it.

pub use strk20_consumer::sync::{
    check_chain_id, full_resync, prune_missing_notes, refresh_spent, register_notes,
    register_scanned_notes, reopen_cursor, run_incoming, run_outgoing, sync_once, ReportNote,
    SyncOptions, SyncReport,
};
