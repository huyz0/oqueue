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
    let task = tokio::spawn(checkpoints_at(
        Arc::clone(&log),
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
