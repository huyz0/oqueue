//! A trim, journaled through the loop (`M5.90`).
//!
//! ⚠️ **The coordinator is the only writer of the log**, so a retention round
//! that decides a partition has expired has no way to act except this one.

#![allow(clippy::expect_used)]

use crate::support::{object, offset, partition, span, start, start_indexed, topic};
use oqueue_coordinator::CoordinatorError;
use oqueue_core::{CommitVersion, Error, FakeMetadataLog, MetadataLog, MetadataRecord};
use std::sync::Arc;

/// A trim takes the next version, lands in the log, and moves the index's log
/// start — a read below it is refused as below the start, not answered empty.
#[tokio::test(start_paused = true)]
async fn a_trim_is_journaled_and_folded() {
    let (coordinator, index, driver) = start_indexed().await;
    let mut stream = coordinator.subscribe();
    for n in 0..2 {
        coordinator
            .commit(object(n), vec![span(5)])
            .await
            .expect("the commit lands");
    }
    let version = coordinator
        .trim(topic(), partition(), offset(5))
        .await
        .expect("a trim inside the log");
    assert_eq!(version, CommitVersion::new(2), "the next version");

    for _ in 0..2 {
        stream.recv().await.expect("a commit delta");
    }
    let pushed = stream.recv().await.expect("the trim delta");
    assert!(matches!(
        pushed.record(),
        MetadataRecord::Trimmed { start, .. } if *start == offset(5)
    ));
    assert!(matches!(
        index.find_batches(&topic(), partition(), offset(0), u64::MAX),
        Err(Error::BelowLogStart { log_start: 5, .. })
    ));
    assert_eq!(
        index
            .find_batches(&topic(), partition(), offset(5), u64::MAX)
            .expect("from the start")
            .len(),
        1
    );

    drop(coordinator);
    driver.await.expect("the loop ends");
}

/// ⚠️ **Refused before the journal**: a trim past the end would be refused by
/// every fold of the log, forever. Nothing is written and no version is spent,
/// so the next commit takes the version the trim would have.
#[tokio::test(start_paused = true)]
async fn a_trim_past_the_end_is_refused_and_journals_nothing() {
    let log: Arc<dyn MetadataLog> = Arc::new(FakeMetadataLog::new());
    let (coordinator, driver) = start(Arc::clone(&log)).await;
    coordinator
        .commit(object(0), vec![span(3)])
        .await
        .expect("the commit lands");

    let refused = coordinator.trim(topic(), partition(), offset(4)).await;
    assert!(matches!(
        refused,
        Err(CoordinatorError::Unassignable(Error::TrimPastEnd {
            start: 4,
            end: 3,
            ..
        }))
    ));
    let at_end = coordinator.trim(topic(), partition(), offset(3)).await;
    assert_eq!(
        at_end.ok(),
        Some(CommitVersion::new(1)),
        "the end itself is allowed"
    );
    let ack = coordinator
        .commit(object(1), vec![span(1)])
        .await
        .expect("the next commit");
    assert_eq!(ack.version(), CommitVersion::new(2));
    assert_eq!(
        log.read_from(CommitVersion::ZERO, 16)
            .await
            .expect("the log reads")
            .len(),
        3,
        "the refused trim wrote nothing"
    );

    drop(coordinator);
    driver.await.expect("the loop ends");
}

#[tokio::test(start_paused = true)]
async fn a_trim_after_the_loop_stopped_is_unavailable() {
    let (coordinator, driver) = start(Arc::new(FakeMetadataLog::new())).await;
    driver.abort();
    let _ = driver.await;
    assert_eq!(
        coordinator
            .trim(topic(), partition(), offset(0))
            .await
            .err(),
        Some(CoordinatorError::Unavailable)
    );
}
