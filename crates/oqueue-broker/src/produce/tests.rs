//! What the produce handler answers, at every version it advertises.
//!
//! ⚠️ **Its own file because `mod.rs` reached the 500-line limit**, and the
//! seam is the ordinary one: `mod.rs` is the handler and this is what it is
//! asserted to do. ⚠️ **`pub(crate)` for one consumer**: `produce/answer.rs`'s
//! own tests build on `produce_body`, `replied` and `verdict`. ⚠️ **Not the
//! fetch or roundtrip suites**, which this claimed until `M3.37` — those use
//! `crate::testing::produce_one`, a second hand-rolled copy of the same
//! request encoding, which is the duplication this note said had been avoided.

#![allow(clippy::expect_used)]
#![allow(clippy::redundant_pub_crate)]

use super::handle;
use crate::authz::AuthzContext;
use crate::connection::HandlerResponse;
use crate::testing::{Fixture, fixture, golden_batch, partition, topic};
use kafka_protocol::messages::produce_request::{PartitionProduceData, TopicProduceData};
use kafka_protocol::messages::{ProduceRequest, ProduceResponse, TopicName};
use kafka_protocol::protocol::{Decodable, Encodable, StrBytes};
use oqueue_codec::frame::RequestPrelude;
use oqueue_core::Operation;

pub(crate) fn produce_body(version: i16, name: &str, acks: i16, records: Vec<u8>) -> Vec<u8> {
    let mut t = TopicProduceData::default();
    t.name = TopicName(StrBytes::from_string(name.to_owned()));
    body_for(version, vec![(t, records)], acks)
}

/// v13's addressing: the topic id, no name (the encoder refuses one).
fn produce_body_by_id(version: i16, id: uuid::Uuid, acks: i16, records: Vec<u8>) -> Vec<u8> {
    let mut t = TopicProduceData::default();
    t.topic_id = id;
    body_for(version, vec![(t, records)], acks)
}

pub(crate) fn body_for(
    version: i16,
    topics: Vec<(TopicProduceData, Vec<u8>)>,
    acks: i16,
) -> Vec<u8> {
    let mut request = ProduceRequest::default();
    request.acks = acks;
    for (mut t, records) in topics {
        let mut p = PartitionProduceData::default();
        p.index = 0;
        p.records = Some(bytes::Bytes::from(records));
        t.partition_data.push(p);
        request.topic_data.push(t);
    }
    let mut out = Vec::new();
    request.encode(&mut out, version).expect("encodes");
    out
}

/// ⚠️ **A null topic name closes rather than being echoed**, at every
/// advertised version that has a name to be null — the third handler to
/// need this. The field is nullable on the request wire and not nullable
/// on the response wire, so echoing it frames a body no client can parse.
///
/// ⚠️ **Every version, not one.** The string encoding changes at v9 (the
/// flexible cutover) and the surrounding request layout changes more than
/// once, so a test at one version pins one encoding and leaves the rest
/// unchecked — the same reason `M3.29`'s `Fetch` sweep runs v4 through v12.
#[tokio::test]
async fn a_null_topic_name_closes_at_every_version_that_has_one() {
    let fixture = fixture(&["zzzprobe"]).await;
    for version in 3..=12 {
        let body = with_null_name(version, "zzzprobe");
        assert!(
            matches!(
                handle(
                    &fixture.cluster,
                    &fixture.session,
                    prelude(version),
                    &body,
                    &AuthzContext {
                        principal: None,
                        credentials_configured: false,
                        topic_grants: &oqueue_core::TopicGrants::default(),
                    }
                )
                .await,
                HandlerResponse::Close
            ),
            "v{version} framed a reply for a null topic name"
        );
    }
}

/// ⚠️ **And a name that is present still replies**, which is the positive
/// control the same sweep needs: `Close` is also what a body this handler
/// cannot decode produces, so a version that stopped decoding at all would
/// otherwise be indistinguishable from the guard firing.
#[tokio::test]
async fn a_present_topic_name_still_replies_at_every_version() {
    let fixture = fixture(&["zzzprobe"]).await;
    for version in 3..=12 {
        let body = produce_body(version, "zzzprobe", 1, golden_batch());
        assert!(
            matches!(
                handle(
                    &fixture.cluster,
                    &fixture.session,
                    prelude(version),
                    &body,
                    &AuthzContext {
                        principal: None,
                        credentials_configured: false,
                        topic_grants: &oqueue_core::TopicGrants::default(),
                    }
                )
                .await,
                HandlerResponse::Reply(_)
            ),
            "v{version} refused a request whose name is present"
        );
    }
}

