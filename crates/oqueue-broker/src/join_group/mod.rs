//! `JoinGroup` (11), v0-9 — `M4.7`.
//!
//! ⚠️ **Member ids are minted, never taken from the wire — except for a
//! rejoin's own fencing check, `M4.11`'s own half.** `member_id.rs`'s own
//! doc names this handler as where minting (an empty `member_id`) or
//! validating an echoed one (a rejoin) happens. An *empty* `member_id`
//! always mints a fresh one, unchecked (there is nothing to look up yet).
//! A *non-empty* one now goes through `crate::fencing::fence`: if this
//! broker's own `heartbeat.rs`'s tracking does not recognise it,
//! `UNKNOWN_MEMBER_ID` refuses the join outright rather than silently
//! admitting a made-up identity — real Kafka's own answer to a rejoin
//! naming an id it never enrolled, telling the client to retry with `""`.
//! No generation or state check: `JoinGroupRequest` carries no generation
//! field, and a tracked member may legitimately rejoin from any live
//! state.
//!
//! ⚠️ **Every join goes through [`round::GroupJoins`]**, `M4.6`'s own
//! `elect` reused both to admit a member (refusing one that would empty the
//! round's own running candidate set, `INCONSISTENT_GROUP_PROTOCOL`,
//! immediately and without enrolling it) and, once more, to close a round —
//! infallible there because every enrolled member already passed the same
//! check. See `round`'s own module doc for the full shape.
//!
//! ⚠️ **Not the sync phase.** `SyncGroup` (`M4.8`) is where the leader
//! submits an assignment and followers receive their own slice; this
//! handler only elects the leader and hands it every member's own
//! subscription metadata, opaque bytes it never reads (`M4.5`'s own
//! contract, unchanged here).

#![allow(clippy::redundant_pub_crate)]

mod deadline;
mod round;

use crate::cluster::Cluster;
use crate::connection::HandlerResponse;
use deadline::barrier_ms;
use oqueue_codec::apikey::ApiKey;
use oqueue_codec::error_codes;
use oqueue_codec::frame::{RequestPrelude, encode_response_header};
use oqueue_codec::join_group::{
    JoinGroupRequest, JoinGroupResponse, JoinGroupResponseMember, decode_request, encode_response,
};
use oqueue_core::{GroupId, MemberId};
pub(crate) use round::GroupJoins;
use round::{JoinOutcome, Reprune, RoundClose, RoundMember, effective_timeout_ms, mint_member_id};

/// The wire code a refused join answers with.
const fn refusal_code(outcome: &JoinOutcome) -> i16 {
    match outcome {
        // A dependency failed; the request was fine.
        JoinOutcome::Unavailable => crate::fencing::Refusal::CoordinatorNotAvailable.error_code(),
        // The group is contended; rejoining is the right response.
        JoinOutcome::Busy => crate::fencing::Refusal::RebalanceInProgress.error_code(),
        // ⚠️ **Listed, not a catch-all, and that is the point.** This arm is
        // the one code the Java consumer raises out of `poll()` rather than
        // retrying, and `JoinOutcome` grew two variants in two review rounds —
        // under `_ =>` the next one would default, silently, to killing the
        // consumer. Naming every variant makes it a compile error instead.
        JoinOutcome::Refused | JoinOutcome::Ready(_) | JoinOutcome::Pending { .. } => {
            error_codes::INCONSISTENT_GROUP_PROTOCOL
        }
    }
}

/// The actor and the coordinator this cluster's rounds transition through —
/// paired here so no call site can answer "which coordinator" and "which
/// actor" separately and get two different answers.
fn coordination(cluster: &Cluster) -> round::Coordination<'_> {
    round::Coordination {
        transitions: cluster.group_transitions(),
        coordinator: cluster.group_coordinator(),
        heartbeats: cluster.heartbeats(),
    }
}

