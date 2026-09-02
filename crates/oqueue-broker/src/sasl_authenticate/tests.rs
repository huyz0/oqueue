#![allow(clippy::expect_used)]

use super::{PlainCredential, PlainCredentials, handle};
use crate::connection::HandlerResponse;
use bytes::Bytes;
use kafka_protocol::messages::{SaslAuthenticateRequest, SaslAuthenticateResponse};
use kafka_protocol::protocol::{Decodable, Encodable};
use oqueue_codec::error_codes;
use oqueue_codec::frame::RequestPrelude;
use oqueue_core::{Principal, Redacted};

const VERSION: i16 = 1;

fn prelude() -> RequestPrelude {
    RequestPrelude {
        api_key: 36,
        api_version: VERSION,
        correlation_id: 11,
    }
}

/// SASL/PLAIN's RFC 4616 wire shape: `authzid NUL authcid NUL passwd`.
/// `authzid` is conventionally empty when the client authenticates as
/// itself, which every test here does.
fn plain_bytes(authcid: &str, password: &str) -> Bytes {
    let mut buf = Vec::new();
    buf.push(0);
    buf.extend_from_slice(authcid.as_bytes());
    buf.push(0);
    buf.extend_from_slice(password.as_bytes());
    Bytes::from(buf)
}

fn one_credential(name: &str, password: &str) -> PlainCredentials {
    PlainCredentials::new(vec![PlainCredential {
        principal: Principal::new(name).expect("valid"),
        password: Redacted::new(password.to_owned()),
    }])
}

fn replied(body: &[u8], tls: bool, credentials: &PlainCredentials) -> SaslAuthenticateResponse {
    let HandlerResponse::Reply(out) = handle(prelude(), body, tls, credentials) else {
        panic!("a well-formed SaslAuthenticate replies");
    };
    let mut rest = &out[4..];
    let response = SaslAuthenticateResponse::decode(&mut rest, VERSION).expect("decodes");
    assert!(rest.is_empty());
    response
}

fn encoded(auth_bytes: Bytes) -> Vec<u8> {
    let request = SaslAuthenticateRequest::default().with_auth_bytes(auth_bytes);
    let mut body = Vec::new();
    request.encode(&mut body, VERSION).expect("encodes");
    body
}

/// ⚠️ **The one property `ADR-0032` exists for**: no credential, however
/// correct, succeeds without TLS. The refusal is load-bearing security,
/// not a formality — `M9.4`'s own reason to test it before anything else.
#[test]
fn correct_credentials_are_refused_without_tls() {
    let creds = one_credential("alice", "secret");
    let body = encoded(plain_bytes("alice", "secret"));

    let response = replied(&body, false, &creds);
    assert_eq!(response.error_code, error_codes::SASL_AUTHENTICATION_FAILED);
    assert!(response.error_message.is_some());
}

#[test]
fn correct_credentials_over_tls_succeed() {
    let creds = one_credential("alice", "secret");
    let body = encoded(plain_bytes("alice", "secret"));

    let response = replied(&body, true, &creds);
    assert_eq!(response.error_code, 0, "{:?}", response.error_message);
    assert!(response.error_message.is_none());
}

#[test]
fn a_wrong_password_over_tls_is_refused() {
    let creds = one_credential("alice", "secret");
    let body = encoded(plain_bytes("alice", "wrong"));

    let response = replied(&body, true, &creds);
    assert_eq!(response.error_code, error_codes::SASL_AUTHENTICATION_FAILED);
}

#[test]
fn an_unknown_principal_over_tls_is_refused() {
    let creds = one_credential("alice", "secret");
    let body = encoded(plain_bytes("mallory", "secret"));

    let response = replied(&body, true, &creds);
    assert_eq!(response.error_code, error_codes::SASL_AUTHENTICATION_FAILED);
}

/// ⚠️ **The same refusal, not a distinguishable one.** A different error
/// for "no such user" vs. "wrong password" would let a client enumerate
/// valid usernames — `security.md`'s own "never trust it" applied to what
/// an error message may reveal.
#[test]
fn an_unknown_principal_and_a_wrong_password_answer_the_same_error_message() {
    let creds = one_credential("alice", "secret");
    let unknown = replied(&encoded(plain_bytes("mallory", "secret")), true, &creds);
    let wrong = replied(&encoded(plain_bytes("alice", "wrong")), true, &creds);
    assert_eq!(unknown.error_message, wrong.error_message);
}

#[test]
fn tls_with_no_credentials_configured_is_still_refused() {
    let body = encoded(plain_bytes("alice", "secret"));
    let response = replied(&body, true, &PlainCredentials::default());
    assert_eq!(response.error_code, error_codes::SASL_AUTHENTICATION_FAILED);
}

/// A second, distinct principal in the same credential set authenticates
/// as itself, not as the first — the multi-tenant shape `M9.13`'s
/// cross-principal testing needs at least two real principals to exist.
#[test]
fn a_second_configured_principal_authenticates_independently() {
    let creds = PlainCredentials::new(vec![
        PlainCredential {
            principal: Principal::new("alice").expect("valid"),
            password: Redacted::new("secret1".to_owned()),
        },
        PlainCredential {
            principal: Principal::new("bob").expect("valid"),
            password: Redacted::new("secret2".to_owned()),
        },
    ]);
    let alice = replied(&encoded(plain_bytes("alice", "secret1")), true, &creds);
    assert_eq!(alice.error_code, 0);
    let bob = replied(&encoded(plain_bytes("bob", "secret2")), true, &creds);
    assert_eq!(bob.error_code, 0);
    // Cross-wired credentials do not authenticate either principal.
    let crossed = replied(&encoded(plain_bytes("alice", "secret2")), true, &creds);
    assert_eq!(crossed.error_code, error_codes::SASL_AUTHENTICATION_FAILED);
}

/// A malformed exchange (not exactly three NUL-separated fields, or
/// non-UTF8 content) is refused, not treated as an unknown principal --
/// the two are different failure classes.
#[test]
fn a_malformed_plain_exchange_over_tls_is_refused() {
    let creds = one_credential("alice", "secret");
    for malformed in [
        Bytes::from_static(b"no nulls at all"),
        Bytes::from_static(b"\x00only-one-field"),
        Bytes::from_static(b"\x00authcid\x00pass\x00extra"),
        Bytes::from_static(&[0, 0xFF, 0xFE, 0, b'p']), // invalid UTF-8 authcid
    ] {
        let response = replied(&encoded(malformed), true, &creds);
        assert_eq!(response.error_code, error_codes::SASL_AUTHENTICATION_FAILED);
    }
}

/// ⚠️ A malformed body (not even a decodable `SaslAuthenticateRequest`)
/// closes, the dispatcher's policy for every unanswerable shape --
/// unchanged from `M9.3`.
#[test]
fn a_malformed_body_closes_rather_than_panicking() {
    let creds = one_credential("alice", "secret");
    assert!(matches!(
        handle(prelude(), &[0xFF, 0xFF], true, &creds),
        HandlerResponse::Close
    ));
}
