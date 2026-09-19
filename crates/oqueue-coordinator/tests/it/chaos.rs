//! FR-51 under a leadership change with the log moving underneath it
//! (`M6.17`, M6's closing review): a node that booted while another led,
//! then took the lease after that leader wrote more and pruned, loses
//! nothing and reuses no offset.

#![allow(clippy::expect_used)]

use crate::support::{object, partition, span, topic};
use oqueue_coordinator::{Coordinator, CoordinatorError, CoordinatorLoop, Reopen};
use oqueue_core::{
    CoordinatorEpoch, FakeClock, FakeMaterializedIndex, FakeObjectStore, IndexReader,
    LEASE_SKEW_MS, LEASE_TTL_MS, MetadataLog, ObjectKey, ObjectStore, ObjectStoreLease,
    ObjectStoreMetadataLog, Offset, Timestamp,
};
use std::sync::Arc;

fn reopener(store: &Arc<dyn ObjectStore>) -> Reopen {
    let store = Arc::clone(store);
    Box::new(move || {
        let store = Arc::clone(&store);
        Box::pin(async move {
            let log = ObjectStoreMetadataLog::open(store, "meta/0").await?;
            Ok(Arc::new(log) as Arc<dyn MetadataLog>)
        })
    })
}

/// A node over `store`: its own log view, opened now, and its own lease.
async fn node(
    store: &Arc<dyn ObjectStore>,
    clock: &Arc<FakeClock>,
    name: &str,
) -> (
    Coordinator,
    CoordinatorLoop,
    IndexReader,
    Arc<ObjectStoreLease>,
    Arc<ObjectStoreMetadataLog>,
) {
    let log = Arc::new(
        ObjectStoreMetadataLog::open(Arc::clone(store), "meta/0")
            .await
            .expect("opens"),
    );
    let lease = Arc::new(ObjectStoreLease::new(
        Arc::clone(store),
        "meta/0",
        name,
        Arc::clone(clock) as _,
    ));
    let (coordinator, serving, reader) = Coordinator::open(
        Arc::clone(&log) as Arc<dyn MetadataLog>,
        Box::new(FakeMaterializedIndex::new()),
        CoordinatorEpoch::ZERO,
        Arc::clone(clock) as _,
    )
    .await
    .expect("replays at boot");
    let serving = serving
        .fenced_by(Arc::clone(&lease))
        .reopening_with(reopener(store));
    assert!(
        format!("{serving:?}").contains("Reopener"),
        "the loop says it can reopen, and nothing more"
    );
    (coordinator, serving, reader, lease, log)
}

async fn commit_all(
    coordinator: &Coordinator,
    range: core::ops::Range<usize>,
    acked: &mut Vec<(ObjectKey, i64)>,
) {
    for i in range {
        let ack = coordinator
            .commit(object(i), vec![span(1)])
            .await
            .expect("the leader commits");
        acked.push((object(i), ack.base_offset(&topic(), partition())));
    }
}

