#![allow(clippy::expect_used)]

use super::handle;
use crate::authz::AuthzContext;
use crate::connection::HandlerResponse;
use crate::testing::fixture;
use kafka_protocol::messages::OffsetFetchRequest as KpRequest;
use kafka_protocol::messages::OffsetFetchResponse as KpResponse;
use kafka_protocol::messages::offset_fetch_request::OffsetFetchRequestTopic as KpTopic;
use kafka_protocol::protocol::{Decodable, Encodable, StrBytes};
use oqueue_codec::frame::RequestPrelude;
use oqueue_core::{Principal, TopicGrants};

const VERSION: i16 = 6;

fn prelude() -> RequestPrelude {
    RequestPrelude {
        api_key: 9,
        api_version: VERSION,
        correlation_id: 1,
    }
}

static EMPTY_GRANTS: std::sync::LazyLock<TopicGrants> = std::sync::LazyLock::new(TopicGrants::new);

/// Credentials off: every fetch fails open, `offset_commit/tests.rs`'s own
/// default `AuthzContext` shape.
fn open() -> AuthzContext<'static> {
    AuthzContext {
        principal: None,
        credentials_configured: false,
        topic_grants: &EMPTY_GRANTS,
    }
}

fn explicit_body(group: &str, topics: &[(&str, &[i32])]) -> Vec<u8> {
    let kp_topics = topics
        .iter()
        .map(|&(name, partitions)| {
            let mut t = KpTopic::default();
            t.name = kafka_protocol::messages::TopicName(StrBytes::from_string(name.to_owned()));
            t.partition_indexes = partitions.to_vec();
            t
        })
        .collect();
    let request = KpRequest::default()
        .with_group_id(kafka_protocol::messages::GroupId(StrBytes::from_string(
            group.to_owned(),
        )))
        .with_topics(Some(kp_topics));
    let mut out = Vec::new();
    request.encode(&mut out, VERSION).expect("encodes");
    out
}

fn all_topics_body(group: &str) -> Vec<u8> {
    let request = KpRequest::default()
        .with_group_id(kafka_protocol::messages::GroupId(StrBytes::from_string(
            group.to_owned(),
        )))
        .with_topics(None);
    let mut out = Vec::new();
    request.encode(&mut out, VERSION).expect("encodes");
    out
}

fn fetch(cluster: &crate::cluster::Cluster, body: &[u8], authz: &AuthzContext<'_>) -> KpResponse {
    let HandlerResponse::Reply(out) = handle(cluster, prelude(), body, authz) else {
        panic!("an OffsetFetch replies");
    };
    let mut rest = &out[5..]; // v6 is flexible: a 5-byte response header.
    let response = KpResponse::decode(&mut rest, VERSION).expect("decodes");
    assert!(rest.is_empty());
    response
}

/// Seats `member_id` for `group` at `Stable`, unless already seated, and
/// returns the group's own current generation.
///
/// ⚠️ **Once per group, not once per call.** Re-firing
/// `Join`/`JoinBarrierComplete`/`SyncComplete` on an already-`Stable`
/// group is legal (`Stable -> PreparingRebalance` is a real transition)
/// but bumps the generation -- a second call for the same group would
/// otherwise silently move the generation a caller's own next commit must
/// be fenced against out from under it. Found by this file's own first
/// version of `all_topics_returns_every_authorized_topic`: its second
/// commit call was silently refused `ILLEGAL_GENERATION` and never
/// landed.
fn seat_once(cluster: &crate::cluster::Cluster, group: &str, member_id: &str) -> i32 {
    let g = oqueue_core::GroupId::new(group).expect("valid");
    if !cluster.heartbeats().is_tracked(&g, member_id) {
        cluster.heartbeats().register(&g, member_id, 30_000);
        cluster
            .group_coordinator()
            .transition(&g, oqueue_core::GroupEvent::Join)
            .expect("Empty -> Join is legal");
        cluster
            .group_coordinator()
            .transition(&g, oqueue_core::GroupEvent::JoinBarrierComplete)
            .expect("PreparingRebalance -> CompletingRebalance is legal");
        cluster
            .group_coordinator()
            .transition(&g, oqueue_core::GroupEvent::SyncComplete)
            .expect("CompletingRebalance -> Stable is legal");
    }
    cluster
        .group_coordinator()
        .record(&g)
        .expect("seated above")
        .generation
        .get()
}

