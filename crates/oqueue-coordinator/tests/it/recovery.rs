//! FR-51 and NFR-20: a coordinator killed under load loses nothing it
//! acknowledged (`M6.3`).
//!
//! ⚠️ **Over the durable log, not the fake**: the successor opens
//! `ObjectStoreMetadataLog` afresh from the object store, so what it finds is
//! only what was made durable — the process's memory is gone with it.

#![allow(clippy::expect_used)]

use crate::support::{object, partition, span, topic};
use oqueue_coordinator::Coordinator;
use oqueue_core::{
    CoordinatorEpoch, FakeClock, FakeMaterializedIndex, FakeObjectStore, MetadataLog, ObjectKey,
    ObjectStore, ObjectStoreMetadataLog, Offset,
};
use std::sync::{Arc, Mutex};

async fn durable_log(store: &Arc<dyn ObjectStore>) -> Arc<dyn MetadataLog> {
    Arc::new(
        ObjectStoreMetadataLog::open(Arc::clone(store), "meta/0")
            .await
            .expect("the log opens"),
    )
}

/// A coordinator over the durable log in `store`, its loop spawned.
async fn opened(
    store: &Arc<dyn ObjectStore>,
) -> (
    Coordinator,
    tokio::task::JoinHandle<()>,
    oqueue_core::IndexReader,
) {
    let (coordinator, serving, reader) = Coordinator::open(
        durable_log(store).await,
        Box::new(FakeMaterializedIndex::new()),
        CoordinatorEpoch::ZERO,
        Arc::new(FakeClock::new()),
    )
    .await
    .expect("the durable log opens and replays");
    (coordinator, tokio::spawn(serving.run()), reader)
}

type Acked = Arc<Mutex<Vec<(ObjectKey, i64)>>>;

/// Eight producers, fifty commits each, recording every acknowledgement.
fn load(coordinator: &Coordinator, acked: &Acked) -> Vec<tokio::task::JoinHandle<()>> {
    (0..8)
        .map(|producer| {
            let (coordinator, acked) = (coordinator.clone(), Arc::clone(acked));
            tokio::spawn(async move {
                for n in 0..50 {
                    let name = object(producer * 1000 + n);
                    let Ok(ack) = coordinator.commit(name.clone(), vec![span(1)]).await else {
                        return;
                    };
                    let base = ack.base_offset(&topic(), partition());
                    acked.lock().expect("unpoisoned").push((name, base));
                    tokio::task::yield_now().await;
                }
            })
        })
        .collect()
}

/// ⚠️ **The kill lands mid-stream**: eight producers commit concurrently, the
/// coordinator's loop is aborted while commits are still queued, and every
/// commit that was acknowledged before the abort must be served by the
/// successor at the offset it was acknowledged at. What was never
/// acknowledged may or may not be there; the successor's next offset must
/// follow whatever is, with no gap and no reuse.
#[tokio::test(start_paused = true)]
async fn a_killed_coordinator_loses_no_acknowledged_record() {
    let store: Arc<dyn ObjectStore> = Arc::new(FakeObjectStore::new());
    let (coordinator, serving, _reader) = opened(&store).await;

    let acked: Acked = Arc::new(Mutex::new(Vec::new()));
    let producers = load(&coordinator, &acked);

    // Let some commits land, then kill the loop with others still queued.
    for _ in 0..40 {
        tokio::task::yield_now().await;
    }
    serving.abort();
    let _ = serving.await;
    for producer in producers {
        producer.await.expect("a producer ends");
    }
    drop(coordinator);
    let acked = acked.lock().expect("unpoisoned").clone();
    assert!(
        !acked.is_empty(),
        "the kill came after some acknowledgements"
    );
    assert!(acked.len() < 400, "and before all of them: {}", acked.len());

    let (successor, serving, reader) = opened(&store).await;

    for (name, base) in &acked {
        let at = Offset::new(*base).expect("a valid offset");
        let found = reader
            .find_batches(&topic(), partition(), at, u64::MAX)
            .expect("an acknowledged offset is readable");
        assert_eq!(
            found.first().map(|batch| batch.reference().object()),
            Some(name),
            "offset {base} is served from the object it was acknowledged for"
        );
    }
    let end = reader.end_offset(&topic(), partition());
    let next = successor
        .commit(object(99_999), vec![span(1)])
        .await
        .expect("the successor assigns");
    assert_eq!(
        next.base_offset(&topic(), partition()),
        end.get(),
        "the next offset follows the replayed line: no gap, no reuse"
    );

    drop(successor);
    serving.await.expect("the loop ends");
}

/// ⚠️ **The log is monotonic, not contiguous**, and a replay follows it: a
/// version above the next is taken as the next, so a commit after a replayed
/// gap takes the version after the gap.
#[tokio::test(start_paused = true)]
async fn a_replay_follows_a_gap_in_the_version_line() {
    use oqueue_core::{CommitVersion, FakeMetadataLog, MetadataEntry, MetadataRecord};
    let log = Arc::new(FakeMetadataLog::new());
    let epoch = |version| {
        MetadataEntry::new(
            CommitVersion::new(version),
            MetadataRecord::EpochChanged {
                epoch: CoordinatorEpoch::ZERO,
            },
        )
    };
    log.append(&[epoch(0), epoch(5)])
        .await
        .expect("a monotonic log");
    let (coordinator, serving, _reader) = Coordinator::open(
        log,
        Box::new(FakeMaterializedIndex::new()),
        CoordinatorEpoch::ZERO,
        Arc::new(FakeClock::new()),
    )
    .await
    .expect("a log with a gap replays");
    let serving = tokio::spawn(serving.run());
    let ack = coordinator
        .commit(object(0), vec![span(1)])
        .await
        .expect("commits");
    assert_eq!(ack.version(), CommitVersion::new(6));
    drop(coordinator);
    serving.await.expect("the loop ends");
}
