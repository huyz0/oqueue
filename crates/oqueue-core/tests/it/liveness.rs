//! Which objects the index still names (`ADR-0045`, `M5.20`).
//!
//! ⚠️ **Every case asks one question: is this object still reachable?** A
//! count that reaches zero too early is a deleted byte a reader was promised
//! (FR-35); one that never reaches zero is only storage, which is why
//! publication is allowed to leak and nothing else is.

#![allow(clippy::expect_used)]

use crate::range_compacted::{key, loaded, offset, partition, reference, swap, topic};
use oqueue_core::{
    ByteRange, CommitVersion, CommittedSpan, IndexState, MetadataEntry, MetadataRecord,
    PartitionId, TAIL_WINDOW_ENTRIES, Timestamp,
};

fn version(n: usize) -> u64 {
    u64::try_from(n).expect("a small count")
}

fn trim(at: u64, start: i64) -> MetadataEntry {
    MetadataEntry::new(
        CommitVersion::new(at),
        MetadataRecord::Trimmed {
            topic: topic(),
            partition: partition(),
            start: offset(start),
        },
    )
}

/// ⚠️ **One live slice keeps the whole object**: an object holding ten
/// partitions' data, nine of them trimmed away, is still named once and must
/// not be deletable — the case FR-35 is about, since the dead nine are what
/// makes it look collectable.
#[test]
fn an_object_with_one_live_slice_is_still_referenced() {
    let span = |p: i32| {
        CommittedSpan::new(
            topic(),
            PartitionId::new(p).expect("a valid partition"),
            1,
            ByteRange::Full,
            None,
        )
    };
    let mut index = IndexState::default();
    index
        .apply(&[MetadataEntry::new(
            CommitVersion::new(1),
            MetadataRecord::BatchCommitted {
                object: key("shared"),
                spans: (0..10).map(span).collect(),
                written_at: Timestamp::EPOCH,
            },
        )])
        .expect("one object, ten partitions");
    assert_eq!(index.references(&key("shared")), 10);

    let trims: Vec<MetadataEntry> = (0..9_i32)
        .map(|p| {
            MetadataEntry::new(
                CommitVersion::new(u64::try_from(p).expect("small") + 2),
                MetadataRecord::Trimmed {
                    topic: topic(),
                    partition: PartitionId::new(p).expect("a valid partition"),
                    start: offset(1),
                },
            )
        })
        .collect();
    index.apply(&trims).expect("nine partitions trimmed");
    assert_eq!(index.references(&key("shared")), 1, "one slice is live");
}

#[test]
fn a_trim_releases_what_it_drops_and_nothing_else() {
    let mut index = loaded(3);
    index.apply(&[trim(4, 2)]).expect("a trim past two objects");
    assert_eq!(index.references(&key("obj-0")), 0);
    assert_eq!(index.references(&key("obj-1")), 0);
    assert_eq!(index.references(&key("obj-2")), 1);
}

/// A commit and the trim that drops it, in one batch, net to nothing — the
/// order the count applies additions and drops in is what makes that so.
#[test]
fn a_commit_and_its_trim_in_one_batch_net_to_zero() {
    let mut index = IndexState::default();
    let commit = MetadataEntry::new(
        CommitVersion::new(1),
        MetadataRecord::BatchCommitted {
            object: key("brief"),
            spans: vec![CommittedSpan::new(
                topic(),
                partition(),
                1,
                ByteRange::Full,
                None,
            )],
            written_at: Timestamp::EPOCH,
        },
    );
    index.apply(&[commit, trim(2, 1)]).expect("both fold");
    assert_eq!(index.references(&key("brief")), 0);
}

#[test]
fn a_swap_releases_the_retired_and_holds_the_installed() {
    let objects = TAIL_WINDOW_ENTRIES + 2;
    let mut index = loaded(objects);
    let retiring = vec![reference("obj-0", 0, 1), reference("obj-1", 1, 1)];
    index
        .apply(&[swap(
            version(objects) + 1,
            retiring,
            vec![reference("merged", 0, 2)],
        )])
        .expect("a covering swap");
    assert_eq!(index.references(&key("obj-0")), 0);
    assert_eq!(index.references(&key("obj-1")), 0);
    assert_eq!(index.references(&key("merged")), 1);
}

/// A refused batch changes no count: guarantee 2 covers this state too.
#[test]
fn a_refused_swap_releases_nothing() {
    let objects = TAIL_WINDOW_ENTRIES + 2;
    let mut index = loaded(objects);
    let refused = index.apply(&[swap(
        version(objects) + 1,
        vec![reference("obj-0", 0, 1)],
        vec![reference("merged", 0, 2)],
    )]);
    assert!(refused.is_err(), "installing more than it retires");
    assert_eq!(index.references(&key("obj-0")), 1);
    assert_eq!(index.references(&key("merged")), 0);
}

/// ⚠️ **Publication never releases**, deliberately (`ADR-0045`): the manifest
/// still names what it absorbed, and this fold cannot see inside it.
#[test]
fn publication_never_releases_what_the_manifest_absorbed() {
    let objects = TAIL_WINDOW_ENTRIES + 2;
    let mut index = loaded(objects);
    index
        .apply(&[MetadataEntry::new(
            CommitVersion::new(version(objects) + 1),
            MetadataRecord::ManifestPublished {
                topic: topic(),
                partition: partition(),
                manifest: key("manifest"),
                upto: offset(2),
            },
        )])
        .expect("a manifest over all of history");
    assert_eq!(index.references(&key("obj-0")), 1);
    assert_eq!(index.references(&key("obj-1")), 1);
}

#[test]
fn clearing_forgets_every_count() {
    let mut index = loaded(2);
    index.clear();
    assert_eq!(index.references(&key("obj-0")), 0);
}

/// A trim reaching into history releases what it drops there and keeps what
/// it does not — the history half of the count, which the tail cases above
/// never reach.
#[test]
fn a_trim_into_history_releases_only_what_it_drops() {
    let objects = TAIL_WINDOW_ENTRIES + 2;
    let mut index = loaded(objects);
    index
        .apply(&[trim(version(objects) + 1, 1)])
        .expect("a trim past the first history object");
    assert_eq!(index.references(&key("obj-0")), 0);
    assert_eq!(index.references(&key("obj-1")), 1, "still in history");
}

/// The fake answers through the seam what the fold answers directly.
#[test]
fn the_fake_answers_the_fold_s_count() {
    use oqueue_core::{FakeMaterializedIndex, MaterializedIndex};
    let fake = FakeMaterializedIndex::new();
    let log: Vec<MetadataEntry> = (0..2_u64)
        .map(|n| {
            MetadataEntry::new(
                CommitVersion::new(n + 1),
                MetadataRecord::BatchCommitted {
                    object: key("twice"),
                    spans: vec![CommittedSpan::new(
                        topic(),
                        partition(),
                        1,
                        ByteRange::Full,
                        None,
                    )],
                    written_at: Timestamp::EPOCH,
                },
            )
        })
        .collect();
    fake.apply(&log).expect("two commits of one object");
    assert_eq!(fake.references(&key("twice")), 2);
    assert_eq!(fake.references(&key("absent")), 0);
}
