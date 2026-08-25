//! What the applier promises: bounded batches, and a restart that replays only
//! the delta.
//!
//! ⚠️ **The bookmark is the index, not the applier**, and that is what these
//! tests are really about. A crash is modelled by dropping the applier and
//! building a new one over the same index — which is exactly what a process
//! restart is, since the applier holds no state of its own to lose.

// Every `expect` is on a value the suite itself built from a literal it
// controls, so a panic means the suite is wrong, not the code under test.
#![allow(clippy::expect_used)]

use oqueue_core::{
    ByteRange, CommitVersion, CommittedSpan, FakeMetadataLog, MaterializedIndex, MetadataEntry,
    MetadataLog, ObjectKey, Offset, PartitionId, TopicId,
};
use oqueue_index::{APPLY_BATCH_ENTRIES, CatchUp, LogApplier, MemoryIndex};
use std::future::Future;
use std::sync::Arc;
use std::task::{Context, Poll, Waker};

/// Drives a future to completion on this thread.
///
/// ⚠️ A spin rather than a runtime, and sound here for the reason
/// `oqueue-core`'s own suites give: everything under test is a synchronous
/// fake, so a `Pending` is transient and a spin always terminates. This crate
/// takes no runtime dependency, and `testing.md`'s no-sleep rule is satisfied
/// by there being nothing to wait for.
fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = Box::pin(future);
    let waker = Waker::noop();
    let mut context = Context::from_waker(waker);
    loop {
        if let Poll::Ready(value) = future.as_mut().poll(&mut context) {
            return value;
        }
    }
}

fn topic() -> TopicId {
    TopicId::new("orders".to_owned()).expect("a valid topic")
}

fn partition() -> PartitionId {
    PartitionId::new(0).expect("a valid partition")
}

fn offset(value: usize) -> Offset {
    Offset::new(i64::try_from(value).expect("a count that fits")).expect("a valid offset")
}

/// One committed object adding one record to `orders`/0.
fn entry(version: u64) -> MetadataEntry {
    MetadataEntry::new(
        CommitVersion::new(version),
        oqueue_core::MetadataRecord::BatchCommitted {
            object: ObjectKey::new(format!("obj-{version}")).expect("a valid key"),
            spans: vec![CommittedSpan::new(topic(), partition(), 1, ByteRange::Full)],
        },
    )
}

/// A log holding `count` entries, versions `0..count`.
fn log_of(count: usize) -> Arc<dyn MetadataLog> {
    let log = Arc::new(FakeMetadataLog::new());
    let entries: Vec<MetadataEntry> = (0..count).map(|n| entry(n as u64)).collect();
    block_on(log.append(&entries)).expect("a fresh log accepts an ordered batch");
    log
}

/// A fresh in-memory index, behind the seam the applier folds into.
fn fresh_index() -> Arc<dyn MaterializedIndex> {
    Arc::new(MemoryIndex::new())
}

#[test]
fn a_fresh_applier_starts_at_the_beginning_of_the_log() {
    let applier = LogApplier::new(log_of(0), fresh_index());
    assert_eq!(
        applier.resume_from().expect("a version"),
        CommitVersion::ZERO
    );
    assert_eq!(
        block_on(applier.catch_up()).expect("caught up"),
        CatchUp {
            entries: 0,
            batches: 0,
            reads: 1
        }
    );
}

#[test]
fn catching_up_folds_every_entry_the_log_holds() {
    let applier = LogApplier::new(log_of(7), fresh_index());
    assert_eq!(
        block_on(applier.catch_up()).expect("caught up"),
        CatchUp {
            entries: 7,
            batches: 1,
            reads: 1
        },
        "⚠️ one read, not two: a short page means the log is exhausted, and \
         reading again to learn that costs a round trip on every engine doc \
         10 #12 names"
    );
    assert_eq!(applier.index().end_offset(&topic(), partition()), offset(7));
    assert_eq!(
        applier.index().applied_upto(),
        Some(CommitVersion::new(6)),
        "the bookmark is the last version folded, not the count"
    );
}

