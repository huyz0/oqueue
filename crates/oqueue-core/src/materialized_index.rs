//! The offset→object index seam: a cache, never the source of truth.

use crate::{
    CommitVersion, CommittedSpan, Error, MetadataEntry, MetadataRecord, ObjectKey, ObjectRef,
    Offset, PartitionId, Result, TailEntry, TopicId,
};
use std::collections::{HashMap, VecDeque};
use std::sync::Mutex;

/// How many entries a partition keeps in the tail tier, byte ranges inline.
///
/// ⚠️ **UNDERIVED, and `M3.md` says so.** The tail/history split is sized
/// against an assumed workload — doc 10 #25, which `M3.md`'s Risks section
/// names as gating this milestone's arithmetic. This value is a placeholder
/// with the right *shape* and not a derived constant; nothing may cite it as
/// one, and the milestone that derives NFR-13 is what replaces it.
///
/// ⚠️ **Bounded per partition, which is not bounded.** The window caps the
/// expensive tier for one partition; the partition map itself has no cap, so
/// steady-state cost still scales with partitions active on the node. That is
/// NFR-11's concern, and `M3.11`'s quota is what has to face it.
///
/// It is a constant rather than a knob, per `AGENTS.md` non-negotiable 2.
pub const TAIL_WINDOW_ENTRIES: usize = 128;

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
/// One partition's two tiers, and the offset they fold to.
///
/// ⚠️ **Two collections rather than one of a two-variant enum.** An enum is as
/// large as its widest variant, so a history entry would pay for the tail's
/// inline [`ByteRange`](crate::ByteRange) whether or not it carried one.
/// Separate collections keep an [`ObjectRef`] at its ~40-byte inline budget.
///
/// ⚠️ This is the two-tier *shape* and not yet the coarse index doc 15 §7
/// resolves doc 10 #8 to: entries are still keyed per (object, partition), so
/// the count is doc 14 §3's ~4M/s row however lean each one is. See
/// [`ObjectRef`] for what closing that would take.
///
/// ⚠️ `Default` by hand rather than derived: `Offset` deliberately has no
/// `Default`, because "the zero offset" and "no offset" are different claims
/// and a derive would quietly pick one. A fresh partition genuinely starts at
/// [`Offset::ZERO`], and saying so here is the only place that choice is made.
#[derive(Debug)]
struct PartitionIndex {
    /// Where the next record lands — the fold of every span's count.
    end_offset: Offset,
    /// The hot window: byte ranges inline, so a tail read is one GET.
    tail: VecDeque<TailEntry>,
    /// Everything older: refs only, ranges resolved from each object's own
    /// footer at 1–3 GETs.
    history: Vec<ObjectRef>,
}

impl Default for PartitionIndex {
    fn default() -> Self {
        Self {
            end_offset: Offset::ZERO,
            tail: VecDeque::new(),
            history: Vec::new(),
        }
    }
}

impl PartitionIndex {
    /// Pushes a newly committed object onto the tail, demoting whatever falls
    /// out of the window.
    /// ⚠️ `if`, not `while`: entries arrive one at a time, so at most one can
    /// fall out of the window per push. A loop here would be an unbounded one
    /// whose bound is a comparison — and a mutation flipping that comparison
    /// turns it into a hang rather than a failure, which is a worse way to
    /// find out.
    fn push(&mut self, entry: TailEntry) {
        self.tail.push_back(entry);
        if self.tail.len() > TAIL_WINDOW_ENTRIES
            && let Some(evicted) = self.tail.pop_front()
        {
            self.history.push(evicted.demote());
        }
    }
}

