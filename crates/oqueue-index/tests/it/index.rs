//! The `MaterializedIndex` contract, run against every implementation of it.
//!
//! ⚠️ **One suite, two implementors — and today that is one fold twice.**
//! `FakeMaterializedIndex` lives beside the trait in `oqueue-core` so
//! downstream crates can test against it without depending on this one
//! (`contracts.md` rules 9 and 11), and [`MemoryIndex`] here is the one a
//! broker serves from. Both wrap the same `IndexState`, so running the suite
//! against both currently buys **one** implementation's worth of assurance,
//! not two — M3's checkpoint review found this file claiming otherwise
//! (`M3.24`), and the sentence it claimed it with ("a contract asserted only
//! against the fake is a contract only the fake has") stays true as a *reason
//! to keep the suite*, which is why it is corrected rather than deleted.
//!
//! ⚠️ The suite earns its shape the moment the two diverge. ⚠️ `M3.11` was to
//! be that moment and is not — a ceiling at this index's keying gives back
//! range a rebuild cannot restore, so `roadmap.md` carries the quota, and the
//! divergence with it, to `M5`. doc 10 #12's engine is the implementation
//! `contracts.md` rule 3 is actually satisfied by.

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

/// A commit whose span names a real bounded region, so the byte budget has a
/// length to charge against. ⚠️ `commit`'s `ByteRange::Full` deliberately has
/// none — `Full` names the whole object, whose size only the store knows.
fn sized_commit(version: u64, records: u32, bytes: u64) -> MetadataEntry {
    MetadataEntry::new(
        CommitVersion::new(version),
        MetadataRecord::BatchCommitted {
            object: ObjectKey::new(format!("obj-{version}")).expect("a valid key"),
            spans: vec![CommittedSpan::new(
                topic("orders"),
                partition(0),
                records,
                ByteRange::bounded(0, bytes).expect("a non-empty range"),
            )],
        },
    )
}

