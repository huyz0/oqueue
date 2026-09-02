//! `Heartbeat` (12), v0-4 — `M4.9`.
//!
//! ⚠️ **No background reaper.** `fetch::park`'s own module doc argues
//! against a poll interval of the broker's own; the same instinct applies
//! here. A timed-out member is noticed lazily, by whichever `Heartbeat`
//! call next touches its own group — every call sweeps its own group for
//! anyone past their own deadline before answering itself, so a member's
//! own silence is discovered by another member's own next heartbeat, not
//! by a clock this broker runs on its own.
//!
//! ⚠️ **Session timeouts are learned from `JoinGroup`, never resent.**
//! `HeartbeatRequest` carries no timeout field on the wire (confirmed
//! against the dependency's own generated source) — `join_group.rs`'s own
//! handler registers each enrolled member's own `session_timeout_ms` here
//! the moment it is answered, and this module remembers it for every
//! later renewal.
//!
//! ⚠️ **A generation mismatch answers `REBALANCE_IN_PROGRESS`, uniformly.**
//! Real Kafka distinguishes a member that never belonged
//! (`UNKNOWN_MEMBER_ID`/`ILLEGAL_GENERATION`) from one whose group has
//! since rebalanced; that distinction is `M4.11`'s own audited fencing
//! path, not this task's to invent ad hoc — every mismatch here is told
//! the same thing: rejoin.
//!
//! ⚠️ **Eviction, and `M4.10`'s own explicit `LeaveGroup`, both reuse
//! `GroupEvent::Join`** — the same event `join_group`'s own handler fires
//! for a membership change on an already-`Stable` group, since the state
//! machine has no event distinct for a member *leaving* rather than
//! joining. `Heartbeats::remove_where` is the one place either path goes
//! through: `sweep` removes by deadline, `leave` removes by name, and both
//! fire at most one transition regardless of how many members left in the
//! same pass (`M4.10`'s own "one rebalance, not N" acceptance criterion).
//! If removal empties a group's own tracked membership entirely,
//! [`oqueue_core::GroupEvent::AllMembersGone`] is fired instead, matching
//! what an empty group actually is.
//!
//! ⚠️ **This module's own transition call does not open a round in
//! `join_group::round`'s own bookkeeping** — `entry.open` only ever hears
//! about a round `join_group.rs`'s own handler started. Round-1 review
//! found this originally left undefended: a group this module moves to
//! `PreparingRebalance` looked, to `join_group::round::open_round`, like
//! its own internal invariant violated (`entry.open` says no round is
//! open; the coordinator disagrees), which it refused as a bug — wedging
//! every subsequent `JoinGroup` for that group permanently, since nothing
//! else can ever fire `JoinBarrierComplete` for a round `join_group::round`
//! never opened. Fixed in `open_round` itself: `PreparingRebalance` with no
//! locally-open round now starts collecting rather than refusing.
//! `M4.10`'s own `LeaveGroup` task reaches the identical arm, for the
//! identical reason — another external event moving a `Stable` group off
//! it without `join_group::round` ever knowing — and needs no further
//! change there, the fix already being general.
//!
//! ⚠️ **A narrower, related race remains, named rather than fixed —
//! `M4.10`'s own round-1 review widened what it covers.** `sweep`/`leave`
//! decide `AllMembersGone` purely from this module's own tracked
//! membership hitting zero — which is a proxy for "the group is truly
//! empty," not the group's own real membership. `remove_where` now holds
//! its own lock across the coordinator call (round-1 review, below), which
//! closes every race *between* calls that go through this function — two
//! concurrent `leave`s, or a `leave` racing a `sweep`, for the same group.
//! It does **not** close a race against `join_group::round`, which
//! synchronizes through a wholly separate lock: a `JoinGroup` can still
//! read `coordinator.record`, decide, and apply its own transition
//! *between* this function's own removal-count decision and its
//! `coordinator.transition` call, because the two paths hold different
//! mutexes over different structures. Concretely — a `leave`/`sweep` that
//! empties this module's own tracked membership for a `Stable` group,
//! racing a `JoinGroup` for a brand-new member that reaches
//! `JoinBarrierComplete` before this function's own `AllMembersGone` call
//! lands — finds `(CompletingRebalance, AllMembersGone)` legal (it is,
//! for the "sole member leaves mid-sync" case this arm also serves) and
//! applies anyway, discarding the round `JoinGroup` just closed. ⚠️ **This
//! is not the permanent wedge the fix above closes**: the group lands on
//! `Empty`, which `open_round` treats as an ordinary "nobody's joined yet"
//! start — the bounced member's own next `JoinGroup` (real clients retry
//! on `REBALANCE_IN_PROGRESS`) opens a fresh round normally. The cost is
//! one spurious extra rebalance for whoever it hits, not an outage.
//! Closing it for real needs the removal decision and the coordinator
//! apply to be atomic with `join_group::round`'s own decision, which
//! means the two locks becoming one — a larger structural change than
//! this task's own scope; worth a fencing check (`M4.11`'s own audited
//! path) or a shared lock (`M4.14`, when group state gains a durable,
//! single-writer home) rather than an ad hoc fix here.