/// A `Produce` request whose one topic's name is the wire's null marker.
///
/// ⚠️ **Built by the dependency and then patched**, because the dependency
/// will not encode a null here — and hand-writing ten versions of the
/// surrounding layout would be re-implementing the request encoder inside
/// a test of the handler.
fn with_null_name(version: i16, name: &str) -> Vec<u8> {
    let body = produce_body(version, name, 1, golden_batch());
    let flexible = version >= 9;
    let encoded: Vec<u8> = if flexible {
        let mut v = vec![u8::try_from(name.len() + 1).expect("a short name")];
        v.extend_from_slice(name.as_bytes());
        v
    } else {
        let mut v = i16::try_from(name.len())
            .expect("a short name")
            .to_be_bytes()
            .to_vec();
        v.extend_from_slice(name.as_bytes());
        v
    };
    let null: Vec<u8> = if flexible {
        vec![0]
    } else {
        (-1_i16).to_be_bytes().to_vec()
    };
    let at = body
        .windows(encoded.len())
        .position(|window| window == encoded.as_slice())
        .expect("the name the encoder wrote is in the body");
    let mut out = body[..at].to_vec();
    out.extend_from_slice(&null);
    out.extend_from_slice(&body[at + encoded.len()..]);
    out
}

fn prelude(version: i16) -> RequestPrelude {
    RequestPrelude {
        api_key: 0,
        api_version: version,
        correlation_id: 8,
    }
}

fn decode(bytes: &[u8], version: i16) -> ProduceResponse {
    let header_len = if version >= 9 { 5 } else { 4 };
    let mut rest = &bytes[header_len..];
    let r = ProduceResponse::decode(&mut rest, version).expect("decodes");
    assert!(rest.is_empty());
    r
}

pub(crate) async fn replied(fixture: &Fixture, version: i16, body: &[u8]) -> ProduceResponse {
    let HandlerResponse::Reply(out) = handle(
        &fixture.cluster,
        &fixture.session,
        prelude(version),
        body,
        &AuthzContext {
            principal: None,
            credentials_configured: false,
            topic_grants: &oqueue_core::TopicGrants::default(),
        },
    )
    .await
    else {
        panic!("an acks != 0 produce replies");
    };
    decode(&out, version)
}

/// The one code a partition may be refused with, or its assigned offset.
pub(crate) fn verdict(response: &ProduceResponse) -> (i16, i64) {
    let p = &response.responses[0].partition_responses[0];
    (p.error_code, p.base_offset)
}

#[tokio::test]
async fn offsets_are_assigned_by_the_commit_and_advance_by_record_count() {
    let fixture = fixture(&["t"]).await;
    for expected_base in [0_i64, 2] {
        let body = produce_body(9, "t", -1, golden_batch());
        assert_eq!(
            verdict(&replied(&fixture, 9, &body).await),
            (0, expected_base)
        );
    }
    assert_eq!(
        fixture
            .cluster
            .high_watermark(&topic("t"), partition(0))
            .get(),
        4,
        "the watermark is derived from the index, not set by the handler"
    );
}

#[tokio::test]
async fn a_refused_batch_costs_no_put_and_stores_nothing() {
    let fixture = fixture(&["t"]).await;
    let mut bad = golden_batch();
    let last = bad.len() - 1;
    bad[last] ^= 0xFF;
    let body = produce_body(9, "t", -1, bad);

    let response = replied(&fixture, 9, &body).await;

    assert_eq!(
        verdict(&response),
        (
            kafka_protocol::error::ResponseError::CorruptMessage.code(),
            -1
        ),
        "a refusal answers -1, never a plausible 0 (doc 13 §8)"
    );
    assert_eq!(
        fixture.store.counts().count(Operation::Put),
        0,
        "nothing unverified reaches object storage"
    );
    assert_eq!(
        fixture
            .cluster
            .high_watermark(&topic("t"), partition(0))
            .get(),
        0
    );
}

/// ⚠️ **The ack's watermark is what the session remembers**, and this is
/// the half of read-your-writes that lives on the write path. Without it
/// the next fetch on this connection has no promise to keep, and hazard
/// H2's protection is gone with nothing to notice — in this broker
/// especially, where the coordinator folds before it acks and every fetch
/// would happen to be fresh anyway.
#[tokio::test]
async fn a_produce_leaves_its_watermark_on_the_session() {
    let fixture = fixture(&["t"]).await;
    assert_eq!(fixture.session.watermark(), None, "nothing produced yet");

    let body = produce_body(9, "t", -1, golden_batch());
    assert_eq!(verdict(&replied(&fixture, 9, &body).await), (0, 0));

    let remembered = fixture
        .session
        .watermark()
        .expect("the ack's watermark is remembered");
    assert_eq!(remembered.epoch(), fixture.cluster.epoch());
    assert_eq!(
        Some(remembered.version()),
        fixture.cluster.watch().applied(),
        "and it is the version the index folded for this commit"
    );
}

