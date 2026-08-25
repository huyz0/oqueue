//! The `MaterializedIndex` contract, run against every implementation of it.
//!
//! ⚠️ **One suite, two implementors**, the shape `oqueue-store`'s backend suite
//! uses. `FakeMaterializedIndex` lives beside the trait in `oqueue-core` so
//! downstream crates can test against it without depending on this one
//! (`contracts.md` rules 9 and 11); [`MemoryIndex`] here is the real fold. A
//! contract asserted only against the fake is a contract only the fake has.

// Every `expect` is on a value the suite itself built from a literal it
// controls, so a panic means the suite is wrong, not the code under test.
#![allow(clippy::expect_used)]

use oqueue_core::{
    ByteRange, CommitVersion, CommittedSpan, FakeMaterializedIndex, MaterializedIndex,
    MetadataEntry, MetadataRecord, ObjectKey, Offset, PartitionId, TopicId,
};
use oqueue_index::MemoryIndex;

fn topic(name: &str) -> TopicId {
    TopicId::new(name.to_owned()).expect("a valid topic")
}

fn partition(index: i32) -> PartitionId {
    PartitionId::new(index).expect("a valid partition")
}

fn offset(value: i64) -> Offset {
    Offset::new(value).expect("a valid offset")
}

/// A commit adding `records` to `orders`/0.
fn commit(version: u64, records: u32) -> MetadataEntry {
    commit_on(version, "orders", 0, records)
}

fn commit_on(version: u64, name: &str, part: i32, records: u32) -> MetadataEntry {
    MetadataEntry::new(
        CommitVersion::new(version),
        MetadataRecord::BatchCommitted {
            object: ObjectKey::new(format!("obj-{version}")).expect("a valid key"),
            spans: vec![CommittedSpan::new(
                topic(name),
                partition(part),
                records,
                ByteRange::Full,
            )],
        },
    )
}

/// The cases, generic over the implementation. Each takes a fresh, empty index.
mod contract {
    use super::{commit, commit_on, offset, partition, topic};
    use oqueue_core::{
        CommitVersion, Error, MaterializedIndex, MetadataEntry, MetadataRecord, Offset,
    };

    /// A fresh index has folded nothing and knows no partition.
    pub(super) fn a_fresh_index_is_empty<I: MaterializedIndex>(index: &I) {
        assert_eq!(index.applied_upto(), None);
        assert_eq!(
            index.end_offset(&topic("orders"), partition(0)),
            Offset::ZERO
        );
    }

    /// ⚠️ The fold, and where gap-freeness actually comes from. Offsets are
    /// never assigned here — they are the running sum of record counts, so
    /// two batches of 3 and 4 put the next record at 7 with nothing in
    /// between deciding it.
    pub(super) fn end_offset_is_the_running_sum_of_counts<I: MaterializedIndex>(index: &I) {
        index.apply(&[commit(1, 3)]).expect("applied");
        assert_eq!(index.end_offset(&topic("orders"), partition(0)), offset(3));

        index.apply(&[commit(2, 4)]).expect("applied");
        assert_eq!(index.end_offset(&topic("orders"), partition(0)), offset(7));
    }

    /// Partitions are independent — one topic's records never move another's
    /// offsets.
    pub(super) fn partitions_are_folded_independently<I: MaterializedIndex>(index: &I) {
        index
            .apply(&[commit_on(1, "orders", 0, 5), commit_on(2, "orders", 1, 2)])
            .expect("applied");
        index
            .apply(&[commit_on(3, "payments", 0, 9)])
            .expect("applied");

        assert_eq!(index.end_offset(&topic("orders"), partition(0)), offset(5));
        assert_eq!(index.end_offset(&topic("orders"), partition(1)), offset(2));
        assert_eq!(
            index.end_offset(&topic("payments"), partition(0)),
            offset(9)
        );
        assert_eq!(
            index.end_offset(&topic("orders"), partition(7)),
            Offset::ZERO
        );
    }

