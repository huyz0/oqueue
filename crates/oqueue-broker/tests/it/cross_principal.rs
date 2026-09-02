//! FR-40: cross-principal refusal, tested on **every** API this broker
//! scopes by topic — not a representative sample. `M9.9`-`M9.12` built the
//! per-handler unit tests that prove each mechanism works in isolation;
//! this is the one place that sweeps every one of them together, through
//! the real [`Dispatcher`], the composition-root shape `matrix.rs`'s own
//! FR-2 precedent already established — applied here to FR-40, which is
//! `m9-complete.sh` (`M9.18`)'s own acceptance evidence.
//!
//! ⚠️ **Two shapes for `Metadata`, not one standing in for the other**
//! (`M9.1`'s verified Kafka finding, `M9.9`/`M9.10`'s own split): an
//! explicitly-named unauthorized topic answers `TOPIC_AUTHORIZATION_FAILED`;
//! a null-topic-array request silently omits it instead of naming it at
//! all. Both are swept here, not one taken as a stand-in for the other.
//!
//! ⚠️ **Two real connections, not one principal switched mid-test.** `M9.7`'s
//! own guarantee is that one connection authenticates at most once — a
//! second `SaslAuthenticate` on the same session is refused, not honoured
//! (`sasl_authenticate/tests.rs`'s own coverage) — so alice and bob are two
//! separate [`Dispatcher`]s sharing one [`Cluster`], each real evidence that
//! its own principal's topic answers and the other's does not.

#![allow(clippy::expect_used)]

use crate::support::{broker, golden_batch};
use kafka_protocol::messages::fetch_request::{FetchPartition, FetchTopic};
use kafka_protocol::messages::list_offsets_request::{ListOffsetsPartition, ListOffsetsTopic};
use kafka_protocol::messages::metadata_request::MetadataRequestTopic;
use kafka_protocol::messages::produce_request::{PartitionProduceData, TopicProduceData};
use kafka_protocol::messages::{
    FetchRequest, FetchResponse, ListOffsetsRequest, ListOffsetsResponse, MetadataRequest,
    MetadataResponse, ProduceRequest, ProduceResponse, RequestHeader, SaslAuthenticateRequest,
    TopicName,
};
use kafka_protocol::protocol::{Decodable, Encodable, StrBytes};
use oqueue_broker::{Dispatcher, Handler, HandlerResponse, PlainCredential, PlainCredentials};
use oqueue_codec::apikey::ApiKey;
use oqueue_core::{Principal, Redacted, TopicGrants, TopicId};
use std::sync::Arc;

