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

mod deadline;

use crate::cluster::Cluster;
use crate::connection::HandlerResponse;
use deadline::MAX_SYNC_WAIT_MS;
use oqueue_codec::apikey::ApiKey;
use oqueue_codec::error_codes;
use oqueue_codec::frame::{RequestPrelude, encode_response_header};
use oqueue_codec::sync_group::{
    SyncGroupRequest, SyncGroupResponse, decode_request, encode_response,
};
use oqueue_core::{GroupEvent, GroupId, GroupState};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::sync::Notify;
use tokio::time::{Duration, Instant};

/// Every member's own slice of the leader's submitted assignment, by member
/// id, together with **the generation it was computed for**.
///
/// ⚠️ **The generation is the whole point, and leaving it out was a real
/// defect `M4.17` caught with a real client.** A follower syncing for
/// generation N would find generation N-1's map already present and be
/// answered from it at once, without waiting for its own generation's
/// leader to submit. For a consumer that joined during N-1 that map holds
/// its own *empty* slice — it was the newcomer, and a cooperative assignor
/// gives a newcomer nothing in the round that admits it — so the consumer
/// was answered "you own nothing", went `steady`, and stayed idle forever
/// while the rest of the group kept every partition. Adding a consumer to a
/// working group simply did not work.
#[derive(Debug)]
struct Assignment {
    generation: i32,
    map: Arc<HashMap<String, Vec<u8>>>,
}

/// The cell a group's current [`Assignment`] lives in.
///
/// ⚠️ **A `Mutex<Option<_>>`, not a `OnceLock`.** Each generation replaces
/// the last, and the previous shape could only ever be *rotated* — replacing
/// the whole entry, and with it the `Notify` any follower was already parked
/// on, so a follower that arrived before its leader was left waiting on a
/// handle nobody would ever signal and timed out. One durable cell and one
/// durable `Notify` per group means a waiter is woken by every submission
/// and re-checks the generation itself.
type Assignments = Arc<Mutex<Option<Assignment>>>;

/// Every group's own assignment barrier: known once the assignment-bearing
/// submission lands, `None` until then.
#[derive(Debug, Default)]
pub(crate) struct SyncGroups {
    entries: Mutex<HashMap<GroupId, Entry>>,
}

#[derive(Debug, Default)]
struct Entry {
    assignments: Assignments,
    notify: Arc<Notify>,
}

/// What the group's cell currently says about `generation`.
#[derive(Debug)]
enum Known {
    /// This generation's assignment, ready to answer with.
    Mine(Arc<HashMap<String, Vec<u8>>>),
    /// The cell holds a *later* generation: the group rebalanced again while
    /// this member was waiting, and its own generation is never coming.
    Superseded,
    /// Nothing yet, or an older generation still. Keep waiting.
    Waiting,
}

/// What the group's cell currently says about `generation`.
fn assignment_for(assignments: &Assignments, generation: i32) -> Known {
    let guard = assignments.lock().unwrap_or_else(PoisonError::into_inner);
    // ⚠️ **An `Ordering` match, not comparison guards.** The three cases are
    // exhaustive and mutually exclusive, and spelling them that way says so
    // to the compiler instead of relying on arm order — with guards, an
    // earlier `==` arm makes the `>` in a later one unmutatable-but-live
    // (`cargo mutants` reports `>` -> `>=` surviving, because equality never
    // reaches it), which is a branch no test can pin. Found by the gate.
    let known = guard.as_ref().map_or(Known::Waiting, |a| {
        match a.generation.cmp(&generation) {
            std::cmp::Ordering::Equal => Known::Mine(Arc::clone(&a.map)),
            // ⚠️ **A later generation means this waiter's own is never
            // coming**, and saying so at once is the difference between a
            // retry and a dead consumer. Without this the member sleeps out
            // the whole `MAX_SYNC_WAIT_MS` — 50 minutes — and is then
            // answered `UNKNOWN_SERVER_ERROR`, which the Java consumer
            // raises out of `poll()` and never retries; real Kafka releases
            // exactly this case immediately with `REBALANCE_IN_PROGRESS`,
            // which is what a client rejoins on. The same lesson `M4.15d`
            // learned four times over: a condition that is nobody's fault
            // must not reach a fatal code.
            // Found by review.
            std::cmp::Ordering::Greater => Known::Superseded,
            // An older generation still on the cell: this member's own
            // leader has not submitted yet. Keep waiting.
            std::cmp::Ordering::Less => Known::Waiting,
        }
    });
    drop(guard);
    known
}

