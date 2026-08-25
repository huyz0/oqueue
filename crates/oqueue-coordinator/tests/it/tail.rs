//! Push for the tail: what a parked reader waits on, and what a follower gets.
//!
//! `M3.9`, and doc 12 §4.4's already-settled split — push the tail, pull the
//! history. What is tested here is the property `M3.md` task 17 is really
//! about: a fetch racing a produce is woken **by the commit**, not by a poll
//! interval, and it is woken only once the index can actually answer it.

// Every `expect` is on a value the suite itself built, or on a step whose
// failure *is* the test failing.
#![allow(clippy::expect_used)]

use crate::support::{RefusableLog, object, offset, parked, partition, span, start_indexed, topic};
use oqueue_coordinator::{Coordinator, REBUILD_PAGE_ENTRIES};
use oqueue_core::{
    CommitVersion, CoordinatorEpoch, FakeMaterializedIndex, FakeMetadataLog, MaterializedIndex,
    MetadataEntry, MetadataLog, MetadataRecord, Offset,
};
use std::sync::Arc;

/// ⚠️ The read-your-writes case (hazard H2) with **no wait at all**: by the
/// time a producer holds its ack, the coordinator's index has already folded
/// the record. A design that folded after acking would leave a window in which
/// the position is durable and unqueryable, which is the same silent wrongness
/// from the other direction.
#[tokio::test]
async fn an_ack_implies_the_index_can_already_answer_for_it() {
    let (coordinator, index, driver) = start_indexed().await;

    let ack = coordinator
        .commit(object(0), vec![span(4)])
        .await
        .expect("the commit lands");

    assert_eq!(index.applied_upto(), Some(ack.version()));
    assert_eq!(
        index.end_offset(&topic(), partition()),
        Offset::new(4).expect("a valid offset"),
        "the high watermark moved before the producer was told its offset"
    );

    drop(coordinator);
    driver.await.expect("the loop ends");
}

/// FR-12's zero-GET case at the seam that decides it: a reader already at the
/// high watermark is told to read nothing, so there is nothing to GET.
#[tokio::test]
async fn a_reader_at_the_high_watermark_is_given_no_object_to_read() {
    let (coordinator, index, driver) = start_indexed().await;
    coordinator
        .commit(object(0), vec![span(3)])
        .await
        .expect("the commit lands");

    let hwm = index.end_offset(&topic(), partition());
    assert!(
        index
            .find_batches(&topic(), partition(), hwm, u64::MAX)
            .expect("a page")
            .is_empty()
    );

    drop(coordinator);
    driver.await.expect("the loop ends");
}

/// ⚠️ **The park, and the whole of `M3.md` task 17.** The waiter is parked
/// before the produce exists; nothing polls; the commit is what wakes it. A
/// poll-interval design would pass this test only by waiting — which is why
/// there is no sleep anywhere in it, and why `tokio::join!` rather than a
/// timeout is what proves the wakeup happened.
#[tokio::test(start_paused = true)]
async fn a_reader_parked_ahead_of_the_log_is_woken_by_the_commit() {
    let (coordinator, index, driver) = start_indexed().await;
    let mut watch = coordinator.watch();
    assert_eq!(watch.applied(), None, "nothing has been folded yet");

    let producer = coordinator.clone();
    let ((), parked) = tokio::join!(
        async move {
            producer
                .commit(object(0), vec![span(2)])
                .await
                .expect("the commit lands");
        },
        async {
            // Version 0 does not exist yet when this future is created.
            assert!(
                parked(watch.wait_for(CommitVersion::ZERO)).await,
                "the wait ended because the version arrived, not because the \
                 coordinator stopped"
            );
            index.end_offset(&topic(), partition())
        }
    );

    assert_eq!(
        parked,
        Offset::new(2).expect("a valid offset"),
        "woken only once the index could answer, never merely once the log \
         held the entry"
    );

    drop(coordinator);
    driver.await.expect("the loop ends");
}

/// A version already folded resolves without waiting for a further change —
/// the level-triggered half, and what stops a fetch for an old offset from
/// parking until its deadline.
#[tokio::test(start_paused = true)]
async fn a_reader_parked_behind_the_index_is_not_made_to_wait() {
    let (coordinator, _index, driver) = start_indexed().await;
    coordinator
        .commit(object(0), vec![span(1)])
        .await
        .expect("the commit lands");
    coordinator
        .commit(object(1), vec![span(1)])
        .await
        .expect("the commit lands");

    let mut watch = coordinator.watch();
    // No further commit will ever happen, so this resolving at all is the
    // assertion.
    assert!(parked(watch.wait_for(CommitVersion::ZERO)).await);
    assert_eq!(watch.applied(), Some(CommitVersion::new(1)));

    drop(coordinator);
    driver.await.expect("the loop ends");
}

/// ⚠️ A wait that ends because the coordinator stopped says so, rather than
/// letting a caller read "I waited" as "it arrived" — which for an
/// `AtLeast(v)` read is hazard H2 with extra steps.
#[tokio::test(start_paused = true)]
async fn a_wait_the_coordinator_outlives_reports_that_it_did_not_arrive() {
    let (coordinator, _index, driver) = start_indexed().await;
    let mut watch = coordinator.watch();
    drop(coordinator);
    driver.await.expect("the loop ends");

    assert!(!parked(watch.wait_for(CommitVersion::new(7))).await);
    assert_eq!(watch.applied(), None, "and nothing was ever folded");
}

