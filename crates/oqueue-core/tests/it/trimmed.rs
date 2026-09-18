//! Trim: the one partition-level retention primitive, and what it does not do.
//!
//! ⚠️ **Metadata-only** (`M5.19`). A trim advances a partition's first readable
//! offset and drops the index entries wholly below it. It writes no object and
//! deletes none — physical deletion is the object lifecycle's, reached only once
//! every partition inside an object is dead, because a bundled object holds
//! other partitions' live records.

// A panic in a test harness is the test failing, which is what it is for.
#![allow(clippy::expect_used)]

use oqueue_core::{
    ByteRange, CommitVersion, CommittedSpan, Error, IndexState, MetadataEntry, MetadataRecord,
    TAIL_WINDOW_ENTRIES, Timestamp,
};

use crate::range_compacted::{key, offset, partition, topic};

/// `objects` commits of `per` records each, versions 1..=objects.
fn committed(objects: usize, per: u32) -> Vec<MetadataEntry> {
    (0..objects)
        .map(|which| {
            MetadataEntry::new(
                CommitVersion::new(u64::try_from(which).expect("a small count") + 1),
                MetadataRecord::BatchCommitted {
                    object: key(&format!("obj-{which}")),
                    spans: vec![CommittedSpan::new(
                        topic(),
                        partition(),
                        per,
                        ByteRange::bounded(0, u64::from(per) * 32).expect("a valid range"),
                        None,
                    )],
                    written_at: Timestamp::EPOCH,
                },
            )
        })
        .collect()
}

fn trim(version: u64, start: i64) -> MetadataEntry {
    MetadataEntry::new(
        CommitVersion::new(version),
        MetadataRecord::Trimmed {
            topic: topic(),
            partition: partition(),
            start: offset(start),
        },
    )
}

/// ⚠️ **No object written, none deleted** — the row's first criterion.
/// Folding a trim is a question about the index, and a fold that reached for
/// the store would be deleting bytes another partition in the same object
/// still serves.
#[test]
fn a_trim_writes_no_object_and_deletes_none() {
    // ⚠️ **The fold holds no store at all**, and that is what this asserts:
    // `IndexState::apply` takes entries and returns a result, so there is no
    // seam through which a trim could reach object storage. What is checked
    // is the other half — that the objects it released are still named by
    // nothing but the partition keeps its offset line — because a trim that
    // "deleted" by dropping the partition would lose the end offset too.
    let mut index = IndexState::new();
    index.apply(&committed(4, 10)).expect("four objects");
    index
        .apply(&[trim(5, 20)])
        .expect("a trim to an object boundary");
    assert_eq!(index.entries(), 2, "two references released");
    assert_eq!(
        index.end_offset(&topic(), partition()),
        offset(40),
        "the offset line is untouched"
    );
}

/// Entries wholly below the start go; the one straddling it stays.
#[test]
fn a_trim_drops_only_what_is_wholly_below_the_start() {
    let mut index = IndexState::new();
    index
        .apply(&committed(4, 10))
        .expect("offsets 0..40 in four objects");
    index
        .apply(&[trim(5, 25)])
        .expect("a trim inside the third object");

    assert_eq!(index.log_start(&topic(), partition()), offset(25));
    assert_eq!(
        index.entries(),
        2,
        "obj-0 and obj-1 are dead; obj-2 holds 25..30 and stays"
    );
    let first = index
        .find_batches(&topic(), partition(), offset(25), u64::MAX)
        .expect("a read at the start");
    assert_eq!(
        first[0].reference().object(),
        &key("obj-2"),
        "a read from the start resolves into the straddling object"
    );
}

/// ⚠️ **A read below the start is refused, not skipped.** A page from the first
/// live offset would move a consumer past data it never saw on a successful
/// poll; the broker maps this to `OFFSET_OUT_OF_RANGE`.
#[test]
fn a_read_below_the_start_is_refused() {
    let mut index = IndexState::new();
    index.apply(&committed(4, 10)).expect("four objects");
    index.apply(&[trim(5, 20)]).expect("a trim");

    let refused = index
        .find_batches(&topic(), partition(), offset(5), u64::MAX)
        .expect_err("offset 5 was trimmed");
    assert!(matches!(
        refused,
        Error::BelowLogStart {
            requested: 5,
            log_start: 20
        }
    ));
}

/// ⚠️ **Idempotent and monotone.** A retention round that re-issues a trim
/// after a crash, or issues one below the current start, must not wedge the
/// fold or move the start backwards over deleted data.
#[test]
fn a_trim_at_or_below_the_start_is_a_no_op() {
    let mut index = IndexState::new();
    index.apply(&committed(4, 10)).expect("four objects");
    index.apply(&[trim(5, 20)]).expect("a trim");
    let after = index.tiers();

    index
        .apply(&[trim(6, 20), trim(7, 10)])
        .expect("repeating it, and going backwards, both fold");
    assert_eq!(index.log_start(&topic(), partition()), offset(20));
    assert_eq!(index.tiers(), after, "and neither dropped anything more");
}

