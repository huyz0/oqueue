#![allow(clippy::expect_used)]

use super::handle;
use crate::connection::HandlerResponse;
use kafka_protocol::messages::{SaslHandshakeRequest, SaslHandshakeResponse};
use kafka_protocol::protocol::{Decodable, Encodable, StrBytes};
use oqueue_codec::error_codes;
use oqueue_codec::frame::RequestPrelude;

const VERSION: i16 = 1;

fn prelude() -> RequestPrelude {
    RequestPrelude {
        api_key: 17,
        api_version: VERSION,
        correlation_id: 9,
    }
}

fn replied(body: &[u8]) -> SaslHandshakeResponse {
    let HandlerResponse::Reply(out) = handle(prelude(), body) else {
        panic!("a well-formed SaslHandshake replies");
    };
    // Response header: correlation id only -- SaslHandshake's response
    // header is always v0 (never tagged fields), the same special case
    // ApiVersions carries.
    let mut rest = &out[4..];
    let response = SaslHandshakeResponse::decode(&mut rest, VERSION).expect("decodes");
    assert!(rest.is_empty());
    response
}

/// The one mechanism `ADR-0032` enables is accepted.
#[test]
fn plain_is_accepted() {
    let request =
        SaslHandshakeRequest::default().with_mechanism(StrBytes::from_static_str("PLAIN"));
    let mut body = Vec::new();
    request.encode(&mut body, VERSION).expect("encodes");

    let response = replied(&body);
    assert_eq!(response.error_code, 0);
    assert_eq!(
        response.mechanisms,
        vec![StrBytes::from_static_str("PLAIN")]
    );
}

/// Anything else is refused, but the mechanism list still comes back so
/// the client can retry with one this broker actually serves.
#[test]
fn an_unsupported_mechanism_is_refused_but_the_list_still_comes_back() {
    let request =
        SaslHandshakeRequest::default().with_mechanism(StrBytes::from_static_str("GSSAPI"));
    let mut body = Vec::new();
    request.encode(&mut body, VERSION).expect("encodes");

    let response = replied(&body);
    assert_eq!(response.error_code, error_codes::UNSUPPORTED_SASL_MECHANISM);
    assert_eq!(
        response.mechanisms,
        vec![StrBytes::from_static_str("PLAIN")]
    );
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
