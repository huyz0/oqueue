//! The ceiling the fold refuses to cross, and what it does not do.
//!
//! ⚠️ **Nothing here evicts**, and every case is written to catch a version
//! that did: `ADR-0043` decision 3, and `M3.11`'s reason before it — entries
//! dropped to stay under a ceiling are entries a replay reproduces and drops
//! again, so the degraded mode's own recovery would be a loop that cannot
//! converge. A refusal leaves a log that a rebuild reaches identically, which
//! is the last case in this file.

// A panic in a test harness is the test failing, which is what it is for.
#![allow(clippy::expect_used)]

use oqueue_core::Timestamp;
use oqueue_core::{
    ByteRange, CommitVersion, CommittedSpan, Error, IndexQuota, IndexState, MetadataEntry,
    MetadataRecord, Offset, PartitionId, Pressure, TAIL_WINDOW_ENTRIES, TopicId,
};

use crate::range_compacted::{key, partition, topic};

fn other_topic() -> TopicId {
    TopicId::new("u").expect("a valid topic")
}

fn commit_to(version: u64, which: usize, topic: TopicId, partition: PartitionId) -> MetadataEntry {
    MetadataEntry::new(
        CommitVersion::new(version),
        MetadataRecord::BatchCommitted {
            object: key(&format!("obj-{which}")),
            spans: vec![CommittedSpan::new(
                topic,
                partition,
                1,
                ByteRange::bounded(0, 32).expect("a valid range"),
                None,
            )],
            written_at: Timestamp::EPOCH,
        },
    )
}

fn commit(version: u64, which: usize) -> MetadataEntry {
    commit_to(version, which, topic(), partition())
}

fn quota(ceiling: usize, alarm_at: usize) -> IndexQuota {
    IndexQuota::new(ceiling, alarm_at).expect("an alarm below a non-zero ceiling")
}

/// ⚠️ **An alarm at the ceiling is not an alarm, it is the incident.**
#[test]
fn a_quota_needs_room_between_its_alarm_and_its_ceiling() {
    assert!(matches!(
        IndexQuota::new(4, 4),
        Err(Error::InvalidIndexQuota {
            ceiling: 4,
            alarm_at: 4
        })
    ));
    assert!(matches!(
        IndexQuota::new(0, 0),
        Err(Error::InvalidIndexQuota { .. })
    ));
    let held = quota(4, 3);
    assert_eq!((held.ceiling(), held.alarm_at()), (4, 3));
    assert!(!held.alarmed(2), "under the alarm");
    assert!(held.alarmed(3), "at it");
    assert!(
        !held.would_cross(4),
        "a fold landing on the ceiling is allowed"
    );
    assert!(held.would_cross(5));
}

/// The quota a state was built with is the one it reports.
#[test]
fn a_state_reports_the_quota_it_folds_under() {
    let index = IndexState::with_quota(quota(9, 7));
    assert_eq!(index.quota(), Some(quota(9, 7)));
}

/// ⚠️ **A batch that shrinks the index can never cross a ceiling**, and the
/// arithmetic has to be a difference for that to hold: a publication absorbs
/// history into one reference, so its contribution is the copy's count *minus*
/// the live one. Summed rather than subtracted, a partition that went from
/// three entries to two would read as five and refuse a fold that frees space.
#[test]
fn a_publication_that_frees_space_is_not_refused() {
    // Past the window, so four references have fallen into history where a
    // manifest can absorb them; a manifest may not reach into the tail.
    let objects = TAIL_WINDOW_ENTRIES + 4;
    let mut index = IndexState::with_quota(quota(objects, objects - 1));
    let log: Vec<MetadataEntry> = (0..objects)
        .map(|which| commit(u64::try_from(which).expect("a small count") + 1, which))
        .collect();
    index.apply(&log).expect("exactly the ceiling");
    assert_eq!(index.entries(), objects);
    assert_eq!(index.tiers().history, 4);

    index
        .apply(&[MetadataEntry::new(
            CommitVersion::new(u64::try_from(objects).expect("a small count") + 1),
            MetadataRecord::ManifestPublished {
                topic: topic(),
                partition: partition(),
                manifest: key("manifest"),
                upto: Offset::new(4).expect("a valid offset"),
            },
        )])
        .expect("absorbing four references into one frees three entries");
    assert_eq!(
        index.entries(),
        TAIL_WINDOW_ENTRIES + 1,
        "four history entries became one manifest reference"
    );
}

