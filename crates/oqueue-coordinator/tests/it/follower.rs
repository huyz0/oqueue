//! Following the tail from outside the coordinator: what a subscriber gets,
//! what it does with an overlap, and how it is told to stop.
//!
//! ⚠️ **Split from `tail.rs` at the 500-line limit, along the concept.** That
//! file is about the *park* — what a reader in this process waits on. This one
//! is about the *stream* — what an agent keeping its own materialization
//! receives, which is doc 12 §4.4's other half.

#![allow(clippy::expect_used)]

use crate::support::{object, offset, parked, partition, span, start_indexed, topic};
use oqueue_coordinator::{Coordinator, CoordinatorError, DeltaLag};
use oqueue_core::{
    CommitVersion, CoordinatorEpoch, FakeMaterializedIndex, FakeMetadataLog, MaterializedIndex,
    MetadataEntry, MetadataLog, MetadataRecord, Offset,
};
use std::sync::Arc;

/// A coordinator over `log` with a fresh index — the setup every case here
/// shares, and one place to change when `open` grows an argument (`M5.86`
/// gave it a clock).
async fn opened(
    log: Arc<dyn MetadataLog>,
) -> (
    Coordinator,
    oqueue_coordinator::CoordinatorLoop,
    oqueue_core::IndexReader,
) {
    Coordinator::open(
        log,
        Box::new(FakeMaterializedIndex::new()),
        CoordinatorEpoch::ZERO,
        Arc::new(oqueue_core::FakeClock::new()),
    )
    .await
    .expect("a fresh log opens")
}

/// ⚠️ The documented bootstrap recipe, exercised rather than described. A
/// follower subscribes first and folds second, so the two overlap — and the
/// overlap is the follower's to strip, because an `apply` whose first entry is
/// already folded is refused **whole**, taking the new entries down with it.
#[tokio::test(start_paused = true)]
async fn a_follower_bootstraps_by_stripping_the_overlap_it_asked_for() {
    let log: Arc<dyn MetadataLog> = Arc::new(FakeMetadataLog::new());
    let (coordinator, driver, _view) = opened(Arc::clone(&log)).await;
    let driver = tokio::spawn(driver.run());

    for n in 0..2 {
        coordinator
            .commit(object(n), vec![span(1)])
            .await
            .expect("the commit lands");
    }

    // Subscribe first: everything from here on arrives twice, once through the
    // log below and once through the stream.
    let mut stream = coordinator.subscribe();
    for n in 2..4 {
        coordinator
            .commit(object(n), vec![span(1)])
            .await
            .expect("the commit lands");
    }

    let follower = FakeMaterializedIndex::new();
    let history = log
        .read_from(CommitVersion::ZERO, 64)
        .await
        .expect("the log reads back");
    follower.apply(&history).expect("the history folds");
    let folded = follower.applied_upto().expect("something was folded");

    // Two entries are waiting on the stream and both are already folded.
    let mut pushed = Vec::new();
    for _ in 0..2 {
        pushed.push(parked(stream.recv()).await.expect("a delta"));
    }
    assert!(
        follower.apply(&pushed).is_err(),
        "folding the overlap whole is refused, which is why it is stripped"
    );

    let fresh: Vec<MetadataEntry> = pushed
        .into_iter()
        .filter(|entry| entry.version() > folded)
        .collect();
    assert!(fresh.is_empty(), "the log had already caught up to both");
    follower.apply(&fresh).expect("an empty fold is a no-op");
    assert_eq!(
        follower.end_offset(&topic(), partition()),
        offset(4),
        "the follower converges on the coordinator's own line"
    );

    drop(coordinator);
    driver.await.expect("the loop ends");
}

