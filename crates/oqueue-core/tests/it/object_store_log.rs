//! The object-storage metadata log (`ADR-0046`, `M6.1`): the conformance
//! suite every `MetadataLog` passes, and the one property the fake cannot
//! have — its entries outlive the value that wrote them.

#![allow(clippy::expect_used)]
#![allow(unreachable_pub)]

use crate::metadata_log::{block_on, commit, conformance};
use oqueue_core::{
    CommitVersion, Error, FakeObjectStore, MetadataLog, ObjectKey, ObjectStore,
    ObjectStoreMetadataLog,
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

/// ⚠️ **A checkpoint folds the live segments into one snapshot, and a reopen
/// is that one GET plus the tail** (`M6.4`, `M6.5`): every entry comes back,
/// the segments the snapshot replaced are gone, and appends continue the line.
#[test]
fn a_checkpoint_snapshots_and_prunes_and_a_reopen_reads_everything() {
    let store: Arc<dyn ObjectStore> = Arc::new(FakeObjectStore::new());
    let log = block_on(ObjectStoreMetadataLog::open(Arc::clone(&store), "meta/0")).expect("opens");
    block_on(log.append(&[commit(0, 1)])).expect("appends");
    block_on(log.append(&[commit(1, 2)])).expect("appends");
    assert!(
        log.unsnapshotted_bytes() > 0,
        "appends count toward the trigger"
    );
    assert!(block_on(log.checkpoint()).expect("checkpoints"));
    assert_eq!(log.unsnapshotted_bytes(), 0, "and a checkpoint resets it");
    assert!(
        !block_on(log.checkpoint()).expect("an empty checkpoint"),
        "nothing new to fold"
    );
    let first_segment = ObjectKey::new("meta/0/log/00000000000000000000").expect("a key");
    assert!(
        matches!(
            block_on(store.get(&first_segment, oqueue_core::ByteRange::Full)),
            Err(Error::ObjectNotFound { .. })
        ),
        "the replaced segment is pruned"
    );
    block_on(log.append(&[commit(2, 3)])).expect("the line continues");
    assert!(block_on(log.checkpoint()).expect("a second checkpoint"));
    let first_snapshot = ObjectKey::new("meta/0/snap/00000000000000000001").expect("a key");
    assert!(
        matches!(
            block_on(store.get(&first_snapshot, oqueue_core::ByteRange::Full)),
            Err(Error::ObjectNotFound { .. })
        ),
        "the snapshot a newer one replaced is pruned too"
    );
    block_on(log.append(&[commit(3, 4)])).expect("appends after it");

    let reopened = block_on(ObjectStoreMetadataLog::open(Arc::clone(&store), "meta/0"))
        .expect("reopens from the newest base");
    assert_eq!(
        block_on(reopened.read_from(CommitVersion::ZERO, 16)).expect("reads"),
        vec![commit(0, 1), commit(1, 2), commit(2, 3), commit(3, 4)]
    );
    block_on(reopened.append(&[commit(4, 5)])).expect("and continues");
}

/// The newest base is found among many generations without a LIST.
#[test]
fn the_newest_of_many_checkpoints_is_found() {
    let store: Arc<dyn ObjectStore> = Arc::new(FakeObjectStore::new());
    let log = block_on(ObjectStoreMetadataLog::open(Arc::clone(&store), "meta/0")).expect("opens");
    for version in 0..13 {
        block_on(log.append(&[commit(version, 1)])).expect("appends");
        assert!(block_on(log.checkpoint()).expect("checkpoints"));
    }
    let reopened =
        block_on(ObjectStoreMetadataLog::open(Arc::clone(&store), "meta/0")).expect("reopens");
    let entries = block_on(reopened.read_from(CommitVersion::ZERO, 64)).expect("reads");
    assert_eq!(entries.len(), 13);
    block_on(reopened.append(&[commit(13, 1)])).expect("the next segment is free");
}

/// ⚠️ **A stale writer cannot checkpoint over a newer one**: once another log
/// has appended past it, its checkpoint is refused and the log still reads
/// whole.
#[test]
fn a_stale_writer_cannot_checkpoint() {
    let store: Arc<dyn ObjectStore> = Arc::new(FakeObjectStore::new());
    let stale =
        block_on(ObjectStoreMetadataLog::open(Arc::clone(&store), "meta/0")).expect("opens");
    block_on(stale.append(&[commit(0, 1)])).expect("appends");
    let current =
        block_on(ObjectStoreMetadataLog::open(Arc::clone(&store), "meta/0")).expect("opens");
    block_on(current.append(&[commit(1, 2)])).expect("the newer writer appends");
    assert!(
        block_on(stale.checkpoint()).is_err(),
        "the stale one is refused"
    );

    let reader =
        block_on(ObjectStoreMetadataLog::open(Arc::clone(&store), "meta/0")).expect("reopens");
    assert_eq!(
        block_on(reader.read_from(CommitVersion::ZERO, 16)).expect("reads"),
        vec![commit(0, 1), commit(1, 2)]
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
    let base = ObjectKey::new("meta/1/base/00000000000000000000").expect("a key");
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

/// The group log is the same segment log with its own format (`M6.6`): what
/// one appends, a second opened over the same store reads back.
#[test]
fn a_reopened_group_log_reads_back_every_append() {
    use oqueue_core::{
        GroupId, GroupMetadataEntry, GroupMetadataLog, GroupMetadataRecord,
        ObjectStoreGroupMetadataLog, TopicId,
    };
    let store: Arc<dyn ObjectStore> = Arc::new(FakeObjectStore::new());
    let entry = GroupMetadataEntry::new(
        CommitVersion::new(0),
        GroupMetadataRecord::OffsetCommitted {
            group: GroupId::new("secret-group").expect("a valid group"),
            topic: TopicId::new("orders").expect("a valid topic"),
            partition: 1,
            offset: 42,
        },
    );
    let first = block_on(ObjectStoreGroupMetadataLog::open(
        Arc::clone(&store),
        "groups/0",
    ))
    .expect("opens");
    block_on(first.append(core::slice::from_ref(&entry))).expect("appends");
    let shown = format!("{first:?}");
    assert!(
        shown.contains("groups/0") && shown.contains("entries: 1"),
        "{shown}"
    );
    assert!(!shown.contains("secret-group"), "{shown}");
    drop(first);

    let second = block_on(ObjectStoreGroupMetadataLog::open(
        Arc::clone(&store),
        "groups/0",
    ))
    .expect("reopens");
    assert_eq!(
        block_on(second.read_from(CommitVersion::ZERO, 16)).expect("reads"),
        vec![entry]
    );
    assert_eq!(
        block_on(second.last_version()).expect("reads"),
        Some(CommitVersion::new(0))
    );
}

/// A store that runs `hook` once, the first time `trigger` is read or
/// written — so a test can land another operation at exactly that moment.
struct Interposing {
    inner: FakeObjectStore,
    trigger: ObjectKey,
    hook: std::sync::Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl std::fmt::Debug for Interposing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Interposing")
    }
}

impl Interposing {
    fn fire(&self, key: &ObjectKey) {
        if *key == self.trigger {
            let hook = self.hook.lock().expect("unpoisoned").take();
            if let Some(hook) = hook {
                hook();
            }
        }
    }
}

impl ObjectStore for Interposing {
    fn get<'a>(
        &'a self,
        key: &'a ObjectKey,
        range: oqueue_core::ByteRange,
    ) -> oqueue_core::BoxFuture<'a, oqueue_core::Result<Vec<u8>>> {
        self.fire(key);
        self.inner.get(key, range)
    }
    fn put<'a>(
        &'a self,
        key: &'a ObjectKey,
        payload: Vec<u8>,
        precondition: Option<oqueue_core::Precondition>,
    ) -> oqueue_core::BoxFuture<'a, oqueue_core::Result<oqueue_core::ObjectMeta>> {
        self.fire(key);
        self.inner.put(key, payload, precondition)
    }
    fn open_multipart<'a>(
        &'a self,
        key: &'a ObjectKey,
    ) -> oqueue_core::BoxFuture<
        'a,
        oqueue_core::Result<Box<dyn oqueue_core::MultipartWriter<'a> + 'a>>,
    > {
        self.inner.open_multipart(key)
    }
    fn delete<'a>(
        &'a self,
        keys: &'a [ObjectKey],
    ) -> oqueue_core::BoxFuture<'a, oqueue_core::Result<()>> {
        self.inner.delete(keys)
    }
}

