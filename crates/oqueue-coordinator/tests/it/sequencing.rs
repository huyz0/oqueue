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
use oqueue_coordinator::{
    Coordinator, CoordinatorError, RejectReason, SpanOutcome, UNASSIGNED_OFFSET,
};
use oqueue_core::{
    ByteRange, CommitVersion, CommittedSpan, CoordinatorEpoch, Error, FakeMaterializedIndex,
    FakeMetadataLog, FaultMetadataLog, LogFaults, MaterializedIndex, MetadataEntry, MetadataLog,
    PartitionId, ProducerEpoch, ProducerId, ProducerIdentity,
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
    // zero, which is silent duplication rather than a failure. ⚠️ **`M6` is
    // what lifts this, not `M3.8`** — `CoordinatorError::ReplayRequired`'s own
    // doc says so: `M3.8`'s replay is the *index*'s and leaves the allocator's
    // offset lines untouched, which is the duplication this refuses to allow.
    let index = FakeMaterializedIndex::new();
    index
        .apply(&[MetadataEntry::new(
            CommitVersion::ZERO,
            oqueue_core::MetadataRecord::EpochChanged {
                epoch: CoordinatorEpoch::ZERO,
            },
        )])
        .expect("a fold this test can look for afterwards");

    let rejected = Coordinator::open(log, Box::new(index), CoordinatorEpoch::ZERO)
        .await
        .expect_err("a non-empty log refuses");

    assert_eq!(
        rejected.error(),
        &CoordinatorError::ReplayRequired { last_version: 0 }
    );
    // ⚠️ **And the index comes back**, unmodified (`M3.34`). `open` takes it by
    // value so the caller keeps no writable handle, which also means a plain
    // `Err` would destroy it. ⚠️ **What the caller does with it is its own
    // business**: feeding it back into `open` is not the remedy — the clear on
    // the success path would wipe anything replayed into it, and `M6` owns
    // that interaction. What this asserts is that the caller still *has* it.
    let returned = rejected.into_index();
    assert_eq!(
        returned.applied_upto(),
        Some(CommitVersion::ZERO),
        "the refusal cleared an index it was only supposed to hand back"
    );
}

/// ⚠️ **`M11.7`, `ADR-0031` point 5's `CoordinatorEpoch` half.** A second
/// coordinator is what a zombie *coordinator* scenario needs to exist at
/// all — and `Coordinator::open` already refuses one over any non-empty
/// log, unconditionally, before it could ever answer a sequence check any
/// more than it could assign an offset. `M6` is what elects a genuine
/// replacement (`coordinator_epoch.rs`'s own doc); until then, a log
/// carrying producer-sequence history is refused open exactly the same as
/// one carrying only offsets — the same `ReplayRequired` this file's own
/// `opening_over_a_log_that_already_holds_entries_refuses` already proves,
/// checked here against a log a producer-bearing commit actually built.
#[tokio::test]
async fn a_log_carrying_producer_sequence_history_refuses_a_second_coordinator_too() {
    let log = Arc::new(FakeMetadataLog::new());
    let (coordinator, driver) = start(log.clone()).await;
    coordinator
        .commit(object(0), vec![producer_span(1, 0, 2)])
        .await
        .expect("the commit lands");
    drop(coordinator);
    driver.await.expect("the loop ends");

    let rejected = Coordinator::open(
        log,
        Box::new(FakeMaterializedIndex::new()),
        CoordinatorEpoch::ZERO,
    )
    .await
    .expect_err("a non-empty log refuses a second coordinator, sequence history or not");

    assert_eq!(
        rejected.error(),
        &CoordinatorError::ReplayRequired { last_version: 0 }
    );
}

