//! Each commit carries the moment the coordinator's log took it.
//!
//! ⚠️ **The coordinator's clock, not a producer's** (`M5.86`). A record's own
//! timestamp is client-supplied and unordered, so retention driven by it is
//! retention a client can defeat by backdating. What FR-33 reads is when the
//! log took the object, and this is the file that holds the coordinator to
//! stamping it.

#![allow(clippy::expect_used)]

use crate::support::{object, span};
use oqueue_coordinator::Coordinator;
use oqueue_core::{
    CommitVersion, CoordinatorEpoch, FakeClock, FakeMaterializedIndex, FakeMetadataLog,
    MetadataLog, MetadataRecord, Timestamp,
};
use std::sync::Arc;

#[tokio::test]
async fn a_commit_is_stamped_with_the_coordinator_s_clock() {
    let log: Arc<dyn MetadataLog> = Arc::new(FakeMetadataLog::new());
    let clock = Arc::new(FakeClock::new());
    let (coordinator, driver, _view) = Coordinator::open(
        Arc::clone(&log),
        Box::new(FakeMaterializedIndex::new()),
        CoordinatorEpoch::ZERO,
        Arc::clone(&clock) as Arc<dyn oqueue_core::Clock>,
    )
    .await
    .expect("a fresh log opens");
    let driver = tokio::spawn(driver.run());

    clock.advance(1_000).expect("the clock moves");
    coordinator
        .commit(object(0), vec![span(1)])
        .await
        .expect("the first commit lands");
    clock.advance(4_000).expect("the clock moves");
    coordinator
        .commit(object(1), vec![span(1)])
        .await
        .expect("the second commit lands");

    let written: Vec<Timestamp> = log
        .read_from(CommitVersion::ZERO, 64)
        .await
        .expect("the log reads back")
        .iter()
        .filter_map(|entry| match entry.record() {
            MetadataRecord::BatchCommitted { written_at, .. } => Some(*written_at),
            MetadataRecord::ManifestPublished { .. }
            | MetadataRecord::RangeCompacted { .. }
            | MetadataRecord::Trimmed { .. }
            | MetadataRecord::EpochChanged { .. }
            | MetadataRecord::TopicRetentionChanged { .. } => None,
        })
        .collect();
    assert_eq!(
        written,
        vec![
            Timestamp::from_millis(1_000).expect("valid"),
            Timestamp::from_millis(5_000).expect("valid"),
        ],
        "each commit carries the clock's reading when the log took it"
    );

    drop(coordinator);
    driver.await.expect("the driver stops cleanly");
}
