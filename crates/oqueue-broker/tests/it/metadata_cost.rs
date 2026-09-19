//! NFR-12: a `Metadata` response's size and CPU cost are O(topics this
//! principal can see), never O(topics that exist) (`M9.17`).
//!
//! ⚠️ **Two catalog sizes an order of magnitude apart** — `M9.md`'s own
//! completion-condition wording, and the reason one size is not evidence: a
//! single catalog cannot distinguish O(1) from O(n) with a large constant.
//! The response has to come out identical at both sizes, not merely small
//! at one of them.
//!
//! ⚠️ **"CPU," without a stopwatch.** `testing.md` rule 11 forbids asserting
//! on wall-clock duration. [`oqueue_broker::Cluster::topic_lookups`]
//! (`M9.17`) is the proxy instead: every per-topic lookup the null-array
//! `Metadata` path makes is counted, and `metadata.rs`'s own
//! `all_topics_names` (`M9.10`) never touches `Cluster::topic_names`
//! (`O(catalog)`) once a principal is granted a scoped set — so the delta
//! this test reads around one request is bounded by the principal's own
//! grant count, never by how many topics exist.

#![allow(clippy::expect_used)]

use crate::support::{Broker, broker};
use kafka_protocol::messages::{
    MetadataRequest, MetadataResponse, RequestHeader, SaslAuthenticateRequest,
};
use kafka_protocol::protocol::{Decodable, Encodable};
use oqueue_broker::{Dispatcher, Handler, HandlerResponse, PlainCredential, PlainCredentials};
use oqueue_codec::apikey::ApiKey;
use oqueue_core::{Principal, Redacted, TopicGrants, TopicId};
use std::sync::Arc;

fn framed(api_key: ApiKey, version: i16, body: &[u8]) -> Vec<u8> {
    let mut frame = Vec::new();
    let mut header = RequestHeader::default();
    header.request_api_key = api_key.as_i16();
    header.request_api_version = version;
    header.correlation_id = 9;
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
    // this principal can see" (`M9.9`/`M9.10`).
    request.topics = None;
    let mut body = Vec::new();
    request.encode(&mut body, 12).expect("encodes");
    framed(ApiKey::Metadata, 12, &body)
}

/// Every resolved topic in a `Metadata` response, sorted by name — order
/// independent, because [`oqueue_core::TopicGrants::topics_for`] iterates a
/// `HashSet` whose own order carries no guarantee and is not itself part of
/// what NFR-12 claims.
fn resolved_topics(out: &[u8]) -> Vec<(String, [u8; 16], i16, usize)> {
    let mut rest = &out[5..];
    let response = MetadataResponse::decode(&mut rest, 12).expect("decodes");
    let mut topics: Vec<(String, [u8; 16], i16, usize)> = response
        .topics
        .iter()
        .map(|t| {
            (
                t.name.as_ref().expect("named").to_string(),
                *t.topic_id.as_bytes(),
                t.error_code,
                t.partitions.len(),
            )
        })
        .collect();
    topics.sort_by(|a, b| a.0.cmp(&b.0));
    topics
}

async fn reply(dispatcher: &Dispatcher, frame: Vec<u8>) -> Vec<u8> {
    match dispatcher.handle(frame).await {
        HandlerResponse::Reply(out) => out,
        other => panic!("expected a reply, got {other:?}"),
    }
}

/// A broker whose catalog has `catalog_size` topics, and one principal
/// (`alice`) authenticated and granted exactly the first `granted` of
/// them — deterministic naming and creation order, so two calls with the
/// same `granted` but different `catalog_size` grant the *identical* set
/// of topics, carrying the identical ids (derived from the name,
/// `oqueue_core::topic_uuid`).
async fn scoped_broker(catalog_size: usize, granted: usize) -> (Broker, Dispatcher) {
    let names: Vec<String> = (0..catalog_size).map(|n| format!("topic-{n:05}")).collect();
    let refs: Vec<&str> = names.iter().map(String::as_str).collect();
    let b = broker(&refs).await;

    let alice = Principal::new("alice").expect("valid");
    let mut grants = TopicGrants::new();
    for name in &names[..granted] {
        grants.grant(alice.clone(), TopicId::new(name).expect("valid"));
    }
    let credentials = PlainCredentials::new(vec![PlainCredential {
        principal: alice,
        password: Redacted::new("alice-secret".to_owned()),
    }]);

    let dispatcher = Dispatcher::new(Arc::clone(&b.cluster))
        .tls_terminated()
        .with_credentials(credentials)
        .with_topic_grants(grants);
    let auth = dispatcher
        .handle(sasl_authenticate_frame("alice", "alice-secret"))
        .await;
    assert!(
        matches!(auth, HandlerResponse::Reply(_)),
        "alice authenticates"
    );

    (b, dispatcher)
}

#[tokio::test]
async fn metadata_response_size_and_lookup_cost_do_not_grow_with_catalog_size() {
    let granted = 5;

    let (small, small_dispatcher) = scoped_broker(50, granted).await;
    let before = small.cluster.topic_lookups();
    let small_reply = reply(&small_dispatcher, metadata_all_frame()).await;
    let small_lookups = small.cluster.topic_lookups() - before;

    // An order of magnitude apart — `M9.md`'s own completion-condition
    // wording.
    let (large, large_dispatcher) = scoped_broker(500, granted).await;
    let before = large.cluster.topic_lookups();
    let large_reply = reply(&large_dispatcher, metadata_all_frame()).await;
    let large_lookups = large.cluster.topic_lookups() - before;

    // Same byte length -- the direct NFR-12 claim -- and, order aside, the
    // identical five topics with the identical ids: ids derive from the name
    // (`oqueue_core::topic_uuid`), so the same first five names in both
    // catalogs carry the same ids, so the content is not just the same
    // size, it is the same response.
    assert_eq!(
        small_reply.len(),
        large_reply.len(),
        "response size must not grow with catalog size"
    );
    assert_eq!(
        resolved_topics(&small_reply),
        resolved_topics(&large_reply),
        "a 50-topic and a 500-topic catalog must resolve the same granted topics identically"
    );
    assert_eq!(
        small_lookups, large_lookups,
        "lookup cost (the CPU proxy) must not grow with catalog size: {small_lookups} vs {large_lookups}"
    );
    // `all_topics_names` does one filtering lookup per granted name, then
    // `resolve_topic` does two more per surviving one (`exists`, then the
    // partition count itself) -- three lookups per granted topic, never a
    // function of the catalog `metadata.rs`'s own doc comments name.
    assert_eq!(
        large_lookups,
        3 * u64::try_from(granted).expect("granted fits u64"),
        "lookup cost must be exactly 3 per granted topic ({granted}), not scaled by the 500-topic catalog"
    );
}
