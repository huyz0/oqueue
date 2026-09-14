//! `SyncGroup` (14), v0-5 — `M4.8`.
//!
//! ⚠️ **The leader is identified by its own submission, not by a remembered
//! identity.** `M4.7`'s own `join_group::round` module tracks who elected
//! whom, but that bookkeeping is scoped to the join barrier and is gone by
//! the time `SyncGroup` lands — real Kafka's own coordinator remembers the
//! elected leader's id across the two phases; this broker instead uses the
//! wire's own signal: the classic protocol's leader submits a non-empty
//! `assignments` list (one entry per group member), and every follower
//! submits an empty one (`oqueue_codec::sync_group`'s own doc). A member
//! whose own submission happens to carry a non-empty list is treated as
//! carrying the real assignment regardless of whether it was actually
//! elected leader — this remains structural, not a fencing gap `M4.11`'s
//! own five codes have an answer for (there is no wire code for "you were
//! not the leader," and nothing here persists an elected identity to check
//! against).
//!
//! ⚠️ **`M4.11`'s own audited seam now closes the other two.** A member
//! this broker never tracked, one naming a stale `request.generation_id`,
//! or one submitting while the group is not `CompletingRebalance` or
//! `Stable` (still `PreparingRebalance`, still collecting joins) is
//! refused via `crate::fencing::fence` before either branch below runs —
//! `UNKNOWN_MEMBER_ID`, `ILLEGAL_GENERATION`, `REBALANCE_IN_PROGRESS`
//! respectively.
//!
//! ⚠️ **No assignor runs here** — `ADR-0033`'s own decision, `M4.8`'s
//! acceptance criterion verbatim: the classic protocol computes assignment
//! client-side, and this broker only relays each member its own slice of
//! whatever the leader submitted, opaque bytes it never reads.
//!
//! ⚠️ **`CompletingRebalance -> Stable` fires the moment the assignment is
//! known**, not once every member has synced — real Kafka's own behaviour,
//! since a member's own sync can arrive well after the group is already
//! serving. A follower's `SyncGroup` after that point is answered at once
//! from the already-known assignment map, never parked.

#![allow(clippy::redundant_pub_crate)]

mod barrier;
mod deadline;

use crate::cluster::Cluster;
use crate::connection::HandlerResponse;
pub(crate) use barrier::SyncGroups;
use barrier::{Assignments, Known, assignment_for};
use deadline::MAX_SYNC_WAIT_MS;
use oqueue_codec::apikey::ApiKey;
use oqueue_codec::error_codes;
use oqueue_codec::frame::{RequestPrelude, encode_response_header};
use oqueue_codec::sync_group::{
    SyncGroupRequest, SyncGroupResponse, decode_request, encode_response,
};
use oqueue_core::{GroupEvent, GroupId, GroupState};
use std::collections::HashMap;
use tokio::sync::Notify;
use tokio::time::{Duration, Instant};

