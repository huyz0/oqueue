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

use oqueue_core::{ByteRange, Error, FakeObjectStore, ObjectKey, ObjectStore};
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

    let meta = block_on(store.put(&k, payload.clone())).expect("put succeeds");
    let fetched = block_on(store.get(&k, ByteRange::Full)).expect("get succeeds");

    assert_eq!(fetched, payload);
    assert_eq!(meta.size, payload.len() as u64);
    assert_eq!(store.len(), 1);
}

/// A key that was never put is not found, rather than empty.
#[test]
fn a_key_that_was_never_put_is_not_found() {
    let store = FakeObjectStore::new();
    let k = key("topics/orders/0/absent.seg");

    assert_eq!(
        block_on(store.get(&k, ByteRange::Full)),
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

    assert_eq!(block_on(store.get(&k, ByteRange::Full)), Ok(Vec::new()));
    assert_eq!(store.len(), 1);
}

/// `put` overwrites, as its documentation says.
#[test]
fn putting_the_same_key_twice_keeps_the_second_value() {
    let store = FakeObjectStore::new();
    let k = key("topics/orders/0/00000000000000000000.seg");

    block_on(store.put(&k, vec![1, 2, 3])).expect("first put");
    block_on(store.put(&k, vec![4, 5])).expect("second put");

    assert_eq!(block_on(store.get(&k, ByteRange::Full)), Ok(vec![4, 5]));
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

    assert_eq!(block_on(store.get(&a, ByteRange::Full)), Ok(vec![1]));
    assert_eq!(block_on(store.get(&b, ByteRange::Full)), Ok(vec![2]));
    assert_eq!(store.len(), 2);
}

/// A bounded range reads exactly the requested slice, not the whole object.
#[test]
fn a_bounded_range_reads_exactly_the_requested_slice() {
    let store = FakeObjectStore::new();
    let k = key("topics/orders/0/range.seg");
    block_on(store.put(&k, vec![0, 1, 2, 3, 4, 5, 6, 7, 8, 9])).expect("put succeeds");

    let range = ByteRange::bounded(2, 3).expect("a valid range");
    assert_eq!(block_on(store.get(&k, range)), Ok(vec![2, 3, 4]));
}

/// A range flush against the object's exact end succeeds.
#[test]
fn a_bounded_range_ending_exactly_at_the_object_size_succeeds() {
    let store = FakeObjectStore::new();
    let k = key("topics/orders/0/edge.seg");
    block_on(store.put(&k, vec![0, 1, 2, 3])).expect("put succeeds");

    let range = ByteRange::bounded(1, 3).expect("a valid range");
    assert_eq!(block_on(store.get(&k, range)), Ok(vec![1, 2, 3]));
}

/// A range that extends past the object's actual size is a distinct error
/// from "not found" — the object exists, the range does not fit.
#[test]
fn a_bounded_range_past_the_object_size_is_out_of_bounds_not_missing() {
    let store = FakeObjectStore::new();
    let k = key("topics/orders/0/short.seg");
    block_on(store.put(&k, vec![0, 1, 2])).expect("put succeeds");

    let range = ByteRange::bounded(1, 5).expect("a valid range");
    assert_eq!(
        block_on(store.get(&k, range)),
        Err(Error::ByteRangeOutOfBounds {
            key: k,
            offset: 1,
            length: 5,
            object_size: 3,
        })
    );
}

/// `offset + length` overflowing `u64` is out of bounds, not a wrapped
/// in-bounds range.
#[test]
fn a_range_whose_end_would_overflow_is_out_of_bounds() {
    let store = FakeObjectStore::new();
    let k = key("topics/orders/0/overflow.seg");
    block_on(store.put(&k, vec![0, 1, 2])).expect("put succeeds");

    let range = ByteRange::bounded(u64::MAX, 5).expect("a valid range");
    assert_eq!(
        block_on(store.get(&k, range)),
        Err(Error::ByteRangeOutOfBounds {
            key: k,
            offset: u64::MAX,
            length: 5,
            object_size: 3,
        })
    );
}

/// A zero-length range is rejected at construction, not at the seam.
#[test]
fn a_zero_length_range_is_rejected_at_construction() {
    assert_eq!(ByteRange::bounded(0, 0), Err(Error::EmptyByteRange));
}

/// A successful `put` returns a precondition token that changes on overwrite,
/// and is never the object's bytes or a hash of them.
#[test]
fn each_put_returns_a_distinct_precondition_token() {
    let store = FakeObjectStore::new();
    let k = key("topics/orders/0/token.seg");

    let first = block_on(store.put(&k, vec![1, 2, 3])).expect("first put");
    let second = block_on(store.put(&k, vec![1, 2, 3])).expect("second put, same bytes");

    assert_ne!(
        first.precondition_token, second.precondition_token,
        "identical bytes must not produce identical tokens — a token is not a content hash"
    );
}

/// Deleting an object removes it, and a subsequent `get` is `ObjectNotFound`.
#[test]
fn deleting_a_key_removes_the_object() {
    let store = FakeObjectStore::new();
    let k = key("topics/orders/0/deleted.seg");
    block_on(store.put(&k, vec![1])).expect("put succeeds");

    block_on(store.delete(std::slice::from_ref(&k))).expect("delete succeeds");

    assert_eq!(
        block_on(store.get(&k, ByteRange::Full)),
        Err(Error::ObjectNotFound { key: k })
    );
    assert!(store.is_empty());
}

/// Deleting several keys at once removes exactly those, leaving the rest.
#[test]
fn deleting_several_keys_leaves_the_rest_untouched() {
    let store = FakeObjectStore::new();
    let a = key("topics/orders/0/a.seg");
    let b = key("topics/orders/0/b.seg");
    let c = key("topics/orders/0/c.seg");
    block_on(store.put(&a, vec![1])).expect("put a");
    block_on(store.put(&b, vec![2])).expect("put b");
    block_on(store.put(&c, vec![3])).expect("put c");

    block_on(store.delete(&[a.clone(), c.clone()])).expect("delete succeeds");

    assert!(block_on(store.get(&a, ByteRange::Full)).is_err());
    assert_eq!(block_on(store.get(&b, ByteRange::Full)), Ok(vec![2]));
    assert!(block_on(store.get(&c, ByteRange::Full)).is_err());
    assert_eq!(store.len(), 1);
}

/// A key's generation survives a delete — a `put` on a key that was deleted
/// and recreated never reuses a generation an earlier, now-deleted object
/// held, matching how neither S3's version id nor GCS's generation number is
/// ever reused after a delete.
#[test]
fn a_deleted_key_recreated_by_put_never_reuses_its_old_precondition_token() {
    let store = FakeObjectStore::new();
    let k = key("topics/orders/0/recreated.seg");

    let before_delete = block_on(store.put(&k, vec![1])).expect("first put");
    block_on(store.delete(std::slice::from_ref(&k))).expect("delete succeeds");
    let after_recreate = block_on(store.put(&k, vec![1])).expect("put recreates the key");

    assert_ne!(
        before_delete.precondition_token, after_recreate.precondition_token,
        "a token from before the delete must not match the recreated object's token"
    );
}

/// Deleting a key with nothing stored under it is not an error — batch
/// delete is idempotent, matching S3's and GCS's own behaviour.
#[test]
fn deleting_an_already_absent_key_succeeds() {
    let store = FakeObjectStore::new();
    let k = key("topics/orders/0/never-existed.seg");

    block_on(store.delete(std::slice::from_ref(&k))).expect("delete of an absent key succeeds");
}

/// Deleting zero keys is a legitimate no-op, not a special case to reject.
#[test]
fn deleting_an_empty_slice_succeeds() {
    let store = FakeObjectStore::new();
    block_on(store.delete(&[])).expect("delete of nothing succeeds");
}

/// The seam is usable as a trait object, which is what the composition root
/// needs — ADR-0002's whole reason for the boxed-future shape.
#[test]
fn the_seam_is_dyn_compatible() {
    let store: std::sync::Arc<dyn ObjectStore> = std::sync::Arc::new(FakeObjectStore::new());
    let k = key("topics/orders/0/dyn.seg");

    block_on(store.put(&k, vec![7])).expect("put through a trait object");
    assert_eq!(block_on(store.get(&k, ByteRange::Full)), Ok(vec![7]));
}

/// And the futures it returns are `Send`, so a spawned task can hold one.
#[test]
fn the_returned_futures_are_send() {
    fn assert_send<T: Send>(_: T) {}
    let store = FakeObjectStore::new();
    let k = key("topics/orders/0/send.seg");
    assert_send(store.get(&k, ByteRange::Full));
    assert_send(store.put(&k, vec![1]));
    assert_send(store.delete(std::slice::from_ref(&k)));
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