/// The state every in-memory materialization keeps, and the fold over it.
///
/// ⚠️ Shared by [`FakeMaterializedIndex`] and by `oqueue-index`'s `MemoryIndex`
/// rather than written twice. Two copies of a fold this subtle — order
/// checking, all-or-nothing application, non-wrapping addition, tier demotion
/// — would drift, and the conformance suite would then be asserting the
/// contract against two different meanings of it.
///
/// ⚠️ **Nested rather than keyed by `(TopicId, PartitionId)`.** A tuple key has
/// no borrowed form, so every lookup would have to `clone()` the topic name to
/// build one — a heap allocation on the Fetch path NFR-2 and NFR-3 bound, and
/// the very per-call allocation this seam refuses to be async in order to
/// avoid. Nested, a read borrows.
#[derive(Debug, Default)]
pub struct IndexState {
    applied_upto: Option<CommitVersion>,
    partitions: HashMap<TopicId, HashMap<PartitionId, PartitionIndex>>,
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
        // Staged as (running end offset, entries this batch adds), so a
        // failure part-way through commits neither — guarantee 2.
        let mut staged: HashMap<(&TopicId, PartitionId), (Offset, Vec<TailEntry>)> = HashMap::new();

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
                MetadataRecord::BatchCommitted { object, spans } => {
                    for span in spans {
                        self.stage_span(&mut staged, object, span)?;
                    }
                }
                // An event about the log, not about any partition.
                MetadataRecord::EpochChanged { .. } => {}
            }
        }

        // ⚠️ The only mutation of `self` in this method, and it is after every
        // fallible step — guarantee 2. The clone happens here, on the write
        // path, rather than on the read path a lookup key would have put it.
        for ((topic, partition), (end, entries)) in staged {
            let slot = self
                .partitions
                .entry(topic.clone())
                .or_default()
                .entry(partition)
                .or_default();
            slot.end_offset = end;
            for entry in entries {
                slot.push(entry);
            }
        }
        self.applied_upto = previous;
        Ok(())
    }

    /// Stages one span's contribution, without touching `self`.
    ///
    /// ⚠️ The base offset comes from the **staged** running value first and
    /// only then from what is already committed, which is what makes a
    /// partition appearing twice in one batch accumulate rather than restart.
    /// `M3.8` commits every N ≥ 1,000 entries, so that is its ordinary case.
    fn stage_span<'a>(
        &self,
        staged: &mut HashMap<(&'a TopicId, PartitionId), (Offset, Vec<TailEntry>)>,
        object: &ObjectKey,
        span: &'a CommittedSpan,
    ) -> Result<()> {
        let key = (span.topic(), span.partition());
        let base = staged
            .get(&key)
            .map(|(end, _)| *end)
            .or_else(|| {
                self.partitions
                    .get(span.topic())
                    .and_then(|parts| parts.get(&span.partition()))
                    .map(|p| p.end_offset)
            })
            .unwrap_or(Offset::ZERO);
        // ⚠️ `Offset::add`, not `+`: a wrapped offset is smaller than the one
        // before it, and every later comparison is then wrong.
        let next = base.add(i64::from(span.record_count()))?;
        let entry = TailEntry::new(
            ObjectRef::new(object.clone(), base, span.record_count()),
            span.bytes(),
        );
        let slot = staged.entry(key).or_insert((base, Vec::new()));
        slot.0 = next;
        slot.1.push(entry);
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
        self.partition(topic, partition)
            .map_or(Offset::ZERO, |p| p.end_offset)
    }

    /// The partition's tail window: entries carrying inline byte ranges, so
    /// reading one is a single GET.
    #[must_use]
    pub fn tail(&self, topic: &TopicId, partition: PartitionId) -> Vec<TailEntry> {
        self.partition(topic, partition)
            .map(|p| p.tail.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// How many entries the partition has demoted to the history tier.
    ///
    /// ⚠️ A count rather than the entries themselves: resolving one needs the
    /// object's own footer, which is the read path `M3.8` builds and not this
    /// type's to perform.
    #[must_use]
    pub fn history_len(&self, topic: &TopicId, partition: PartitionId) -> usize {
        self.partition(topic, partition)
            .map_or(0, |p| p.history.len())
    }

    fn partition(&self, topic: &TopicId, partition: PartitionId) -> Option<&PartitionIndex> {
        self.partitions
            .get(topic)
            .and_then(|parts| parts.get(&partition))
    }

    /// Returns it to its fresh state.
    pub fn clear(&mut self) {
        self.applied_upto = None;
        self.partitions.clear();
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
