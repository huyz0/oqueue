//! The synchronous half of the join barrier: every decision that can be made
//! under [`super::GroupJoins`]'s own mutex, with none of the awaiting.
//!
//! ⚠️ **The split is `M4.15d`'s, and it follows the lock.** Routing the
//! round's own transitions through
//! [`crate::group_transitions::GroupTransitions`] made two of them `async`,
//! which `async-concurrency.md` rule 6 forbids doing under a
//! `std::sync::Mutex` — so each operation became "decide under the lock,
//! apply with it released, finalize under it again". This module is every
//! *decide* and every *finalize*; `super`'s own `join` and
//! `close_on_deadline` are the `.await`s between them. Keeping them in one
//! file put it at 671 lines against `code-structure.md` rule 16's 500.

use super::GroupJoins;
use super::state::{
    Coordination, Entry, INITIAL_REBALANCE_DELAY, JoinOutcome, OpenRound, RoundClose, RoundMember,
    RoundOutcome, Step,
};
use oqueue_core::{Candidate, GenerationId, GroupCoordinator, GroupEvent, GroupId, GroupState};
use std::collections::HashMap;
use std::sync::{Arc, OnceLock};
use tokio::sync::Notify;
use tokio::time::{Duration, Instant};

/// Each member's own protocol names, borrowed from `members` — the scratch
/// storage a caller must keep alive alongside `members` itself for as long
/// as the [`Candidate`] slices built over it are in use.
///
/// ⚠️ **Its own step, not folded into building `Candidate`s directly.**
/// `Candidate::protocols` is a borrowed slice (`&'a [&'a str]`), so its own
/// backing storage must outlive the `elect` call it feeds — a `Vec<&str>`
/// built and returned from *this* function lives in the caller's own scope,
/// where `elect` is then called, rather than inside a deeper helper whose
/// locals do not live that long.
fn protocol_names(members: &[RoundMember]) -> Vec<Vec<&str>> {
    members
        .iter()
        .map(|m| m.protocols.iter().map(|(name, _)| name.as_str()).collect())
        .collect()
}

/// Builds [`Candidate`]s over `members`, using `names` (this group's own
/// [`protocol_names`] output) for each one's own protocol list.
fn as_candidates<'a>(members: &'a [RoundMember], names: &'a [Vec<&'a str>]) -> Vec<Candidate<'a>> {
    members
        .iter()
        .zip(names)
        .map(|(m, protocols)| Candidate {
            member_id: m.member_id.as_str(),
            protocols: protocols.as_slice(),
        })
        .collect()
}

/// Which event opens a round for `group`, given the coordinator's own
/// current state — the decision half of what used to be `open_round`.
///
/// `None` means "a round is already open at the coordinator and needs no
/// event"; `Err(())` means the state admits no legal opening at all.
pub(super) fn opening_event(
    coordinator: &dyn GroupCoordinator,
    group: &GroupId,
) -> Result<Option<GroupEvent>, ()> {
    match coordinator.record(group).map(|r| r.state) {
        None | Some(GroupState::Empty | GroupState::Stable) => Ok(Some(GroupEvent::Join)),
        Some(GroupState::CompletingRebalance) => Ok(Some(GroupEvent::MemberJoinedDuringSync)),
        // ⚠️ **Already `PreparingRebalance`, with no locally-open round —
        // not this module's own bug.** An external actor moved the
        // coordinator here directly: `heartbeat.rs`'s own eviction sweep
        // fires `GroupEvent::Join` on a partial membership loss without
        // ever touching this bookkeeping (`M4.9`'s own finding, fixed
        // there rather than deferred once it turned out to permanently
        // wedge a group — every future `JoinGroup` would otherwise see
        // exactly this state and refuse forever, since nothing else can
        // ever fire `JoinBarrierComplete` for a round this module never
        // opened). The round genuinely is open already, just not tracked
        // here yet — start collecting without re-firing `Join`, which
        // has no legal arm from `PreparingRebalance` in the first place.
        Some(GroupState::PreparingRebalance) => Ok(None),
        // `Dead`: unreachable, module doc above.
        Some(GroupState::Dead) => Err(()),
    }
}

