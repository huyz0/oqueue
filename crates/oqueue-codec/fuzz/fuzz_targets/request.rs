//! The whole request path, exactly as a socket delivers it: prelude,
//! `supports()` gate, every hand-rolled message decoder, and the handlers
//! over a live stub — `testing.md` rule 24's first-listed priority, and the
//! target whose first run found the `M2.26`/`M2.27` allocation `DoS`.
//!
//! ⚠️ **That `DoS` cannot recur here by construction now** (`ADR-0019`, `M2.34`):
//! no runtime code on this path imports `kafka-protocol`, and every hand-rolled
//! decoder bounds a count against the input before growing anything. No byte
//! string may panic or over-allocate; the worst legal outcome is `Close`.
#![no_main]

use libfuzzer_sys::fuzz_target;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

fuzz_target!(|data: &[u8]| {
    let cluster = Arc::new(oqueue_broker::StubCluster::new("fuzz", 1));
    cluster.ensure_topic("t");
    let dispatcher = oqueue_broker::Dispatcher::new(cluster);
    let future = oqueue_broker::Handler::handle(&dispatcher, data.to_vec());
    let mut future = Box::pin(future);
    let waker = Waker::noop();
    let mut context = Context::from_waker(&waker);
    // The dispatcher's future is ready-made (nothing awaits yet); a single
    // poll runs the entire decode-and-answer path.
    assert!(matches!(future.as_mut().poll(&mut context), Poll::Ready(_)));
});