#[tokio::test]
async fn an_unknown_topic_is_that_partitions_error_not_a_close() {
    let fixture = fixture(&[]).await;
    let body = produce_body(9, "ghost", -1, golden_batch());
    assert_eq!(
        verdict(&replied(&fixture, 9, &body).await),
        (
            kafka_protocol::error::ResponseError::UnknownTopicOrPartition.code(),
            -1
        )
    );
}

/// ⚠️ **A hosted topic does not make every partition hosted.** A topic
/// with one partition is `0` only; `1` is a client addressing something
/// this broker does not have, and answering `NONE` for it would tell the
/// producer its records landed somewhere.
#[tokio::test]
async fn a_partition_past_the_topics_count_is_refused() {
    let fixture = fixture(&["t"]).await;
    let mut request = ProduceRequest::default();
    request.acks = -1;
    let mut t = TopicProduceData::default();
    t.name = TopicName(StrBytes::from_static_str("t"));
    let mut p = PartitionProduceData::default();
    p.index = 1;
    p.records = Some(bytes::Bytes::from(golden_batch()));
    t.partition_data.push(p);
    request.topic_data.push(t);
    let mut body = Vec::new();
    request.encode(&mut body, 9).expect("encodes");

    assert_eq!(
        verdict(&replied(&fixture, 9, &body).await),
        (
            kafka_protocol::error::ResponseError::UnknownTopicOrPartition.code(),
            -1
        )
    );
    assert_eq!(fixture.store.counts().count(Operation::Put), 0);
}

#[tokio::test]
async fn invalid_acks_is_refused_and_stores_nothing() {
    let fixture = fixture(&["t"]).await;
    let body = produce_body(9, "t", 2, golden_batch());
    assert_eq!(
        verdict(&replied(&fixture, 9, &body).await),
        (
            kafka_protocol::error::ResponseError::InvalidRequiredAcks.code(),
            -1
        )
    );
    assert_eq!(fixture.store.counts().count(Operation::Put), 0);
}

#[tokio::test]
async fn v13_addresses_the_topic_by_id_and_echoes_it() {
    let fixture = fixture(&["t"]).await;
    let id = fixture.cluster.topic_id("t").expect("an id");
    let body = produce_body_by_id(13, id, -1, golden_batch());
    let response = replied(&fixture, 13, &body).await;
    assert_eq!(response.responses[0].topic_id, id, "the id is the echo");
    assert_eq!(verdict(&response), (0, 0));
}

#[tokio::test]
async fn v13_with_an_unknown_id_refuses_per_partition_echoing_the_id() {
    let fixture = fixture(&[]).await;
    let ghost = uuid::Uuid::from_u128(0xDEAD);
    let body = produce_body_by_id(13, ghost, -1, golden_batch());
    let response = replied(&fixture, 13, &body).await;
    assert_eq!(response.responses[0].topic_id, ghost);
    assert_eq!(
        verdict(&response),
        (
            kafka_protocol::error::ResponseError::UnknownTopicId.code(),
            -1
        ),
        "ids have their own refusal, echoed through the id"
    );
}

/// v3: legacy header, v0 response header — the half librdkafka will not
/// negotiate, pinned here instead.
#[tokio::test]
async fn the_nonflexible_low_versions_answer_too() {
    let fixture = fixture(&["t"]).await;
    let body = produce_body(3, "t", -1, golden_batch());
    assert_eq!(verdict(&replied(&fixture, 3, &body).await), (0, 0));
}

#[tokio::test]
async fn acks_zero_is_silent_but_still_flushes_and_commits() {
    let fixture = fixture(&["t"]).await;
    let body = produce_body(9, "t", 0, golden_batch());
    assert!(matches!(
        handle(
            &fixture.cluster,
            &fixture.session,
            prelude(9),
            &body,
            &AuthzContext {
                principal: None,
                credentials_configured: false,
                topic_grants: &oqueue_core::TopicGrants::default(),
            }
        )
        .await,
        HandlerResponse::Silent
    ));
    assert_eq!(fixture.store.counts().count(Operation::Put), 1);
    assert_eq!(
        fixture
            .cluster
            .high_watermark(&topic("t"), partition(0))
            .get(),
        2,
        "fire-and-forget still lands"
    );
}

/// `M9.12`'s own tests: per-principal scoping on `Produce`, `Metadata`'s
/// `M9.9` shape reused rather than reinvented.
mod authorization;
