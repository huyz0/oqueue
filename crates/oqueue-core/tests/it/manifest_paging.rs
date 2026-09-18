//! Paging invariance: the same log folds the same however it is chunked.
//!
//! ⚠️ **Its own file because it tests a property of the fold rather than of
//! publication** (`code-structure.md` rule 18), and because it is the property
//! `M5.62` took four review rounds to get right — each round found a different
//! input class where a whole-batch fold and an entry-at-a-time fold reached
//! different answers. `M3.8` pages the metadata log by entry count and a
//! coordinator rebuilding after a restart has no guarantee of landing on the
//! same boundaries, so a fold that depends on them has one broker
//! materializing an index another refuses.
//!
//! ⚠️ **Two claims, not one.** A *well-formed* log reaches the same state at
//! every page size. A malformed one is *refused* at every page size — but what
//! it leaves behind differs, because a batch is all-or-nothing and a refused
//! batch discards the commits that shared it. The decision is the invariant;
//! the debris is not.

// A panic in a test harness is the test failing, which is what it is for.
#![allow(clippy::expect_used)]

use oqueue_core::{
    ByteRange, CommitVersion, CommittedSpan, IndexState, MetadataEntry, MetadataRecord, ObjectKey,
    ObjectRef, Offset, PartitionId, TAIL_WINDOW_ENTRIES, TopicId,
};

fn topic() -> TopicId {
    TopicId::new("t").expect("a valid topic")
}

fn partition() -> PartitionId {
    PartitionId::new(0).expect("a valid partition")
}

fn key(name: &str) -> ObjectKey {
    ObjectKey::new(name).expect("a valid key")
}

fn offset(value: i64) -> Offset {
    Offset::new(value).expect("a valid offset")
}