/// ⚠️ **A transient log read is the case this matters for.** `ReplayRequired`
/// is `Never`-retryable, so a caller holding it changes course; a `Journal`
/// error wrapping [`Error::Transient`] is the one where the right move is to
/// *try again* — and a caller cannot, if the attempt consumed its index.
///
/// ⚠️ **For `MemoryIndex` that costs an allocation and for doc 10 #12's engine
/// it costs an open database**, which is why the shape is worth a type: the
/// seam is the same either way, and the engine behind it is what `ADR-0020`
/// point 5 left undecided.
#[tokio::test]
async fn a_transient_journal_failure_hands_the_index_back_to_be_retried_with() {
    let log = Arc::new(FaultMetadataLog::new(FakeMetadataLog::new()));
    log.set_faults(LogFaults {
        refuse_read: true,
        ..LogFaults::default()
    });

    // ⚠️ **A fold nobody else could have made**, so "the index came back" is a
    // claim about *this* index rather than about some index: a refusal that
    // handed back a freshly built one would satisfy every other assertion here
    // while costing the caller exactly what this row is about.
    let index = FakeMaterializedIndex::new();
    index
        .apply(&[MetadataEntry::new(
            CommitVersion::ZERO,
            oqueue_core::MetadataRecord::EpochChanged {
                epoch: CoordinatorEpoch::ZERO,
            },
        )])
        .expect("a fold this test can look for afterwards");

    let rejected = Coordinator::open(
        Arc::clone(&log) as Arc<dyn MetadataLog>,
        Box::new(index),
        CoordinatorEpoch::ZERO,
    )
    .await
    .expect_err("a log that will not answer cannot be opened over");

    let (error, index) = rejected.into_parts();
    assert_eq!(error, CoordinatorError::Journal(Error::Transient));
    assert_eq!(
        index.applied_upto(),
        Some(CommitVersion::ZERO),
        "the index handed back is not the one that went in"
    );

    // The same index, into the retry that now succeeds.
    log.set_faults(LogFaults::default());
    let (coordinator, driver, _reader) =
        Coordinator::open(log as Arc<dyn MetadataLog>, index, CoordinatorEpoch::ZERO)
            .await
            .expect("the retry opens over the same index");
    let driver = tokio::spawn(driver.run());
    coordinator
        .commit(object(0), vec![span(2)])
        .await
        .expect("and it works");
    drop(coordinator);
    driver.await.expect("the loop ends");
}

/// A span carrying a producer identity, for `M11.3`'s interim refusal.
fn producer_span(id: i64, sequence: i32, records: u32) -> CommittedSpan {
    producer_span_at(id, ProducerEpoch::ZERO, sequence, records)
}

/// The same, at a chosen epoch — `M11.7`'s zombie/bump tests need one other
/// than zero.
fn producer_span_at(id: i64, epoch: ProducerEpoch, sequence: i32, records: u32) -> CommittedSpan {
    CommittedSpan::new(
        topic(),
        partition(),
        records,
        ByteRange::Full,
        Some(ProducerIdentity::new(
            ProducerId::new(id).expect("a valid producer id"),
            epoch,
            sequence,
        )),
    )
}

/// ⚠️ **`ADR-0031` point 2's transparent-success case, `M11.6`.** An exact
/// replay resolves without ever reaching `stage` or the journal, and answers
/// as an ordinary success naming the recorded offset — not the interim
/// refusal `M11.3` shipped until a caller could report a per-span outcome.
#[tokio::test]
async fn an_exact_replay_answers_success_with_the_recorded_offset() {
    let log = Arc::new(FakeMetadataLog::new());
    let (coordinator, driver) = start(log.clone()).await;

    let first = coordinator
        .commit(object(0), vec![producer_span(1, 0, 2)])
        .await
        .expect("the first sequence commits normally");
    assert_eq!(first.base_offset(&topic(), partition()), 0);

    let replay = coordinator
        .commit(object(1), vec![producer_span(1, 0, 2)])
        .await
        .expect("a replay is an ordinary success, not a refusal");
    assert_eq!(
        replay.outcomes(),
        first.outcomes(),
        "the same span, answered the same way, both times"
    );
    assert_eq!(replay.base_offset(&topic(), partition()), 0);
    assert_eq!(
        replay.version(),
        first.version(),
        "nothing new was committed, so the version does not move"
    );
    assert_eq!(
        log.read_from(CommitVersion::ZERO, 16)
            .await
            .expect("the log reads back")
            .len(),
        1,
        "the replay never reached the journal"
    );

    drop(coordinator);
    driver
        .await
        .expect("the loop ends when its last handle drops");
}