/// ⚠️ **The seam blesses dropping this index at any moment** — under memory
/// pressure, on a restart, when `M3.11`'s quota trips — so the coordinator
/// must not assume it owns one that is never dropped. Folding one entry onto a
/// forgotten partition would base it at `Offset::ZERO` while the log and the
/// producer's ack place it far higher, and an index that is *wrong* is worse
/// than one that is empty.
#[tokio::test(start_paused = true)]
async fn an_index_dropped_behind_the_coordinator_s_back_is_rebuilt_not_re_based() {
    let (coordinator, index, driver) = start_indexed().await;
    for n in 0..3 {
        coordinator
            .commit(object(n), vec![span(2)])
            .await
            .expect("the commit lands");
    }
    assert_eq!(index.end_offset(&topic(), partition()), offset(6));

    index.clear();
    assert_eq!(index.applied_upto(), None, "the cache is gone");

    coordinator
        .commit(object(3), vec![span(2)])
        .await
        .expect("the commit lands");

    assert_eq!(
        index.end_offset(&topic(), partition()),
        offset(8),
        "rebuilt from the log, not re-based at zero"
    );
    let page = index
        .find_batches(&topic(), partition(), Offset::ZERO, u64::MAX)
        .expect("a page");
    assert_eq!(
        page.iter()
            .map(|batch| batch.reference().base_offset().get())
            .collect::<Vec<_>>(),
        vec![0, 2, 4, 6],
        "every object is back where the log says it is"
    );

    drop(coordinator);
    driver.await.expect("the loop ends");
}

/// ⚠️ **The watch never claims more than the index holds.** It is
/// level-triggered, so a version published above the index would wake every
/// reader at or below it *immediately*, onto a cache that cannot answer — H2
/// arriving through the mechanism built to prevent it.
#[tokio::test(start_paused = true)]
async fn the_watch_never_reports_a_version_the_index_does_not_hold() {
    let (coordinator, index, driver) = start_indexed().await;
    let watch = coordinator.watch();

    for n in 0..2 {
        coordinator
            .commit(object(n), vec![span(1)])
            .await
            .expect("the commit lands");
        assert_eq!(watch.applied(), index.applied_upto());
    }

    index.clear();
    coordinator
        .commit(object(2), vec![span(1)])
        .await
        .expect("the commit lands");
    assert_eq!(
        watch.applied(),
        index.applied_upto(),
        "after a rebuild as much as before one"
    );

    drop(coordinator);
    driver.await.expect("the loop ends");
}

/// ⚠️ **The ordinary commit reads the log not at all**, and a coordinator that
/// rebuilt on every commit would be indistinguishable by its *results* — which
/// is why this proves it by making a read fail rather than by counting one. A
/// rebuild per commit is a full replay of the log on the produce-ack path, and
/// it is the shape a wrong "is my index current" answer takes.
#[tokio::test(start_paused = true)]
async fn an_ordinary_commit_never_reads_the_log_back() {
    let log = Arc::new(RefusableLog::new());
    let index: Arc<dyn MaterializedIndex> = Arc::new(FakeMaterializedIndex::new());
    let (coordinator, driver) = Coordinator::open(
        Arc::clone(&log) as Arc<dyn MetadataLog>,
        Arc::clone(&index),
        CoordinatorEpoch::ZERO,
    )
    .await
    .expect("a fresh log opens");
    let driver = tokio::spawn(driver.run());

    log.refuse_reads(true);
    for n in 0..3 {
        coordinator
            .commit(object(n), vec![span(2)])
            .await
            .expect("the commit lands");
    }
    assert_eq!(
        index.end_offset(&topic(), partition()),
        offset(6),
        "the fold happened, so no read was needed and none was attempted"
    );

    drop(coordinator);
    driver.await.expect("the loop ends");
}

/// A rebuild larger than one page reads every page, not just the first.
#[tokio::test(start_paused = true)]
async fn a_rebuild_spanning_more_than_one_page_reads_all_of_them() {
    let (coordinator, index, driver) = start_indexed().await;
    let total = REBUILD_PAGE_ENTRIES + 5;
    for n in 0..total {
        coordinator
            .commit(object(n), vec![span(1)])
            .await
            .expect("the commit lands");
    }

    index.clear();
    coordinator
        .commit(object(total), vec![span(1)])
        .await
        .expect("the commit lands");

    assert_eq!(
        index.end_offset(&topic(), partition()),
        offset(i64::try_from(total + 1).expect("a count that fits")),
        "a rebuild that stopped at the first full page would be short by the \
         rest of the log"
    );

    drop(coordinator);
    driver.await.expect("the loop ends");
}

/// ⚠️ An index arriving with a foreign version folded into it is **cleared**,
/// never trusted. Seeding the watch from it would answer `true` at once for
/// every `AtLeast(v)` below that version and serve a reader offsets from a
/// different line believing they were fresh — H2 through the mechanism built
/// to prevent it.
#[tokio::test(start_paused = true)]
async fn an_index_carrying_another_line_s_version_is_cleared_at_open() {
    let log: Arc<dyn MetadataLog> = Arc::new(FakeMetadataLog::new());
    let stale = FakeMaterializedIndex::new();
    stale
        .apply(&[MetadataEntry::new(
            CommitVersion::new(500),
            MetadataRecord::BatchCommitted {
                object: object(99),
                spans: vec![span(7)],
            },
        )])
        .expect("some other line folded into it");
    let index: Arc<dyn MaterializedIndex> = Arc::new(stale);

    let (coordinator, driver) = Coordinator::open(log, Arc::clone(&index), CoordinatorEpoch::ZERO)
        .await
        .expect("a fresh log opens");
    let driver = tokio::spawn(driver.run());

    assert_eq!(index.applied_upto(), None, "the foreign line is gone");
    assert_eq!(
        coordinator.watch().applied(),
        None,
        "and the watch never claimed it"
    );
    assert_eq!(index.end_offset(&topic(), partition()), Offset::ZERO);

    drop(coordinator);
    driver.await.expect("the loop ends");
}
