//! The offset→object index seam: a cache, never the source of truth.

use crate::{
    CommitVersion, IndexState, IndexedBatch, MetadataEntry, ObjectKey, Offset, PartitionId, Result,
    TimeSpan, TopicId,
};
use std::sync::Mutex;

/// A fold of the metadata log into something a fetch can query.
///
/// # ⚠️ It is a cache, and that is a contract rather than a description
///
/// `M3.md` task 9. The metadata log ([`MetadataLog`](crate::MetadataLog)) is
/// the source of truth; everything here is derivable from it by replay. So an
/// implementation may be **dropped at any moment** — under memory pressure,
/// on a restart, when `M5`'s quota gives range back — and refilling
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
/// engine choice (`SQLite`, `redb`, `RocksDB`, `fjall`, `SlateDB`) later. An
/// async read here would put object-storage reads *inside* the index, which is
/// the layering this seam exists to prevent: the index says which objects to
/// read, and the reader reads them.
///
/// ⚠️ **The allocation argument that used to stand here does not survive
/// `find_batches`** (`ADR-0022`), and is corrected rather than deleted because
/// `ADR-0020`'s `M3.5` note still records it. It ran: a boxed future per lookup
/// is the per-call allocation `ADR-0004` rejected on [`Clock`](crate::Clock) by
/// name. True of a lookup returning a scalar; not true of one returning a
/// `Vec` of entries each cloning an [`ObjectKey`](crate::ObjectKey), which is a
/// `String` — a full page costs on the order of 65 allocations against the one
/// a future would have. Being synchronous is still right; making the read side
/// allocation-free is a change to what `find_batches` returns.
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
/// 4. **[`entries`](Self::entries) is exact and cheap**, and moves only when
///    the index does: a refused `apply` leaves it where guarantee 2 leaves
///    everything else, and `clear` takes it to zero along with everything it
///    counted. ⚠️ **Named here because a number in a doc comment is a
///    suggestion and a number in this list is a contract**: `M5`'s quota is
///    enforced against it, which an approximation cannot carry, and it is read
///    **once per fold**, which a traversal cannot be. ⚠️ **Not NFR-2**, which
///    is the tail-read budget and reads this number nowhere: the fold is on
///    the produce path, under NFR-1. `M3.31` cited the wrong requirement and
///    `M3.37` corrected it. ⚠️ **Both halves, for
///    the same reason** — an implementation answering exactly by counting its
///    partition map satisfies one and breaks the other, and the conformance
///    suite asserts values and can never assert cost.
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

    /// How many entries this index holds, across every partition and tier.
    ///
    /// `ADR-0025`, with the estimate licence withdrawn by `M3.31`. ⚠️ **It
    /// must be cheap *and* exact**, and those are not in tension *for this
    /// method set*: every mutation crosses the seam — `apply` receives the
    /// entries and `clear` drops them all — so an implementation always knows
    /// the delta, and a maintained counter is both. Never a traversal of the
    /// partition map, and never a `SELECT count(*)` for doc 10 #12's engine.
    ///
    /// ⚠️ **`M5`'s eviction is what could break that**, and it is named here
    /// so the next contract decision is not a surprise: the cheap eviction for
    /// an LSM-backed engine is engine-managed — compaction dropping whole
    /// tables — which removes entries the implementor never observes, and a
    /// maintained counter would drift with nothing to correct it. Whoever adds
    /// eviction routes the delta back through the seam, or this guarantee
    /// changes with an ADR rather than quietly.
    ///
    /// ⚠️ **The estimate `ADR-0025` allowed could not have been used**, which
    /// is why it is gone rather than kept for an engine that might want it:
    /// the conformance suite has asserted this number *exactly* since the
    /// method existed, so an implementation taking the licence would fail
    /// there rather than here — long after someone chose an engine on the
    /// strength of a promise this trait had made.
    ///
    /// ⚠️ **What decides it is not that the suite may not move**, which is a
    /// judgement rather than a rule: revising an over-asserting conformance
    /// case is legitimate, and non-negotiable 2 forbids weakening a check *to
    /// make it pass*, which is a different act. What decides it is that `M5`'s
    /// quota is enforced against this number, and a quota over an
    /// approximation is not a quota. ⚠️ **`M7` verifies NFR-11 by measuring
    /// *memory*** — `requirements.md` asks for a test that node memory is flat
    /// as the **cluster-wide** partition count grows, which is a different
    /// number from this one and the requirement's whole content: cost is
    /// proportional to the partitions active on a node and never to the
    /// cluster's total. So this count is the *mechanism's*, not that
    /// verification's, and claiming otherwise would be `M3.11`'s error again.
    ///
    /// ⚠️ **It is a measurement, not a limit.** M3 bounds nothing: enforcing a
    /// ceiling at this index's keying gives back range a rebuild cannot
    /// restore, so `roadmap.md` carries the enforcement to `M5` beside the
    /// coarse per-object keying that makes a bound feasible, and `M7` is where
    /// NFR-11 is verified.
    fn entries(&self) -> usize;

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
    /// disagree, which is the bug task 13 is about.
    ///
    /// ⚠️ **The last stable offset is derived the same way and is never ahead
    /// of this one** (hazard H3, `M3.10`). Today it *is* this number: an LSO
    /// is the offset below which no transaction is still open, and until
    /// `M11` builds idempotent producers there are no transactions, so
    /// nothing can be open. It gets its own accessor when `M11` gives the two
    /// something to differ by — and when it does, the rule that survives is
    /// that it is **derived**, from the same fold, with no setter, exactly as
    /// this one is. An LSO that could be set is an LSO that can be set above
    /// the high watermark, which tells a client that records it cannot read
    /// are committed.
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

    /// The manifest naming this partition's older history, and how far it
    /// covers — `ADR-0042`'s read column.
    ///
    /// `None` for a partition no compaction has published one for, which is
    /// every partition until one does. `Some((key, upto))` means the objects
    /// holding `[0, upto)` are named by that manifest and by nothing this
    /// index still holds: [`find_batches`](Self::find_batches) returns nothing
    /// below `upto`, because the entries it would have returned are what the
    /// manifest replaced. A reader below `upto` GETs the manifest, binary-
    /// searches it, and issues one ranged GET — two reads cold, one once the
    /// manifest is held, three across a chain hop.
    ///
    /// ⚠️ **It reads, it does not resolve.** The manifest is an object and
    /// this seam is synchronous by contract (see above), so what comes back is
    /// where to look rather than what is there. Resolving it inside the index
    /// would put object-storage reads behind a local fold, which is the
    /// layering this seam exists to prevent.
    fn manifest(&self, topic: &TopicId, partition: PartitionId) -> Option<(ObjectKey, Offset)>;

    /// The oldest and newest commit time folded for this partition
    /// (`ADR-0044`).
    ///
    /// ⚠️ **`None` means nothing was committed to it**, not "committed at the
    /// epoch": a retention round reading `EPOCH` for an uncommitted partition
    /// would reap it on its first sweep. Answered from memory — a round asks
    /// it of every partition it holds.
    fn time_span(&self, topic: &TopicId, partition: PartitionId) -> Option<TimeSpan>;

    /// The first readable offset: zero until a trim moves it (`ADR-0044`).
    fn log_start(&self, topic: &TopicId, partition: PartitionId) -> Offset;

    /// How many index entries name `object` (`ADR-0045`). Zero is the
    /// precondition for deleting it; ⚠️ an object absorbed into a partition
    /// manifest never reaches zero, because this index does not read
    /// manifests and so cannot know the manifest stopped naming it.
    fn references(&self, object: &ObjectKey) -> usize;

    /// Discards everything, returning it to its fresh state.
    ///
    /// ⚠️ Safe **for the writer of this index**, and the reason this trait
    /// exists: nothing here is unrecoverable, because the log can refill it.
    ///
    /// ⚠️ **It is not safe for anyone else**, and an index therefore has one
    /// writer — `ADR-0024`, which is where the reasoning lives rather than
    /// repeated here. In short: [`apply`](Self::apply) checks version *order*
    /// and not contiguity, so a clear landing between a writer's own "is this
    /// current" check and its fold is accepted, and every forgotten partition
    /// re-bases at [`Offset::ZERO`].
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