/// Seats `"m1"` and commits `offset` for `group`/`topic`/`partition`
/// through the real `OffsetCommit` handler — `testing.rs`'s own "the write
/// path is the fixture for the read path" precedent, applied here rather
/// than reaching into `CommittedOffsets`' own internals by hand.
async fn commit(
    cluster: &crate::cluster::Cluster,
    group: &str,
    topic: &str,
    partition: i32,
    offset: i64,
) {
    use kafka_protocol::messages::OffsetCommitRequest as KpCommitRequest;
    use kafka_protocol::messages::offset_commit_request::{
        OffsetCommitRequestPartition as KpCommitPartition,
        OffsetCommitRequestTopic as KpCommitTopic,
    };

    let generation = seat_once(cluster, group, "m1");

    let mut p = KpCommitPartition::default();
    p.partition_index = partition;
    p.committed_offset = offset;
    let mut t = KpCommitTopic::default();
    t.name = kafka_protocol::messages::TopicName(StrBytes::from_string(topic.to_owned()));
    t.partitions = vec![p];
    let request = KpCommitRequest::default()
        .with_group_id(kafka_protocol::messages::GroupId(StrBytes::from_string(
            group.to_owned(),
        )))
        .with_generation_id_or_member_epoch(generation)
        .with_member_id(StrBytes::from_static_str("m1"))
        .with_topics(vec![t]);
    let mut body = Vec::new();
    request.encode(&mut body, 8).expect("encodes");
    let commit_prelude = RequestPrelude {
        api_key: 8,
        api_version: 8,
        correlation_id: 1,
    };
    let HandlerResponse::Reply(out) =
        crate::offset_commit::handle(cluster, commit_prelude, &body, &open()).await
    else {
        panic!("an OffsetCommit replies");
    };
    // ⚠️ Checked, not assumed: a silently-refused seed commit (a fencing
    // mismatch, say) would otherwise leave a test asserting against data
    // that was never actually written -- this file's own precedent, above.
    let mut rest = &out[5..];
    let response =
        kafka_protocol::messages::OffsetCommitResponse::decode(&mut rest, 8).expect("decodes");
    assert_eq!(
        response.topics[0].partitions[0].error_code, 0,
        "the fixture's own seed commit must actually land"
    );
}

/// A partition this group has never committed answers `-1`, `NONE` — not
/// a refusal.
#[tokio::test(start_paused = true)]
async fn a_never_committed_partition_answers_unassigned() {
    let fixture = fixture(&["orders"]).await;
    let response = fetch(
        &fixture.cluster,
        &explicit_body("g", &[("orders", &[0])]),
        &open(),
    );
    assert_eq!(response.topics[0].partitions[0].committed_offset, -1);
    assert_eq!(response.topics[0].partitions[0].error_code, 0);
}

/// A real commit is read back exactly, through the explicit-topics form.
#[tokio::test(start_paused = true)]
async fn a_real_commit_is_read_back() {
    let fixture = fixture(&["orders"]).await;
    commit(&fixture.cluster, "g", "orders", 0, 42).await;
    let response = fetch(
        &fixture.cluster,
        &explicit_body("g", &[("orders", &[0])]),
        &open(),
    );
    assert_eq!(response.topics[0].partitions[0].committed_offset, 42);
    assert_eq!(response.topics[0].partitions[0].error_code, 0);
}

/// An explicitly-named topic this principal cannot see is refused
/// `TOPIC_AUTHORIZATION_FAILED` per partition, `Metadata`'s own
/// explicit-name shape (`M9.9`/`M9.10`).
#[tokio::test(start_paused = true)]
async fn an_unauthorized_explicit_topic_is_refused_per_partition() {
    let fixture = fixture(&["bob-topic"]).await;
    commit(&fixture.cluster, "g", "bob-topic", 0, 42).await;

    let mut grants = TopicGrants::new();
    grants.grant(
        Principal::new("bob").expect("valid"),
        oqueue_core::TopicId::new("bob-topic").expect("valid"),
    );
    let alice = Principal::new("alice").expect("valid");
    let authz = AuthzContext {
        principal: Some(&alice),
        credentials_configured: true,
        topic_grants: &grants,
    };

    let response = fetch(
        &fixture.cluster,
        &explicit_body("g", &[("bob-topic", &[0])]),
        &authz,
    );
    assert_eq!(
        response.topics[0].partitions[0].error_code,
        oqueue_codec::error_codes::TOPIC_AUTHORIZATION_FAILED
    );
}

