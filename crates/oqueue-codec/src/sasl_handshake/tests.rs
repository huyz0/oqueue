#![allow(clippy::expect_used)]

use super::{SaslHandshakeResponse, decode_request, encode_response};

/// Every advertised `SaslHandshake` version.
const VERSIONS: [i16; 2] = [0, 1];

/// The `ADR-0019` oracle: our decoder reads what the dependency's encoder
/// wrote, at every advertised version — never flexible, so one shape
/// covers both.
#[test]
fn our_request_decode_matches_the_dependency() {
    use kafka_protocol::messages::SaslHandshakeRequest as KpRequest;
    use kafka_protocol::protocol::{Encodable, StrBytes};

    for version in VERSIONS {
        let kp = KpRequest::default().with_mechanism(StrBytes::from_static_str("PLAIN"));
        let mut bytes = Vec::new();
        kp.encode(&mut bytes, version).expect("dependency encodes");

        let ours = decode_request(&bytes).expect("ours decodes");
        assert_eq!(ours.mechanism, "PLAIN", "v{version}");
    }
}

/// The other direction: the dependency's decoder reads what we wrote,
/// field for field, at every advertised version.
#[test]
fn our_response_encode_matches_the_dependency() {
    use kafka_protocol::messages::SaslHandshakeResponse as KpResponse;
    use kafka_protocol::protocol::{Decodable, StrBytes};

    for version in VERSIONS {
        let response = SaslHandshakeResponse {
            error_code: 0,
            mechanisms: &["PLAIN"],
        };
        let mut bytes = Vec::new();
        encode_response(&mut bytes, &response);

        let mut rest = &bytes[..];
        let theirs = KpResponse::decode(&mut rest, version).expect("the dependency decodes");
        assert!(rest.is_empty(), "v{version}: nothing after the body");
        assert_eq!(theirs.error_code, 0, "v{version}");
        assert_eq!(
            theirs.mechanisms,
            vec![StrBytes::from_static_str("PLAIN")],
            "v{version}"
        );
    }
}

/// A refusal still carries the mechanism list, so the client can retry.
#[test]
fn a_refusal_still_carries_the_mechanism_list() {
    use kafka_protocol::messages::SaslHandshakeResponse as KpResponse;
    use kafka_protocol::protocol::{Decodable, StrBytes};

    for version in VERSIONS {
        let response = SaslHandshakeResponse {
            error_code: 33, // UNSUPPORTED_SASL_MECHANISM
            mechanisms: &["PLAIN"],
        };
        let mut bytes = Vec::new();
        encode_response(&mut bytes, &response);

        let mut rest = &bytes[..];
        let theirs = KpResponse::decode(&mut rest, version).expect("the dependency decodes");
        assert_eq!(theirs.error_code, 33, "v{version}");
        assert_eq!(
            theirs.mechanisms,
            vec![StrBytes::from_static_str("PLAIN")],
            "v{version}: the list survives a refusal"
        );
    }
}

/// ⚠️ **A truncated body is refused, not guessed at**, `security.md` rule 3.
#[test]
fn every_prefix_of_a_request_is_refused_rather_than_panicking() {
    use kafka_protocol::messages::SaslHandshakeRequest as KpRequest;
    use kafka_protocol::protocol::{Encodable, StrBytes};

    let kp = KpRequest::default().with_mechanism(StrBytes::from_static_str("PLAIN"));
    let mut bytes = Vec::new();
    kp.encode(&mut bytes, 1).expect("dependency encodes");

    for cut in 0..bytes.len() {
        let _ = decode_request(&bytes[..cut]);
    }
    assert!(decode_request(&bytes).is_ok());
}

/// A null mechanism is refused rather than decoded as an empty string —
/// the schema declares the field non-nullable.
#[test]
fn a_null_mechanism_is_refused() {
    use crate::flex::put_nullable_string;
    use crate::wire::DecodeError;

    let mut bytes = Vec::new();
    put_nullable_string(&mut bytes, false, None);
    assert!(matches!(
        decode_request(&bytes),
        Err(DecodeError::UnexpectedNull { .. })
    ));
}
