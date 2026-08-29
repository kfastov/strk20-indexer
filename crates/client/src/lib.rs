//! strk20-client — the keyless discovery client (spec §7). Holds the viewing
//! key locally, downloads and verifies the public feed, and runs the
//! unmodified upstream discovery-core engine over the local mirror. Links no
//! server code (spec R5: the secret-bearing binary and the server binary are
//! separated at the crate graph level).

pub mod store;
pub mod sync;
pub mod transport;
pub mod verify;