/// Installs a fresh, empty round on `group`'s own entry — the application
/// half of what used to be `open_round`, run only once the opening event
/// has durably landed.
pub(super) fn install_round(
    entries: &mut HashMap<GroupId, Entry>,
    group: &GroupId,
    rebalance_timeout: Duration,
) {
    let entry = entries.entry(group.clone()).or_default();
    // ⚠️ **Every id from the previous roster, live or not.** Liveness belongs
    // to `GroupJoins::reprune_awaiting` alone — filtering here as well was a
    // second copy nothing could pin, since the re-prune reaches the same state
    // on the waiter's first pass. A roster whose members are all dead empties
    // there and closes the round at once rather than on its deadline, which is
    // what the single-consumer restart needs.
    //
    // ⚠️ `None` means "no previous roster at all" — a brand-new group — and
    // that round waits out its deadline, because a roster it never had cannot
    // tell it anything. That is *not* the same as a roster that exists and has
    // died, which is why this is not collapsed to `None`.
    let awaiting = entry.last_round_members.clone();
    let first_round = awaiting.is_none();
    entry.open = Some(OpenRound {
        members: Vec::new(),
        awaiting,
        // ⚠️ **A round with no roster to wait for closes on the initial
        // delay, not on the client's own timeout.** Nothing can empty an
        // `awaiting` that is `None`, so this deadline is the *only* thing
        // that closes such a round — and `rebalance_timeout` is 300 s with
        // either real client's defaults. `min`, because a client asking for
        // less than the delay must still get the answer it asked for.
        deadline: Instant::now()
            + if first_round {
                INITIAL_REBALANCE_DELAY.min(rebalance_timeout)
            } else {
                rebalance_timeout
            },
        notify: Arc::new(Notify::new()),
        outcome: Arc::new(OnceLock::new()),
    });
}

/// Decides one pass of [`GroupJoins::join`] under the lock.
///
/// ⚠️ **Enrolment happens here, and it is deliberately synchronous.**
/// Admitting a member changes no coordinator state — it is this module's
/// own bookkeeping plus `M4.6`'s own pure `elect` — so it needs no actor
/// round trip and gains nothing from one. Only the two *transitions*
/// (opening a round, closing it) leave this lock.
pub(super) fn plan_join(
    entries: &mut HashMap<GroupId, Entry>,
    co: Coordination<'_>,
    group: &GroupId,
    member: &RoundMember,
    rebalance_timeout: Duration,
) -> Step {
    let needs_open = entries
        .get(group)
        .is_none_or(|e| e.open.as_ref().is_none_or(|r| r.outcome.get().is_some()));

    if needs_open && let Some(step) = plan_open(entries, co, group, rebalance_timeout) {
        return step;
    }

    let Some(entry) = entries.get_mut(group) else {
        return Step::Done(JoinOutcome::Refused);
    };
    let Some(round) = entry.open.as_mut() else {
        return Step::Done(JoinOutcome::Refused);
    };

    let mut trial = round.members.clone();
    if let Some(existing) = trial.iter_mut().find(|m| m.member_id == member.member_id) {
        *existing = member.clone();
    } else {
        trial.push(member.clone());
    }
    let names = protocol_names(&trial);
    if oqueue_core::elect(&as_candidates(&trial, &names)).is_none() {
        return Step::Done(JoinOutcome::Refused);
    }
    round.members = trial;
    // ⚠️ **Closed when every member the group already had has rejoined**, not
    // when a count is reached. A newcomer is not in `awaiting`, so it cannot
    // close a round on its own and leave the incumbent holding partitions
    // nobody asked it to give up — see `OpenRound::awaiting`.
    // ⚠️ **Only the enroller is struck off here — liveness is not consulted.**
    // Striking off ids that have *died* is `GroupJoins::reprune_awaiting`'s
    // job, and it runs at the top of every `wait_for_close` pass, so a member
    // that dies at any point is dropped there. Testing liveness here as well
    // was redundant: deleting it left the whole suite green, because the
    // re-prune reached the same state one hop later. Three copies of a
    // predicate, two of which nothing could pin, is what `is_own_open_round`
    // was just collapsed for. Found by review.
    if let Some(awaiting) = round.awaiting.as_mut() {
        awaiting.retain(|id| id != &member.member_id);
    }
    let full = round.awaiting.as_ref().is_some_and(Vec::is_empty);

    // ⚠️ **Claim the slot so neither a second joiner nor a waiter's own
    // deadline fires `JoinBarrierComplete` for this round as well.**
    //
    // ⚠️ **And if somebody else already holds it, this member waits on the
    // *round*, not on the slot.** `round.members` was mutated above, so by
    // the time this runs the member is already enrolled — handing back
    // `Step::Wait`, which carries only the slot's `Notify`, dropped it into a
    // round it is a member of with no handle to the close it is part of. The
    // holder's `finalize_close` then answers every *other* member with a
    // roster naming this one, and this one re-plans into the next round and
    // sits out a whole rebalance timeout while the leader assigns it
    // partitions at a generation it will never sync at. Found by review.
    // `Pending` is correct here for the same reason it is correct below: the
    // member is in the round, and whoever closes it publishes to the very
    // `outcome` handed back.
    if full && GroupJoins::claim_or_wait(entries, group).is_none() {
        return Step::Close;
    }

    let Some(round) = entries.get(group).and_then(|e| e.open.as_ref()) else {
        return Step::Done(JoinOutcome::Refused);
    };
    Step::Done(JoinOutcome::Pending {
        notify: Arc::clone(&round.notify),
        deadline: round.deadline,
        outcome: Arc::clone(&round.outcome),
    })
}