/// Every acknowledged offset is served from the object it was acknowledged for.
fn assert_served(reader: &IndexReader, acked: &[(ObjectKey, i64)]) {
    for (name, base) in acked {
        let at = Offset::new(*base).expect("an offset");
        let found = reader
            .find_batches(&topic(), partition(), at, u64::MAX)
            .expect("readable");
        assert_eq!(
            found.first().map(|batch| batch.reference().object()),
            Some(name),
            "offset {base}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn a_successor_that_booted_early_loses_nothing() {
    let store: Arc<dyn ObjectStore> = Arc::new(FakeObjectStore::new());
    let clock = Arc::new(FakeClock::starting_at(
        Timestamp::from_millis(1_700_000_000_000).expect("a valid time"),
    ));
    let mut acked = Vec::new();

    let (a, a_loop, _a_reader, a_lease, _a_log) = node(&store, &clock, "a").await;
    assert!(a_lease.acquire().await.expect("a leads"));
    let a_serving = tokio::spawn(a_loop.run());
    commit_all(&a, 0..10, &mut acked).await;

    // B boots now, replaying A's first ten, while A still leads.
    let (b, b_loop, _b_reader, b_lease, _b_log) = node(&store, &clock, "b").await;
    let b_serving = tokio::spawn(b_loop.run());
    assert_eq!(
        b.commit(object(500), vec![span(1)]).await.err(),
        Some(CoordinatorError::Fenced),
        "B does not lead yet"
    );

    // A writes ten more and checkpoints, pruning the segments B last saw.
    commit_all(&a, 10..20, &mut acked).await;
    let current = ObjectStoreMetadataLog::open(Arc::clone(&store), "meta/0")
        .await
        .expect("a current view");
    assert!(current.checkpoint().await.expect("checkpoints"));

    // A's lease lapses; B takes it and writes.
    clock
        .advance(LEASE_TTL_MS + LEASE_SKEW_MS)
        .expect("the clock moves");
    assert!(b_lease.acquire().await.expect("B takes over"));
    let ack = b
        .commit(object(20), vec![span(1)])
        .await
        .expect("the new leader commits");
    assert_eq!(
        ack.base_offset(&topic(), partition()),
        20,
        "after everything A acknowledged, not after what B saw at boot"
    );
    acked.push((object(20), 20));
    assert_eq!(
        a.commit(object(501), vec![span(1)]).await.err(),
        Some(CoordinatorError::Fenced),
        "and A writes nothing more"
    );

    // Every acknowledgement survives, read from the store alone.
    let (_c, _c_loop, fresh, _c_lease, _c_log) = node(&store, &clock, "c").await;
    assert_served(&fresh, &acked);
    assert_eq!(fresh.end_offset(&topic(), partition()).get(), 21);
    drop((a, b));
    a_serving.await.expect("the loop ends");
    b_serving.await.expect("the loop ends");
}

/// A view whose first read answers and every later one fails — a replay that
/// gets its first page and then loses the store.
#[derive(Debug)]
struct FirstPageOnly {
    inner: Arc<dyn MetadataLog>,
    reads: std::sync::atomic::AtomicU32,
}

impl MetadataLog for FirstPageOnly {
    fn append<'a>(
        &'a self,
        entries: &'a [oqueue_core::MetadataEntry],
    ) -> oqueue_core::BoxFuture<'a, oqueue_core::Result<()>> {
        self.inner.append(entries)
    }

    fn read_from(
        &self,
        start: oqueue_core::CommitVersion,
        max_entries: usize,
    ) -> oqueue_core::BoxFuture<'_, oqueue_core::Result<Vec<oqueue_core::MetadataEntry>>> {
        if self.reads.fetch_add(1, std::sync::atomic::Ordering::SeqCst) > 0 {
            return Box::pin(async { Err(oqueue_core::Error::Transient) });
        }
        self.inner.read_from(start, max_entries)
    }

    fn last_version(
        &self,
    ) -> oqueue_core::BoxFuture<'_, oqueue_core::Result<Option<oqueue_core::CommitVersion>>> {
        self.inner.last_version()
    }
}

/// A log already holding one three-record commit.
async fn seeded(clock: &Arc<FakeClock>) -> Arc<dyn MetadataLog> {
    let log: Arc<dyn MetadataLog> = Arc::new(oqueue_core::FakeMetadataLog::new());
    let (seed, seeding, _reader) = Coordinator::open(
        Arc::clone(&log),
        Box::new(FakeMaterializedIndex::new()),
        CoordinatorEpoch::ZERO,
        Arc::clone(clock) as _,
    )
    .await
    .expect("opens");
    let seeding = tokio::spawn(seeding.run());
    seed.commit(object(0), vec![span(3)])
        .await
        .expect("commits");
    drop(seed);
    seeding.await.expect("the loop ends");
    log
}

/// ⚠️ **A failed term reopen leaves the served index as it was** (`M6.17`'s
/// review): readers keep answering from it while the new leader cannot yet
/// read its log, rather than being handed an emptied index.
#[tokio::test(start_paused = true)]
async fn a_failed_term_replay_does_not_empty_the_index_readers_use() {
    let store: Arc<dyn ObjectStore> = Arc::new(FakeObjectStore::new());
    let clock = Arc::new(FakeClock::starting_at(
        Timestamp::from_millis(1_700_000_000_000).expect("a valid time"),
    ));
    let log = seeded(&clock).await;

    let lease = Arc::new(ObjectStoreLease::new(
        Arc::clone(&store),
        "meta/0",
        "b",
        Arc::clone(&clock) as _,
    ));
    let (b, serving, reader) = Coordinator::open(
        Arc::clone(&log),
        Box::new(FakeMaterializedIndex::new()),
        CoordinatorEpoch::ZERO,
        Arc::clone(&clock) as _,
    )
    .await
    .expect("replays at boot");
    let reopen_log = Arc::clone(&log);
    let serving = tokio::spawn(
        serving
            .fenced_by(Arc::clone(&lease))
            .reopening_with(Box::new(move || {
                let view = Arc::new(FirstPageOnly {
                    inner: Arc::clone(&reopen_log),
                    reads: std::sync::atomic::AtomicU32::new(0),
                });
                Box::pin(async move { Ok(view as Arc<dyn MetadataLog>) })
            }))
            .run(),
    );
    assert_eq!(reader.end_offset(&topic(), partition()).get(), 3);

    assert!(lease.acquire().await.expect("takes the lease"));
    assert_eq!(
        b.commit(object(1), vec![span(1)]).await.err(),
        Some(CoordinatorError::Unavailable),
        "the term replay could not read its log"
    );
    assert_eq!(
        reader.end_offset(&topic(), partition()).get(),
        3,
        "and the index readers use is untouched"
    );
    drop(b);
    serving.await.expect("the loop ends");
}
