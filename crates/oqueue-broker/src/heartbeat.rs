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
//! ⚠️ **`M4.11`'s own audited fencing path now makes the distinction real
//! Kafka does.** A member that never belonged (or no longer does) is told
//! `UNKNOWN_MEMBER_ID`; one this module still tracks but naming a stale
//! generation is told `ILLEGAL_GENERATION`; one tracked, current
//! generation, but whose group has since moved off `Stable` is told
//! `REBALANCE_IN_PROGRESS` — `crate::fencing::fence`, not an ad hoc
//! `is_current` check this module used to carry.
//!
//! ⚠️ **Eviction, and `M4.10`'s own explicit `LeaveGroup`, both fire
//! [`oqueue_core::GroupEvent::MemberLeft`]** — the event `M4.34` added
//! because the state machine had none for a member *leaving* rather than
//! joining, and this module borrowed `GroupEvent::Join` instead. That is
//! right only from `Stable`: the table admits `Join` from `Empty | Stable`
//! alone, so a partial loss from either rebalancing state took a
//! transition the coordinator refused, and nothing here read the result —
//! a leader dying while its group waited on its `SyncGroup` left that
//! group holding `CompletingRebalance` at the old generation with nobody
//! able to sync it. `Heartbeats::remove_where` is the one place either path goes
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

pub(crate) mod deadline;
mod reply;

use crate::authz::{GroupAuthzContext, group_authorized};
use crate::cluster::Cluster;
use crate::connection::HandlerResponse;
use deadline::clamp_session_timeout_ms;
use oqueue_codec::error_codes;
use oqueue_codec::frame::RequestPrelude;
use oqueue_codec::heartbeat::decode_request;
use oqueue_core::{GroupEvent, GroupId, GroupState};
use reply::reply;
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

/// The three collaborators a removal has to reach, bundled.
///
/// ⚠️ **A struct because `remove_where` reached
/// `clippy::too_many_arguments` once the barrier joined the transitions
/// actor and the coordinator**, and because `cluster.rs` is at
/// `code-structure.md` rule 16's limit so the accessor could not live
/// there. ⚠️ **`join_group::round::state::Coordination` is a strict
/// superset of this since `M4.58`, and the two still do not collapse.**
/// The distinction written here was the field sets — "that one carries
/// `heartbeats` where this carries `sync_groups`" — and `M4.58` gave
/// `Coordination` a `sync_groups` too, which makes that sentence false and
/// is why it is gone. What remains true is the direction of the borrow:
/// `remove_where` is `&self` on `Heartbeats`, so a `heartbeats` field on
/// its bundle would be a second path to the map it is already mutating —
/// reachable from a caller that cannot see which handle is which.
/// ⚠️ **This is about ownership, not hold time.** That function takes
/// `self.lock()` briefly and drops it before awaiting the transition,
/// which its own doc below spends three paragraphs establishing; nothing
/// here should be read as a lock held across an `.await`.
/// ⚠️ Third wording of this paragraph in two milestones: the first two
/// were claims about field sets, which is the part that moves, and the
/// third had to be scoped because "it already holds that lock" read as the
/// opposite of what the function does. `M4.47`, `M4.58`.
pub(crate) struct Removal<'a> {
    pub(crate) transitions: &'a crate::group_transitions::GroupTransitions,
    pub(crate) coordinator: &'a dyn oqueue_core::GroupCoordinator,
    /// ⚠️ **The one `M4.47` added**, and the reason this is a struct. A
    /// member lost by any route leaves the followers parked on *its*
    /// generation's assignment barrier waiting for something that is not
    /// coming, and until `M4.47` only a leader's refused submission told
    /// them.
    pub(crate) sync_groups: &'a crate::sync_group::SyncGroups,
}

impl<'a> Removal<'a> {
    /// The three, taken off the cluster that owns them.
    ///
    /// ⚠️ **Here rather than on `Cluster`**, so the bundle is built beside
    /// the thing it is a bundle *of* — and because `cluster.rs` is at
    /// `code-structure.md` rule 16's limit, which this accessor pushed it
    /// over. A caller that reaches for two of the three and forgets the
    /// barrier is the defect `M4.47` fixed; there is now no spelling of
    /// this call that omits one.
    pub(crate) fn of(cluster: &'a Cluster) -> Self {
        Self {
            transitions: cluster.group_transitions(),
            coordinator: cluster.group_coordinator(),
            sync_groups: cluster.sync_groups(),
        }
    }
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
    async fn sweep(&self, group: &GroupId, removal: Removal<'_>) {
        let now = Instant::now();
        self.remove_where(group, removal, |_, session| session.deadline > now)
            .await;
    }

