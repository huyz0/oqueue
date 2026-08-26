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
    ByteRange, CommitVersion, CommittedSpan, CoordinatorEpoch, Error, FakeMaterializedIndex,
    IndexState, MAX_BATCHES_PER_PAGE, MaterializedIndex, MetadataEntry, MetadataRecord, ObjectKey,
    Offset, PartitionId, TAIL_WINDOW_ENTRIES, TopicId,
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
            spans: vec![CommittedSpan::new(
                topic("orders"),
                partition(0),
                records,
                ByteRange::Full,
            )],
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

/// ⚠️ **The carried assertion `M3.5`'s review asked for.** A partition
/// appearing twice in one batch must accumulate from the *staged* running
/// offset, not restart from the committed one — `M3.8` commits every N≥1,000
/// entries, which makes one batch repeating a partition its normal case.
/// Without pinning an absolute value here, last-write-wins passes.
#[test]
fn a_partition_repeated_in_one_batch_accumulates() {
    let mut state = IndexState::new();
    state.apply(&[commit(1, 3)]).expect("applied");

    // Two more commits for the same partition, in a single batch.
    state.apply(&[commit(2, 4), commit(3, 5)]).expect("applied");

    assert_eq!(
        state.end_offset(&topic("orders"), partition(0)),
        offset(12),
        "3 + 4 + 5 — a fold that restarted from the committed base would say 8"
    );
}

/// ⚠️ The two tiers, and the demotion between them. The tail carries inline
/// byte ranges so a read of one is a single GET; everything past the window
/// keeps only the ~40-byte ref, whose range a reader resolves from the
/// object's own footer. Doc 14 §3 is why the second tier has to exist.
#[test]
fn entries_past_the_tail_window_demote_to_history() {
    let mut state = IndexState::new();
    let t = topic("orders");

    let window = u64::try_from(TAIL_WINDOW_ENTRIES).expect("the window fits a u64");
    for version in 1..=window {
        state.apply(&[commit(version, 1)]).expect("applied");
    }
    assert_eq!(state.tail(&t, partition(0)).len(), TAIL_WINDOW_ENTRIES);
    assert_eq!(
        state.history_len(&t, partition(0)),
        0,
        "nothing demoted yet"
    );

    state.apply(&[commit(window + 1, 1)]).expect("applied");

    assert_eq!(
        state.tail(&t, partition(0)).len(),
        TAIL_WINDOW_ENTRIES,
        "the window is bounded"
    );
    assert_eq!(
        state.history_len(&t, partition(0)),
        1,
        "the oldest entry demoted rather than being dropped"
    );
    // ⚠️ Demotion loses the range, never the records: the fold is unaffected.
    assert_eq!(
        state.end_offset(&t, partition(0)),
        offset(i64::try_from(TAIL_WINDOW_ENTRIES).expect("the window fits an i64") + 1)
    );
}

/// A tail entry carries the offsets its object contributed, so a fetch can
/// pick the right one without reading anything.
#[test]
fn tail_entries_carry_the_offsets_their_object_contributed() {
    let mut state = IndexState::new();
    state.apply(&[commit(1, 3), commit(2, 4)]).expect("applied");

    let tail = state.tail(&topic("orders"), partition(0));
    assert_eq!(tail.len(), 2);

    assert_eq!(tail[0].reference().base_offset(), Offset::ZERO);
    assert_eq!(tail[0].reference().record_count(), 3);
    assert_eq!(tail[1].reference().base_offset(), offset(3));
    assert_eq!(tail[1].reference().record_count(), 4);

    // The two together cover 0..7 with no gap and no overlap.
    assert_eq!(
        tail[0].reference().end_offset().expect("in range"),
        offset(3)
    );
    assert_eq!(
        tail[1].reference().end_offset().expect("in range"),
        offset(7)
    );
}

