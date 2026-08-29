//! strk20-client — the keyless discovery client (spec §7). Holds the viewing
//! key locally, downloads and verifies the public feed, and runs the
//! unmodified upstream discovery-core engine over the local mirror. Links no
//! server code (spec R5: the secret-bearing binary and the server binary are
//! separated at the crate graph level).
//!
//! # Privacy locks (compile-fail, spec §10.1)
//!
//! The viewing key type refuses serde serialization — a leak through any
//! serializer is a compile error:
//!
//! ```compile_fail
//! fn requires_serialize<T: serde::Serialize>() {}
//! requires_serialize::<discovery_core::privacy_pool::types::SecretFelt>();
//! ```
//!
//! No `FeedTransport` method accepts a user-derived value — asking the feed
//! about an address is unrepresentable:
//!
//! ```compile_fail
//! use strk20_client::transport::FeedTransport;
//! async fn leak(t: &dyn FeedTransport, address: starknet_types_core::felt::Felt) {
//!     let _ = t.fetch_manifest(address).await;
//! }
//! ```

pub mod store;
pub mod sync;
pub mod transport;
pub mod verify;
