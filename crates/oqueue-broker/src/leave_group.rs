//! `LeaveGroup` (13), v0-5 — `M4.10`, `M4.11`.
//!
//! ⚠️ **Removed, not the same as fenced.** Every named member is removed
//! from `heartbeat.rs`'s own tracked membership regardless of whether it
//! was ever really there — `join_group.rs`'s own precedent: a member id is
//! accepted at face value, never validated, and removing an unknown one is
//! harmless (`Heartbeats::leave`'s own no-op for a name it does not hold).
//! ⚠️ **But the *response* now answers per member** — `M4.11`'s own audited
//! seam: a member this broker never tracked is told `UNKNOWN_MEMBER_ID`,
//! not `NONE`, so a client can tell "you already left" from "you were
//! never here" — checked *before* removal runs, since after it every
//! member in the batch would look equally untracked.
//!
//! ⚠️ **Batched, and batched means one rebalance.** `heartbeat.rs`'s own
//! `Heartbeats::leave` removes every named member under one lock
//! acquisition and fires at most one coordinator transition — `M4.10`'s
//! own acceptance criterion, verbatim: three members named in one request
//! trigger exactly one rebalance, not three. No generation or state check:
//! `LeaveGroupRequest` carries no generation field, and a member may leave
//! from any live state.

#![allow(clippy::redundant_pub_crate)]

use crate::cluster::Cluster;
use crate::connection::HandlerResponse;
use crate::fencing::{FencingContext, Refusal, fence};
use oqueue_codec::apikey::ApiKey;
use oqueue_codec::error_codes;
use oqueue_codec::frame::{RequestPrelude, encode_response_header};
use oqueue_codec::leave_group::{LeaveGroupResponse, MemberLeft, decode_request, encode_response};
use oqueue_core::GroupId;

/// Decodes, answers each named member per `M4.11`'s own fencing seam,
/// removes every one of them in one batch, and replies — or closes the
/// connection on a malformed body.
pub(crate) fn handle(cluster: &Cluster, prelude: RequestPrelude, body: &[u8]) -> HandlerResponse {
    let version = prelude.api_version;
    let Ok(request) = decode_request(body, version) else {
        return HandlerResponse::Close;
    };
    let Ok(group) = GroupId::new(request.group_id) else {
        return reply(prelude, version, &refusal(error_codes::INVALID_REQUEST));
    };

    let member_ids: Vec<&str> = request.members.iter().map(|m| m.member_id).collect();
    // Checked before removal: `Heartbeats::is_tracked` would answer `false`
    // for every member here once `leave` below has run.
    let record = cluster.group_coordinator().record(&group);
    let members: Vec<MemberLeft<'_>> = member_ids
        .iter()
        .map(|&member_id| {
            let member_tracked = cluster.heartbeats().is_tracked(&group, member_id);
            let ctx = FencingContext::for_this_node(
                cluster.replay_in_progress(),
                member_tracked,
                record.as_ref(),
                None,
                None,
            );
            let error_code = fence(&ctx).map_or_else(Refusal::error_code, |()| error_codes::NONE);
            MemberLeft {
                member_id,
                error_code,
            }
        })
        .collect();

    cluster
        .heartbeats()
        .leave(&group, &member_ids, cluster.group_coordinator());

    reply(
        prelude,
        version,
        &LeaveGroupResponse {
            error_code: error_codes::NONE,
            members,
        },
    )
}

/// A top-level refusal: no per-member answers, since none was ever reached.
const fn refusal(error_code: i16) -> LeaveGroupResponse<'static> {
    LeaveGroupResponse {
        error_code,
        members: Vec::new(),
    }
}

fn reply(
    prelude: RequestPrelude,
    version: i16,
    response: &LeaveGroupResponse<'_>,
) -> HandlerResponse {
    let mut out = Vec::new();
    if encode_response_header(
        &mut out,
        ApiKey::LeaveGroup,
        version,
        prelude.correlation_id,
    )
    .is_err()
    {
        return HandlerResponse::Close;
    }
    encode_response(&mut out, version, response);
    HandlerResponse::Reply(out)
}

#[cfg(test)]
mod tests;