/// The cases, generic over the implementation. Each takes a fresh, empty index.
mod contract {
    use super::{commit, commit_on, offset, partition, sized_commit, topic};
    use oqueue_core::{
        CommitVersion, Error, MAX_BATCHES_PER_PAGE, MaterializedIndex, MetadataEntry,
        MetadataRecord, Offset,
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
    /// memory pressure, and what `M5`'s degraded mode will rest on. The
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

    /// ⚠️ **`entries()` is part of guarantee 3's "same observable state"**
    /// (`ADR-0025`), so a refill has to reproduce it too. An implementation
    /// keeping a maintained counter — which the trait asks for, because this
    /// is read on paths NFR-2 bounds — and forgetting to zero it in `clear`
    /// reports twice the truth after a drop-and-refill, and `M5`'s quota is
    /// what would then act on the number.
    pub(super) fn the_entry_count_survives_a_drop_and_refill<I: MaterializedIndex>(index: &I) {
        let log = [commit(1, 3), commit_on(2, "payments", 1, 4), commit(3, 2)];
        index.apply(&log).expect("applied");
        let before = index.entries();
        assert_eq!(before, 3, "one entry per span folded");

        index.clear();
        assert_eq!(index.entries(), 0, "a dropped cache holds nothing");

        index.apply(&log).expect("refilled");
        assert_eq!(
            index.entries(),
            before,
            "and a refill reproduces it exactly"
        );
    }
    /// FR-12's zero-GET case, at the seam that decides it: a fetch that is
    /// already at the high watermark is told to read nothing, so there is
    /// nothing for it to GET.
    pub(super) fn a_fetch_at_the_high_watermark_finds_no_batches<I: MaterializedIndex>(index: &I) {
        index.apply(&[commit(1, 3), commit(2, 4)]).expect("applied");
        let hwm = index.end_offset(&topic("orders"), partition(0));
        assert_eq!(hwm, offset(7));
        assert_eq!(
            index
                .find_batches(&topic("orders"), partition(0), hwm, u64::MAX)
                .expect("a page"),
            Vec::new(),
            "nothing to read at the end of the log"
        );
        // And past it, which is what a consumer racing a produce asks for.
        assert!(
            index
                .find_batches(&topic("orders"), partition(0), offset(99), u64::MAX)
                .expect("a page")
                .is_empty()
        );
    }

    /// A partition the index never folded is empty rather than an error — the
    /// index knows what the log said, not which topics exist.
    pub(super) fn an_unknown_partition_finds_no_batches<I: MaterializedIndex>(index: &I) {
        assert!(
            index
                .find_batches(&topic("ghost"), partition(9), Offset::ZERO, u64::MAX)
                .expect("a page")
                .is_empty()
        );
    }

    /// Everything from `start` onward, in ascending offset order — not just
    /// the one object that contains `start`.
    pub(super) fn a_page_runs_from_start_to_the_end_of_the_log<I: MaterializedIndex>(index: &I) {
        // ⚠️ Sized, not `commit`: an unknown length ends the page after one
        // batch, which is the case the test below this one is about.
        for version in 1..=4 {
            index
                .apply(&[sized_commit(version, 2, 10)])
                .expect("applied");
        }
        let page = index
            .find_batches(&topic("orders"), partition(0), offset(3), u64::MAX)
            .expect("a page");
        let bases: Vec<i64> = page
            .iter()
            .map(|b| b.reference().base_offset().get())
            .collect();
        assert_eq!(
            bases,
            vec![2, 4, 6],
            "the object holding offset 3 and every one after it, in order"
        );
    }

    /// The budget is charged against known lengths, and never returns empty
    /// because the first batch is too big.
    pub(super) fn the_byte_budget_bounds_the_page_but_always_yields_one<I: MaterializedIndex>(
        index: &I,
    ) {
        index
            .apply(&[
                sized_commit(1, 2, 100),
                sized_commit(2, 2, 100),
                sized_commit(3, 2, 100),
            ])
            .expect("applied");

        let two = index
            .find_batches(&topic("orders"), partition(0), Offset::ZERO, 250)
            .expect("a page");
        assert_eq!(two.len(), 2, "100 + 100 fits in 250, a third does not");

        let one = index
            .find_batches(&topic("orders"), partition(0), Offset::ZERO, 1)
            .expect("a page");
        assert_eq!(
            one.len(),
            1,
            "a budget below the first batch still yields it, or the consumer \
                 never advances"
        );
    }

    /// A batch the index cannot price charges nothing against the byte budget
    /// — it is unpriceable, not free, and the page count is what bounds it.
    pub(super) fn an_unknown_length_charges_nothing<I: MaterializedIndex>(index: &I) {
        // `commit` uses ByteRange::Full, whose length only the store knows.
        index
            .apply(&[commit(1, 2), commit(2, 2), sized_commit(3, 2, 10)])
            .expect("applied");
        let page = index
            .find_batches(&topic("orders"), partition(0), Offset::ZERO, 10)
            .expect("a page");
        assert_eq!(
            page.len(),
            3,
            "two cannot be priced and pass through the budget; the third is \
             charged against it and exactly fills it"
        );
        assert_eq!(page[0].known_len(), None);
        assert_eq!(page[2].known_len(), Some(10));

        assert_eq!(
            index
                .find_batches(&topic("orders"), partition(0), Offset::ZERO, 5)
                .expect("a page")
                .len(),
            2,
            "the priced one is refused by a budget below it; the unpriceable \
             ones before it are not"
        );
    }

    /// However many batches a partition has, one page names at most
    /// [`MAX_BATCHES_PER_PAGE`] of them — NFR-30's bound on a cold read.
    pub(super) fn a_page_is_bounded_by_its_batch_count<I: MaterializedIndex>(index: &I) {
        for version in 1..=(MAX_BATCHES_PER_PAGE as u64 + 5) {
            index.apply(&[commit(version, 1)]).expect("applied");
        }
        assert_eq!(
            index
                .find_batches(&topic("orders"), partition(0), Offset::ZERO, u64::MAX)
                .expect("a page")
                .len(),
            MAX_BATCHES_PER_PAGE
        );
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
    contract::a_fetch_at_the_high_watermark_finds_no_batches(&make());
    contract::an_unknown_partition_finds_no_batches(&make());
    contract::a_page_runs_from_start_to_the_end_of_the_log(&make());
    contract::the_byte_budget_bounds_the_page_but_always_yields_one(&make());
    contract::an_unknown_length_charges_nothing(&make());
    contract::a_page_is_bounded_by_its_batch_count(&make());
    contract::the_entry_count_survives_a_drop_and_refill(&make());
}

#[test]
fn the_memory_index_satisfies_the_contract() {
    run_contract(MemoryIndex::new);
}

#[test]
fn the_fake_satisfies_the_contract() {
    run_contract(FakeMaterializedIndex::new);
}
