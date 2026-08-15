//! The `ObjectStore` seam, driven without an async runtime.
//!
//! ⚠️ **No runtime, not even under `[dev-dependencies]`.** ADR-0001, ADR-0002
//! and `M0.16` all rest NFR-56's pre-commit floor on no M0 task compiling one,
//! and `check-layering.sh` never reads `[dev-dependencies]`, so nothing would
//! catch a violation. The seam's futures are driven here by `Waker::noop()` and
//! a busy poll, which needs nothing outside `std`.

// The workspace denies `expect_used`; these sites are on values this test just
// constructed from literals it controls, so a panic means the test is wrong.
#![allow(clippy::expect_used)]

use oqueue_core::{Error, FakeObjectStore, ObjectKey, ObjectStore};
use std::future::Future;
use std::task::{Context, Poll, Waker};

/// Drives a future to completion on this thread.
///
/// ⚠️ A busy poll is correct **only** because every future in this test
/// completes on its first poll — the fake does no I/O and never registers a
/// waker. Against a real backend this would spin forever, which is exactly why
/// it is confined to this file rather than offered as a helper.
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

/// The acceptance criterion: a put/get round trip through the seam.
#[test]
fn an_object_that_was_put_comes_back_byte_for_byte() {
    let store = FakeObjectStore::new();
    let k = key("topics/orders/0/00000000000000000000.seg");
    let payload = vec![0x00, 0xff, 0x42, 0x00, 0x7f];

    block_on(store.put(&k, payload.clone())).expect("put succeeds");
    let fetched = block_on(store.get(&k)).expect("get succeeds");

    assert_eq!(fetched, payload);
    assert_eq!(store.len(), 1);
}

/// A key that was never put is not found, rather than empty.
#[test]
fn a_key_that_was_never_put_is_not_found() {
    let store = FakeObjectStore::new();
    let k = key("topics/orders/0/absent.seg");

    assert_eq!(
        block_on(store.get(&k)),
        Err(Error::ObjectNotFound { key: k })
    );
    assert!(store.is_empty());
}

/// ⚠️ An empty object is a real object, and must not be confused with a missing
/// one — the distinction the index depends on when a segment has no records.
#[test]
fn an_empty_object_is_stored_and_is_not_the_same_as_a_missing_one() {
    let store = FakeObjectStore::new();
    let k = key("topics/orders/0/empty.seg");

    block_on(store.put(&k, Vec::new())).expect("put succeeds");

    assert_eq!(block_on(store.get(&k)), Ok(Vec::new()));
    assert_eq!(store.len(), 1);
}

/// `put` overwrites, as its documentation says.
#[test]
fn putting_the_same_key_twice_keeps_the_second_value() {
    let store = FakeObjectStore::new();
    let k = key("topics/orders/0/00000000000000000000.seg");

    block_on(store.put(&k, vec![1, 2, 3])).expect("first put");
    block_on(store.put(&k, vec![4, 5])).expect("second put");

    assert_eq!(block_on(store.get(&k)), Ok(vec![4, 5]));
    assert_eq!(store.len(), 1, "an overwrite is not a second object");
}

/// Distinct keys do not collide.
#[test]
fn distinct_keys_hold_distinct_objects() {
    let store = FakeObjectStore::new();
    let a = key("topics/orders/0/a.seg");
    let b = key("topics/orders/0/b.seg");

    block_on(store.put(&a, vec![1])).expect("put a");
    block_on(store.put(&b, vec![2])).expect("put b");

    assert_eq!(block_on(store.get(&a)), Ok(vec![1]));
    assert_eq!(block_on(store.get(&b)), Ok(vec![2]));
    assert_eq!(store.len(), 2);
}

/// The seam is usable as a trait object, which is what the composition root
/// needs — ADR-0002's whole reason for the boxed-future shape.
#[test]
fn the_seam_is_dyn_compatible() {
    let store: std::sync::Arc<dyn ObjectStore> = std::sync::Arc::new(FakeObjectStore::new());
    let k = key("topics/orders/0/dyn.seg");

    block_on(store.put(&k, vec![7])).expect("put through a trait object");
    assert_eq!(block_on(store.get(&k)), Ok(vec![7]));
}

/// And the futures it returns are `Send`, so a spawned task can hold one.
#[test]
fn the_returned_futures_are_send() {
    fn assert_send<T: Send>(_: T) {}
    let store = FakeObjectStore::new();
    let k = key("topics/orders/0/send.seg");
    assert_send(store.get(&k));
    assert_send(store.put(&k, vec![1]));
}

/// ⚠️ The fake's `Debug` must not print payloads — a payload is customer data,
/// and a shared test double is exactly what ends up in a failure dump. The
/// derived `Debug` *did* print them, via `Mutex`'s. Pinned rather than screened,
/// for the reason `redaction.rs` records: a containment check only rules out
/// the encodings it was written to think of.
#[test]
fn the_fake_never_prints_the_payloads_it_holds() {
    let store = FakeObjectStore::new();
    block_on(store.put(&key("topics/acme/0/x.seg"), vec![0xde, 0xad, 0xbe, 0xef]))
        .expect("put succeeds");

    assert_eq!(format!("{store:?}"), "FakeObjectStore { objects: 1 }");
    assert_eq!(
        format!("{store:#?}"),
        "FakeObjectStore {\n    objects: 1,\n}"
    );
}

/// `is_empty` distinguishes both states, not just the one the other tests use.
#[test]
fn is_empty_tracks_whether_anything_is_stored() {
    let store = FakeObjectStore::new();
    assert!(store.is_empty());
    block_on(store.put(&key("topics/acme/0/x.seg"), vec![1])).expect("put succeeds");
    assert!(!store.is_empty());
}
