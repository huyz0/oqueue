//! The candidate sweep: what a round is made of, and what it costs to find.

// A panic in a test harness is the test failing, which is what it is for.
#![allow(clippy::expect_used)]

use oqueue_compact::{COMPACTION_PLAN_RECORDS_BUDGET, Candidate, sweep};
use oqueue_core::{
    ByteRange, CommitVersion, CommittedSpan, FakeMaterializedIndex, MAX_BATCHES_PER_PAGE,
    MaterializedIndex, MetadataEntry, MetadataRecord, ObjectKey, Offset, TAIL_WINDOW_ENTRIES,
};

use crate::support::{CountingIndex, partition_n, topic};

/// An index holding `partitions` partitions of one topic, where those named in
/// `amplified` hold `TAIL_WINDOW_ENTRIES + 13` one-record objects and the rest
/// hold one.
fn cluster(partitions: i32, amplified: &[i32]) -> FakeMaterializedIndex {
    let index = FakeMaterializedIndex::new();
    let mut entries = Vec::new();
    let mut version = 0_u64;
    let mut push = |entries: &mut Vec<MetadataEntry>, partition: i32, which: usize| {
        version += 1;
        entries.push(MetadataEntry::new(
            CommitVersion::new(version),
            MetadataRecord::BatchCommitted {
                object: ObjectKey::new(format!("p{partition}-{which}")).expect("a valid key"),
                spans: vec![CommittedSpan::new(
                    topic(),
                    partition_n(partition),
                    1,
                    ByteRange::bounded(0, 1).expect("a valid range"),
                    None,
                )],
            },
        ));
    };
    for partition in 0..partitions {
        if amplified.contains(&partition) {
            for which in 0..(TAIL_WINDOW_ENTRIES + 13) {
                push(&mut entries, partition, which);
            }
        } else {
            push(&mut entries, partition, 0);
        }
    }
    index.apply(&entries).expect("a valid fold");
    index
}

fn every_partition(partitions: i32) -> Vec<Candidate> {
    (0..partitions)
        .map(|index| Candidate::new(topic(), partition_n(index), Offset::ZERO))
        .collect()
}

/// ⚠️ **The sweep's cost scales with partitions and its output does not.**
/// Every partition is looked at; three are worth compacting, so three plans
/// come back — which is `ADR-0036`'s whole argument that the cadence prices a
/// memory scan rather than a PUT per partition.
#[test]
fn a_sweep_over_many_partitions_plans_only_the_amplified_ones() {
    // ⚠️ **100,000, the count `ADR-0036` and doc 14 §7 are written about.**
    // A first version used 10,000 on a cost claim round one measured false:
    // the whole file runs in 0.37 s at 100k against 0.04 s at 10k.
    let partitions = 100_000;
    let amplified = [17, 42_424, 99_999];
    let index = cluster(partitions, &amplified);

    let swept = sweep(&index, &every_partition(partitions)).expect("an index that answers");
    assert_eq!(swept.round().len(), 3, "three amplified partitions");
    let planned: Vec<i32> = swept
        .round()
        .iter()
        .map(|plan| plan.partition().get())
        .collect();
    assert_eq!(planned, amplified.to_vec());
    assert!(swept.held_over().is_empty());
}

/// A partition the index has never folded is not a candidate.
#[test]
fn a_partition_the_index_does_not_know_is_skipped() {
    let index = cluster(4, &[1]);
    let mut candidates = every_partition(4);
    candidates.push(Candidate::new(topic(), partition_n(4_000), Offset::ZERO));
    let swept = sweep(&index, &candidates).expect("an index that answers");
    assert_eq!(swept.round().len(), 1);
}

/// ⚠️ **A round is one object's worth.** What does not fit is next round's,
/// not a smaller plan: trimming here would plan a range nobody measured.
#[test]
fn candidates_over_the_round_budget_are_held_over() {
    let index = FakeMaterializedIndex::new();
    let per = 32_768_u32;
    let objects = 16_usize;
    let mut entries = Vec::new();
    let mut version = 0_u64;
    for partition in 0..2_i32 {
        for which in 0..(objects + TAIL_WINDOW_ENTRIES) {
            version += 1;
            entries.push(MetadataEntry::new(
                CommitVersion::new(version),
                MetadataRecord::BatchCommitted {
                    object: ObjectKey::new(format!("q{partition}-{which}")).expect("a valid key"),
                    spans: vec![CommittedSpan::new(
                        topic(),
                        partition_n(partition),
                        per,
                        ByteRange::bounded(0, u64::from(per)).expect("a valid range"),
                        None,
                    )],
                },
            ));
        }
    }
    index.apply(&entries).expect("a valid fold");

    let swept = sweep(&index, &every_partition(2)).expect("an index that answers");
    assert_eq!(
        swept.round().len(),
        1,
        "one partition fills the round's budget"
    );
    assert_eq!(
        swept.held_over(),
        &[(topic(), partition_n(1))],
        "the other waits for the next round, and says which"
    );
    assert!(
        swept.round()[0].cost().records_rewritten() <= COMPACTION_PLAN_RECORDS_BUDGET,
        "and what is in the round fits it"
    );
}

