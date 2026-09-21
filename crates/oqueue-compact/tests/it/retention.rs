//! The retention round: which partitions' data has outlived its retention.
//!
//! ⚠️ **FR-33's own criterion is the first case** — a partition receiving no
//! write for longer than its retention is trimmed on schedule, with nothing
//! arriving to trigger it (`M5.18`).

// A panic in a test harness is the test failing, which is what it is for.
#![allow(clippy::expect_used)]

use oqueue_compact::ExpiryHeap;
use oqueue_core::{
    ByteRange, CommitVersion, CommittedSpan, FakeMaterializedIndex, MaterializedIndex,
    MetadataEntry, MetadataRecord, ObjectKey, Offset, PartitionId, Timestamp, TopicId,
};

use crate::support::{partition_n, topic};

const RETENTION: i64 = 1_000;

fn at(millis: i64) -> Timestamp {
    Timestamp::from_millis(millis).expect("a valid time")
}

fn commit(version: u64, partition: i32, when: i64) -> MetadataEntry {
    MetadataEntry::new(
        CommitVersion::new(version),
        MetadataRecord::BatchCommitted {
            object: ObjectKey::new(format!("p{partition}-v{version}")).expect("a valid key"),
            spans: vec![CommittedSpan::new(
                topic(),
                partition_n(partition),
                10,
                ByteRange::bounded(0, 10).expect("a valid range"),
                None,
            )],
            written_at: at(when),
        },
    )
}

fn catalog(partitions: i32) -> Vec<(TopicId, PartitionId)> {
    (0..partitions).map(|p| (topic(), partition_n(p))).collect()
}

/// ⚠️ **An idle partition is trimmed on schedule, with no write to trigger
/// it.** Its data was last written at 5,000; with a retention of 1,000 it
/// expires at 6,000. A round before that trims nothing, and one at or after
/// trims it to its end — the whole partition, since everything in it is older
/// than the retention.
#[test]
fn an_idle_partition_is_trimmed_on_schedule() {
    let index = FakeMaterializedIndex::new();
    index
        .apply(&[commit(1, 0, 4_000), commit(2, 0, 5_000)])
        .expect("two commits, then silence");
    let mut heap = ExpiryHeap::rebuilt(&index, &catalog(1), RETENTION);

    assert!(
        heap.due(&index, at(5_999)).expect("a round").is_empty(),
        "one millisecond early is not due"
    );
    let trims = heap.due(&index, at(6_000)).expect("a round");
    assert_eq!(
        trims,
        vec![MetadataRecord::Trimmed {
            topic: topic(),
            partition: partition_n(0),
            start: Offset::new(20).expect("a valid offset"),
        }],
        "trimmed to its end, with nothing written to trigger it"
    );
}

/// ⚠️ **A round over 100,000 partitions with three expiring touches three**
/// (`ADR-0036`'s reason for a heap). Counted as what comes out, because what a
/// round costs is what it pops: every other partition's deadline is still in
/// the future and the heap stops at the first of them.
#[test]
fn a_round_over_many_partitions_touches_only_the_expiring_ones() {
    let partitions = 100_000;
    let expiring = [17, 42_424, 99_999];
    let index = FakeMaterializedIndex::new();
    let entries: Vec<MetadataEntry> = (0..partitions)
        .map(|p| {
            let when = if expiring.contains(&p) { 1_000 } else { 50_000 };
            commit(u64::try_from(p).expect("a small count") + 1, p, when)
        })
        .collect();
    index.apply(&entries).expect("one commit per partition");
    let mut heap = ExpiryHeap::rebuilt(&index, &catalog(partitions), RETENTION);

    let trims = heap.due(&index, at(2_000)).expect("a round");
    let mut trimmed: Vec<i32> = trims
        .iter()
        .filter_map(|record| match record {
            MetadataRecord::Trimmed { partition, .. } => Some(partition.get()),
            _ => None,
        })
        .collect();
    trimmed.sort_unstable();
    assert_eq!(trimmed, expiring.to_vec());
    // ⚠️ **What it popped, not only what it emitted.** A round that popped
    // and re-pushed every entry would leave the heap the same length and emit
    // the same three trims; the pop count is what separates them.
    assert_eq!(heap.popped_in_last_round(), expiring.len(), "three pops");
    assert_eq!(
        heap.deadlines().len(),
        usize::try_from(partitions).expect("a positive count") - expiring.len(),
        "and every other partition is still waiting, untouched"
    );
}

