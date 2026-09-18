//! Publishing a manifest: what the fold does to a partition's history.
//!
//! ⚠️ **`ADR-0042`'s state column, made observable.** A partition's seven-day
//! history at doc 14 §3's working set is 97 TB of entries; what the
//! coordinator holds after this event is one reference. `IndexState::entries`
//! is what says so, and these are the tests that hold it to it.

// A panic in a test harness is the test failing, which is what it is for.
#![allow(clippy::expect_used)]

use oqueue_core::{
    ByteRange, CommitVersion, CommittedSpan, Error, FakeMaterializedIndex, IndexState,
    MaterializedIndex, MetadataEntry, MetadataRecord, ObjectKey, Offset, PartitionId,
    TAIL_WINDOW_ENTRIES, TopicId,
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

/// An index holding `objects` one-record objects, folded one entry each.
fn folded(objects: usize) -> IndexState {
    let mut index = IndexState::default();
    let entries: Vec<MetadataEntry> = (0..objects)
        .map(|which| commit(u64::try_from(which).expect("a small count") + 1, which, 1))
        .collect();
    index.apply(&entries).expect("a valid fold");
    index
}

/// ⚠️ **`M5.62`'s acceptance**: publishing leaves the partition's history at
/// one entry however many objects it replaces.
#[test]
fn publishing_leaves_one_entry_for_any_number_of_objects() {
    for history in [2_usize, 64] {
        let objects = history + TAIL_WINDOW_ENTRIES;
        let mut index = folded(objects);
        assert_eq!(
            index.entries(),
            objects,
            "one entry per object before publishing"
        );

        index
            .apply(&[publish(
                u64::try_from(objects).expect("a small count") + 1,
                i64::try_from(history).expect("a small count"),
            )])
            .expect("a manifest that meets history");

        assert_eq!(
            index.entries(),
            TAIL_WINDOW_ENTRIES + 1,
            "the tail, plus one reference where {history} history entries were"
        );
    }
}

/// ⚠️ **A second manifest supersedes the first rather than joining it**, so
/// the count does not creep by one per round.
#[test]
fn a_later_manifest_replaces_the_earlier_one() {
    let objects = 64 + TAIL_WINDOW_ENTRIES;
    let mut index = folded(objects);
    index
        .apply(&[publish(
            u64::try_from(objects).expect("a small count") + 1,
            30,
        )])
        .expect("a manifest");
    // ⚠️ **Absolute, not relative to what this run itself produced.** The
    // first version read `entries()` here and asserted the second publish
    // moved it by 34, so a mutation shifting both readings equally survived —
    // which one did: `before - history.len()` read as `+`.
    assert_eq!(
        index.entries(),
        TAIL_WINDOW_ENTRIES + 34 + 1,
        "30 of 64 history entries absorbed, 34 left, and one reference"
    );
    index
        .apply(&[publish(
            u64::try_from(objects).expect("a small count") + 2,
            64,
        )])
        .expect("a later manifest");
    assert_eq!(
        index.entries(),
        TAIL_WINDOW_ENTRIES + 1,
        "the rest absorbed, and still one reference"
    );
}

/// ⚠️ **The fold refuses a manifest that does not meet what is left.** Below
/// the boundary is records nothing can serve; above it is records served
/// twice, and a reader binary-searching the manifest cannot tell either from
/// a healthy one.
///
/// ⚠️ **Ten-record objects, because one-record ones make every offset a
/// boundary** — the first version of this test used them and asserted nothing.
#[test]
fn a_manifest_that_does_not_meet_history_is_refused() {
    let objects = 8 + TAIL_WINDOW_ENTRIES;
    let mut index = IndexState::default();
    let entries: Vec<MetadataEntry> = (0..objects)
        .map(|which| commit(u64::try_from(which).expect("a small count") + 1, which, 10))
        .collect();
    index.apply(&entries).expect("a valid fold");
    let before = index.entries();

    // Object 3 holds 30..40, so 35 is inside it and not a boundary.
    let err = index
        .apply(&[publish(
            u64::try_from(objects).expect("a small count") + 1,
            35,
        )])
        .expect_err("an offset inside an object is not a boundary");
    assert!(
        matches!(
            err,
            Error::ManifestDoesNotMeetHistory {
                upto: 35,
                expected: 40
            }
        ),
        "and it names the boundary above: {err:?}"
    );
    assert_eq!(index.entries(), before, "a refusal changes nothing");

    index
        .apply(&[publish(
            u64::try_from(objects).expect("a small count") + 1,
            40,
        )])
        .expect("the boundary itself is accepted");
    assert!(index.entries() < before, "and it absorbs what it covers");
}

/// ⚠️ **Refused before anything is mutated** — `IndexState`'s guarantee 2. A
/// manifest naming a partition the index has never seen is the simplest case
/// of that, and the batch it arrives in must land nothing.
#[test]
fn a_manifest_for_an_unknown_partition_leaves_the_index_untouched() {
    let mut index = folded(4);
    let before = index.entries();
    let err = index
        .apply(&[MetadataEntry::new(
            CommitVersion::new(99),
            MetadataRecord::ManifestPublished {
                topic: TopicId::new("other").expect("a valid topic"),
                partition: partition(),
                manifest: key("manifest"),
                upto: offset(1),
            },
        )])
        .expect_err("a partition the index does not hold");
    assert!(matches!(err, Error::ManifestDoesNotMeetHistory { .. }));
    assert_eq!(index.entries(), before, "and nothing changed");
}

/// ⚠️ **A rebuild reproduces one entry, not N**, which is the property that
/// makes the state bounded rather than momentarily small: replaying the log
/// from zero folds the publication too.
#[test]
fn a_rebuild_reproduces_one_reference_rather_than_the_entries_it_replaced() {
    let history = 64_usize;
    let objects = history + TAIL_WINDOW_ENTRIES;
    let mut log: Vec<MetadataEntry> = (0..objects)
        .map(|which| commit(u64::try_from(which).expect("a small count") + 1, which, 1))
        .collect();
    log.push(publish(
        u64::try_from(objects).expect("a small count") + 1,
        i64::try_from(history).expect("a small count"),
    ));

    let mut index = IndexState::default();
    index.apply(&log).expect("a valid fold");
    let once = index.entries();

    let mut replayed = IndexState::default();
    for entry in &log {
        replayed
            .apply(std::slice::from_ref(entry))
            .expect("a valid fold, one entry at a time");
    }
    assert_eq!(replayed.entries(), once, "however the log is paged in");
    assert_eq!(once, TAIL_WINDOW_ENTRIES + 1);
}

/// ⚠️ **A manifest may cover history and never the tail.** `absorb` retains
/// over history alone, so a manifest reaching into the tail would leave every
/// tail entry below it in place and those records would be named twice. It is
/// also what compaction produces: the age guard refuses to rewrite tail data,
/// so an `upto` above the tail's start is a manifest no compaction wrote.
#[test]
fn a_manifest_reaching_into_the_tail_is_refused() {
    let history = 2_usize;
    let objects = history + TAIL_WINDOW_ENTRIES;
    let version = u64::try_from(objects).expect("a small count");
    let mut index = folded(objects);
    let before = index.entries();

    let err = index
        .apply(&[publish(version + 1, 64)])
        .expect_err("64 is a tail base, not a history one");
    assert!(
        matches!(
            err,
            Error::ManifestDoesNotMeetHistory {
                upto: 64,
                expected: 2
            }
        ),
        "and it names where the tail begins: {err:?}"
    );
    assert_eq!(index.entries(), before);

    index
        .apply(&[publish(version + 1, 2)])
        .expect("the tail's own start is the most a manifest may cover");
    assert_eq!(
        index.entries(),
        TAIL_WINDOW_ENTRIES + 1,
        "all of history absorbed, and one reference"
    );
}

/// A partition whose objects are all still in the tail has no history for a
/// manifest to cover, so the only boundary is zero.
#[test]
fn a_partition_with_no_history_admits_only_an_empty_manifest() {
    let objects = 4_usize;
    let version = u64::try_from(objects).expect("a small count");
    let mut index = folded(objects);
    index
        .apply(&[publish(version + 1, 4)])
        .expect_err("everything is still hot");
    index
        .apply(&[publish(version + 1, 0)])
        .expect("covering nothing is vacuous but well formed");
    assert_eq!(index.entries(), objects + 1);
}

/// ⚠️ **A batch holding two publications is judged exactly as two batches
/// would judge them**, whichever way the log is paged. Keeping only the last
/// was tried and made the fold's result depend on paging: one page accepted a
/// descending pair and two pages refused it, so two brokers rebuilding the
/// same log disagreed about whether it could be materialized at all. Found by
/// review, which measured both pagings.
#[test]
fn a_batch_of_two_publications_decides_as_two_batches_would() {
    let objects = 64 + TAIL_WINDOW_ENTRIES;
    let version = u64::try_from(objects).expect("a small count");

    // Ascending: accepted either way, and the state is the same.
    let mut batched = folded(objects);
    batched
        .apply(&[publish(version + 1, 30), publish(version + 2, 64)])
        .expect("ascending is accepted in one page");
    let mut paged = folded(objects);
    paged.apply(&[publish(version + 1, 30)]).expect("and");
    paged.apply(&[publish(version + 2, 64)]).expect("in two");
    assert_eq!(batched.entries(), paged.entries());
    assert_eq!(batched.entries(), TAIL_WINDOW_ENTRIES + 1);

    // Descending: refused either way. ⚠️ **The states differ and that is
    // right**: a batch is all-or-nothing, so one page leaves the index
    // untouched while two pages have already applied the first publication.
    // What must agree is the decision, not the bookkeeping around it.
    let mut batched = folded(objects);
    batched
        .apply(&[publish(version + 1, 64), publish(version + 2, 30)])
        .expect_err("descending is refused in one page");
    assert_eq!(batched.entries(), objects, "and the batch applied nothing");
    let mut paged = folded(objects);
    paged.apply(&[publish(version + 1, 64)]).expect("the first");
    paged
        .apply(&[publish(version + 2, 30)])
        .expect_err("and the second is refused in two pages too");
}

/// ⚠️ **A manifest covering less than the one before it is refused.** It
/// un-covers records that are already in an object nothing else names, and no
/// compaction produces one.
#[test]
fn a_manifest_that_covers_less_than_its_predecessor_is_refused() {
    let objects = 64 + TAIL_WINDOW_ENTRIES;
    let version = u64::try_from(objects).expect("a small count");
    let mut index = folded(objects);
    index.apply(&[publish(version + 1, 64)]).expect("the first");
    let before = index.entries();

    let err = index
        .apply(&[publish(version + 2, 30)])
        .expect_err("a manifest that covers less");
    assert!(
        matches!(
            err,
            Error::ManifestDoesNotMeetHistory {
                upto: 30,
                expected: 64
            }
        ),
        "and it names what is already covered: {err:?}"
    );
    assert_eq!(index.entries(), before, "a refusal changes nothing");
}

/// ⚠️ **Republishing the same boundary is accepted**, in one batch or two. A
/// replay re-applies records it has already seen, and the rule is that a
/// manifest may not cover *less* than the one before it — not that it must
/// cover more.
#[test]
fn republishing_the_same_boundary_is_accepted() {
    let objects = 64 + TAIL_WINDOW_ENTRIES;
    let version = u64::try_from(objects).expect("a small count");

    let mut batched = folded(objects);
    batched
        .apply(&[publish(version + 1, 64), publish(version + 2, 64)])
        .expect("the same boundary twice in one batch");
    assert_eq!(batched.entries(), TAIL_WINDOW_ENTRIES + 1);

    let mut paged = folded(objects);
    paged.apply(&[publish(version + 1, 64)]).expect("the first");
    paged
        .apply(&[publish(version + 2, 64)])
        .expect("and again in its own batch");
    assert_eq!(paged.entries(), batched.entries());
}

/// ⚠️ **What the index knows about a manifest, and all it knows** — the
/// accessor `M5.62` deferred and `ADR-0042` point 4 records. A reader below
/// the offset it returns finds nothing in `find_batches`, because the entries
/// it would have found are what the manifest replaced; the key is where to
/// look instead.
#[test]
fn the_index_reports_the_manifest_and_how_far_it_covers() {
    let objects = 2 + TAIL_WINDOW_ENTRIES;
    let version = u64::try_from(objects).expect("a small count");
    let mut index = IndexState::default();
    assert_eq!(
        index.manifest(&topic(), partition()),
        None,
        "a partition nothing published for has no manifest"
    );

    let log: Vec<MetadataEntry> = (0..objects)
        .map(|which| commit(u64::try_from(which).expect("a small count") + 1, which, 1))
        .collect();
    index.apply(&log).expect("a plain log folds");
    index
        .apply(&[publish(version + 1, 2)])
        .expect("a manifest meeting a boundary");

    assert_eq!(
        index.manifest(&topic(), partition()),
        Some((key("manifest"), offset(2))),
        "the key it published, and how far it covers"
    );
    assert_eq!(
        index.manifest(&topic(), PartitionId::new(1).expect("a valid partition")),
        None,
        "and only for the partition it named"
    );
}

/// ⚠️ **The seam reports it too**, which is a separate claim from the fold
/// doing so: `FakeMaterializedIndex` is what every crate downstream of this
/// one tests against (`contracts.md` rule 9), and a fake that answered `None`
/// here would have every one of them testing a read path no broker takes.
#[test]
fn the_fake_reports_the_manifest_the_fold_holds() {
    let objects = 2 + TAIL_WINDOW_ENTRIES;
    let version = u64::try_from(objects).expect("a small count");
    let index = FakeMaterializedIndex::new();
    assert_eq!(index.manifest(&topic(), partition()), None);

    let log: Vec<MetadataEntry> = (0..objects)
        .map(|which| commit(u64::try_from(which).expect("a small count") + 1, which, 1))
        .collect();
    index.apply(&log).expect("a plain log folds");
    index
        .apply(&[publish(version + 1, 2)])
        .expect("a manifest meeting a boundary");

    assert_eq!(
        index.manifest(&topic(), partition()),
        Some((key("manifest"), offset(2))),
        "the seam answers what the fold holds"
    );
}