/// ⚠️ **A partition larger than one plan's budget is swept in windows.**
/// Planning it whole would be over budget on every sweep for ever — measured,
/// before `M5.40` windowed it — so the sweep plans its first window instead.
#[test]
fn a_partition_larger_than_the_budget_is_swept_in_windows() {
    let index = FakeMaterializedIndex::new();
    // Objects small enough to clear the amplification threshold, in a count
    // whose records exceed what one plan may rewrite.
    let per = 15_420_u32;
    let objects = 35_usize;
    let mut entries = Vec::new();
    for (which, version) in (0..(objects + TAIL_WINDOW_ENTRIES)).zip(1_u64..) {
        entries.push(MetadataEntry::new(
            CommitVersion::new(version),
            MetadataRecord::BatchCommitted {
                object: ObjectKey::new(format!("big-{which}")).expect("a valid key"),
                spans: vec![CommittedSpan::new(
                    topic(),
                    partition_n(0),
                    per,
                    ByteRange::bounded(0, u64::from(per)).expect("a valid range"),
                    None,
                )],
            },
        ));
    }
    index.apply(&entries).expect("a valid fold");

    let swept = sweep(&index, &every_partition(1)).expect("an index that answers");
    assert_eq!(swept.round().len(), 1, "the first window is planned");
    assert!(swept.held_over().is_empty());
    let planned = &swept.round()[0];
    assert_eq!(planned.start(), Offset::ZERO);
    assert!(
        planned.cost().records_rewritten() <= COMPACTION_PLAN_RECORDS_BUDGET,
        "and the window fits one plan's budget"
    );
}

/// ⚠️ **The round's remaining budget is what each plan spends from**, so two
/// plans that fit together both run. A round that admitted only the first
/// would compact a tenth of what a round can, at the same PUT.
#[test]
fn two_plans_that_fit_together_are_both_in_the_round() {
    let index = FakeMaterializedIndex::new();
    let per = 3_000_u32;
    let objects = 34_usize;
    let mut entries = Vec::new();
    let mut version = 0_u64;
    for partition in 0..2_i32 {
        for which in 0..(objects + TAIL_WINDOW_ENTRIES) {
            version += 1;
            entries.push(MetadataEntry::new(
                CommitVersion::new(version),
                MetadataRecord::BatchCommitted {
                    object: ObjectKey::new(format!("r{partition}-{which}")).expect("a valid key"),
                    spans: vec![CommittedSpan::new(
                        topic(),
                        partition_n(partition),
                        per,
                        ByteRange::bounded(0, u64::from(per)).expect("a valid range"),
                        None,
                    )],
                },
            ));
        }
    }
    index.apply(&entries).expect("a valid fold");

    let swept = sweep(&index, &every_partition(2)).expect("an index that answers");
    assert_eq!(swept.round().len(), 2, "both fit one round's budget");
    assert!(swept.held_over().is_empty());
    let total: i64 = swept
        .round()
        .iter()
        .map(|plan| plan.cost().records_rewritten())
        .sum();
    assert!(
        total <= COMPACTION_PLAN_RECORDS_BUDGET,
        "and together they are still inside it"
    );
}

/// ⚠️ **A cold catalog costs one lookup per partition, not one scan**, and
/// this asserts the *walks* rather than the answer: `plan` on an unfolded
/// partition returns `NotWorthIt` either way, so a test asserting only the
/// round's length says nothing about what the sweep spent. What holds it is
/// the `from >= end` check in `sweep`, which for an unfolded partition
/// compares a zero cursor against a zero end and skips before planning.
#[test]
fn an_unfolded_partition_costs_no_index_walk() {
    let index = CountingIndex::over(cluster(4, &[1]));
    let mut candidates = every_partition(4);
    for far in 4_000..4_100 {
        candidates.push(Candidate::new(topic(), partition_n(far), Offset::ZERO));
    }

    let swept = sweep(&index, &candidates).expect("an index that answers");
    assert_eq!(swept.round().len(), 1, "one amplified partition");
    assert_eq!(
        index.walks(),
        6,
        "one walk for each of the three one-object partitions and three for \
         the amplified one, whose measurement pages; and none at all for the \
         hundred unfolded ones, which would each add one"
    );
}

