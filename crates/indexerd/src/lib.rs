//! strk20-indexerd — server side of the STRK20 open note indexer
//! (docs/spec/architecture.md). The canonical product is the feed directory;
//! this crate ingests the chain, maintains the SQLite mirror, cuts epochs,
//! and serves the HTTP surface.

pub mod bridge;
pub mod compat;
pub mod config;
pub mod cutter;
pub mod db;
pub mod ingest;
pub mod live;
pub mod rpc;
pub mod server;
pub mod stats;
