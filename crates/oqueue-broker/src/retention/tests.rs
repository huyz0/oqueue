//! A follower that could not rebuild must not delete (`M5.91`'s review).

#![allow(clippy::expect_used)]

use super::Retention;
use oqueue_compact::{DEFAULT_RETENTION_MS, DELETION_DELAY_MS};
use oqueue_coordinator::Coordinator;
use oqueue_core::{
    BoxFuture, ByteRange, CommitVersion, CommittedSpan, CoordinatorEpoch, Error, FakeClock,
    FakeMaterializedIndex, FakeMetadataLog, FakeObjectStore, MetadataEntry, MetadataLog,
    MetadataRecord, ObjectKey, ObjectStore, PartitionId, Result, Timestamp, TopicId,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

/// A log whose reads fail while `failing` is set, and whose next
/// `refuse_trims` trim appends are refused.
#[derive(Debug, Default)]
struct Flaky {
    inner: FakeMetadataLog,
    failing: AtomicBool,
    refuse_trims: AtomicUsize,
}

impl MetadataLog for Flaky {
    fn append<'a>(&'a self, entries: &'a [MetadataEntry]) -> BoxFuture<'a, Result<()>> {
        let trims = entries
            .iter()
            .any(|entry| matches!(entry.record(), MetadataRecord::Trimmed { .. }));
        if trims
            && self
                .refuse_trims
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |n| n.checked_sub(1))
                .is_ok()
        {
            return Box::pin(async { Err(Error::Transient) });
        }
        self.inner.append(entries)
    }

    fn read_from(
        &self,
        start: CommitVersion,
        max_entries: usize,
    ) -> BoxFuture<'_, Result<Vec<MetadataEntry>>> {
        if self.failing.load(Ordering::SeqCst) {
            return Box::pin(async { Err(Error::Transient) });
        }
        self.inner.read_from(start, max_entries)
    }

    fn last_version(&self) -> BoxFuture<'_, Result<Option<CommitVersion>>> {
        self.inner.last_version()
    }
}

fn span(partition: i32) -> CommittedSpan {
    CommittedSpan::new(
        TopicId::new("t").expect("a valid topic"),
        PartitionId::new(partition).expect("a valid partition"),
        1,
        ByteRange::Full,
        None,
    )
}

/// A shard where one object is named by two partitions and only partition 0
/// has expired.
async fn shared_object() -> (
    Arc<Flaky>,
    Arc<FakeClock>,
    Arc<FakeObjectStore>,
    Coordinator,
    tokio::task::JoinHandle<()>,
    ObjectKey,
) {
    let log = Arc::new(Flaky::default());
    let clock = Arc::new(FakeClock::starting_at(
        Timestamp::from_millis(1_700_000_000_000).expect("a valid time"),
    ));
    let store = Arc::new(FakeObjectStore::new());
    let (coordinator, serving, _reader) = Coordinator::open(
        log.clone(),
        Box::new(FakeMaterializedIndex::new()),
        CoordinatorEpoch::ZERO,
        clock.clone(),
    )
    .await
    .expect("a fresh log opens");
    let serving = tokio::spawn(serving.run());
    let shared = ObjectKey::new("shared").expect("a valid key");
    store
        .put(&shared, vec![0_u8; 8], None)
        .await
        .expect("a put");
    coordinator
        .commit(shared.clone(), vec![span(0), span(1)])
        .await
        .expect("one object, two partitions");
    clock
        .advance(DEFAULT_RETENTION_MS - 1)
        .expect("the clock moves");
    // Partition 1 is written again, so only partition 0 expires.
    coordinator
        .commit(ObjectKey::new("later").expect("a valid key"), vec![span(1)])
        .await
        .expect("a later write to partition 1");

    (log, clock, store, coordinator, serving, shared)
}

/// ⚠️ **The blocking case**: an object partition 0 released and partition 1
/// still reads is held — and stays held across a rebuild that failed, where
/// an empty follower would have said nothing names it.
#[tokio::test(start_paused = true)]
async fn a_failed_rebuild_deletes_nothing() {
    let (log, clock, store, coordinator, serving, shared) = shared_object().await;
    let mut retention = Retention::new(
        coordinator.clone(),
        log.clone(),
        Box::new(FakeMaterializedIndex::new()),
        store.clone(),
        clock.clone(),
    )
    .expect("the configured delay satisfies FR-35");
    retention.rebuild().await;
    assert!(retention.synced);
    clock.advance(2).expect("the clock moves");
    assert!(retention.round().await, "partition 0 is trimmed");
    assert_eq!(retention.lifecycle.backlog(), 1, "shared is released");

    log.failing.store(true, Ordering::SeqCst);
    retention.rebuild().await;
    assert!(!retention.synced, "the rebuild failed");
    for _ in 0..3 {
        clock.advance(DELETION_DELAY_MS).expect("the clock moves");
        assert!(retention.round().await);
    }
    assert!(
        store.get(&shared, ByteRange::Full).await.is_ok(),
        "partition 1 still reads it"
    );

    log.failing.store(false, Ordering::SeqCst);
    clock.advance(DELETION_DELAY_MS).expect("the clock moves");
    assert!(retention.round().await);
    assert!(retention.synced, "the next round rebuilt it");
    assert!(
        store.get(&shared, ByteRange::Full).await.is_ok(),
        "and a synced follower still sees partition 1 naming it"
    );

    drop(retention);
    drop(coordinator);
    serving.await.expect("the loop ends");
}

/// ⚠️ **A trim the journal refused is retried, not forgotten**: the heap has
/// already popped the partition, and an idle one gets no commit to re-arm it.
#[tokio::test(start_paused = true)]
async fn a_refused_trim_is_retried_next_round() {
    let (log, clock, store, coordinator, serving, shared) = shared_object().await;
    let mut retention = Retention::new(
        coordinator.clone(),
        log.clone(),
        Box::new(FakeMaterializedIndex::new()),
        store.clone(),
        clock.clone(),
    )
    .expect("the configured delay satisfies FR-35");
    retention.rebuild().await;
    log.refuse_trims.store(1, Ordering::SeqCst);
    clock.advance(2).expect("the clock moves");
    assert!(retention.round().await);
    assert_eq!(
        retention.lifecycle.backlog(),
        0,
        "refused: nothing released"
    );
    assert!(retention.round().await);
    assert_eq!(retention.lifecycle.backlog(), 1, "retried: shared released");
    assert!(store.get(&shared, ByteRange::Full).await.is_ok());

    drop(retention);
    drop(coordinator);
    serving.await.expect("the loop ends");
}
