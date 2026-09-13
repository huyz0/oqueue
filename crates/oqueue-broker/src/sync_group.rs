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
        // Retriable: the client rejoins and syncs at the new generation.
        Known::Superseded | Known::Refused => {
            return reply(
                prelude,
                &refusal(crate::fencing::Refusal::RebalanceInProgress.error_code()),
            );
        }
        Known::Waiting => return reply(prelude, &refusal(error_codes::UNKNOWN_SERVER_ERROR)),
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
    // worst available answer.** Every member is told its assignment is
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
    // either way. What the code buys is the difference between a client
    // that comes back and one that does not: it is retriable, where the
    // alternative this branch had was telling every member `NONE` and
    // letting them consume against a group the coordinator never saw reach
    // `Stable`. It also matches `join_group`'s answer for the identical
    // `Applied::Unavailable` failure, which is the reason to prefer it over
    // `REBALANCE_IN_PROGRESS`. Found by M4's closing review; the claim
    // about the generation corrected by review of the fix.
    match cluster
        .group_transitions()
        .transition(group.clone(), GroupEvent::SyncComplete)
        .await
    {
        Ok(_) => {}
        // ⚠️ **Illegal is two cases, not one, and `M4.42` is the second.**
        // The arm was argued for a double submission racing another
        // connection: the group is already `Stable` at this generation, the
        // map this submission carries is the one that produced that, and
        // answering with it is right. It also caught a newcomer's
        // `JoinGroup` landing between the fence's state read and the actor
        // — `MemberJoinedDuringSync` is legal from `CompletingRebalance`,
        // so this `SyncComplete` is illegal from `PreparingRebalance` — and
        // swallowing *that* tells the leader and every follower it feeds
        // that generation N stands while the coordinator assembles N+1, in
        // which a newcomer holds some of the same partitions. That is
        // FR-20's revoke-before-reassign, and real Kafka answers
        // `REBALANCE_IN_PROGRESS`.
        //
        // ⚠️ **The state the group is actually in is what separates them**,
        // re-read after the actor rather than inferred from the error's own
        // `Debug`-formatted strings. `M4.35`'s guard does not reach this:
        // the cell holds nothing newer than N, so the submission would be
        // accepted. Found by review of `M4.33`.
        Err(oqueue_core::Error::IllegalGroupTransition { .. }) => {
            let synced = cluster.group_coordinator().record(group).is_some_and(|r| {
                r.state == GroupState::Stable && r.generation.get() == request.generation_id
            });
            if !synced {
                cluster.sync_groups().refuse(group, request.generation_id);
                return reply(
                    prelude,
                    &refusal(crate::fencing::Refusal::RebalanceInProgress.error_code()),
                );
            }
        }
        Err(_) => {
            // ⚠️ **And the followers already parked on the barrier, which
            // `M4.33` left and `M4.43` fixed.** They are normally released
            // as `Known::Superseded` by the next generation's submission —
            // but the refusal here *is* the log being unreachable, and every
            // path to a next generation goes through the same log, so while
            // the outage lasts there is no next generation and they would
            // wait out `MAX_SYNC_WAIT_MS` for an assignment nobody submits.
            cluster.sync_groups().refuse(group, request.generation_id);
            return reply(
                prelude,
                &refusal(crate::fencing::Refusal::CoordinatorNotAvailable.error_code()),
            );
        }
    }
    let Some(map) = cluster
        .sync_groups()
        .submit(group, request.generation_id, map)
    else {
        // The group left this generation while the transition was in
        // flight. Retriable, and the same answer its own followers get
        // from `Known::Superseded`.
        //
        // ⚠️ **No `refuse` call here, unlike the two arms above.** `submit`
        // returned `None` precisely because the cell already holds a
        // *newer* generation, so every follower parked on this one already
        // reads `Known::Superseded` and was woken by the submission that
        // put it there. Publishing a refusal would overwrite that newer
        // map, which is the thing `M4.35` exists to prevent.
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
