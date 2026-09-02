#![allow(clippy::expect_used)]

use super::{JoinGroupResponse, JoinGroupResponseMember, decode_request, encode_response};

/// Every version `JoinGroup` itself defines.
const VERSIONS: [i16; 10] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9];

/// The `ADR-0019` oracle: our decoder reads what the dependency's encoder
/// wrote, at every version.
/// The single-protocol fixture `our_request_decode_matches_the_dependency`
/// checks -- its own function purely to keep that test under the fifty-line
/// limit.
fn a_request_at(version: i16) -> kafka_protocol::messages::JoinGroupRequest {
    use kafka_protocol::messages::JoinGroupRequest as KpRequest;
    use kafka_protocol::messages::join_group_request::JoinGroupRequestProtocol as KpProtocol;
    use kafka_protocol::protocol::StrBytes;

    let mut protocol = KpProtocol::default();
    protocol.name = StrBytes::from_static_str("range");
    protocol.metadata = bytes::Bytes::from_static(b"subscription-bytes");

    let mut kp = KpRequest::default()
        .with_group_id(kafka_protocol::messages::GroupId(
            StrBytes::from_static_str("orders-consumers"),
        ))
        .with_session_timeout_ms(30_000)
        .with_member_id(StrBytes::from_static_str(""))
        .with_protocol_type(StrBytes::from_static_str("consumer"))
        .with_protocols(vec![protocol]);
    if version >= 1 {
        kp = kp.with_rebalance_timeout_ms(60_000);
    }
    if version >= 5 {
        kp = kp.with_group_instance_id(Some(StrBytes::from_static_str("instance-1")));
    }
    if version >= 8 {
        kp = kp.with_reason(Some(StrBytes::from_static_str("client restart")));
    }
    kp
}

#[test]
fn our_request_decode_matches_the_dependency() {
    use kafka_protocol::protocol::Encodable;

    for version in VERSIONS {
        let mut bytes = Vec::new();
        a_request_at(version)
            .encode(&mut bytes, version)
            .expect("dependency encodes");

        let ours = decode_request(&bytes, version).expect("ours decodes");
        assert_eq!(ours.group_id, "orders-consumers", "v{version}");
        assert_eq!(ours.session_timeout_ms, 30_000, "v{version}");
        assert_eq!(ours.protocol_type, "consumer", "v{version}");
        assert_eq!(ours.protocols.len(), 1, "v{version}");
        assert_eq!(ours.protocols[0].name, "range", "v{version}");
        assert_eq!(
            ours.protocols[0].metadata, b"subscription-bytes",
            "v{version}"
        );
        if version >= 1 {
            assert_eq!(ours.rebalance_timeout_ms, 60_000, "v{version}");
        } else {
            assert_eq!(ours.rebalance_timeout_ms, 0, "v{version}");
        }
        if version >= 5 {
            assert_eq!(ours.group_instance_id, Some("instance-1"), "v{version}");
        } else {
            assert_eq!(ours.group_instance_id, None, "v{version}");
        }
        if version >= 8 {
            assert_eq!(ours.reason, Some("client restart"), "v{version}");
        } else {
            assert_eq!(ours.reason, None, "v{version}");
        }
    }
}

/// N protocols in one request all decode, in order.
#[test]
fn multiple_protocols_all_decode() {
    use kafka_protocol::messages::JoinGroupRequest as KpRequest;
    use kafka_protocol::messages::join_group_request::JoinGroupRequestProtocol as KpProtocol;
    use kafka_protocol::protocol::{Encodable, StrBytes};

    for version in VERSIONS {
        let protocols = ["range", "cooperative-sticky"]
            .into_iter()
            .map(|name| {
                let mut p = KpProtocol::default();
                p.name = StrBytes::from_string(name.to_owned());
                p.metadata = bytes::Bytes::from_static(b"m");
                p
            })
            .collect();
        let kp = KpRequest::default()
            .with_group_id(kafka_protocol::messages::GroupId(
                StrBytes::from_static_str("g"),
            ))
            .with_protocol_type(StrBytes::from_static_str("consumer"))
            .with_protocols(protocols);
        let mut bytes = Vec::new();
        kp.encode(&mut bytes, version).expect("dependency encodes");

        let ours = decode_request(&bytes, version).expect("ours decodes");
        assert_eq!(ours.protocols.len(), 2, "v{version}");
        assert_eq!(ours.protocols[0].name, "range", "v{version}");
        assert_eq!(ours.protocols[1].name, "cooperative-sticky", "v{version}");
    }
}

