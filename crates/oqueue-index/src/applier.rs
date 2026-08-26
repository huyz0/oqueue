//! Pumping the metadata log into the index, a bounded batch at a time.

use oqueue_core::{CommitVersion, IndexReader, MaterializedIndex, MetadataLog, Result};
use std::sync::Arc;

/// The most log entries one apply covers.
///
/// ⚠️ **A ceiling on the transaction, which is what bounds the transaction
/// *rate*** (`M3.md` task 10). ⚠️ Doc 13 §6 makes the measured claim for
/// **`SQLite`** — ~142 individual commits/s, which "would not meet our rate,
/// so batching is mandatory, not optional" — and for the other four of doc 10
/// #12's candidates it publishes no individual-commit figure at all; of `redb`
/// it records the opposite direction, that it "wins single-record commits by
/// 2× and loses batched commits by 4.5×". The engine benchmark doc 13 §6 asks
/// for is still unrun and doc 10 #12 still open, so the general reason is the
/// mechanism rather than the measurement: every candidate pays a durable-commit
/// cost per *transaction* that a batch amortizes and a single entry cannot.
/// Capping entries per transaction at N caps transactions at
/// `arrival rate ÷ N`: doc 14 §3's ~4M entries/s becomes ~4,000 commits/s
/// rather than ~4M, and that division is the whole of what task 10 buys.
///
/// ⚠️ **It does not make an apply cover *at least* N entries, and nothing here
/// does.** A log with three new entries commits three. That is not the
/// pathology task 10 names — an engine asked for three commits a second is not
/// being asked to sustain anything — but it does mean the amortization is a
/// property of the arrival rate rather than of this constant, and a *poller*
/// that polls faster than entries arrive would defeat it. Choosing that poll
/// cadence is `M3.9`'s, which is where the freshness half of the same trade
/// (`ADR-0021`'s 5 s) is decided.
///
/// ⚠️ It is also the **bound on replay after a crash**: an apply is
/// all-or-nothing, so the most a restart can have to redo is one batch's worth
/// of entries plus whatever arrived since. Raising it trades apply throughput
/// for restart time, which is why it is a constant rather than a knob
/// (`AGENTS.md` non-negotiable 2) and why `M6`, which owns RTO, is where it
/// would be re-derived.
pub const APPLY_BATCH_ENTRIES: usize = 1_000;

/// What one [`LogApplier::catch_up`] cost.
///
/// ⚠️ **Neither count is decoration**, and they are the two terms `M6`'s RTO is
/// actually measured in. An engine's durable cost is per *transaction*, not per
/// entry — the whole reason [`APPLY_BATCH_ENTRIES`] exists — and a log's cost
/// is per *read*. They differ by exactly one whenever the last page came back
/// short, which is the round trip `catch_up`'s short-page exit saves and the
/// only thing that makes that exit observable rather than a claim.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CatchUp {
    /// How many log entries were folded.
    pub entries: usize,
    /// How many durable apply transactions that took.
    pub batches: usize,
    /// How many reads of the log it took to find them.
    pub reads: usize,
}

/// Folds a [`MetadataLog`] into a [`MaterializedIndex`], resuming from
/// whatever the index has already folded.
///
/// # Why the index is the bookmark
///
/// ⚠️ **`applied_upto` is not tracked here, and that is `M3.md` task 11.** The
/// applier holds no cursor of its own: where to resume is read back from the
/// index, which advances it inside the same all-or-nothing `apply` as the
/// entries it covers. A cursor kept beside the index would be a second copy of
/// the same fact, updated by a second write that a crash can land between —
/// and the two failure orders are opposite bugs. Ahead of the index, entries
/// are skipped and the fold silently loses records; behind it, entries are
/// replayed and the fold refuses them as non-monotonic. Reading the bookmark
/// out of the thing it describes makes both unrepresentable.
///
/// # What a restart costs
///
/// A warm restart replays `committed − applied_upto`, never the whole log
/// (`M3.md` task 11). A crash mid-apply replays at most
/// [`APPLY_BATCH_ENTRIES`] of already-folded entries, because the batch it
/// interrupted folded nothing.
/// ⚠️ **Both sides behind `Arc<dyn …>`**, for the two reasons that also put
/// `ObjectStore` behind one: doc 10 #12's engine is chosen at startup rather
/// than at compile time, and the index folded here is the same object a Fetch
/// path queries. ⚠️ **That second handle is an
/// [`IndexReader`](Self::index), not another `Arc`** (`ADR-0024`): this
/// applier is the index's sole writer, and a reader that could also write is
/// the wrong-offsets race rather than a convenience.
#[derive(Debug)]
pub struct LogApplier {
    log: Arc<dyn MetadataLog>,
    index: Arc<dyn MaterializedIndex>,
}

