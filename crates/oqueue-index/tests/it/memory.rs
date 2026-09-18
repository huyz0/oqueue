//! `MemoryIndex` itself: what it reports about its own state, and that the seam
//! it sits behind is `dyn`-compatible.
//!
//! ⚠️ **Split from `index.rs` at the 500-line limit, along the concept.** That
//! file is the *contract*, run against every implementation of it; this is
//! about the one a broker serves reads from.

#![allow(clippy::expect_used)]

use oqueue_core::Timestamp;
use oqueue_core::{
    ByteRange, CommitVersion, CommittedSpan, MaterializedIndex, MetadataEntry, MetadataRecord,
    ObjectKey, Offset, PartitionId, TopicId,
};
use oqueue_index::MemoryIndex;

fn topic(name: &str) -> TopicId {
    TopicId::new(name.to_owned()).expect("a valid topic")
}

fn partition(index: i32) -> PartitionId {
    PartitionId::new(index).expect("a valid partition")
}

fn offset(value: i64) -> Offset {
    Offset::new(value).expect("a valid offset")
}

fn commit_on(version: u64, name: &str, part: i32, records: u32) -> MetadataEntry {
    MetadataEntry::new(
        CommitVersion::new(version),
        MetadataRecord::BatchCommitted {
            object: ObjectKey::new(format!("obj-{version}")).expect("a valid key"),
            spans: vec![CommittedSpan::new(
                topic(name),
                partition(part),
                records,
                ByteRange::Full,
                None,
            )],
            written_at: Timestamp::EPOCH,
        },
    )
}

fn commit(version: u64, records: u32) -> MetadataEntry {
    commit_on(version, "orders", 0, records)
}

/// ⚠️ Reports how far it has folded and never the partitions it holds — a
/// `Debug` line in a test failure must not become a listing of a tenant's
/// topics. Asserting only what is *absent* would pass against a `Debug` that
/// rendered nothing, so what it does render is pinned too.
#[test]
fn the_memory_index_reports_its_progress_without_listing_topics() {
    let index = MemoryIndex::new();
    index.apply(&[commit(1, 5)]).expect("applied");

    let rendered = format!("{index:?}");
    assert!(!rendered.contains("orders"), "rendered: {rendered}");
    assert!(rendered.contains("MemoryIndex"), "rendered: {rendered}");
    assert!(rendered.contains('1'), "rendered: {rendered}");
}

/// The seam is `dyn`-compatible — the broker holds whichever materialization
/// it was configured with, the same reason `ObjectStore` is shaped this way.
#[test]
fn the_seam_is_dyn_compatible() {
    let index: std::sync::Arc<dyn MaterializedIndex> = std::sync::Arc::new(MemoryIndex::new());
    index.apply(&[commit(1, 2)]).expect("applied");
    assert_eq!(index.end_offset(&topic("orders"), partition(0)), offset(2));
}

/// ⚠️ **The growth number a broker's materialization reports** (`M3.11`), and
/// the reason it is here rather than only on the fold: `MemoryIndex` is what a
/// node serves reads from, so it is what an operator asks how large it has
/// become. M3 only measures — a ceiling on this index's keying gives back range
/// a rebuild cannot restore, so `roadmap.md` carries the enforcement to `M5`.
#[test]
fn the_memory_index_reports_how_many_entries_it_holds() {
    let index = MemoryIndex::new();
    assert_eq!(index.entries(), 0);

    index.apply(&[commit(1, 3)]).expect("applied");
    assert_eq!(index.entries(), 1, "one entry per span, not per record");

    index
        .apply(&[commit_on(2, "orders", 1, 4), commit_on(3, "payments", 0, 2)])
        .expect("applied");
    assert_eq!(index.entries(), 3, "one per span, across topics");

    index.clear();
    assert_eq!(index.entries(), 0, "and a dropped cache holds nothing");
}
