//! The single coordinator: one queue, one allocator, one serialization point.

use crate::allocator::Allocator;
use crate::commit::CommitAck;
use crate::error::{CoordinatorError, OpenRejected};
#[cfg(doc)]
use crate::serve::REBUILD_PAGE_ENTRIES;
use crate::serve::{CommitRequest, CoordinatorLoop, Request};
use crate::subscribe::{DELTA_BUFFER_ENTRIES, DeltaStream, IndexWatch};
use oqueue_core::{
    CommitVersion, CommittedSpan, CoordinatorEpoch, IndexReader, MaterializedIndex, MetadataEntry,
    MetadataLog, ObjectKey,
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
    commits: mpsc::Sender<Request>,
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
    /// # ⚠️ The coordinator owns `index`, and that is why it arrives as a `Box`
    ///
    /// `ADR-0024`, which carries the reasoning. A caller that constructed an
    /// `Arc` would keep one, and an `Arc<dyn MaterializedIndex>` carries
    /// `apply` and `clear`; moving a `Box` in leaves the caller nothing to
    /// write through *on the path that succeeds* — ⚠️ **a refusal hands it
    /// back**, which is `M3.34`'s exception and is stated in full under
    /// `# Errors` below rather than qualified here twice. What comes back is an [`IndexReader`] — every read a
    /// fetch needs, with the write side simply absent.
    ///
    /// Dropping the cache stays available and goes through
    /// [`drop_cache`](Self::drop_cache), which queues it behind the folds it
    /// must not interleave with.
    ///
    /// # Errors
    ///
    /// [`OpenRejected`], carrying [`CoordinatorError::ReplayRequired`] if `log`
    /// already holds entries — see that variant for why this refuses rather
    /// than resumes — or [`CoordinatorError::Journal`] if the log cannot be
    /// read at all.
    ///
    /// ⚠️ **A refusal hands `index` back** (`M3.34`). Taking it by value is
    /// what makes the sole-writer rule structural, and it is also what makes a
    /// plain `Err` destroy the thing the caller may not be able to rebuild —
    /// doc 10 #12's engine is an open database, not an allocation. Both
    /// refusals happen before the index is touched, so what comes back is
    /// exactly what went in.
    ///
    /// ⚠️ **Not cancel-safe, and the same ownership is why**
    /// (`async-concurrency.md` rule 10). This future owns `index` across its
    /// one await — the log's `last_version` — so dropping it takes the index
    /// with it and leaves the caller nothing: no [`OpenRejected`] to catch,
    /// and the loss this row exists to prevent arriving through a `select!`
    /// arm or a startup `timeout`. Bound the log's own read instead of this
    /// call, or accept that a cancelled open costs an index.
    pub async fn open(
        log: Arc<dyn MetadataLog>,
        index: Box<dyn MaterializedIndex>,
        epoch: CoordinatorEpoch,
    ) -> Result<(Self, CoordinatorLoop, IndexReader), OpenRejected> {
        let last = match log.last_version().await {
            Ok(last) => last,
            Err(error) => {
                return Err(OpenRejected::new(CoordinatorError::Journal(error), index));
            }
        };
        if let Some(last) = last {
            return Err(OpenRejected::new(
                CoordinatorError::ReplayRequired {
                    last_version: last.get(),
                },
                index,
            ));
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
        // ⚠️ The `Box` becomes an `Arc` **here**, inside the seam — `ADR-0024`.
        // A caller that had constructed the `Arc` would have kept one, and an
        // `Arc<dyn MaterializedIndex>` carries `apply` and `clear`. Moving a
        // `Box` in is what makes "the coordinator is the sole writer" a thing
        // the type system holds rather than a thing four rustdocs ask for.
        let index: Arc<dyn MaterializedIndex> = Arc::from(index);
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
                index: Arc::clone(&index),
                epoch,
                allocator: Allocator::new(),
                requests,
                deltas,
                published,
                last_committed: None,
            },
            IndexReader::new(index),
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
            .send(Request::Commit(CommitRequest {
                object,
                spans,
                reply,
            }))
            .await
            .map_err(|_| CoordinatorError::Unavailable)?;
        answer.await.map_err(|_| CoordinatorError::Unavailable)?
    }

    /// Discards the materialization, which the log can refill.
    ///
    /// ⚠️ **The one door the API offers, and it is queued** (`ADR-0024`). An
    /// [`IndexReader`] does not expose `clear`, so nothing a caller was *handed*
    /// can reach it — a delegating newtype it wrote itself still could, which
    /// the ADR states plainly. Two
    /// writers on one index is not a race that loses a write but one that
    /// produces *wrong offsets*, since `apply` checks version order and not
    /// contiguity. Routing the drop through this queue puts it in the same
    /// serial order as the folds it must not interleave with.
    ///
    /// Resolves once the cache is gone. The next commit rebuilds from the log,
    /// and a reader parked on [`watch`](Self::watch) waits out its own deadline
    /// in the meantime rather than being woken onto an empty index.
    ///
    /// ⚠️ **That rebuild is where the cost lands, and it is on the ack path.**
    /// The next commit replays the whole log,
    /// [`REBUILD_PAGE_ENTRIES`] at a time, with every queued producer waiting
    /// behind it — so this is cheap to *ask* and expensive to have asked.
    /// `M3.18` owns moving that replay off the ack path.
    ///
    /// ⚠️ **Nothing in M3 decides *when*.** `M3.11` set out to and found a
    /// ceiling unachievable at this index's keying, so `roadmap.md` carries the
    /// quota to `M5`. This is the mechanism whatever decides will call.
    ///
    /// ⚠️ **Not cancellation-safe** (`async-concurrency.md` rule 10), the same
    /// way [`commit`](Self::commit) is not: dropping this future after the
    /// request is queued does not un-queue it, so a caller whose deadline fired
    /// may conclude the drop did not happen while the loop performs it anyway.
    /// A quota that then retries pays a second rebuild.
    ///
    /// ⚠️ **It shares the commit queue**, so it waits behind whatever is in it —
    /// up to [`COMMIT_QUEUE_DEPTH`] durable appends, each of which folds more
    /// into the index the drop was called to shrink. A second channel selected
    /// in the loop would serialize against the fold just as well; the shared
    /// queue is a choice, and this is its cost.
    ///
    /// # Errors
    ///
    /// [`CoordinatorError::Unavailable`] if the loop has stopped.
    pub async fn drop_cache(&self) -> Result<(), CoordinatorError> {
        let (reply, answer) = oneshot::channel();
        self.commits
            .send(Request::DropCache(reply))
            .await
            .map_err(|_| CoordinatorError::Unavailable)?;
        answer.await.map_err(|_| CoordinatorError::Unavailable)
    }
}