/// ⚠️ **A partition named twice would fail the whole round.** `merge_round`
/// refuses an overlapping round outright, so one caller's duplicate would
/// discard every other partition's work — repeatedly.
#[test]
fn a_duplicated_candidate_is_planned_once() {
    let index = cluster(3, &[0, 2]);
    let mut candidates = every_partition(3);
    candidates.extend(every_partition(3));

    let swept = sweep(&index, &candidates).expect("an index that answers");
    assert_eq!(
        swept.round().len(),
        2,
        "two amplified partitions, once each"
    );
}

/// ⚠️ **A partition behind a long compacted prefix is planned past it, because
/// the caller says where compaction reached.** Planning every candidate from
/// offset zero left any partition bigger than one plan's budget unplannable
/// for ever; fixed windows from zero moved the wall to four windows; and
/// deriving the start from the first object smaller than a compacted one
/// anchored permanently on any early window that was merely not worth
/// compacting. The cursor is the caller's, and `M5.44` is where it gets an
/// owner.
#[test]
fn a_partition_behind_a_long_compacted_prefix_is_still_planned() {
    let index = FakeMaterializedIndex::new();
    let mut entries = Vec::new();
    let mut version = 0_u64;
    let mut push = |entries: &mut Vec<MetadataEntry>, name: String, count: u32| {
        version += 1;
        entries.push(MetadataEntry::new(
            CommitVersion::new(version),
            MetadataRecord::BatchCommitted {
                object: ObjectKey::new(name).expect("a valid key"),
                spans: vec![CommittedSpan::new(
                    topic(),
                    partition_n(0),
                    count,
                    ByteRange::bounded(0, u64::from(count)).expect("a valid range"),
                    None,
                )],
            },
        ));
    };
    // ⚠️ **Seventy budget-widths of compacted prefix, not six.** The walk
    // pages at `MAX_BATCHES_PER_PAGE`, so a prefix shorter than one page never
    // reaches the loop's own resume — and a planted regression there would go
    // unobserved, which is the shape round two found twice.
    let compacted = u32::try_from(COMPACTION_PLAN_RECORDS_BUDGET).expect("a small budget");
    let prefix = MAX_BATCHES_PER_PAGE + 6;
    for which in 0..prefix {
        push(&mut entries, format!("done-{which}"), compacted);
    }
    // Then twenty small objects, and a tail window behind them.
    for which in 0..20 {
        push(&mut entries, format!("small-{which}"), 20_000);
    }
    for which in 0..TAIL_WINDOW_ENTRIES {
        push(&mut entries, format!("tail-{which}"), 1);
    }
    index.apply(&entries).expect("a valid fold");

    let from = Offset::new(
        COMPACTION_PLAN_RECORDS_BUDGET * i64::try_from(prefix).expect("a small prefix"),
    )
    .expect("a valid offset");
    let swept = sweep(&index, &[Candidate::new(topic(), partition_n(0), from)])
        .expect("an index that answers");
    assert_eq!(swept.round().len(), 1, "the prefix is skipped, not a wall");
    assert_eq!(
        swept.round()[0].start().get(),
        COMPACTION_PLAN_RECORDS_BUDGET * i64::try_from(prefix).expect("a small prefix"),
        "and the plan starts where the caller said compaction had reached"
    );
}

/// A cursor at or past the partition's end has nothing to look at, and costs
/// no walk to find that out.
#[test]
fn a_cursor_at_the_end_of_the_partition_plans_nothing() {
    let index = CountingIndex::over(cluster(2, &[0, 1]));
    let end = index.end_offset(&topic(), partition_n(0));
    let swept = sweep(
        &index,
        &[
            Candidate::new(topic(), partition_n(0), end),
            Candidate::new(topic(), partition_n(1), Offset::ZERO),
        ],
    )
    .expect("an index that answers");
    assert_eq!(swept.round().len(), 1, "only the partition with work left");
    assert_eq!(
        swept.round()[0].partition(),
        partition_n(1),
        "and it is the one whose cursor is behind its end"
    );
}
