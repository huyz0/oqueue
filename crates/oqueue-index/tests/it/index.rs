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

#![allow(unreachable_pub)]
// Every `expect` is on a value the suite itself built from a literal it
// controls, so a panic means the suite is wrong, not the code under test.
#![allow(clippy::expect_used)]

use oqueue_core::{
    ByteRange, CommitVersion, CommittedSpan, FakeMaterializedIndex, MaterializedIndex,
    MetadataEntry, MetadataRecord, ObjectKey, Offset, PartitionId, TopicId,
};
use oqueue_index::MemoryIndex;

pub fn topic(name: &str) -> TopicId {
    TopicId::new(name.to_owned()).expect("a valid topic")
}

pub fn partition(index: i32) -> PartitionId {
    PartitionId::new(index).expect("a valid partition")
}

pub fn offset(value: i64) -> Offset {
    Offset::new(value).expect("a valid offset")
}

/// A commit adding `records` to `orders`/0.
pub fn commit(version: u64, records: u32) -> MetadataEntry {
    commit_on(version, "orders", 0, records)
}

pub fn commit_on(version: u64, name: &str, part: i32, records: u32) -> MetadataEntry {
    MetadataEntry::new(
        CommitVersion::new(version),
        MetadataRecord::BatchCommitted {
            object: ObjectKey::new(format!("obj-{version}")).expect("a valid key"),
            spans: vec![CommittedSpan::new(
                topic(name),
                partition(part),
                records,
                ByteRange::Full,
                None,
            )],
        },
    )
}

/// A commit whose span names a real bounded region, so the byte budget has a
/// length to charge against. ⚠️ `commit`'s `ByteRange::Full` deliberately has
/// none — `Full` names the whole object, whose size only the store knows.
pub fn sized_commit(version: u64, records: u32, bytes: u64) -> MetadataEntry {
    MetadataEntry::new(
        CommitVersion::new(version),
        MetadataRecord::BatchCommitted {
            object: ObjectKey::new(format!("obj-{version}")).expect("a valid key"),
            spans: vec![CommittedSpan::new(
                topic("orders"),
                partition(0),
                records,
                ByteRange::bounded(0, bytes).expect("a non-empty range"),
                None,
            )],
        },
    )
}

/// The cases, generic over the implementation. Each takes a fresh, empty index.
mod contract {
    use super::{commit, commit_on, offset, partition, topic};
    use oqueue_core::{
        CommitVersion, Error, MaterializedIndex, MetadataEntry, MetadataRecord, ObjectKey, Offset,
        TAIL_WINDOW_ENTRIES,
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
                        None,
                    ),
                    CommittedSpan::new(
                        topic("payments"),
                        partition(0),
                        3,
                        ByteRange::bounded(64, 96).expect("a valid region"),
                        None,
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
        let entries = index.entries();
        index
            .apply(&[commit(2, 5), commit(2, 5)])
            .expect_err("refused for its second entry");

        assert_eq!(index.end_offset(&topic("orders"), partition(0)), offset(3));
        assert_eq!(index.applied_upto(), Some(CommitVersion::new(1)));
        // ⚠️ **The count is part of "folds nothing" too** (guarantee 4,
        // `M3.31`). An implementation that pushes an entry and increments
        // before validating the *next* one leaves the count a batch ahead of
        // everything the guarantee does restore, and a caller that retries the
        // refused batch folds onto an index already carrying part of it.
        // ⚠️ **A drop-and-refill *would* correct it** — `clear` zeroes the
        // counter and the replay recounts — so what this pins is the window
        // between the refusal and a rebuild nobody has asked for. `M5`'s quota
        // is what acts on the number inside that window.
        assert_eq!(
            index.entries(),
            entries,
            "a refused apply moved the entry count"
        );
        // ⚠️ **And no `ObjectRef` from the refused batch is *findable*, which
        // is the half a count cannot see** (`M3.33`). `end_offset` and
        // `applied_upto` are scalars: an implementation that eagerly pushed the
        // first entry into its partitions and then rolled back only those two
        // would leave a reference behind that satisfies both, and hand a fetch
        // at the high watermark an object *past* it. That is FR-12's zero-GET
        // claim broken — a read where the index promised none — and a batch a
        // consumer would be served from a commit it was told did not happen.
        let page = index
            .find_batches(&topic("orders"), partition(0), offset(3), 1 << 20)
            .expect("a lookup at the watermark is not an error");
        assert!(
            page.is_empty(),
            "the refused batch left {} findable reference(s) past the watermark",
            page.len()
        );
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
    /// (`ADR-0025`), so a refill has to reproduce it too. ⚠️ **And of
    /// guarantee 4**, which `M3.31` added after finding this suite asserting
    /// the number *exactly* while the trait said an implementation "may return
    /// an estimate" — one of the two had to give, and an estimator would have
    /// failed here rather than at the trait, long after an engine was chosen
    /// on the strength of that promise. An implementation
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

    /// ⚠️ **`ADR-0042`'s read column, and the one thing the index knows about
    /// a manifest.** A partition with no publication answers `None` — a
    /// reader that took `Some` from that would spend a GET on an object no
    /// compaction wrote. After one, it answers the key and how far it covers,
    /// and `find_batches` names nothing below that offset, because the entries
    /// it would have named are what the manifest replaced.
    pub(super) fn a_publication_is_where_the_manifest_is_and_how_far_it_covers<
        I: MaterializedIndex,
    >(
        index: &I,
    ) {
        assert_eq!(
            index.manifest(&topic("orders"), partition(0)),
            None,
            "a partition nothing published for has no manifest"
        );

        let objects = TAIL_WINDOW_ENTRIES + 2;
        let log: Vec<MetadataEntry> = (0..objects)
            .map(|which| {
                commit_on(
                    u64::try_from(which).expect("a small count") + 1,
                    "orders",
                    0,
                    1,
                )
            })
            .collect();
        index.apply(&log).expect("a plain log folds");
        let upto = offset(2);
        index
            .apply(&[MetadataEntry::new(
                CommitVersion::new(u64::try_from(objects).expect("a small count") + 1),
                MetadataRecord::ManifestPublished {
                    topic: topic("orders"),
                    partition: partition(0),
                    manifest: ObjectKey::new("m").expect("a valid key"),
                    upto,
                },
            )])
            .expect("a manifest meeting a boundary is folded");

        assert_eq!(
            index.manifest(&topic("orders"), partition(0)),
            Some((ObjectKey::new("m").expect("a valid key"), upto)),
            "the key it published, and how far it covers"
        );
        assert_eq!(
            index.manifest(&topic("orders"), partition(1)),
            None,
            "and only for the partition it named"
        );
        assert!(
            index
                .find_batches(&topic("orders"), partition(0), offset(0), u64::MAX)
                .expect("a folded partition")
                .iter()
                .all(|batch| batch.reference().base_offset() >= upto),
            "the index names nothing the manifest covers"
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
    contract::a_publication_is_where_the_manifest_is_and_how_far_it_covers(&make());
    crate::paging::a_fetch_at_the_high_watermark_finds_no_batches(&make());
    crate::paging::an_unknown_partition_finds_no_batches(&make());
    crate::paging::a_page_runs_from_start_to_the_end_of_the_log(&make());
    crate::paging::the_byte_budget_bounds_the_page_but_always_yields_one(&make());
    crate::paging::an_unknown_length_charges_nothing(&make());
    crate::paging::a_page_is_bounded_by_its_batch_count(&make());
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
