//! Hot standby (`M6.8`): failover as a promotion, not a rebuild.

#![allow(clippy::expect_used)]

use crate::support::{object, partition, span, topic};
use oqueue_coordinator::{Coordinator, Standby};
use oqueue_core::{
    CoordinatorEpoch, FakeClock, FakeMaterializedIndex, FakeObjectStore, MetadataLog, ObjectStore,
    ObjectStoreMetadataLog, Offset,
};
use std::sync::Arc;

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

/// ⚠️ **Failover is a promotion, not a rebuild**: the standby folded the log
/// as it grew, so promoting it folds only the tail since its last catch-up —
/// and it serves every record the leader acknowledged, at its offset.
#[tokio::test(start_paused = true)]
async fn a_promoted_standby_folds_only_the_tail_and_serves_everything() {
    let store: Arc<dyn ObjectStore> = Arc::new(FakeObjectStore::new());
    leader(&store, 20).await;
    let mut standby = Standby::new(Box::new(FakeMaterializedIndex::new()));
    assert_eq!(
        standby.catch_up(&*view(&store).await).await.expect("folds"),
        20
    );
    assert_eq!(
        standby.folded_upto(),
        Some(oqueue_core::CommitVersion::new(19))
    );
    let shown = format!("{standby:?}");
    assert!(shown.contains("last: Some"), "{shown}");
    assert!(!shown.contains("o-"), "names no object: {shown}");
    more(&store, 20, 5).await;
    assert_eq!(
        standby.catch_up(&*view(&store).await).await.expect("folds"),
        5,
        "a catch-up folds only what is new"
    );
    more(&store, 25, 3).await;

    let (coordinator, serving, reader) = standby
        .promote(
            view(&store).await as Arc<dyn MetadataLog>,
            CoordinatorEpoch::ZERO,
            Arc::new(FakeClock::new()),
        )
        .await
        .expect("promotes");
    let serving = tokio::spawn(serving.run());
    for i in 0..28 {
        let at = Offset::new(i64::try_from(i).expect("small")).expect("an offset");
        let found = reader
            .find_batches(&topic(), partition(), at, u64::MAX)
            .expect("readable");
        assert_eq!(
            found.first().map(|batch| batch.reference().object()),
            Some(&object(i))
        );
    }
    let ack = coordinator
        .commit(object(99), vec![span(1)])
        .await
        .expect("the promoted standby leads");
    assert_eq!(ack.base_offset(&topic(), partition()), 28);
    drop(coordinator);
    serving.await.expect("the loop ends");
}
