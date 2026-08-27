//! Assign → journal → ack: what the single coordinator promises about offsets.
//!
//! `M3.7`, and the FR-11 property the milestone's completion condition runs:
//! concurrent producers over one partition see exactly `0..k` with no gaps and
//! no overlaps. The other tests here are the ordering claims that make that
//! property hold rather than happen to hold — a position is journaled before
//! it is acknowledged, a refused journal consumes neither a version nor an
//! offset, and nothing on an error path reports `0`.

// Every `expect` below is on a value the test itself constructed, or on a step
// whose failure *is* the test failing. Same allowance, same reason, as
// `oqueue-core`'s own suites.
#![allow(clippy::expect_used)]

use crate::support::{object, partition, span, start, topic};
use oqueue_coordinator::{Coordinator, CoordinatorError, UNASSIGNED_OFFSET};
use oqueue_core::{
    CommitVersion, CoordinatorEpoch, Error, FakeMaterializedIndex, FakeMetadataLog,
    FaultMetadataLog, LogFaults, MetadataEntry, MetadataLog, PartitionId,
};
use std::sync::Arc;

#[tokio::test]
async fn a_position_is_journaled_before_it_is_acknowledged() {
    let log = Arc::new(FakeMetadataLog::new());
    let (coordinator, driver) = start(log.clone()).await;

    let ack = coordinator
        .commit(object(0), vec![span(3)])
        .await
        .expect("the commit lands");

    // The ack names a version the log is already holding — not one the
    // coordinator intends to write. `ADR-0020` point 3.
    let stored = log
        .read_from(CommitVersion::ZERO, 16)
        .await
        .expect("the log reads back");
    assert_eq!(stored.len(), 1, "the acknowledged commit is in the log");
    assert_eq!(stored[0].version(), ack.version());
    assert_eq!(ack.base_offset(&topic(), partition()), 0);

    drop(coordinator);
    driver
        .await
        .expect("the loop ends when its last handle drops");
}

#[tokio::test]
async fn a_commit_version_is_stamped_in_append_order() {
    let log = Arc::new(FakeMetadataLog::new());
    let (coordinator, driver) = start(log.clone()).await;

    let mut acked = Vec::new();
    for n in 0..3 {
        acked.push(
            coordinator
                .commit(object(n), vec![span(1)])
                .await
                .expect("the commit lands")
                .version(),
        );
    }

    let stored: Vec<CommitVersion> = log
        .read_from(CommitVersion::ZERO, 16)
        .await
        .expect("the log reads back")
        .iter()
        .map(MetadataEntry::version)
        .collect();
    assert_eq!(acked, stored, "every acked version is where the log put it");
    assert_eq!(
        stored,
        vec![
            CommitVersion::new(0),
            CommitVersion::new(1),
            CommitVersion::new(2)
        ],
        "one allocator, counting up, with no holes"
    );

    drop(coordinator);
    driver
        .await
        .expect("the loop ends when its last handle drops");
}

#[tokio::test]
async fn a_refused_journal_consumes_neither_a_version_nor_an_offset() {
    let log = Arc::new(FaultMetadataLog::new(FakeMetadataLog::new()));
    let (coordinator, driver) = start(log.clone()).await;

    log.set_faults(LogFaults {
        refuse_append: true,
        ..LogFaults::default()
    });
    let refused = coordinator
        .commit(object(0), vec![span(5)])
        .await
        .expect_err("a journal that refuses is not an ack");
    assert_eq!(refused, CoordinatorError::Journal(Error::Transient));
    assert_eq!(log.inner().len(), 0, "a refused append stores nothing");

    // The retry gets the position the refused attempt would have had. If the
    // failed attempt had advanced the allocator, this would start at offset 5
    // — a gap FR-11 forbids — or at version 1.
    log.set_faults(LogFaults::default());
    let ack = coordinator
        .commit(object(1), vec![span(5)])
        .await
        .expect("the retry lands");
    assert_eq!(ack.version(), CommitVersion::ZERO);
    assert_eq!(ack.base_offset(&topic(), partition()), 0);

    drop(coordinator);
    driver
        .await
        .expect("the loop ends when its last handle drops");
}

#[tokio::test]
async fn a_partition_no_ack_covers_reports_the_unset_sentinel() {
    let log = Arc::new(FakeMetadataLog::new());
    let (coordinator, driver) = start(log).await;

    let ack = coordinator
        .commit(object(0), vec![span(1)])
        .await
        .expect("the commit lands");

    let other = PartitionId::new(7).expect("7 is a valid partition");
    assert_eq!(
        ack.base_offset(&topic(), other),
        UNASSIGNED_OFFSET,
        "an uncovered partition has no offset, and says so"
    );
    assert_eq!(
        UNASSIGNED_OFFSET, -1,
        "the sentinel is -1; `0` is a plausible-looking offset and a safety bug"
    );

    drop(coordinator);
    driver
        .await
        .expect("the loop ends when its last handle drops");
}

#[tokio::test]
async fn a_coordinator_that_has_stopped_refuses_rather_than_answering() {
    let log = Arc::new(FakeMetadataLog::new());
    let (coordinator, driver, _view) = Coordinator::open(
        log,
        Box::new(FakeMaterializedIndex::new()),
        CoordinatorEpoch::ZERO,
    )
    .await
    .expect("a fresh log opens");
    drop(driver);

    assert_eq!(
        coordinator.commit(object(0), vec![span(1)]).await,
        Err(CoordinatorError::Unavailable),
        "no loop means no position — never an offset invented to fill the hole"
    );
}

#[tokio::test]
async fn opening_over_a_log_that_already_holds_entries_refuses() {
    let log = Arc::new(FakeMetadataLog::new());
    let (coordinator, driver) = start(log.clone()).await;
    coordinator
        .commit(object(0), vec![span(2)])
        .await
        .expect("the commit lands");
    drop(coordinator);
    driver.await.expect("the loop ends");

    // A second coordinator over the same log would restart its offset line at
    // zero, which is silent duplication rather than a failure. `M3.8` replays;
    // until it does, this refuses.
    assert_eq!(
        Coordinator::open(
            log,
            Box::new(FakeMaterializedIndex::new()),
            CoordinatorEpoch::ZERO
        )
        .await
        .err(),
        Some(CoordinatorError::ReplayRequired { last_version: 0 })
    );
}