/// Decodes, submits or awaits the group's own assignment, and answers — or
/// closes the connection on a malformed body.
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

    // ⚠️ **`M4.11`'s own audited seam**: a member this broker never
    // tracked, or one naming a stale generation, or one submitting outside
    // `CompletingRebalance`/`Stable` (still `PreparingRebalance`, still
    // collecting joins) is refused before either branch below runs a
    // submission or a wait — module doc's own "both fencing gaps M4.11
    // owns" now closed.
    let member_tracked = cluster.heartbeats().is_tracked(&group, request.member_id);
    let record = cluster.group_coordinator().record(&group);
    let ctx = crate::fencing::FencingContext::for_this_node(
        cluster.replay_in_progress(),
        member_tracked,
        record.as_ref(),
        Some(request.generation_id),
        Some(&[GroupState::CompletingRebalance, GroupState::Stable]),
    );
    if let Err(refused) = crate::fencing::fence(&ctx) {
        return reply(prelude, &refusal(refused.error_code()));
    }

    if !request.assignments.is_empty() {
        return submit_assignment(cluster, prelude, &group, &request, version).await;
    }

    // A follower: answer at once if the assignment is already known,
    // otherwise wait for it (or this call's own deadline).
    let (assignments, notify) = cluster.sync_groups().entry_for(&group);
    let map = match wait_for_assignment(&assignments, &notify, request.generation_id).await {
        Known::Mine(map) => map,
        // ⚠️ **Every way of not having this generation's assignment is
        // answered the same, and `M4.48` is what made that true.** The
        // three differ in what the cell says — a later generation is
        // already there (`Superseded`), this one was refused
        // (`Refused`), or the wait simply ran out (`Waiting`) — and they
        // do not differ in what the client should do, which is rejoin.
        // Spelling them as one arm says so; `clippy::match_same_arms`
        // insists on it once they agree.
        //
        // ⚠️ **`Waiting` answered `UNKNOWN_SERVER_ERROR` until `M4.48`**,
        // while every sibling give-up arm had already been changed away
        // from it — `join_group/mod.rs`'s own two, each recording that the
        // Java consumer raises that code out of `poll()` and never
        // retries, so the difference is whether the client comes back at
        // all. Nothing enforced the rule: `check-fencing-seam.sh` covered
        // `Refusal`'s six codes and this is not one of them, which is the
        // leg `M4.48` added beside it.
        //
        // ⚠️ **`Waiting` here is reachable with nothing wrong**, which is
        // what makes a fatal answer wrong: a leader that dies between
        // `JoinBarrierComplete` and its own `SyncGroup` leaves the barrier
        // holding nothing, nothing reaps in the background, and a parked
        // follower is not heartbeating so nothing sweeps on its behalf.
        Known::Superseded | Known::Refused | Known::Waiting => {
            return reply(
                prelude,
                &refusal(crate::fencing::Refusal::RebalanceInProgress.error_code()),
            );
        }
    };
    reply(prelude, &response_for(&map, &request, version))
}

/// The assignment-bearing submission: build the map every member's own
/// slice comes from, fire the state transition, and submit it for *this*
/// generation — which `M4.35` made conditional, so a submission the group
/// has already moved past is refused rather than replacing the map that
/// overtook it (`submit`'s own doc).
///
/// ⚠️ **Its own function because `handle` reached `clippy::too_many_lines`
/// the moment the transition stopped being one discarded statement.** The
/// split is along the branch `handle` already had, so the follower path
/// below is unchanged.
/// Whether `group` is `Stable` at `generation` as of right now.
///
/// ⚠️ **Asked twice of one submission, and that is the point of `M4.65`.**
/// Once of the record `SyncComplete` produced, which is the state at the
/// moment the actor applied it; and again inside `barrier::submit`, under
/// the cell's own lock, because the group can leave the generation between
/// those two moments and the second reading is the one that authorises the
/// write. See that function's doc for why the question cannot be asked from
/// the other side of the lock.
fn is_stable_at(cluster: &Cluster, group: &GroupId, generation: i32) -> bool {
    cluster
        .group_coordinator()
        .record(group)
        .is_some_and(|r| r.state == GroupState::Stable && r.generation.get() == generation)
}

