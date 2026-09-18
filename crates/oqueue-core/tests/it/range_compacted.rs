//! The swap: a run of a partition's references becomes an equivalent run.
//!
//! ⚠️ **Compaction must never affect correctness, only efficiency** (FR-34).
//! Every case here asks one of two questions: does a fetch over the compacted
//! range return the same records, and does a swap that cannot prove it kept
//! them get refused with the inputs left live.

// A panic in a test harness is the test failing, which is what it is for.
#![allow(clippy::expect_used)]
// ⚠️ `pub` here is `pub(crate)` in effect — `main.rs` is the only root of this
// test binary and nothing outside it can name these. `batched_effects.rs`
// shares the fixture rather than owning a second copy of it.
#![allow(unreachable_pub)]

use oqueue_core::Timestamp;
use oqueue_core::{
    ByteRange, CommitVersion, CommittedSpan, Error, IndexState, MetadataEntry, MetadataRecord,
    ObjectKey, ObjectRef, Offset, PartitionId, TAIL_WINDOW_ENTRIES, TopicId,
};

pub fn topic() -> TopicId {
    TopicId::new("t").expect("a valid topic")
}

pub fn partition() -> PartitionId {
    PartitionId::new(0).expect("a valid partition")
}

pub fn key(name: &str) -> ObjectKey {
    ObjectKey::new(name).expect("a valid key")
}

pub fn offset(value: i64) -> Offset {
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
                ByteRange::bounded(0, u64::from(records) * 32).expect("a valid range"),
                None,
            )],
            written_at: Timestamp::EPOCH,
        },
    )
}

pub fn swap(version: u64, retiring: Vec<ObjectRef>, installing: Vec<ObjectRef>) -> MetadataEntry {
    MetadataEntry::new(
        CommitVersion::new(version),
        MetadataRecord::RangeCompacted {
            topic: topic(),
            partition: partition(),
            retiring,
            installing,
        },
    )
}

pub fn reference(name: &str, base: i64, records: u32) -> ObjectRef {
    ObjectRef::new(key(name), offset(base), records)
}

/// An index holding `objects` one-record objects, of which all but the last
/// `TAIL_WINDOW_ENTRIES` are in history and therefore compactable.
pub fn loaded(objects: usize) -> IndexState {
    let mut index = IndexState::default();
    let log: Vec<MetadataEntry> = (0..objects)
        .map(|which| commit(u64::try_from(which).expect("a small count") + 1, which, 1))
        .collect();
    index.apply(&log).expect("a plain log folds");
    index
}

/// How many references a fetch from `start` resolves to.
fn resolves_to(index: &IndexState, start: i64) -> usize {
    index
        .find_batches(&topic(), partition(), offset(start), u64::MAX)
        .expect("a folded partition")
        .len()
}

/// ⚠️ **`M5.5`'s index-side criterion, which only becomes true once the swap
/// has happened**: a fetch over the compacted range resolves to exactly one
/// reference where it resolved to N before.
#[test]
fn a_fetch_over_the_compacted_range_resolves_to_one_reference() {
    let objects = 8 + TAIL_WINDOW_ENTRIES;
    let mut index = loaded(objects);
    let before = index.history_len(&topic(), partition());
    assert_eq!(before, 8, "eight objects in history, the rest in the tail");
    assert!(resolves_to(&index, 0) > 8, "a fetch names each of them");

    let retiring: Vec<ObjectRef> = (0..8)
        .map(|which| reference(&format!("obj-{which}"), i64::from(which), 1))
        .collect();
    let installing = vec![reference("merged", 0, 8)];
    index
        .apply(&[swap(
            u64::try_from(objects).expect("a small count") + 1,
            retiring,
            installing,
        )])
        .expect("outputs covering the inputs exactly");

    assert_eq!(
        index.history_len(&topic(), partition()),
        1,
        "eight references became one"
    );
    assert_eq!(
        index
            .find_batches(&topic(), partition(), offset(0), u64::MAX)
            .expect("a folded partition")[0]
            .reference()
            .object(),
        &key("merged"),
        "and the fetch starts at the object the swap installed"
    );
}

