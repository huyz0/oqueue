#![allow(clippy::expect_used)]

use super::{describe, list};
use crate::authz::{GroupAuthzContext, unconfigured_group_authz};
use crate::connection::HandlerResponse;
use crate::testing::fixture;
use kafka_protocol::messages::{
    DescribeGroupsResponse as KpDescribe, ListGroupsResponse as KpList,
};
use kafka_protocol::protocol::Decodable;
use oqueue_codec::describe_groups::decode_request as decode_describe;
use oqueue_codec::flex::{TaggedFields, put_array_len, put_string, put_tagged_fields};
use oqueue_codec::frame::RequestPrelude;
use oqueue_codec::list_groups::decode_request as decode_list;
use oqueue_core::{
    GroupCoordinator, GroupEvent, GroupGrants, GroupId, GroupRosterSnapshot, Principal,
};

fn prelude(api_key: i16, version: i16) -> RequestPrelude {
    RequestPrelude {
        api_key,
        api_version: version,
        correlation_id: 19,
    }
}

fn describe_body(group: &str, version: i16) -> Vec<u8> {
    let flexible = version >= 5;
    let mut body = Vec::new();
    put_array_len(&mut body, flexible, Some(1));
    put_string(&mut body, flexible, group);
    if version >= 3 {
        body.push(0);
    }
    if flexible {
        put_tagged_fields(&mut body, &TaggedFields::default());
    }
    body
}

fn list_body(version: i16) -> Vec<u8> {
    let mut body = Vec::new();
    if version >= 4 {
        put_array_len(&mut body, true, Some(0));
    }
    if version >= 3 {
        put_tagged_fields(&mut body, &TaggedFields::default());
    }
    body
}

fn list_body_with_states(states: &[&str]) -> Vec<u8> {
    let mut body = Vec::new();
    put_array_len(&mut body, true, Some(states.len()));
    for state in states {
        put_string(&mut body, true, state);
    }
    put_tagged_fields(&mut body, &TaggedFields::default());
    body
}

fn response_body(bytes: &[u8], header_len: usize) -> &[u8] {
    &bytes[header_len..]
}

#[tokio::test]
async fn unknown_describe_group_uses_group_id_not_found() {
    let fixture = fixture(&[]).await;
    let response = describe(
        &fixture.cluster,
        prelude(15, 2),
        &describe_body("missing", 2),
        &unconfigured_group_authz(),
    );
    let HandlerResponse::Reply(bytes) = response else {
        panic!("DescribeGroups replies");
    };
    let mut body = response_body(&bytes, 4);
    let decoded = KpDescribe::decode(&mut body, 2).expect("response decodes");
    assert!(body.is_empty());
    assert_eq!(decoded.groups[0].error_code, 69);
    assert!(decoded.groups[0].members.is_empty());
}

#[tokio::test]
async fn empty_group_is_described_without_stale_members() {
    let fixture = fixture(&[]).await;
    let group = GroupId::new("orders").expect("valid");
    fixture
        .group_coordinator
        .transition(&group, GroupEvent::Join)
        .expect("legal");
    fixture
        .group_coordinator
        .transition(&group, GroupEvent::AllMembersGone)
        .expect("legal");

    let response = describe(
        &fixture.cluster,
        prelude(15, 2),
        &describe_body("orders", 2),
        &unconfigured_group_authz(),
    );
    let HandlerResponse::Reply(bytes) = response else {
        panic!("DescribeGroups replies");
    };
    let mut body = response_body(&bytes, 4);
    let decoded = KpDescribe::decode(&mut body, 2).expect("response decodes");
    assert_eq!(decoded.groups[0].error_code, 0);
    let state: &str = decoded.groups[0].group_state.as_ref();
    assert_eq!(state, "Empty");
    assert!(decoded.groups[0].members.is_empty());
}

