//! Driving Block B's `async` with no executor (§3.2).
//!
//! `discovery-core`'s entry points and `strk20-consumer`'s `sync_once` are
//! `async`, but over a `MemStore` view and a `StagedFeed` transport **no future
//! ever suspends**: every leaf resolves from a `BTreeMap` on its first poll. So
//! the whole pipeline runs to completion in a single `poll`, and the module
//! needs neither `wasm-bindgen-futures` nor a `block_on` — which is exactly what
//! keeps the ABI synchronous and the module free of a scheduler.
//!
//! `Pending` is treated as a **programming error, not a runtime path**. If a
//! future dependency ever introduces a leaf that genuinely suspends, this
//! panics on the first test run instead of hanging a browser tab on a waker
//! that will never be woken.

use std::future::Future;
use std::pin::pin;
use std::task::{Context, Poll, Waker};

/// Run `f` to completion with one poll. Panics if it suspends.
pub fn drive<F: Future>(f: F) -> F::Output {
    let mut f = pin!(f);
    let mut cx = Context::from_waker(Waker::noop());
    match f.as_mut().poll(&mut cx) {
        Poll::Ready(v) => v,
        Poll::Pending => panic!(
            "strk20-engine: an engine future pended over an in-memory view. Nothing in \
             Block B's wasm host may suspend — this is a dependency regression, not a \
             condition to retry."
        ),
    }
}
