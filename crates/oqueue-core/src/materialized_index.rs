//! The offset→object index seam: a cache, never the source of truth.

use crate::{
    CommitVersion, Error, MetadataEntry, MetadataRecord, Offset, PartitionId, Result, TopicId,
};
use std::collections::HashMap;
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
///    order is refused with [`Error::NonMonotonicCommitVersion`], never
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
    /// [`Error::NonMonotonicCommitVersion`] if the batch is not strictly
    /// increasing, or does not follow what is already applied. ⚠️ On this
    /// error the index is unchanged.
    ///
    /// [`Error::OffsetOverflow`] if a partition's running sum would leave the
    /// protocol's `i64` range.
    fn apply(&self, entries: &[MetadataEntry]) -> Result<()>;

    /// The highest version folded in, or `None` if nothing has been.
    fn applied_upto(&self) -> Option<CommitVersion>;

    /// The offset the next record for this partition will occupy.
    ///
    /// [`Offset::ZERO`] for a partition nothing has been folded for, which is
    /// the same answer as for one that exists and is empty — the index does
    /// not know which topics exist, only what the log has said about them.
    fn end_offset(&self, topic: &TopicId, partition: PartitionId) -> Offset;

    /// Discards everything, returning it to its fresh state.
    ///
    /// ⚠️ Safe by construction, and the reason this trait exists: a caller may
    /// do this whenever it likes, because the log can refill it.
    fn clear(&self);
}

/// The state every in-memory materialization keeps, and the fold over it.
///
/// ⚠️ Shared by [`FakeMaterializedIndex`] and by `oqueue-index`'s
/// `MemoryIndex` rather than written twice. Two copies of a fold this subtle —
/// order checking, all-or-nothing application, non-wrapping addition — would
/// drift, and the conformance suite would then be asserting the contract
/// against two different meanings of it.
/// ⚠️ **Nested rather than keyed by `(TopicId, PartitionId)`.** A tuple key has
/// no borrowed form, so every lookup would have to `clone()` the topic name to
/// build one — a heap allocation on the Fetch path NFR-2 and NFR-3 bound, and
/// the very per-call allocation this seam refuses to be async in order to
/// avoid. Nested, a read borrows.
#[derive(Debug, Default)]
pub struct IndexState {
    applied_upto: Option<CommitVersion>,
    end_offsets: HashMap<TopicId, HashMap<PartitionId, Offset>>,
}

impl IndexState {
    /// An empty state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Guarantee 1 and 2: validate the whole batch, then fold it.
    ///
    /// # Errors
    ///
    /// [`Error::NonMonotonicCommitVersion`] or [`Error::OffsetOverflow`], with
    /// the state left untouched in either case.
    pub fn apply(&mut self, entries: &[MetadataEntry]) -> Result<()> {
        // ⚠️ Computed into a scratch map first, so a failure part-way through
        // cannot leave a partially folded batch behind — guarantee 2. The
        // scratch holds only the partitions this batch touches.
        let mut previous = self.applied_upto;
        let mut staged: HashMap<(&TopicId, PartitionId), Offset> = HashMap::new();

        for entry in entries {
            if let Some(prev) = previous
                && entry.version() <= prev
            {
                return Err(Error::NonMonotonicCommitVersion {
                    expected_above: prev.get(),
                    got: entry.version().get(),
                });
            }
            previous = Some(entry.version());

            match entry.record() {
                MetadataRecord::BatchCommitted { spans, .. } => {
                    for span in spans {
                        let key = (span.topic(), span.partition());
                        let current = staged
                            .get(&key)
                            .copied()
                            .or_else(|| {
                                self.end_offsets
                                    .get(span.topic())
                                    .and_then(|parts| parts.get(&span.partition()))
                                    .copied()
                            })
                            .unwrap_or(Offset::ZERO);
                        // ⚠️ `Offset::add`, not `+`: a wrapped offset is
                        // smaller than the one before it, and every later
                        // comparison is then wrong.
                        let next = current.add(i64::from(span.record_count()))?;
                        staged.insert(key, next);
                    }
                }
                // An event about the log, not about any partition.
                MetadataRecord::EpochChanged { .. } => {}
            }
        }

        // ⚠️ The only mutation of `self` in this method, and it is after every
        // fallible step — guarantee 2. The clone happens here, on the write
        // path, rather than on the read path a lookup key would have put it.
        for ((topic, partition), end) in staged {
            self.end_offsets
                .entry(topic.clone())
                .or_default()
                .insert(partition, end);
        }
        self.applied_upto = previous;
        Ok(())
    }

    /// The highest version folded in.
    #[must_use]
    pub const fn applied_upto(&self) -> Option<CommitVersion> {
        self.applied_upto
    }

    /// The offset the next record for this partition will occupy.
    #[must_use]
    pub fn end_offset(&self, topic: &TopicId, partition: PartitionId) -> Offset {
        self.end_offsets
            .get(topic)
            .and_then(|parts| parts.get(&partition))
            .copied()
            .unwrap_or(Offset::ZERO)
    }

    /// Returns it to its fresh state.
    pub fn clear(&mut self) {
        self.applied_upto = None;
        self.end_offsets.clear();
    }
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

    fn clear(&self) {
        self.lock().clear();
    }
}