/// ⚠️ **The count is what compaction buys the coordinator**, and it moves by
/// the difference rather than by either side.
#[test]
fn the_entry_count_falls_by_what_the_swap_collapsed() {
    let objects = 8 + TAIL_WINDOW_ENTRIES;
    let mut index = loaded(objects);
    let before = index.entries();

    let retiring: Vec<ObjectRef> = (0..8)
        .map(|which| reference(&format!("obj-{which}"), i64::from(which), 1))
        .collect();
    index
        .apply(&[swap(
            u64::try_from(objects).expect("a small count") + 1,
            retiring,
            vec![reference("merged", 0, 8)],
        )])
        .expect("a valid swap");

    assert_eq!(index.entries(), before - 7, "eight retired, one installed");
}

/// ⚠️ **A swap whose outputs do not cover its inputs is refused and the inputs
/// stay live** — the row's own words, and NFR-20's shape. The outputs here are
/// a perfectly good object that simply stops one record early.
#[test]
fn a_swap_missing_one_range_is_refused_and_the_inputs_stay_live() {
    let objects = 8 + TAIL_WINDOW_ENTRIES;
    let mut index = loaded(objects);
    let before = index.entries();
    let resolved = resolves_to(&index, 0);

    let retiring: Vec<ObjectRef> = (0..8)
        .map(|which| reference(&format!("obj-{which}"), i64::from(which), 1))
        .collect();
    let refused = index
        .apply(&[swap(
            u64::try_from(objects).expect("a small count") + 1,
            retiring,
            vec![reference("merged", 0, 7)],
        )])
        .expect_err("outputs seven records short");
    // ⚠️ **The fallback sentence, pinned.** This is the one case that reaches
    // it, so without an assertion here the whole coverage branch could say
    // "history does not hold" — the other half of the check — and stay green,
    // sending an operator to look for a reference that is right where it
    // should be.
    let Error::SwapRefused {
        topic: ref named,
        partition: ref named_partition,
        ref because,
    } = refused
    else {
        panic!("a refusal naming the swap: {refused:?}");
    };
    assert_eq!((named.as_str(), *named_partition), ("t", 0));
    assert!(
        because.contains("do not cover exactly") && because.contains("obj-0"),
        "it names the coverage half of the check, and the run: {because}"
    );

    assert_eq!(index.entries(), before, "nothing was applied");
    assert_eq!(
        resolves_to(&index, 0),
        resolved,
        "the inputs are still live"
    );
}

/// ⚠️ **A swap retiring a reference the index does not hold is refused.**
/// Otherwise a stale plan — one built before another swap landed — installs
/// its outputs beside references that already cover the same offsets, and the
/// records are served twice.
#[test]
fn a_swap_retiring_a_reference_the_index_does_not_hold_is_refused() {
    let objects = 8 + TAIL_WINDOW_ENTRIES;
    let mut index = loaded(objects);
    let before = index.entries();

    let refused = index
        .apply(&[swap(
            u64::try_from(objects).expect("a small count") + 1,
            vec![reference("never-committed", 0, 8)],
            vec![reference("merged", 0, 8)],
        )])
        .expect_err("the index holds no such reference");
    assert!(
        matches!(refused, Error::SwapRefused { ref topic, partition, .. } if topic == "t" && partition == 0),
        "the refusal names the partition it is about: {refused:?}"
    );
    assert_eq!(index.entries(), before, "nothing was applied");
}

/// ⚠️ **A swap naming a *tail* reference is refused**, and this is not
/// fussiness. A tail entry carries a byte range a fetch reads directly; a
/// history entry carries none. Retiring one for the other turns a one-GET read
/// into a footer resolution — and compaction's own age guard refuses to
/// rewrite tail data, so a swap naming one is a planner and an index
/// disagreeing about what the partition holds.
#[test]
fn a_swap_naming_a_tail_reference_is_refused() {
    let objects = 8 + TAIL_WINDOW_ENTRIES;
    let mut index = loaded(objects);
    let before = index.entries();
    // The newest object is in the tail window by construction.
    let newest = objects - 1;
    let tail_ref = reference(
        &format!("obj-{newest}"),
        i64::try_from(newest).expect("a small count"),
        1,
    );

    let refused = index
        .apply(&[swap(
            u64::try_from(objects).expect("a small count") + 1,
            vec![tail_ref],
            vec![reference(
                "merged",
                i64::try_from(newest).expect("small"),
                1,
            )],
        )])
        .expect_err("the tail is not compactable");
    assert!(
        matches!(refused, Error::SwapRefused { ref topic, partition, .. } if topic == "t" && partition == 0),
        "the refusal names the partition it is about: {refused:?}"
    );
    assert_eq!(index.entries(), before, "nothing was applied");
}

