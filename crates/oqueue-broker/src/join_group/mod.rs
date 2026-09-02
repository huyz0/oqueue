//! `JoinGroup` (11), v0-9 — `M4.7`.
//!
//! ⚠️ **Member ids are minted, never taken from the wire.** `member_id.rs`'s
//! own doc names this handler as where minting (an empty `member_id`) or
//! validating an echoed one (a rejoin) happens; this task builds only the
//! first half. A non-empty `member_id` is accepted at face value — never
//! looked up, never refused as unrecognized — because `UNKNOWN_MEMBER_ID`
//! is `M4.9`'s own path to invent (`M4.7`'s backlog row, verbatim).
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
    JoinGroupResponse, JoinGroupResponseMember, decode_request, encode_response,
};
use oqueue_core::{GroupId, MemberId};
pub(crate) use round::GroupJoins;
use round::{JoinOutcome, RoundClose, RoundMember, effective_timeout_ms, mint_member_id};

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
    let member = RoundMember {
        member_id: member_id.as_str().to_owned(),
        protocol_type: request.protocol_type.to_owned(),
        protocols: request
            .protocols
            .iter()
            .map(|p| (p.name.to_owned(), p.metadata.to_owned()))
            .collect(),
    };
    let rebalance_timeout = tokio::time::Duration::from_millis(barrier_ms(effective_timeout_ms(
        request.rebalance_timeout_ms,
        request.session_timeout_ms,
    )));

    let close = match cluster.group_joins().join(
        cluster.group_coordinator(),
        &group,
        member,
        rebalance_timeout,
    ) {
        JoinOutcome::Refused => {
            return reply(prelude, &refusal(error_codes::INCONSISTENT_GROUP_PROTOCOL));
        }
        JoinOutcome::Ready(close) => close,
        pending @ JoinOutcome::Pending { .. } => {
            let Some(close) = wait_for_close(cluster, &group, pending).await else {
                // See `wait_for_close`'s own doc: unreachable outside an
                // internal invariant violation, answered rather than held
                // open forever waiting on a close that evidently is not
                // coming.
                return reply(prelude, &refusal(error_codes::UNKNOWN_SERVER_ERROR));
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
/// Returns `None` only if the deadline has already been tried once (this
/// call's own [`GroupJoins::close_on_deadline`] call) and `outcome` is
/// *still* unset — `round.rs`'s own module doc names the only way that
/// happens: this bookkeeping and the coordinator have diverged, a bug
/// rather than a case a client can trigger. Bailing out here rather than
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
        if let Some(close) = outcome.get() {
            return Some(std::sync::Arc::clone(close));
        }
        tokio::select! {
            () = tokio::time::sleep_until(deadline) => {
                cluster.group_joins().close_on_deadline(
                    cluster.group_coordinator(),
                    group,
                    &outcome,
                );
                if deadline_tried {
                    return None;
                }
                deadline_tried = true;
            }
            () = notify.notified() => {}
        }
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
