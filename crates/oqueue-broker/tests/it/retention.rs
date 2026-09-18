//! FR-33 end to end: an idle partition's data is deleted on schedule, with no
//! write to trigger it (`M5.91`).
//!
//! ⚠️ **Nothing is produced after the first commit.** Time passing is the only
//! input, which is the whole of the requirement: retention driven by the next
//! produce never runs on the partition that most needs it.

#![allow(clippy::expect_used)]

use oqueue_broker::{RETENTION_ROUND_INTERVAL, Retention};
use oqueue_compact::{DEFAULT_RETENTION_MS, DELETION_DELAY_MS};
use oqueue_coordinator::Coordinator;
use oqueue_core::{
    ByteRange, CommitVersion, CommittedSpan, CoordinatorEpoch, Error, FakeClock,
    FakeMaterializedIndex, FakeMetadataLog, FakeObjectStore, MetadataLog, MetadataRecord,
    ObjectKey, ObjectStore, PartitionId, Timestamp, TopicId,
};
use std::sync::Arc;

/// Lets `rounds` retention rounds run.
async fn rounds(rounds: u32) {
    tokio::time::sleep(RETENTION_ROUND_INTERVAL * rounds).await;
}

/// A shard with retention running over it: the log, the clock, the store,
/// a coordinator handle, and the two tasks.
type Shard = (
    Arc<dyn MetadataLog>,
    Arc<FakeClock>,
    Arc<FakeObjectStore>,
    Coordinator,
    tokio::task::JoinHandle<()>,
    tokio::task::JoinHandle<()>,
);

async fn shard() -> Shard {
    let log: Arc<dyn MetadataLog> = Arc::new(FakeMetadataLog::new());
    // ⚠️ Not the epoch: a commit stamped there is never judged, the guard
    // `ExpiryHeap` keeps against a host clock set before 1970.
    let clock = Arc::new(FakeClock::starting_at(
        Timestamp::from_millis(1_700_000_000_000).expect("a valid time"),
    ));
    let store = Arc::new(FakeObjectStore::new());
    let (coordinator, serving, _reader) = Coordinator::open(
        Arc::clone(&log),
        Box::new(FakeMaterializedIndex::new()),
        CoordinatorEpoch::ZERO,
        clock.clone(),
    )
    .await
    .expect("a fresh log opens");
    let serving = tokio::spawn(serving.run());
    let retention = Retention::new(
        coordinator.clone(),
        Arc::clone(&log),
        Box::new(FakeMaterializedIndex::new()),
        store.clone(),
        clock.clone(),
    )
    .expect("the configured delay satisfies FR-35");
    let retention = tokio::spawn(retention.run());

    (log, clock, store, coordinator, serving, retention)
}

/// The one write the test makes: an object and its commit.
async fn write_once(store: &FakeObjectStore, coordinator: &Coordinator) -> ObjectKey {
    write(store, coordinator, "idle-0").await
}

/// One object named `name`, committed to the idle partition.
async fn write(store: &FakeObjectStore, coordinator: &Coordinator, name: &str) -> ObjectKey {
    let object = ObjectKey::new(name).expect("a valid key");
    store
        .put(&object, vec![1_u8; 16], None)
        .await
        .expect("the fake accepts a put");
    coordinator
        .commit(
            object.clone(),
            vec![CommittedSpan::new(
                TopicId::new("idle").expect("a valid topic"),
                PartitionId::new(0).expect("a valid partition"),
                4,
                ByteRange::Full,
                None,
            )],
        )
        .await
        .expect("the one and only write");

    object
}

#[tokio::test(start_paused = true)]
async fn an_idle_partition_is_reaped_with_no_write() {
    let (log, clock, store, coordinator, serving, retention) = shard().await;
    let object = write_once(&store, &coordinator).await;

    // Inside retention: kept.
    rounds(3).await;
    clock
        .advance(DEFAULT_RETENTION_MS - 1)
        .expect("the clock moves");
    rounds(3).await;
    assert!(
        store.get(&object, ByteRange::Full).await.is_ok(),
        "not yet expired"
    );

    // Past retention: trimmed, and then — once FR-35's delay has passed since
    // the lifecycle saw it unreferenced — deleted.
    clock.advance(2).expect("the clock moves");
    rounds(3).await;
    assert!(
        store.get(&object, ByteRange::Full).await.is_ok(),
        "trimmed, still waiting"
    );
    clock
        .advance(DELETION_DELAY_MS + 1)
        .expect("the clock moves");
    rounds(3).await;
    assert!(matches!(
        store.get(&object, ByteRange::Full).await,
        Err(Error::ObjectNotFound { .. })
    ));

    let entries = log
        .read_from(CommitVersion::ZERO, 16)
        .await
        .expect("the log reads");
    assert_eq!(
        entries.len(),
        2,
        "one commit and the trim: nothing was written to trigger it"
    );
    assert!(matches!(
        entries[1].record(),
        MetadataRecord::Trimmed { .. }
    ));

    // ⚠️ Retention holds a coordinator handle, so it is stopped first: the
    // loop ends only once every handle is gone.
    retention.abort();
    let _ = retention.await;
    drop(coordinator);
    serving.await.expect("the loop ends");
}

/// ⚠️ **What the log held before retention subscribed is read, not missed**:
/// a commit made before the task existed reaches it only through the
/// rebuild, and is reaped all the same.
#[tokio::test(start_paused = true)]
async fn a_partition_committed_before_retention_started_is_still_reaped() {
    let log: Arc<dyn MetadataLog> = Arc::new(FakeMetadataLog::new());
    let clock = Arc::new(FakeClock::starting_at(
        Timestamp::from_millis(1_700_000_000_000).expect("a valid time"),
    ));
    let store = Arc::new(FakeObjectStore::new());
    let (coordinator, serving, _reader) = Coordinator::open(
        Arc::clone(&log),
        Box::new(FakeMaterializedIndex::new()),
        CoordinatorEpoch::ZERO,
        clock.clone(),
    )
    .await
    .expect("a fresh log opens");
    let serving = tokio::spawn(serving.run());
    // ⚠️ Two objects, so the walk that finds what a trim drops has to keep
    // going past the first.
    let object = write_once(&store, &coordinator).await;
    let second = write(&store, &coordinator, "idle-1").await;

    let retention = Retention::new(
        coordinator.clone(),
        Arc::clone(&log),
        Box::new(FakeMaterializedIndex::new()),
        store.clone(),
        clock.clone(),
    )
    .expect("the configured delay satisfies FR-35");
    let shown = format!("{retention:?}");
    assert!(shown.contains("armed"), "{shown}");
    assert!(!shown.contains("idle"), "no topic in Debug: {shown}");
    let retention = tokio::spawn(retention.run());

    rounds(1).await;
    clock
        .advance(DEFAULT_RETENTION_MS + 1)
        .expect("the clock moves");
    rounds(3).await;
    clock
        .advance(DELETION_DELAY_MS + 1)
        .expect("the clock moves");
    rounds(3).await;
    assert!(matches!(
        store.get(&object, ByteRange::Full).await,
        Err(Error::ObjectNotFound { .. })
    ));
    assert!(matches!(
        store.get(&second, ByteRange::Full).await,
        Err(Error::ObjectNotFound { .. })
    ));

    retention.abort();
    let _ = retention.await;
    drop(coordinator);
    serving.await.expect("the loop ends");
}
