#![allow(clippy::expect_used)]

use super::{FindCoordinatorResponse, decode_request, encode_response};

/// Every advertised `FindCoordinator` version, single-key.
const VERSIONS: [i16; 4] = [0, 1, 2, 3];

/// The `ADR-0019` oracle: our decoder reads what the dependency's encoder
/// wrote, at every advertised version.
#[test]
fn our_request_decode_matches_the_dependency() {
    use kafka_protocol::messages::FindCoordinatorRequest as KpRequest;
    use kafka_protocol::protocol::{Encodable, StrBytes};

    for version in VERSIONS {
        let kp = KpRequest::default()
            .with_key(StrBytes::from_static_str("orders"))
            .with_key_type(0);
        let mut bytes = Vec::new();
        kp.encode(&mut bytes, version).expect("dependency encodes");

        let ours = decode_request(&bytes, version).expect("ours decodes");
        assert_eq!(ours.key, "orders", "v{version}");
        assert_eq!(ours.key_type, 0, "v{version}");
    }
}

/// A non-zero `key_type` (`TRANSACTION`, from v1) still decodes — this
/// module's own doc names why it is accepted without being branched on.
#[test]
fn a_transaction_key_type_decodes_from_v1() {
    use kafka_protocol::messages::FindCoordinatorRequest as KpRequest;
    use kafka_protocol::protocol::{Encodable, StrBytes};

    for version in VERSIONS.into_iter().filter(|&v| v >= 1) {
        let kp = KpRequest::default()
            .with_key(StrBytes::from_static_str("txn-1"))
            .with_key_type(1);
        let mut bytes = Vec::new();
        kp.encode(&mut bytes, version).expect("dependency encodes");

        let ours = decode_request(&bytes, version).expect("ours decodes");
        assert_eq!(ours.key_type, 1, "v{version}");
    }
}

/// The other direction: the dependency's decoder reads what we wrote, field
/// for field, at every advertised version.
#[test]
fn our_response_encode_matches_the_dependency_on_success() {
    use kafka_protocol::messages::FindCoordinatorResponse as KpResponse;
    use kafka_protocol::protocol::Decodable;

    for version in VERSIONS {
        let response = FindCoordinatorResponse {
            error_code: 0,
            error_message: None,
            node_id: 0,
            host: "h",
            port: 9092,
        };
        let mut bytes = Vec::new();
        encode_response(&mut bytes, version, &response);

        let mut rest = &bytes[..];
        let theirs = KpResponse::decode(&mut rest, version).expect("the dependency decodes");
        assert!(rest.is_empty(), "v{version}: nothing after the body");
        assert_eq!(theirs.error_code, 0, "v{version}");
        assert_eq!(
            theirs.node_id,
            kafka_protocol::messages::BrokerId(0),
            "v{version}"
        );
        assert_eq!(theirs.host.as_str(), "h", "v{version}");
        assert_eq!(theirs.port, 9092, "v{version}");
        if version >= 1 {
            assert_eq!(theirs.error_message, None, "v{version}");
        }
    }
}

/// A refusal carries an error message from v1 on.
#[test]
fn our_response_encode_matches_the_dependency_on_refusal() {
    use kafka_protocol::messages::FindCoordinatorResponse as KpResponse;
    use kafka_protocol::protocol::{Decodable, StrBytes};

    for version in VERSIONS {
        let response = FindCoordinatorResponse {
            error_code: 15, // COORDINATOR_NOT_AVAILABLE, real Kafka's own code
            error_message: Some("no coordinator available"),
            node_id: -1,
            host: "",
            port: -1,
        };
        let mut bytes = Vec::new();
        encode_response(&mut bytes, version, &response);

        let mut rest = &bytes[..];
        let theirs = KpResponse::decode(&mut rest, version).expect("the dependency decodes");
        assert_eq!(theirs.error_code, 15, "v{version}");
        if version >= 1 {
            assert_eq!(
                theirs.error_message,
                Some(StrBytes::from_static_str("no coordinator available")),
                "v{version}"
            );
        }
    }
}

/// ⚠️ **A truncated body is refused, not guessed at**, `security.md` rule 3.
#[test]
fn every_prefix_of_a_request_is_refused_rather_than_panicking() {
    use kafka_protocol::messages::FindCoordinatorRequest as KpRequest;
    use kafka_protocol::protocol::{Encodable, StrBytes};

    for version in VERSIONS {
        let kp = KpRequest::default().with_key(StrBytes::from_static_str("orders"));
        let mut bytes = Vec::new();
        kp.encode(&mut bytes, version).expect("dependency encodes");

        for cut in 0..bytes.len() {
            let _ = decode_request(&bytes[..cut], version);
        }
        assert!(decode_request(&bytes, version).is_ok(), "v{version}");
    }
}