/// `clear` drops both tiers, not just the offsets.
#[test]
fn clear_drops_both_tiers() {
    let mut state = IndexState::new();
    state.apply(&[commit(1, 3)]).expect("applied");
    assert_eq!(state.tail(&topic("orders"), partition(0)).len(), 1);

    state.clear();

    assert!(state.tail(&topic("orders"), partition(0)).is_empty());
    assert_eq!(state.history_len(&topic("orders"), partition(0)), 0);
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

/// A commit whose span names a real bounded region, so the byte budget has a
/// length to charge against.
fn sized(version: u64, records: u32, bytes: u64) -> MetadataEntry {
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

fn page(state: &IndexState, start: i64, max_bytes: u64) -> Vec<i64> {
    state
        .find_batches(&topic("orders"), partition(0), offset(start), max_bytes)
        .expect("a page")
        .iter()
        .map(|batch| batch.reference().base_offset().get())
        .collect()
}

/// ⚠️ The boundary that decides whether a consumer at the high watermark reads
/// anything: an object is in the page when it still holds records **at or
/// after** `start`, so one ending exactly at `start` is not.
#[test]
fn a_page_starts_at_the_object_still_holding_start() {
    let mut state = IndexState::new();
    state
        .apply(&[sized(1, 2, 10), sized(2, 2, 10), sized(3, 2, 10)])
        .expect("applied");

    assert_eq!(page(&state, 0, u64::MAX), vec![0, 2, 4]);
    assert_eq!(
        page(&state, 1, u64::MAX),
        vec![0, 2, 4],
        "1 is inside [0, 2)"
    );
    assert_eq!(page(&state, 2, u64::MAX), vec![2, 4], "[0, 2) is behind it");
    assert_eq!(page(&state, 5, u64::MAX), vec![4]);
    assert_eq!(page(&state, 6, u64::MAX), Vec::<i64>::new(), "at the end");
}

/// The budget admits a batch that exactly fills it and refuses the one that
/// would exceed it.
#[test]
fn the_budget_is_inclusive_at_its_own_edge() {
    let mut state = IndexState::new();
    state
        .apply(&[sized(1, 1, 10), sized(2, 1, 10), sized(3, 1, 10)])
        .expect("applied");

    assert_eq!(page(&state, 0, 20), vec![0, 1], "10 + 10 is exactly 20");
    assert_eq!(page(&state, 0, 19), vec![0], "one byte short of the second");
    assert_eq!(page(&state, 0, 0), vec![0], "and one is always returned");
}

/// ⚠️ The same `start` boundary as above, but in the **history** tier — the
/// two tiers are filtered by separate loops, so a comparison correct in one
/// says nothing about the other.
#[test]
fn the_history_tier_honours_the_same_start_boundary() {
    let mut state = IndexState::new();
    for version in 1..=(TAIL_WINDOW_ENTRIES + 3) {
        state
            .apply(&[sized(version as u64, 1, 10)])
            .expect("applied");
    }
    assert_eq!(state.history_len(&topic("orders"), partition(0)), 3);

    // History holds objects at offsets 0, 1, 2. Starting at 1 must drop the
    // first and keep the second, whose records are [1, 2).
    let from_one = page(&state, 1, u64::MAX);
    assert_eq!(
        from_one[0], 1,
        "the object ending exactly at 1 is behind it"
    );

    let from_three = page(&state, 3, u64::MAX);
    assert_eq!(
        from_three[0], 3,
        "past the whole history tier, the page starts in the tail"
    );
}

/// History is older than the tail, so a page that spans both is in offset
/// order only if history comes first.
#[test]
fn a_page_spanning_both_tiers_stays_in_offset_order() {
    let mut state = IndexState::new();
    let total = TAIL_WINDOW_ENTRIES + 3;
    for version in 1..=total {
        state
            .apply(&[sized(version as u64, 1, 10)])
            .expect("applied");
    }
    assert_eq!(
        state.history_len(&topic("orders"), partition(0)),
        3,
        "three entries fell out of the window"
    );

    let bases = page(&state, 0, u64::MAX);
    assert_eq!(
        bases.len(),
        MAX_BATCHES_PER_PAGE,
        "the count bound closes a page that spans both tiers"
    );
    assert!(
        bases.windows(2).all(|pair| pair[0] < pair[1]),
        "ascending: {bases:?}"
    );
    assert_eq!(bases[0], 0, "the oldest history entry leads");

    // And the page starting inside the tail is all tail, still ascending.
    let tail_only = page(
        &state,
        i64::try_from(total - 2).expect("a count that fits"),
        u64::MAX,
    );
    assert_eq!(tail_only.len(), 2);
}

/// A batch whose length the index cannot price charges nothing against the
/// byte budget — it is unpriceable, not free, and the count is what bounds it.
#[test]
fn an_unknown_length_charges_nothing_against_the_budget() {
    let mut state = IndexState::new();
    // `commit` builds a `ByteRange::Full` span, whose length only the store
    // knows.
    state
        .apply(&[sized(1, 1, 10), commit(2, 1), commit(3, 1), sized(4, 1, 10)])
        .expect("applied");

    assert_eq!(
        page(&state, 0, 20),
        vec![0, 1, 2, 3],
        "two sized batches spend the whole budget and the unpriceable ones \
         pass through it"
    );
    assert_eq!(
        page(&state, 0, 5),
        vec![0, 1, 2],
        "the second sized batch is refused; the ones before it are not"
    );
}

/// A partition the fold has never seen has no batches, and is not an error.
#[test]
fn an_unfolded_partition_yields_an_empty_page() {
    let state = IndexState::new();
    assert_eq!(page(&state, 0, u64::MAX), Vec::<i64>::new());
}

/// The tail tier answers with its inline region; the history tier does not
/// have one to answer with. ⚠️ This is the whole point of two tiers: a tail
/// read is one GET because the range came back with the reference.
#[test]
fn only_the_tail_tier_answers_with_a_region() {
    let mut state = IndexState::new();
    for version in 1..=(TAIL_WINDOW_ENTRIES + 1) {
        state
            .apply(&[sized(version as u64, 1, 10)])
            .expect("applied");
    }
    let batches = state
        .find_batches(&topic("orders"), partition(0), Offset::ZERO, u64::MAX)
        .expect("a page");

    assert_eq!(
        batches[0].bytes(),
        None,
        "demoted: resolved from the footer"
    );
    assert_eq!(
        batches[1].bytes(),
        Some(ByteRange::bounded(0, 10).expect("a non-empty range")),
        "in the window: the region is inline"
    );
}