/// Ensures a round is open for `group`, or says what must happen first.
///
/// `None` means "a round is open now, carry on enrolling"; `Some(step)` is
/// work [`GroupJoins::join`] must do with the lock released.
pub(super) fn plan_open(
    entries: &mut HashMap<GroupId, Entry>,
    co: Coordination<'_>,
    group: &GroupId,
    rebalance_timeout: Duration,
) -> Option<Step> {
    if let Some(notify) = GroupJoins::claim_or_wait(entries, group) {
        return Some(Step::Wait(notify));
    }
    // ⚠️ **No re-check of "is a round already open" here, and that is
    // deliberate.** An earlier version had one, with a comment about the task
    // that just released the slot having installed the round — but this
    // function has one caller, guarded by `needs_open`, under the *same*
    // guard, and `claim_or_wait` touches only `in_flight`. The branch was the
    // exact negation of `needs_open` and had never run. Found by review:
    // documenting protection that the lock discipline is actually providing
    // is how a later refactor gets judged safe on a check that does nothing.
    match opening_event(co.coordinator, group) {
        // The slot stays claimed: `join` releases it once the transition
        // has landed, or failed.
        Ok(Some(event)) => Some(Step::Open(event)),
        // ⚠️ **Already `PreparingRebalance` at the coordinator: nothing to
        // fire, so nothing to await.** Install the round and carry straight
        // on to enrolment — there is no transition, so no reason to give up
        // the lock and re-plan.
        Ok(None) => {
            install_round(entries, group, rebalance_timeout);
            GroupJoins::release(entries, group);
            None
        }
        Err(()) => {
            GroupJoins::release(entries, group);
            Some(Step::Done(JoinOutcome::Refused))
        }
    }
}

/// What a deadline-driven close may do, decided under the lock.
pub(super) enum DeadlineClaim {
    /// No round of this caller's own is open and unclosed — nothing to do.
    NotMine,
    /// Someone else holds this group's own in-flight slot.
    Wait(Arc<Notify>),
    /// This caller holds the slot and must close the round.
    Claimed,
}

