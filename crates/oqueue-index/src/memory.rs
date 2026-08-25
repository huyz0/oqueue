//! The in-memory materialization of the offset→object index.

use oqueue_core::{
    CommitVersion, IndexState, IndexedBatch, MaterializedIndex, MetadataEntry, Offset, PartitionId,
    Result, TopicId,
};
use std::sync::Mutex;

/// The index the broker holds: the metadata log, folded into what a fetch can
/// query.
///
/// ⚠️ **A cache, and droppable at any moment.** Everything here is derivable
/// from the log by replay, which is what lets `M3.11`'s degraded mode discard
/// it under quota pressure rather than fail. See
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

    fn clear(&self) {
        self.lock().clear();
    }
}