impl SyncGroups {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<GroupId, Entry>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// `group`'s own current entry, creating one if this is its first
    /// `SyncGroup` for the generation now completing.
    fn entry_for(&self, group: &GroupId) -> (Assignments, Arc<Notify>) {
        let mut entries = self.lock();
        let entry = entries.entry(group.clone()).or_default();
        let handles = (Arc::clone(&entry.assignments), Arc::clone(&entry.notify));
        drop(entries);
        handles
    }

    /// Submits `group`'s own assignment map for `generation` — the leader's
    /// own call.
    ///
    /// ⚠️ **Replaces the previous generation's, and wakes every waiter on
    /// the group's own durable `Notify`.** Followers for this generation
    /// that arrived first are parked on it; followers still holding a
    /// *stale* generation see the new one on waking and re-check rather
    /// than being answered from a map that was never about them.
    ///
    /// `None` means the cell already holds a **later** generation and this
    /// submission is refused — the caller answers `REBALANCE_IN_PROGRESS`.
    ///
    /// ⚠️ **The guard is `>`, not `>=` or `!=`: a leader resubmitting its
    /// own generation still replaces its own map**, which is the retry path
    /// `a_resubmitted_assignment_for_the_same_generation_replaces_the_first`
    /// and `durability::a_second_submission_against_an_already_stable_group_is_still_answered`
    /// both pin — a leader retrying after a lost response would otherwise be
    /// refused and burn a generation. `>=` is the mutation that matters and
    /// turns all three red.
    ///
    /// ⚠️ **`M4.35`: this was an unconditional write, and it was the one
    /// blind post-`await` write M4 left.** `handle`'s fence reads the
    /// group's record synchronously and the submission then awaits a full
    /// round trip through the single transitions actor, so a generation
    /// N submission can arrive after N+1's leader has already submitted —
    /// and the stale map then replaced it, leaving every N+1 follower not
    /// yet answered looking at a cell that had gone backwards, waiting out
    /// `MAX_SYNC_WAIT_MS` for an `UNKNOWN_SERVER_ERROR`. Every other
    /// post-await write in M4 has this guard: `CommittedOffsets::apply`
    /// compares a `CommitVersion`, `is_own_open_round` pointer-compares the
    /// round, `close_on_deadline` re-reads `outcome`. Found by M4's closing
    /// review.
    fn submit(
        &self,
        group: &GroupId,
        generation: i32,
        map: HashMap<String, Vec<u8>>,
    ) -> Option<Arc<HashMap<String, Vec<u8>>>> {
        let mut entries = self.lock();
        let entry = entries.entry(group.clone()).or_default();
        let map = Arc::new(map);
        let mut guard = entry
            .assignments
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        if guard.as_ref().is_some_and(|a| a.generation > generation) {
            drop(guard);
            drop(entries);
            return None;
        }
        *guard = Some(Assignment {
            generation,
            map: Arc::clone(&map),
        });
        drop(guard);
        let notify = Arc::clone(&entry.notify);
        drop(entries);
        notify.notify_waiters();
        Some(map)
    }
}

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
        Known::Superseded => {
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
    // worst available answer**, and this refusal leaves one thing
    // unanswered: a follower already parked on the barrier is not woken,
    // because `submit` never runs. It is released as `Known::Superseded`
    // by the next generation's submission -- but while the log stays down
    // there is no next generation, so it waits out `MAX_SYNC_WAIT_MS`.
    // Filed rather than fixed here: giving the barrier a failure channel
    // is a change to `SyncGroups`, not to this branch. Every member is told its assignment is
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
                return reply(
                    prelude,
                    &refusal(crate::fencing::Refusal::RebalanceInProgress.error_code()),
                );
            }
        }
        Err(_) => {
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
