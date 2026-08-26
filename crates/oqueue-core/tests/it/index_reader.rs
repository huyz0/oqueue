//! The read-only handle: what it answers, and what it does not carry.
//!
//! ⚠️ **Here as well as in `oqueue-coordinator`, and not by accident.**
//! Mutation testing narrows to one crate and runs only that crate's tests, so a
//! reader whose `find_batches` returned an empty page — or whose `Debug`
//! rendered nothing — would survive every mutant unless this crate's own suite
//! looked at it. The same reasoning `materialized_index.rs` records for the
//! fold.

// Every `expect` is on a value the test built from a literal it controls.
#![allow(clippy::expect_used)]

use oqueue_core::{
    ByteRange, CommitVersion, CommittedSpan, FakeMaterializedIndex, IndexReader, MaterializedIndex,
    MetadataEntry, MetadataRecord, ObjectKey, Offset, PartitionId, TopicId,
};
use std::sync::Arc;

fn topic(name: &str) -> TopicId {
    TopicId::new(name.to_owned()).expect("a valid topic")
}

fn partition() -> PartitionId {
    PartitionId::new(0).expect("a valid partition")
}

fn commit(version: u64, records: u32, bytes: u64) -> MetadataEntry {
    MetadataEntry::new(
        CommitVersion::new(version),
        MetadataRecord::BatchCommitted {
            object: ObjectKey::new(format!("obj-{version}")).expect("a valid key"),
            spans: vec![CommittedSpan::new(
                topic("secret-tenant-topic"),
                partition(),
                records,
                ByteRange::bounded(0, bytes).expect("a non-empty range"),
            )],
        },
    )
}

/// A writer's index and the reader it hands out answer the same questions the
/// same way — the reader delegates rather than caching anything of its own.
#[test]
fn a_reader_answers_exactly_what_the_index_it_wraps_holds() {
    let index = Arc::new(FakeMaterializedIndex::new());
    let reader = IndexReader::new(Arc::clone(&index) as Arc<dyn MaterializedIndex>);

    assert_eq!(reader.applied_upto(), None, "nothing folded yet");
    assert_eq!(
        reader.end_offset(&topic("secret-tenant-topic"), partition()),
        Offset::ZERO
    );
    assert!(
        reader
            .find_batches(
                &topic("secret-tenant-topic"),
                partition(),
                Offset::ZERO,
                u64::MAX
            )
            .expect("a page")
            .is_empty()
    );

    index
        .apply(&[commit(0, 2, 10), commit(1, 3, 10)])
        .expect("the fold succeeds");

    assert_eq!(reader.applied_upto(), Some(CommitVersion::new(1)));
    assert_eq!(
        reader.end_offset(&topic("secret-tenant-topic"), partition()),
        Offset::new(5).expect("a valid offset")
    );
    let page = reader
        .find_batches(
            &topic("secret-tenant-topic"),
            partition(),
            Offset::ZERO,
            u64::MAX,
        )
        .expect("a page");
    assert_eq!(
        page.iter()
            .map(|batch| batch.reference().base_offset().get())
            .collect::<Vec<_>>(),
        vec![0, 2],
        "both objects, in offset order, through the reader"
    );
}

/// ⚠️ A reader is reachable from an operator's error path, so its `Debug` must
/// not become a listing of a tenant's topics. Asserting only what is *absent*
/// would pass against a formatter rendering nothing, so what it does render is
/// pinned too.
#[test]
fn formatting_a_reader_summarises_rather_than_lists() {
    let index = Arc::new(FakeMaterializedIndex::new());
    index.apply(&[commit(0, 1, 10)]).expect("the fold succeeds");
    let reader = IndexReader::new(index as Arc<dyn MaterializedIndex>);

    let rendered = format!("{reader:?}");
    assert!(
        !rendered.contains("secret-tenant-topic"),
        "a Debug line must not name a tenant's topics: {rendered}"
    );
    assert!(
        rendered.contains("IndexReader") && rendered.contains("applied_upto"),
        "and must still say how far it has folded: {rendered}"
    );
}