    /// Removes every one of `member_ids` from `group`'s own tracked
    /// membership — `LeaveGroup`'s own call (`M4.10`). Batched: every
    /// named member is removed under one lock acquisition, so a request
    /// naming several members triggers at most one coordinator transition,
    /// not one per member (`M4.10`'s own acceptance criterion).
    pub(crate) async fn leave(&self, group: &GroupId, member_ids: &[&str], removal: Removal<'_>) {
        self.remove_where(group, removal, |id, _| !member_ids.contains(&id))
            .await;
    }

    /// Removes every member of `group` for which `keep` returns `false`,
    /// firing one coordinator transition if any were removed — the shared
    /// "batch removal, one rebalance" shape [`sweep`](Self::sweep)
    /// (session-timeout eviction) and [`leave`](Self::leave) (an explicit
    /// `LeaveGroup`) both need.
    ///
    /// ⚠️ **The decision and the enqueue stay inside this lock's own
    /// scope, deliberately** — `join_group::round`'s own `join_locked`
    /// holds *its* lock across its own coordinator call for the identical
    /// reason: releasing the lock between deciding an event and firing it
    /// lets a second `remove_where` call (another `sweep`, or a second
    /// `leave`) for the same group interleave in between, deciding its
    /// own event from the same pre-removal membership count and then
    /// applying a now-stale decision. Round-1 review found this
    /// empirically reachable when the lock was released early; holding it
    /// across the enqueue closes every race between calls that go
    /// through this one function.
    ///
    /// ⚠️ **Since `M4.15c`, "the call" is [`GroupTransitions::enqueue`],
    /// not `GroupCoordinator::transition` directly** — enqueuing is
    /// itself synchronous (an `mpsc` send never awaits), so it is exactly
    /// as safe to hold this lock across as the old direct call was
    /// (`async-concurrency.md` rule 6 is about `.await`, not about a
    /// non-async function call). The response *is* awaited, unlike a
    /// first attempt at this retrofit assumed: the caller's own
    /// subsequent logic — `heartbeat.rs`'s own fencing decision right
    /// after `sweep`, `leave_group/tests.rs`'s own
    /// `transition_calls()` assertion right after `leave` — reads the
    /// coordinator's record expecting this exact removal to have already
    /// landed, so the await happens here, after the lock (not the
    /// decision) is released, never held across it.
    async fn remove_where(
        &self,
        group: &GroupId,
        removal: Removal<'_>,
        mut keep: impl FnMut(&str, &MemberSession) -> bool,
    ) {
        // ⚠️ **Read before the removal, and used whatever the transition
        // answers.** `MemberLeft` and `AllMembersGone` both carry the
        // generation through unchanged, so this names the same one the
        // actor would return — and unlike that record it exists on the
        // failure path too. `members.retain` below drops the member
        // *unconditionally*, before the enqueue, so a refused or
        // unavailable transition still leaves it gone: the coordinator was
        // not moved, but the member was, and its generation's assignment is
        // not coming either. The first version of this called `refuse` only
        // on `Ok` and said in a comment that the failure path owed nothing
        // — review proved otherwise by refusing the metadata append and
        // watching the follower wait out `MAX_SYNC_WAIT_MS` for `-1`, which
        // is the row's own defect one path over. `submit_assignment`'s own
        // `Err(_)` arm refuses for this identical outage.
        let generation = removal
            .coordinator
            .record(group)
            .map(|r| r.generation.get());
        let receiver = {
            let mut groups = self.lock();
            let receiver = groups.get_mut(group).and_then(|members| {
                let before = members.len();
                members.retain(|id, session| keep(id, session));
                let removed = members.len() != before;
                if !removed {
                    return None;
                }
                let event = if members.is_empty() {
                    GroupEvent::AllMembersGone
                } else {
                    GroupEvent::MemberLeft
                };
                Some(removal.transitions.enqueue(group.clone(), event))
            });
            drop(groups);
            receiver
        };
        let Some(receiver) = receiver else { return };
        let _ = receiver.await;

        // ⚠️ **And the followers already parked on the sync barrier**, which
        // `M4.43` built the channel for and wired to `submit_assignment`
        // alone. `M4.34` makes the coordinator reopen the barrier when a
        // member is lost from `CompletingRebalance`; until `M4.47` nothing
        // told the members already waiting on the *assignment* barrier, so
        // a follower whose leader left waited out `MAX_SYNC_WAIT_MS` — 50
        // minutes — and was answered `UNKNOWN_SERVER_ERROR`. Measured
        // before the fix at `waited_ms=3000000`.
        //
        // ⚠️ **Here rather than at the two call sites**, because this
        // function's own doc calls itself "the one place either path goes
        // through": a third caller gets the wake by construction instead of
        // by remembering. That is the property the fix it repairs did not
        // have, four times over in this milestone.
        //
        // ⚠️ **`refuse` never overwrites an assignment**, so calling it on
        // every removal is safe: a leader that submitted before a follower
        // left keeps its published map (the guard declines to unpublish
        // this generation's own), and a later generation's map is
        // protected by the same guard's `>` half.
        if let Some(generation) = generation {
            removal.sync_groups.refuse(group, generation);
        }
    }

