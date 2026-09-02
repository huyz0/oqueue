#![allow(clippy::expect_used)]

use super::handle;
use crate::connection::HandlerResponse;
use bytes::Bytes;
use kafka_protocol::messages::{SaslAuthenticateRequest, SaslAuthenticateResponse};
use kafka_protocol::protocol::{Decodable, Encodable};
use oqueue_codec::error_codes;
use oqueue_codec::frame::RequestPrelude;

const VERSION: i16 = 1;

fn prelude() -> RequestPrelude {
    RequestPrelude {
        api_key: 36,
        api_version: VERSION,
        correlation_id: 11,
    }
}

fn replied(body: &[u8]) -> SaslAuthenticateResponse {
    let HandlerResponse::Reply(out) = handle(prelude(), body) else {
        panic!("a well-formed SaslAuthenticate replies");
    };
    // Response header: correlation id only -- SaslAuthenticate's response
    // header is v1 (tagged fields) only from v2, and this test runs at v1.
    let mut rest = &out[4..];
    let response = SaslAuthenticateResponse::decode(&mut rest, VERSION).expect("decodes");
    assert!(rest.is_empty());
    response
}

/// ⚠️ **Every attempt is refused, with no credential source configured.**
/// The `M9.3`-before-`M9.4` state, tested as real behaviour: a real
/// `SASL/PLAIN` triple gets exactly the same refusal an empty one does,
/// since nothing here checks the bytes yet.
#[test]
fn every_attempt_is_refused_with_no_credential_source_configured() {
    let request = SaslAuthenticateRequest::default()
        .with_auth_bytes(Bytes::from_static(b"\x00alice\x00secret"));
    let mut body = Vec::new();
    request.encode(&mut body, VERSION).expect("encodes");

    let response = replied(&body);
    assert_eq!(response.error_code, error_codes::SASL_AUTHENTICATION_FAILED);
    assert!(response.error_message.is_some());
    assert!(response.auth_bytes.is_empty());
    assert_eq!(response.session_lifetime_ms, 0);
}

/// The refusal does not depend on what the auth bytes look like — an
/// empty exchange is refused the same way.
#[test]
fn an_empty_exchange_is_refused_too() {
    let request = SaslAuthenticateRequest::default().with_auth_bytes(Bytes::new());
    let mut body = Vec::new();
    request.encode(&mut body, VERSION).expect("encodes");

    let response = replied(&body);
    assert_eq!(response.error_code, error_codes::SASL_AUTHENTICATION_FAILED);
}

/// ⚠️ A malformed body closes, the dispatcher's policy for every
/// unanswerable shape.
#[test]
fn a_malformed_body_closes_rather_than_panicking() {
    assert!(matches!(
        handle(prelude(), &[0xFF, 0xFF]),
        HandlerResponse::Close
    ));
}