/// Whether `round` is the caller's own round, and still open.
///
/// ⚠️ `own_outcome` is how a stale waiter is told its own round is gone, not
/// the group's current one. `entry.open` is *replaced* the moment a new round
/// opens, keyed only by `GroupId` — so a waiter whose own round already closed
/// without waking it would otherwise, on its own stale deadline, act on
/// whatever round is open *now*, a different generation than the one it ever
/// joined. Pointer equality is what tells "my round, not yet closed" apart
/// from "a round, but not mine".
///
/// ⚠️ **One copy, called from every place that needs it.** It was written out
/// three times — `deadline_claim`, `GroupJoins::reprune_awaiting` and
/// `GroupJoins::withdraw` — and the copies no test reached were invisible
/// until `cargo mutants` flipped one's `||` to `&&` and nothing failed. Both
/// later copies were found by review, the second only after this comment
/// claimed there were two. ⚠️ **Do not write the predicate out again**: it
/// decides whether a stale waiter may act on a round that is no longer its
/// own, and a copy nothing exercises is a copy that can drift silently.
pub(super) fn is_own_open_round(
    round: &OpenRound,
    own_outcome: &Arc<OnceLock<RoundOutcome>>,
) -> bool {
    Arc::ptr_eq(&round.outcome, own_outcome) && round.outcome.get().is_none()
}

pub(super) fn deadline_claim(
    entries: &mut HashMap<GroupId, Entry>,
    group: &GroupId,
    own_outcome: &Arc<OnceLock<RoundOutcome>>,
) -> DeadlineClaim {
    let Some(entry) = entries.get(group) else {
        return DeadlineClaim::NotMine;
    };
    let Some(round) = entry.open.as_ref() else {
        return DeadlineClaim::NotMine;
    };
    if !is_own_open_round(round, own_outcome) {
        return DeadlineClaim::NotMine;
    }
    GroupJoins::claim_or_wait(entries, group).map_or(DeadlineClaim::Claimed, DeadlineClaim::Wait)
}

/// Drops `group`'s own open round and wakes whoever was waiting on it.
///
/// ⚠️ **A refused `JoinBarrierComplete` must destroy the round, not leave it
/// open.** Round two of this task's review replaced a wrong-generation
/// success with "return `None` and let the caller re-plan" — but re-planning
/// cannot choose differently while a round is open, because `plan_join` only
/// consults the coordinator when it needs to *open* one. Every pass re-enrols
/// the same member, finds the round still full, and fires the same illegal
/// event: the group wedges on this node until the process restarts, and a
/// full round answers `INCONSISTENT_GROUP_PROTOCOL` after eight round trips.
/// Reachable whenever the last tracked member leaves while a replacement is
/// mid-join, which fires `AllMembersGone` and takes the group out of
/// `PreparingRebalance` — the only state a barrier is legal from. Found by
/// review, which reproduced both halves.
///
/// Waiters are notified rather than left to their deadlines: their round is
/// gone, so the sooner they learn it the sooner they retry.
pub(super) fn abandon_round(
    entries: &mut HashMap<GroupId, Entry>,
    group: &GroupId,
    forget_roster: bool,
) {
    if let Some(entry) = entries.get_mut(group)
        && let Some(round) = entry.open.take()
    {
        // ⚠️ **Publish the abandonment, do not merely drop the round.** Every
        // other member enrolled in it is parked holding this same `outcome`;
        // without a value they wait out their full rebalance timeout and are
        // then answered `UNKNOWN_SERVER_ERROR`, which the Java consumer does
        // not retry. `None` tells them at once, and `join_group::mod` answers
        // `REBALANCE_IN_PROGRESS`, which is what a client rejoins on.
        let _ = round.outcome.set(None);
        round.notify.notify_waiters();
        // ⚠️ **Forget the roster only when the members really are gone.** If
        // the barrier was *illegal* — `AllMembersGone` took the group out of
        // `PreparingRebalance` — a fresh round must not wait for members that
        // are not coming, or it waits out its own deadline too. But if the
        // append merely *failed*, every member is sitting in the next round
        // already: clearing the roster there leaves it with no early-close
        // signal, so a storage blip that healed in milliseconds costs the
        // group its whole initial-rebalance delay before anybody is answered
        // again — `INITIAL_REBALANCE_DELAY`, since a forgotten roster is a
        // round with nothing to wait for. That was a full
        // `rebalance_timeout_ms` (300 s with either real client's defaults)
        // until `M4.29` bounded it. Found by review; `apply_open` keeps the
        // same two causes apart one function away, for the same reason.
        if forget_roster {
            entry.last_round_members = None;
        }
    }
}

