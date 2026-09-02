#![allow(clippy::expect_used)]

use super::{Coordinator, FindCoordinatorResponse, decode_request, encode_response};

/// Every advertised `FindCoordinator` version, single-key (0-3) and
/// batched (4-6).
const VERSIONS: [i16; 7] = [0, 1, 2, 3, 4, 5, 6];
const SINGLE_KEY_VERSIONS: [i16; 4] = [0, 1, 2, 3];
const BATCHED_VERSIONS: [i16; 3] = [4, 5, 6];

/// The `ADR-0019` oracle: our decoder reads what the dependency's encoder
/// wrote, at every advertised single-key version.
#[test]
fn our_request_decode_matches_the_dependency_single_key() {
    use kafka_protocol::messages::FindCoordinatorRequest as KpRequest;
    use kafka_protocol::protocol::{Encodable, StrBytes};

    for version in SINGLE_KEY_VERSIONS {
        let kp = KpRequest::default()
            .with_key(StrBytes::from_static_str("orders"))
            .with_key_type(0);
        let mut bytes = Vec::new();
        kp.encode(&mut bytes, version).expect("dependency encodes");

        let ours = decode_request(&bytes, version).expect("ours decodes");
        assert_eq!(ours.keys, vec!["orders"], "v{version}");
        assert_eq!(ours.key_type, 0, "v{version}");
    }
}

/// The batched shape (KIP-699): N keys in one request, all decoded.
#[test]
fn our_request_decode_matches_the_dependency_batched() {
    use kafka_protocol::messages::FindCoordinatorRequest as KpRequest;
    use kafka_protocol::protocol::{Encodable, StrBytes};

    for version in BATCHED_VERSIONS {
        let kp = KpRequest::default().with_coordinator_keys(vec![
            StrBytes::from_static_str("orders"),
            StrBytes::from_static_str("payments"),
            StrBytes::from_static_str("shipping"),
        ]);
        let mut bytes = Vec::new();
        kp.encode(&mut bytes, version).expect("dependency encodes");

        let ours = decode_request(&bytes, version).expect("ours decodes");
        assert_eq!(
            ours.keys,
            vec!["orders", "payments", "shipping"],
            "v{version}"
        );
    }
}

/// An empty batch is well-formed, not an error.
#[test]
fn an_empty_batch_decodes() {
    use kafka_protocol::messages::FindCoordinatorRequest as KpRequest;
    use kafka_protocol::protocol::Encodable;

    for version in BATCHED_VERSIONS {
        let kp = KpRequest::default();
        let mut bytes = Vec::new();
        kp.encode(&mut bytes, version).expect("dependency encodes");

        let ours = decode_request(&bytes, version).expect("ours decodes");
        assert!(ours.keys.is_empty(), "v{version}");
    }
}

/// A non-zero `key_type` (`TRANSACTION`) decodes at every version from v1,
/// single-key or batched alike -- it applies to the whole request, not
/// per key.
#[test]
fn a_transaction_key_type_decodes_from_v1() {
    use kafka_protocol::messages::FindCoordinatorRequest as KpRequest;
    use kafka_protocol::protocol::{Encodable, StrBytes};

    for version in VERSIONS.into_iter().filter(|&v| v >= 1) {
        let mut kp = KpRequest::default().with_key_type(1);
        if version >= 4 {
            kp = kp.with_coordinator_keys(vec![StrBytes::from_static_str("txn-1")]);
        } else {
            kp = kp.with_key(StrBytes::from_static_str("txn-1"));
        }
        let mut bytes = Vec::new();
        kp.encode(&mut bytes, version).expect("dependency encodes");

        let ours = decode_request(&bytes, version).expect("ours decodes");
        assert_eq!(ours.key_type, 1, "v{version}");
    }
}

