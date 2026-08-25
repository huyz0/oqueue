//! The metadata-log seam's conformance suite, and the fake satisfying it.
//!
//! ⚠️ **Written as a suite rather than as tests of the fake**, because the
//! fake is not the point: `ADR-0020` point 5 defers the durable engine (doc 10
//! #12) to a benchmark that has not been run, so the first real implementor
//! does not exist yet. Every case below takes an `impl MetadataLog`, so the
//! day one does, it inherits the contract rather than being trusted to have
//! reimplemented it — the same discipline `oqueue-store`'s backend suite
//! carries, minus the backend matrix, which has nothing to enumerate yet.
//!
//! ⚠️ **No async runtime**, same floor as this crate's other seam tests: the
//! driver below busy-polls, which is sound here because nothing under test
//! ever parks.

// Every `expect` is on a value the suite itself constructed from a literal it
// controls, so a panic means the suite is wrong, not the code under test.
#![allow(clippy::expect_used)]

use oqueue_core::{
    ByteRange, CommitVersion, CommittedSpan, FakeMetadataLog, MetadataEntry, MetadataLog,
    MetadataRecord, ObjectKey, PartitionId, TopicId,
};
use std::future::Future;
use std::task::{Context, Poll, Waker};

fn block_on<F: Future>(future: F) -> F::Output {
    let mut future = Box::pin(future);
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return value,
            Poll::Pending => std::hint::spin_loop(),
        }
    }
}

fn commit(version: u64, records: u32) -> MetadataEntry {
    let span = CommittedSpan::new(
        TopicId::new("orders").expect("a valid topic"),
        PartitionId::new(0).expect("a valid partition"),
        records,
        ByteRange::Full,
    );
    MetadataEntry::new(
        CommitVersion::new(version),
        MetadataRecord::BatchCommitted {
            object: ObjectKey::new(format!("obj-{version}")).expect("a valid key"),
            spans: vec![span],
        },
    )
}

/// The backend-agnostic cases. Each takes a freshly built, empty log.
mod conformance {
    use super::{block_on, commit};
    use oqueue_core::{
        CommitVersion, CoordinatorEpoch, Error, MetadataEntry, MetadataLog, MetadataRecord,
    };

    /// What was appended reads back, in the order it was appended.
    pub(super) fn append_then_read_returns_what_went_in<L: MetadataLog>(log: &L) {
        let batch = vec![commit(1, 10), commit(2, 20), commit(3, 30)];
        block_on(log.append(&batch)).expect("a strictly increasing batch is accepted");

        let read = block_on(log.read_from(CommitVersion::ZERO, 100)).expect("read succeeds");
        assert_eq!(read, batch);
    }

    /// ⚠️ `M3.md` task 14. A batch whose versions do not strictly increase is
    /// refused — the log is the serialization point, and an out-of-order
    /// entry would make the fold that derives offsets depend on arrival order.
    pub(super) fn an_out_of_order_batch_is_rejected<L: MetadataLog>(log: &L) {
        let err = block_on(log.append(&[commit(2, 1), commit(1, 1)]))
            .expect_err("a decreasing batch is refused");
        assert_eq!(
            err,
            Error::NonMonotonicCommitVersion {
                expected_above: 2,
                got: 1
            }
        );
    }

    /// A repeat of the same version is out of order too — strictly
    /// increasing, not merely non-decreasing. Two records at one version
    /// would give the fold two answers for the same position.
    pub(super) fn a_repeated_version_is_rejected<L: MetadataLog>(log: &L) {
        let err = block_on(log.append(&[commit(5, 1), commit(5, 1)]))
            .expect_err("a repeated version is refused");
        assert_eq!(
            err,
            Error::NonMonotonicCommitVersion {
                expected_above: 5,
                got: 5
            }
        );
    }

    /// Ordering holds *across* appends, not only within one.
    pub(super) fn an_append_below_the_last_stored_version_is_rejected<L: MetadataLog>(log: &L) {
        block_on(log.append(&[commit(7, 1)])).expect("the first append is accepted");
        let err = block_on(log.append(&[commit(4, 1)])).expect_err("going backwards is refused");
        assert_eq!(
            err,
            Error::NonMonotonicCommitVersion {
                expected_above: 7,
                got: 4
            }
        );
    }

