//! Acceptance-test harness for the STRK20 indexer (spec §10.3): fixture
//! chain + RPC server, recording proxy, byte scanner, dual oracle, and real
//! binary process management.

pub mod bins;
pub mod chain;
pub mod feed_urls;
pub mod fixture;
pub mod oracle;
pub mod proxy;
pub mod rpc_server;
pub mod scanner;
pub mod snapshot_fmt;
pub mod storage_proof;
pub mod sse;
pub mod tcp_proxy;
