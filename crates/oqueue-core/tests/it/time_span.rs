//! A partition's age, folded from the log.
//!
//! ⚠️ **What FR-33's decision reads** (`M5.86`). A retention round asks every
//! partition a node holds how old its data is, so the answer has to come out
//! of the index rather than out of an object — and it has to be the *same*
//! answer whichever way the log reached the fold, because a replaying
//! coordinator and a live one must agree about what to delete.

// A panic in a test harness is the test failing, which is what it is for.
#![allow(clippy::expect_used)]

use oqueue_core::{
    ByteRange, CommitVersion, CommittedSpan, IndexState, MetadataEntry, MetadataRecord, Timestamp,
};

use crate::range_compacted::{key, offset, partition, topic};

fn at(millis: i64) -> Timestamp {
    Timestamp::from_millis(millis).expect("a valid timestamp")
}

fn commit(version: u64, which: usize, when: i64) -> MetadataEntry {
    MetadataEntry::new(
        CommitVersion::new(version),
        MetadataRecord::BatchCommitted {
            object: key(&format!("obj-{which}")),
            spans: vec![CommittedSpan::new(
                topic(),
                partition(),
                1,
                ByteRange::bounded(0, 32).expect("a valid range"),
                None,
            )],
            written_at: at(when),
        },
    )
}

/// A partition's extent is the oldest and newest commit that touched it.
#[test]
fn a_partition_knows_when_its_commits_happened() {
    let mut index = IndexState::new();
    index
        .apply(&[
            commit(1, 0, 1_000),
            commit(2, 1, 5_000),
            commit(3, 2, 3_000),
        ])
        .expect("a plain log folds");
    let span = index
        .time_span(&topic(), partition())
        .expect("committed to");
    assert_eq!(span.min(), at(1_000));
    assert_eq!(
        span.max(),
        at(5_000),
        "the newest commit, not the last one in log order"
    );
}

/// ⚠️ **The same log folds to the same age at any page size.** A replaying
/// coordinator pages the log and a live one folds it an entry at a time, and a
/// retention round on either must delete the same data.
#[test]
fn the_extent_does_not_depend_on_how_the_log_was_paged() {
    let log = [
        commit(1, 0, 4_000),
        commit(2, 1, 1_000),
        commit(3, 2, 9_000),
    ];

    let mut whole = IndexState::new();
    whole.apply(&log).expect("one page");

    let mut paged = IndexState::new();
    for entry in &log {
        paged
            .apply(core::slice::from_ref(entry))
            .expect("one entry a page");
    }

    assert_eq!(
        whole.time_span(&topic(), partition()),
        paged.time_span(&topic(), partition())
    );
}

/// ⚠️ **No commit is not "committed at the epoch".** A round reading `EPOCH`
/// for a partition nothing has written to would reap it on its first sweep.
#[test]
fn a_partition_nothing_committed_to_has_no_age() {
    let index = IndexState::new();
    assert_eq!(index.time_span(&topic(), partition()), None);
}

fn publish(version: u64, upto: i64) -> MetadataEntry {
    MetadataEntry::new(
        CommitVersion::new(version),
        MetadataRecord::ManifestPublished {
            topic: topic(),
            partition: partition(),
            manifest: key("manifest"),
            upto: offset(upto),
        },
    )
}

/// ⚠️ **A batch that commits and publishes for one partition goes through the
/// projection, not the staged loop** — and the projection is then the only
/// place that partition's extent is written. Every other case here holds
/// commits alone, so without this one the projected path could drop the
/// batch's commit time and nothing would fail: a retention round would read
/// the partition as having no age, or the age it had before the batch. Found
/// by `M5.86`'s first round.
#[test]
fn a_batch_mixing_a_commit_and_a_publication_keeps_the_commit_s_time() {
    let mut index = IndexState::new();
    index
        .apply(&[commit(1, 0, 5_000), publish(2, 0)])
        .expect("a commit and a publication meeting its tail");
    let span = index
        .time_span(&topic(), partition())
        .expect("committed to");
    assert_eq!((span.min(), span.max()), (at(5_000), at(5_000)));
}

/// And the projected path widens what was already there, rather than
/// replacing it with the batch's own extent.
#[test]
fn the_projected_path_widens_an_existing_extent() {
    let mut index = IndexState::new();
    index
        .apply(&[commit(1, 0, 1_000)])
        .expect("an earlier commit");
    index
        .apply(&[commit(2, 1, 9_000), publish(3, 0)])
        .expect("a later commit and a publication in one batch");
    let span = index
        .time_span(&topic(), partition())
        .expect("committed to");
    assert_eq!(
        (span.min(), span.max()),
        (at(1_000), at(9_000)),
        "the earlier commit is still the oldest"
    );
}