/// Decodes, joins `group`'s own round (minting a member id if none was
/// given), waits for it to close, and answers — or closes the connection on
/// a malformed body.
pub(crate) async fn handle(
    cluster: &Cluster,
    prelude: RequestPrelude,
    body: &[u8],
) -> HandlerResponse {
    let version = prelude.api_version;
    let Ok(request) = decode_request(body, version) else {
        return HandlerResponse::Close;
    };
    let Ok(group) = GroupId::new(request.group_id) else {
        return reply(prelude, &refusal(error_codes::INVALID_REQUEST));
    };
    let member_id = member_id_for(request.member_id);
    if !request.member_id.is_empty()
        && let Err(refused) = fence_rejoin(cluster, &group, member_id.as_str())
    {
        return reply(prelude, &refusal(refused.error_code()));
    }
    let member = round_member(&request, member_id.as_str());
    let rebalance_timeout = tokio::time::Duration::from_millis(barrier_ms(effective_timeout_ms(
        request.rebalance_timeout_ms,
        request.session_timeout_ms,
    )));

    let close = match cluster
        .group_joins()
        .join(coordination(cluster), &group, member, rebalance_timeout)
        .await
    {
        // ⚠️ Two refusals, two codes, and only one of them is the client's
        // fault: `Unavailable` means a dependency failed, and answering it
        // `INCONSISTENT_GROUP_PROTOCOL` would kill the consumer permanently.
        // See `JoinOutcome::Unavailable`'s own doc.
        outcome @ (JoinOutcome::Refused | JoinOutcome::Unavailable | JoinOutcome::Busy) => {
            return reply(prelude, &refusal(refusal_code(&outcome)));
        }
        JoinOutcome::Ready(close) => close,
        pending @ JoinOutcome::Pending { .. } => {
            let Some(close) = wait_for_close(cluster, &group, member_id.as_str(), pending).await
            else {
                // ⚠️ **`REBALANCE_IN_PROGRESS`, not `UNKNOWN_SERVER_ERROR`,
                // and the difference is whether the client comes back.** Since
                // `M4.15d` this is reachable without anything being wrong: the
                // round was abandoned because the group moved on under it (a
                // refused barrier), or a holder of the group's own in-flight
                // slot outlasted both deadline passes. Rejoining is exactly
                // the right response to both, and it is what this code asks
                // for — `UNKNOWN_SERVER_ERROR` is not retriable in the Java
                // consumer, which raises it out of `poll()`. Found by review.
                return reply(
                    prelude,
                    &refusal(crate::fencing::Refusal::RebalanceInProgress.error_code()),
                );
            };
            close
        }
    };
    register_heartbeat(
        cluster,
        &group,
        member_id.as_str(),
        request.session_timeout_ms,
    );
    reply(prelude, &response_for(&close, member_id.as_str(), version))
}

/// `M4.11`'s own check for a rejoin naming a non-empty `member_id`: refuses
/// `UNKNOWN_MEMBER_ID` unless `heartbeat.rs`'s own tracking already
/// recognises it. No generation or state check — module doc's own reasons.
fn fence_rejoin(
    cluster: &Cluster,
    group: &GroupId,
    member_id: &str,
) -> Result<(), crate::fencing::Refusal> {
    let member_tracked = cluster.heartbeats().is_tracked(group, member_id);
    let record = cluster.group_coordinator().record(group);
    let ctx = crate::fencing::FencingContext::for_this_node(
        cluster.replay_in_progress(),
        member_tracked,
        record.as_ref(),
        None,
        None,
    );
    crate::fencing::fence(&ctx)
}

/// This member's own contribution to the round it is about to join —
/// pulled out of `handle` purely for `code-structure.md`'s own fifty-line
/// limit.
fn round_member(request: &JoinGroupRequest<'_>, member_id: &str) -> RoundMember {
    RoundMember {
        member_id: member_id.to_owned(),
        protocol_type: request.protocol_type.to_owned(),
        protocols: request
            .protocols
            .iter()
            .map(|p| (p.name.to_owned(), p.metadata.to_owned()))
            .collect(),
    }
}

/// Registers this member's own session timeout with `heartbeat.rs`'s own
/// tracking — `M4.9`'s job, called only once a member is genuinely
/// enrolled (never on [`JoinOutcome::Refused`]), so `Heartbeat`'s own
/// handler has something to track a fresh member against from the moment
/// it can legitimately send one.
fn register_heartbeat(
    cluster: &Cluster,
    group: &GroupId,
    member_id: &str,
    session_timeout_ms: i32,
) {
    cluster
        .heartbeats()
        .register(group, member_id, session_timeout_ms);
}

