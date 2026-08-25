//! `IndexState`'s fold, and the fake built on it.
//!
//! ⚠️ **Not a duplicate of `oqueue-index`'s conformance suite.** That suite
//! asserts the *trait contract* holds across every implementation of it; this
//! one asserts the fold `oqueue-core` itself owns. The distinction has teeth:
//! `IndexState` lives here, so mutation testing narrowed to this crate runs
//! only this crate's tests — and without these, eight mutants of the fold
//! survived while the contract suite two crates away would have killed them.

// Every `expect` is on a value the test itself built from a literal it
// controls, so a panic means the test is wrong, not the code under test.
#![allow(clippy::expect_used)]

use oqueue_core::{
    CommitVersion, CommittedSpan, CoordinatorEpoch, Error, FakeMaterializedIndex, IndexState,
    MaterializedIndex, MetadataEntry, MetadataRecord, ObjectKey, Offset, PartitionId, TopicId,
};

fn topic(name: &str) -> TopicId {
    TopicId::new(name.to_owned()).expect("a valid topic")
}

fn partition(index: i32) -> PartitionId {
    PartitionId::new(index).expect("a valid partition")
}

fn offset(value: i64) -> Offset {
    Offset::new(value).expect("a valid offset")
}

fn commit(version: u64, records: u32) -> MetadataEntry {
    MetadataEntry::new(
        CommitVersion::new(version),
        MetadataRecord::BatchCommitted {
            object: ObjectKey::new(format!("obj-{version}")).expect("a valid key"),
            spans: vec![CommittedSpan::new(topic("orders"), partition(0), records)],
        },
    )
}

/// The fold is the running sum of counts, and `applied_upto` follows it.
#[test]
fn the_fold_sums_counts_and_tracks_the_version() {
    let mut state = IndexState::new();
    assert_eq!(state.applied_upto(), None);
    assert_eq!(
        state.end_offset(&topic("orders"), partition(0)),
        Offset::ZERO
    );

    state.apply(&[commit(1, 3)]).expect("applied");
    assert_eq!(state.end_offset(&topic("orders"), partition(0)), offset(3));
    assert_eq!(state.applied_upto(), Some(CommitVersion::new(1)));

    state.apply(&[commit(2, 4)]).expect("applied");
    assert_eq!(state.end_offset(&topic("orders"), partition(0)), offset(7));
    assert_eq!(state.applied_upto(), Some(CommitVersion::new(2)));
}

/// ⚠️ Both directions of guarantee 1, and the boundary between them. A
/// decrease and a *repeat* are equally refused, and the repeat is the cell an
/// ack-lost retry lands in — folding it twice would double-count its records.
#[test]
fn the_fold_refuses_anything_not_strictly_increasing() {
    let mut state = IndexState::new();
    state.apply(&[commit(5, 2)]).expect("applied");

    assert_eq!(
        state
            .apply(&[commit(4, 1)])
            .expect_err("a decrease is refused"),
        Error::NonMonotonicCommitVersion {
            expected_above: 5,
            got: 4
        }
    );
    assert_eq!(
        state
            .apply(&[commit(5, 1)])
            .expect_err("a repeat is refused"),
        Error::NonMonotonicCommitVersion {
            expected_above: 5,
            got: 5
        }
    );
    state.apply(&[commit(6, 1)]).expect("6 follows 5");

    // The refusals changed nothing; only the accepted batch moved the fold.
    assert_eq!(state.end_offset(&topic("orders"), partition(0)), offset(3));
}

/// ⚠️ Guarantee 2. The batch is staged and applied whole, so a rejection
/// part-way through leaves no prefix behind — a caller that retries would
/// otherwise fold onto an index already carrying some of it.
#[test]
fn a_refused_batch_leaves_no_prefix_behind() {
    let mut state = IndexState::new();
    state.apply(&[commit(1, 3)]).expect("applied");

    state
        .apply(&[commit(2, 100), commit(2, 100)])
        .expect_err("refused for its second entry");

    assert_eq!(state.end_offset(&topic("orders"), partition(0)), offset(3));
    assert_eq!(state.applied_upto(), Some(CommitVersion::new(1)));
}

/// An epoch change moves the version and no offset.
#[test]
fn an_epoch_change_moves_no_offset() {
    let mut state = IndexState::new();
    state.apply(&[commit(1, 4)]).expect("applied");
    state
        .apply(&[MetadataEntry::new(
            CommitVersion::new(2),
            MetadataRecord::EpochChanged {
                epoch: CoordinatorEpoch::new(1),
            },
        )])
        .expect("applied");

    assert_eq!(state.end_offset(&topic("orders"), partition(0)), offset(4));
    assert_eq!(state.applied_upto(), Some(CommitVersion::new(2)));
}

/// `clear` returns it to its fresh state — both halves, which is the one a
/// mutant that cleared only the offsets would slip through.
#[test]
fn clear_resets_both_the_offsets_and_the_version() {
    let mut state = IndexState::new();
    state.apply(&[commit(1, 3)]).expect("applied");

    state.clear();

    assert_eq!(state.applied_upto(), None, "clear kept the version");
    assert_eq!(
        state.end_offset(&topic("orders"), partition(0)),
        Offset::ZERO,
        "clear kept the offsets"
    );
    // And it is genuinely fresh: the log replays from the beginning.
    state
        .apply(&[commit(1, 3)])
        .expect("a cleared index accepts v1 again");
    assert_eq!(state.end_offset(&topic("orders"), partition(0)), offset(3));
}

/// The fake delegates every method to the fold rather than reimplementing it.
#[test]
fn the_fake_delegates_to_the_fold() {
    let index = FakeMaterializedIndex::new();
    assert_eq!(index.applied_upto(), None);

    index.apply(&[commit(1, 5)]).expect("applied");
    assert_eq!(index.end_offset(&topic("orders"), partition(0)), offset(5));
    assert_eq!(index.applied_upto(), Some(CommitVersion::new(1)));

    index
        .apply(&[commit(1, 5)])
        .expect_err("the fake enforces the same ordering rule");

    index.clear();
    assert_eq!(index.applied_upto(), None);
    assert_eq!(
        index.end_offset(&topic("orders"), partition(0)),
        Offset::ZERO
    );
}

/// ⚠️ Reports how far it has folded and never the partitions it holds. As with
/// `FakeMetadataLog`, asserting only what is *absent* would pass against a
/// `Debug` that rendered nothing, so what it does render is pinned too.
#[test]
fn the_fake_reports_its_progress_without_listing_topics() {
    let index = FakeMaterializedIndex::new();
    index.apply(&[commit(1, 5)]).expect("applied");

    let rendered = format!("{index:?}");
    assert!(!rendered.contains("orders"), "rendered: {rendered}");
    assert!(
        rendered.contains("FakeMaterializedIndex"),
        "rendered: {rendered}"
    );
    assert!(rendered.contains('1'), "rendered: {rendered}");
}