type Hook = std::sync::Mutex<Option<Box<dyn FnOnce() + Send>>>;

fn interposing(trigger: &str) -> Arc<Interposing> {
    Arc::new(Interposing {
        inner: FakeObjectStore::new(),
        trigger: ObjectKey::new(trigger).expect("a key"),
        hook: Hook::new(None),
    })
}

/// ⚠️ **An open that races a prune starts over** (`M6.4`'s review): a
/// checkpoint deletes segment 2 while a reader is between segments 1 and 2,
/// and the reader must not take the deleted key for the tail — it would then
/// append behind the new base, where no later open reads.
#[test]
fn an_open_racing_a_checkpoint_reads_from_the_new_base() {
    let store = interposing("meta/0/log/00000000000000000002");
    let shared: Arc<dyn ObjectStore> = Arc::clone(&store) as Arc<dyn ObjectStore>;
    let writer = Arc::new(
        block_on(ObjectStoreMetadataLog::open(Arc::clone(&shared), "meta/0")).expect("opens"),
    );
    for version in 0..5 {
        block_on(writer.append(&[commit(version, 1)])).expect("appends");
    }
    let pruner = Arc::clone(&writer);
    *store.hook.lock().expect("unpoisoned") = Some(Box::new(move || {
        assert!(block_on(pruner.checkpoint()).expect("checkpoints"));
    }));

    let reader = block_on(ObjectStoreMetadataLog::open(Arc::clone(&shared), "meta/0"))
        .expect("the racing open restarts and succeeds");
    assert_eq!(
        block_on(reader.read_from(CommitVersion::ZERO, 16))
            .expect("reads")
            .len(),
        5,
        "every entry, from the new base's snapshot"
    );
    assert!(
        block_on(reader.append(&[commit(5, 1)])).is_ok(),
        "and its next append lands at the real tail"
    );
    let later =
        block_on(ObjectStoreMetadataLog::open(Arc::clone(&shared), "meta/0")).expect("reopens");
    assert_eq!(
        block_on(later.read_from(CommitVersion::ZERO, 16))
            .expect("reads")
            .len(),
        6,
        "and a later open sees that append"
    );
}

