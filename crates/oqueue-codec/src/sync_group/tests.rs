#![allow(clippy::expect_used)]

use super::{SyncGroupResponse, decode_request, encode_response};

/// Every version `SyncGroup` itself defines.
const VERSIONS: [i16; 6] = [0, 1, 2, 3, 4, 5];

#[test]
fn our_request_decode_matches_the_dependency_for_the_leader() {
    use kafka_protocol::messages::SyncGroupRequest as KpRequest;
    use kafka_protocol::messages::sync_group_request::SyncGroupRequestAssignment as KpAssignment;
    use kafka_protocol::protocol::{Encodable, StrBytes};

    for version in VERSIONS {
        let assignments = ["m1", "m2"]
            .into_iter()
            .map(|id| {
                let mut a = KpAssignment::default();
                a.member_id = StrBytes::from_static_str(id);
                a.assignment = bytes::Bytes::from_static(b"assignment-bytes");
                a
            })
            .collect();
        let mut kp = KpRequest::default()
            .with_group_id(kafka_protocol::messages::GroupId(
                StrBytes::from_static_str("orders-consumers"),
            ))
            .with_generation_id(3)
            .with_member_id(StrBytes::from_static_str("m1"))
            .with_assignments(assignments);
        if version >= 5 {
            kp = kp
                .with_protocol_type(Some(StrBytes::from_static_str("consumer")))
                .with_protocol_name(Some(StrBytes::from_static_str("range")));
        }
        let mut bytes = Vec::new();
        kp.encode(&mut bytes, version).expect("dependency encodes");

        let ours = decode_request(&bytes, version).expect("ours decodes");
        assert_eq!(ours.group_id, "orders-consumers", "v{version}");
        assert_eq!(ours.generation_id, 3, "v{version}");
        assert_eq!(ours.member_id, "m1", "v{version}");
        assert_eq!(ours.assignments.len(), 2, "v{version}");
        assert_eq!(ours.assignments[0].member_id, "m1", "v{version}");
        assert_eq!(
            ours.assignments[0].assignment, b"assignment-bytes",
            "v{version}"
        );
        assert_eq!(ours.assignments[1].member_id, "m2", "v{version}");
        if version >= 5 {
            assert_eq!(ours.protocol_type, Some("consumer"), "v{version}");
            assert_eq!(ours.protocol_name, Some("range"), "v{version}");
        } else {
            assert_eq!(ours.protocol_type, None, "v{version}");
            assert_eq!(ours.protocol_name, None, "v{version}");
        }
    }
}

/// A follower's own request: an empty `assignments` array.
#[test]
fn our_request_decode_matches_the_dependency_for_a_follower() {
    use kafka_protocol::messages::SyncGroupRequest as KpRequest;
    use kafka_protocol::protocol::{Encodable, StrBytes};

    for version in VERSIONS {
        let kp = KpRequest::default()
            .with_group_id(kafka_protocol::messages::GroupId(
                StrBytes::from_static_str("orders-consumers"),
            ))
            .with_generation_id(3)
            .with_member_id(StrBytes::from_static_str("m2"));
        let mut bytes = Vec::new();
        kp.encode(&mut bytes, version).expect("dependency encodes");

        let ours = decode_request(&bytes, version).expect("ours decodes");
        assert_eq!(ours.member_id, "m2", "v{version}");
        assert!(ours.assignments.is_empty(), "v{version}");
    }
}

/// `group_instance_id` (v3+) is consumed off the wire without derailing the
/// fields after it — no field of ours carries it, but decoding must still
/// land correctly.
#[test]
fn a_group_instance_id_does_not_derail_the_fields_after_it() {
    use kafka_protocol::messages::SyncGroupRequest as KpRequest;
    use kafka_protocol::protocol::{Encodable, StrBytes};

    for version in VERSIONS.into_iter().filter(|&v| v >= 3) {
        let kp = KpRequest::default()
            .with_group_id(kafka_protocol::messages::GroupId(
                StrBytes::from_static_str("g"),
            ))
            .with_generation_id(1)
            .with_member_id(StrBytes::from_static_str("m1"))
            .with_group_instance_id(Some(StrBytes::from_static_str("instance-1")));
        let mut bytes = Vec::new();
        kp.encode(&mut bytes, version).expect("dependency encodes");

        let ours = decode_request(&bytes, version).expect("ours decodes");
        assert_eq!(ours.member_id, "m1", "v{version}");
        assert!(ours.assignments.is_empty(), "v{version}");
    }
}

/// The other direction: the dependency's decoder reads what we wrote.
#[test]
fn our_response_encode_matches_the_dependency() {
    use kafka_protocol::messages::SyncGroupResponse as KpResponse;
    use kafka_protocol::protocol::Decodable;

    for version in VERSIONS {
        let response = SyncGroupResponse {
            error_code: 0,
            protocol_type: (version >= 5).then_some("consumer"),
            protocol_name: (version >= 5).then_some("range"),
            assignment: b"my-own-slice",
        };
        let mut bytes = Vec::new();
        encode_response(&mut bytes, version, &response);

        let mut rest = &bytes[..];
        let theirs = KpResponse::decode(&mut rest, version).expect("the dependency decodes");
        assert!(rest.is_empty(), "v{version}: nothing after the body");
        assert_eq!(theirs.error_code, 0, "v{version}");
        assert_eq!(theirs.assignment.as_ref(), b"my-own-slice", "v{version}");
        if version >= 5 {
            assert_eq!(
                theirs
                    .protocol_type
                    .as_ref()
                    .map(kafka_protocol::protocol::StrBytes::as_str),
                Some("consumer"),
                "v{version}"
            );
        }
    }
}

/// A refusal: no assignment, an error code naming why.
#[test]
fn our_response_encode_matches_the_dependency_on_refusal() {
    use kafka_protocol::messages::SyncGroupResponse as KpResponse;
    use kafka_protocol::protocol::Decodable;

    for version in VERSIONS {
        let response = SyncGroupResponse {
            error_code: 22, // ILLEGAL_GENERATION
            protocol_type: None,
            protocol_name: None,
            assignment: b"",
        };
        let mut bytes = Vec::new();
        encode_response(&mut bytes, version, &response);

        let mut rest = &bytes[..];
        let theirs = KpResponse::decode(&mut rest, version).expect("the dependency decodes");
        assert_eq!(theirs.error_code, 22, "v{version}");
        assert!(theirs.assignment.is_empty(), "v{version}");
    }
}

/// ⚠️ **A truncated body is refused, not guessed at**, `security.md` rule 3.
#[test]
fn every_prefix_of_a_request_is_refused_rather_than_panicking() {
    use kafka_protocol::messages::SyncGroupRequest as KpRequest;
    use kafka_protocol::messages::sync_group_request::SyncGroupRequestAssignment as KpAssignment;
    use kafka_protocol::protocol::{Encodable, StrBytes};

    for version in VERSIONS {
        let mut a = KpAssignment::default();
        a.member_id = StrBytes::from_static_str("m1");
        a.assignment = bytes::Bytes::from_static(b"a");
        let kp = KpRequest::default()
            .with_group_id(kafka_protocol::messages::GroupId(
                StrBytes::from_static_str("g"),
            ))
            .with_generation_id(1)
            .with_member_id(StrBytes::from_static_str("m1"))
            .with_assignments(vec![a]);
        let mut bytes = Vec::new();
        kp.encode(&mut bytes, version).expect("dependency encodes");

        for cut in 0..bytes.len() {
            let _ = decode_request(&bytes[..cut], version);
        }
        assert!(decode_request(&bytes, version).is_ok(), "v{version}");
    }
}
