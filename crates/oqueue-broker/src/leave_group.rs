//! `LeaveGroup` (13), v0-5 — `M4.10`.
//!
//! ⚠️ **Removal, not fencing.** Every named member is removed from
//! `heartbeat.rs`'s own tracked membership and answered `NONE`
//! unconditionally — whether it was ever really a member of this group is
//! a question `M4.11`'s own audited fencing path answers, not this
//! handler's to invent (`join_group.rs`'s own precedent: a member id is
//! accepted at face value, never validated).
//!
//! ⚠️ **Batched, and batched means one rebalance.** `heartbeat.rs`'s own
//! `Heartbeats::leave` removes every named member under one lock
//! acquisition and fires at most one coordinator transition — `M4.10`'s
//! own acceptance criterion, verbatim: three members named in one request
//! trigger exactly one rebalance, not three.

#![allow(clippy::redundant_pub_crate)]

use crate::cluster::Cluster;
use crate::connection::HandlerResponse;
use oqueue_codec::apikey::ApiKey;
use oqueue_codec::error_codes;
use oqueue_codec::frame::{RequestPrelude, encode_response_header};
use oqueue_codec::leave_group::{LeaveGroupResponse, MemberLeft, decode_request, encode_response};
use oqueue_core::GroupId;

/// Decodes, removes every named member in one batch, and answers — or
/// closes the connection on a malformed body.
pub(crate) fn handle(cluster: &Cluster, prelude: RequestPrelude, body: &[u8]) -> HandlerResponse {
    let version = prelude.api_version;
    let Ok(request) = decode_request(body, version) else {
        return HandlerResponse::Close;
    };
    let Ok(group) = GroupId::new(request.group_id) else {
        return reply(prelude, version, &refusal(error_codes::INVALID_REQUEST));
    };

    let member_ids: Vec<&str> = request.members.iter().map(|m| m.member_id).collect();
    cluster
        .heartbeats()
        .leave(&group, &member_ids, cluster.group_coordinator());

    let members = member_ids
        .iter()
        .map(|&member_id| MemberLeft {
            member_id,
            error_code: error_codes::NONE,
        })
        .collect();
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