async fn submit_assignment(
    cluster: &Cluster,
    prelude: RequestPrelude,
    group: &GroupId,
    request: &SyncGroupRequest<'_>,
    version: i16,
) -> HandlerResponse {
    let map: HashMap<String, Vec<u8>> = request
        .assignments
        .iter()
        .map(|a| (a.member_id.to_owned(), a.assignment.to_owned()))
        .collect();
    // ⚠️ **An illegal transition is a lost race; anything else is a broken
    // dependency, and the two must not be conflated** -- `join_group`'s
    // `apply` and `close_generation` each say the same thing at their own
    // call site, and this was the third and last one still conflating them.
    // `transition` was pure bookkeeping when the discarded `let _ =` was
    // written; `M4.15d` put a durable append inside it, so `Err` stopped
    // meaning only "the coordinator disagrees".
    //
    // Illegal is the case that `let _ =` was written for and is right
    // about: already `Stable`, a double submission racing another
    // connection, and the map this submission carries is still the right
    // one to answer with.
    //
    // ⚠️ **An unreachable log is the case where answering `NONE` is the
    // worst available answer**, and what follows describes that *rejected*
    // answer rather than the code below — a paragraph inserted to correct
    // the generation claim orphaned the sentence, which then read as
    // annotating the branch it sits above. `M4.33` recorded it; `M4.55`
    // rewrote it. Under `NONE`, every member is told its assignment is
    // valid and starts consuming, while the group never left
    // `CompletingRebalance` -- so `fence_commit`'s own `[Stable]` fence
    // refuses every `OffsetCommit` and every heartbeat answers
    // `REBALANCE_IN_PROGRESS`, and nothing re-fires `SyncComplete` when the
    // log heals, so the group recovers only by burning a whole extra
    // generation.
    //
    // ⚠️ **`COORDINATOR_NOT_AVAILABLE` does not save the generation, and
    // saying it did was this fix's own first claim.** Both reference
    // clients rejoin on *any* non-`NONE` SyncGroup code -- the Java
    // consumer's `SyncGroupResponseHandler` calls `requestRejoin` and
    // additionally `markCoordinatorUnknown` for this one, and librdkafka
    // falls through to `rd_kafka_cgrp_rejoin` -- so the generation is spent
    // either way. ⚠️ **What the code buys is an immediate retriable answer
    // instead of a false success corrected a heartbeat later** — this said
    // "the difference between a client that comes back and one that does
    // not", which over-claims: under the rejected `NONE` the leader's next
    // heartbeat is fenced on `[Stable]`, answers `REBALANCE_IN_PROGRESS`,
    // and the Java consumer rejoins within one `heartbeat.interval.ms`. The
    // client comes back either way. What it does not do either way is
    // consume against a group the coordinator never saw reach `Stable`, and
    // closing that window is the whole gain. `M4.33` recorded the
    // over-claim; `M4.55` corrected it. It also matches `join_group`'s answer for the identical
    // `Applied::Unavailable` failure, which is the reason to prefer it over
    // `REBALANCE_IN_PROGRESS`. Found by M4's closing review; the claim
    // about the generation corrected by review of the fix.
    // ⚠️ **One question, asked once, whatever the transition returned**:
    // is this group `Stable` at *this* generation? That is the only state
    // in which the map this submission carries is the one to answer with,
    // and `M4.42` and `M4.45` are the two halves of arriving at it.
    //
    // `M4.42` split the illegal arm, which had been argued for a double
    // submission racing another connection — already `Stable` at this
    // generation, map still right — but also caught a newcomer's
    // `JoinGroup` landing between the fence's state read and the actor:
    // `MemberJoinedDuringSync` is legal from `CompletingRebalance`, so this
    // `SyncComplete` is illegal from `PreparingRebalance`, and swallowing
    // it told the leader generation N stood while the coordinator assembled
    // N+1.
    //
    // ⚠️ **`M4.45` is the same hazard through the `Ok` arm**, which that
    // split left alone: let the competing round get as far as
    // `CompletingRebalance@N+1` and the parked `SyncComplete` is *legal*
    // again, so it succeeds — at the wrong generation — and answering with
    // the map tells N's leader its assignment stands while the group is on
    // N+1, and hands N's parked followers that map. ⚠️ Not a harm to N+1's
    // followers, which an earlier version of this claimed: they read
    // `Waiting` and are released normally by their own leader's
    // submission. Neither existing guard reaches it: `M4.35`'s because the
    // cell is empty, `M4.42`'s because the transition did not fail. Asking
    // the one question of both outcomes is what ends the series, rather
    // than a third arm-specific check.
    //
    // ⚠️ **`Ok` carries the record it produced**, and the illegal arm has to
    // re-read because the error carries nothing but `Debug`-formatted
    // strings. ⚠️ **What this paragraph used to add — that the `Ok` half
    // "needs no second read and no second race" — was false, and `M4.65` is
    // the race it named as absent.** The record `Ok` carries is the state at
    // the moment the actor applied `SyncComplete`, not the state when this
    // task is next polled, and `MemberLeft` is legal from `Stable`. The
    // question is therefore asked a second time below, under the barrier's
    // own lock, where `barrier::submit`'s doc argues why that is the only
    // side of the lock it can be asked from.
    let record = match cluster
        .group_transitions()
        .transition(group.clone(), GroupEvent::SyncComplete)
        .await
    {
        Ok(record) => Some(record),
        Err(oqueue_core::Error::IllegalGroupTransition { .. }) => {
            cluster.group_coordinator().record(group)
        }
        Err(_) => {
            // ⚠️ **And the followers already parked on the barrier, which
            // `M4.33` left and `M4.43` fixed.** They are normally released
            // as `Known::Superseded` by the next generation's submission —
            // and when this arm is an unreachable log, every path to a next
            // generation goes through that same log, so while the outage
            // lasts there is no next generation and they would wait out
            // `MAX_SYNC_WAIT_MS` for an assignment nobody submits.
            //
            // ⚠️ **This arm is not only the log being unreachable, and
            // saying it was is what `M4.43` recorded against itself.**
            // `Error::Transient` reaches here with a *healthy* log — after
            // `MAX_APPEND_RETRIES` version collisions on a log shared with
            // `OffsetCommit` writers, or when the actor task has ended — and
            // in the contention case a next generation is reachable, so the
            // followers would have been released anyway. The `refuse` call
            // is right regardless: the map never landed, so rejoining is the
            // answer whichever failure this was. Only the justification was
            // too narrow. `M4.55`.
            cluster.sync_groups().refuse(group, request.generation_id);
            return reply(
                prelude,
                &refusal(crate::fencing::Refusal::CoordinatorNotAvailable.error_code()),
            );
        }
    };
    let synced = record.is_some_and(|r| {
        r.state == GroupState::Stable && r.generation.get() == request.generation_id
    });
    if !synced {
        // ⚠️ **Why this `refuse` is safe, which the arm it replaced carried
        // bare.** `M4.43` recorded that the `IllegalGroupTransition` arm
        // called `refuse` with no stated reason while its two siblings each
        // gave one, and that its reason is a *different* one. ⚠️ **It does
        // not rest on a next generation being on its way**, which is not
        // true of every state reaching here — an `AllMembersGone` group
        // re-reads as `Empty`, and an illegal transition never reached
        // `append_durably` at all. What makes the call safe in every case is
        // that `refuse` never overwrites a *later* generation's map
        // (`M4.35`'s guard, and `barrier.rs`'s own note), so publishing a
        // refusal for this generation cannot strand anyone waiting on a
        // newer one. That arm has since been folded into this one question
        // asked of every outcome, so the claim's subject is gone and its
        // reason belongs here. `M4.55`.
        cluster.sync_groups().refuse(group, request.generation_id);
        return reply(
            prelude,
            &refusal(crate::fencing::Refusal::RebalanceInProgress.error_code()),
        );
    }
    let Some(map) = cluster.sync_groups().submit(
        group,
        request.generation_id,
        map,
        // ⚠️ **The same question, asked again under the cell's lock**, which
        // `M4.65` added and `barrier::submit`'s own doc argues. The record
        // above is the one `SyncComplete` produced, and the group can leave
        // this generation between that transition landing and this task
        // being polled again -- a `MemberLeft` is legal from `Stable` and
        // publishes a refusal for exactly this generation.
        || is_stable_at(cluster, group, request.generation_id),
    ) else {
        // The group left this generation while the transition was in
        // flight. Retriable, and the same answer its own followers get
        // from `Known::Superseded`.
        //
        // ⚠️ **`refuse` here too, and this arm used to say the opposite.**
        // Until `M4.65` `submit` returned `None` for one reason only — the
        // cell already holds a *newer* generation — so every follower
        // parked on this one already read `Known::Superseded` and had been
        // woken by the submission that put it there, and this arm said so.
        // The closure above is a second reason, and on that path the cell
        // may hold nothing for this generation and nobody was woken: the
        // group left it by a route that published no refusal, which a
        // newcomer's `JoinGroup` on a `Stable` group is
        // (`join_group::round` refuses the barrier only for
        // `MemberJoinedDuringSync`). Without this call the followers park
        // out `MAX_SYNC_WAIT_MS` — the `waited_ms=3000000` stranding
        // `M4.43` and `M4.47` exist to end, through a route this row would
        // have opened. Found by review of the fix.
        //
        // ⚠️ **Unconditional, and it does not overwrite the newer map** —
        // that was the old comment's stated reason for having no call and
        // it was wrong about `refuse`, whose own guard declines a cell
        // holding a later generation. The old case is a no-op; only the new
        // one writes. It is also what the `!synced` arm above already does
        // for the identical condition.
        cluster.sync_groups().refuse(group, request.generation_id);
        return reply(
            prelude,
            &refusal(crate::fencing::Refusal::RebalanceInProgress.error_code()),
        );
    };
    reply(prelude, &response_for(&map, request, version))
}