/// A full request frame: header at the API's own header version, then
/// `body` — `matrix.rs`'s own helper, the same shape.
fn framed(api_key: ApiKey, version: i16, body: &[u8]) -> Vec<u8> {
    let mut frame = Vec::new();
    let mut header = RequestHeader::default();
    header.request_api_key = api_key.as_i16();
    header.request_api_version = version;
    header.correlation_id = 42;
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

fn metadata_explicit_frame(topic: &str) -> Vec<u8> {
    let mut request = MetadataRequest::default();
    let mut entry = MetadataRequestTopic::default();
    entry.name = Some(TopicName(StrBytes::from_string(topic.to_owned())));
    request.topics = Some(vec![entry]);
    let mut body = Vec::new();
    request.encode(&mut body, 12).expect("encodes");
    framed(ApiKey::Metadata, 12, &body)
}

fn metadata_all_frame() -> Vec<u8> {
    let mut request = MetadataRequest::default();
    // ⚠️ **Null, not the dependency's own default.** `MetadataRequest::default()`'s
    // `topics` is `Some(vec![])` — from v1 that means "no topics", the
    // opposite sentinel (`metadata.rs`'s own test names the same trap). The
    // null-array "every topic" case has to be set explicitly.
    request.topics = None;
    let mut body = Vec::new();
    request.encode(&mut body, 12).expect("encodes");
    framed(ApiKey::Metadata, 12, &body)
}

fn produce_frame(topic: &str) -> Vec<u8> {
    let mut request = ProduceRequest::default();
    request.acks = -1;
    let mut t = TopicProduceData::default();
    t.name = TopicName(StrBytes::from_string(topic.to_owned()));
    let mut p = PartitionProduceData::default();
    p.index = 0;
    p.records = Some(bytes::Bytes::from(golden_batch(&[b"cross-principal"])));
    t.partition_data.push(p);
    request.topic_data.push(t);
    let mut body = Vec::new();
    request.encode(&mut body, 9).expect("encodes");
    framed(ApiKey::Produce, 9, &body)
}

fn fetch_frame(topic: &str) -> Vec<u8> {
    let mut request = FetchRequest::default();
    request.max_wait_ms = 0;
    let mut t = FetchTopic::default();
    t.topic = TopicName(StrBytes::from_string(topic.to_owned()));
    let mut p = FetchPartition::default();
    p.partition = 0;
    p.partition_max_bytes = 1 << 20;
    t.partitions.push(p);
    request.topics.push(t);
    let mut body = Vec::new();
    request.encode(&mut body, 11).expect("encodes");
    framed(ApiKey::Fetch, 11, &body)
}

fn list_offsets_frame(topic: &str) -> Vec<u8> {
    let mut request = ListOffsetsRequest::default();
    request.replica_id = kafka_protocol::messages::BrokerId(-1);
    let mut t = ListOffsetsTopic::default();
    t.name = TopicName(StrBytes::from_string(topic.to_owned()));
    let mut p = ListOffsetsPartition::default();
    p.partition_index = 0;
    p.timestamp = -1; // LATEST
    t.partitions.push(p);
    request.topics.push(t);
    let mut body = Vec::new();
    request.encode(&mut body, 9).expect("encodes");
    framed(ApiKey::ListOffsets, 9, &body)
}

async fn replied(dispatcher: &Dispatcher, frame: Vec<u8>) -> Vec<u8> {
    match dispatcher.handle(frame).await {
        HandlerResponse::Reply(out) => out,
        other => panic!("expected a reply, got {other:?}"),
    }
}

fn topic_error_code_metadata(out: &[u8]) -> i16 {
    let mut rest = &out[5..];
    let response = MetadataResponse::decode(&mut rest, 12).expect("decodes");
    response.topics[0].error_code
}

fn metadata_topic_names(out: &[u8]) -> Vec<String> {
    let mut rest = &out[5..];
    let response = MetadataResponse::decode(&mut rest, 12).expect("decodes");
    response
        .topics
        .iter()
        .map(|t| t.name.as_ref().expect("named").to_string())
        .collect()
}

fn topic_error_code_produce(out: &[u8]) -> i16 {
    let mut rest = &out[5..];
    let response = ProduceResponse::decode(&mut rest, 9).expect("decodes");
    response.responses[0].partition_responses[0].error_code
}

fn topic_error_code_fetch(out: &[u8]) -> i16 {
    // v11 is not yet flexible: a plain i32 correlation id header.
    let mut rest = &out[4..];
    let response = FetchResponse::decode(&mut rest, 11).expect("decodes");
    response.responses[0].partitions[0].error_code
}

fn topic_error_code_list_offsets(out: &[u8]) -> i16 {
    let mut rest = &out[5..];
    let response = ListOffsetsResponse::decode(&mut rest, 9).expect("decodes");
    response.topics[0].partitions[0].error_code
}

/// Every scoped API's own answer for `topic`, on `dispatcher`'s
/// already-authenticated connection: `TOPIC_AUTHORIZATION_FAILED` on
/// refusal, `NONE` (0) on success — the explicit-name shape every one of
/// these four handlers shares.
async fn explicit_error_codes(dispatcher: &Dispatcher, topic: &str) -> Vec<(&'static str, i16)> {
    vec![
        (
            "Metadata (explicit)",
            topic_error_code_metadata(&replied(dispatcher, metadata_explicit_frame(topic)).await),
        ),
        (
            "Produce",
            topic_error_code_produce(&replied(dispatcher, produce_frame(topic)).await),
        ),
        (
            "Fetch",
            topic_error_code_fetch(&replied(dispatcher, fetch_frame(topic)).await),
        ),
        (
            "ListOffsets",
            topic_error_code_list_offsets(&replied(dispatcher, list_offsets_frame(topic)).await),
        ),
    ]
}

fn one_credential(name: &str, password: &str) -> PlainCredential {
    PlainCredential {
        principal: Principal::new(name).expect("valid"),
        password: Redacted::new(password.to_owned()),
    }
}

/// A broker hosting `alice-topic`/`bob-topic`, and two already-authenticated
/// `Dispatcher`s — one per principal, `M9.7`'s own "at most once per
/// connection" guarantee is why this is two, not one switched mid-test.
///
/// ⚠️ **The `Broker` rides along, not dropped here.** Its own `Drop` aborts
/// the coordinator loop both dispatchers depend on — split out of the test
/// itself purely to keep that function under the fifty-line limit, not to
/// let the fixture go out of scope early.
async fn authenticated_dispatchers() -> (crate::support::Broker, Dispatcher, Dispatcher) {
    let b = broker(&["alice-topic", "bob-topic"]).await;
    let credentials = PlainCredentials::new(vec![
        one_credential("alice", "alice-secret"),
        one_credential("bob", "bob-secret"),
    ]);
    let mut grants = TopicGrants::new();
    grants.grant(
        Principal::new("alice").expect("valid"),
        TopicId::new("alice-topic").expect("valid"),
    );
    grants.grant(
        Principal::new("bob").expect("valid"),
        TopicId::new("bob-topic").expect("valid"),
    );

    let alice = Dispatcher::new(Arc::clone(&b.cluster))
        .tls_terminated()
        .with_credentials(credentials.clone())
        .with_topic_grants(grants.clone());
    let auth = alice
        .handle(sasl_authenticate_frame("alice", "alice-secret"))
        .await;
    assert!(
        matches!(auth, HandlerResponse::Reply(_)),
        "alice authenticates"
    );

    let bob = Dispatcher::new(Arc::clone(&b.cluster))
        .tls_terminated()
        .with_credentials(credentials)
        .with_topic_grants(grants);
    let auth = bob
        .handle(sasl_authenticate_frame("bob", "bob-secret"))
        .await;
    assert!(
        matches!(auth, HandlerResponse::Reply(_)),
        "bob authenticates"
    );

    (b, alice, bob)
}

#[tokio::test]
async fn every_scoped_api_refuses_a_cross_principal_topic() {
    let (_broker, alice, bob) = authenticated_dispatchers().await;

    // ── Every scoped API, explicit-name shape ───────────────────────────
    for (api, code) in explicit_error_codes(&alice, "alice-topic").await {
        assert_eq!(code, 0, "{api}: alice's own topic must answer");
    }
    for (api, code) in explicit_error_codes(&alice, "bob-topic").await {
        assert_eq!(
            code,
            oqueue_codec::error_codes::TOPIC_AUTHORIZATION_FAILED,
            "{api}: alice must be refused bob's topic"
        );
    }
    for (api, code) in explicit_error_codes(&bob, "bob-topic").await {
        assert_eq!(code, 0, "{api}: bob's own topic must answer");
    }
    for (api, code) in explicit_error_codes(&bob, "alice-topic").await {
        assert_eq!(
            code,
            oqueue_codec::error_codes::TOPIC_AUTHORIZATION_FAILED,
            "{api}: bob must be refused alice's topic"
        );
    }

    // ── `Metadata`'s other shape: the null-topic-array case ─────────────
    // ⚠️ Silent omission, not an error entry — `M9.1`'s verified Kafka
    // finding, `M9.10`'s own implementation. Each principal's "every topic
    // I can see" answer must name its own topic and never the other's.
    let alice_all = metadata_topic_names(&replied(&alice, metadata_all_frame()).await);
    assert_eq!(alice_all, vec!["alice-topic".to_string()]);
    let bob_all = metadata_topic_names(&replied(&bob, metadata_all_frame()).await);
    assert_eq!(bob_all, vec!["bob-topic".to_string()]);
}
