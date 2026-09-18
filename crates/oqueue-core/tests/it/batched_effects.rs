//! Swaps that share a batch, and what the fold makes of them.
//!
//! ⚠️ **Split from `range_compacted.rs` along the concept**
//! (`code-structure.md` rule 18): that file is what one swap does to one
//! partition; this is what two effects in one batch do to each other. Every
//! case here was a defect before the fold became a projection — `M5.13`'s
//! first round measured three of them, each a check pass disagreeing with the
//! apply pass it was supposed to mirror.

// A panic in a test harness is the test failing, which is what it is for.
#![allow(clippy::expect_used)]

use oqueue_core::{
    CommitVersion, Error, MetadataEntry, MetadataRecord, ObjectRef, TAIL_WINDOW_ENTRIES,
};

use crate::range_compacted::{
    history_keys, key, loaded, offset, partition, reference, swap, topic,
};

/// ⚠️ **Two swaps in one batch are judged one after the other, not both
/// against the state before either.** The second retires references the first
/// already retired, so it is refused — and the whole batch with it.
///
/// ⚠️ **Measured before the fold became a projection**: both were accepted,
/// history held two references covering the same offsets, and paged one entry
/// at a time the second was refused. `M5.13`'s first round.
#[test]
fn two_swaps_with_the_same_inputs_in_one_batch_are_refused() {
    let objects = 8 + TAIL_WINDOW_ENTRIES;
    let mut index = loaded(objects);
    let before = index.entries();
    let version = u64::try_from(objects).expect("a small count");
    let retiring: Vec<ObjectRef> = (0..8)
        .map(|which| reference(&format!("obj-{which}"), i64::from(which), 1))
        .collect();

    let refused = index
        .apply(&[
            swap(
                version + 1,
                retiring.clone(),
                vec![reference("merged-a", 0, 8)],
            ),
            swap(version + 2, retiring, vec![reference("merged-b", 0, 8)]),
        ])
        .expect_err("the second retires what the first retired");
    // ⚠️ **The reason, not only the variant.** Two swaps for one partition
    // refuse for different causes, and a message that said the same thing for
    // both would tell an operator which partition and nothing else.
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
        because.contains("history does not hold"),
        "the second swap retires what the first already retired: {because}"
    );
    assert_eq!(index.entries(), before, "and neither was applied");
}

/// ⚠️ **A swap chained onto another in the same batch is accepted**, because
/// the second sees the first's output. The alternative — judging both against
/// the pre-batch index — refused this in one page and accepted it in two.
#[test]
fn a_swap_chained_onto_another_in_one_batch_is_accepted() {
    let objects = 8 + TAIL_WINDOW_ENTRIES;
    let version = u64::try_from(objects).expect("a small count");
    let first: Vec<ObjectRef> = (0..4)
        .map(|which| reference(&format!("obj-{which}"), i64::from(which), 1))
        .collect();
    let log = [
        swap(version + 1, first, vec![reference("merged-a", 0, 4)]),
        swap(
            version + 2,
            vec![reference("merged-a", 0, 4)],
            vec![reference("merged-b", 0, 4)],
        ),
    ];

    let mut once = loaded(objects);
    once.apply(&log).expect("one page");
    let mut paged = loaded(objects);
    for entry in &log {
        paged.apply(std::slice::from_ref(entry)).expect("two pages");
    }
    assert_eq!(once.entries(), paged.entries());
    assert_eq!(history_keys(&once), history_keys(&paged));
    assert!(
        history_keys(&once).contains(&"merged-b".to_owned()),
        "the chain's last output is what history holds: {:?}",
        history_keys(&once)
    );
}