/// Waits for `assignments` to become known, or gives up after
/// [`MAX_SYNC_WAIT_MS`] — `join_group::mod::wait_for_close`'s own
/// bounded-wait shape, `security.md` rule 3's instinct against an
/// unbounded hold. ⚠️ **Not derived from any client-supplied value**:
/// unlike `JoinGroup`'s `rebalance_timeout_ms`, `SyncGroupRequest` carries
/// no timeout field on the wire, so this is a fixed ceiling rather than the
/// group's own configured rebalance timeout — a provisional answer until a
/// later task (`M4.9`/`M4.11`) tracks that value across the two phases.
async fn wait_for_assignment(assignments: &Assignments, notify: &Notify, generation: i32) -> Known {
    let deadline = Instant::now() + Duration::from_millis(MAX_SYNC_WAIT_MS);
    loop {
        // ⚠️ **Register for the wakeup *before* looking, not after.**
        // `Notify::notified()` only registers the waiter when the future is
        // first *polled*, so checking first and constructing the future
        // second leaves a window: a `submit` landing in between calls
        // `notify_waiters`, which reaches only tasks already registered, and
        // this one is not yet — it then parks and sleeps out the whole
        // `MAX_SYNC_WAIT_MS` for an assignment that arrived while it was
        // looking. `enable()` performs the registration eagerly, so the
        // check below happens with the waiter already armed and a submission
        // in the window wakes it. `join_group::round`'s own
        // `Box::pin(..).as_mut().enable()` under the lock, for this reason.
        // Found by review, which also caught this function's own doc
        // claiming the durable `Notify` made a lost wakeup impossible.
        let mut notified = Box::pin(notify.notified());
        notified.as_mut().enable();
        match assignment_for(assignments, generation) {
            Known::Waiting => {}
            answer => return answer,
        }
        tokio::select! {
            () = tokio::time::sleep_until(deadline) => {
                return assignment_for(assignments, generation);
            }
            () = notified => {}
        }
    }
}

