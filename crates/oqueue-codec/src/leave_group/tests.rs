#![allow(clippy::expect_used)]

use super::{LeaveGroupResponse, MemberLeft, decode_request, encode_response};

/// Every version `LeaveGroup` itself defines.
const VERSIONS: [i16; 6] = [0, 1, 2, 3, 4, 5];

#[test]
fn our_request_decode_matches_the_dependency_for_a_single_member() {
    use kafka_protocol::messages::LeaveGroupRequest as KpRequest;
    use kafka_protocol::messages::leave_group_request::MemberIdentity as KpMember;
    use kafka_protocol::protocol::{Encodable, StrBytes};

    for version in VERSIONS {
        let kp = if version <= 2 {
            KpRequest::default()
                .with_group_id(kafka_protocol::messages::GroupId(
                    StrBytes::from_static_str("orders-consumers"),
                ))
                .with_member_id(StrBytes::from_static_str("m1"))
        } else {
            let mut member = KpMember::default();
            member.member_id = StrBytes::from_static_str("m1");
            if version >= 5 {
                member.reason = Some(StrBytes::from_static_str("bye"));
            }
            KpRequest::default()
                .with_group_id(kafka_protocol::messages::GroupId(
                    StrBytes::from_static_str("orders-consumers"),
                ))
                .with_members(vec![member])
        };
        let mut bytes = Vec::new();
        kp.encode(&mut bytes, version).expect("dependency encodes");

        let ours = decode_request(&bytes, version).expect("ours decodes");
        assert_eq!(ours.group_id, "orders-consumers", "v{version}");
        assert_eq!(ours.members.len(), 1, "v{version}");
        assert_eq!(ours.members[0].member_id, "m1", "v{version}");
    }
}

/// The batched form (v3+): N members in one request, in order.
#[test]
fn our_request_decode_matches_the_dependency_for_a_batch() {
    use kafka_protocol::messages::LeaveGroupRequest as KpRequest;
    use kafka_protocol::messages::leave_group_request::MemberIdentity as KpMember;
    use kafka_protocol::protocol::{Encodable, StrBytes};

    for version in VERSIONS.into_iter().filter(|&v| v >= 3) {
        let members = ["m1", "m2", "m3"]
            .into_iter()
            .map(|id| {
                let mut m = KpMember::default();
                m.member_id = StrBytes::from_static_str(id);
                m.group_instance_id = Some(StrBytes::from_static_str("instance"));
                m
            })
            .collect();
        let kp = KpRequest::default()
            .with_group_id(kafka_protocol::messages::GroupId(
                StrBytes::from_static_str("g"),
            ))
            .with_members(members);
        let mut bytes = Vec::new();
        kp.encode(&mut bytes, version).expect("dependency encodes");

        let ours = decode_request(&bytes, version).expect("ours decodes");
        assert_eq!(ours.members.len(), 3, "v{version}");
        assert_eq!(ours.members[0].member_id, "m1", "v{version}");
        assert_eq!(ours.members[1].member_id, "m2", "v{version}");
        assert_eq!(ours.members[2].member_id, "m3", "v{version}");
    }
}

/// The other direction: the dependency's decoder reads what we wrote, at
/// every version, batched.
#[test]
fn our_response_encode_matches_the_dependency() {
    use kafka_protocol::messages::LeaveGroupResponse as KpResponse;
    use kafka_protocol::protocol::Decodable;

    for version in VERSIONS {
        let response = LeaveGroupResponse {
            error_code: 0,
            members: vec![
                MemberLeft {
                    member_id: "m1",
                    error_code: 0,
                },
                MemberLeft {
                    member_id: "m2",
                    error_code: 0,
                },
            ],
        };
        let mut bytes = Vec::new();
        encode_response(&mut bytes, version, &response);

        let mut rest = &bytes[..];
        let theirs = KpResponse::decode(&mut rest, version).expect("the dependency decodes");
        assert!(rest.is_empty(), "v{version}: nothing after the body");
        assert_eq!(theirs.error_code, 0, "v{version}");
        if version >= 3 {
            assert_eq!(theirs.members.len(), 2, "v{version}");
            assert_eq!(theirs.members[0].member_id.as_str(), "m1", "v{version}");
            assert_eq!(theirs.members[1].member_id.as_str(), "m2", "v{version}");
        } else {
            assert!(theirs.members.is_empty(), "v{version}");
        }
    }
}

/// A top-level refusal (a malformed request never reaching any member):
/// no per-member answers, just the top-level code.
#[test]
fn our_response_encode_matches_the_dependency_on_a_top_level_refusal() {
    use kafka_protocol::messages::LeaveGroupResponse as KpResponse;
    use kafka_protocol::protocol::Decodable;

    for version in VERSIONS {
        let response = LeaveGroupResponse {
            error_code: 42, // INVALID_REQUEST
            members: Vec::new(),
        };
        let mut bytes = Vec::new();
        encode_response(&mut bytes, version, &response);

        let mut rest = &bytes[..];
        let theirs = KpResponse::decode(&mut rest, version).expect("the dependency decodes");
        assert_eq!(theirs.error_code, 42, "v{version}");
        assert!(theirs.members.is_empty(), "v{version}");
    }
}

/// ⚠️ **A truncated body is refused, not guessed at**, `security.md` rule 3.
#[test]
fn every_prefix_of_a_request_is_refused_rather_than_panicking() {
    use kafka_protocol::messages::LeaveGroupRequest as KpRequest;
    use kafka_protocol::messages::leave_group_request::MemberIdentity as KpMember;
    use kafka_protocol::protocol::{Encodable, StrBytes};

    for version in VERSIONS {
        let kp = if version <= 2 {
            KpRequest::default()
                .with_group_id(kafka_protocol::messages::GroupId(
                    StrBytes::from_static_str("g"),
                ))
                .with_member_id(StrBytes::from_static_str("m1"))
        } else {
            let mut member = KpMember::default();
            member.member_id = StrBytes::from_static_str("m1");
            KpRequest::default()
                .with_group_id(kafka_protocol::messages::GroupId(
                    StrBytes::from_static_str("g"),
                ))
                .with_members(vec![member])
        };
        let mut bytes = Vec::new();
        kp.encode(&mut bytes, version).expect("dependency encodes");

        for cut in 0..bytes.len() {
            let _ = decode_request(&bytes[..cut], version);
        }
        assert!(decode_request(&bytes, version).is_ok(), "v{version}");
    }
}