impl LogApplier {
    /// Pairs a log with the index folding it.
    ///
    /// ⚠️ **An `Arc`, unlike `Coordinator::open`'s `Box`, and the asymmetry is
    /// deliberate** (`ADR-0024` decision 2). Taking a `Box` would make this
    /// applier the index's only owner, and the one thing an applier has to be
    /// able to express is an index that **outlives** it — a warm restart is a
    /// new applier over the state that survived, which is exactly what
    /// `tests/it/applier.rs`'s restart case exercises.
    ///
    /// ⚠️ So the sole-writer rule is *not* type-checked on this side. What
    /// changed is the need: [`index`](Self::index) answers every read, so a
    /// caller keeping the `Arc` is doing something deliberate rather than
    /// taking the path the API offered. A coordinator's caller had no such
    /// choice, which is why that half moved to a `Box`.
    #[must_use]
    pub const fn new(log: Arc<dyn MetadataLog>, index: Arc<dyn MaterializedIndex>) -> Self {
        Self { log, index }
    }

    /// A read-only handle on the index being folded into.
    ///
    /// ⚠️ **A reader, not the `Arc`** — `ADR-0024`. This applier is the sole
    /// writer of that index, and handing out something carrying `apply` and
    /// `clear` would make every caller a candidate second writer: a clear
    /// landing between [`resume_from`](Self::resume_from) and the fold is
    /// *accepted*, because an emptied index has no `applied_upto` to refuse
    /// against, and every forgotten partition re-bases at
    /// [`Offset::ZERO`](oqueue_core::Offset::ZERO) while the log holds those
    /// records far higher.
    #[must_use]
    pub fn index(&self) -> IndexReader {
        IndexReader::new(Arc::clone(&self.index))
    }

    /// Discards the materialization, which the log can refill.
    ///
    /// ⚠️ **The writer's own door** — `ADR-0024`. The index is not reachable as
    /// something clearable through [`index`](Self::index), so dropping the
    /// cache is asked of whoever writes it; here that is this applier, and for
    /// a coordinator's index it is `Coordinator::drop_cache`. A clear that took
    /// any other route could land between
    /// [`resume_from`](Self::resume_from) and the fold, where it is *accepted*
    /// and re-bases every forgotten partition at zero.
    ///
    /// The next [`catch_up`](Self::catch_up) refills from the log, starting
    /// over because the bookmark went with the cache.
    pub fn drop_cache(&self) {
        self.index.clear();
    }

    /// The version the next batch will start reading at.
    ///
    /// [`CommitVersion::ZERO`] for an index that has folded nothing — a fresh
    /// one, or one that was dropped and is being refilled.
    ///
    /// # Errors
    ///
    /// [`Error::CommitVersionOverflow`](oqueue_core::Error::CommitVersionOverflow)
    /// if the index has folded `u64::MAX`, so no version follows it.
    pub fn resume_from(&self) -> Result<CommitVersion> {
        self.index
            .applied_upto()
            .map_or(Ok(CommitVersion::ZERO), |applied| applied.advance(1))
    }

    /// Folds one batch of at most [`APPLY_BATCH_ENTRIES`] entries.
    ///
    /// Returns how many were folded — `0` when the index has caught up.
    ///
    /// ⚠️ **One writer per index, and this is it** — `ADR-0024`. The method
    /// reads the bookmark, awaits the log, and then folds, so a second writer
    /// landing in between makes the loser's fold refused as
    /// [`NonMonotonicCommitVersion`](oqueue_core::Error::NonMonotonicCommitVersion),
    /// an error saying the log lost ordering when nothing is wrong with it.
    /// A `Coordinator`'s index is now unreachable as an
    /// [`Arc<dyn MaterializedIndex>`](oqueue_core::MaterializedIndex), so
    /// pointing an applier at one is no longer expressible; two *appliers*
    /// over one index still is, and is the case this warning is left for.
    ///
    /// # Errors
    ///
    /// Whatever the log fails to read with, or the index fails to fold with.
    /// ⚠️ On either the index is unchanged, so the failed batch is exactly what
    /// the next call reads.
    pub async fn apply_next_batch(&self) -> Result<usize> {
        let start = self.resume_from()?;
        let page = self.log.read_from(start, APPLY_BATCH_ENTRIES).await?;
        if page.is_empty() {
            return Ok(0);
        }
        self.index.apply(&page)?;
        Ok(page.len())
    }

    /// Folds everything the log holds past what the index has.
    ///
    /// Returns what it cost — see [`CatchUp`].
    ///
    /// ⚠️ **Two exits, and both are load-bearing.** A short page means the log
    /// had no more to give — [`MetadataLog`]'s guarantee 5, which exists
    /// because this exit rests on it — and stopping there saves a round trip
    /// per catch-up on a seam every implementation of which is a real store.
    /// An *empty*
    /// page is the case a short one cannot cover: a log holding an exact
    /// multiple of [`APPLY_BATCH_ENTRIES`] never returns a short page, so
    /// without the empty check the last full page would be followed by
    /// another read forever.
    ///
    /// # Errors
    ///
    /// As [`apply_next_batch`](Self::apply_next_batch). ⚠️ A failure part-way
    /// leaves the batches already folded folded — this is resumable, not
    /// atomic, and the index's own bookmark is what makes resuming correct.
    pub async fn catch_up(&self) -> Result<CatchUp> {
        let mut cost = CatchUp::default();
        loop {
            let batch = self.apply_next_batch().await?;
            cost.reads += 1;
            if batch == 0 {
                return Ok(cost);
            }
            cost.entries += batch;
            cost.batches += 1;
            if batch < APPLY_BATCH_ENTRIES {
                return Ok(cost);
            }
        }
    }
}