/// ⚠️ **`M4.13`'s own acceptance criterion, verbatim**: an all-topics
/// `OffsetFetch` from principal A never returns an offset committed by
/// principal B in the same group -- silently omitted, not named as
/// refused, `Metadata`'s own null-topic-array precedent (`M9.9`/`M9.10`).
#[tokio::test(start_paused = true)]
async fn all_topics_never_returns_a_topic_committed_by_another_principal() {
    let fixture = fixture(&["alice-topic", "bob-topic"]).await;
    commit(&fixture.cluster, "g", "alice-topic", 0, 1).await;
    commit(&fixture.cluster, "g", "bob-topic", 0, 2).await;

    let mut grants = TopicGrants::new();
    grants.grant(
        Principal::new("alice").expect("valid"),
        oqueue_core::TopicId::new("alice-topic").expect("valid"),
    );
    grants.grant(
        Principal::new("bob").expect("valid"),
        oqueue_core::TopicId::new("bob-topic").expect("valid"),
    );
    let alice = Principal::new("alice").expect("valid");
    let authz = AuthzContext {
        principal: Some(&alice),
        credentials_configured: true,
        topic_grants: &grants,
    };

    let response = fetch(&fixture.cluster, &all_topics_body("g"), &authz);
    let names: Vec<&str> = response.topics.iter().map(|t| t.name.as_str()).collect();
    assert_eq!(
        names,
        vec!["alice-topic"],
        "bob's own topic must never appear, not even refused"
    );
}

/// The all-topics form returns every topic this principal *can* see.
#[tokio::test(start_paused = true)]
async fn all_topics_returns_every_authorized_topic() {
    let fixture = fixture(&["orders", "payments"]).await;
    commit(&fixture.cluster, "g", "orders", 0, 10).await;
    commit(&fixture.cluster, "g", "payments", 0, 20).await;

    let response = fetch(&fixture.cluster, &all_topics_body("g"), &open());
    let mut names: Vec<&str> = response.topics.iter().map(|t| t.name.as_str()).collect();
    names.sort_unstable();
    assert_eq!(names, vec!["orders", "payments"]);
}

/// An explicitly-named topic that fails to construct (the empty string,
/// `TopicId::new`'s only rejection) is refused `UNKNOWN_TOPIC_OR_PARTITION`
/// -- `offset_commit.rs`'s own `commit_topic` precedent for the identical
/// input, not a silent `-1` indistinguishable from a genuinely
/// uncommitted partition. Round-1 review's own finding.
#[tokio::test(start_paused = true)]
async fn an_unconstructible_explicit_topic_name_is_refused_unknown_topic_or_partition() {
    let fixture = fixture(&[]).await;
    let response = fetch(
        &fixture.cluster,
        &explicit_body("g", &[("", &[0])]),
        &open(),
    );
    assert_eq!(
        response.topics[0].partitions[0].error_code,
        oqueue_codec::error_codes::UNKNOWN_TOPIC_OR_PARTITION
    );
    assert_eq!(response.topics[0].partitions[0].committed_offset, -1);
}

/// A malformed `group_id` is a group-level refusal: no topics, just the
/// top-level code.
#[tokio::test(start_paused = true)]
async fn a_malformed_group_id_is_a_group_level_refusal() {
    let fixture = fixture(&[]).await;
    let request = KpRequest::default()
        .with_group_id(kafka_protocol::messages::GroupId(
            StrBytes::from_static_str(""),
        ))
        .with_topics(None);
    let mut body = Vec::new();
    request.encode(&mut body, VERSION).expect("encodes");
    let response = fetch(&fixture.cluster, &body, &open());
    assert_eq!(
        response.error_code,
        oqueue_codec::error_codes::INVALID_REQUEST
    );
    assert!(response.topics.is_empty());
}

/// A malformed body closes the connection rather than answering — every
/// other handler's own policy for a frame this broker cannot decode.
#[tokio::test(start_paused = true)]
async fn a_malformed_body_closes_rather_than_panicking() {
    let fixture = fixture(&[]).await;
    let response = handle(&fixture.cluster, prelude(), &[0xFF; 3], &open());
    assert!(matches!(response, HandlerResponse::Close));
}