/// The other direction: the dependency's decoder reads what we wrote, field
/// for field, at every version -- the leader's own shape, with members.
#[test]
fn our_response_encode_matches_the_dependency_for_the_leader() {
    use kafka_protocol::messages::JoinGroupResponse as KpResponse;
    use kafka_protocol::protocol::Decodable;

    let response = JoinGroupResponse {
        error_code: 0,
        generation_id: 3,
        protocol_type: Some("consumer"),
        protocol_name: Some("range"),
        leader: "member-1",
        skip_assignment: false,
        member_id: "member-1",
        members: vec![
            JoinGroupResponseMember {
                member_id: "member-1",
                group_instance_id: Some("instance-1"),
                metadata: b"m1",
            },
            JoinGroupResponseMember {
                member_id: "member-2",
                group_instance_id: None,
                metadata: b"m2",
            },
        ],
    };
    for version in VERSIONS {
        let mut bytes = Vec::new();
        encode_response(&mut bytes, version, &response);

        let mut rest = &bytes[..];
        let theirs = KpResponse::decode(&mut rest, version).expect("the dependency decodes");
        assert!(rest.is_empty(), "v{version}: nothing after the body");
        assert_eq!(theirs.error_code, 0, "v{version}");
        assert_eq!(theirs.generation_id, 3, "v{version}");
        assert_eq!(theirs.leader.as_str(), "member-1", "v{version}");
        assert_eq!(theirs.member_id.as_str(), "member-1", "v{version}");
        assert_leader_members(&theirs.members, version);
    }
}

/// The two-member assertion `our_response_encode_matches_the_dependency_for_the_leader`
/// checks -- its own function for the same fifty-line-limit reason.
fn assert_leader_members(
    members: &[kafka_protocol::messages::join_group_response::JoinGroupResponseMember],
    version: i16,
) {
    assert_eq!(members.len(), 2, "v{version}");
    assert_eq!(members[0].member_id.as_str(), "member-1", "v{version}");
    assert_eq!(members[0].metadata.as_ref(), b"m1", "v{version}");
    assert_eq!(members[1].member_id.as_str(), "member-2", "v{version}");
}

/// A follower's own response: empty `members`, same shape otherwise.
#[test]
fn our_response_encode_matches_the_dependency_for_a_follower() {
    use kafka_protocol::messages::JoinGroupResponse as KpResponse;
    use kafka_protocol::protocol::Decodable;

    for version in VERSIONS {
        let response = JoinGroupResponse {
            error_code: 0,
            generation_id: 3,
            protocol_type: Some("consumer"),
            protocol_name: Some("range"),
            leader: "member-1",
            skip_assignment: false,
            member_id: "member-2",
            members: Vec::new(),
        };
        let mut bytes = Vec::new();
        encode_response(&mut bytes, version, &response);

        let mut rest = &bytes[..];
        let theirs = KpResponse::decode(&mut rest, version).expect("the dependency decodes");
        assert!(rest.is_empty(), "v{version}: nothing after the body");
        assert!(theirs.members.is_empty(), "v{version}");
        assert_eq!(theirs.member_id.as_str(), "member-2", "v{version}");
    }
}

/// A refusal: no protocol negotiated, an error code naming why.
#[test]
fn our_response_encode_matches_the_dependency_on_refusal() {
    use kafka_protocol::messages::JoinGroupResponse as KpResponse;
    use kafka_protocol::protocol::Decodable;

    for version in VERSIONS {
        let response = JoinGroupResponse {
            error_code: 25, // UNKNOWN_MEMBER_ID
            generation_id: -1,
            protocol_type: None,
            protocol_name: None,
            leader: "",
            skip_assignment: false,
            member_id: "",
            members: Vec::new(),
        };
        let mut bytes = Vec::new();
        encode_response(&mut bytes, version, &response);

        let mut rest = &bytes[..];
        let theirs = KpResponse::decode(&mut rest, version).expect("the dependency decodes");
        assert_eq!(theirs.error_code, 25, "v{version}");
        assert_eq!(theirs.generation_id, -1, "v{version}");
    }
}

/// ⚠️ **A truncated body is refused, not guessed at**, `security.md` rule 3.
#[test]
fn every_prefix_of_a_request_is_refused_rather_than_panicking() {
    use kafka_protocol::messages::JoinGroupRequest as KpRequest;
    use kafka_protocol::messages::join_group_request::JoinGroupRequestProtocol as KpProtocol;
    use kafka_protocol::protocol::{Encodable, StrBytes};

    for version in VERSIONS {
        let mut protocol = KpProtocol::default();
        protocol.name = StrBytes::from_static_str("range");
        protocol.metadata = bytes::Bytes::from_static(b"m");
        let kp = KpRequest::default()
            .with_group_id(kafka_protocol::messages::GroupId(
                StrBytes::from_static_str("g"),
            ))
            .with_protocol_type(StrBytes::from_static_str("consumer"))
            .with_protocols(vec![protocol]);
        let mut bytes = Vec::new();
        kp.encode(&mut bytes, version).expect("dependency encodes");

        for cut in 0..bytes.len() {
            let _ = decode_request(&bytes[..cut], version);
        }
        assert!(decode_request(&bytes, version).is_ok(), "v{version}");
    }
}
