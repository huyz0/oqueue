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
                None,
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
    assert_eq!(reader.entries(), 0);
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
        reader.entries(),
        2,
        "⚠️ `ADR-0025`, and the reason it is on the reader: `ADR-0024` moved a \
         coordinator's index behind this handle, so without it the one \
         materialization folding every partition on a shard is the one nobody \
         can measure"
    );
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

/// The fake delegates its read side to the fold as well as its write side.
///
/// ⚠️ Here rather than only in `oqueue-index`'s contract suite: mutation
/// testing narrows to one crate and runs only that crate's tests, so a fake
/// whose `find_batches` returned an empty page would survive every mutant
/// unless this crate's own suite looked at it.
#[test]
fn the_fake_delegates_its_read_side_too() {
    let index = FakeMaterializedIndex::new();
    index
        .apply(&[commit(1, 2, 10), commit(2, 2, 10)])
        .expect("applied");

    let page = index
        .find_batches(
            &topic("secret-tenant-topic"),
            partition(),
            Offset::ZERO,
            u64::MAX,
        )
        .expect("a page");
    assert_eq!(page.len(), 2);
    assert_eq!(
        index.entries(),
        2,
        "the fake answers the growth question through the trait like any other"
    );
    assert_eq!(page[0].reference().base_offset(), Offset::ZERO);
    assert_eq!(
        page[1].reference().base_offset(),
        Offset::new(2).expect("a valid offset")
    );
    assert_eq!(page[1].known_len(), Some(10));
}

/// ⚠️ **It passes the manifest through** — `ADR-0042` point 4, and the one
/// thing a reader needs before it can read a compacted history at all. A
/// handle answering `None` here would have every fetch below the boundary find
/// nothing and report an empty partition, which is the silent wrongness this
/// project is written against.
#[test]
fn the_handle_reports_the_manifest_the_index_holds() {
    let index = Arc::new(FakeMaterializedIndex::new());
    let reader = IndexReader::new(Arc::clone(&index) as Arc<dyn MaterializedIndex>);
    assert_eq!(
        reader.manifest(&topic("secret-tenant-topic"), partition()),
        None
    );

    let objects = 130_u64;
    let log: Vec<MetadataEntry> = (1..=objects).map(|version| commit(version, 1, 8)).collect();
    index.apply(&log).expect("a plain log folds");
    let upto = Offset::new(2).expect("a valid offset");
    let manifest = ObjectKey::new("m").expect("a valid key");
    index
        .apply(&[MetadataEntry::new(
            CommitVersion::new(objects + 1),
            MetadataRecord::ManifestPublished {
                topic: topic("secret-tenant-topic"),
                partition: partition(),
                manifest: manifest.clone(),
                upto,
            },
        )])
        .expect("a manifest meeting a boundary");

    assert_eq!(
        reader.manifest(&topic("secret-tenant-topic"), partition()),
        Some((manifest, upto)),
        "the handle answers what the index holds"
    );
}
