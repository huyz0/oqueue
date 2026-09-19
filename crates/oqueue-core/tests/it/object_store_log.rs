//! The object-storage metadata log (`ADR-0046`, `M6.1`): the conformance
//! suite every `MetadataLog` passes, and the one property the fake cannot
//! have — its entries outlive the value that wrote them.

#![allow(clippy::expect_used)]
#![allow(unreachable_pub)]

use crate::metadata_log::{block_on, commit, conformance};
use oqueue_core::{
    CommitVersion, Error, FakeObjectStore, MetadataLog, ObjectKey, ObjectStore,
    ObjectStoreMetadataLog, Precondition,
};
use std::sync::Arc;

fn fresh() -> ObjectStoreMetadataLog {
    block_on(ObjectStoreMetadataLog::open(
        Arc::new(FakeObjectStore::new()),
        "meta/0",
    ))
    .expect("an empty prefix opens as an empty log")
}

/// ⚠️ Each case gets its own store, as the fake's run does.
#[test]
fn the_object_store_log_satisfies_the_conformance_suite() {
    conformance::append_then_read_returns_what_went_in(&fresh());
    conformance::an_out_of_order_batch_is_rejected(&fresh());
    conformance::a_repeated_version_is_rejected(&fresh());
    conformance::an_append_below_the_last_stored_version_is_rejected(&fresh());
    conformance::a_version_equal_to_the_last_stored_one_is_rejected(&fresh());
    conformance::a_rejected_append_stores_nothing(&fresh());
    conformance::read_from_starts_at_the_requested_version(&fresh());
    conformance::a_read_past_the_end_is_empty(&fresh());
    conformance::read_from_honours_its_page_bound(&fresh());
    conformance::an_empty_log_has_no_last_version(&fresh());
    conformance::last_version_follows_the_appends(&fresh());
    conformance::an_empty_append_is_a_no_op(&fresh());
    conformance::an_epoch_change_round_trips(&fresh());
    conformance::a_producer_identity_round_trips(&fresh());
}

/// ⚠️ **Guarantee 1, checkable for the first time**: a second log opened over
/// the same store reads back every entry the first appended.
#[test]
fn a_reopened_log_reads_back_every_append() {
    let store: Arc<dyn ObjectStore> = Arc::new(FakeObjectStore::new());
    let first =
        block_on(ObjectStoreMetadataLog::open(Arc::clone(&store), "meta/0")).expect("opens");
    block_on(first.append(&[commit(0, 1), commit(1, 2)])).expect("appends");
    block_on(first.append(&[commit(2, 3)])).expect("appends");
    drop(first);

    let second =
        block_on(ObjectStoreMetadataLog::open(Arc::clone(&store), "meta/0")).expect("reopens");
    let entries = block_on(second.read_from(CommitVersion::ZERO, 16)).expect("reads");
    assert_eq!(entries, vec![commit(0, 1), commit(1, 2), commit(2, 3)]);
    block_on(second.append(&[commit(3, 4)])).expect("and continues the line");
}

/// ⚠️ **The conditional write is the fence**: two logs over one store both
/// target the next segment, one lands, and the loser stays refused.
#[test]
fn two_writers_racing_leave_exactly_one_append() {
    let store: Arc<dyn ObjectStore> = Arc::new(FakeObjectStore::new());
    let a = block_on(ObjectStoreMetadataLog::open(Arc::clone(&store), "meta/0")).expect("opens");
    let b = block_on(ObjectStoreMetadataLog::open(Arc::clone(&store), "meta/0")).expect("opens");
    block_on(a.append(&[commit(0, 1)])).expect("the first lands");
    assert!(matches!(
        block_on(b.append(&[commit(0, 9)])),
        Err(Error::PreconditionFailed { .. })
    ));
    assert!(
        block_on(b.append(&[commit(1, 9)])).is_err(),
        "a fenced writer stays fenced"
    );

    let reader =
        block_on(ObjectStoreMetadataLog::open(Arc::clone(&store), "meta/0")).expect("reopens");
    assert_eq!(
        block_on(reader.read_from(CommitVersion::ZERO, 16)).expect("reads"),
        vec![commit(0, 1)]
    );
}

/// Opening is reading forward from the base: a base naming segment 1 skips
/// segment 0, which is how pruning will retire segments (`M6.5`).
#[test]
fn opening_starts_at_the_base() {
    let store: Arc<dyn ObjectStore> = Arc::new(FakeObjectStore::new());
    let writer =
        block_on(ObjectStoreMetadataLog::open(Arc::clone(&store), "meta/0")).expect("opens");
    block_on(writer.append(&[commit(0, 1)])).expect("appends");
    block_on(writer.append(&[commit(1, 2)])).expect("appends");
    let mut base = b"OQMB".to_vec();
    base.extend_from_slice(&1_u64.to_be_bytes());
    let key = ObjectKey::new("meta/0/base").expect("a key");
    block_on(store.put(&key, base, Some(Precondition::IfAbsent))).expect("a base");

    let reader =
        block_on(ObjectStoreMetadataLog::open(Arc::clone(&store), "meta/0")).expect("reopens");
    assert_eq!(
        block_on(reader.read_from(CommitVersion::ZERO, 16)).expect("reads"),
        vec![commit(1, 2)]
    );
}

/// A corrupt segment or base refuses the open rather than opening a shorter
/// log.
#[test]
fn a_corrupt_segment_or_base_refuses_the_open() {
    let store: Arc<dyn ObjectStore> = Arc::new(FakeObjectStore::new());
    let key = ObjectKey::new("meta/0/log/00000000000000000000").expect("a key");
    block_on(store.put(&key, b"garbage".to_vec(), None)).expect("a put");
    assert!(matches!(
        block_on(ObjectStoreMetadataLog::open(Arc::clone(&store), "meta/0")),
        Err(Error::MalformedMetadataSegment { .. })
    ));

    let other: Arc<dyn ObjectStore> = Arc::new(FakeObjectStore::new());
    let base = ObjectKey::new("meta/1/base").expect("a key");
    block_on(other.put(&base, b"OQMBshort".to_vec(), None)).expect("a put");
    assert!(block_on(ObjectStoreMetadataLog::open(Arc::clone(&other), "meta/1")).is_err());
}

/// `Debug` names the prefix and a count, never a record.
#[test]
fn debug_names_no_record() {
    let log = fresh();
    block_on(log.append(&[commit(0, 1)])).expect("appends");
    let shown = format!("{log:?}");
    assert!(
        shown.contains("meta/0") && shown.contains("entries: 1"),
        "{shown}"
    );
    assert!(
        !shown.contains("orders") && !shown.contains("obj"),
        "{shown}"
    );
}