    /// Whether `member_id` is tracked **and** its session has not run out.
    ///
    /// ⚠️ **Tracked is not the same as alive, and `M4.16` is why that
    /// matters.** Nothing reaps in the background — `sweep` runs only when
    /// some *other* member of the same group sends a `Heartbeat` — so a
    /// consumer killed with `SIGKILL` stays in this map indefinitely, and in a
    /// single-consumer group there is nobody left to trigger the sweep at all.
    /// A round that waits for every *tracked* id therefore waits for the dead
    /// one until its own `rebalance_timeout_ms` expires: 300 s with the Java
    /// consumer's defaults, against a 45 s session timeout. Reading the
    /// deadline directly bounds that wait at the session timeout instead.
    ///
    /// ⚠️ **That is a tighter bound than Kafka's own**, which is
    /// `rebalance.timeout.ms`, and it is only sound because `handle` renews
    /// a session before answering `REBALANCE_IN_PROGRESS` — so a member
    /// heartbeating through a long rebalance stays live and is still waited
    /// for. Remove that renewal and this predicate starts dropping healthy
    /// incumbents from the roster. Found by review.
    pub(crate) fn is_live(&self, group: &GroupId, member_id: &str) -> bool {
        let now = Instant::now();
        let groups = self.lock();
        let live = groups
            .get(group)
            .and_then(|members| members.get(member_id))
            .is_some_and(|session| session.deadline > now);
        drop(groups);
        live
    }

    /// The latest session deadline among `member_ids`, if any of them is
    /// tracked at all — **whether or not it has already passed**.
    ///
    /// ⚠️ **The instant at which waiting for this set could become
    /// pointless.** A round waits for the members in `OpenRound::awaiting`,
    /// and nothing strikes a *dead* id off that set except
    /// `GroupJoins::reprune_awaiting`. Waking at this instant and re-pruning
    /// is what stops a round whose awaited members have all died from sitting
    /// until its own `rebalance_timeout_ms` — 300 s with the Java consumer's
    /// defaults. A member that is genuinely alive has renewed by then and
    /// pushed its own deadline past it, so the wake re-arms rather than
    /// closing on a live member.
    ///
    /// ⚠️ **No liveness filter here, deliberately, and the caller owes one.**
    /// The one caller has just retained only live ids, so re-testing
    /// `deadline > now` would be a second copy of `is_live`'s own boundary
    /// that no test could tell apart. ⚠️ **A caller that does not prune first
    /// gets a past instant back**, and a past instant used as a wake-up time
    /// is a hot loop until the round's deadline — so prune, then ask. `None`
    /// therefore means "none of these ids is tracked", which after that prune
    /// is the same as "nothing left to wait for". Found by review.
    pub(crate) fn latest_deadline(
        &self,
        group: &GroupId,
        member_ids: &[String],
    ) -> Option<Instant> {
        let groups = self.lock();
        let latest = groups.get(group).and_then(|members| {
            member_ids
                .iter()
                .filter_map(|id| members.get(id).map(|session| session.deadline))
                .max()
        });
        drop(groups);
        latest
    }