/// ⚠️ **Two attempts at one plan, and the index names exactly one** — the
/// row's own acceptance for `ADR-0037`'s unique-key obligation. The second
/// attempt retires references the first already retired, so the fold refuses
/// it and the second object is orphaned rather than installed beside the
/// first.
#[test]
fn a_second_attempt_at_one_plan_is_refused_and_the_index_names_one() {
    let objects = 8 + TAIL_WINDOW_ENTRIES;
    let mut index = loaded(objects);
    let version = u64::try_from(objects).expect("a small count");
    let retiring: Vec<ObjectRef> = (0..8)
        .map(|which| reference(&format!("obj-{which}"), i64::from(which), 1))
        .collect();

    index
        .apply(&[swap(
            version + 1,
            retiring.clone(),
            vec![reference("merged-a", 0, 8)],
        )])
        .expect("the first attempt lands");
    let after_first = index.entries();

    let refused = index
        .apply(&[swap(
            version + 2,
            retiring,
            vec![reference("merged-b", 0, 8)],
        )])
        .expect_err("the references it retires are gone");
    assert!(
        matches!(refused, Error::SwapRefused { ref topic, partition, .. } if topic == "t" && partition == 0),
        "the refusal names the partition it is about: {refused:?}"
    );
    assert_eq!(index.entries(), after_first, "the index names one of them");

    let found = index
        .find_batches(&topic(), partition(), offset(0), u64::MAX)
        .expect("a folded partition");
    assert_eq!(found[0].reference().object(), &key("merged-a"));
}

/// ⚠️ **The swap is an append and rewrites nothing** (`ADR-0038`, an
/// acceptance criterion on this row rather than advice). A rebuild from the
/// log reaches the same state, which is only true because every original
/// `BatchCommitted` is still there to be folded.
#[test]
fn a_rebuild_from_the_log_reaches_the_same_state() {
    let objects = 8 + TAIL_WINDOW_ENTRIES;
    let mut log: Vec<MetadataEntry> = (0..objects)
        .map(|which| commit(u64::try_from(which).expect("a small count") + 1, which, 1))
        .collect();
    let retiring: Vec<ObjectRef> = (0..8)
        .map(|which| reference(&format!("obj-{which}"), i64::from(which), 1))
        .collect();
    log.push(swap(
        u64::try_from(objects).expect("a small count") + 1,
        retiring,
        vec![reference("merged", 0, 8)],
    ));

    let mut once = IndexState::default();
    once.apply(&log).expect("one page");
    let mut paged = IndexState::default();
    for chunk in log.chunks(7) {
        paged.apply(chunk).expect("several pages");
    }

    assert_eq!(once.entries(), paged.entries());
    assert_eq!(
        once.history_len(&topic(), partition()),
        paged.history_len(&topic(), partition())
    );
    assert_eq!(
        once.end_offset(&topic(), partition()),
        paged.end_offset(&topic(), partition())
    );
}

/// The history references a fetch names, in order.
pub fn history_keys(index: &IndexState) -> Vec<String> {
    index
        .find_batches(&topic(), partition(), offset(0), u64::MAX)
        .expect("a folded partition")
        .iter()
        .take(index.history_len(&topic(), partition()))
        .map(|batch| batch.reference().object().as_str().to_owned())
        .collect()
}