/// A fold landing exactly on the ceiling is allowed; the next one is not.
#[test]
fn the_ceiling_is_a_count_the_index_may_hold() {
    let mut index = IndexState::with_quota(quota(3, 2));
    index
        .apply(&[commit(1, 0), commit(2, 1), commit(3, 2)])
        .expect("three entries under a ceiling of three");
    assert_eq!(index.entries(), 3);

    let refused = index
        .apply(&[commit(4, 3)])
        .expect_err("the fourth crosses");
    assert!(matches!(
        refused,
        Error::IndexQuotaExceeded {
            would_be: 4,
            ceiling: 3,
            ..
        }
    ));
}

/// ⚠️ **Guarantee 2, and the part of it this task is about**: the refused
/// batch leaves the index exactly as it found it — not one entry fewer, which
/// is what an evicting version would do, and not one more.
#[test]
fn a_refused_fold_changes_nothing() {
    let mut index = IndexState::with_quota(quota(2, 1));
    index
        .apply(&[commit(1, 0), commit(2, 1)])
        .expect("two entries under a ceiling of two");
    let before = index.tiers();
    let end = index.end_offset(&topic(), partition());

    index.apply(&[commit(3, 2)]).expect_err("the third crosses");

    assert_eq!(
        index.tiers(),
        before,
        "no entry was added, and none dropped"
    );
    assert_eq!(index.end_offset(&topic(), partition()), end);
}

/// ⚠️ **The refusal names a partition, because a total cannot be acted on** —
/// and it names the batch's largest contributor rather than whichever the hash
/// map yielded first, so two runs report one incident rather than two.
#[test]
fn the_refusal_names_the_largest_contributor() {
    let mut index = IndexState::with_quota(quota(4, 3));
    let small = PartitionId::new(1).expect("a valid partition");
    let batch = vec![
        commit_to(1, 0, other_topic(), small),
        commit_to(2, 1, topic(), partition()),
        commit_to(3, 2, topic(), partition()),
        commit_to(4, 3, topic(), partition()),
        commit_to(5, 4, topic(), partition()),
    ];

    let refused = index.apply(&batch).expect_err("five entries over four");
    let Error::IndexQuotaExceeded {
        topic: named,
        partition: named_partition,
        would_be,
        ceiling,
    } = refused
    else {
        panic!("a quota refusal");
    };
    assert_eq!(
        named, "t",
        "the topic that contributed four, not the one that contributed one"
    );
    assert_eq!(named_partition, partition().get());
    assert_eq!((would_be, ceiling), (5, 4));
}

/// The alarm is readable while the index is still folding.
#[test]
fn the_alarm_fires_before_the_refusal() {
    let mut index = IndexState::with_quota(quota(3, 2));
    index.apply(&[commit(1, 0)]).expect("one entry");
    assert_eq!(index.pressure(), Pressure::Nominal);

    index.apply(&[commit(2, 1)]).expect("a second entry");
    assert_eq!(
        index.pressure(),
        Pressure::Alarmed,
        "at the alarm, and still accepting folds"
    );

    index
        .apply(&[commit(3, 2)])
        .expect("a third, still under the ceiling");
    assert_eq!(index.pressure(), Pressure::Alarmed);
}

/// ⚠️ **No quota is no ceiling**, which is what every existing caller relies
/// on: this type is the fold every materialization shares, and a default
/// ceiling would put an arbitrary number in the path of callers with no memory
/// to run out of.
#[test]
fn a_state_without_a_quota_folds_whatever_the_log_holds() {
    let mut index = IndexState::new();
    assert_eq!(index.quota(), None);
    let log: Vec<MetadataEntry> = (0..64)
        .map(|which| commit(u64::try_from(which).expect("a small count") + 1, which))
        .collect();
    index.apply(&log).expect("no ceiling to cross");
    assert_eq!(index.entries(), 64);
    assert_eq!(index.pressure(), Pressure::Nominal);
}

/// ⚠️ **The case the whole design is for** (`ADR-0043` decision 3). A rebuild
/// replays the same log against a fresh state and reaches the same place: the
/// same entries folded, the same batch refused, the same count held. An
/// evicting degraded mode is what cannot do this — it sheds entries, the
/// replay reproduces them, and the shedding starts again.
#[test]
fn a_rebuild_replaying_the_same_log_converges_rather_than_re_entering() {
    let accepted: Vec<MetadataEntry> = (0..3)
        .map(|which| commit(u64::try_from(which).expect("a small count") + 1, which))
        .collect();
    let crossing = vec![commit(4, 3)];

    let mut live = IndexState::with_quota(quota(3, 2));
    live.apply(&accepted).expect("three under the ceiling");
    live.apply(&crossing).expect_err("the fourth crosses");

    // The rebuild: a fresh state, the same quota, the same log in the same
    // order — including the batch that was refused, because the log is what a
    // replay reads and a refusal writes nothing to it.
    let mut rebuilt = IndexState::with_quota(quota(3, 2));
    rebuilt.apply(&accepted).expect("three under the ceiling");
    rebuilt
        .apply(&crossing)
        .expect_err("the fourth crosses again");

    assert_eq!(
        rebuilt.tiers(),
        live.tiers(),
        "the same entries, in the same tiers"
    );
    assert_eq!(
        rebuilt.end_offset(&topic(), partition()),
        live.end_offset(&topic(), partition())
    );
    assert_eq!(rebuilt.pressure(), live.pressure());
}

