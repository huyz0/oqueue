//! The single-writer seam every group-state-mutating handler routes
//! through — `M4.15c`, `ADR-0020`'s "the ack IS the durable write"
//! instinct applied to `GroupCoordinator` instead of `CommittedOffsets`.
//!
//! ⚠️ **A single-consumer actor, not `M4.14`'s version-guarded merge —
//! `M4.15b`'s own dissolution note.** `CommittedOffsets`'s own race (two
//! commits to the same key landing their durable append and their
//! in-memory update in opposite orders) was fixable with "highest version
//! wins," because an offset commit has no relationship to the one before
//! it. `GroupState::transition` is not like that: replaying `Join` then
//! `JoinBarrierComplete` produces a different, legal result than replaying
//! them in the other order, which would likely just refuse the second one
//! outright. Durability has to land in the *same relative order* the live
//! transition actually happened in, for every group, across every call
//! site — which a single ordered queue gives for free and a per-key
//! version guard cannot.
//!
//! ⚠️ **Validated before it is durable, not after** — [`handle_one`]'s own
//! doc. A refused transition is never appended; a durable append failure
//! never reaches the live `GroupCoordinator`. Either way the two stay in
//! lockstep, the same property `CommittedOffsets::commit`'s own
//! append-before-insert order protects, adapted here because the "insert"
//! step (`GroupCoordinator::transition`) is also the validator and has no
//! way to be undone once called.
//!
//! ⚠️ **Only three of `M4`'s own four mutating call sites route through
//! this, by design — `M4.15c`'s own scope.** `heartbeat.rs`'s eviction
//! sweep (shared by `leave_group.rs`) and `sync_group.rs`'s own
//! `SyncComplete` call enqueue while already holding their own lock and
//! await the response after releasing it — `heartbeat.rs`'s own doc
//! explains why that is safe under `async-concurrency.md` rule 6.
//! `join_group::round.rs`'s own join-barrier bookkeeping decides its own
//! mutations *from* the transition's result inside the same lock, which
//! this shape cannot serve without an optimistic-retry redesign — `M4.15d`,
//! not this row.
//!
//! ⚠️ **The consequence, named rather than left implicit: a group whose
//! own state has only ever been touched by `JoinGroup` does not survive a
//! restart yet.** `Join`, `JoinBarrierComplete`, and
//! `MemberJoinedDuringSync` are still applied by `join_group::round.rs`'s
//! own direct `coordinator.transition` call, never durably logged here —
//! most real groups begin with exactly one of those, so this row's own
//! acceptance criterion is proven against `heartbeat`/`leave_group`/
//! `sync_group`-driven transitions specifically (seeded through this same
//! actor in the test, not through a real `JoinGroup` request), not "every
//! group survives a restart" in general. `M4.15d` is what makes that
//! claim true.
//!
//! ⚠️ **That gap is contained to the group it affects, not the whole
//! cluster — [`GroupTransitionsTask::replay`]'s own doc, round-1 review's
//! own finding against this row's first version.** Because most real
//! groups' own first durable record (`sync_group.rs`'s `SyncComplete`, or
//! an eviction) has no durable `Join` behind it yet, replaying that entry
//! against a fresh coordinator is `IllegalGroupTransition` on essentially
//! every restart of a broker that has served real traffic — the first
//! version of `replay` propagated that as a fatal error, which stalled
//! `M4.15a`'s own `ReplayGate` forever for *every* group and every offset
//! operation, not just the one group this gap actually affects. Replay
//! now poisons only that one group (it simply never accrues a record,
//! same as before this row existed) and continues past it.

// ⚠️ `pub(crate)` inside a private module: `unreachable_pub` denies the
// `pub` clippy's `redundant_pub_crate` asks for — `fencing.rs`'s own
// precedent for the same standoff.
#![allow(clippy::redundant_pub_crate)]

use oqueue_core::{
    AssignmentEpoch, CommitVersion, Error, GenerationId, GroupCoordinator, GroupEvent, GroupId,
    GroupMetadataEntry, GroupMetadataLog, GroupMetadataRecord, GroupRecord, GroupState, Result,
};
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};

/// How many entries [`replay`] reads per page — `CommittedOffsets::replay`'s
/// own precedent, `GroupMetadataLog`'s guarantee 5 unchanged.
const REPLAY_PAGE_SIZE: usize = 256;

/// How many times [`append_durably`] retries a version conflict before
/// giving up. ⚠️ Its own constant, not `offset_commit.rs`'s
/// `MAX_COMMIT_RETRIES` reused: the two are two independent writers
/// racing for versions in the *same* shared log, but a parallel constant
/// rather than shared code, `FaultGroupMetadataLog`'s own "two small
/// things, no shared code" precedent (`ADR-0035`). Bounded for
/// `security.md` rule 13's own instinct: a caller that never stops
/// retrying is an unbounded hold wearing a retry loop.
const MAX_APPEND_RETRIES: u32 = 8;