#[tokio::test]
async fn stable_group_describes_persisted_non_sensitive_roster() {
    let fixture = fixture(&[]).await;
    let group = GroupId::new("orders").expect("valid");
    fixture
        .group_coordinator
        .transition(&group, GroupEvent::Join)
        .expect("legal");
    fixture
        .group_coordinator
        .transition(&group, GroupEvent::JoinBarrierComplete)
        .expect("legal");
    fixture
        .group_coordinator
        .transition(&group, GroupEvent::SyncComplete)
        .expect("legal");
    fixture.group_coordinator.set_roster(
        &group,
        Some(GroupRosterSnapshot {
            protocol_type: "consumer".to_owned(),
            protocol_name: "range".to_owned(),
            member_ids: vec!["member-1".to_owned()],
        }),
    );

    let response = describe(
        &fixture.cluster,
        prelude(15, 4),
        &describe_body("orders", 4),
        &unconfigured_group_authz(),
    );
    let HandlerResponse::Reply(bytes) = response else {
        panic!("DescribeGroups replies");
    };
    let mut body = response_body(&bytes, 4);
    let decoded = KpDescribe::decode(&mut body, 4).expect("response decodes");
    let protocol_type: &str = decoded.groups[0].protocol_type.as_ref();
    let protocol_data: &str = decoded.groups[0].protocol_data.as_ref();
    assert_eq!(protocol_type, "consumer");
    assert_eq!(protocol_data, "range");
    assert_eq!(decoded.groups[0].members.len(), 1);
    let member_id: &str = decoded.groups[0].members[0].member_id.as_ref();
    assert_eq!(member_id, "member-1");
    assert!(decoded.groups[0].members[0].member_metadata.is_empty());
    assert!(decoded.groups[0].members[0].member_assignment.is_empty());
}

#[tokio::test]
async fn rebalance_and_dead_groups_do_not_expose_stale_rosters() {
    let fixture = fixture(&[]).await;
    let group = GroupId::new("orders").expect("valid");
    let coordinator = &fixture.group_coordinator;
    coordinator
        .transition(&group, GroupEvent::Join)
        .expect("legal");
    coordinator
        .transition(&group, GroupEvent::JoinBarrierComplete)
        .expect("legal");
    let response = describe(
        &fixture.cluster,
        prelude(15, 3),
        &describe_body("orders", 3),
        &unconfigured_group_authz(),
    );
    let HandlerResponse::Reply(bytes) = response else {
        panic!("DescribeGroups replies");
    };
    let mut body = response_body(&bytes, 4);
    let decoded = KpDescribe::decode(&mut body, 3).expect("response decodes");
    let group_state: &str = decoded.groups[0].group_state.as_ref();
    assert_eq!(group_state, "CompletingRebalance");
    assert!(decoded.groups[0].members.is_empty());
    assert_eq!(decoded.groups[0].authorized_operations, i32::MIN);

    coordinator
        .transition(&group, GroupEvent::SyncComplete)
        .expect("legal");
    coordinator
        .transition(&group, GroupEvent::AllMembersGone)
        .expect("legal");
    coordinator
        .transition(&group, GroupEvent::Expire)
        .expect("legal");
    let response = describe(
        &fixture.cluster,
        prelude(15, 2),
        &describe_body("orders", 2),
        &unconfigured_group_authz(),
    );
    let HandlerResponse::Reply(bytes) = response else {
        panic!("DescribeGroups replies");
    };
    let mut body = response_body(&bytes, 4);
    let decoded = KpDescribe::decode(&mut body, 2).expect("response decodes");
    let group_state: &str = decoded.groups[0].group_state.as_ref();
    assert_eq!(group_state, "Dead");
    assert!(decoded.groups[0].members.is_empty());
    let protocol_type: &str = decoded.groups[0].protocol_type.as_ref();
    assert!(protocol_type.is_empty());
}