/// A minted id for an empty `member_id`; an echoed one accepted at face
/// value otherwise — `member_id.rs`'s own "minting or validating happens in
/// `M4.7`'s handler" doc, this task's own half of it.
fn member_id_for(requested: &str) -> MemberId {
    if requested.is_empty() {
        mint_member_id()
    } else {
        // Non-empty, and `MemberId::new` refuses only an empty string.
        MemberId::new(requested)
            .unwrap_or_else(|_| unreachable!("`requested` is checked non-empty above"))
    }
}

/// Waits out a [`JoinOutcome::Pending`] round: whichever fires first between
/// the round's own notify and its own deadline, re-checking `outcome` on
/// every wakeup — `fetch::park`'s own re-check-don't-trust-the-wakeup shape.
///
/// Returns `None` in two cases, and ⚠️ **since `M4.15d` neither is a bug**.
/// One: the round was *abandoned* — a refused `JoinBarrierComplete` destroyed
/// it and published `Some(None)`, which this returns as `None` on the first
/// pass with no deadline attempt at all. Two: the deadline has already been
/// tried once and `outcome` is still unset, which a holder of the group's own
/// in-flight slot slower than `SLOT_WAIT` reaches with nothing diverged. It
/// used to mean only that this bookkeeping and the coordinator had diverged.
/// ⚠️ The caller answers `REBALANCE_IN_PROGRESS` — **not**
/// `UNKNOWN_SERVER_ERROR`, which the Java consumer raises out of `poll()`
/// rather than retrying. Bailing out here rather than
/// looping (the deadline has already passed, so `sleep_until` would return
/// at once, forever) is `security.md` rule 3's instinct against an
/// unbounded hold applied to this module's own internal invariant instead
/// of untrusted wire bytes.
///
/// # Panics
///
/// Never: called only with [`JoinOutcome::Pending`] (the caller's own match
/// already handled [`JoinOutcome::Ready`]/[`JoinOutcome::Refused`]).
async fn wait_for_close(
    cluster: &Cluster,
    group: &GroupId,
    member_id: &str,
    pending: JoinOutcome,
) -> Option<std::sync::Arc<RoundClose>> {
    let JoinOutcome::Pending {
        notify,
        deadline,
        outcome,
    } = pending
    else {
        unreachable!("wait_for_close is only ever called with JoinOutcome::Pending")
    };
    let mut deadline_tried = false;
    loop {
        if let Some(published) = outcome.get() {
            return published.clone();
        }
        let next_check = next_wake(cluster, group, &outcome, deadline).await;
        if let Some(published) = outcome.get() {
            return published.clone();
        }
        tokio::select! {
            () = tokio::time::sleep_until(next_check) => {
                // ⚠️ **Not necessarily the round's own deadline**: an early
                // `Recheck` wake lands here too, and it has exhausted
                // nothing. `deadline_tried` is what gives up and withdraws
                // the member, so only a pass that really reached the deadline
                // may spend it.
                let at_deadline = tokio::time::Instant::now() >= deadline;
                if !at_deadline {
                    continue;
                }
                cluster
                    .group_joins()
                    .close_on_deadline(coordination(cluster), group, &outcome)
                    .await;
                // ⚠️ **Re-read the outcome before giving up.** Since
                // `M4.15d`, `close_on_deadline` can return having closed
                // nothing — another task held the group's own in-flight slot
                // and this one's bounded wait lapsed — which the synchronous
                // version it replaced could not do. Without this check a
                // member whose round *did* close, at the generation every
                // other member was answered with, is told
                // `UNKNOWN_SERVER_ERROR`. Found by review; the trigger is a
                // slow durable append behind the single transition actor, not
                // an invariant violation.
                if let Some(published) = outcome.get() {
                    return published.clone();
                }
                if deadline_tried {
                    // ⚠️ **Withdraw before answering, and do it here rather
                    // than at the call site.** This member is being told to
                    // rejoin; leaving it in the round's roster gets it
                    // assigned partitions nobody will ever consume. The
                    // withdrawal belongs to the function that decides to give
                    // up, and it needs `outcome` to prove which round the
                    // enrolment is in. See `GroupJoins::withdraw`.
                    cluster.group_joins().withdraw(group, member_id, &outcome);
                    return None;
                }
                deadline_tried = true;
            }
            () = notify.notified() => {}
        }
    }
}