/// ⚠️ **A live handle must not keep a dead loop's stream open.** `broadcast`
/// reports `Closed` only once every *sender* is gone, so a handle that held
/// one would park a follower forever against a loop that had already stopped —
/// and `IndexWatch` would already be reporting `false` for the same event,
/// which is the asymmetry that hides it.
#[tokio::test(start_paused = true)]
async fn a_follower_is_told_it_is_over_even_while_a_handle_is_still_held() {
    let (coordinator, driver, _view) = opened(Arc::new(FakeMetadataLog::new())).await;
    let mut stream = coordinator.subscribe();
    let mut watch = coordinator.watch();

    // ⚠️ The loop is gone while a handle is very much alive — never spawned
    // here; a task that panicked or was cancelled at shutdown in production.
    // Dropping the *last handle* is the orderly stop and is a different event;
    // this is the one a follower must not park through.
    drop(driver);

    assert!(
        !parked(watch.wait_for(CommitVersion::ZERO)).await,
        "the watch reports the stop"
    );
    assert_eq!(
        parked(stream.recv()).await,
        Err(DeltaLag::Closed),
        "and so does the stream, with a handle still alive"
    );
    assert_eq!(
        coordinator.commit(object(0), vec![span(1)]).await.err(),
        Some(CoordinatorError::Unavailable),
        "all three paths agree that it is over"
    );
}

/// ⚠️ A stopped coordinator with no handles left is a **terminal** condition,
/// and saying so is what stops a follower re-bootstrapping in a loop against
/// something that will never publish again.
#[tokio::test(start_paused = true)]
async fn a_follower_of_a_finished_coordinator_is_told_it_is_over() {
    let (coordinator, _index, driver) = start_indexed().await;
    let mut stream = coordinator.subscribe();
    drop(coordinator);
    driver.await.expect("the loop ends");

    assert_eq!(parked(stream.recv()).await, Err(DeltaLag::Closed));
}

/// A follower gets the entries themselves, event-shaped, so its own fold is
/// the identical one — doc 12 §4.4's push half.
#[tokio::test(start_paused = true)]
async fn a_follower_receives_the_committed_entries_in_order() {
    let (coordinator, _index, driver) = start_indexed().await;
    let mut stream = coordinator.subscribe();

    for n in 0..3 {
        coordinator
            .commit(object(n), vec![span(1)])
            .await
            .expect("the commit lands");
    }

    let follower = FakeMaterializedIndex::new();
    for expected in 0..3_u64 {
        let entry = parked(stream.recv()).await.expect("a delta");
        assert_eq!(entry.version(), CommitVersion::new(expected));
        assert!(matches!(
            entry.record(),
            MetadataRecord::BatchCommitted { .. }
        ));
        follower.apply(&[entry]).expect("the follower folds it");
    }
    assert_eq!(
        follower.end_offset(&topic(), partition()),
        Offset::new(3).expect("a valid offset"),
        "the pushed fold and the coordinator's own arrive at the same place"
    );

    drop(coordinator);
    driver.await.expect("the loop ends");
}

/// ⚠️ **A follower that falls behind is told, never silently skipped.** This is
/// where push hands over to pull: the gap is real, and folding what comes next
/// onto an index missing the middle is the double-count the fold refuses. What
/// the follower does about it — re-bootstrap from the log — is the history
/// half doc 12 §4.4 pairs with the push half.
#[tokio::test(start_paused = true)]
async fn a_follower_that_falls_behind_is_told_to_re_bootstrap() {
    let log: Arc<dyn MetadataLog> = Arc::new(FakeMetadataLog::new());
    let (coordinator, driver, _view) = opened(Arc::clone(&log)).await;
    let driver = tokio::spawn(driver.run());
    let mut stream = coordinator.subscribe();

    // Overrun the buffer without ever receiving.
    let overrun = oqueue_coordinator::DELTA_BUFFER_ENTRIES + 8;
    for n in 0..overrun {
        coordinator
            .commit(object(n), vec![span(1)])
            .await
            .expect("the commit lands");
    }

    let missed = match parked(stream.recv()).await {
        Err(DeltaLag::Lagged { missed }) => missed,
        other => panic!("expected a lag, got {other:?}"),
    };
    assert_eq!(missed, 8, "exactly what overran the buffer");

    // The history half: the log still has every entry, so the follower
    // re-bootstraps to the same place the pushed fold would have reached.
    let follower = FakeMaterializedIndex::new();
    let entries = log
        .read_from(CommitVersion::ZERO, overrun)
        .await
        .expect("the log reads back");
    follower
        .apply(&entries)
        .expect("the follower folds the log");
    assert_eq!(
        follower.end_offset(&topic(), partition()),
        Offset::new(i64::try_from(overrun).expect("a count that fits")).expect("a valid offset")
    );

    drop(coordinator);
    driver.await.expect("the loop ends");
}
