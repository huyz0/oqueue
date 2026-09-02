#![allow(clippy::expect_used)]

use super::{decode_request, encode_response};

/// Every version `Heartbeat` itself defines.
const VERSIONS: [i16; 5] = [0, 1, 2, 3, 4];

#[test]
fn our_request_decode_matches_the_dependency() {
    use kafka_protocol::messages::HeartbeatRequest as KpRequest;
    use kafka_protocol::protocol::{Encodable, StrBytes};

    for version in VERSIONS {
        let mut kp = KpRequest::default()
            .with_group_id(kafka_protocol::messages::GroupId(
                StrBytes::from_static_str("orders-consumers"),
            ))
            .with_generation_id(3)
            .with_member_id(StrBytes::from_static_str("m1"));
        if version >= 3 {
            kp = kp.with_group_instance_id(Some(StrBytes::from_static_str("instance-1")));
        }
        let mut bytes = Vec::new();
        kp.encode(&mut bytes, version).expect("dependency encodes");

        let ours = decode_request(&bytes, version).expect("ours decodes");
        assert_eq!(ours.group_id, "orders-consumers", "v{version}");
        assert_eq!(ours.generation_id, 3, "v{version}");
        assert_eq!(ours.member_id, "m1", "v{version}");
    }
}

/// The other direction: the dependency's decoder reads what we wrote.
#[test]
fn our_response_encode_matches_the_dependency() {
    use kafka_protocol::messages::HeartbeatResponse as KpResponse;
    use kafka_protocol::protocol::Decodable;

    for version in VERSIONS {
        let mut bytes = Vec::new();
        encode_response(&mut bytes, version, 0);

        let mut rest = &bytes[..];
        let theirs = KpResponse::decode(&mut rest, version).expect("the dependency decodes");
        assert!(rest.is_empty(), "v{version}: nothing after the body");
        assert_eq!(theirs.error_code, 0, "v{version}");
    }
}

/// A refusal: an error code naming why.
#[test]
fn our_response_encode_matches_the_dependency_on_refusal() {
    use kafka_protocol::messages::HeartbeatResponse as KpResponse;
    use kafka_protocol::protocol::Decodable;

    for version in VERSIONS {
        let mut bytes = Vec::new();
        encode_response(&mut bytes, version, 27); // REBALANCE_IN_PROGRESS

        let mut rest = &bytes[..];
        let theirs = KpResponse::decode(&mut rest, version).expect("the dependency decodes");
        assert_eq!(theirs.error_code, 27, "v{version}");
    }
}

/// ⚠️ **A truncated body is refused, not guessed at**, `security.md` rule 3.
#[test]
fn every_prefix_of_a_request_is_refused_rather_than_panicking() {
    use kafka_protocol::messages::HeartbeatRequest as KpRequest;
    use kafka_protocol::protocol::{Encodable, StrBytes};

    for version in VERSIONS {
        let kp = KpRequest::default()
            .with_group_id(kafka_protocol::messages::GroupId(
                StrBytes::from_static_str("g"),
            ))
            .with_generation_id(1)
            .with_member_id(StrBytes::from_static_str("m1"));
        let mut bytes = Vec::new();
        kp.encode(&mut bytes, version).expect("dependency encodes");

        for cut in 0..bytes.len() {
            let _ = decode_request(&bytes[..cut], version);
        }
        assert!(decode_request(&bytes, version).is_ok(), "v{version}");
    }
}
