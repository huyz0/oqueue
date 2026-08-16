//! Op-class accounting: cost is a testable property of the seam.

// The workspace denies `expect_used`; every site here is on a value this
// test just constructed from a literal it controls, so a panic means the
// test is wrong.
#![allow(clippy::expect_used)]

use oqueue_core::{
    ByteRange, CountingObjectStore, FakeObjectStore, ObjectKey, ObjectStore, Operation,
};
use std::future::Future;
use std::task::{Context, Poll, Waker};

/// Drives a future to completion on this thread — same busy-poll driver
/// every seam test in this crate uses; no async runtime is compiled here.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = Box::pin(future);
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::hint::spin_loop(),
        }
    }
}

fn key(name: &str) -> ObjectKey {
    ObjectKey::new(name).expect("a non-empty key")
}

/// A freshly wrapped store starts every counter at zero.
#[test]
fn counts_start_at_zero() {
    let store = CountingObjectStore::new(FakeObjectStore::new());
    assert_eq!(store.counts().count(Operation::Get), 0);
    assert_eq!(store.counts().count(Operation::Put), 0);
    assert_eq!(store.counts().count(Operation::Delete), 0);
}

/// A sequence of calls through the wrapper increments exactly the matching
/// counters, and no others.
#[test]
fn a_sequence_of_calls_increments_exactly_the_matching_counters() {
    let store = CountingObjectStore::new(FakeObjectStore::new());
    let k = key("topics/orders/0/x.seg");

    block_on(store.put(&k, vec![1], None)).expect("put succeeds");
    block_on(store.put(&k, vec![2], None)).expect("put succeeds");
    block_on(store.put(&k, vec![3], None)).expect("put succeeds");
    block_on(store.get(&k, ByteRange::Full)).expect("get succeeds");
    block_on(store.delete(std::slice::from_ref(&k))).expect("delete succeeds");

    assert_eq!(store.counts().count(Operation::Put), 3);
    assert_eq!(store.counts().count(Operation::Get), 1);
    assert_eq!(store.counts().count(Operation::Delete), 1);
}

/// A call is counted even when it fails — S3 and GCS bill a failed
/// conditional write the same as a successful one, and undercounting a
/// failure would hide exactly the cost op-class accounting exists to make
/// visible.
#[test]
fn a_failing_call_is_still_counted() {
    let store = CountingObjectStore::new(FakeObjectStore::new());
    let k = key("topics/orders/0/missing.seg");

    let result = block_on(store.get(&k, ByteRange::Full));
    assert!(result.is_err(), "the key was never put, so get must fail");
    assert_eq!(
        store.counts().count(Operation::Get),
        1,
        "the failed call must still be counted"
    );
}

/// The wrapper is usable as a trait object and behaves exactly like the
/// store it wraps — wrapping changes nothing about `ObjectStore`'s
/// observable behaviour, only what is counted alongside it.
#[test]
fn the_wrapper_behaves_exactly_like_the_store_it_wraps() {
    let store: std::sync::Arc<dyn ObjectStore> =
        std::sync::Arc::new(CountingObjectStore::new(FakeObjectStore::new()));
    let k = key("topics/orders/0/wrapped.seg");

    block_on(store.put(&k, vec![7], None)).expect("put through a wrapped trait object");
    assert_eq!(block_on(store.get(&k, ByteRange::Full)), Ok(vec![7]));
}