    /// One object spanning many partitions is one entry — FR-32's whole point.
    pub(super) fn one_entry_can_span_many_partitions<I: MaterializedIndex>(index: &I) {
        use oqueue_core::{ByteRange, CommittedSpan, ObjectKey};
        let entry = MetadataEntry::new(
            CommitVersion::new(1),
            MetadataRecord::BatchCommitted {
                object: ObjectKey::new("bundle".to_owned()).expect("a valid key"),
                // ⚠️ Distinct bounded regions, not two `Full`s. Two spans of
                // one object that both claim all of it are not two regions,
                // and asserting that shape here would bless it.
                spans: vec![
                    CommittedSpan::new(
                        topic("orders"),
                        partition(0),
                        2,
                        ByteRange::bounded(0, 64).expect("a valid region"),
                    ),
                    CommittedSpan::new(
                        topic("payments"),
                        partition(0),
                        3,
                        ByteRange::bounded(64, 96).expect("a valid region"),
                    ),
                ],
            },
        );
        index.apply(std::slice::from_ref(&entry)).expect("applied");

        assert_eq!(index.end_offset(&topic("orders"), partition(0)), offset(2));
        assert_eq!(
            index.end_offset(&topic("payments"), partition(0)),
            offset(3)
        );
    }

    /// `applied_upto` tracks the highest version folded.
    pub(super) fn applied_upto_follows_the_fold<I: MaterializedIndex>(index: &I) {
        index.apply(&[commit(1, 1), commit(4, 1)]).expect("applied");
        assert_eq!(index.applied_upto(), Some(CommitVersion::new(4)));
    }

    /// ⚠️ `M3.md` task 14 again, on this side of the seam. A delta applied out
    /// of order would make the running sum depend on arrival order, so it is
    /// refused rather than sorted.
    pub(super) fn an_out_of_order_delta_is_rejected<I: MaterializedIndex>(index: &I) {
        index.apply(&[commit(5, 1)]).expect("applied");
        let err = index
            .apply(&[commit(3, 1)])
            .expect_err("going backwards is refused");
        assert_eq!(
            err,
            Error::NonMonotonicCommitVersion {
                expected_above: 5,
                got: 3
            }
        );
    }

    /// A replayed version is out of order too — this is the ack-lost retry,
    /// and folding it twice would double-count the object's records.
    pub(super) fn a_replayed_version_is_rejected<I: MaterializedIndex>(index: &I) {
        index.apply(&[commit(5, 2)]).expect("applied");
        index
            .apply(&[commit(5, 2)])
            .expect_err("re-applying the same version is refused");
        assert_eq!(index.end_offset(&topic("orders"), partition(0)), offset(2));
    }

    /// ⚠️ A rejected apply changes nothing, for the same reason the log's does:
    /// a caller retrying after a rejection would otherwise fold onto an index
    /// already carrying part of the batch.
    pub(super) fn a_rejected_apply_folds_nothing<I: MaterializedIndex>(index: &I) {
        index.apply(&[commit(1, 3)]).expect("applied");
        index
            .apply(&[commit(2, 5), commit(2, 5)])
            .expect_err("refused for its second entry");

        assert_eq!(index.end_offset(&topic("orders"), partition(0)), offset(3));
        assert_eq!(index.applied_upto(), Some(CommitVersion::new(1)));
    }

    /// An epoch change advances the version without moving any offset — it is
    /// an event about the log, not about a partition.
    pub(super) fn an_epoch_change_moves_no_offset<I: MaterializedIndex>(index: &I) {
        use oqueue_core::CoordinatorEpoch;
        index.apply(&[commit(1, 4)]).expect("applied");
        index
            .apply(&[MetadataEntry::new(
                CommitVersion::new(2),
                MetadataRecord::EpochChanged {
                    epoch: CoordinatorEpoch::new(1),
                },
            )])
            .expect("applied");

        assert_eq!(index.end_offset(&topic("orders"), partition(0)), offset(4));
        assert_eq!(index.applied_upto(), Some(CommitVersion::new(2)));
    }

