//! What a reader is given: the index, minus every way to write to it.

use crate::{CommitVersion, IndexedBatch, MaterializedIndex, Offset, PartitionId, Result, TopicId};
use std::sync::Arc;

/// A read-only handle on an index somebody else writes.
///
/// ⚠️ **`ADR-0024`, and the point is what is *absent*.**
/// [`MaterializedIndex`]'s `apply` and `clear` are not reachable through this
/// type, so a reader given one is not the second writer by accident.
/// ⚠️ **"By accident" is the honest claim** and the ADR's "no handle left" is
/// the overstatement: nothing stops a caller implementing `MaterializedIndex`
/// on a newtype that delegates to an [`Arc`] it keeps. What changes is that the
/// writable handle stops being what the API hands you and becomes a wrapper
/// somebody had to mean to write. Two writers on one index is not a race that loses a
/// write: `apply` checks version *order* and not contiguity, so a `clear`
/// landing between the coordinator's own check and its fold is **accepted**,
/// and every forgotten partition re-bases at [`Offset::ZERO`] while the log and
/// the ack already given to the producer place those records far higher. An
/// index that is wrong is worse than one that is empty, and nothing detects it.
///
/// ⚠️ **Dropping the cache stays available to the writer**, and for a
/// coordinator's index that is `Coordinator::drop_cache`, which queues it
/// behind the folds it must not interleave with. `M5`'s quota is the row that
/// will want it — `M3.11` found a ceiling unachievable at this index's keying,
/// so `roadmap.md` carries the enforcement there with the re-keying.
#[derive(Clone)]
pub struct IndexReader {
    index: Arc<dyn MaterializedIndex>,
}

impl IndexReader {
    /// Wraps an index in a handle that can only read it.
    ///
    /// ⚠️ Whoever calls this **is** the writer — it takes the `Arc` the writer
    /// holds. Handing one out is how a writer publishes its index without
    /// publishing the ability to write it.
    #[must_use]
    pub const fn new(index: Arc<dyn MaterializedIndex>) -> Self {
        Self { index }
    }

    /// The highest version folded in, or `None` if nothing has been.
    ///
    /// ⚠️ **What the index holds, which is not always what the log holds.** A
    /// cache that was rebuilt, or emptied after a failed rebuild, reports what
    /// it actually has — `oqueue-coordinator`'s watch reads back from this same
    /// place for the same reason.
    #[must_use]
    pub fn applied_upto(&self) -> Option<CommitVersion> {
        self.index.applied_upto()
    }

    /// How many entries the index holds, across every partition and tier.
    ///
    /// `ADR-0025`, and the reason it is on the *reader*: `ADR-0024` moved a
    /// coordinator's index behind this handle, so without it the one
    /// materialization that folds every partition on a shard is the one nobody
    /// can measure. ⚠️ **A measurement, not a limit** — nothing in M3 bounds
    /// it; `roadmap.md` carries the enforcement to `M5`.
    ///
    /// ⚠️ **Exact**, per [`MaterializedIndex`]'s guarantee 4: this is the
    /// number `M5`'s quota is enforced against, and a quota over an
    /// approximation is not one.
    #[must_use]
    pub fn entries(&self) -> usize {
        self.index.entries()
    }

    /// Where the next record for this partition lands — its high watermark.
    #[must_use]
    pub fn end_offset(&self, topic: &TopicId, partition: PartitionId) -> Offset {
        self.index.end_offset(topic, partition)
    }

    /// The objects a fetch from `start` must read, in ascending offset order.
    ///
    /// See [`MaterializedIndex::find_batches`] for the paging rule, and
    /// `ADR-0022` for why `max_bytes` bounds what the index can price rather
    /// than what the reader will fetch.
    ///
    /// # Errors
    ///
    /// [`Error::OffsetOverflow`](crate::Error::OffsetOverflow) if a stored
    /// entry's end offset is unrepresentable.
    pub fn find_batches(
        &self,
        topic: &TopicId,
        partition: PartitionId,
        start: Offset,
        max_bytes: u64,
    ) -> Result<Vec<IndexedBatch>> {
        self.index.find_batches(topic, partition, start, max_bytes)
    }
}

/// ⚠️ Renders how far it has folded, never the partitions it holds — a `Debug`
/// line in a log or a test failure must not become a listing of a tenant's
/// topics.
impl core::fmt::Debug for IndexReader {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("IndexReader")
            .field("applied_upto", &self.applied_upto())
            .finish()
    }
}
