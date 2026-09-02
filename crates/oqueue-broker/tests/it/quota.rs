//! `security.md` rule 13, FR-45: the in-flight bound is per principal, wired
//! through the real [`Dispatcher`] (`M9.16`).
//!
//! ⚠️ **The wiring, not the counting.** `oqueue-core`'s own
//! `principal_quota` unit tests already prove the count itself — admits up
//! to the bound, refuses over it, releases on drop, one principal's own
//! count independent of another's. What only a `Dispatcher`-level test can
//! show is that `dispatch`'s new check actually consults `self.quota` for a
//! real request and actually keys it by the connection's own authenticated
//! principal — `cross_principal.rs`'s own precedent for what "sweep it
//! through the real thing, not a synthetic call" looks like, applied to a
//! quota instead of a topic grant.

#![allow(clippy::expect_used)]

use crate::support::broker;
use kafka_protocol::messages::{MetadataRequest, RequestHeader, SaslAuthenticateRequest};
use kafka_protocol::protocol::Encodable;
use oqueue_broker::{Dispatcher, Handler, HandlerResponse, PlainCredential, PlainCredentials};
use oqueue_codec::apikey::ApiKey;
use oqueue_core::{Principal, PrincipalQuota, Redacted};
use std::sync::Arc;

fn framed(api_key: ApiKey, version: i16, body: &[u8]) -> Vec<u8> {
    let mut frame = Vec::new();
    let mut header = RequestHeader::default();
    header.request_api_key = api_key.as_i16();
    header.request_api_version = version;
    header.correlation_id = 1;
    header
        .encode(&mut frame, api_key.request_header_version(version))
        .expect("header encodes");
    frame.extend_from_slice(body);
    frame
}

fn sasl_authenticate_frame(authcid: &str, password: &str) -> Vec<u8> {
    let mut auth_bytes = vec![0u8];
    auth_bytes.extend_from_slice(authcid.as_bytes());
    auth_bytes.push(0);
    auth_bytes.extend_from_slice(password.as_bytes());
    let mut body = Vec::new();
    SaslAuthenticateRequest::default()
        .with_auth_bytes(bytes::Bytes::from(auth_bytes))
        .encode(&mut body, 1)
        .expect("encodes");
    framed(ApiKey::SaslAuthenticate, 1, &body)
}

fn metadata_all_frame() -> Vec<u8> {
    let mut request = MetadataRequest::default();
    // Null, not the dependency's own `Some(vec![])` default — "every topic
    // this principal can see" (`M9.9`/`M9.10`), answerable with zero grants.
    request.topics = None;
    let mut body = Vec::new();
    request.encode(&mut body, 12).expect("encodes");
    framed(ApiKey::Metadata, 12, &body)
}

fn one_credential(name: &str, password: &str) -> PlainCredential {
    PlainCredential {
        principal: Principal::new(name).expect("valid"),
        password: Redacted::new(password.to_owned()),
    }
}

async fn authenticate(dispatcher: &Dispatcher, authcid: &str, password: &str) {
    let reply = dispatcher
        .handle(sasl_authenticate_frame(authcid, password))
        .await;
    assert!(
        matches!(reply, HandlerResponse::Reply(_)),
        "{authcid} authenticates: {reply:?}"
    );
}

#[tokio::test]
async fn a_principal_over_its_bound_is_refused_while_another_is_unaffected() {
    let b = broker(&["some-topic"]).await;
    let credentials = PlainCredentials::new(vec![
        one_credential("alice", "alice-secret"),
        one_credential("bob", "bob-secret"),
    ]);
    // One shared `Arc`, not two independent quotas — `Dispatcher::with_quota`'s
    // own doc names this as the load-bearing part: a fresh quota per
    // dispatcher would bound nothing real.
    let quota = Arc::new(PrincipalQuota::new(1));

    let alice = Dispatcher::new(Arc::clone(&b.cluster))
        .tls_terminated()
        .with_credentials(credentials.clone())
        .with_quota(Arc::clone(&quota));
    authenticate(&alice, "alice", "alice-secret").await;

    let bob = Dispatcher::new(Arc::clone(&b.cluster))
        .tls_terminated()
        .with_credentials(credentials)
        .with_quota(Arc::clone(&quota));
    authenticate(&bob, "bob", "bob-secret").await;

    // Occupy alice's one slot directly on the shared quota — standing in
    // for a second request of alice's own already running concurrently,
    // the real-world case `metadata_handle`'s own synchronous completion
    // cannot reproduce through `Dispatcher::handle` alone within one test.
    let alice_principal = Principal::new("alice").expect("valid");
    let _alice_in_flight =
        PrincipalQuota::admit(&quota, &alice_principal).expect("alice's first slot is free");

    let refused = alice.handle(metadata_all_frame()).await;
    assert!(
        matches!(refused, HandlerResponse::Close),
        "alice is over her own bound of 1: {refused:?}"
    );

    let ok = bob.handle(metadata_all_frame()).await;
    assert!(
        matches!(ok, HandlerResponse::Reply(_)),
        "bob's own count starts at zero, independent of alice's: {ok:?}"
    );
}

#[tokio::test]
async fn no_quota_configured_bounds_nothing() {
    let b = broker(&["some-topic"]).await;
    let credentials = PlainCredentials::new(vec![one_credential("alice", "alice-secret")]);
    // `Dispatcher::new`'s own default: no `.with_quota(...)` call at all —
    // every one of this workspace's existing call sites, unchanged.
    let alice = Dispatcher::new(Arc::clone(&b.cluster))
        .tls_terminated()
        .with_credentials(credentials);
    authenticate(&alice, "alice", "alice-secret").await;

    for _ in 0..5 {
        let reply = alice.handle(metadata_all_frame()).await;
        assert!(
            matches!(reply, HandlerResponse::Reply(_)),
            "no quota configured must never refuse: {reply:?}"
        );
    }
}
