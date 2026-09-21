//! `DescribeGroups` (15) and `ListGroups` (16) — `M12.7`.

#![allow(clippy::redundant_pub_crate)]

use crate::authz::{GroupAuthzContext, group_authorized};
use crate::cluster::Cluster;
use crate::connection::HandlerResponse;
use oqueue_codec::apikey::ApiKey;
use oqueue_codec::describe_groups::{
    DescribeGroup, DescribeGroupMember, DescribeGroupsResponse, decode_request as decode_describe,
    encode_response as encode_describe,
};
use oqueue_codec::error_codes;
use oqueue_codec::frame::{RequestPrelude, encode_response_header};
use oqueue_codec::list_groups::{
    ListGroupsResponse, ListedGroup, decode_request as decode_list, encode_response as encode_list,
};
use oqueue_core::{GroupId, GroupRecord, GroupState};

/// Answers a named-group description, omitting opaque member data by design.
pub(crate) fn describe(
    cluster: &Cluster,
    prelude: RequestPrelude,
    body: &[u8],
    authz: &GroupAuthzContext<'_>,
) -> HandlerResponse {
    let Ok(request) = decode_describe(body, prelude.api_version) else {
        return HandlerResponse::Close;
    };
    let groups = request
        .groups
        .into_iter()
        .map(|name| describe_one(cluster, name, authz, request.include_authorized_operations))
        .collect();
    let response = DescribeGroupsResponse {
        throttle_time_ms: 0,
        groups,
    };
    reply_describe(prelude, &response)
}

fn describe_one(
    cluster: &Cluster,
    name: String,
    authz: &GroupAuthzContext<'_>,
    include_authorized_operations: bool,
) -> DescribeGroup {
    let empty = |error_code| DescribeGroup {
        error_code,
        group_id: name.clone(),
        group_state: String::new(),
        protocol_type: String::new(),
        protocol_data: String::new(),
        members: Vec::new(),
        authorized_operations: i32::MIN,
    };
    let Ok(group) = GroupId::new(&name) else {
        return empty(error_codes::INVALID_REQUEST);
    };
    if !group_authorized(&group, authz) {
        return empty(error_codes::GROUP_AUTHORIZATION_FAILED);
    }
    let Some(record) = cluster.group_coordinator().record(&group) else {
        return empty(error_codes::GROUP_ID_NOT_FOUND);
    };
    describe_record(cluster, name, &group, record, include_authorized_operations)
}

fn describe_record(
    cluster: &Cluster,
    name: String,
    group: &GroupId,
    record: GroupRecord,
    _include_authorized_operations: bool,
) -> DescribeGroup {
    let roster = cluster.group_coordinator().roster(group);
    let (protocol_type, protocol_data, member_ids) = if record.state == GroupState::Stable {
        roster.map_or_else(
            || (String::new(), String::new(), Vec::new()),
            |roster| {
                (
                    roster.protocol_type,
                    roster.protocol_name,
                    roster.member_ids,
                )
            },
        )
    } else {
        (String::new(), String::new(), Vec::new())
    };
    DescribeGroup {
        error_code: error_codes::NONE,
        group_id: name,
        group_state: group_state_name(record.state).to_owned(),
        protocol_type,
        protocol_data,
        members: member_ids
            .into_iter()
            .map(|member_id| DescribeGroupMember {
                member_id,
                group_instance_id: None,
                client_id: String::new(),
                client_host: String::new(),
                member_metadata: Vec::new(),
                member_assignment: Vec::new(),
            })
            .collect(),
        // Group operation authorization is not implemented yet. Kafka's
        // sentinel distinguishes "not computed" from "computed: none".
        authorized_operations: i32::MIN,
    }
}

#[cfg(test)]
mod tests;

/// Answers a principal-scoped group listing. Unauthorized groups are omitted.
pub(crate) fn list(
    cluster: &Cluster,
    prelude: RequestPrelude,
    body: &[u8],
    authz: &GroupAuthzContext<'_>,
) -> HandlerResponse {
    let Ok(request) = decode_list(body, prelude.api_version) else {
        return HandlerResponse::Close;
    };
    let mut groups: Vec<_> = cluster
        .group_coordinator()
        .records()
        .into_iter()
        .filter(|(group, record)| {
            group_authorized(group, authz)
                && request.states_filter.as_ref().is_none_or(|states| {
                    states.is_empty()
                        || states
                            .iter()
                            .any(|state| state == group_state_name(record.state))
                })
        })
        .map(|(group, record)| ListedGroup {
            group_id: group.as_str().to_owned(),
            protocol_type: if record.state == GroupState::Stable {
                cluster
                    .group_coordinator()
                    .roster(&group)
                    .map_or_else(String::new, |roster| roster.protocol_type)
            } else {
                String::new()
            },
            group_state: group_state_name(record.state).to_owned(),
        })
        .collect();
    groups.sort_by(|left, right| left.group_id.cmp(&right.group_id));
    let response = ListGroupsResponse {
        throttle_time_ms: 0,
        error_code: error_codes::NONE,
        groups,
    };
    reply_list(prelude, &response)
}

const fn group_state_name(state: GroupState) -> &'static str {
    match state {
        GroupState::Empty => "Empty",
        GroupState::PreparingRebalance => "PreparingRebalance",
        GroupState::CompletingRebalance => "CompletingRebalance",
        GroupState::Stable => "Stable",
        GroupState::Dead => "Dead",
    }
}

fn reply_describe(prelude: RequestPrelude, response: &DescribeGroupsResponse) -> HandlerResponse {
    let mut out = Vec::new();
    if encode_response_header(
        &mut out,
        ApiKey::DescribeGroups,
        prelude.api_version,
        prelude.correlation_id,
    )
    .is_err()
    {
        return HandlerResponse::Close;
    }
    encode_describe(&mut out, prelude.api_version, response);
    HandlerResponse::Reply(out)
}

fn reply_list(prelude: RequestPrelude, response: &ListGroupsResponse) -> HandlerResponse {
    let mut out = Vec::new();
    if encode_response_header(
        &mut out,
        ApiKey::ListGroups,
        prelude.api_version,
        prelude.correlation_id,
    )
    .is_err()
    {
        return HandlerResponse::Close;
    }
    encode_list(&mut out, prelude.api_version, response);
    HandlerResponse::Reply(out)
}
