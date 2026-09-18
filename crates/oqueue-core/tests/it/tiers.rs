//! Each tier's count moves when its own tier does, and not otherwise.
//!
//! ⚠️ **The attribution `entries()` cannot give** (`M5.71`, `ADR-0043`
//! decision 1). A total that has risen does not say whether the tail grew with
//! the node's partitions — bounded, expected — or whether un-absorbed history
//! is running away because the compaction sweep has stalled, which is the only
//! term that can. Every case here moves one tier and asserts the other two
//! stood still.

// A panic in a test harness is the test failing, which is what it is for.
#![allow(clippy::expect_used)]

use core::mem::size_of;
use oqueue_core::{
    BundleNamer, ByteRange, CommitVersion, CommittedSpan, IndexState, MetadataEntry,
    MetadataRecord, ObjectKey, ObjectRef, Offset, TAIL_WINDOW_ENTRIES, TailEntry, Tiers,
};

use crate::range_compacted::{key, offset, partition, reference, swap, topic};

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
    let log: Vec<MetadataEntry> = (0..objects)
        .map(|which| commit(u64::try_from(which).expect("a small count") + 1, which, 1))
        .collect();
    index.apply(&log).expect("a plain log folds");
    index
}

/// ⚠️ **The sum is what the quota reads**, so the split is only trustworthy if
/// it agrees with the number that was there before it.
#[test]
fn the_three_counts_sum_to_the_entries_they_replaced() {
    let index = folded(4 + TAIL_WINDOW_ENTRIES);
    assert_eq!(index.tiers().total(), index.entries());
    assert_eq!(index.tiers().tail, TAIL_WINDOW_ENTRIES);
    assert_eq!(index.tiers().history, 4);
    assert_eq!(index.tiers().manifests, 0);
}

/// A commit inside the window is tail growth and nothing else.
#[test]
fn a_commit_inside_the_window_moves_only_the_tail() {
    let mut index = folded(3);
    let before = index.tiers();
    index.apply(&[commit(4, 3, 1)]).expect("a plain commit");

    assert_eq!(index.tiers().tail, before.tail + 1, "one more tail entry");
    assert_eq!(index.tiers().history, before.history, "history stood still");
    assert_eq!(index.tiers().manifests, before.manifests);
}

/// ⚠️ **Past the window a commit moves two tiers at once**, which is the case
/// the old arithmetic could not express: it added exactly one per span folded,
/// so it could say the total rose and never which tier received it. A push
/// demotes at most one entry, so the tail holds and history gains.
#[test]
fn a_commit_past_the_window_moves_the_tail_and_history_together() {
    let objects = TAIL_WINDOW_ENTRIES;
    let mut index = folded(objects);
    let before = index.tiers();
    assert_eq!(
        before.history, 0,
        "nothing has fallen out of the window yet"
    );

    index
        .apply(&[commit(
            u64::try_from(objects).expect("a small count") + 1,
            objects,
            1,
        )])
        .expect("a plain commit");

    assert_eq!(
        index.tiers().tail,
        before.tail,
        "the window is full, so it holds"
    );
    assert_eq!(
        index.tiers().history,
        1,
        "the demoted entry landed in history"
    );
    assert_eq!(index.tiers().manifests, before.manifests);
}

/// ⚠️ **A publication is the one event that moves history *down***, and it
/// moves the manifest count up by exactly one however many entries it absorbed
/// — `ADR-0042`'s whole point, visible here as a number rather than as prose.
#[test]
fn a_publication_moves_history_and_the_manifest_count_and_not_the_tail() {
    let objects = 6 + TAIL_WINDOW_ENTRIES;
    let mut index = folded(objects);
    let before = index.tiers();
    assert_eq!(before.history, 6);

    index
        .apply(&[publish(
            u64::try_from(objects).expect("a small count") + 1,
            offset(6).get(),
        )])
        .expect("a manifest meeting the remaining history");

    assert_eq!(index.tiers().history, 0, "six absorbed");
    assert_eq!(index.tiers().manifests, 1, "into one reference");
    assert_eq!(index.tiers().tail, before.tail, "the tail is untouched");
}

/// A swap moves history alone, in the direction compaction exists for.
#[test]
fn a_swap_moves_only_history() {
    let objects = 8 + TAIL_WINDOW_ENTRIES;
    let mut index = folded(objects);
    let before = index.tiers();

    let retiring: Vec<ObjectRef> = (0..8)
        .map(|which| reference(&format!("obj-{which}"), i64::from(which), 1))
        .collect();
    index
        .apply(&[swap(
            u64::try_from(objects).expect("a small count") + 1,
            retiring,
            vec![reference("merged", 0, 8)],
        )])
        .expect("outputs covering the inputs exactly");

    assert_eq!(index.tiers().history, 1, "eight references became one");
    assert_eq!(index.tiers().tail, before.tail, "the tail is untouched");
    assert_eq!(index.tiers().manifests, before.manifests);
}

/// ⚠️ **What this pins is the arithmetic, not the widths.** `index_cost.rs`
/// owns the three widths and asserts `Tiers::bytes` against them one tier at a
/// time; what is left to check is that a mix of tiers costs each one at its
/// own width rather than at an average — so the key width here is
/// `BundleNamer`'s own, not a literal, and the expected total is built from
/// `size_of` for the same reason `index_cost.rs` refuses a literal: a constant
/// compared to the constant above it binds nothing.
#[test]
fn a_mix_of_tiers_costs_each_one_at_its_own_width() {
    let key_width = BundleNamer::new("400000-18d649b1560f6000-1")
        .expect("a valid writer")
        .next_key()
        .expect("a first key")
        .as_str()
        .len();
    let history = size_of::<ObjectRef>() + key_width;
    let tail = size_of::<TailEntry>() + key_width;
    let manifest = size_of::<ObjectKey>() + size_of::<Offset>() + key_width;

    let tiers = Tiers {
        tail: 3,
        history: 5,
        manifests: 1,
    };
    assert_eq!(tiers.bytes(key_width), 3 * tail + 5 * history + manifest);

    // ⚠️ And the mix is not the average: five history entries and three tail
    // ones at one blended width would come to a different number, which is
    // the mistake a single per-entry constant would make.
    assert_ne!(
        tiers.bytes(key_width),
        9 * ((tail + history + manifest) / 3)
    );
}
