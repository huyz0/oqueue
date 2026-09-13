#![allow(clippy::expect_used)]

use super::{PlainCredential, PlainCredentials, handle};
use crate::connection::HandlerResponse;
use crate::session::Session;
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
    replied_on(body, tls, credentials, &Session::default())
}

fn replied_on(
    body: &[u8],
    tls: bool,
    credentials: &PlainCredentials,
    session: &Session,
) -> SaslAuthenticateResponse {
    let HandlerResponse::Reply(out) = handle(prelude(), body, tls, credentials, session) else {
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
    // ⚠️ **`0` is KIP-368's "this session never expires", and a wrong value
    // here is protocol-visible in a way no other test would notice.** This
    // shipped as `i64::MAX` on the belief that `0` meant "expires
    // immediately"; `M4.18` measured the truth the first time anything
    // actually spoke `SASL/PLAIN` to this broker — librdkafka's reauth
    // deadline overflows, it re-authenticates at once, and
    // `Session::authenticate` is first-wins, so the connection is torn down
    // and rebuilt in a loop. ⚠️ **Pinned here because the only other thing
    // that catches a revert is `scripts/harness/tls_sasl.py`**, which is not
    // a pre-commit gate and skips itself without `openssl`. Found by review.
    assert_eq!(
        response.session_lifetime_ms, 0,
        "0 means no expiry (KIP-368); i64::MAX makes a real client \
         re-authenticate immediately and lose the connection"
    );
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
        handle(prelude(), &[0xFF, 0xFF], true, &creds, &Session::default()),
        HandlerResponse::Close
    ));
}

/// ⚠️ **`M9.7`'s own dependency**: a successful exchange is the one place a
/// `Principal` is ever attached to a connection — the authorization decision
/// point has nothing to read if this does not happen.
#[test]
fn a_successful_exchange_attaches_the_principal_to_the_session() {
    let creds = one_credential("alice", "secret");
    let session = Session::default();
    assert_eq!(session.principal(), None, "nothing authenticated yet");

    let response = replied_on(
        &encoded(plain_bytes("alice", "secret")),
        true,
        &creds,
        &session,
    );

    assert_eq!(response.error_code, 0);
    assert_eq!(
        session.principal(),
        Some(Principal::new("alice").expect("valid"))
    );
}

/// A refused exchange — wrong password, this time — leaves the session
/// exactly as unauthenticated as it started: a failed attempt must not
/// silently attach an identity nobody proved.
#[test]
fn a_refused_exchange_does_not_attach_a_principal() {
    let creds = one_credential("alice", "secret");
    let session = Session::default();

    let response = replied_on(
        &encoded(plain_bytes("alice", "wrong")),
        true,
        &creds,
        &session,
    );

    assert_eq!(response.error_code, error_codes::SASL_AUTHENTICATION_FAILED);
    assert_eq!(session.principal(), None);
}

/// ⚠️ **`Session::authenticate`'s own guarantee, exercised at the one place
/// that could break it.** A second, otherwise-correct exchange for a
/// *different* principal on an already-authenticated connection is refused,
/// not honoured — `ADR-0032` carries no re-authentication story, so a
/// connection's identity is fixed the moment the first exchange succeeds.
#[test]
fn a_second_exchange_on_an_authenticated_session_is_refused_and_does_not_replace_the_principal() {
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
    let session = Session::default();

    let first = replied_on(
        &encoded(plain_bytes("alice", "secret1")),
        true,
        &creds,
        &session,
    );
    assert_eq!(first.error_code, 0, "the first exchange succeeds");
    assert_eq!(
        session.principal(),
        Some(Principal::new("alice").expect("valid"))
    );

    let second = replied_on(
        &encoded(plain_bytes("bob", "secret2")),
        true,
        &creds,
        &session,
    );
    assert_eq!(
        second.error_code,
        error_codes::SASL_AUTHENTICATION_FAILED,
        "a correct, but second, credential is still refused"
    );
    assert_eq!(
        session.principal(),
        Some(Principal::new("alice").expect("valid")),
        "the original principal survives the refused second exchange"
    );
}
