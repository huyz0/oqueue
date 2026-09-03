#![allow(clippy::expect_used)]

use super::handle;
use crate::authz::AuthzContext;
use crate::connection::HandlerResponse;
use crate::testing::fixture;
use kafka_protocol::messages::OffsetCommitRequest as KpRequest;
use kafka_protocol::messages::OffsetCommitResponse as KpResponse;
use kafka_protocol::messages::offset_commit_request::{
    OffsetCommitRequestPartition as KpPartition, OffsetCommitRequestTopic as KpTopic,
};
use kafka_protocol::protocol::{Decodable, Encodable, StrBytes};
use oqueue_codec::frame::RequestPrelude;
use oqueue_core::{Principal, TopicGrants, TopicId};

const VERSION: i16 = 8;

fn prelude() -> RequestPrelude {
    RequestPrelude {
        api_key: 8,
        api_version: VERSION,
        correlation_id: 1,
    }
}

/// Credentials off: every commit fails open, `testing.rs`'s own default
/// `AuthzContext` shape.
fn open() -> AuthzContext<'static> {
    AuthzContext {
        principal: None,
        credentials_configured: false,
        topic_grants: &EMPTY_GRANTS,
    }
}

static EMPTY_GRANTS: std::sync::LazyLock<TopicGrants> = std::sync::LazyLock::new(TopicGrants::new);

fn body(group: &str, generation: i32, member_id: &str, topics: &[(&str, &[i32])]) -> Vec<u8> {
    let kp_topics = topics
        .iter()
        .map(|&(name, partitions)| {
            let mut t = KpTopic::default();
            t.name = kafka_protocol::messages::TopicName(StrBytes::from_string(name.to_owned()));
            t.partitions = partitions
                .iter()
                .map(|&index| {
                    let mut p = KpPartition::default();
                    p.partition_index = index;
                    p.committed_offset = 42;
                    p
                })
                .collect();
            t
        })
        .collect();
    let request = KpRequest::default()
        .with_group_id(kafka_protocol::messages::GroupId(StrBytes::from_string(
            group.to_owned(),
        )))
        .with_generation_id_or_member_epoch(generation)
        .with_member_id(StrBytes::from_string(member_id.to_owned()))
        .with_topics(kp_topics);
    let mut out = Vec::new();
    request.encode(&mut out, VERSION).expect("encodes");
    out
}

fn commit(cluster: &crate::cluster::Cluster, body: &[u8], authz: &AuthzContext<'_>) -> KpResponse {
    let HandlerResponse::Reply(out) = handle(cluster, prelude(), body, authz) else {
        panic!("an OffsetCommit replies");
    };
    let mut rest = &out[5..]; // v8 is flexible: a 5-byte response header.
    let response = KpResponse::decode(&mut rest, VERSION).expect("decodes");
    assert!(rest.is_empty());
    response
}

