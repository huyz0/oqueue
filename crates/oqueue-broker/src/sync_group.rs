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
//! elected leader, and `request.generation_id` is decoded but never
//! checked against anything — a stale-generation or wrong-leader
//! submission is answered exactly like a current, correct one. Both are
//! fencing gaps this milestone's own `M4.11` ("fencing errors as one
//! audited path") is where validating belongs, not this task's to invent
//! ad hoc.
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
use oqueue_core::{GroupEvent, GroupId};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use tokio::sync::Notify;
use tokio::time::{Duration, Instant};

/// Every member's own slice of the leader's submitted assignment, by
/// member id — the map [`Entry::assignments`] resolves once the
/// assignment-bearing submission lands.
type Assignments = Arc<OnceLock<Arc<HashMap<String, Vec<u8>>>>>;

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

    /// Submits `group`'s own assignment map — the leader's own call.
    ///
    /// ⚠️ **Starts a fresh entry if one is already known.** `OnceLock::set`
    /// on an already-set cell silently no-ops, so without this a group's
    /// *second* rebalance would keep answering every follower with its
    /// *first* generation's assignment forever — not the adversarial
    /// fencing gap this module's own doc defers to `M4.11`, but the
    /// ordinary case of a group rebalancing more than once. A follower
    /// already parked on the entry being replaced keeps its own captured
    /// handles (`entry_for`'s return value) rather than looking the group
    /// back up, so it is never corrupted by this rotation — merely
    /// abandoned to its own deadline, the same safe "time out honestly"
    /// outcome an unresponsive leader would already produce.
    fn submit(
        &self,
        group: &GroupId,
        map: HashMap<String, Vec<u8>>,
    ) -> Arc<HashMap<String, Vec<u8>>> {
        let mut entries = self.lock();
        let entry = entries.entry(group.clone()).or_default();
        if entry.assignments.get().is_some() {
            *entry = Entry::default();
        }
        let map = Arc::new(map);
        let _ = entry.assignments.set(Arc::clone(&map));
        let notify = Arc::clone(&entry.notify);
        drop(entries);
        notify.notify_waiters();
        map
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

    if !request.assignments.is_empty() {
        // The assignment-bearing submission: build the map every member's
        // own slice comes from, fire the state transition, and submit it
        // (which starts a fresh entry if a prior generation's own
        // assignment was already known, `submit`'s own doc).
        let map: HashMap<String, Vec<u8>> = request
            .assignments
            .iter()
            .map(|a| (a.member_id.to_owned(), a.assignment.to_owned()))
            .collect();
        // Refused only if this bookkeeping and the coordinator have
        // diverged (already `Stable`, a double submission racing another
        // connection) -- the map this submission carries is still the
        // right one to answer with either way, so a refusal here does not
        // stop the reply.
        let _ = cluster
            .group_coordinator()
            .transition(&group, GroupEvent::SyncComplete);
        let map = cluster.sync_groups().submit(&group, map);
        return reply(prelude, &response_for(&map, &request, version));
    }

    // A follower: answer at once if the assignment is already known,
    // otherwise wait for it (or this call's own deadline).
    let (assignments, notify) = cluster.sync_groups().entry_for(&group);
    let map = if let Some(map) = assignments.get() {
        Arc::clone(map)
    } else {
        match wait_for_assignment(&assignments, &notify).await {
            Some(map) => map,
            None => return reply(prelude, &refusal(error_codes::UNKNOWN_SERVER_ERROR)),
        }
    };
    reply(prelude, &response_for(&map, &request, version))
}

/// Waits for `assignments` to become known, or gives up after
/// [`MAX_SYNC_WAIT_MS`] — `join_group::mod::wait_for_close`'s own
/// bounded-wait shape, `security.md` rule 3's instinct against an
/// unbounded hold. ⚠️ **Not derived from any client-supplied value**:
/// unlike `JoinGroup`'s `rebalance_timeout_ms`, `SyncGroupRequest` carries
/// no timeout field on the wire, so this is a fixed ceiling rather than the
/// group's own configured rebalance timeout — a provisional answer until a
/// later task (`M4.9`/`M4.11`) tracks that value across the two phases.
async fn wait_for_assignment(
    assignments: &Assignments,
    notify: &Notify,
) -> Option<Arc<HashMap<String, Vec<u8>>>> {
    let deadline = Instant::now() + Duration::from_millis(MAX_SYNC_WAIT_MS);
    loop {
        if let Some(map) = assignments.get() {
            return Some(Arc::clone(map));
        }
        tokio::select! {
            () = tokio::time::sleep_until(deadline) => return assignments.get().map(Arc::clone),
            () = notify.notified() => {}
        }
    }
}

/// This member's own slice of `map` — empty if the leader's own submission
/// never named it (a fencing gap `M4.11` owns, not this module's to
/// invent a code for). `protocol_type`/`protocol_name` (v5+) are echoed
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
