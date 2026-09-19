//! A coordinator whose loop replays its own log (`M6.10`): no hard dependency
//! at boot, and no write served before the replay lands.

#![allow(clippy::expect_used)]

use crate::support::{object, partition, span, topic};
use oqueue_coordinator::{Coordinator, CoordinatorError};
use oqueue_core::{
    CoordinatorEpoch, FakeClock, FakeMaterializedIndex, FakeMetadataLog, FaultMetadataLog,
    LogFaults, MetadataLog,
};
use std::sync::Arc;

/// A log already holding one two-record commit, behind a fault decorator.
async fn history() -> Arc<FaultMetadataLog<FakeMetadataLog>> {
    let log = Arc::new(FaultMetadataLog::new(FakeMetadataLog::new()));
    let (coordinator, serving, _reader) = Coordinator::open(
        Arc::clone(&log) as Arc<dyn MetadataLog>,
        Box::new(FakeMaterializedIndex::new()),
        CoordinatorEpoch::ZERO,
        Arc::new(FakeClock::new()),
    )
    .await
    .expect("opens");
    let serving = tokio::spawn(serving.run());
    coordinator
        .commit(object(0), vec![span(2)])
        .await
        .expect("commits");
    drop(coordinator);
    serving.await.expect("the loop ends");
    log
}

fn deferred(
    log: &Arc<FaultMetadataLog<FakeMetadataLog>>,
) -> (Coordinator, oqueue_coordinator::CoordinatorLoop) {
    let (coordinator, serving, _reader) = Coordinator::open_deferred(
        Arc::clone(log) as Arc<dyn MetadataLog>,
        Box::new(FakeMaterializedIndex::new()),
        CoordinatorEpoch::ZERO,
        Arc::new(FakeClock::new()),
    );
    (coordinator, serving)
}

fn refuse_reads(log: &FaultMetadataLog<FakeMetadataLog>, refuse: bool) {
    log.set_faults(LogFaults {
        refuse_read: refuse,
        ..LogFaults::default()
    });
}

/// ⚠️ **An unreplayed loop never writes**: its allocator is empty, and a commit
/// served from it would reuse the offsets the log already holds. It refuses
/// while the log is unreadable, and serves the next offset once it is not.
#[tokio::test(start_paused = true)]
async fn an_unreplayed_coordinator_refuses_writes_until_its_log_is_read() {
    let log = history().await;
    refuse_reads(&log, true);
    let (coordinator, serving) = deferred(&log);
    let serving = tokio::spawn(serving.run());
    assert_eq!(
        coordinator.commit(object(1), vec![span(1)]).await.err(),
        Some(CoordinatorError::Unavailable)
    );
    refuse_reads(&log, false);
    let ack = coordinator
        .commit(object(2), vec![span(1)])
        .await
        .expect("serves once replayed");
    assert_eq!(
        ack.base_offset(&topic(), partition()),
        2,
        "after the replayed two"
    );
    drop(coordinator);
    serving.await.expect("the loop ends");
}

/// `run_retrying` keeps trying on the runner's pause until the log reads, and
/// serves from the replayed line after.
#[tokio::test(start_paused = true)]
async fn a_retrying_loop_replays_once_the_log_can_be_read() {
    let log = history().await;
    refuse_reads(&log, true);
    let (coordinator, serving) = deferred(&log);
    let healer = Arc::clone(&log);
    let serving = tokio::spawn(serving.run_retrying(move || {
        refuse_reads(&healer, false);
        async {}
    }));
    let ack = coordinator
        .commit(object(1), vec![span(1)])
        .await
        .expect("served after the retried replay");
    assert_eq!(ack.base_offset(&topic(), partition()), 2);
    drop(coordinator);
    serving.await.expect("the loop ends");
}