/// Fires `JoinBarrierComplete` through the actor and answers with the
/// generation the round closes at.
///
/// `None` means the barrier was refused — the caller must not publish a
/// close.
///
/// ⚠️ **`M4.15c` fell back to the coordinator's current generation here, and
/// that became wrong when `M4.15d` widened the window.** The fallback was
/// written for a divergence between this bookkeeping and the coordinator,
/// which two adjacent statements under one mutex made near-impossible. With
/// a durable append in between, an eviction's `AllMembersGone` landing first
/// makes `JoinBarrierComplete` illegal on an ordinary schedule — and the
/// fallback then answers every member of the round `error_code: NONE` at a
/// generation one behind, against a group now `Empty`. A silently wrong
/// success is worse than an error, so the refusal is now reported.
pub(super) async fn close_generation(co: Coordination<'_>, group: &GroupId) -> Barrier {
    match co
        .transitions
        .transition(group.clone(), GroupEvent::JoinBarrierComplete)
        .await
    {
        Ok(record) => Barrier::Closed(record.generation),
        // The group moved out from under this round — its members really are
        // gone, so the next round must not expect them.
        Err(oqueue_core::Error::IllegalGroupTransition { .. }) => Barrier::Illegal,
        // The log is unreachable. The members are all still here.
        Err(_) => Barrier::Unavailable,
    }
}

/// What firing a round's own `JoinBarrierComplete` produced.
pub(super) enum Barrier {
    Closed(GenerationId),
    /// The coordinator refused it — the group moved on under this round.
    Illegal,
    /// A dependency failed; nothing is known about the group's own state.
    Unavailable,
}

/// Elects a leader/protocol over `group`'s own round and publishes the
/// close to every waiter — the part that must happen under the lock, run
/// after `close_generation` has durably landed the transition.
///
/// # Panics
///
/// Never in practice: `round.members` is non-empty (a round only exists
/// once its first member has joined) and every member was admitted only
/// after `elect` confirmed it kept the round's own candidate set non-empty
/// — so `elect` here cannot return `None`. Falls back to the round's own
/// first member as a leaderless-protocol placeholder rather than panicking
/// if that invariant is ever violated, `security.md` rule 3's instinct.
pub(super) fn finalize_close(
    entries: &mut HashMap<GroupId, Entry>,
    group: &GroupId,
    generation: GenerationId,
) -> Option<Arc<RoundClose>> {
    let entry = entries.get_mut(group)?;
    let round = entry.open.as_mut()?;
    if let Some(already) = round.outcome.get() {
        return already.clone();
    }
    let members = std::mem::take(&mut round.members);
    let names = protocol_names(&members);
    let elected = oqueue_core::elect(&as_candidates(&members, &names));
    let (leader, protocol_name) = elected.map_or_else(
        || (members.first().map_or("", |m| m.member_id.as_str()), ""),
        |e| (e.leader, e.protocol_name),
    );
    let protocol_type = members
        .iter()
        .find(|m| m.member_id == leader)
        .map_or_else(String::new, |m| m.protocol_type.clone());
    let close = Arc::new(RoundClose {
        generation,
        leader: leader.to_owned(),
        protocol_type,
        protocol_name: protocol_name.to_owned(),
        members,
    });
    let _ = round.outcome.set(Some(Arc::clone(&close)));
    round.notify.notify_waiters();
    entry.last_round_members = Some(close.members.iter().map(|m| m.member_id.clone()).collect());
    Some(close)
}