    /// ⚠️ Equality **across** batches, which is the cell an ack-lost retry
    /// lands in first: the coordinator appended version 7, the acknowledgement
    /// was lost, and it re-offers 7. An engine that checks in-batch order with
    /// `<=` but guards its persisted high-water mark with `<` passes every
    /// other case in this suite and accepts this one — and the fold then
    /// counts that object's records twice, in silence.
    pub(super) fn a_version_equal_to_the_last_stored_one_is_rejected<L: MetadataLog>(log: &L) {
        block_on(log.append(&[commit(7, 1)])).expect("the first append is accepted");
        let err = block_on(log.append(&[commit(7, 1)]))
            .expect_err("re-offering the last stored version is refused");
        assert_eq!(
            err,
            Error::NonMonotonicCommitVersion {
                expected_above: 7,
                got: 7
            }
        );
        assert_eq!(
            block_on(log.read_from(CommitVersion::ZERO, 100))
                .expect("read succeeds")
                .len(),
            1,
            "the refused retry was stored anyway"
        );
    }

    /// ⚠️ A rejected append leaves the log exactly as it was. Without this a
    /// caller that retries after a rejection replays onto a log holding a
    /// prefix of the batch it thinks was refused, and the fold silently
    /// double-counts.
    pub(super) fn a_rejected_append_stores_nothing<L: MetadataLog>(log: &L) {
        block_on(log.append(&[commit(1, 1)])).expect("the first append is accepted");
        let before = block_on(log.read_from(CommitVersion::ZERO, 100)).expect("read succeeds");

        block_on(log.append(&[commit(2, 1), commit(2, 1)]))
            .expect_err("the batch is refused for its second entry");

        let after = block_on(log.read_from(CommitVersion::ZERO, 100)).expect("read succeeds");
        assert_eq!(before, after, "a refused batch stored a prefix");
    }

    /// `read_from` is inclusive of its start and skips everything below it.
    pub(super) fn read_from_starts_at_the_requested_version<L: MetadataLog>(log: &L) {
        block_on(log.append(&[commit(1, 1), commit(2, 2), commit(3, 3)])).expect("accepted");

        let read = block_on(log.read_from(CommitVersion::new(2), 100)).expect("read succeeds");
        assert_eq!(read, vec![commit(2, 2), commit(3, 3)]);
    }

    /// A read past the end is empty rather than an error — the caller is
    /// caught up, which is the ordinary case for a tail subscriber.
    pub(super) fn a_read_past_the_end_is_empty<L: MetadataLog>(log: &L) {
        block_on(log.append(&[commit(1, 1)])).expect("accepted");
        let read = block_on(log.read_from(CommitVersion::new(99), 100)).expect("read succeeds");
        assert!(read.is_empty());
    }

    /// ⚠️ The page bound is honoured. `M3.9` pulls history in bounded pages,
    /// and a `max_entries` a backend ignores turns a cold catch-up into an
    /// unbounded allocation driven by how far behind the reader is.
    pub(super) fn read_from_honours_its_page_bound<L: MetadataLog>(log: &L) {
        block_on(log.append(&[commit(1, 1), commit(2, 2), commit(3, 3)])).expect("accepted");
        let read = block_on(log.read_from(CommitVersion::ZERO, 2)).expect("read succeeds");
        assert_eq!(read, vec![commit(1, 1), commit(2, 2)]);
    }

    /// An empty log has no last version — distinct from having version zero,
    /// which is a real position a real entry can occupy.
    pub(super) fn an_empty_log_has_no_last_version<L: MetadataLog>(log: &L) {
        assert_eq!(block_on(log.last_version()).expect("read succeeds"), None);
    }

    /// `last_version` tracks the highest stored version.
    pub(super) fn last_version_follows_the_appends<L: MetadataLog>(log: &L) {
        block_on(log.append(&[commit(1, 1), commit(9, 1)])).expect("accepted");
        assert_eq!(
            block_on(log.last_version()).expect("read succeeds"),
            Some(CommitVersion::new(9))
        );
    }

    /// An empty append is accepted and changes nothing — a flush that turned
    /// out to cover no records is not an error.
    pub(super) fn an_empty_append_is_a_no_op<L: MetadataLog>(log: &L) {
        block_on(log.append(&[commit(3, 1)])).expect("accepted");
        block_on(log.append(&[])).expect("an empty batch is accepted");
        assert_eq!(
            block_on(log.last_version()).expect("read succeeds"),
            Some(CommitVersion::new(3))
        );
    }

    /// The log carries every record variant, not only commits.
    pub(super) fn an_epoch_change_round_trips<L: MetadataLog>(log: &L) {
        let entry = MetadataEntry::new(
            CommitVersion::new(1),
            MetadataRecord::EpochChanged {
                epoch: CoordinatorEpoch::new(4),
            },
        );
        block_on(log.append(std::slice::from_ref(&entry))).expect("accepted");
        let read = block_on(log.read_from(CommitVersion::ZERO, 10)).expect("read succeeds");
        assert_eq!(read, vec![entry]);
    }
}