/// A genuine sequence gap is named per span, without failing what would
/// otherwise be an entirely empty commit.
#[tokio::test]
async fn a_rejected_span_is_named_without_reaching_the_journal() {
    let log = Arc::new(FakeMetadataLog::new());
    let (coordinator, driver) = start(log.clone()).await;

    let ack = coordinator
        .commit(object(0), vec![producer_span(1, 9, 1)])
        .await
        .expect("a rejection is an ordinary answer, not a refused commit");
    assert_eq!(
        ack.outcomes(),
        &[SpanOutcome::Rejected(RejectReason::OutOfOrder)]
    );
    assert!(
        ack.assignments().is_empty(),
        "a rejected span gets no offset"
    );
    assert_eq!(
        log.read_from(CommitVersion::ZERO, 16)
            .await
            .expect("the log reads back")
            .len(),
        0,
        "an all-rejected commit never reaches the journal"
    );

    drop(coordinator);
    driver
        .await
        .expect("the loop ends when its last handle drops");
}

/// ⚠️ **The property `M11.3`'s own admission-layer test already proved, now
/// checked through the public `Coordinator::commit` this milestone exists to
/// wire it to.** One producer's rejected gap must not cost a different
/// producer's legitimate span, in the same bundled commit, its real offset.
#[tokio::test]
async fn one_producers_rejection_does_not_cost_anothers_real_offset() {
    let log = Arc::new(FakeMetadataLog::new());
    let (coordinator, driver) = start(log).await;

    let ack = coordinator
        .commit(
            object(0),
            vec![producer_span(1, 9, 1), producer_span(2, 0, 3)],
        )
        .await
        .expect("one rejection does not fail the whole commit");
    assert_eq!(
        ack.outcomes()[0],
        SpanOutcome::Rejected(RejectReason::OutOfOrder),
        "producer 1's gap"
    );
    let SpanOutcome::Assigned(assignment) = &ack.outcomes()[1] else {
        panic!("producer 2's legitimate first span must be admitted");
    };
    assert_eq!(assignment.base_offset().get(), 0);
    assert_eq!(ack.assignments(), std::slice::from_ref(assignment));

    drop(coordinator);
    driver
        .await
        .expect("the loop ends when its last handle drops");
}

/// ⚠️ **`M11.7`, `ADR-0031` point 5: a zombie is named without reaching the
/// journal**, on `a_rejected_span_is_named_without_reaching_the_journal`'s
/// own precedent — a stale epoch is excluded by `Allocator::admit` exactly
/// as a sequence gap is, never staged, never journaled.
#[tokio::test]
async fn a_zombie_epoch_is_named_without_reaching_the_journal() {
    let log = Arc::new(FakeMetadataLog::new());
    let (coordinator, driver) = start(log.clone()).await;

    // Some other incarnation of producer 1 already bumped past epoch 0.
    coordinator
        .commit(
            object(0),
            vec![producer_span_at(
                1,
                ProducerEpoch::new(1).expect("valid"),
                0,
                2,
            )],
        )
        .await
        .expect("the bump's own genuine first send commits normally");

    // This sender is still at epoch 0 — a zombie.
    let ack = coordinator
        .commit(object(1), vec![producer_span(1, 1, 1)])
        .await
        .expect("a zombie is an ordinary answer, not a refused commit");
    assert_eq!(
        ack.outcomes(),
        &[SpanOutcome::Rejected(RejectReason::StaleEpoch)]
    );
    assert_eq!(
        log.read_from(CommitVersion::ZERO, 16)
            .await
            .expect("the log reads back")
            .len(),
        1,
        "the zombie's commit never reached the journal"
    );

    drop(coordinator);
    driver
        .await
        .expect("the loop ends when its last handle drops");
}