/// Registers `member_id` with `heartbeat.rs`'s own tracking and drives the
/// coordinator to `Stable` at generation `1` — `sync_group/tests.rs`'s own
/// `seat` helper, the same shape.
fn seat(cluster: &crate::cluster::Cluster, group: &str, member_id: &str) {
    let g = oqueue_core::GroupId::new(group).expect("valid");
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

/// ⚠️ **`M4.11`'s own fencing seam, reused rather than reinvented** — a
/// member this broker never tracked is refused `UNKNOWN_MEMBER_ID`, echoed
/// on every named topic/partition (the wire has no top-level error field
/// of its own to answer with instead).
#[tokio::test(start_paused = true)]
async fn an_untracked_member_is_refused_unknown_member_id() {
    let fixture = fixture(&["orders"]).await;
    let response = commit(
        &fixture.cluster,
        &body("g", 1, "ghost", &[("orders", &[0])]),
        &open(),
    );
    assert_eq!(response.topics.len(), 1);
    assert_eq!(
        response.topics[0].partitions[0].error_code,
        oqueue_codec::error_codes::UNKNOWN_MEMBER_ID
    );
}

/// A tracked member naming a stale generation is refused
/// `ILLEGAL_GENERATION`, distinct from `UNKNOWN_MEMBER_ID` — the same
/// distinction `heartbeat/tests.rs` already proves for `Heartbeat`.
#[tokio::test(start_paused = true)]
async fn a_tracked_member_with_a_stale_generation_is_refused_illegal_generation() {
    let fixture = fixture(&["orders"]).await;
    seat(&fixture.cluster, "g", "m1");
    let response = commit(
        &fixture.cluster,
        &body("g", 999, "m1", &[("orders", &[0])]),
        &open(),
    );
    assert_eq!(
        response.topics[0].partitions[0].error_code,
        oqueue_codec::error_codes::ILLEGAL_GENERATION
    );
}

/// A commit this broker cannot fence at all (no such group has ever
/// existed) is refused the same way an untracked member is -- there is no
/// coordinator record to distinguish "unknown group" from "unknown
/// member" against.
#[tokio::test(start_paused = true)]
async fn a_commit_against_a_group_nothing_ever_joined_is_refused() {
    let fixture = fixture(&["orders"]).await;
    let response = commit(
        &fixture.cluster,
        &body("never-joined", 1, "m1", &[("orders", &[0])]),
        &open(),
    );
    assert_eq!(
        response.topics[0].partitions[0].error_code,
        oqueue_codec::error_codes::UNKNOWN_MEMBER_ID
    );
}

/// A fenced, well-formed commit actually lands -- readable back through
/// `CommittedOffsets`' own test-only accessor.
#[tokio::test(start_paused = true)]
async fn a_fenced_commit_actually_lands() {
    let fixture = fixture(&["orders"]).await;
    seat(&fixture.cluster, "g", "m1");
    let response = commit(
        &fixture.cluster,
        &body("g", 1, "m1", &[("orders", &[0, 1])]),
        &open(),
    );
    for partition in &response.topics[0].partitions {
        assert_eq!(partition.error_code, 0);
    }
    let group = oqueue_core::GroupId::new("g").expect("valid");
    let topic = TopicId::new("orders").expect("valid");
    assert_eq!(
        fixture.cluster.committed_offsets().get(&group, &topic, 0),
        Some(42)
    );
    assert_eq!(
        fixture.cluster.committed_offsets().get(&group, &topic, 1),
        Some(42)
    );
}

/// ⚠️ **`M4.12`'s own acceptance criterion, verbatim**: a commit from
/// principal A cannot land under a group session authenticated as
/// principal B -- `M9.7`'s own authorization decision point, reused here
/// rather than a second one invented. Batched: alice's own request names
/// both her own topic and bob's in one call, so this also proves one
/// unauthorized topic does not refuse the other, `M9.12`'s own
/// per-topic-not-per-request precedent.
#[tokio::test(start_paused = true)]
async fn a_commit_from_principal_a_cannot_land_under_principal_bs_topic() {
    let fixture = fixture(&["alice-topic", "bob-topic"]).await;
    seat(&fixture.cluster, "g", "m1");

    let mut grants = TopicGrants::new();
    grants.grant(
        Principal::new("alice").expect("valid"),
        TopicId::new("alice-topic").expect("valid"),
    );
    grants.grant(
        Principal::new("bob").expect("valid"),
        TopicId::new("bob-topic").expect("valid"),
    );
    let alice = Principal::new("alice").expect("valid");
    let authz = AuthzContext {
        principal: Some(&alice),
        credentials_configured: true,
        topic_grants: &grants,
    };

    let response = commit(
        &fixture.cluster,
        &body("g", 1, "m1", &[("alice-topic", &[0]), ("bob-topic", &[0])]),
        &authz,
    );
    assert_response_lands_alice_refuses_bob(&response);
    assert_store_lands_alice_refuses_bob(&fixture.cluster);
}

/// Split out of the test above purely to keep it under
/// `code-structure.md`'s fifty-line limit — same acceptance criterion,
/// the response half.
fn assert_response_lands_alice_refuses_bob(response: &KpResponse) {
    assert_eq!(response.topics.len(), 2);
    let alice_topic = response
        .topics
        .iter()
        .find(|t| t.name.as_str() == "alice-topic")
        .expect("named");
    assert_eq!(
        alice_topic.partitions[0].error_code, 0,
        "alice's own topic must land"
    );
    let bob_topic = response
        .topics
        .iter()
        .find(|t| t.name.as_str() == "bob-topic")
        .expect("named");
    assert_eq!(
        bob_topic.partitions[0].error_code,
        oqueue_codec::error_codes::TOPIC_AUTHORIZATION_FAILED,
        "bob's own topic must be refused"
    );
}

/// The store half of the same acceptance criterion.
fn assert_store_lands_alice_refuses_bob(cluster: &crate::cluster::Cluster) {
    let group = oqueue_core::GroupId::new("g").expect("valid");
    assert_eq!(
        cluster
            .committed_offsets()
            .get(&group, &TopicId::new("alice-topic").expect("valid"), 0),
        Some(42),
        "alice's own commit landed"
    );
    assert_eq!(
        cluster
            .committed_offsets()
            .get(&group, &TopicId::new("bob-topic").expect("valid"), 0),
        None,
        "the refused commit never landed"
    );
}

/// A malformed body closes the connection rather than answering — every
/// other handler's own policy for a frame this broker cannot decode.
#[tokio::test(start_paused = true)]
async fn a_malformed_body_closes_rather_than_panicking() {
    let fixture = fixture(&[]).await;
    let response = handle(&fixture.cluster, prelude(), &[0xFF; 3], &open());
    assert!(matches!(response, HandlerResponse::Close));
}
