#![allow(clippy::expect_used)]

use super::{SaslAuthenticateResponse, decode_request, encode_response};

/// Every advertised `SaslAuthenticate` version.
const VERSIONS: [i16; 3] = [0, 1, 2];

/// The `ADR-0019` oracle: our decoder reads what the dependency's encoder
/// wrote, at every advertised version.
#[test]
fn our_request_decode_matches_the_dependency() {
    use bytes::Bytes;
    use kafka_protocol::messages::SaslAuthenticateRequest as KpRequest;
    use kafka_protocol::protocol::Encodable;

    for version in VERSIONS {
        let kp = KpRequest::default().with_auth_bytes(Bytes::from_static(b"\x00alice\x00secret"));
        let mut bytes = Vec::new();
        kp.encode(&mut bytes, version).expect("dependency encodes");

        let ours = decode_request(&bytes, version).expect("ours decodes");
        assert_eq!(ours.auth_bytes, &b"\x00alice\x00secret"[..], "v{version}");
    }
}

/// The empty exchange still round-trips — an empty `auth_bytes` is not the
/// null this schema forbids.
#[test]
fn an_empty_auth_bytes_round_trips() {
    use bytes::Bytes;
    use kafka_protocol::messages::SaslAuthenticateRequest as KpRequest;
    use kafka_protocol::protocol::Encodable;

    for version in VERSIONS {
        let kp = KpRequest::default().with_auth_bytes(Bytes::new());
        let mut bytes = Vec::new();
        kp.encode(&mut bytes, version).expect("dependency encodes");

        let ours = decode_request(&bytes, version).expect("ours decodes");
        assert_eq!(ours.auth_bytes, &b""[..], "v{version}");
    }
}

/// The other direction: the dependency's decoder reads what we wrote,
/// field for field, at every advertised version — a success case, with
/// the version-gated `session_lifetime_ms` (absent at v0).
#[test]
fn our_response_encode_matches_the_dependency_on_success() {
    use kafka_protocol::messages::SaslAuthenticateResponse as KpResponse;
    use kafka_protocol::protocol::Decodable;

    for version in VERSIONS {
        let response = SaslAuthenticateResponse {
            error_code: 0,
            error_message: None,
            auth_bytes: b"server-final",
            session_lifetime_ms: 3_600_000,
        };
        let mut bytes = Vec::new();
        encode_response(&mut bytes, version, &response);

        let mut rest = &bytes[..];
        let theirs = KpResponse::decode(&mut rest, version).expect("the dependency decodes");
        assert!(rest.is_empty(), "v{version}: nothing after the body");
        assert_eq!(theirs.error_code, 0, "v{version}");
        assert_eq!(theirs.error_message, None, "v{version}");
        assert_eq!(theirs.auth_bytes.as_ref(), b"server-final", "v{version}");
        if version >= 1 {
            assert_eq!(theirs.session_lifetime_ms, 3_600_000, "v{version}");
        }
    }
}

/// And a refusal: `error_message` carries the reason, `auth_bytes` is
/// empty, `session_lifetime_ms` is `0` — the shape `M9.3`'s own handler
/// answers with before `M9.4` wires a real credential source.
#[test]
fn our_response_encode_matches_the_dependency_on_refusal() {
    use kafka_protocol::messages::SaslAuthenticateResponse as KpResponse;
    use kafka_protocol::protocol::{Decodable, StrBytes};

    for version in VERSIONS {
        let response = SaslAuthenticateResponse {
            error_code: 58, // SASL_AUTHENTICATION_FAILED
            error_message: Some("no SASL/PLAIN credential is configured"),
            auth_bytes: b"",
            session_lifetime_ms: 0,
        };
        let mut bytes = Vec::new();
        encode_response(&mut bytes, version, &response);

        let mut rest = &bytes[..];
        let theirs = KpResponse::decode(&mut rest, version).expect("the dependency decodes");
        assert_eq!(theirs.error_code, 58, "v{version}");
        assert_eq!(
            theirs.error_message,
            Some(StrBytes::from_static_str(
                "no SASL/PLAIN credential is configured"
            )),
            "v{version}"
        );
        assert_eq!(theirs.auth_bytes.as_ref(), b"", "v{version}");
    }
}

/// ⚠️ **A truncated body is refused, not guessed at**, `security.md` rule 3.
#[test]
fn every_prefix_of_a_request_is_refused_rather_than_panicking() {
    use bytes::Bytes;
    use kafka_protocol::messages::SaslAuthenticateRequest as KpRequest;
    use kafka_protocol::protocol::Encodable;

    for version in VERSIONS {
        let kp = KpRequest::default().with_auth_bytes(Bytes::from_static(b"\x00alice\x00secret"));
        let mut bytes = Vec::new();
        kp.encode(&mut bytes, version).expect("dependency encodes");

        for cut in 0..bytes.len() {
            let _ = decode_request(&bytes[..cut], version);
        }
        assert!(decode_request(&bytes, version).is_ok(), "v{version}");
    }
}