#[test]
fn a_batch_is_bounded_and_more_than_one_covers_the_rest() {
    let total = APPLY_BATCH_ENTRIES + 250;
    let applier = LogApplier::new(log_of(total), fresh_index());

    let first = block_on(applier.apply_next_batch()).expect("a batch");
    assert_eq!(
        first, APPLY_BATCH_ENTRIES,
        "one apply covers at most the batch size — the ceiling that divides the transaction rate down"
    );

    let rest = block_on(applier.catch_up()).expect("caught up");
    assert_eq!(
        rest,
        CatchUp {
            entries: 250,
            batches: 1,
            reads: 1
        }
    );
    assert_eq!(
        applier.index().end_offset(&topic(), partition()),
        offset(total)
    );
}

/// ⚠️ A log holding an exact multiple of the batch size never returns a short
/// page, so the loop's *empty*-page exit is the only thing that ends it.
#[test]
fn a_log_that_is_an_exact_multiple_of_the_batch_size_still_terminates() {
    let applier = LogApplier::new(log_of(APPLY_BATCH_ENTRIES), fresh_index());
    assert_eq!(
        block_on(applier.catch_up()).expect("caught up"),
        CatchUp {
            entries: APPLY_BATCH_ENTRIES,
            batches: 1,
            reads: 2
        },
        "a full page cannot end the loop, so the empty read that does costs a \
         round trip and no transaction"
    );
    assert_eq!(
        applier.index().end_offset(&topic(), partition()),
        offset(APPLY_BATCH_ENTRIES)
    );
}

#[test]
fn catching_up_twice_folds_nothing_the_second_time() {
    let applier = LogApplier::new(log_of(5), fresh_index());
    assert_eq!(block_on(applier.catch_up()).expect("caught up").entries, 5);
    assert_eq!(
        block_on(applier.catch_up()).expect("caught up").entries,
        0,
        "a replayed version would be refused as non-monotonic, so this must \
         not re-read what it already folded"
    );
}

/// The `M3.8` fault-injection case: killed mid-apply, a restart replays the
/// delta and not the log.
///
/// ⚠️ **Mid-batch and between-batch are indistinguishable here, and that is
/// the property.** An apply is all-or-nothing, so a batch interrupted part-way
/// folded nothing and left the bookmark where the previous batch put it. What
/// a restart therefore has to redo is bounded by one batch, whichever moment
/// it died at.
#[test]
fn a_restart_replays_the_delta_rather_than_the_log() {
    let total = APPLY_BATCH_ENTRIES * 2 + 500;
    let log = log_of(total);
    let index = Arc::new(fresh_index());

    let dying = LogApplier::new(Arc::clone(&log), Arc::clone(&index));
    assert_eq!(
        block_on(dying.apply_next_batch()).expect("a batch"),
        APPLY_BATCH_ENTRIES
    );
    drop(dying);

    // The process comes back. The applier is new; the index is what survived.
    let restarted = LogApplier::new(Arc::clone(&log), Arc::clone(&index));
    assert_eq!(
        restarted.resume_from().expect("a version"),
        CommitVersion::new(APPLY_BATCH_ENTRIES as u64),
        "resumption starts one past what was folded, read out of the index"
    );

    let replayed = block_on(restarted.catch_up()).expect("caught up").entries;
    assert_eq!(
        replayed,
        total - APPLY_BATCH_ENTRIES,
        "the delta, not the log — a restart that re-folded everything would \
         report {total}"
    );
    assert_eq!(index.end_offset(&topic(), partition()), offset(total));
}

/// A dropped index is refilled from the log, and arrives at the same place —
/// the cache property, driven by the thing that actually refills it.
#[test]
fn a_dropped_index_is_refilled_from_the_log() {
    let applier = LogApplier::new(log_of(9), fresh_index());
    block_on(applier.catch_up()).expect("caught up");
    let before = applier.index().end_offset(&topic(), partition());

    applier.index().clear();
    assert_eq!(applier.index().applied_upto(), None);
    assert_eq!(
        applier.resume_from().expect("a version"),
        CommitVersion::ZERO,
        "a cleared index resumes from the start, because that is where its \
         bookmark now says it is"
    );

    assert_eq!(block_on(applier.catch_up()).expect("caught up").entries, 9);
    assert_eq!(applier.index().end_offset(&topic(), partition()), before);
}
