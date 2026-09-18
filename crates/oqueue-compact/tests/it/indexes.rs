//! The index-side fixtures: an index folded from counts, and one that counts
//! how it is read.
//!
//! ⚠️ **Split from `support.rs` at the 500-line limit, along the concept**
//! (`code-structure.md` rule 18): that file is the store side — objects
//! written, inputs read, plans derived — and this is what the planner reads
//! them *through*.

#![allow(clippy::expect_used)]
#![allow(clippy::redundant_pub_crate)]

use oqueue_core::{
    ByteRange, CommitVersion, CommittedSpan, FakeMaterializedIndex, IndexedBatch,
    MaterializedIndex, MetadataEntry, ObjectKey, Offset, PartitionId, Result, TopicId,
};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::support::{committed, key, partition, topic};

/// An index folded from `counts`, one object per entry, in order.
pub(crate) fn history_index(counts: &[u32]) -> FakeMaterializedIndex {
    let index = FakeMaterializedIndex::new();
    let entries: Vec<MetadataEntry> = counts
        .iter()
        .enumerate()
        .map(|(i, count)| {
            committed(
                i as u64 + 1,
                key(&format!("obj-{i}")),
                vec![CommittedSpan::new(
                    topic(),
                    partition(),
                    *count,
                    ByteRange::bounded(0, u64::from(*count)).expect("a valid range"),
                    None,
                )],
            )
        })
        .collect();
    index.apply(&entries).expect("a valid fold");
    index
}

/// A `MaterializedIndex` that counts the walks asked of it.
///
/// ⚠️ **Walks, not lookups.** `end_offset` is a map lookup and `find_batches`
/// is a scan, and the difference is what a sweep over a cold catalog costs.
#[derive(Debug)]
pub(crate) struct CountingIndex {
    inner: FakeMaterializedIndex,
    walks: AtomicUsize,
}

impl CountingIndex {
    /// Wraps an index.
    pub(crate) const fn over(inner: FakeMaterializedIndex) -> Self {
        Self {
            inner,
            walks: AtomicUsize::new(0),
        }
    }

    /// How many `find_batches` calls it has answered.
    pub(crate) fn walks(&self) -> usize {
        self.walks.load(Ordering::Relaxed)
    }
}

impl MaterializedIndex for CountingIndex {
    fn apply(&self, entries: &[MetadataEntry]) -> Result<()> {
        self.inner.apply(entries)
    }

    fn applied_upto(&self) -> Option<CommitVersion> {
        self.inner.applied_upto()
    }

    fn entries(&self) -> usize {
        self.inner.entries()
    }

    fn end_offset(&self, topic: &TopicId, partition: PartitionId) -> Offset {
        self.inner.end_offset(topic, partition)
    }

    fn find_batches(
        &self,
        topic: &TopicId,
        partition: PartitionId,
        start: Offset,
        max_bytes: u64,
    ) -> Result<Vec<IndexedBatch>> {
        self.walks.fetch_add(1, Ordering::Relaxed);
        self.inner.find_batches(topic, partition, start, max_bytes)
    }

    fn manifest(&self, topic: &TopicId, partition: PartitionId) -> Option<(ObjectKey, Offset)> {
        self.inner.manifest(topic, partition)
    }

    fn time_span(&self, topic: &TopicId, partition: PartitionId) -> Option<oqueue_core::TimeSpan> {
        self.inner.time_span(topic, partition)
    }

    fn log_start(&self, topic: &TopicId, partition: PartitionId) -> Offset {
        self.inner.log_start(topic, partition)
    }

    fn clear(&self) {
        self.inner.clear();
    }
}