/// ⚠️ **Which references survive, not how many.** A swap that retired the
/// wrong ones — or kept the wrong ones — leaves a history of the right length
/// naming the wrong objects, and every count-based assertion passes. Measured:
/// inverting the retain predicate keeps exactly the retired references and
/// drops the rest, with the counts unchanged.
#[test]
fn a_swap_retires_the_references_it_names_and_keeps_the_rest() {
    let objects = 8 + TAIL_WINDOW_ENTRIES;
    let mut index = loaded(objects);
    assert_eq!(
        history_keys(&index),
        (0..8).map(|n| format!("obj-{n}")).collect::<Vec<_>>(),
        "eight objects in history before the swap"
    );

    // Retire the middle four, leaving obj-0, obj-1, obj-6 and obj-7.
    let retiring: Vec<ObjectRef> = (2..6)
        .map(|which| reference(&format!("obj-{which}"), i64::from(which), 1))
        .collect();
    index
        .apply(&[swap(
            u64::try_from(objects).expect("a small count") + 1,
            retiring,
            vec![reference("merged", 2, 4)],
        )])
        .expect("a valid swap");

    assert_eq!(
        history_keys(&index),
        vec!["obj-0", "obj-1", "merged", "obj-6", "obj-7"],
        "the retired four are gone, the rest are untouched, and order is kept"
    );
    assert_eq!(
        index.entries(),
        TAIL_WINDOW_ENTRIES + 5,
        "four retired, one installed"
    );
}

/// ⚠️ **A reference still in the tail at the end of the batch is refused, even
/// when the same batch committed it.** The check asks where the tail *will*
/// start, so an object the batch's own commits have not demoted is not
/// compactable — and a check that asked only "did this batch mention it" would
/// admit exactly the tail rewrite the age guard exists to prevent.
#[test]
fn a_swap_naming_a_reference_its_own_batch_left_in_the_tail_is_refused() {
    let objects = 8 + TAIL_WINDOW_ENTRIES;
    let newest = objects - 1;
    let mut log: Vec<MetadataEntry> = (0..objects)
        .map(|which| commit(u64::try_from(which).expect("a small count") + 1, which, 1))
        .collect();
    log.push(swap(
        u64::try_from(objects).expect("a small count") + 1,
        vec![reference(
            &format!("obj-{newest}"),
            i64::try_from(newest).expect("a small count"),
            1,
        )],
        vec![reference(
            "merged",
            i64::try_from(newest).expect("a small count"),
            1,
        )],
    ));

    let mut index = IndexState::default();
    let refused = index
        .apply(&log)
        .expect_err("the newest object is still in the tail");
    assert!(
        matches!(refused, Error::SwapRefused { ref topic, partition, .. } if topic == "t" && partition == 0),
        "the refusal names the partition it is about: {refused:?}"
    );
}

/// ⚠️ **A committed span of no records is refused** (`M5.77`). It would become
/// a reference covering no offsets — which `M5.13` made permanent, since
/// `contiguous_span` refuses any set containing one and no `RangeCompacted`
/// could then name it — and it would sort into history ahead of the real
/// object at its base offset, where a fetch resuming there skips an
/// acknowledged record.
///
/// ⚠️ **No flush produces one**, and that is why this is here: the index
/// refuses a malformed log rather than trusting the writer, which is the
/// standard every other check in the fold is held to.
#[test]
fn a_committed_span_of_no_records_is_refused() {
    let mut index = IndexState::default();
    index.apply(&[commit(1, 0, 3)]).expect("a real commit");
    let before = index.entries();
    let end = index.end_offset(&topic(), partition());

    // ⚠️ **A non-empty byte range with a zero record count**, because
    // `ByteRange::bounded` refuses a zero-length one — so the malformed shape
    // this plants is a region holding bytes and claiming no records, which is
    // the one a footer could carry.
    let empty_span = MetadataEntry::new(
        CommitVersion::new(2),
        MetadataRecord::BatchCommitted {
            object: key("obj-1"),
            spans: vec![CommittedSpan::new(
                topic(),
                partition(),
                0,
                ByteRange::bounded(0, 32).expect("a valid range"),
                None,
            )],
            written_at: Timestamp::EPOCH,
        },
    );
    let refused = index
        .apply(&[empty_span])
        .expect_err("a span of no records is not a span");
    // ⚠️ **The object too, and asserted by name.** It is the field that says
    // *where to look* in a log somebody else wrote, and under a `..` it was
    // the one field a mutation could empty with every test still green.
    let Error::EmptySpanInLog {
        topic: ref named,
        partition: ref named_partition,
        ref object,
    } = refused
    else {
        panic!("a refusal naming the span: {refused:?}");
    };
    assert_eq!((named.as_str(), *named_partition), ("t", 0));
    assert_eq!(object, "obj-1", "and the object the span belongs to");
    assert_eq!(index.entries(), before, "nothing was applied");
    assert_eq!(
        index.end_offset(&topic(), partition()),
        end,
        "and the partition did not move"
    );
}
