//! Compaction over a partition retention has trimmed.
//!
//! ⚠️ **Split from `sweep.rs` at the 500-line limit, along the concept**
//! (`code-structure.md` rule 18). A trim moves a partition's log start, and
//! the index then refuses a read below it (`M5.19`) — which every walk
//! compaction starts from a caller's cursor has to survive rather than fail
//! the round over.

#![allow(clippy::expect_used)]
#![allow(clippy::redundant_pub_crate)]

use oqueue_compact::{PlannedInputs, sweep};
use oqueue_core::{
    ByteRange, CommitVersion, CommittedSpan, FakeMaterializedIndex, MaterializedIndex,
    MetadataEntry, MetadataRecord, ObjectKey, Offset, Timestamp,
};

use crate::support::{partition_n, topic};
use crate::sweep::{cluster, every_partition};

/// Adds `extra` one-record objects to `partition`, from commit version `from`.
pub(crate) fn grow(index: &FakeMaterializedIndex, partition: i32, extra: u64, from: u64) {
    let more: Vec<MetadataEntry> = (0..extra)
        .map(|which| {
            MetadataEntry::new(
                CommitVersion::new(from + which),
                MetadataRecord::BatchCommitted {
                    object: ObjectKey::new(format!("p{partition}-more-{which}"))
                        .expect("a valid key"),
                    spans: vec![CommittedSpan::new(
                        topic(),
                        partition_n(partition),
                        1,
                        ByteRange::bounded(0, 1).expect("a valid range"),
                        None,
                    )],
                    written_at: Timestamp::EPOCH,
                },
            )
        })
        .collect();
    index.apply(&more).expect("the partition grows");
}

/// ⚠️ **A sweep from zero over a trimmed partition plans from the log start**
/// (`M5.19`). The index refuses a read below a partition's start, and a
/// sweep's default cursor is `Offset::ZERO` — so before the survey learned to
/// begin at the start, a sweep over one trimmed partition failed outright and
/// took every other candidate in its round with it. Found by `M5.19`'s first
/// round.
#[test]
fn a_sweep_over_a_trimmed_partition_plans_from_the_log_start() {
    let index = cluster(3, &[0, 2]);
    // ⚠️ **Thirty more for partition 0**, so it is still amplified once the
    // trim takes five: `cluster`'s thirteen history objects less five is
    // eight, under the threshold of twelve, and the partition would then drop
    // out of the round for the right reason — which is not what this asserts.
    grow(&index, 0, 30, 500);
    let trimmed_to = 5;
    index
        .apply(&[MetadataEntry::new(
            CommitVersion::new(1_000),
            MetadataRecord::Trimmed {
                topic: topic(),
                partition: partition_n(0),
                start: Offset::new(trimmed_to).expect("a valid offset"),
            },
        )])
        .expect("a trim inside partition 0's history");

    let swept =
        sweep(&index, &every_partition(3)).expect("a trimmed partition does not fail the round");
    let planned: Vec<i32> = swept
        .round()
        .iter()
        .map(|plan| plan.plan().partition().get())
        .collect();
    assert_eq!(
        planned,
        vec![0, 2],
        "both amplified partitions, the trimmed one included"
    );

    let trimmed = &swept.round()[0];
    assert!(
        trimmed.plan().start() >= Offset::new(trimmed_to).expect("a valid offset"),
        "and its plan begins at or after the log start, not over trimmed records: {:?}",
        trimmed.plan().start()
    );
    assert!(
        trimmed
            .inputs()
            .iter()
            .all(|input| input.base_offset().get() >= trimmed_to),
        "no input holds only records the trim removed"
    );
}

/// ⚠️ **A trim that lands between planning and derivation.** The plan was
/// measured before the trim, so its start is below the log start by the time
/// its inputs are derived, and the index refuses that read. The derivation
/// resumes from the start the refusal names rather than failing the round —
/// and the inputs it derives hold no object the trim wholly removed.
#[test]
fn a_trim_between_plan_and_derivation_resumes_at_the_log_start() {
    let index = cluster(1, &[0]);
    grow(&index, 0, 30, 500);
    let swept = sweep(&index, &every_partition(1)).expect("an index that answers");
    let planned = swept.round()[0].plan().clone();
    assert_eq!(planned.start(), Offset::ZERO, "planned from the beginning");

    index
        .apply(&[MetadataEntry::new(
            CommitVersion::new(1_000),
            MetadataRecord::Trimmed {
                topic: topic(),
                partition: partition_n(0),
                start: Offset::new(5).expect("a valid offset"),
            },
        )])
        .expect("a trim after the plan");

    let derived = PlannedInputs::derive(&index, planned)
        .expect("the derivation resumes at the log start rather than failing");
    assert!(
        derived
            .inputs()
            .iter()
            .all(|input| input.base_offset().get() >= 5),
        "no input the trim wholly removed"
    );
    assert!(
        !derived.inputs().is_empty(),
        "and the live history is still derived"
    );
}
