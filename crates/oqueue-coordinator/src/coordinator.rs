//! The single coordinator: one queue, one allocator, one serialization point.

use crate::allocator::Allocator;
use crate::commit::CommitAck;
use crate::error::CoordinatorError;
use crate::subscribe::{DELTA_BUFFER_ENTRIES, DeltaStream, IndexWatch};
use oqueue_core::{
    CommitVersion, CommittedSpan, CoordinatorEpoch, MaterializedIndex, MetadataEntry, MetadataLog,
    ObjectKey,
};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, oneshot, watch};

/// How many commits may be queued before a producer waits.
///
/// ⚠️ **A bound rather than a buffer.** An unbounded queue turns a coordinator
/// that has fallen behind into memory growth with no signal; a bounded one
/// makes [`Coordinator::commit`] wait, which is backpressure arriving where the
/// producer can see it. The number is a constant, not a knob (`AGENTS.md`
/// non-negotiable 2), and it is **UNDERIVED** — sized to be comfortably above
/// the in-flight flush count a broker node holds and below anything that would
/// matter for memory. `M14` measures what it should be.
pub const COMMIT_QUEUE_DEPTH: usize = 1024;

/// How many entries one page of a rebuild reads.
///
/// ⚠️ A rebuild is not the apply *rate* `oqueue-index`'s `APPLY_BATCH_ENTRIES`
/// bounds — it happens when a cache was dropped, not on every commit — so this
/// is sized to bound the memory one page costs and nothing else. It is a
/// constant rather than a knob, per `AGENTS.md` non-negotiable 2.
pub const REBUILD_PAGE_ENTRIES: usize = 1024;

/// One producer's commit, and where to send its answer.
#[derive(Debug)]
struct CommitRequest {
    object: ObjectKey,
    spans: Vec<CommittedSpan>,
    reply: oneshot::Sender<Result<CommitAck, CoordinatorError>>,
}

/// A handle on the coordinator for one metadata shard.
///
/// Cheap to clone, and every clone reaches the same allocator — which is the
/// point. ⚠️ `Clone` is written by hand rather than derived: a
/// `broadcast::Receiver` has no `Clone`, and the resubscribe this uses instead
/// is exactly the right thing — a fresh handle follows the tail from *now*,
/// which is what [`subscribe`](Self::subscribe) promises anyway. `ADR-0020`: the log append is the only serialization point in the
/// produce path, so producers may PUT concurrently and then queue here.
#[derive(Debug)]
pub struct Coordinator {
    epoch: CoordinatorEpoch,
    commits: mpsc::Sender<CommitRequest>,
    /// ⚠️ **A receiver, not the sender**, and that is what makes
    /// [`DeltaLag::Closed`](crate::DeltaLag::Closed) mean what its message
    /// says. `broadcast` reports `Closed` only once every *sender* is gone, so
    /// a handle holding one would keep a follower parked forever against a
    /// loop that had already stopped — the loop owns the only sender, exactly
    /// as it owns the only [`watch`] sender, and the two now fail alike.
    /// `resubscribe` gives the same subscribe-from-the-tail semantics
    /// `subscribe` did.
    deltas: broadcast::Receiver<MetadataEntry>,
    applied: watch::Receiver<Option<CommitVersion>>,
}

impl Clone for Coordinator {
    fn clone(&self) -> Self {
        Self {
            epoch: self.epoch,
            commits: self.commits.clone(),
            deltas: self.deltas.resubscribe(),
            applied: self.applied.clone(),
        }
    }
}