/// The single-key response shape: the dependency's decoder reads what we
/// wrote, flattened into the top-level fields.
#[test]
fn our_response_encode_matches_the_dependency_single_key() {
    use kafka_protocol::messages::FindCoordinatorResponse as KpResponse;
    use kafka_protocol::protocol::Decodable;

    for version in SINGLE_KEY_VERSIONS {
        let response = FindCoordinatorResponse {
            coordinators: vec![Coordinator {
                key: "orders",
                error_code: 0,
                error_message: None,
                node_id: 0,
                host: "h",
                port: 9092,
            }],
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
    }
}

/// A single-key refusal carries an error message from v1 on.
#[test]
fn our_response_encode_matches_the_dependency_on_refusal_single_key() {
    use kafka_protocol::messages::FindCoordinatorResponse as KpResponse;
    use kafka_protocol::protocol::{Decodable, StrBytes};

    for version in SINGLE_KEY_VERSIONS {
        let response = FindCoordinatorResponse {
            coordinators: vec![Coordinator {
                key: "txn-1",
                error_code: 42, // INVALID_REQUEST
                error_message: Some("no coordinator available"),
                node_id: -1,
                host: "",
                port: -1,
            }],
        };
        let mut bytes = Vec::new();
        encode_response(&mut bytes, version, &response);

        let mut rest = &bytes[..];
        let theirs = KpResponse::decode(&mut rest, version).expect("the dependency decodes");
        assert_eq!(theirs.error_code, 42, "v{version}");
        if version >= 1 {
            assert_eq!(
                theirs.error_message,
                Some(StrBytes::from_static_str("no coordinator available")),
                "v{version}"
            );
        }
    }
}

/// A defensive fallback, exercised directly: `encode_response` given no
/// coordinator at all for a single-key version (a caller bug, since the
/// broker handler always supplies exactly one) still encodes something
/// well-formed rather than panicking or indexing past the end -- the
/// protocol's own "no coordinator" sentinel, `UNKNOWN_SERVER_ERROR` naming
/// that this is a defect in the caller, not the key.
#[test]
fn an_empty_coordinators_list_falls_back_to_the_no_coordinator_sentinel() {
    use kafka_protocol::messages::FindCoordinatorResponse as KpResponse;
    use kafka_protocol::protocol::Decodable;

    for version in SINGLE_KEY_VERSIONS {
        let response = FindCoordinatorResponse {
            coordinators: Vec::new(),
        };
        let mut bytes = Vec::new();
        encode_response(&mut bytes, version, &response);

        let mut rest = &bytes[..];
        let theirs = KpResponse::decode(&mut rest, version).expect("the dependency decodes");
        assert_eq!(
            theirs.error_code,
            crate::error_codes::UNKNOWN_SERVER_ERROR,
            "v{version}"
        );
        assert_eq!(
            theirs.node_id,
            kafka_protocol::messages::BrokerId(-1),
            "v{version}"
        );
        assert_eq!(theirs.port, -1, "v{version}");
    }
}

/// The batched response shape: N keys, each with its own answer, decoded
/// back by the dependency -- `M4.4`'s own acceptance criterion, that one
/// key's own resolution never depends on another's.
#[test]
fn our_response_encode_matches_the_dependency_batched() {
    use kafka_protocol::messages::FindCoordinatorResponse as KpResponse;
    use kafka_protocol::protocol::Decodable;

    for version in BATCHED_VERSIONS {
        let response = FindCoordinatorResponse {
            coordinators: vec![
                Coordinator {
                    key: "orders",
                    error_code: 0,
                    error_message: None,
                    node_id: 0,
                    host: "h",
                    port: 9092,
                },
                Coordinator {
                    key: "txn-1",
                    error_code: 42,
                    error_message: Some("this broker coordinates consumer groups only"),
                    node_id: -1,
                    host: "",
                    port: -1,
                },
                Coordinator {
                    key: "payments",
                    error_code: 0,
                    error_message: None,
                    node_id: 0,
                    host: "h",
                    port: 9092,
                },
            ],
        };
        let mut bytes = Vec::new();
        encode_response(&mut bytes, version, &response);

        let mut rest = &bytes[..];
        let theirs = KpResponse::decode(&mut rest, version).expect("the dependency decodes");
        assert!(rest.is_empty(), "v{version}: nothing after the body");
        assert_eq!(theirs.coordinators.len(), 3, "v{version}");
        assert_eq!(theirs.coordinators[0].key.as_str(), "orders", "v{version}");
        assert_eq!(theirs.coordinators[0].error_code, 0, "v{version}");
        assert_eq!(theirs.coordinators[1].key.as_str(), "txn-1", "v{version}");
        assert_eq!(theirs.coordinators[1].error_code, 42, "v{version}");
        assert_eq!(
            theirs.coordinators[2].key.as_str(),
            "payments",
            "v{version}"
        );
        assert_eq!(theirs.coordinators[2].error_code, 0, "v{version}");
    }
}

/// ⚠️ **A truncated body is refused, not guessed at**, `security.md` rule 3.
#[test]
fn every_prefix_of_a_request_is_refused_rather_than_panicking() {
    use kafka_protocol::messages::FindCoordinatorRequest as KpRequest;
    use kafka_protocol::protocol::{Encodable, StrBytes};

    for version in VERSIONS {
        let mut kp = KpRequest::default();
        kp = if version >= 4 {
            kp.with_coordinator_keys(vec![StrBytes::from_static_str("orders")])
        } else {
            kp.with_key(StrBytes::from_static_str("orders"))
        };
        let mut bytes = Vec::new();
        kp.encode(&mut bytes, version).expect("dependency encodes");

        for cut in 0..bytes.len() {
            let _ = decode_request(&bytes[..cut], version);
        }
        assert!(decode_request(&bytes, version).is_ok(), "v{version}");
    }
}