/// ⚠️ **A rebuilt heap has the deadlines of the one it replaces.** It is
/// derived state: a coordinator that restarts rebuilds it from the index, and
/// a rebuild that disagreed would trim a partition early or keep one forever.
#[test]
fn a_rebuilt_heap_produces_the_same_deadlines() {
    let index = FakeMaterializedIndex::new();
    index
        .apply(&[
            commit(1, 0, 1_000),
            commit(2, 1, 3_000),
            commit(3, 2, 2_000),
            commit(4, 1, 9_000),
        ])
        .expect("three partitions");
    let mut live = ExpiryHeap::new(RETENTION);
    for (t, p) in catalog(3) {
        live.track(&index, &t, p);
    }
    let rebuilt = ExpiryHeap::rebuilt(&index, &catalog(3), RETENTION);
    assert_eq!(live.deadlines(), rebuilt.deadlines());
    assert!(live.due(&index, at(0)).expect("a round").is_empty());
}

/// A changed topic replaces only its own ordered entry. The unrelated topic
/// remains armed, and the set still contains exactly one entry per partition.
#[test]
fn changing_topic_retention_keeps_the_heap_bounded() {
    let other = TopicId::new("payments").expect("a topic");
    let index = FakeMaterializedIndex::new();
    index
        .apply(&[
            commit(1, 0, 1_000),
            MetadataEntry::new(
                CommitVersion::new(2),
                MetadataRecord::BatchCommitted {
                    object: ObjectKey::new("payments-v2").expect("a key"),
                    spans: vec![CommittedSpan::new(
                        other.clone(),
                        partition_n(0),
                        10,
                        ByteRange::bounded(0, 10).expect("a range"),
                        None,
                    )],
                    written_at: at(2_000),
                },
            ),
        ])
        .expect("commits");
    let mut heap = ExpiryHeap::rebuilt(
        &index,
        &[(topic(), partition_n(0)), (other.clone(), partition_n(0))],
        RETENTION,
    );

    heap.set_topic_retention(&index, &topic(), Some(100));

    assert_eq!(heap.len(), 2, "one ordered entry per armed partition");
    assert_eq!(
        heap.deadlines(),
        vec![
            (1_100, topic(), partition_n(0)),
            (3_000, other, partition_n(0)),
        ]
    );
}

/// ⚠️ **A write after the heap was built moves the deadline, and the stale
/// entry does not trim.** The heap is re-checked against the index when an
/// entry pops; believed instead, it would reap data acknowledged a moment ago.
#[test]
fn a_partition_written_to_after_it_was_tracked_is_not_trimmed_early() {
    let index = FakeMaterializedIndex::new();
    index.apply(&[commit(1, 0, 1_000)]).expect("an old commit");
    let mut heap = ExpiryHeap::rebuilt(&index, &catalog(1), RETENTION);
    index
        .apply(&[commit(2, 0, 1_900)])
        .expect("a write just before its old deadline");

    assert!(
        heap.due(&index, at(2_000)).expect("a round").is_empty(),
        "the old deadline has passed, but the partition was written to since"
    );
    assert_eq!(
        heap.due(&index, at(2_900)).expect("a round").len(),
        1,
        "and it expires at its new deadline instead"
    );
}

/// ⚠️ **Data stamped at the epoch is not judged** (`M5.86`'s second round).
/// A host clock before 1970 stamps every commit at `EPOCH`; treating that as
/// ancient would trim freshly acknowledged records on the first round.
#[test]
fn a_partition_stamped_at_the_epoch_is_not_trimmed() {
    let index = FakeMaterializedIndex::new();
    index
        .apply(&[commit(1, 0, 0)])
        .expect("a commit from a clock before 1970");
    let mut heap = ExpiryHeap::rebuilt(&index, &catalog(1), RETENTION);
    assert!(
        heap.deadlines().is_empty(),
        "no deadline it could honestly set"
    );
    assert!(
        heap.due(&index, at(i64::MAX / 2))
            .expect("a round")
            .is_empty()
    );
}

