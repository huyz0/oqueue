//! Recovery time, measured two ways (`M6.13`, NFR-22, `ADR-0048`).
//!
//! ⚠️ **Here, in the I/O shell's tests, because it reads the real clock**,
//! which non-negotiable 5 keeps out of the coordinator crate's tests too.

#![allow(clippy::expect_used)]

use oqueue_coordinator::{Coordinator, Standby};
use oqueue_core::{
    ByteRange, CommittedSpan, CoordinatorEpoch, FakeClock, FakeMaterializedIndex, FakeObjectStore,
    MetadataLog, ObjectKey, ObjectStore, ObjectStoreMetadataLog, PartitionId, TopicId,
};
use std::sync::Arc;

fn topic() -> TopicId {
    TopicId::new("t").expect("a valid topic")
}

fn partition() -> PartitionId {
    PartitionId::new(0).expect("a valid partition")
}

fn object(n: usize) -> ObjectKey {
    ObjectKey::new(format!("o-{n}")).expect("a valid key")
}

fn span(records: u32) -> CommittedSpan {
    CommittedSpan::new(topic(), partition(), records, ByteRange::Full, None)
}

/// A fresh view of the durable log — another process's appends included.
async fn view(store: &Arc<dyn ObjectStore>) -> Arc<ObjectStoreMetadataLog> {
    Arc::new(
        ObjectStoreMetadataLog::open(Arc::clone(store), "meta/0")
            .await
            .expect("the log opens"),
    )
}

/// A leader over the durable log in `store` that has committed `n` records.
async fn leader(store: &Arc<dyn ObjectStore>, n: usize) -> Arc<ObjectStoreMetadataLog> {
    let log = view(store).await;
    let (coordinator, serving, _reader) = Coordinator::open(
        Arc::clone(&log) as Arc<dyn MetadataLog>,
        Box::new(FakeMaterializedIndex::new()),
        CoordinatorEpoch::ZERO,
        Arc::new(FakeClock::new()),
    )
    .await
    .expect("opens");
    let serving = tokio::spawn(serving.run());
    for i in 0..n {
        coordinator
            .commit(object(i), vec![span(1)])
            .await
            .expect("commits");
    }
    drop(coordinator);
    serving.await.expect("the loop ends");
    log
}

async fn more(store: &Arc<dyn ObjectStore>, from: usize, n: usize) {
    let (coordinator, serving, _reader) = Coordinator::open(
        view(store).await as Arc<dyn MetadataLog>,
        Box::new(FakeMaterializedIndex::new()),
        CoordinatorEpoch::ZERO,
        Arc::new(FakeClock::new()),
    )
    .await
    .expect("reopens");
    let serving = tokio::spawn(serving.run());
    for i in from..from + n {
        coordinator
            .commit(object(i), vec![span(1)])
            .await
            .expect("commits");
    }
    drop(coordinator);
    serving.await.expect("the loop ends");
}

/// ⚠️ **NFR-22, measured and not bounded** (`ADR-0048`): over one
/// checkpointed log, a hot standby's promotion and a cold rebuild are timed
/// separately and both reported. Nothing here asserts a bound or an order —
/// there is no target to hold them to — only that both were measured, over
/// the same log, and that both came up serving the same line.
#[tokio::test]
async fn failover_and_cold_rebuild_are_measured_separately() {
    let store: Arc<dyn ObjectStore> = Arc::new(FakeObjectStore::new());
    let log = leader(&store, 2_000).await;
    log.checkpoint().await.expect("a checkpoint");
    let mut standby = Standby::new(Box::new(FakeMaterializedIndex::new()));
    standby.catch_up(&*view(&store).await).await.expect("folds");
    more(&store, 2_000, 10).await;

    let started = std::time::Instant::now();
    let (_hot, _hot_serving, hot_reader) = standby
        .promote(
            view(&store).await as Arc<dyn MetadataLog>,
            CoordinatorEpoch::ZERO,
            Arc::new(FakeClock::new()),
        )
        .await
        .expect("promotes");
    let hot = started.elapsed();

    let started = std::time::Instant::now();
    let (_cold, _cold_serving, cold_reader) = Coordinator::open(
        view(&store).await as Arc<dyn MetadataLog>,
        Box::new(FakeMaterializedIndex::new()),
        CoordinatorEpoch::ZERO,
        Arc::new(FakeClock::new()),
    )
    .await
    .expect("rebuilds");
    let cold = started.elapsed();

    println!(
        "RTO hot_standby_us={} cold_rebuild_us={}",
        hot.as_micros(),
        cold.as_micros()
    );
    assert!(
        hot.as_nanos() > 0 && cold.as_nanos() > 0,
        "both were measured"
    );
    assert_eq!(
        hot_reader.end_offset(&topic(), partition()),
        cold_reader.end_offset(&topic(), partition()),
        "and both came up on the same line"
    );
}