    /// Whether `member_id` is currently tracked in `group` — `M4.11`'s own
    /// real caller: `crate::fencing::FencingContext::member_tracked` for
    /// every `M4.7`-`M4.10` handler now reads this, not only tests.
    ///
    /// ⚠️ **Says nothing about whether that member is still alive**; see
    /// [`Heartbeats::is_live`], which is the one a round's roster must use.
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
/// ⚠️ **`async` since `M4.15c`** — `sweep`'s own enqueue through
/// `crate::group_transitions` must be awaited before the fencing check
/// below reads the coordinator's record, or this handler could answer on
/// a record that has not yet caught up to the eviction it just decided.
pub(crate) async fn handle(
    cluster: &Cluster,
    prelude: RequestPrelude,
    body: &[u8],
    authz: &GroupAuthzContext<'_>,
) -> HandlerResponse {
    let version = prelude.api_version;
    let Ok(request) = decode_request(body, version) else {
        return HandlerResponse::Close;
    };
    let Ok(group) = GroupId::new(request.group_id) else {
        return reply(prelude, version, error_codes::INVALID_REQUEST);
    };
    if !group_authorized(&group, authz) {
        return reply(prelude, version, error_codes::GROUP_AUTHORIZATION_FAILED);
    }

    // Every heartbeat is what notices another member's own silence —
    // module doc's own "no background reaper" answer.
    cluster
        .heartbeats()
        .sweep(&group, Removal::of(cluster))
        .await;

    // ⚠️ **`M4.11`'s own audited seam, not an ad hoc `is_current` check.**
    // `Stable` is the only state a heartbeat may succeed in, matching real
    // Kafka's own behaviour — a heartbeat during `PreparingRebalance`/
    // `CompletingRebalance` answers `REBALANCE_IN_PROGRESS` regardless of
    // whether the generation number happens to still match, since
    // `GenerationId` itself only advances at `JoinBarrierComplete`
    // (`ADR-0033`, `M4.1`) and a group an eviction just moved off `Stable`
    // still carries its old generation number until the next completed
    // join. A member this module never tracked at all (or no longer does)
    // is told `UNKNOWN_MEMBER_ID` instead, distinct from a tracked member
    // naming a stale generation (`ILLEGAL_GENERATION`) — the distinction
    // this module's own doc used to defer to this task by name.
    let member_tracked = cluster.heartbeats().is_tracked(&group, request.member_id);
    let record = cluster.group_coordinator().record(&group);
    let ctx = crate::fencing::FencingContext::for_this_node(
        cluster.replay_in_progress(),
        member_tracked,
        record.as_ref(),
        Some(request.generation_id),
        Some(&[GroupState::Stable]),
    );
    if let Err(refusal) = crate::fencing::fence(&ctx) {
        // ⚠️ **A rebalance renews the session; it does not suspend it.**
        // `REBALANCE_IN_PROGRESS` is the answer to a *correctly behaving*
        // consumer — tracked, current generation, group merely mid-round —
        // and returning it without renewing means a rebalance that outlives
        // `session_timeout_ms` expires every member still faithfully
        // heartbeating through it. That matters now that `M4.16` prunes
        // `OpenRound::awaiting` by `is_live`: the incumbent holding the
        // partitions drops out of the roster it is being waited for in, the
        // round closes without it, and the leader hands its partitions to
        // somebody else while it is still consuming them — `FR-20`'s
        // revoke-before-reassign invariant failing by a second route.
        // Real Kafka renews here too: `handleHeartbeat` calls
        // `completeAndScheduleNextHeartbeatExpiration` *before* answering
        // `REBALANCE_IN_PROGRESS`. Found by review.
        //
        // ⚠️ **Only this refusal renews.** `UNKNOWN_MEMBER_ID` has nothing to
        // renew, and `ILLEGAL_GENERATION` is a member that has lost track of
        // the group — neither is evidence of a healthy session.
        if matches!(refusal, crate::fencing::Refusal::RebalanceInProgress) {
            cluster.heartbeats().renew(&group, request.member_id);
        }
        return reply(prelude, version, refusal.error_code());
    }

    cluster.heartbeats().renew(&group, request.member_id);
    reply(prelude, version, error_codes::NONE)
}

#[cfg(test)]
mod tests;
