//! FR-44's second net: the log scan `M9.14`'s static type rule cannot be
//! (`security.md` rule 6, `M9.15`).
//!
//! ⚠️ **"Every node's log," scoped to what this suite can actually observe
//! a node produce.** `security.md` rule 6 says the end-to-end suite
//! "searches every node's log for known values" — but no crate in this
//! workspace calls `tracing::info!`/`event!` anywhere, and nothing
//! initializes a subscriber (`bin/oqueue/src/main.rs`'s own comment:
//! "nothing initialises it yet", `run`'s `println!` is deliberately *before*
//! that point). Wiring a subscriber up is a real project of its own — what
//! to instrument, what a production log line should say — and `M12` names
//! "distributed tracing across the produce and fetch paths" as its own
//! scope, not this task's to invent as a byproduct of a test. Until a
//! process log genuinely exists, this test scans every text surface this
//! suite *can* reach a node produce from: the raw wire reply bytes, and the
//! `Debug` rendering of [`Dispatcher`] itself — which nests both `Session`
//! (`M9.2`'s authenticated [`Principal`]) and `PlainCredentials` (`M9.4`'s
//! password) in the one value the milestone names them together in. ⚠️
//! **Widen this the day something in this workspace emits a real log** —
//! this file's own docstring is the honest marker for when that has
//! happened and this scan is due to grow to match, the same discipline
//! `M9.md`'s own "FR-44 has a gate that cannot be complete" already names.

#![allow(clippy::expect_used)]

use crate::support::broker;
use kafka_protocol::messages::{RequestHeader, SaslAuthenticateRequest};
use kafka_protocol::protocol::Encodable;
use oqueue_broker::{Dispatcher, Handler, HandlerResponse, PlainCredential, PlainCredentials};
use oqueue_codec::apikey::ApiKey;
use oqueue_core::{Principal, Redacted};
use std::sync::Arc;

/// Distinctive enough that its appearance anywhere in this test's captured
/// text could only be the real password leaking, never a coincidence.
const CANARY: &str = "m9.15-canary-hunter7-do-not-log-me";

fn framed(api_key: ApiKey, version: i16, body: &[u8]) -> Vec<u8> {
    let mut frame = Vec::new();
    let mut header = RequestHeader::default();
    header.request_api_key = api_key.as_i16();
    header.request_api_version = version;
    header.correlation_id = 7;
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

/// Every text surface this suite can currently observe a node produce for
/// one exchange: the raw reply bytes, lossily decoded, and `dispatcher`'s
/// own `Debug` — which reaches both the credential that was checked and the
/// principal a success would have attached to the session, in one value.
fn observed_text(dispatcher: &Dispatcher, reply: &[u8]) -> String {
    format!("{}\n{dispatcher:?}", String::from_utf8_lossy(reply))
}

async fn reply(dispatcher: &Dispatcher, frame: Vec<u8>) -> Vec<u8> {
    match dispatcher.handle(frame).await {
        HandlerResponse::Reply(out) => out,
        other => panic!("expected a reply, got {other:?}"),
    }
}

#[tokio::test]
async fn sasl_plain_exchange_never_leaks_the_password() {
    let b = broker(&["some-topic"]).await;
    let credentials = PlainCredentials::new(vec![PlainCredential {
        principal: Principal::new("alice").expect("valid"),
        password: Redacted::new(CANARY.to_owned()),
    }]);

    // ── The correct password: the success path, which attaches a real
    //    `Principal` (`M9.2`) to the session right where a naive log line
    //    would be tempted to also print what unlocked it ──────────────────
    let succeeding = Dispatcher::new(Arc::clone(&b.cluster))
        .tls_terminated()
        .with_credentials(credentials.clone());
    let out = reply(&succeeding, sasl_authenticate_frame("alice", CANARY)).await;
    let text = observed_text(&succeeding, &out);
    assert!(
        !text.contains(CANARY),
        "the canary password leaked through the success path: {text}"
    );

    // ── A wrong password: the refusal path, the one carrying the real
    //    bytes furthest through `parse_plain`/`verify` before being
    //    discarded — `check-secrets.sh`'s static rule cannot see this far,
    //    since nothing here is a struct field ─────────────────────────────
    let refusing = Dispatcher::new(Arc::clone(&b.cluster))
        .tls_terminated()
        .with_credentials(credentials);
    let out = reply(
        &refusing,
        sasl_authenticate_frame("alice", &format!("{CANARY}-wrong")),
    )
    .await;
    let text = observed_text(&refusing, &out);
    assert!(
        !text.contains(CANARY),
        "the canary password leaked through the refusal path: {text}"
    );
}