impl Coordinator {
    /// Opens a coordinator over `log`, returning the handle and the loop that
    /// serves it.
    ///
    /// ⚠️ **The loop is returned rather than spawned.** `async-concurrency.md`
    /// rule 13: a spawned task needs an owner that can observe its completion
    /// and its panic, and this crate does not know who that is. The caller
    /// spawns [`CoordinatorLoop::run`] and holds its handle.
    ///
    /// # ⚠️ The coordinator is the sole writer of `index`, and the only party
    /// # that may clear it
    ///
    /// **Decided here** (`M3.9`), because `oqueue-index`'s `LogApplier` folds
    /// the same log into the same kind of index and something had to say which
    /// of them owns a given one. The rule is doc 12 §4.4's own model: every
    /// agent keeps *its own* materialization. A coordinator maintains the one
    /// it is given; a follower elsewhere keeps a different one and fills it
    /// with `LogApplier` from [`subscribe`](Self::subscribe) and the log.
    ///
    /// Handing one index to both is a defect and nothing detects it. The two
    /// interleavings are: the applier folding first, so every later commit
    /// finds the index ahead of it and pays a **full log replay inside the
    /// ack path**; and the coordinator folding between the applier's read and
    /// its apply, so the applier is refused with
    /// [`NonMonotonicCommitVersion`](oqueue_core::Error::NonMonotonicCommitVersion)
    /// — an error that says the log lost ordering when nothing is wrong with
    /// it.
    ///
    /// ⚠️ **"Only party that may clear it" is the other half**, and it is not
    /// pedantry: `apply` checks version order and *not* contiguity, so a
    /// `clear` landing between this loop's own check and its fold would be
    /// accepted and would re-base every partition at [`Offset::ZERO`]. Whoever
    /// wants the cache dropped — `M3.11`'s quota is the row that will — asks
    /// the coordinator, and does not reach for the handle.
    ///
    /// [`Offset::ZERO`]: oqueue_core::Offset::ZERO
    ///
    /// # Errors
    ///
    /// [`CoordinatorError::ReplayRequired`] if `log` already holds entries —
    /// see that variant for why this refuses rather than resumes.
    /// [`CoordinatorError::Journal`] if the log cannot be read at all.
    pub async fn open(
        log: Arc<dyn MetadataLog>,
        index: Arc<dyn MaterializedIndex>,
        epoch: CoordinatorEpoch,
    ) -> Result<(Self, CoordinatorLoop), CoordinatorError> {
        if let Some(last) = log
            .last_version()
            .await
            .map_err(CoordinatorError::Journal)?
        {
            return Err(CoordinatorError::ReplayRequired {
                last_version: last.get(),
            });
        }
        // ⚠️ **Cleared, not trusted.** The log has just been checked empty, so
        // an index arriving with a version folded into it holds one from some
        // *other* line — a rotated log, a re-shard, or doc 10 #12's disk
        // engine outliving the process. Seeding the watch from it would have
        // `IndexWatch` answer `true` at once for every `AtLeast(v)` below that
        // version and serve a reader offsets from a different line believing
        // they are fresh, which is hazard H2 through the mechanism built to
        // prevent it. Clearing is the operation this seam guarantees is always
        // safe, and it makes the invariant true by construction rather than by
        // a check that has to be remembered.
        index.clear();
        let (commits, requests) = mpsc::channel(COMMIT_QUEUE_DEPTH);
        let (deltas, listener) = broadcast::channel(DELTA_BUFFER_ENTRIES);
        let (published, applied) = watch::channel(None);
        Ok((
            Self {
                epoch,
                commits,
                deltas: listener,
                applied,
            },
            CoordinatorLoop {
                log,
                index,
                epoch,
                allocator: Allocator::new(),
                requests,
                deltas,
                published,
            },
        ))
    }

    /// Follows the tail of this shard's log.
    ///
    /// Doc 12 §4.4's push half, and `M3.md` task 15. A subscriber that falls
    /// behind is told to re-bootstrap from the log rather than silently
    /// skipping — see [`DeltaLag`](crate::DeltaLag).
    ///
    /// ⚠️ **It begins at the *next* commit, not at the beginning.** A fresh
    /// follower subscribes first and folds the log second, so the two overlap
    /// rather than leaving a gap — a gap is unrecoverable and an overlap is
    /// not.
    ///
    /// ⚠️ **The follower has to strip the overlap itself**, and an earlier
    /// version of this paragraph said the fold would do it. It will not: an
    /// [`apply`](oqueue_core::MaterializedIndex::apply) is all-or-nothing, so
    /// a batch whose *first* entry is one already folded is refused **whole**,
    /// taking the new entries after it down with the duplicate. Drop every
    /// pushed entry at or below
    /// [`applied_upto`](oqueue_core::MaterializedIndex::applied_upto) before
    /// folding, and fold what is left.
    ///
    /// ⚠️ **Bootstrap here is a full log replay, not a snapshot**, and doc 12
    /// §4.4's snapshot half is `M6`'s (`M6.md` tasks 8 and 15: a
    /// `SnapshotCommitted` record, one GET, then replay of the tail only).
    /// Until it exists, N agents restarting together produce N full replays —
    /// the thundering herd doc 12 §4.4 introduces snapshots to avoid — which
    /// is a real cost and is recorded in `roadmap.md`'s deferral table rather
    /// than implied by this paragraph's silence.
    #[must_use]
    pub fn subscribe(&self) -> DeltaStream {
        DeltaStream::new(self.deltas.resubscribe())
    }

    /// Which incarnation this coordinator is.
    ///
    /// ⚠️ What a reader compares its cache against — [`CacheState::admits`]
    /// takes it, and hazard H5 is the whole reason: a cache from an
    /// incarnation that is gone holds positions on a line a failover may have
    /// rewound, so it can be arbitrarily far ahead by version and must still
    /// not answer.
    ///
    /// ⚠️ **M3 builds one incarnation and never bumps this** — `ADR-0020`
    /// point 6 puts failover in `M6`. The fence is here so the reader side is
    /// already written against it rather than retrofitted onto readers that
    /// learned to trust a cache unconditionally.
    ///
    /// [`CacheState::admits`]: oqueue_core::CacheState::admits
    #[must_use]
    pub const fn epoch(&self) -> CoordinatorEpoch {
        self.epoch
    }

