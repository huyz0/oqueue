//! The offset→object index seam: a cache, never the source of truth.

use crate::{
    CommitVersion, IndexState, IndexedBatch, MetadataEntry, Offset, PartitionId, Result, TopicId,
};
use std::sync::Mutex;

/// A fold of the metadata log into something a fetch can query.
///
/// # ⚠️ It is a cache, and that is a contract rather than a description
///
/// `M3.md` task 9. The metadata log ([`MetadataLog`](crate::MetadataLog)) is
/// the source of truth; everything here is derivable from it by replay. So an
/// implementation may be **dropped at any moment** — under memory pressure,
/// on a restart, when `M3.11`'s quota trips its degraded mode — and refilling
/// it from the log must reproduce it exactly. Nothing may be stored here that
/// the log cannot put back, and nothing may treat this as authoritative.
///
/// ⚠️ **There is no setter.** Offsets are not assigned here; they are the
/// running sum of the record counts the log's [`CommittedSpan`]s carry, which
/// is what makes them gap-free without an allocator having to be careful
/// (`M3.md` task 13 — the high watermark `M3.8` derives has no setter for the
/// same reason).
///
/// [`CommittedSpan`]: crate::CommittedSpan
///
/// # Why this is not async
///
/// Unlike [`ObjectStore`](crate::ObjectStore), which is a network seam, every
/// implementation of this is a local fold — memory today, and doc 10 #12's
/// engine choice (`SQLite`, `redb`, `RocksDB`, `fjall`, `SlateDB`) later. The
/// read side is on the Fetch hot path that NFR-2 and NFR-3 bound, and a boxed
/// future per lookup is the per-call allocation `ADR-0004` rejected on
/// [`Clock`](crate::Clock) by name.
///
/// ⚠️ **This is the seam's open question, not a settled one.** A disk-backed
/// engine may want an async read, and if it does, that is a contract change
/// with an ADR — doc 10 #12 is where it would be argued, and `M6` owns it.
///
/// # What an implementor must guarantee
///
/// 1. **Deltas fold in strictly increasing [`CommitVersion`] order.** Out of
///    order is refused with [`Error::NonMonotonicCommitVersion`](crate::Error::NonMonotonicCommitVersion), never
///    sorted — a replayed version folded twice double-counts its records
///    (`M3.md` task 14).
/// 2. ⚠️ **A refused `apply` folds nothing**, not even the entries before the
///    offending one, or a caller retrying after a rejection folds onto an
///    index already carrying part of that batch.
/// 3. **`clear` returns it to its fresh state**, and replaying the same log
///    reproduces the same observable state.
pub trait MaterializedIndex: Send + Sync + core::fmt::Debug {
    /// Folds a batch of log entries in.
    ///
    /// # Errors
    ///
    /// [`Error::NonMonotonicCommitVersion`](crate::Error::NonMonotonicCommitVersion) if the batch is not strictly
    /// increasing, or does not follow what is already applied. ⚠️ On this
    /// error the index is unchanged.
    ///
    /// [`Error::OffsetOverflow`](crate::Error::OffsetOverflow) if a partition's running sum would leave the
    /// protocol's `i64` range.
    fn apply(&self, entries: &[MetadataEntry]) -> Result<()>;

    /// The highest version folded in, or `None` if nothing has been.
    fn applied_upto(&self) -> Option<CommitVersion>;

    /// The offset the next record for this partition will occupy — which is
    /// also its **high watermark**.
    ///
    /// [`Offset::ZERO`] for a partition nothing has been folded for, which is
    /// the same answer as for one that exists and is empty — the index does
    /// not know which topics exist, only what the log has said about them.
    ///
    /// ⚠️ **`M3.md` task 13 asks for a derived high watermark with no setter,
    /// and this is it rather than a second method** (`ADR-0022`). Nothing
    /// enters this index before its metadata record commits, so "the end of
    /// the committed log" and "where the next record lands" are one number.
    /// A second name for it would be the first opportunity for the two to
    /// disagree, which is the bug task 13 is about. ⚠️ `M3.10`'s last stable
    /// offset is a genuinely different number — never *ahead* of this one —
    /// and does need its own accessor when transactions exist to part them.
    fn end_offset(&self, topic: &TopicId, partition: PartitionId) -> Offset;

    /// The objects a fetch from `start` must read, in ascending offset order.
    ///
    /// `M3.md` task 12, and the reason this seam exists: FR-13 and NFR-30 say
    /// a reader resolves offset→object through the index and never enumerates
    /// object storage, so this answers without a single store call.
    ///
    /// Empty for a partition the index has not folded, and for a `start` at or
    /// past the [`end_offset`](Self::end_offset) — which is the FR-12 case
    /// that must cost **zero GETs**.
    ///
    /// # The byte budget, and what it is a budget of
    ///
    /// ⚠️ **`max_bytes` is a bound the index honours where it can, and the
    /// caller is what actually enforces the budget** (`ADR-0022`). A batch
    /// whose length [is knowable](IndexedBatch::known_len) is charged against
    /// it; one whose length is not charges nothing, because learning that
    /// length *is* the footer read the budget exists to bound. What bounds a
    /// page of those is
    /// [`MAX_BATCHES_PER_PAGE`](crate::MAX_BATCHES_PER_PAGE) — without it a
    /// cold read would name the whole of a partition's history, and with it
    /// NFR-30's "bounded GETs, zero LIST" has a number behind it.
    ///
    /// At least one batch is returned whenever one exists, so a partition
    /// whose next object exceeds `max_bytes` still lets the consumer advance.
    /// What comes back is the ordered list of objects a fetch **may** read;
    /// the reader stops when its own budget fills.
    ///
    /// # Errors
    ///
    /// [`Error::OffsetOverflow`](crate::Error::OffsetOverflow) if a stored entry's end offset is
    /// unrepresentable, which means the fold that produced it was already
    /// wrong.
    fn find_batches(
        &self,
        topic: &TopicId,
        partition: PartitionId,
        start: Offset,
        max_bytes: u64,
    ) -> Result<Vec<IndexedBatch>>;

    /// Discards everything, returning it to its fresh state.
    ///
    /// ⚠️ Safe by construction, and the reason this trait exists: a caller may
    /// do this whenever it likes, because the log can refill it.
    fn clear(&self);
}

/// An in-memory [`MaterializedIndex`], faithful to the documented contract.
///
/// ⚠️ Here rather than in `oqueue-testkit` because `contracts.md` rule 9 puts
/// a fake beside its trait: a downstream crate testing against this must not
/// have to depend on `oqueue-index`.
pub struct FakeMaterializedIndex {
    state: Mutex<IndexState>,
}

impl FakeMaterializedIndex {
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

impl Default for FakeMaterializedIndex {
    fn default() -> Self {
        Self::new()
    }
}

/// ⚠️ Renders how far it has folded, never the partitions it holds — a `Debug`
/// line in a test failure must not become a listing of a tenant's topics.
impl core::fmt::Debug for FakeMaterializedIndex {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FakeMaterializedIndex")
            .field("applied_upto", &self.lock().applied_upto())
            .finish()
    }
}

impl MaterializedIndex for FakeMaterializedIndex {
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