#![allow(clippy::redundant_pub_crate)]

mod deadline;

use crate::cluster::Cluster;
use crate::connection::HandlerResponse;
use deadline::clamp_session_timeout_ms;
use oqueue_codec::apikey::ApiKey;
use oqueue_codec::error_codes;
use oqueue_codec::frame::{RequestPrelude, encode_response_header};
use oqueue_codec::heartbeat::{decode_request, encode_response};
use oqueue_core::{GroupCoordinator, GroupEvent, GroupId, GroupState};
use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};
use tokio::time::{Duration, Instant};

/// One member's own tracked session — when it next times out, and how long
/// to extend it by on its own next renewal.
#[derive(Debug, Clone, Copy)]
struct MemberSession {
    deadline: Instant,
    session_timeout_ms: i32,
}

/// Every group's own tracked membership, for session-timeout eviction.
#[derive(Debug, Default)]
pub(crate) struct Heartbeats {
    groups: Mutex<HashMap<GroupId, HashMap<String, MemberSession>>>,
}

impl Heartbeats {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<GroupId, HashMap<String, MemberSession>>> {
        self.groups.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Starts (or restarts) tracking `member_id`'s own session —
    /// `join_group.rs`'s own call, the moment a member is genuinely
    /// enrolled.
    pub(crate) fn register(&self, group: &GroupId, member_id: &str, session_timeout_ms: i32) {
        let mut groups = self.lock();
        groups.entry(group.clone()).or_default().insert(
            member_id.to_owned(),
            MemberSession {
                deadline: Instant::now()
                    + Duration::from_millis(clamp_session_timeout_ms(session_timeout_ms)),
                session_timeout_ms,
            },
        );
        drop(groups);
    }

    /// Extends `member_id`'s own deadline by its own tracked session
    /// timeout — a no-op if this group or member is not tracked (a
    /// `Heartbeat` for a member this broker never saw enrolled; validating
    /// that is `M4.11`'s own fencing path, not this call's to refuse).
    fn renew(&self, group: &GroupId, member_id: &str) {
        let mut groups = self.lock();
        if let Some(session) = groups.get_mut(group).and_then(|m| m.get_mut(member_id)) {
            session.deadline = Instant::now()
                + Duration::from_millis(clamp_session_timeout_ms(session.session_timeout_ms));
        }
        drop(groups);
    }

    /// Evicts every member of `group` whose own deadline has already
    /// passed, firing one coordinator transition if any were — module
    /// doc's own "which event" answer.
    fn sweep(&self, group: &GroupId, coordinator: &dyn GroupCoordinator) {
        let now = Instant::now();
        self.remove_where(group, coordinator, |_, session| session.deadline > now);
    }

    /// Removes every one of `member_ids` from `group`'s own tracked
    /// membership — `LeaveGroup`'s own call (`M4.10`). Batched: every
    /// named member is removed under one lock acquisition, so a request
    /// naming several members triggers at most one coordinator transition,
    /// not one per member (`M4.10`'s own acceptance criterion).
    pub(crate) fn leave(
        &self,
        group: &GroupId,
        member_ids: &[&str],
        coordinator: &dyn GroupCoordinator,
    ) {
        self.remove_where(group, coordinator, |id, _| !member_ids.contains(&id));
    }