/// This member's own slice of `map` — empty if the leader's own submission
/// never named it. A tracked member in good standing that the leader
/// simply omitted has no wire code of its own among `M4.11`'s five
/// (real Kafka answers this the same way: empty bytes, not a refusal), so
/// this stays a plain lookup rather than routing through `fence`.
/// `protocol_type`/`protocol_name` (v5+) are echoed
/// straight from this member's own request — the wire's own answer to
/// what the response should carry, needing no state this module would
/// otherwise have to persist from `JoinGroup`.
fn response_for<'a>(
    map: &'a HashMap<String, Vec<u8>>,
    request: &SyncGroupRequest<'a>,
    version: i16,
) -> SyncGroupResponse<'a> {
    SyncGroupResponse {
        error_code: error_codes::NONE,
        protocol_type: (version >= 5).then_some(request.protocol_type).flatten(),
        protocol_name: (version >= 5).then_some(request.protocol_name).flatten(),
        assignment: map.get(request.member_id).map_or(&b""[..], Vec::as_slice),
    }
}

/// A refusal: no assignment, an error code naming why.
const fn refusal(error_code: i16) -> SyncGroupResponse<'static> {
    SyncGroupResponse {
        error_code,
        protocol_type: None,
        protocol_name: None,
        assignment: &[],
    }
}

fn reply(prelude: RequestPrelude, response: &SyncGroupResponse<'_>) -> HandlerResponse {
    let mut out = Vec::new();
    if encode_response_header(
        &mut out,
        ApiKey::SyncGroup,
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
