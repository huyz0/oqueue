//! The single coordinator: one queue, one allocator, one serialization point.

use crate::allocator::Allocator;
use crate::commit::CommitAck;
#[cfg(doc)]
use crate::commit::UNASSIGNED_OFFSET;
use crate::error::CoordinatorError;
use oqueue_core::{CommittedSpan, CoordinatorEpoch, MetadataLog, ObjectKey};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

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
/// point. `ADR-0020`: the log append is the only serialization point in the
/// produce path, so producers may PUT concurrently and then queue here.
#[derive(Debug, Clone)]
pub struct Coordinator {
    commits: mpsc::Sender<CommitRequest>,
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
    /// # Errors
    ///
    /// [`CoordinatorError::ReplayRequired`] if `log` already holds entries —
    /// see that variant for why this refuses rather than resumes.
    /// [`CoordinatorError::Journal`] if the log cannot be read at all.
    pub async fn open(
        log: Arc<dyn MetadataLog>,
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
        let (commits, requests) = mpsc::channel(COMMIT_QUEUE_DEPTH);
        Ok((
            Self { commits },
            CoordinatorLoop {
                log,
                epoch,
                allocator: Allocator::new(),
                requests,
            },
        ))
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
    epoch: CoordinatorEpoch,
    allocator: Allocator,
    requests: mpsc::Receiver<CommitRequest>,
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
        let (version, assignments) = self.allocator.apply(staged);
        Ok(CommitAck::new(version, self.epoch, assignments))
    }
}