/// ⚠️ **A batch that both frees and grows is counted by its net**, and the net
/// is a difference — not a ratio, and not a sum. A publication absorbing four
/// references while six commits arrive on the same partition leaves it three
/// entries larger; a ceiling three above where it started is crossed by
/// exactly that batch and by nothing smaller.
#[test]
fn a_batch_that_frees_and_grows_is_refused_on_its_net() {
    let objects = TAIL_WINDOW_ENTRIES + 4;
    let ceiling = objects + 2;
    let mut index = IndexState::with_quota(quota(ceiling, ceiling - 1));
    let log: Vec<MetadataEntry> = (0..objects)
        .map(|which| commit(u64::try_from(which).expect("a small count") + 1, which))
        .collect();
    index.apply(&log).expect("four under the ceiling");
    let before = index.tiers();

    let next = u64::try_from(objects).expect("a small count") + 1;
    let mut batch = vec![MetadataEntry::new(
        CommitVersion::new(next),
        MetadataRecord::ManifestPublished {
            topic: topic(),
            partition: partition(),
            manifest: key("manifest"),
            upto: Offset::new(4).expect("a valid offset"),
        },
    )];
    batch.extend((0..6usize).map(|which| {
        commit(
            next + 1 + u64::try_from(which).expect("a small count"),
            objects + which,
        )
    }));

    let refused = index
        .apply(&batch)
        .expect_err("absorbing four and adding six is three more than the ceiling allows");
    assert!(
        matches!(refused, Error::IndexQuotaExceeded { would_be, .. } if would_be == objects + 3),
        "the net of the whole batch, not the growth alone: {refused:?}"
    );
    assert_eq!(index.tiers(), before, "and nothing was applied");
}

/// ⚠️ **A tie is broken by name, so it is broken the same way twice.** Two
/// partitions contributing equally is the ordinary case for a bundled object,
/// and a refusal that named whichever the hash map yielded first would read as
/// two incidents across two runs of the same log.
#[test]
fn a_tie_is_broken_by_name_rather_than_by_iteration_order() {
    let mut index = IndexState::with_quota(quota(3, 2));
    let batch = vec![
        commit_to(1, 0, other_topic(), partition()),
        commit_to(2, 1, topic(), partition()),
        commit_to(3, 2, other_topic(), partition()),
        commit_to(4, 3, topic(), partition()),
    ];

    for _ in 0..8 {
        let mut fresh = IndexState::with_quota(quota(3, 2));
        let refused = fresh.apply(&batch).expect_err("four entries over three");
        let Error::IndexQuotaExceeded { topic: named, .. } = refused else {
            panic!("a quota refusal");
        };
        assert_eq!(named, "t", "the earlier name, every time");
    }

    let refused = index.apply(&batch).expect_err("four entries over three");
    assert!(matches!(refused, Error::IndexQuotaExceeded { .. }));
}

/// A fold that crosses the alarm and not the ceiling is accepted, and says so.
#[test]
fn a_fold_that_only_crosses_the_alarm_is_accepted() {
    let mut index = IndexState::with_quota(quota(4, 2));
    index
        .apply(&[commit(1, 0), commit(2, 1), commit(3, 2)])
        .expect("three, past the alarm and under the ceiling");
    assert_eq!(index.pressure(), Pressure::Alarmed);
    assert_eq!(index.entries(), 3, "and every entry was folded");
}

/// ⚠️ **And a tie on the name is broken by the partition**, which is the case
/// a bundled object actually produces: one object contributing equally to two
/// partitions of the same topic (FR-32). The name arm cannot separate those.
#[test]
fn a_tie_within_one_topic_is_broken_by_partition() {
    let second = PartitionId::new(1).expect("a valid partition");
    let batch = vec![
        commit_to(1, 0, topic(), second),
        commit_to(2, 1, topic(), partition()),
        commit_to(3, 2, topic(), second),
        commit_to(4, 3, topic(), partition()),
    ];

    for _ in 0..8 {
        let mut fresh = IndexState::with_quota(quota(3, 2));
        let refused = fresh.apply(&batch).expect_err("four entries over three");
        let Error::IndexQuotaExceeded {
            partition: named, ..
        } = refused
        else {
            panic!("a quota refusal");
        };
        assert_eq!(named, partition().get(), "the lower partition, every time");
    }
}