/// ⚠️ **This process's own appends during a checkpoint do not refuse it**
/// (`M6.4`'s review): the coordinator keeps appending while a snapshot
/// uploads, and those appends are live segments past the new base.
#[test]
fn an_append_during_a_checkpoint_does_not_refuse_it() {
    let store = interposing("meta/0/snap/00000000000000000001");
    let shared: Arc<dyn ObjectStore> = Arc::clone(&store) as Arc<dyn ObjectStore>;
    let log = Arc::new(
        block_on(ObjectStoreMetadataLog::open(Arc::clone(&shared), "meta/0")).expect("opens"),
    );
    block_on(log.append(&[commit(0, 1)])).expect("appends");
    block_on(log.append(&[commit(1, 1)])).expect("appends");
    let appender = Arc::clone(&log);
    *store.hook.lock().expect("unpoisoned") = Some(Box::new(move || {
        block_on(appender.append(&[commit(2, 1)])).expect("an append mid-checkpoint");
    }));

    assert!(block_on(log.checkpoint()).expect("the checkpoint still commits"));
    let reopened =
        block_on(ObjectStoreMetadataLog::open(Arc::clone(&shared), "meta/0")).expect("reopens");
    assert_eq!(
        block_on(reopened.read_from(CommitVersion::ZERO, 16)).expect("reads"),
        vec![commit(0, 1), commit(1, 1), commit(2, 1)]
    );
}
