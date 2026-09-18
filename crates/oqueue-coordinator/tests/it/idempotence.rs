//! Idempotent produce: what a retry gets, and what a zombie gets.
//!
//! ⚠️ **Split from `sequencing.rs` by concept** (`code-structure.md` rule 18,
//! and the 500-line limit `M5.49` reached). That file is about offsets — a
//! position journaled before it is acknowledged, a refused journal consuming
//! neither a version nor an offset. This one is about `ADR-0031`: a producer's
//! sequence, what a replay answers, and what compaction does not take away
//! (`ADR-0038`).

// Every `expect` below is on a value the test itself constructed, or on a step
// whose failure *is* the test failing. Same allowance, same reason, as
// `oqueue-core`'s own suites.
#![allow(clippy::expect_used)]

use crate::support::{object, partition, span, start, topic};
use oqueue_coordinator::{Coordinator, CoordinatorError, RejectReason, SpanOutcome};
use oqueue_core::{
    ByteRange, CommitVersion, CommittedSpan, CoordinatorEpoch, FakeMaterializedIndex,
    FakeMetadataLog, MetadataLog, ProducerEpoch, ProducerId, ProducerIdentity,
};
use std::sync::Arc;

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

/// ⚠️ **A retry still dedupes across a commit that rewrote its records**
/// (`ADR-0038`, `M5.49`). The merged object carries no producer identity —
/// the footer has none by design (`M11.5`) and `take_regions` writes
/// `producer: None` — and this asserts that costs the guarantee nothing,
/// because identity lives in the log and in the allocator's own state rather
/// than in the object.
///
/// ⚠️ **The middle commit stands in for `M5.13`'s swap, which does not
/// exist**: it is an ordinary commit of a span with no producer, which is the
/// shape a merged span has. What it cannot stand in for is the swap's removal
/// of the input refs from the index, and `ADR-0038` obligation 1 is that the
/// swap is an append that leaves the original events in the log — so what this
/// test pins is the property, and that obligation is `M5.13`'s to keep.
#[tokio::test]
async fn a_retry_after_a_span_with_no_producer_is_still_deduplicated() {
    let log = Arc::new(FakeMetadataLog::new());
    let (coordinator, driver) = start(log.clone()).await;

    let first = coordinator
        .commit(object(0), vec![producer_span(1, 0, 2)])
        .await
        .expect("the first sequence commits normally");
    assert_eq!(first.base_offset(&topic(), partition()), 0);

    coordinator
        .commit(object(1), vec![span(2)])
        .await
        .expect("a span carrying no producer identity commits");

    let replay = coordinator
        .commit(object(2), vec![producer_span(1, 0, 2)])
        .await
        .expect("the retry is an ordinary success, not a refusal");
    assert_eq!(
        replay.base_offset(&topic(), partition()),
        0,
        "the offset the first commit recorded, not a new one"
    );
    assert_eq!(
        replay.outcomes(),
        first.outcomes(),
        "the same span, answered the same way, either side of the rewrite"
    );
    assert_eq!(
        log.read_from(CommitVersion::ZERO, 16)
            .await
            .expect("the log reads back")
            .len(),
        2,
        "the retry never reached the journal, so the rewrite is the last entry"
    );

    drop(coordinator);
    driver
        .await
        .expect("the loop ends when its last handle drops");
}

/// ⚠️ **And nothing rebuilds producer state from the log**, which is the
/// premise `M5.49` was filed on and the reason `ADR-0038` exists: a second
/// coordinator over a non-empty log refuses rather than folding it, so FR-14's
/// window is the live coordinator's own memory — bounded by
/// `MAX_TRACKED_PRODUCERS` — and compaction cannot erase what no path reads.
/// Building the replay path `ReplayRequired` names fails this test, which is
/// where `ADR-0038`'s two obligations have to be kept.
///
/// ⚠️ **`M11.7`, `ADR-0031` point 5's `CoordinatorEpoch` half.** A second
/// coordinator is what a zombie *coordinator* scenario needs to exist at
/// all — and `Coordinator::open` already refuses one over any non-empty
/// log, unconditionally, before it could ever answer a sequence check any
/// more than it could assign an offset. `M6` is what elects a genuine
/// replacement (`coordinator_epoch.rs`'s own doc); until then, a log
/// carrying producer-sequence history is refused open exactly the same as
/// one carrying only offsets — the same `ReplayRequired` this file's own
/// `opening_over_a_log_that_already_holds_entries_refuses` already proves,
/// checked here against a log a producer-bearing commit actually built —
/// which is what makes it the one that would notice a replay path folding
/// that history instead.
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
        Arc::new(oqueue_core::FakeClock::new()),
    )
    .await
    .expect_err("a non-empty log refuses a second coordinator, sequence history or not");

    assert_eq!(
        rejected.error(),
        &CoordinatorError::ReplayRequired { last_version: 0 }
    );
}