    /// An empty batch is accepted and changes nothing — `M3.8`'s batched
    /// applier hits empty remainders at page boundaries, and the paired
    /// `MetadataLog` seam pins the same case.
    pub(super) fn an_empty_apply_is_a_no_op<I: MaterializedIndex>(index: &I) {
        index.apply(&[commit(3, 2)]).expect("applied");
        index.apply(&[]).expect("an empty batch is accepted");

        assert_eq!(index.applied_upto(), Some(CommitVersion::new(3)));
        assert_eq!(index.end_offset(&topic("orders"), partition(0)), offset(2));
    }

    /// ⚠️ **The cache property, and the reason this is a trait at all.**
    ///
    /// `M3.md` task 9: the index is a cache, never the source of truth. So
    /// dropping it must lose nothing that the metadata log cannot put back —
    /// which is what makes it safe for the coordinator to discard one under
    /// memory pressure, and what `M3.11`'s degraded mode will rest on. The
    /// assertion is equality of the *whole* observable state before and after,
    /// not merely that a refill succeeds.
    pub(super) fn dropping_and_refilling_reproduces_the_index<I: MaterializedIndex>(index: &I) {
        let log = [
            commit_on(1, "orders", 0, 3),
            commit_on(2, "payments", 1, 7),
            commit_on(3, "orders", 0, 4),
        ];
        index.apply(&log).expect("applied");

        let before = (
            index.applied_upto(),
            index.end_offset(&topic("orders"), partition(0)),
            index.end_offset(&topic("payments"), partition(1)),
        );

        index.clear();
        assert_eq!(index.applied_upto(), None, "clear left state behind");
        assert_eq!(
            index.end_offset(&topic("orders"), partition(0)),
            Offset::ZERO
        );

        index.apply(&log).expect("the same log refills it");

        let after = (
            index.applied_upto(),
            index.end_offset(&topic("orders"), partition(0)),
            index.end_offset(&topic("payments"), partition(1)),
        );
        assert_eq!(before, after, "a refill did not reproduce the index");
    }
}

/// Runs every case against one implementation. Each case gets a fresh index —
/// sharing one would let an earlier case's fold decide a later one's outcome,
/// and the ordering rules under test are exactly the kind that would then pass
/// for the wrong reason.
fn run_contract<I: MaterializedIndex>(make: impl Fn() -> I) {
    contract::a_fresh_index_is_empty(&make());
    contract::end_offset_is_the_running_sum_of_counts(&make());
    contract::partitions_are_folded_independently(&make());
    contract::one_entry_can_span_many_partitions(&make());
    contract::applied_upto_follows_the_fold(&make());
    contract::an_out_of_order_delta_is_rejected(&make());
    contract::a_replayed_version_is_rejected(&make());
    contract::a_rejected_apply_folds_nothing(&make());
    contract::an_epoch_change_moves_no_offset(&make());
    contract::an_empty_apply_is_a_no_op(&make());
    contract::dropping_and_refilling_reproduces_the_index(&make());
}

#[test]
fn the_memory_index_satisfies_the_contract() {
    run_contract(MemoryIndex::new);
}

#[test]
fn the_fake_satisfies_the_contract() {
    run_contract(FakeMaterializedIndex::new);
}

/// ⚠️ Reports how far it has folded and never the partitions it holds — a
/// `Debug` line in a test failure must not become a listing of a tenant's
/// topics. Asserting only what is *absent* would pass against a `Debug` that
/// rendered nothing, so what it does render is pinned too.
#[test]
fn the_memory_index_reports_its_progress_without_listing_topics() {
    let index = MemoryIndex::new();
    index.apply(&[commit(1, 5)]).expect("applied");

    let rendered = format!("{index:?}");
    assert!(!rendered.contains("orders"), "rendered: {rendered}");
    assert!(rendered.contains("MemoryIndex"), "rendered: {rendered}");
    assert!(rendered.contains('1'), "rendered: {rendered}");
}

/// The seam is `dyn`-compatible — the broker holds whichever materialization
/// it was configured with, the same reason `ObjectStore` is shaped this way.
#[test]
fn the_seam_is_dyn_compatible() {
    let index: std::sync::Arc<dyn MaterializedIndex> = std::sync::Arc::new(MemoryIndex::new());
    index.apply(&[commit(1, 2)]).expect("applied");
    assert_eq!(index.end_offset(&topic("orders"), partition(0)), offset(2));
}