/// ⚠️ **A publication and a swap for one partition in one batch do not both
/// claim the same offsets.** The publication absorbs the history below its
/// boundary; the swap that follows finds those references gone and is refused,
/// which is what keeps a manifest and a reference from covering the same
/// records.
///
/// ⚠️ **Measured before the rewrite**: both applied, and history held a
/// reference on top of a manifest already covering it.
#[test]
fn a_publication_and_a_swap_for_one_partition_do_not_overlap() {
    let objects = 8 + TAIL_WINDOW_ENTRIES;
    let mut index = loaded(objects);
    let before = index.entries();
    let version = u64::try_from(objects).expect("a small count");
    let retiring: Vec<ObjectRef> = (0..8)
        .map(|which| reference(&format!("obj-{which}"), i64::from(which), 1))
        .collect();

    let refused = index
        .apply(&[
            MetadataEntry::new(
                CommitVersion::new(version + 1),
                MetadataRecord::ManifestPublished {
                    topic: topic(),
                    partition: partition(),
                    manifest: key("manifest"),
                    upto: offset(8),
                },
            ),
            swap(version + 2, retiring, vec![reference("merged", 0, 8)]),
        ])
        .expect_err("the manifest absorbed what the swap would retire");
    // The publication absorbed what the swap would retire, so the swap names a
    // reference the partition's history no longer holds.
    let Error::SwapRefused { ref because, .. } = refused else {
        panic!("a refusal naming the swap: {refused:?}");
    };
    assert!(
        because.contains("history does not hold"),
        "the manifest took it first: {because}"
    );
    assert_eq!(index.entries(), before, "and neither was applied");
}

/// ⚠️ **The swap that spans exactly what it retires and still loses a
/// record.** `installing` carries a zero-record reference alongside the real
/// output: the span matches, so a contiguity check admits it, and the empty
/// reference then sorts into history ahead of the real object at the same base
/// offset. A fetch resuming at that offset lands on the empty one and resumes
/// at the *next* object — the acknowledged record is never returned, and only
/// from the offset a consumer actually resumes at.
///
/// Found by `M5.13`'s third round, with a randomized differential harness over
/// 1,199 seeds.
#[test]
fn a_swap_installing_an_empty_reference_is_refused() {
    let objects = 8 + TAIL_WINDOW_ENTRIES;
    let mut index = loaded(objects);
    let before = index.entries();
    let served = history_keys(&index);

    let refused = index
        .apply(&[swap(
            u64::try_from(objects).expect("a small count") + 1,
            vec![reference("obj-0", 0, 1)],
            vec![reference("merged", 0, 1), reference("empty", 1, 0)],
        )])
        .expect_err("a reference covering no offsets is not an output");
    // ⚠️ **The sentence has to name the empty reference, not the bounds.** A
    // zero-offset output passes a comparison of bounds — `[0,1)` against
    // `[0,1)` — so a message about coverage would send an operator to check
    // the one thing that is not wrong.
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
        because.contains("empty") && because.contains("covers no offsets"),
        "it names the reference covering no offsets: {because}"
    );
    assert_eq!(index.entries(), before, "nothing was applied");
    assert_eq!(history_keys(&index), served, "and the inputs stay live");
}

/// ⚠️ **A swap with an empty side is refused, and says which side.** Retiring
/// a run and installing nothing would delete acknowledged records from the
/// index with no object holding them; installing a run against nothing to
/// retire would name records twice. Both fail the coverage check, and both
/// look like a bounds mismatch unless the message says otherwise — the
/// operator reading it has two lists, one of which is empty, and needs to be
/// told that is the whole problem.
#[test]
fn a_swap_with_an_empty_side_is_refused_and_says_so() {
    let objects = 8 + TAIL_WINDOW_ENTRIES;
    let version = u64::try_from(objects).expect("a small count") + 1;

    for (retiring, installing) in [
        (vec![reference("obj-0", 0, 1)], Vec::new()),
        (Vec::new(), vec![reference("merged", 0, 1)]),
    ] {
        let mut index = loaded(objects);
        let before = index.entries();

        let refused = index
            .apply(&[swap(version, retiring, installing)])
            .expect_err("neither side of a swap may be empty");
        let Error::SwapRefused { ref because, .. } = refused else {
            panic!("a refusal naming the swap: {refused:?}");
        };
        assert!(
            because.contains("neither side of a swap may be empty"),
            "it names the empty side rather than the bounds: {because}"
        );
        assert_eq!(index.entries(), before, "and nothing was applied");
    }
}