/// One enqueued request: fire `event` against `group`, durably, in order.
struct Request {
    group: GroupId,
    event: GroupEvent,
    respond_to: oneshot::Sender<Result<GroupRecord>>,
}

/// The handle every call site holds — cheap to clone (an `mpsc` sender),
/// shared across every connection the same way `Cluster`'s other seams
/// are.
#[derive(Debug, Clone)]
pub(crate) struct GroupTransitions {
    sender: mpsc::UnboundedSender<Request>,
}

/// The other half: owns the receiving end, replayed and then served by
/// whichever task `Cluster::new` spawns — kept apart from
/// [`GroupTransitions`] so the handle exists (and can be enqueued into)
/// before replay has actually run, the same "return at once, populate in
/// the background" shape `M4.15a` already established for offsets.
pub(crate) struct GroupTransitionsTask {
    receiver: mpsc::UnboundedReceiver<Request>,
}

impl GroupTransitions {
    /// Builds a fresh, unreplayed pair. Enqueuing into the returned
    /// [`GroupTransitions`] before [`GroupTransitionsTask::replay`] and
    /// [`GroupTransitionsTask::serve`] have run leaves the request
    /// pending forever — never observed in practice, since nothing can
    /// reach a real call site until `M4.15a`'s own `ReplayGate` opens,
    /// which happens only after replay finishes.
    pub(crate) fn new() -> (Self, GroupTransitionsTask) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (Self { sender }, GroupTransitionsTask { receiver })
    }

    /// Enqueues `(group, event)` without waiting for it to be durably
    /// applied — safe to call while holding a `std::sync::Mutex`
    /// (`async-concurrency.md` rule 6): the send itself never awaits.
    /// `heartbeat.rs`'s own `remove_where` is the real caller this shape
    /// exists for: the decision (which event to fire) and the enqueue
    /// happen atomically under its own lock, and the response is awaited
    /// only after that lock is released.
    pub(crate) fn enqueue(
        &self,
        group: GroupId,
        event: GroupEvent,
    ) -> oneshot::Receiver<Result<GroupRecord>> {
        let (respond_to, response) = oneshot::channel();
        // `Err` only if the serving task itself has ended (replay failed,
        // or it panicked) — `response.await` then resolves `Err` on its
        // own, which `Self::transition`'s own `unwrap_or` turns into
        // `Error::Transient` rather than a hang.
        let _ = self.sender.send(Request {
            group,
            event,
            respond_to,
        });
        response
    }

    /// Enqueues and awaits in one call — every call site that holds no
    /// competing lock across the decision uses this directly.
    pub(crate) async fn transition(
        &self,
        group: GroupId,
        event: GroupEvent,
    ) -> Result<GroupRecord> {
        self.enqueue(group, event)
            .await
            .unwrap_or(Err(Error::Transient))
    }
}

impl GroupTransitionsTask {
    /// Replays every already-durable [`GroupMetadataRecord::GroupTransitioned`]
    /// record into `coordinator`, oldest first — `GroupState::transition`'s
    /// own determinism (doc 02 §3.1) is what makes re-firing the identical
    /// event sequence equivalent to what happened live before a restart.
    ///
    /// ⚠️ **An illegal transition poisons only its own group, not the
    /// whole replay — round-1 review's own finding.** `join_group::round.rs`
    /// does not durably log `Join`/`JoinBarrierComplete`/`MemberJoinedDuringSync`
    /// yet (`M4.15d`), so *every* group a real client ever joins durably
    /// records `sync_group.rs`'s own `SyncComplete` (or `heartbeat.rs`'s
    /// own eviction) with no durable predecessor — replaying that entry
    /// against a fresh, `Empty` coordinator is `IllegalGroupTransition` by
    /// construction, on essentially every restart of a broker that has
    /// served real traffic. Aborting the whole function on the first such
    /// error (this function's own first version) would have left `Cluster`
    /// permanently `COORDINATOR_LOAD_IN_PROGRESS` for *every* group and
    /// every offset operation too (`tokio::join!`'s own all-or-nothing
    /// gate in `cluster.rs`), not merely the one group whose own durability
    /// this row does not yet cover — a regression far worse than the
    /// "does not survive a restart" degradation this row's own module doc
    /// already names as accepted. Instead: a group whose replay hits an
    /// illegal transition is marked poisoned and every later entry for it
    /// is skipped (not applied, not retried) — it simply never accrues a
    /// record, `coordinator.record` answering `None` for it exactly as it
    /// would have before this row existed. Every *other* group, and the
    /// offset replay running beside this one, are unaffected.
    ///
    /// # Errors
    ///
    /// Whatever `log.read_from` fails with — a genuine I/O failure is
    /// still fatal to the whole replay, unlike a single group's own
    /// illegal transition.
    pub(crate) async fn replay(
        &self,
        coordinator: &dyn GroupCoordinator,
        log: &dyn GroupMetadataLog,
    ) -> Result<()> {
        let mut start = CommitVersion::ZERO;
        let mut poisoned: std::collections::HashSet<GroupId> = std::collections::HashSet::new();
        loop {
            let page = log.read_from(start, REPLAY_PAGE_SIZE).await?;
            let got = page.len();
            for entry in &page {
                if let GroupMetadataRecord::GroupTransitioned { group, event } = entry.record() {
                    if poisoned.contains(group) {
                        continue;
                    }
                    match coordinator.transition(group, *event) {
                        Ok(_) => {}
                        Err(Error::IllegalGroupTransition { .. }) => {
                            poisoned.insert(group.clone());
                        }
                        Err(other) => return Err(other),
                    }
                }
            }
            // ⚠️ A short page means the log has no more —
            // `GroupMetadataLog`'s own guarantee 5, `CommittedOffsets::replay`'s
            // own identical loop shape.
            let Some(last) = page.last() else {
                break;
            };
            if got < REPLAY_PAGE_SIZE {
                break;
            }
            start = last.version().advance(1)?;
        }
        Ok(())
    }

