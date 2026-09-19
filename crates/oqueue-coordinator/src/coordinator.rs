//! The single coordinator: one queue, one allocator, one serialization point.

use crate::allocator::Allocator;
use crate::commit::CommitAck;
use crate::error::{CoordinatorError, OpenRejected};
#[cfg(doc)]
use crate::serve::REBUILD_PAGE_ENTRIES;
use crate::serve::{CommitRequest, CoordinatorLoop, Request, TrimRequest};
use crate::subscribe::{DELTA_BUFFER_ENTRIES, DeltaStream, IndexWatch};
use oqueue_core::{
    Clock, CommitVersion, CommittedSpan, CoordinatorEpoch, IndexReader, MaterializedIndex,
    MetadataEntry, MetadataLog, ObjectKey, Offset, PartitionId, TopicId,
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
    /// [`OpenRejected`], carrying [`CoordinatorError::Journal`] if the log
    /// cannot be read, or [`CoordinatorError::Unreplayable`] if it holds an
    /// entry the replay cannot fold.
    ///
    /// ⚠️ **A non-empty log is replayed, not refused** (`M6.3`): every entry
    /// is folded into the allocator — offsets, the version line, producer
    /// sequence state — and the index, by the arithmetic that served it, before
    /// this returns. Until snapshots land (`M6.4`, `M6.5`) the replay is the
    /// whole log.
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
    /// ⚠️ **`clock` is what makes retention possible at all** (`M5.86`).
    /// Every `BatchCommitted` carries the moment the log took it, the fold
    /// keeps a per-partition extent from those, and FR-33's decision — is this
    /// partition older than its retention — then costs no object-storage
    /// operation. ⚠️ **The coordinator's clock, not the producer's
    /// timestamps**: a record's own time is client-supplied and unordered, so
    /// retention driven by it is retention a client can defeat by backdating.
    pub async fn open(
        log: Arc<dyn MetadataLog>,
        index: Box<dyn MaterializedIndex>,
        epoch: CoordinatorEpoch,
        clock: Arc<dyn Clock>,
    ) -> Result<(Self, CoordinatorLoop, IndexReader), OpenRejected> {
        let (allocator, last) = match replay(&*log, &*index).await {
            Ok(replayed) => replayed,
            Err(error) => return Err(OpenRejected::new(error, index)),
        };
        // ⚠️ The `Box` becomes an `Arc` **here**, inside the seam — `ADR-0024`.
        // A caller that had constructed the `Arc` would have kept one, and an
        // `Arc<dyn MaterializedIndex>` carries `apply` and `clear`. Moving a
        // `Box` in is what makes "the coordinator is the sole writer" a thing
        // the type system holds rather than a thing four rustdocs ask for.
        let index: Arc<dyn MaterializedIndex> = Arc::from(index);
        let (commits, requests) = mpsc::channel(COMMIT_QUEUE_DEPTH);
        let (deltas, listener) = broadcast::channel(DELTA_BUFFER_ENTRIES);
        let (published, applied) = watch::channel(last);
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
                allocator,
                clock,
                requests,
                deltas,
                published,
                last_committed: last,
                lease: None,
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

    /// Journals a trim: `partition`'s records below `start` stop being
    /// readable (`M5.90`).
    ///
    /// ⚠️ **The only way a trim reaches the log**, and retention's only way to
    /// act (FR-33): a round decides from the index which partitions have
    /// expired (`ExpiryHeap`), and this is where that decision becomes
    /// durable. Queued behind the commits already waiting, so it takes the
    /// next version in the one order everything else does.
    ///
    /// ⚠️ **Not cancellation-safe**, as [`commit`](Self::commit) is not: once
    /// queued it is journaled whether or not the caller still waits.
    ///
    /// # Errors
    ///
    /// [`CoordinatorError::Unavailable`] if the loop has stopped,
    /// [`CoordinatorError::Unassignable`] carrying
    /// [`Error::TrimPastEnd`](oqueue_core::Error::TrimPastEnd) if `start` is
    /// past the partition's end — refused before the journal, so the log never
    /// holds a trim its own fold would refuse — and
    /// [`CoordinatorError::Journal`] if the append was refused.
    pub async fn trim(
        &self,
        topic: TopicId,
        partition: PartitionId,
        start: Offset,
    ) -> Result<CommitVersion, CoordinatorError> {
        let (reply, answer) = oneshot::channel();
        self.commits
            .send(Request::Trim(TrimRequest {
                topic,
                partition,
                start,
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

/// Folds the whole log into a fresh allocator and `index` (`M6.3`).
///
/// ⚠️ **Paged**, [`REBUILD_PAGE_ENTRIES`] at a time, so a long log costs
/// bounded memory per page rather than one read of everything.
///
/// ⚠️ **`index` is cleared only once the first page has been read**, and
/// cleared again on any later failure. An index arriving with a version folded
/// into it holds one from some other line — a rotated log, a re-shard, a disk
/// engine outliving the process — so it is never trusted; but a first read
/// that fails transiently leaves it exactly as it came in, so the caller can
/// retry with it (`M3.34`). A failure after the clear hands back an empty
/// index, never a partly replayed one.
async fn replay(
    log: &dyn MetadataLog,
    index: &dyn MaterializedIndex,
) -> Result<(Allocator, Option<CommitVersion>), CoordinatorError> {
    let first = log
        .read_from(CommitVersion::ZERO, crate::serve::REBUILD_PAGE_ENTRIES)
        .await
        .map_err(CoordinatorError::Journal)?;
    index.clear();
    let replayed = fold_pages(log, index, first).await;
    if replayed.is_err() {
        index.clear();
    }
    replayed
}

async fn fold_pages(
    log: &dyn MetadataLog,
    index: &dyn MaterializedIndex,
    mut page: Vec<MetadataEntry>,
) -> Result<(Allocator, Option<CommitVersion>), CoordinatorError> {
    let mut allocator = Allocator::new();
    let mut last = None;
    while let Some(tail) = page.last() {
        let unreplayable = |version: CommitVersion| {
            move |source| CoordinatorError::Unreplayable {
                version: version.get(),
                source,
            }
        };
        for entry in &page {
            allocator
                .replay(entry)
                .map_err(unreplayable(entry.version()))?;
        }
        index.apply(&page).map_err(unreplayable(tail.version()))?;
        last = Some(tail.version());
        let from = tail
            .version()
            .advance(1)
            .map_err(unreplayable(tail.version()))?;
        page = log
            .read_from(from, crate::serve::REBUILD_PAGE_ENTRIES)
            .await
            .map_err(CoordinatorError::Journal)?;
    }
    Ok((allocator, last))
}