/// Every case above, against the in-memory fake.
///
/// ⚠️ Each case gets its own log. Sharing one would let an earlier case's
/// entries decide a later one's outcome, and the ordering rules under test are
/// exactly the kind that would then pass for the wrong reason.
#[test]
fn the_fake_satisfies_the_conformance_suite() {
    conformance::append_then_read_returns_what_went_in(&FakeMetadataLog::new());
    conformance::an_out_of_order_batch_is_rejected(&FakeMetadataLog::new());
    conformance::a_repeated_version_is_rejected(&FakeMetadataLog::new());
    conformance::an_append_below_the_last_stored_version_is_rejected(&FakeMetadataLog::new());
    conformance::a_version_equal_to_the_last_stored_one_is_rejected(&FakeMetadataLog::new());
    conformance::a_rejected_append_stores_nothing(&FakeMetadataLog::new());
    conformance::read_from_starts_at_the_requested_version(&FakeMetadataLog::new());
    conformance::a_read_past_the_end_is_empty(&FakeMetadataLog::new());
    conformance::read_from_honours_its_page_bound(&FakeMetadataLog::new());
    conformance::an_empty_log_has_no_last_version(&FakeMetadataLog::new());
    conformance::last_version_follows_the_appends(&FakeMetadataLog::new());
    conformance::an_empty_append_is_a_no_op(&FakeMetadataLog::new());
    conformance::an_epoch_change_round_trips(&FakeMetadataLog::new());
}

/// The seam is `dyn`-compatible: `bin/oqueue` must be able to hold an
/// `Arc<dyn MetadataLog>` and choose an engine at startup, the same reason
/// `ObjectStore` is shaped this way (ADR-0002).
#[test]
fn the_seam_is_dyn_compatible() {
    let log: std::sync::Arc<dyn MetadataLog> = std::sync::Arc::new(FakeMetadataLog::new());
    block_on(log.append(&[commit(1, 1)])).expect("accepted");
    assert_eq!(
        block_on(log.last_version()).expect("read succeeds"),
        Some(CommitVersion::new(1))
    );
}

/// The fake never prints the records it holds — same rule its sibling
/// `FakeObjectStore` follows, so a `Debug` line in a test failure cannot
/// become a transcript of the log.
///
/// ⚠️ Asserting only what is *absent* would pass against a `Debug` that
/// rendered nothing at all, so what it does render is pinned too. Mutation
/// testing is what surfaced that: `fmt -> Ok(Default::default())` survived the
/// absence assertions alone.
#[test]
fn the_fake_reports_its_size_without_printing_what_it_holds() {
    let log = FakeMetadataLog::new();
    block_on(log.append(&[commit(1, 42)])).expect("accepted");

    let rendered = format!("{log:?}");
    assert!(!rendered.contains("orders"), "rendered: {rendered}");
    assert!(!rendered.contains("obj-1"), "rendered: {rendered}");
    assert!(rendered.contains("FakeMetadataLog"), "rendered: {rendered}");
    assert!(rendered.contains('1'), "rendered: {rendered}");
}

/// ⚠️ Building the future is not appending.
///
/// A caller that loses a `select!` to a deadline drops the future unpolled.
/// The fake must agree with a real engine that nothing was stored — otherwise
/// the retry path diverges between them, which is the one thing a fake exists
/// to rule out.
#[test]
fn an_append_whose_future_is_never_polled_stores_nothing() {
    let log = FakeMetadataLog::new();

    let batch = [commit(1, 1)];
    let unpolled = log.append(&batch);
    drop(unpolled);

    assert_eq!(log.len(), 0, "building the future stored the batch");
    assert!(log.is_empty());
    assert_eq!(block_on(log.last_version()).expect("read succeeds"), None);
}

/// `len` and `is_empty` track what was appended. Inspection helpers, but a
/// test asserting a log is empty is worth no more than they are.
#[test]
fn len_and_is_empty_track_the_appends() {
    let log = FakeMetadataLog::new();
    assert_eq!(log.len(), 0);
    assert!(log.is_empty());

    block_on(log.append(&[commit(1, 1), commit(2, 1)])).expect("accepted");
    assert_eq!(log.len(), 2);
    assert!(!log.is_empty());

    block_on(log.append(&[commit(3, 1)])).expect("accepted");
    assert_eq!(log.len(), 3);
}
