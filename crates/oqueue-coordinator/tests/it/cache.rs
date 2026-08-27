//! The cache the coordinator owns: who may write it, what happens when it is
//! dropped, and what a reader is given instead of a handle.
//!
//! ⚠️ **Split from `tail.rs` at the 500-line limit, along the concept.** That
//! file is about the *park* — what a reader waits on and when it is woken.
//! This one is about the index itself: `ADR-0024`'s read-only view, the one
//! door that drops the cache, and the rebuild that a drop makes necessary.

#![allow(clippy::expect_used)]

use crate::support::{object, offset, partition, span, start_indexed, topic};
use oqueue_coordinator::{Coordinator, CoordinatorError, REBUILD_PAGE_ENTRIES};
use oqueue_core::{
    CommitVersion, CoordinatorEpoch, FakeMaterializedIndex, FakeMetadataLog, FaultMetadataLog,
    LogFaults, MaterializedIndex, MetadataEntry, MetadataLog, MetadataRecord, Offset,
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
    let log = Arc::new(FaultMetadataLog::new(FakeMetadataLog::new()));
    let (coordinator, driver, index) = Coordinator::open(
        Arc::clone(&log) as Arc<dyn MetadataLog>,
        Box::new(FakeMaterializedIndex::new()),
        CoordinatorEpoch::ZERO,
    )
    .await
    .expect("a fresh log opens");
    let driver = tokio::spawn(driver.run());

    log.set_faults(LogFaults {
        refuse_read: true,
        ..LogFaults::default()
    });
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
    let log = Arc::new(FaultMetadataLog::new(FakeMetadataLog::new()));
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
    log.set_faults(LogFaults {
        refuse_read: true,
        ..LogFaults::default()
    });
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
/// held by the queue, and this said it was "asserted by nothing" until
/// `M3.37`. ⚠️ **`M3.32` asserted it**: see
/// [`a_drop_cannot_land_between_a_commits_journal_and_its_fold`] below, which
/// `FaultMetadataLog`'s `hold_append` makes possible by holding a commit open
/// inside `append` and racing a drop against it.
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

/// ⚠️ **`ADR-0024`'s serialization property, raced rather than argued.** A
/// commit is assign → journal → fold, and `ADR-0020` makes that whole sequence
/// one serialization point: nothing else may act on the index between the
/// append and the fold. `drop_cache` is the one operation that would corrupt
/// the index if it did — a cache dropped *after* the journal and *before* the
/// fold loses a commit the client has been told is durable, and the log still
/// holds it, so the loss is silent until somebody replays.
///
/// ⚠️ **Nothing could assert this until `M3.32`.** The old fake's `append`
/// either succeeded or refused, both after the fact; what this needs is the
/// append held *open*, so the drop is issued while the commit is provably
/// inside the window. `FaultMetadataLog::parked` is what makes it a fact
/// rather than a race the test might lose.
///
/// ⚠️ **An implementation clearing the index straight from a handle would pass
/// the suite unchanged** — which is `ADR-0024`'s own stated worry, and the
/// reason the handle is a `Box`. This is the assertion behind that argument.
#[tokio::test]
async fn a_drop_cannot_land_between_a_commits_journal_and_its_fold() {
    let log = Arc::new(FaultMetadataLog::new(FakeMetadataLog::new()));
    let (coordinator, driver, index) = Coordinator::open(
        Arc::clone(&log) as Arc<dyn MetadataLog>,
        Box::new(FakeMaterializedIndex::new()),
        CoordinatorEpoch::ZERO,
    )
    .await
    .expect("a fresh log opens");
    let driver = tokio::spawn(driver.run());
    let coordinator = Arc::new(coordinator);

    // ⚠️ **One commit first, and it is what makes the window observable.** The
    // assertion inside the window is about the *index*, so the index has to
    // hold something: against an empty one, a `clear` landing mid-window looks
    // exactly like the empty index that was already there.
    coordinator
        .commit(object(9), vec![span(3)])
        .await
        .expect("the seed commit lands");
    assert_eq!(index.end_offset(&topic(), partition()), offset(3));

    // A commit that will park inside its journal append, and a drop issued
    // while it is provably in there.
    log.set_faults(LogFaults {
        hold_append: true,
        ..LogFaults::default()
    });
    let committing = spawn_commit(&coordinator, 0, 3);
    parked(&log).await;
    let dropping = tokio::spawn({
        let coordinator = Arc::clone(&coordinator);
        async move { coordinator.drop_cache().await }
    });
    settle().await;

    // ⚠️ **The index, not the reply.** A drop that has not *returned* proves
    // only that its reply is still queued — and a handle that cleared the
    // index and then queued anyway would satisfy that while doing exactly what
    // `ADR-0024` forbids. What must be true is that nothing has touched the
    // index while the loop is inside the commit.
    assert_eq!(
        index.end_offset(&topic(), partition()),
        offset(3),
        "the index was cleared while a commit was inside its journal step"
    );
    assert!(
        !dropping.is_finished(),
        "a drop was served while a commit was inside its journal step"
    );

    released_and_recovered(&log, committing, dropping, &coordinator, &index).await;

    drop(coordinator);
    driver.await.expect("the loop ends");
}

/// A commit, in a task, so a test can watch what happens while it runs.
fn spawn_commit(
    coordinator: &Arc<Coordinator>,
    key: usize,
    records: u32,
) -> tokio::task::JoinHandle<Result<oqueue_coordinator::CommitAck, CoordinatorError>> {
    let coordinator = Arc::clone(coordinator);
    tokio::spawn(async move { coordinator.commit(object(key), vec![span(records)]).await })
}

/// The half after the window: everything finishes, and nothing was lost.
///
/// ⚠️ **Its own function for the 50-line limit, and the seam is where the
/// window ends.** What is above it is the claim — nothing touches the index
/// while a commit is inside its journal step — and what is here is that the
/// claim did not cost anything: both calls complete, and the commit that raced
/// the drop is still recoverable from the log.
async fn released_and_recovered(
    log: &FaultMetadataLog<FakeMetadataLog>,
    committing: tokio::task::JoinHandle<Result<oqueue_coordinator::CommitAck, CoordinatorError>>,
    dropping: tokio::task::JoinHandle<Result<(), CoordinatorError>>,
    coordinator: &Arc<Coordinator>,
    index: &oqueue_core::IndexReader,
) {
    log.release();
    committing
        .await
        .expect("the task joins")
        .expect("the commit lands");
    dropping
        .await
        .expect("the task joins")
        .expect("the drop is served once the commit is done");

    // ⚠️ **The commit survived the drop**, which is the half that says the
    // ordering was the right way round rather than merely serialized. The
    // *log* is where it survived: the append completed before the drop was
    // served, so the entry is durable whatever the cache does next.
    assert_eq!(log.inner().len(), 2, "the journal step completed first");
    // ⚠️ **The index is empty here, and that is not the defect** — a dropped
    // cache refills on the next commit rather than at the drop, which is the
    // design and is why `roadmap.md` carries "a dropped cache has no refill
    // trigger but a commit" to `M5`. What would be a defect is the entries
    // being *unrecoverable*, so the next commit is what proves they are not.
    assert_eq!(index.end_offset(&topic(), partition()), offset(0));

    coordinator
        .commit(object(1), vec![span(2)])
        .await
        .expect("the commit lands");
    assert_eq!(
        index.end_offset(&topic(), partition()),
        offset(8),
        "the rebuild replayed the commit that raced the drop, and lost nothing"
    );
}

/// Waits until an `append` is parked inside the log, or gives up.
///
/// ⚠️ **Bounded, for the reason `drive_pinned` is** (`oqueue-core`): an
/// unbounded `while log.parked() == 0` turns any regression that stops the
/// commit reaching the log into an indefinite hang of the whole test binary
/// rather than a failure — and `cargo-mutants` replacing `parked` with `0` is
/// exactly such a regression, reported as a timeout rather than a kill.
async fn parked(log: &FaultMetadataLog<FakeMetadataLog>) {
    for _ in 0..1_000 {
        if log.parked() > 0 {
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("no append ever reached the log");
}

/// Runs every other ready task to a standstill.
///
/// ⚠️ **Bounded yields, not a sleep** (`tdd.md`'s no-flake rule): what is being
/// asserted is that something has *not* happened, and a `sleep` would make the
/// assertion a bet on scheduling. Sixty-four turns is far more than the two the
/// queue needs to serve a request that was going to be served.
async fn settle() {
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }
}
