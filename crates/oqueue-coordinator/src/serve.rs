//! The task that owns the allocator, the log and the index — and serves the
//! one queue everything else goes through.
//!
//! ⚠️ **Split from `coordinator.rs` at the 500-line limit, along the seam**:
//! that file is the *handle* a producer or a reader holds, and this is the one
//! task on the other side of the queue. Nothing here is reachable except
//! through that queue, which is the whole of why the ordering claims hold.

// ⚠️ `redundant_pub_crate` and `unreachable_pub` disagree about a `pub(crate)`
// item in a private module. `pub(crate)` is the visibility that is actually
// true of the request types below — `coordinator.rs` builds them, nothing
// outside the crate sees them — so the lint that disagrees is the one allowed.
#![allow(clippy::redundant_pub_crate)]

use crate::allocator::Allocator;
use crate::commit::CommitAck;
use crate::error::CoordinatorError;
use oqueue_core::{
    CommitVersion, CommittedSpan, CoordinatorEpoch, MaterializedIndex, MetadataEntry, MetadataLog,
    ObjectKey,
};
use std::sync::Arc;
use tokio::sync::{broadcast, mpsc, oneshot, watch};

/// What the loop serves.
///
/// ⚠️ **Dropping the cache is a queued request, not a method on a handle**
/// (`ADR-0024`). It has to be serialized against the fold by the same queue
/// that serializes everything else: `MaterializedIndex::apply` checks version
/// order and not contiguity, so a `clear` landing between the loop's own "is
/// this current" check and its fold is *accepted*, and every forgotten
/// partition re-bases at `Offset::ZERO` while the log says otherwise.
#[derive(Debug)]
pub(crate) enum Request {
    /// Give this object a position.
    Commit(CommitRequest),
    /// Discard the materialization; the log can refill it.
    DropCache(oneshot::Sender<()>),
}

/// How many entries one page of a rebuild reads.
///
/// ⚠️ A rebuild is not the apply *rate* `oqueue-index`'s `APPLY_BATCH_ENTRIES`
/// bounds — it happens when a cache was dropped, not on every commit — so this
/// is sized to bound the memory one page costs and nothing else. It is a
/// constant rather than a knob, per `AGENTS.md` non-negotiable 2.
pub const REBUILD_PAGE_ENTRIES: usize = 1024;

/// One producer's commit, and where to send its answer.
#[derive(Debug)]
pub(crate) struct CommitRequest {
    pub(crate) object: ObjectKey,
    pub(crate) spans: Vec<CommittedSpan>,
    pub(crate) reply: oneshot::Sender<Result<CommitAck, CoordinatorError>>,
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
    pub(crate) log: Arc<dyn MetadataLog>,
    pub(crate) index: Arc<dyn MaterializedIndex>,
    pub(crate) epoch: CoordinatorEpoch,
    pub(crate) allocator: Allocator,
    pub(crate) requests: mpsc::Receiver<Request>,
    pub(crate) deltas: broadcast::Sender<MetadataEntry>,
    pub(crate) published: watch::Sender<Option<CommitVersion>>,
}

impl CoordinatorLoop {
    /// Serves commits until every [`Coordinator`](crate::Coordinator) handle has
    /// been dropped.
    ///
    /// ⚠️ **That is the shutdown signal** (`async-concurrency.md` rule 14):
    /// the last handle going away closes the queue, `recv` answers `None`, and
    /// this returns rather than being killed mid-append by the process exiting.
    /// Commits already queued are served first, because `recv` drains before it
    /// reports the close.
    pub async fn run(mut self) {
        while let Some(request) = self.requests.recv().await {
            match request {
                Request::Commit(commit) => {
                    let outcome = self.serve(commit.object, commit.spans).await;
                    // ⚠️ Ignored on purpose: a caller that stopped waiting is
                    // the cancellation case `Coordinator::commit` documents,
                    // and the commit has already happened either way.
                    drop(commit.reply.send(outcome));
                }
                Request::DropCache(reply) => {
                    self.index.clear();
                    self.publish_applied();
                    // ⚠️ Ignored on purpose, same as a commit's: a caller that
                    // stopped waiting does not un-drop the cache.
                    let _ = reply.send(());
                }
            }
        }
    }

    /// Admit → assign → journal → ack, in that order and no other.
    ///
    /// ⚠️ **`admission.replayed`/`.rejected` are always empty today** —
    /// nothing in this workspace yet constructs a [`CommittedSpan`] carrying
    /// a producer identity (`M11.5` is what first makes one), so
    /// `Allocator::admit` can only ever take its no-identity branch here.
    /// ⚠️ **Refused, not asserted, if that ever stops being true**
    /// (`M11.3`, on `M10.17`'s own precedent against trusting "provably
    /// safe today" silently — the lesson that precedent actually teaches,
    /// not a panic dressed as a trip-wire): a `debug_assert!` here would
    /// unwind this shard's *whole* serializing loop over one producer's
    /// ordinary retry, taking every other producer's in-flight commit down
    /// with it, and a release build would silently drop the very spans
    /// this mechanism exists to report correctly. `M11.6` replaces
    /// [`CoordinatorError::ProducerSequenceUnsupported`] with real
    /// per-`(topic, partition)` reporting once a caller can reach this.
    async fn serve(
        &mut self,
        object: ObjectKey,
        spans: Vec<CommittedSpan>,
    ) -> Result<CommitAck, CoordinatorError> {
        let admission = self.allocator.admit(spans);
        if !admission.replayed.is_empty() || !admission.rejected.is_empty() {
            return Err(CoordinatorError::ProducerSequenceUnsupported {
                rejected: admission.rejected.len(),
                replayed: admission.replayed.len(),
            });
        }
        let staged = self
            .allocator
            .stage(object, admission.admitted)
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
            // `M5`'s quota trips. Folding one entry onto a dropped index
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