    /// Watches how far the coordinator's own index has folded.
    ///
    /// What a parked fetch waits on, per `M3.md` task 17.
    #[must_use]
    pub fn watch(&self) -> IndexWatch {
        IndexWatch::new(self.applied.clone())
    }

    /// Commits an object's position, and answers with the offsets it took.
    ///
    /// The whole of `ADR-0020` point 3 from a caller's side: when this resolves
    /// `Ok`, the record covering those offsets is durable in the metadata log,
    /// and the caller may acknowledge to its client. When it resolves `Err`
    /// there is no offset to report — [`UNASSIGNED_OFFSET`](crate::UNASSIGNED_OFFSET),
    /// never `0`.
    ///
    /// ⚠️ **[`CommitAck::assignments`] answers `spans` position for position.**
    /// One assignment per span, in the order given, whether or not two spans
    /// name the same `(topic, partition)` — which is how a caller bundling
    /// several producers' batches into one object attributes an offset back to
    /// the producer that earned it. [`CommitAck::base_offset`] answers the
    /// coarser question and is not a substitute for it.
    ///
    /// ⚠️ **Not cancellation-safe, and the caller must know which half**
    /// (`async-concurrency.md` rule 10). Dropping this future before it
    /// resolves does **not** un-commit anything: the request may already be
    /// queued, and the loop will journal it and find nobody to answer. That is
    /// the written-but-not-acknowledged ambiguity a dropped connection already
    /// produces, arriving from inside the process — rule 11 says route it
    /// through the same reconciliation path rather than special-casing it. What
    /// it is never allowed to become is a *gap*: the offsets were taken and the
    /// records are in the log, so no later commit reuses them.
    ///
    /// # Errors
    ///
    /// [`CoordinatorError::Unavailable`] if the loop has stopped,
    /// [`CoordinatorError::Unassignable`] if no position exists to give, and
    /// [`CoordinatorError::Journal`] if one exists but could not be made
    /// durable.
    pub async fn commit(
        &self,
        object: ObjectKey,
        spans: Vec<CommittedSpan>,
    ) -> Result<CommitAck, CoordinatorError> {
        let (reply, answer) = oneshot::channel();
        self.commits
            .send(CommitRequest {
                object,
                spans,
                reply,
            })
            .await
            .map_err(|_| CoordinatorError::Unavailable)?;
        answer.await.map_err(|_| CoordinatorError::Unavailable)?
    }
}

/// The task that owns the allocator and the log.
///
/// ⚠️ **A single owner rather than a shared lock**, per
/// `async-concurrency.md` rules 7 and 8. Serializing "assign, then append" is a
/// sequencing problem: a mutex expressing it would have to be held across the
/// append's `.await`, which rule 6 forbids outright, and releasing it in
/// between would let two commits reach the log out of version order — which the
/// log refuses, correctly, leaving a hole in a line that must not have one.
#[derive(Debug)]
pub struct CoordinatorLoop {
    log: Arc<dyn MetadataLog>,
    index: Arc<dyn MaterializedIndex>,
    epoch: CoordinatorEpoch,
    allocator: Allocator,
    requests: mpsc::Receiver<CommitRequest>,
    deltas: broadcast::Sender<MetadataEntry>,
    published: watch::Sender<Option<CommitVersion>>,
}

impl CoordinatorLoop {
    /// Serves commits until every [`Coordinator`] handle has been dropped.
    ///
    /// ⚠️ **That is the shutdown signal** (`async-concurrency.md` rule 14):
    /// the last handle going away closes the queue, `recv` answers `None`, and
    /// this returns rather than being killed mid-append by the process exiting.
    /// Commits already queued are served first, because `recv` drains before it
    /// reports the close.
    pub async fn run(mut self) {
        while let Some(request) = self.requests.recv().await {
            let outcome = self.serve(request.object, request.spans).await;
            // ⚠️ Ignored on purpose: a caller that stopped waiting is the
            // cancellation case `Coordinator::commit` documents, and the commit
            // has already happened either way.
            drop(request.reply.send(outcome));
        }
    }

    /// Assign → journal → ack, in that order and no other.
    async fn serve(
        &mut self,
        object: ObjectKey,
        spans: Vec<CommittedSpan>,
    ) -> Result<CommitAck, CoordinatorError> {
        let staged = self
            .allocator
            .stage(object, spans)
            .map_err(CoordinatorError::Unassignable)?;
        // ⚠️ Journaled before the allocator takes the position, so a refusal
        // leaves the line exactly where it was. The reverse order would leave a
        // gap that nothing later could fill.
        self.log
            .append(core::slice::from_ref(staged.entry()))
            .await
            .map_err(CoordinatorError::Journal)?;
        let entry = staged.entry().clone();
        let (version, assignments) = self.allocator.apply(staged);
        self.publish(entry).await;
        Ok(CommitAck::new(version, self.epoch, assignments))
    }