/// When this waiter should next wake — and, if every member the round still
/// awaits has died, the close that makes waiting moot.
///
/// ⚠️ **This is the only place a *dead* id is struck off `awaiting`.**
/// `plan_join` strikes off the enrolling member's own id and nothing else, so
/// without this a round would wait for every member of the previous roster
/// until its own `rebalance_timeout_ms` — 300 s with the Java consumer's
/// defaults — including members that are never coming. The common case is a
/// consumer restarting *inside* its own session window: it returns under a
/// new minted id while the old one is still tracked and still live when the
/// round is installed. The count-based rule this replaced closed such a round
/// at once, which makes the stall one the roster rule introduced and has to
/// answer for itself.
///
/// ⚠️ **Liveness was once tested here *and* at install *and* on every
/// enrolment**, and review showed the other two could each be deleted with
/// the whole suite still green, because this one reaches the same state a hop
/// later. They are gone: a branch nothing can pin is a branch nothing is
/// checking. Do not add them back. Found by review.
///
/// ⚠️ **A live member has renewed before the instant returned here**, because
/// `heartbeat::handle` renews even while answering `REBALANCE_IN_PROGRESS` —
/// so an early wake either finds the member gone or pushes the next check
/// out. Without that renewal this would close rounds on members that are
/// merely mid-rebalance.
async fn next_wake(
    cluster: &Cluster,
    group: &GroupId,
    outcome: &std::sync::Arc<std::sync::OnceLock<round::RoundOutcome>>,
    deadline: tokio::time::Instant,
) -> tokio::time::Instant {
    match cluster
        .group_joins()
        .reprune_awaiting(coordination(cluster), group, outcome)
    {
        // ⚠️ **Close now, and only here.** The round waits on nobody. This is
        // not a deadline attempt and must not spend `deadline_tried`, which
        // is what eventually withdraws the member. If it closes nothing
        // (another task holds the slot) this yields `deadline` and the caller
        // sleeps there, so a failed early close costs a wait, not a spin.
        Reprune::RosterDead => {
            cluster
                .group_joins()
                .close_on_deadline(coordination(cluster), group, outcome)
                .await;
            deadline
        }
        // Never *later* than the round's own deadline, which is the
        // client-facing promise; this only ever moves the wake earlier.
        Reprune::Recheck(next) => next.min(deadline),
        // ⚠️ Not the same as `RosterDead`: a brand-new group has no roster to
        // wait for and must still wait out its deadline.
        Reprune::NotWaiting => deadline,
    }
}

/// This member's own view of `close`: the leader's own response carries
/// every member's own metadata; a follower's carries none — the acceptance
/// criterion this task's own backlog row names.
fn response_for<'a>(
    close: &'a RoundClose,
    member_id: &'a str,
    version: i16,
) -> JoinGroupResponse<'a> {
    let is_leader = close.leader == member_id;
    let members = if is_leader {
        close
            .members
            .iter()
            .map(|m| JoinGroupResponseMember {
                member_id: m.member_id.as_str(),
                group_instance_id: None,
                metadata: m
                    .protocols
                    .iter()
                    .find(|(name, _)| name == &close.protocol_name)
                    .map_or(&b""[..], |(_, metadata)| metadata.as_slice()),
            })
            .collect()
    } else {
        Vec::new()
    };
    JoinGroupResponse {
        error_code: error_codes::NONE,
        generation_id: close.generation.get(),
        protocol_type: (version >= 7).then_some(close.protocol_type.as_str()),
        protocol_name: Some(close.protocol_name.as_str()),
        leader: close.leader.as_str(),
        skip_assignment: false,
        member_id,
        members,
    }
}

/// A refusal nobody joined: no generation, no leader, no protocol.
const fn refusal(error_code: i16) -> JoinGroupResponse<'static> {
    JoinGroupResponse {
        error_code,
        generation_id: -1,
        protocol_type: None,
        protocol_name: None,
        leader: "",
        skip_assignment: false,
        member_id: "",
        members: Vec::new(),
    }
}

fn reply(prelude: RequestPrelude, response: &JoinGroupResponse<'_>) -> HandlerResponse {
    let mut out = Vec::new();
    if encode_response_header(
        &mut out,
        ApiKey::JoinGroup,
        prelude.api_version,
        prelude.correlation_id,
    )
    .is_err()
    {
        return HandlerResponse::Close;
    }
    encode_response(&mut out, prelude.api_version, response);
    HandlerResponse::Reply(out)
}

#[cfg(test)]
mod tests;