/// A partition already trimmed to its end yields nothing more.
#[test]
fn an_already_trimmed_partition_is_not_trimmed_again() {
    let index = FakeMaterializedIndex::new();
    index.apply(&[commit(1, 0, 1_000)]).expect("a commit");
    index
        .apply(&[MetadataEntry::new(
            CommitVersion::new(2),
            MetadataRecord::Trimmed {
                topic: topic(),
                partition: partition_n(0),
                start: Offset::new(10).expect("a valid offset"),
            },
        )])
        .expect("already trimmed to its end");
    let mut heap = ExpiryHeap::rebuilt(&index, &catalog(1), RETENTION);
    assert!(heap.due(&index, at(5_000)).expect("a round").is_empty());
}

/// ⚠️ **Trimmed, written again, gone idle: trimmed again.** A trimmed
/// partition leaves the heap, and the caller re-arms it after each commit.
/// Without that, a partition trimmed once would keep every later write
/// forever — FR-33's idle case, failed after the first time. Found by
/// `M5.18`'s first round.
#[test]
fn a_partition_trimmed_then_written_again_expires_again() {
    let index = FakeMaterializedIndex::new();
    index.apply(&[commit(1, 0, 1_000)]).expect("an old commit");
    let mut heap = ExpiryHeap::rebuilt(&index, &catalog(1), RETENTION);
    let first = heap.due(&index, at(2_000)).expect("a round");
    index
        .apply(&[MetadataEntry::new(CommitVersion::new(2), first[0].clone())])
        .expect("the trim, journaled");

    index.apply(&[commit(3, 0, 3_000)]).expect("written again");
    heap.track(&index, &topic(), partition_n(0));

    assert!(heap.due(&index, at(3_999)).expect("a round").is_empty());
    assert_eq!(
        heap.due(&index, at(4_000)).expect("a round"),
        vec![MetadataRecord::Trimmed {
            topic: topic(),
            partition: partition_n(0),
            start: Offset::new(20).expect("a valid offset"),
        }],
        "and trimmed again once the new data has aged out"
    );
}

/// ⚠️ **Tracking on every commit neither grows the heap nor doubles a trim.**
/// One live deadline per partition: each `track` supersedes the last, and a
/// superseded entry is dropped when it pops.
#[test]
fn tracking_every_commit_keeps_one_deadline_and_one_trim() {
    let index = FakeMaterializedIndex::new();
    let mut heap = ExpiryHeap::new(RETENTION);
    assert!(heap.is_empty(), "a new heap holds nothing");
    for (version, when) in [(1, 1_000), (2, 1_200), (3, 1_400)] {
        index.apply(&[commit(version, 0, when)]).expect("a commit");
        heap.track(&index, &topic(), partition_n(0));
    }
    // ⚠️ **The heap itself, not the armed map.** `deadlines()` reads one entry
    // per partition whatever the heap holds, so asserting on it passed while
    // every later commit left a stale entry behind. Found by `M5.18`'s second
    // round.
    assert_eq!(heap.len(), 1, "one heap entry, not one per commit");
    assert!(
        heap.due(&index, at(2_000)).expect("a round").is_empty(),
        "the first commit's deadline has passed, but the last one's has not"
    );
    assert_eq!(
        heap.len(),
        1,
        "re-armed at the later deadline, still one entry"
    );
    assert!(!heap.is_empty());
    assert_eq!(
        heap.due(&index, at(2_400)).expect("a round").len(),
        1,
        "and one trim when the newest commit ages out"
    );
    assert!(heap.is_empty(), "a trimmed partition leaves the heap");
}

/// The heap's size is one per armed partition, however many there are.
#[test]
fn the_heap_holds_one_entry_per_armed_partition() {
    let index = FakeMaterializedIndex::new();
    index
        .apply(&[
            commit(1, 0, 1_000),
            commit(2, 1, 1_000),
            commit(3, 2, 1_000),
        ])
        .expect("three partitions");
    let heap = ExpiryHeap::rebuilt(&index, &catalog(3), RETENTION);
    assert_eq!(heap.len(), 3);
}
