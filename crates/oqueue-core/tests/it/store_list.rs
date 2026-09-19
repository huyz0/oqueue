//! `MaintenanceStore::list` over the fake (`M7.3a`).
//!
//! ⚠️ **The ordering, paging and prefix cases themselves live in
//! `oqueue-store`'s conformance suite**, which runs them over this fake and
//! over S3. What is here is what that suite cannot see: that a deleted key's surviving
//! slot is not listed, and that the fake's fault injection reaches a listing
//! like any other call.

// The workspace denies `expect_used`; these sites are on values this test just
// constructed from literals it controls, so a panic means the test is wrong.
#![allow(clippy::expect_used)]

use oqueue_core::{
    Error, FakeObjectStore, FaultConfig, MaintenanceStore, ObjectKey, ObjectStore, StormKind,
};
use std::future::Future;
use std::task::{Context, Poll, Waker};

/// Drives a future to completion on this thread; the fake never pends except
/// under injected latency, which this file does not install.
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

fn seeded() -> FakeObjectStore {
    let store = FakeObjectStore::new();
    for name in ["t/b", "t/a", "t/c", "u/a"] {
        block_on(store.put(&key(name), vec![1], None)).expect("put succeeds");
    }
    store
}

fn names(keys: &[ObjectKey]) -> Vec<&str> {
    keys.iter().map(ObjectKey::as_str).collect()
}

#[test]
fn the_fake_lists_in_order_after_the_bound_within_the_limit() {
    let store = seeded();
    let all = block_on(store.list("t/", None, 10)).expect("list succeeds");
    assert_eq!(names(&all), ["t/a", "t/b", "t/c"]);
    let after = key("t/a");
    let page = block_on(store.list("t/", Some(&after), 1)).expect("list succeeds");
    assert_eq!(names(&page), ["t/b"]);
    assert!(
        block_on(store.list("t/", None, 0))
            .expect("list succeeds")
            .is_empty()
    );
}

#[test]
fn the_fake_does_not_list_a_deleted_key() {
    let store = seeded();
    block_on(store.delete(&[key("t/b")])).expect("delete succeeds");
    let all = block_on(store.list("t/", None, 10)).expect("list succeeds");
    assert_eq!(names(&all), ["t/a", "t/c"]);
}

#[test]
fn a_storm_reaches_list() {
    let store = seeded();
    store.set_faults(FaultConfig {
        storm: Some((StormKind::Transient, 1)),
        ..FaultConfig::default()
    });
    assert_eq!(block_on(store.list("t/", None, 10)), Err(Error::Transient));
    assert_eq!(
        block_on(store.list("t/", None, 10)).map(|k| k.len()),
        Ok(3),
        "a one-call storm is spent by the first list"
    );
}