fn commit(version: u64, which: usize, records: u32) -> MetadataEntry {
    MetadataEntry::new(
        CommitVersion::new(version),
        MetadataRecord::BatchCommitted {
            object: key(&format!("obj-{which}")),
            spans: vec![CommittedSpan::new(
                topic(),
                partition(),
                records,
                ByteRange::bounded(0, u64::from(records)).expect("a valid range"),
                None,
            )],
        },
    )
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

/// ⚠️ **A well-formed log folds to the same state however it is paged**,
/// which three earlier attempts at this rule did not manage: one accepted a
/// descending pair in one page and refused it in two, the next did the reverse
/// for a manifest landing inside the tail, and the third let a commit *after*
/// a publication widen the tail window the publication was judged against.
/// All three were measured by review.
///
/// ⚠️ **It does not reach every class on its own**, and saying so here is
/// the point: `upto` is an object base offset in this log, which `boundary`
/// answers on its `Ordering::Equal` branch however wide the staged window
/// grew. The two tests below carry the classes this one cannot.
///
/// ⚠️ **The signature is four numbers, not `entries()`.** A count collides:
/// the round-four defect produced the same `entries()` from one paging that
/// applied the manifest and another that refused it, so a test comparing
/// counts alone watched the divergence happen.
#[test]
fn a_well_formed_log_folds_the_same_at_any_page_size() {
    // ⚠️ **Publications in the middle, not at the end.** With every commit
    // first, a whole-batch fold and an entry-at-a-time fold see the same
    // staged prefix for every publication, which is the one arrangement the
    // round-four defect cannot be reached from.
    let mut log = Vec::new();
    let mut version = 0_u64;
    let mut which = 0_usize;
    for _ in 0..(2 + TAIL_WINDOW_ENTRIES) {
        version += 1;
        log.push(commit(version, which, 1));
        which += 1;
    }
    version += 1;
    log.push(publish(version, 1));
    for _ in 0..4 {
        version += 1;
        log.push(commit(version, which, 1));
        which += 1;
    }
    version += 1;
    log.push(publish(version, 2));

    let states: Vec<Signature> = [log.len(), 64, 7, 1]
        .into_iter()
        .map(|page| {
            let mut index = IndexState::default();
            for chunk in log.chunks(page) {
                index.apply(chunk).expect("a well-formed log folds");
            }
            signature(&index)
        })
        .collect();
    assert!(
        states.iter().all(|state| *state == states[0]),
        "every page size must reach the same state: {states:?}"
    );
    // The manifest covers [0, 2), so one reference stands where two entries
    // did and the tail is full.
    assert_eq!(
        states[0],
        (TAIL_WINDOW_ENTRIES + 5, 4, TAIL_WINDOW_ENTRIES, 134),
        "one manifest reference, four demoted objects, a full tail"
    );
}

/// ⚠️ **A malformed one is refused at every page size**, which is the claim
/// that can be made about it. ⚠️ **What it leaves behind differs by paging and
/// that is not a defect**: a batch is all-or-nothing, so a refused batch
/// discards the commits that shared it, and how many shared it is what the
/// page size decides. The decision is the invariant; the debris is not.
///
/// ⚠️ **It asserts the error, not that one occurred.** `refused > 0` alone
/// holds for a fold that refuses everything, which is the mutation this rule
/// is one line of code away from at all times.
#[test]
fn a_malformed_log_is_refused_at_every_page_size() {
    let objects = 2 + TAIL_WINDOW_ENTRIES;
    let version = u64::try_from(objects).expect("a small count");
    let mut log: Vec<MetadataEntry> = (0..objects)
        .map(|which| commit(u64::try_from(which).expect("a small count") + 1, which, 1))
        .collect();
    // 64 is a tail base, so no compaction wrote this manifest: it would leave
    // every tail entry below it in place, naming those records twice.
    log.push(publish(version + 1, 64));

    for page in [log.len(), 64, 7, 1] {
        let mut index = IndexState::default();
        let refusals: Vec<String> = log
            .chunks(page)
            .filter_map(|chunk| index.apply(chunk).err())
            .map(|error| format!("{error}"))
            .collect();
        assert_eq!(
            refusals.len(),
            1,
            "page size {page} refuses exactly the one"
        );
        assert!(
            refusals[0] == "a manifest covering up to 64 does not meet history at 2",
            "page size {page} refuses it for the boundary, not incidentally: {}",
            refusals[0]
        );
    }
}

/// What a fold reached, in enough detail that two different states cannot
/// report the same thing: entries, history depth, tail depth, end offset.
type Signature = (usize, usize, usize, i64);

fn signature(index: &IndexState) -> Signature {
    (
        index.entries(),
        index.history_len(&topic(), partition()),
        index.tail(&topic(), partition()).len(),
        index.end_offset(&topic(), partition()).get(),
    )
}

/// ⚠️ **A commit after a publication must not widen the window it is judged
/// against**, which is the class `a_well_formed_log_folds_the_same_at_any_page_size`
/// cannot reach: the `upto` here is neither a history base nor the tail's
/// start under one paging and is exactly the tail's start under the other, so
/// it is decided on `boundary`'s limit rather than on an equality that holds
/// whatever the window is.
///
/// Measured: with the publication judged against the whole batch instead of
/// the part preceding it, the whole-page fold returns `Ok` and the
/// entry-at-a-time fold returns `Err` — `M5.62`'s fourth round.
#[test]
fn a_commit_after_a_publication_does_not_widen_what_it_may_cover() {
    let mut log: Vec<MetadataEntry> = (0..TAIL_WINDOW_ENTRIES)
        .map(|which| commit(u64::try_from(which).expect("a small count") + 1, which, 1))
        .collect();
    let version = u64::try_from(TAIL_WINDOW_ENTRIES).expect("a small count");
    // Every object is in the tail, so the only boundary is base 0; `upto = 1`
    // reaches into the tail and no compaction wrote it. The commit that
    // follows demotes base 0 into history — but it is not this publication's
    // to see.
    log.push(publish(version + 1, 1));
    log.push(commit(version + 2, TAIL_WINDOW_ENTRIES, 1));

    for page in [log.len(), 1] {
        let mut index = IndexState::default();
        let refusals: Vec<String> = log
            .chunks(page)
            .filter_map(|chunk| index.apply(chunk).err())
            .map(|error| format!("{error}"))
            .collect();
        assert_eq!(
            refusals,
            vec!["a manifest covering up to 1 does not meet history at 0"],
            "page size {page} must refuse it too"
        );
    }
}

/// ⚠️ **An admitted publication is applied, even for a partition nothing has
/// committed to yet.** A fresh partition ends at offset zero and a manifest
/// covering `[0, 0)` meets it, so the fold accepts one — and dropping it for
/// want of a slot made the same log reach two states: folded whole, the
/// commits that follow create the partition before the manifest lands; folded
/// one entry at a time, there is no slot when it lands and it is discarded.
/// `M5.62`'s fifth round.
#[test]
fn a_publication_before_any_commit_is_applied_rather_than_dropped() {
    let log = [publish(1, 0), commit(2, 0, 1), commit(3, 1, 3)];

    let states: Vec<Signature> = [log.len(), 1]
        .into_iter()
        .map(|page| {
            let mut index = IndexState::default();
            for chunk in log.chunks(page) {
                index
                    .apply(chunk)
                    .expect("a manifest covering nothing meets a fresh partition");
            }
            signature(&index)
        })
        .collect();
    assert_eq!(states[0], states[1], "both pagings reach one state");
    // Two objects in the tail and the manifest reference: three entries.
    assert_eq!(states[0], (3, 0, 2, 4));
}

/// ⚠️ **A swap folds the same however its log is paged** — the property
/// `M5.62` spent four review rounds establishing for publication, asserted for
/// the second record to need it. A swap retires *history* references, and the
/// commits that demote an object into history may sit in the same batch, so a
/// check against the pre-batch index alone refuses in one page what it accepts
/// in several.
#[test]
fn a_swap_folds_the_same_at_any_page_size() {
    let objects = 8 + TAIL_WINDOW_ENTRIES;
    let mut log: Vec<MetadataEntry> = (0..objects)
        .map(|which| commit(u64::try_from(which).expect("a small count") + 1, which, 1))
        .collect();
    let retiring: Vec<ObjectRef> = (0..8)
        .map(|which| ObjectRef::new(key(&format!("obj-{which}")), offset(i64::from(which)), 1))
        .collect();
    log.push(MetadataEntry::new(
        CommitVersion::new(u64::try_from(objects).expect("a small count") + 1),
        MetadataRecord::RangeCompacted {
            topic: topic(),
            partition: partition(),
            retiring,
            installing: vec![ObjectRef::new(key("merged"), offset(0), 8)],
        },
    ));

    let states: Vec<Signature> = [log.len(), 64, 7, 1]
        .into_iter()
        .map(|page| {
            let mut index = IndexState::default();
            for chunk in log.chunks(page) {
                index.apply(chunk).expect("a well-formed log folds");
            }
            signature(&index)
        })
        .collect();
    assert!(
        states.iter().all(|state| *state == states[0]),
        "every page size must reach the same state: {states:?}"
    );
    assert_eq!(
        states[0],
        (TAIL_WINDOW_ENTRIES + 1, 1, TAIL_WINDOW_ENTRIES, 136),
        "one history reference where there were eight"
    );
}