    /// Removes every member of `group` for which `keep` returns `false`,
    /// firing one coordinator transition if any were removed — the shared
    /// "batch removal, one rebalance" shape [`sweep`](Self::sweep)
    /// (session-timeout eviction) and [`leave`](Self::leave) (an explicit
    /// `LeaveGroup`) both need.
    ///
    /// ⚠️ **The coordinator call stays inside this lock's own scope,
    /// deliberately** — `join_group::round`'s own `join_locked` holds
    /// *its* lock across its own `coordinator.transition` call for the
    /// identical reason: releasing the lock between deciding an event and
    /// firing it lets a second `remove_where` call (another `sweep`, or a
    /// second `leave`) for the same group interleave in between, deciding
    /// its own event from the same pre-removal membership count and then
    /// applying a now-stale decision. Round-1 review found this
    /// empirically reachable when the lock was released early; holding it
    /// across the call closes every race between calls that go through
    /// this one function.
    fn remove_where(
        &self,
        group: &GroupId,
        coordinator: &dyn GroupCoordinator,
        mut keep: impl FnMut(&str, &MemberSession) -> bool,
    ) {
        let mut groups = self.lock();
        let Some(members) = groups.get_mut(group) else {
            drop(groups);
            return;
        };
        let before = members.len();
        members.retain(|id, session| keep(id, session));
        let removed = members.len() != before;
        let emptied = removed && members.is_empty();
        if removed {
            let event = if emptied {
                GroupEvent::AllMembersGone
            } else {
                GroupEvent::Join
            };
            let _ = coordinator.transition(group, event);
        }
        drop(groups);
    }

    /// Whether `member_id` is currently tracked in `group` — test-only
    /// introspection with no production caller; every real decision this
    /// module makes goes through `register`/`renew`/`sweep` instead.
    ///
    /// ⚠️ **`pub(crate)`, not private** — `leave_group`'s own tests
    /// (`M4.10`) need this from a sibling module, not just this one's own
    /// `tests` submodule.
    #[cfg(test)]
    pub(crate) fn is_tracked(&self, group: &GroupId, member_id: &str) -> bool {
        let groups = self.lock();
        let tracked = groups
            .get(group)
            .is_some_and(|members| members.contains_key(member_id));
        drop(groups);
        tracked
    }
}

/// Decodes, sweeps this group for anyone timed out, and answers — or
/// closes the connection on a malformed body.
///
/// ⚠️ **Not `async`, unlike `join_group`/`sync_group`'s own handlers** —
/// `find_coordinator.rs`'s own precedent: nothing here parks, so there is
/// no `.await` for an `async fn` to own.
pub(crate) fn handle(cluster: &Cluster, prelude: RequestPrelude, body: &[u8]) -> HandlerResponse {
    let version = prelude.api_version;
    let Ok(request) = decode_request(body, version) else {
        return HandlerResponse::Close;
    };
    let Ok(group) = GroupId::new(request.group_id) else {
        return reply(prelude, version, error_codes::INVALID_REQUEST);
    };

    // Every heartbeat is what notices another member's own silence —
    // module doc's own "no background reaper" answer.
    cluster
        .heartbeats()
        .sweep(&group, cluster.group_coordinator());

    // ⚠️ **State, not only generation.** `GenerationId` itself only
    // advances at `JoinBarrierComplete` (`ADR-0033`, `M4.1`) — a group an
    // eviction just moved off `Stable` still carries its old generation
    // number until the next completed join, so a heartbeat naming that
    // same number must still be refused: `Stable` is the only state a
    // heartbeat may succeed in, matching real Kafka's own behaviour (a
    // heartbeat during `PreparingRebalance`/`CompletingRebalance` answers
    // `REBALANCE_IN_PROGRESS` regardless of whether the generation number
    // happens to still match).
    let current = cluster.group_coordinator().record(&group);
    let is_current = current.is_some_and(|record| {
        record.state == GroupState::Stable && record.generation.get() == request.generation_id
    });
    if !is_current {
        return reply(prelude, version, error_codes::REBALANCE_IN_PROGRESS);
    }

    cluster.heartbeats().renew(&group, request.member_id);
    reply(prelude, version, error_codes::NONE)
}

fn reply(prelude: RequestPrelude, version: i16, error_code: i16) -> HandlerResponse {
    let mut out = Vec::new();
    if encode_response_header(&mut out, ApiKey::Heartbeat, version, prelude.correlation_id).is_err()
    {
        return HandlerResponse::Close;
    }
    encode_response(&mut out, version, error_code);
    HandlerResponse::Reply(out)
}

#[cfg(test)]
mod tests;