    /// Folds the committed entry into the local index, then tells everyone.
    ///
    /// ⚠️ **Before the ack returns**, which is what makes read-your-writes
    /// (hazard H2) hold locally by construction: a producer holding a
    /// [`CommitAck`] can fetch at its version and the index already has it,
    /// with no window in which the position is durable and unqueryable.
    ///
    /// ⚠️ **Nothing here can fail the commit.** The position is durable in the
    /// log whatever happens to the cache, so refusing the ack would deny a
    /// producer an offset that exists.
    async fn publish(&self, entry: MetadataEntry) {
        if self.index_precedes(entry.version()) {
            if self.index.apply(core::slice::from_ref(&entry)).is_err() {
                // A fold refused by an index the coordinator *knows* the
                // position of is a defect, not a dropped cache. Empty is the
                // one state that cannot be wrong, and `publish_applied` then
                // reports `None` rather than a version nothing holds.
                self.index.clear();
            }
        } else {
            // ⚠️ **The index is a cache anyone may drop**, and the seam says so
            // in as many words — under memory pressure, on a restart, when
            // `M3.11`'s quota trips. Folding one entry onto a dropped index
            // does not merely lose the old records: `IndexState` bases a
            // partition it has forgotten at `Offset::ZERO`, so the fold would
            // place these records at 0 while the log, the allocator and the
            // ack already given to the producer place them at the true offset.
            // An index that is *wrong* is worse than one that is empty.
            //
            // ⚠️ The rebuild covers **this** entry too, because the journal
            // step above already put it in the log. Folding it again
            // afterwards is the replay the fold refuses, by design.
            self.rebuild().await;
        }
        self.publish_applied();
        // ⚠️ Ignored on purpose: no subscribers is the ordinary case, not a
        // failure. A *lagging* subscriber is a different thing and is told so
        // on its own `recv`.
        drop(self.deltas.send(entry));
    }

    /// Whether the index sits exactly one version below `version`.
    ///
    /// ⚠️ **Exactly, not "at least".** A gap means entries are missing and the
    /// fold would mis-base; being *ahead* means something else is writing to
    /// this index, which is the same problem from the other side. Both are
    /// answered by rebuilding, because both mean the coordinator does not know
    /// what the index holds.
    fn index_precedes(&self, version: CommitVersion) -> bool {
        self.index
            .applied_upto()
            .map_or(version == CommitVersion::ZERO, |applied| {
                applied.advance(1).is_ok_and(|next| next == version)
            })
    }

    /// Re-derives the whole index from the log.
    ///
    /// ⚠️ **Deliberately not `oqueue-index`'s `LogApplier`**, which does the
    /// same fold: `architecture.md`'s star topology means this crate depends on
    /// `oqueue-core` and nothing else in the workspace, and the two are not the
    /// same operation anyway — that one resumes from a bookmark and this one
    /// starts from nothing, because the bookmark is exactly what was lost.
    ///
    /// ⚠️ **A failure here leaves the index empty rather than partial.** An
    /// empty cache is answered by a reader waiting out its deadline; a partial
    /// one is answered with wrong offsets.
    ///
    /// ⚠️ **It reads until a page comes back empty**, and deliberately does
    /// *not* stop early on a short one the way `oqueue-index`'s `catch_up`
    /// does. That saves a round trip per call and is worth it there, where a
    /// catch-up happens constantly; here it would buy one read on an operation
    /// that only runs when a cache was dropped, in exchange for a branch no
    /// test can distinguish from its own mutation.
    async fn rebuild(&self) {
        self.index.clear();
        let mut start = CommitVersion::ZERO;
        loop {
            let Ok(page) = self.log.read_from(start, REBUILD_PAGE_ENTRIES).await else {
                self.index.clear();
                return;
            };
            let Some(last) = page.last().map(MetadataEntry::version) else {
                return;
            };
            if self.index.apply(&page).is_err() {
                self.index.clear();
                return;
            }
            let Ok(next) = last.advance(1) else {
                return;
            };
            start = next;
        }
    }

    /// Publishes what the index **actually** holds.
    ///
    /// ⚠️ **Read back from the index, never remembered separately.** The watch
    /// is level-triggered, so a version published above what the index holds
    /// wakes every reader waiting at or below it *immediately*, onto an index
    /// that cannot answer them — hazard H2 arriving through the very mechanism
    /// built to prevent it. A second copy of this fact is a second thing to be
    /// wrong.
    fn publish_applied(&self) {
        self.published.send_replace(self.index.applied_upto());
    }
}
