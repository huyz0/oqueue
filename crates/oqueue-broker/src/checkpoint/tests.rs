//! The cadence checkpoints on bytes, and only on bytes.

#![allow(clippy::expect_used)]

use super::checkpoints_at;
use core::time::Duration;
use oqueue_core::{
    ByteRange, CommitVersion, CoordinatorEpoch, FakeObjectStore, MetadataEntry, MetadataLog,
    MetadataRecord, ObjectStore, ObjectStoreMetadataLog,
};
use std::sync::Arc;

fn entry(version: u64) -> MetadataEntry {
    MetadataEntry::new(
        CommitVersion::new(version),
        MetadataRecord::EpochChanged {
            epoch: CoordinatorEpoch::ZERO,
        },
    )
}

#[tokio::test(start_paused = true)]
async fn a_checkpoint_follows_the_journal_not_the_clock() {
    let store: Arc<dyn ObjectStore> = Arc::new(FakeObjectStore::new());
    let log = Arc::new(
        ObjectStoreMetadataLog::open(Arc::clone(&store), "meta/0")
            .await
            .expect("opens"),
    );
    log.append(&[entry(0)]).await.expect("appends");
    let one = log.unsnapshotted_bytes();
    let current = Arc::new(std::sync::Mutex::new(Arc::clone(&log)));
    let task = tokio::spawn(checkpoints_at(
        current,
        None,
        one * 3,
        Duration::from_secs(1),
    ));

    tokio::time::sleep(Duration::from_mins(1)).await;
    assert_eq!(
        log.unsnapshotted_bytes(),
        one,
        "time alone checkpoints nothing"
    );

    log.append(&[entry(1)]).await.expect("appends");
    log.append(&[entry(2)]).await.expect("appends");
    tokio::time::sleep(Duration::from_secs(2)).await;
    assert_eq!(
        log.unsnapshotted_bytes(),
        0,
        "three appends' worth triggers one"
    );
    task.abort();

    let first = oqueue_core::ObjectKey::new("meta/0/log/00000000000000000000").expect("a key");
    assert!(
        store.get(&first, ByteRange::Full).await.is_err(),
        "and it pruned"
    );
}

/// ⚠️ **Only the leader checkpoints** (`M6.17`): with a lease this node does
/// not hold, the cadence leaves the journal alone however much has piled up.
#[tokio::test(start_paused = true)]
async fn a_node_without_the_lease_does_not_checkpoint() {
    let store: Arc<dyn ObjectStore> = Arc::new(FakeObjectStore::new());
    let log = Arc::new(
        ObjectStoreMetadataLog::open(Arc::clone(&store), "meta/0")
            .await
            .expect("opens"),
    );
    log.append(&[entry(0)]).await.expect("appends");
    let clock = Arc::new(oqueue_core::FakeClock::new());
    let lease = Arc::new(oqueue_core::ObjectStoreLease::new(
        Arc::clone(&store),
        "meta/0",
        "never-acquired",
        clock,
    ));
    let current = Arc::new(std::sync::Mutex::new(Arc::clone(&log)));
    let task = tokio::spawn(checkpoints_at(
        current,
        Some(lease),
        1,
        Duration::from_secs(1),
    ));
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert!(log.unsnapshotted_bytes() > 0, "nothing was checkpointed");
    task.abort();
}
