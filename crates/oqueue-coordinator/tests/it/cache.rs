//! The cache the coordinator owns: who may write it, what happens when it is
//! dropped, and what a reader is given instead of a handle.
//!
//! ⚠️ **Split from `tail.rs` at the 500-line limit, along the concept.** That
//! file is about the *park* — what a reader waits on and when it is woken.
//! This one is about the index itself: `ADR-0024`'s read-only view, the one
//! door that drops the cache, and the rebuild that a drop makes necessary.

#![allow(clippy::expect_used)]

use crate::support::{RefusableLog, object, offset, partition, span, start_indexed, topic};
use oqueue_coordinator::{Coordinator, CoordinatorError, REBUILD_PAGE_ENTRIES};
use oqueue_core::{
    CommitVersion, CoordinatorEpoch, FakeMaterializedIndex, FakeMetadataLog, MaterializedIndex,
    MetadataEntry, MetadataLog, MetadataRecord, Offset,
};
use std::sync::Arc;

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

    coordinator.drop_cache().await.expect("the loop is serving");
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
    let (coordinator, driver, index) = Coordinator::open(
        Arc::clone(&log) as Arc<dyn MetadataLog>,
        Box::new(FakeMaterializedIndex::new()),
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

/// ⚠️ **A rebuild that fails leaves the index empty *and the watch silent*.**
/// This is the one case where what the index holds and the version just
/// committed differ, so it is the only case that can tell a watch reading back
/// from the index from one remembering the entry's own version — and the
/// difference is a reader woken instantly onto an index that cannot answer it.
#[tokio::test(start_paused = true)]
async fn a_failed_rebuild_leaves_the_index_empty_and_says_so() {
    let log = Arc::new(RefusableLog::new());
    let (coordinator, driver, index) = Coordinator::open(
        Arc::clone(&log) as Arc<dyn MetadataLog>,
        Box::new(FakeMaterializedIndex::new()),
        CoordinatorEpoch::ZERO,
    )
    .await
    .expect("a fresh log opens");
    let driver = tokio::spawn(driver.run());
    let watch = coordinator.watch();

    coordinator
        .commit(object(0), vec![span(3)])
        .await
        .expect("the commit lands");
    assert_eq!(watch.applied(), Some(CommitVersion::ZERO));

    // The cache is dropped and the log will not give it back.
    coordinator.drop_cache().await.expect("the loop is serving");
    log.refuse_reads(true);
    coordinator
        .commit(object(1), vec![span(3)])
        .await
        .expect("the position is durable whatever happens to the cache");

    assert_eq!(index.applied_upto(), None, "empty rather than partial");
    assert_eq!(
        watch.applied(),
        None,
        "and the watch says so — a version published here would wake every \
         reader at or below it onto an index holding nothing"
    );
    assert_eq!(index.end_offset(&topic(), partition()), Offset::ZERO);

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

    coordinator.drop_cache().await.expect("the loop is serving");
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
    let (coordinator, driver, index) =
        Coordinator::open(log, Box::new(stale), CoordinatorEpoch::ZERO)
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

/// ⚠️ **The read-only view is the whole of what a reader gets** (`ADR-0024`).
/// Everything a fetch needs is on it; `apply` and `clear` are not, so the
/// sole-writer rule is a thing a caller cannot break rather than one it must
/// remember. What this test can assert is that the reads work and agree with
/// the coordinator; that the writes are *absent* is asserted by the compiler,
/// which is the point of moving the rule there. ⚠️ Its `Debug` redaction is
/// pinned in `oqueue-core`, where the type lives, rather than duplicated here.
#[tokio::test(start_paused = true)]
async fn the_view_answers_every_read_a_fetch_needs() {
    let (coordinator, index, driver) = start_indexed().await;

    let ack = coordinator
        .commit(object(0), vec![span(3)])
        .await
        .expect("the commit lands");

    assert_eq!(index.applied_upto(), Some(ack.version()));
    assert_eq!(index.end_offset(&topic(), partition()), offset(3));
    let page = index
        .find_batches(&topic(), partition(), Offset::ZERO, u64::MAX)
        .expect("a page");
    assert_eq!(page.len(), 1);
    assert_eq!(page[0].reference().base_offset(), Offset::ZERO);

    drop(coordinator);
    driver.await.expect("the loop ends");
}

/// A dropped cache is refilled from the log, not re-based at zero — which is
/// what makes dropping it safe to offer at all.
///
/// ⚠️ **What this does *not* pin is the serialization**, and saying so beats
/// implying otherwise: `drop_cache` is awaited to completion with no commit in
/// flight, so an implementation that cleared the index straight from the handle
/// would pass this unchanged. The property — that a clear cannot land between
/// the loop's own "is this current" check and its fold, where `apply` accepts
/// it because an emptied index has no `applied_upto` to refuse against — is
/// held by the queue and asserted by nothing. `M3.18` owns the fault decorator
/// that would let a test hold a commit open inside `append` and race a drop
/// against it.
#[tokio::test(start_paused = true)]
async fn dropping_the_cache_goes_through_the_queue_and_the_log_refills_it() {
    let (coordinator, index, driver) = start_indexed().await;
    for n in 0..3 {
        coordinator
            .commit(object(n), vec![span(2)])
            .await
            .expect("the commit lands");
    }
    assert_eq!(index.end_offset(&topic(), partition()), offset(6));

    coordinator.drop_cache().await.expect("the loop is serving");
    assert_eq!(index.applied_upto(), None, "the cache is gone");
    assert_eq!(
        coordinator.watch().applied(),
        None,
        "and the watch says so rather than claiming a version nothing holds"
    );

    coordinator
        .commit(object(3), vec![span(2)])
        .await
        .expect("the commit lands");
    assert_eq!(
        index.end_offset(&topic(), partition()),
        offset(8),
        "rebuilt from the log, not re-based at zero"
    );

    drop(coordinator);
    driver.await.expect("the loop ends");
}

/// A drop asked of a coordinator that has stopped is refused, not ignored.
#[tokio::test(start_paused = true)]
async fn dropping_the_cache_on_a_stopped_coordinator_is_refused() {
    let (coordinator, driver, _view) = Coordinator::open(
        Arc::new(FakeMetadataLog::new()),
        Box::new(FakeMaterializedIndex::new()),
        CoordinatorEpoch::ZERO,
    )
    .await
    .expect("a fresh log opens");
    drop(driver);

    assert_eq!(
        coordinator.drop_cache().await.err(),
        Some(CoordinatorError::Unavailable)
    );
}
