//! The in-memory materialization of the offset→object index.

use oqueue_core::{
    CommitVersion, IndexState, IndexedBatch, MaterializedIndex, MetadataEntry, ObjectKey, Offset,
    PartitionId, Result, TimeSpan, TopicId,
};
use std::sync::Mutex;

/// The index the broker holds: the metadata log, folded into what a fetch can
/// query.
///
/// ⚠️ **Identical to [`FakeMaterializedIndex`](oqueue_core::FakeMaterializedIndex)
/// today, and that is a fact about now rather than a design.** Both are thin
/// `Mutex<IndexState>` wrappers over the one fold in `oqueue-core`, so the
/// conformance suite runs one implementation twice — M3's checkpoint review
/// found the documents claiming otherwise (`M3.24`). What separates them is
/// coming and is not cosmetic — a broker's working set gets a quota and an
/// eviction policy that a downstream crate's test double must not — but it is
/// coming at `M5`, not at `M3.11`: enforcing a ceiling on this index's keying
/// gives back range a rebuild cannot restore, so `roadmap.md` carries the
/// enforcement to `M5` beside the coarse keying that makes it feasible.
///
/// ⚠️ **A cache, and droppable at any moment.** Everything here is derivable
/// from the log by replay, which is what lets a writer discard it under
/// pressure rather than fail. See
/// [`MaterializedIndex`](oqueue_core::MaterializedIndex) for the contract this
/// is held to; the conformance suite in `tests/it/index.rs` runs that contract
/// against this type and against the fake beside the trait.
///
/// ⚠️ **In-memory is `M3`'s answer, not the project's.** Doc 10 #12 — which of
/// `SQLite`, `redb`, `RocksDB`, `fjall` or `SlateDB` — is open, and doc 13 §6
/// asks for a benchmark rather than an argument. The index growth arithmetic
/// in `M3.md` (~14 GB/day, ~97 GB over a 7-day retention) is why that question
/// exists: this type does not fit that, and is not meant to.
pub struct MemoryIndex {
    state: Mutex<IndexState>,
}

impl MemoryIndex {
    /// A fresh, empty index.
    #[must_use]
    pub fn new() -> Self {
        Self {
            state: Mutex::new(IndexState::new()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, IndexState> {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

impl Default for MemoryIndex {
    fn default() -> Self {
        Self::new()
    }
}

/// ⚠️ Renders how far it has folded, never the partitions it holds.
impl core::fmt::Debug for MemoryIndex {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("MemoryIndex")
            .field("applied_upto", &self.lock().applied_upto())
            .finish()
    }
}

impl MaterializedIndex for MemoryIndex {
    fn apply(&self, entries: &[MetadataEntry]) -> Result<()> {
        self.lock().apply(entries)
    }

    fn applied_upto(&self) -> Option<CommitVersion> {
        self.lock().applied_upto()
    }

    fn entries(&self) -> usize {
        self.lock().entries()
    }

    fn end_offset(&self, topic: &TopicId, partition: PartitionId) -> Offset {
        self.lock().end_offset(topic, partition)
    }

    fn find_batches(
        &self,
        topic: &TopicId,
        partition: PartitionId,
        start: Offset,
        max_bytes: u64,
    ) -> Result<Vec<IndexedBatch>> {
        self.lock().find_batches(topic, partition, start, max_bytes)
    }

    fn manifest(&self, topic: &TopicId, partition: PartitionId) -> Option<(ObjectKey, Offset)> {
        self.lock().manifest(topic, partition)
    }

    fn time_span(&self, topic: &TopicId, partition: PartitionId) -> Option<TimeSpan> {
        self.lock().time_span(topic, partition)
    }

    fn log_start(&self, topic: &TopicId, partition: PartitionId) -> Offset {
        self.lock().log_start(topic, partition)
    }

    fn references(&self, object: &ObjectKey) -> usize {
        self.lock().references(object)
    }

    fn clear(&self) {
        self.lock().clear();
    }
}
