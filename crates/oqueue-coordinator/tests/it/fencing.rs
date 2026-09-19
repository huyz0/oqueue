//! A coordinator fenced by its lease (`M6.7`).

#![allow(clippy::expect_used)]

use crate::support::{object, partition, span, topic};
use oqueue_coordinator::{Coordinator, CoordinatorError};
use oqueue_core::{
    CoordinatorEpoch, FakeClock, FakeMaterializedIndex, FakeObjectStore, LEASE_SKEW_MS,
    LEASE_TTL_MS, MetadataLog, ObjectStore, ObjectStoreLease, ObjectStoreMetadataLog, Timestamp,
};
use std::sync::Arc;

/// A coordinator over the durable log in `store`, fenced by `lease`.
async fn fenced(
    store: &Arc<dyn ObjectStore>,
    clock: &Arc<FakeClock>,
    lease: Arc<ObjectStoreLease>,
) -> (Coordinator, tokio::task::JoinHandle<()>) {
    let log: Arc<dyn MetadataLog> = Arc::new(
        ObjectStoreMetadataLog::open(Arc::clone(store), "meta/0")
            .await
            .expect("the log opens"),
    );
    let (coordinator, serving, _reader) = Coordinator::open(
        log,
        Box::new(FakeMaterializedIndex::new()),
        CoordinatorEpoch::ZERO,
        Arc::clone(clock) as _,
    )
    .await
    .expect("opens");
    (coordinator, tokio::spawn(serving.fenced_by(lease).run()))
}

/// ⚠️ **Task 13: a coordinator that lost its lease and does not know**. It is
/// paused past its TTL — no renewal runs — while a successor takes the lease;
/// when it runs again it answers no commit, and the successor serves.
#[tokio::test(start_paused = true)]
async fn a_paused_coordinator_fences_itself() {
    let store: Arc<dyn ObjectStore> = Arc::new(FakeObjectStore::new());
    let clock = Arc::new(FakeClock::starting_at(
        Timestamp::from_millis(1_700_000_000_000).expect("a valid time"),
    ));
    let lease_for = |name: &str| {
        Arc::new(ObjectStoreLease::new(
            Arc::clone(&store),
            "meta/0",
            name,
            Arc::clone(&clock) as _,
        ))
    };

    let first_lease = lease_for("first");
    assert!(first_lease.acquire().await.expect("acquires"));
    let (first, first_serving) = fenced(&store, &clock, Arc::clone(&first_lease)).await;
    first
        .commit(object(0), vec![span(1)])
        .await
        .expect("the leader serves");

    // The pause: time passes and the first coordinator renews nothing.
    clock
        .advance(LEASE_TTL_MS + LEASE_SKEW_MS)
        .expect("the clock moves");
    let second_lease = lease_for("second");
    assert!(second_lease.acquire().await.expect("takes over"));
    let (second, second_serving) = fenced(&store, &clock, second_lease).await;

    assert_eq!(
        first.commit(object(1), vec![span(1)]).await.err(),
        Some(CoordinatorError::Fenced),
        "the paused leader, running again, assigns nothing"
    );
    let ack = second
        .commit(object(2), vec![span(1)])
        .await
        .expect("the successor serves");
    assert_eq!(
        ack.base_offset(&topic(), partition()),
        1,
        "after the first's one record"
    );

    drop((first, second));
    first_serving.await.expect("the loop ends");
    second_serving.await.expect("the loop ends");
}

/// A fenced loop refuses a trim as it refuses a commit (`M6.7`'s review).
#[tokio::test(start_paused = true)]
async fn a_fenced_coordinator_journals_no_trim() {
    let store: Arc<dyn ObjectStore> = Arc::new(FakeObjectStore::new());
    let clock = Arc::new(FakeClock::starting_at(
        Timestamp::from_millis(1_700_000_000_000).expect("a valid time"),
    ));
    let lease = Arc::new(ObjectStoreLease::new(
        Arc::clone(&store),
        "meta/0",
        "only",
        Arc::clone(&clock) as _,
    ));
    assert!(lease.acquire().await.expect("acquires"));
    let (coordinator, serving) = fenced(&store, &clock, lease).await;
    coordinator
        .commit(object(0), vec![span(2)])
        .await
        .expect("commits");
    clock.advance(LEASE_TTL_MS).expect("the clock moves");
    assert_eq!(
        coordinator
            .trim(
                topic(),
                partition(),
                oqueue_core::Offset::new(1).expect("an offset")
            )
            .await
            .err(),
        Some(CoordinatorError::Fenced)
    );
    drop(coordinator);
    serving.await.expect("the loop ends");
}