/// ⚠️ **Past the end is refused, not clamped** — the next produce would land
/// below a start that far out, acknowledged and unreadable.
#[test]
fn a_trim_past_the_end_is_refused_and_changes_nothing() {
    let mut index = IndexState::new();
    index.apply(&committed(4, 10)).expect("offsets 0..40");
    let before = index.tiers();

    let refused = index
        .apply(&[trim(5, 41)])
        .expect_err("41 is past the end at 40");
    assert!(matches!(
        refused,
        Error::TrimPastEnd {
            start: 41,
            end: 40,
            ..
        }
    ));
    assert_eq!(index.tiers(), before, "guarantee 2: nothing applied");
    assert_eq!(index.log_start(&topic(), partition()), offset(0));
}

/// A trim to the end kills every entry, and the partition keeps its end so the
/// next produce continues the offset line rather than restarting it.
#[test]
fn a_trim_to_the_end_leaves_an_empty_partition_that_keeps_its_offsets() {
    let mut index = IndexState::new();
    index.apply(&committed(4, 10)).expect("offsets 0..40");
    index.apply(&[trim(5, 40)]).expect("everything is dead");
    assert_eq!(index.entries(), 0);
    assert_eq!(index.end_offset(&topic(), partition()), offset(40));
    assert_eq!(index.log_start(&topic(), partition()), offset(40));
}

/// ⚠️ **History as well as the tail.** Every case above fits inside the tail
/// window, so a trim that forgot the history tier — or kept exactly the dead
/// entries there — would pass all of them. Past the window, objects are in
/// history, and it is history a retention round mostly trims.
#[test]
fn a_trim_reaches_into_history() {
    let objects = TAIL_WINDOW_ENTRIES + 6;
    let mut index = IndexState::new();
    index
        .apply(&committed(objects, 1))
        .expect("six objects in history");
    assert_eq!(index.tiers().history, 6);

    index
        .apply(&[trim(u64::try_from(objects).expect("a small count") + 1, 4)])
        .expect("a trim inside history");

    assert_eq!(
        index.tiers().history,
        2,
        "obj-0..3 gone, obj-4 and obj-5 stay"
    );
    assert_eq!(
        index.tiers().tail,
        TAIL_WINDOW_ENTRIES,
        "the tail is untouched"
    );
    let first = index
        .find_batches(&topic(), partition(), offset(4), u64::MAX)
        .expect("a read at the start");
    assert_eq!(first[0].reference().object(), &key("obj-4"));
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

/// ⚠️ **The manifest goes only when everything it covers is dead.** A trim
/// below its `upto` must keep it — the records between the start and `upto`
/// are live and the manifest is where a reader finds them — and a trim at or
/// past `upto` must drop it, or the index names an object holding nothing
/// readable.
#[test]
fn a_manifest_goes_only_when_everything_it_covers_is_dead() {
    let objects = TAIL_WINDOW_ENTRIES + 6;
    let next = u64::try_from(objects).expect("a small count") + 1;

    let mut kept = IndexState::new();
    kept.apply(&committed(objects, 1)).expect("six in history");
    kept.apply(&[publish(next, 6)])
        .expect("a manifest over all six");
    kept.apply(&[trim(next + 1, 3)])
        .expect("a trim inside the manifest");
    assert!(
        kept.manifest(&topic(), partition()).is_some(),
        "records 3..6 are live and only the manifest names them"
    );

    let mut dropped = IndexState::new();
    dropped
        .apply(&committed(objects, 1))
        .expect("six in history");
    dropped
        .apply(&[publish(next, 6)])
        .expect("a manifest over all six");
    dropped
        .apply(&[trim(next + 1, 6)])
        .expect("a trim at the manifest's end");
    assert!(
        dropped.manifest(&topic(), partition()).is_none(),
        "everything it covers is dead"
    );
}

/// ⚠️ **Commits and the trim that follows them, in one batch.** An effect is
/// checked against the state the log had built when it was written, and the
/// commits before a trim are part of that state. A first draft checked it
/// against the end *before the batch*, so this log folded one entry at a time
/// and was refused whole with `TrimPastEnd { start: 20, end: 0 }` — a valid
/// log treated as malformed depending on how a replay paged it, which stops
/// the fold for good. Found by `M5.19`'s first round.
#[test]
fn a_trim_folds_the_same_whole_or_an_entry_at_a_time() {
    let mut log = committed(4, 10);
    log.push(trim(5, 20));
    log.extend(committed(6, 10).into_iter().skip(4).map(|entry| {
        let version = entry.version().get() + 1;
        MetadataEntry::new(CommitVersion::new(version), entry.record().clone())
    }));

    let mut whole = IndexState::new();
    whole
        .apply(&log)
        .expect("one page: commits, a trim, more commits");

    let mut paged = IndexState::new();
    for entry in &log {
        paged
            .apply(core::slice::from_ref(entry))
            .expect("one entry a page");
    }

    for index in [&whole, &paged] {
        assert_eq!(index.log_start(&topic(), partition()), offset(20));
        assert_eq!(index.end_offset(&topic(), partition()), offset(60));
    }
    assert_eq!(whole.tiers(), paged.tiers());
}