#[tokio::test]
async fn list_groups_is_deterministic_and_returns_only_authorized_records() {
    let fixture = fixture(&[]).await;
    for name in ["zulu", "alpha"] {
        let group = GroupId::new(name).expect("valid");
        fixture
            .group_coordinator
            .transition(&group, GroupEvent::Join)
            .expect("legal");
    }
    let response = list(
        &fixture.cluster,
        prelude(16, 4),
        &list_body(4),
        &unconfigured_group_authz(),
    );
    let HandlerResponse::Reply(bytes) = response else {
        panic!("ListGroups replies");
    };
    let mut body = response_body(&bytes, 5);
    let decoded = KpList::decode(&mut body, 4).expect("response decodes");
    assert_eq!(decoded.error_code, 0);
    assert_eq!(decoded.groups.len(), 2);
    let ids: Vec<String> = decoded
        .groups
        .iter()
        .map(|group| group.group_id.0.as_str().to_owned())
        .collect();
    assert_eq!(ids, ["alpha", "zulu"]);
}

#[tokio::test]
async fn list_groups_applies_state_filter_and_only_stable_groups_expose_protocol_type() {
    let fixture = fixture(&[]).await;
    let stable = GroupId::new("stable").expect("valid");
    for event in [
        GroupEvent::Join,
        GroupEvent::JoinBarrierComplete,
        GroupEvent::SyncComplete,
    ] {
        fixture
            .group_coordinator
            .transition(&stable, event)
            .expect("legal");
    }
    fixture.group_coordinator.set_roster(
        &stable,
        Some(GroupRosterSnapshot {
            protocol_type: "consumer".to_owned(),
            protocol_name: "range".to_owned(),
            member_ids: vec!["member-1".to_owned()],
        }),
    );
    let preparing = GroupId::new("preparing").expect("valid");
    fixture
        .group_coordinator
        .transition(&preparing, GroupEvent::Join)
        .expect("legal");

    let response = list(
        &fixture.cluster,
        prelude(16, 4),
        &list_body_with_states(&["Stable"]),
        &unconfigured_group_authz(),
    );
    let HandlerResponse::Reply(bytes) = response else {
        panic!("ListGroups replies");
    };
    let mut body = response_body(&bytes, 5);
    let decoded = KpList::decode(&mut body, 4).expect("response decodes");
    assert_eq!(decoded.groups.len(), 1);
    assert_eq!(decoded.groups[0].group_id.0.as_str(), "stable");
    let protocol_type: &str = decoded.groups[0].protocol_type.as_ref();
    assert_eq!(protocol_type, "consumer");
}

#[tokio::test]
async fn configured_group_grants_hide_unrelated_descriptions_and_list_entries() {
    let fixture = fixture(&[]).await;
    for name in ["owned", "unrelated"] {
        let group = GroupId::new(name).expect("valid");
        fixture
            .group_coordinator
            .transition(&group, GroupEvent::Join)
            .expect("legal");
    }
    let principal = Principal::new("alice").expect("valid");
    let mut grants = GroupGrants::new();
    grants.grant(principal.clone(), GroupId::new("owned").expect("valid"));
    let authz = GroupAuthzContext {
        principal: Some(&principal),
        credentials_configured: true,
        group_grants: &grants,
    };

    let described = describe(
        &fixture.cluster,
        prelude(15, 2),
        &describe_body("unrelated", 2),
        &authz,
    );
    let HandlerResponse::Reply(bytes) = described else {
        panic!("DescribeGroups replies");
    };
    let mut body = response_body(&bytes, 4);
    let decoded = KpDescribe::decode(&mut body, 2).expect("response decodes");
    assert_eq!(decoded.groups[0].error_code, 30);
    assert_eq!(decoded.groups[0].group_id.0.as_str(), "unrelated");

    let listed = list(&fixture.cluster, prelude(16, 2), &list_body(2), &authz);
    let HandlerResponse::Reply(bytes) = listed else {
        panic!("ListGroups replies");
    };
    let mut body = response_body(&bytes, 4);
    let decoded = KpList::decode(&mut body, 2).expect("response decodes");
    assert_eq!(decoded.groups.len(), 1);
    assert_eq!(decoded.groups[0].group_id.0.as_str(), "owned");
}

#[test]
fn request_codecs_reject_trailing_bytes() {
    let mut describe = describe_body("orders", 5);
    describe.push(1);
    assert!(decode_describe(&describe, 5).is_err());

    let mut list = list_body(4);
    list.push(1);
    assert!(decode_list(&list, 4).is_err());
}
