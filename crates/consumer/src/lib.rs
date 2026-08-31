//! `strk20-consumer` — **Block B**, the consumer state machine, extracted from
//! the native client so it has exactly one implementation.
//!
//! The system is two blocks with one seam: Block A ingests the chain into a
//! public feed, Block B folds that feed and answers a key-holder's questions
//! about it. Block B has to run in two hosts — the native CLI over SQLite and
//! the browser over an in-memory view. Anything of Block B that stays welded to
//! one host becomes a *second implementation* in the other, and the moment
//! there are two implementations the equality claim the whole design rests on
//! ("the same bytes give the same answer everywhere") is gone.
//!
//! So everything here is host-independent by construction:
//!
//! * no `rusqlite`, no `tokio`, no `reqwest`, no filesystem;
//! * `cargo build -p strk20-consumer --target wasm32-unknown-unknown` is a
//!   gate, not an aspiration;
//! * the three host-shaped things Block B genuinely needs are traits —
//!   [`store::ConsumerStore`] (where rows live), [`transport::FeedTransport`]
//!   (how bytes arrive, and how they are inflated), and
//!   [`anchors::ProofSource`] (how the user's own RPC is asked for a storage
//!   proof).
//!
//! [`mem::MemStore`] is the second implementation of the first of those, and
//! the conformance leg in `e2e-tests` runs this crate's `sync_once` over both
//! it and the SQLite store from the same feed bytes, demanding identical
//! notes, balances, spent-state and report. That test is what makes the seam
//! checkable instead of merely claimed.

pub mod anchors;
pub mod apply;
pub mod mem;
pub mod store;
pub mod sync;
pub mod transport;

pub use store::{ApplyOutcome, ColdStart, ConsumerStore, NoteRow, Range};
pub use sync::{SyncOptions, SyncReport};