    /// Serves every enqueued request forever, in the order it arrives —
    /// the actual single-writer seam, run only after [`Self::replay`] has
    /// already brought `coordinator` up to date. Never returns under
    /// ordinary operation; ends only if every [`GroupTransitions`] handle
    /// (and therefore every sender) has been dropped.
    pub(crate) async fn serve(
        mut self,
        coordinator: Arc<dyn GroupCoordinator>,
        log: Arc<dyn GroupMetadataLog>,
    ) {
        while let Some(Request {
            group,
            event,
            respond_to,
        }) = self.receiver.recv().await
        {
            let result = handle_one(coordinator.as_ref(), log.as_ref(), &group, event).await;
            let _ = respond_to.send(result);
        }
    }
}

/// Validates `event` against `group`'s own current record — a pure,
/// side-effect-free check, since [`GroupState::transition`] never mutates
/// — durably appends it only if legal, and only then applies it to the
/// live `coordinator`.
///
/// ⚠️ **The dry run and the real call are guaranteed to agree *for a
/// group only `M4.15c`'s own three migrated call sites ever touch*.**
/// [`GroupTransitionsTask::serve`] processes one request at a time, so
/// nothing enqueued through [`GroupTransitions`] can change `group`'s own
/// record between the dry run above and the real call below. It is not
/// yet true universally: `join_group::round.rs` still calls
/// `coordinator.transition` directly (module doc's own "only three of
/// four" scoping, `M4.15d` closes the gap) — a `JoinGroup` racing a
/// heartbeat eviction or a `LeaveGroup` for the same group can still
/// invalidate this dry run between the two calls, the identical
/// cross-lock race `heartbeat.rs`'s own module doc already names and
/// does not claim to have fixed.
async fn handle_one(
    coordinator: &dyn GroupCoordinator,
    log: &dyn GroupMetadataLog,
    group: &GroupId,
    event: GroupEvent,
) -> Result<GroupRecord> {
    let current = coordinator.record(group).unwrap_or(GroupRecord {
        state: GroupState::Empty,
        generation: GenerationId::INITIAL,
        assignment_epoch: AssignmentEpoch::INITIAL,
    });
    // Refused here, nothing is appended and the live coordinator is never
    // touched — "a refused fold leaves nothing applied," `GroupCoordinator`'s
    // own doc, honoured before any I/O rather than after.
    current
        .state
        .transition(event, current.generation, current.assignment_epoch)?;

    append_durably(log, group, event).await?;

    coordinator.transition(group, event)
}

/// Durably appends `GroupTransitioned { group, event }`, retrying a
/// racing writer's own version — `CommittedOffsets::commit`'s own shape:
/// the log's own [`Error::NonMonotonicCommitVersion`] validation is the
/// real serialization point, not a lock this function holds.
async fn append_durably(
    log: &dyn GroupMetadataLog,
    group: &GroupId,
    event: GroupEvent,
) -> Result<()> {
    let record = GroupMetadataRecord::GroupTransitioned {
        group: group.clone(),
        event,
    };
    for _ in 0..MAX_APPEND_RETRIES {
        let next = match log.last_version().await? {
            Some(last) => last.advance(1)?,
            None => CommitVersion::ZERO,
        };
        let entry = GroupMetadataEntry::new(next, record.clone());
        match log.append(std::slice::from_ref(&entry)).await {
            Ok(()) => return Ok(()),
            Err(Error::NonMonotonicCommitVersion { .. }) => {
                // A racing writer (another transition, or an OffsetCommit
                // sharing the same log) landed first; retry against the
                // new last_version rather than fail a legal transition.
            }
            Err(other) => return Err(other),
        }
    }
    Err(Error::Transient)
}

#[cfg(test)]
mod tests;
