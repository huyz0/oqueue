#![allow(clippy::expect_used)]

use super::{AuthzContext, handle};
use crate::connection::HandlerResponse;
use crate::testing::{Fixture, fixture, golden_batch, produce_one};
use kafka_protocol::messages::list_offsets_request::{ListOffsetsPartition, ListOffsetsTopic};
use kafka_protocol::messages::{ListOffsetsRequest, ListOffsetsResponse, TopicName};
use kafka_protocol::protocol::{Decodable, Encodable, StrBytes};
use oqueue_codec::frame::RequestPrelude;

const VERSION: i16 = 9;

fn body(topic: &str, partition: i32, timestamp: i64) -> Vec<u8> {
    let mut request = ListOffsetsRequest::default();
    request.replica_id = kafka_protocol::messages::BrokerId(-1);
    let mut t = ListOffsetsTopic::default();
    t.name = TopicName(StrBytes::from_string(topic.to_owned()));
    let mut p = ListOffsetsPartition::default();
    p.partition_index = partition;
    p.timestamp = timestamp;
    t.partitions.push(p);
    request.topics.push(t);
    let mut out = Vec::new();
    request.encode(&mut out, VERSION).expect("encodes");
    out
}

async fn replied(fixture: &Fixture, body: &[u8]) -> ListOffsetsResponse {
    let prelude = RequestPrelude {
        api_key: 2,
        api_version: VERSION,
        correlation_id: 11,
    };
    let HandlerResponse::Reply(out) = handle(
        &fixture.cluster,
        prelude,
        body,
        &AuthzContext {
            principal: None,
            credentials_configured: false,
            topic_grants: &oqueue_core::TopicGrants::default(),
        },
    )
    .await
    else {
        panic!("a ListOffsets with a legal isolation level replies");
    };
    let mut rest = &out[5..];
    let response = ListOffsetsResponse::decode(&mut rest, VERSION).expect("decodes");
    assert!(rest.is_empty());
    response
}

async fn replied_as(
    fixture: &Fixture,
    body: &[u8],
    authz: &AuthzContext<'_>,
) -> ListOffsetsResponse {
    let prelude = RequestPrelude {
        api_key: 2,
        api_version: VERSION,
        correlation_id: 11,
    };
    let HandlerResponse::Reply(out) = handle(&fixture.cluster, prelude, body, authz).await else {
        panic!("a ListOffsets with a legal isolation level replies");
    };
    let mut rest = &out[5..];
    let response = ListOffsetsResponse::decode(&mut rest, VERSION).expect("decodes");
    assert!(rest.is_empty());
    response
}

/// ⚠️ **Hazard H1: the end offset is the real one, not a cached one.**
/// After three produces the log ends at 6, and a client computing lag from
/// anything smaller gets a negative number — the silent wrongness this
/// milestone is written against.
#[tokio::test]
async fn latest_is_the_offset_after_the_last_record() {
    let fixture = fixture(&["t"]).await;
    for _ in 0..3 {
        produce_one(&fixture, "t", golden_batch()).await;
    }

    let response = replied(&fixture, &body("t", 0, -1)).await;

    let p = &response.topics[0].partitions[0];
    assert_eq!(p.error_code, 0);
    assert_eq!(p.offset, 6, "two records per produce, three produces");
}

/// ⚠️ **And it tracks, rather than being read once.** A `ListOffsets` after
/// a further produce must move; one that did not would be exactly the
/// stale answer H1 describes.
#[tokio::test]
async fn latest_moves_when_the_log_does() {
    let fixture = fixture(&["t"]).await;
    produce_one(&fixture, "t", golden_batch()).await;
    assert_eq!(
        replied(&fixture, &body("t", 0, -1)).await.topics[0].partitions[0].offset,
        2
    );

    produce_one(&fixture, "t", golden_batch()).await;

    assert_eq!(
        replied(&fixture, &body("t", 0, -1)).await.topics[0].partitions[0].offset,
        4
    );
}

/// ⚠️ **`EARLIEST` is where the log still begins**, which is `0` until
/// something is deleted — `M5`'s retention is what makes it move.
#[tokio::test]
async fn earliest_is_the_first_offset_still_held() {
    let fixture = fixture(&["t"]).await;
    produce_one(&fixture, "t", golden_batch()).await;

    let p = &replied(&fixture, &body("t", 0, -2)).await.topics[0].partitions[0];
    assert_eq!(p.error_code, 0);
    assert_eq!(p.offset, 0);
}

/// ⚠️ **An empty partition ends where it begins.** `EARLIEST == LATEST ==
/// 0` is what tells a consumer there is nothing rather than something it
/// cannot see.
#[tokio::test]
async fn an_empty_partition_reports_zero_both_ways() {
    let fixture = fixture(&["t"]).await;
    for timestamp in [-1_i64, -2] {
        let p = &replied(&fixture, &body("t", 0, timestamp)).await.topics[0].partitions[0];
        assert_eq!(p.error_code, 0, "ts {timestamp}");
        assert_eq!(p.offset, 0, "ts {timestamp}");
    }
}

/// ⚠️ **A wall-clock timestamp is refused, not approximated.** Answering
/// the nearest offset would send a consumer somewhere it did not ask for,
/// silently; there is no index by time to answer it from.
///
/// ⚠️ **And refused with a code the consumer *raises*.** The obvious
/// choice, `UNSUPPORTED_FOR_MESSAGE_FORMAT`, is the one the Java consumer
/// turns back into silence: `offsetsForTimes` records no offset for the
/// partition and returns null, which an application cannot tell from a
/// truthful "nothing at or after that time". Picking a code the client
/// swallows would undo the whole argument for refusing.
#[tokio::test]
async fn a_real_timestamp_is_refused_with_a_code_the_client_cannot_swallow() {
    let fixture = fixture(&["t"]).await;
    produce_one(&fixture, "t", golden_batch()).await;

    let p = &replied(&fixture, &body("t", 0, 1_700_000_000_000))
        .await
        .topics[0]
        .partitions[0];
    assert_eq!(
        p.error_code,
        kafka_protocol::error::ResponseError::UnsupportedVersion.code()
    );
    assert_ne!(
        p.error_code,
        kafka_protocol::error::ResponseError::UnsupportedForMessageFormat.code(),
        "code 43 is the one `offsetsForTimes` turns into a null"
    );
    assert_eq!(p.offset, -1, "a refusal answers -1, never a plausible 0");
}

/// ⚠️ **An unknown topic or partition is refused with `-1`.** A `0` here is
/// the doc 13 §8 inversion in its most damaging place: a consumer told a
/// partition it cannot reach ends at 0 computes negative lag forever.
#[tokio::test]
async fn an_unknown_topic_or_partition_answers_minus_one() {
    let fixture = fixture(&["t"]).await;
    for (topic, partition) in [("ghost", 0), ("t", 1), ("t", -1)] {
        let p = &replied(&fixture, &body(topic, partition, -1)).await.topics[0].partitions[0];
        assert_eq!(
            p.error_code,
            kafka_protocol::error::ResponseError::UnknownTopicOrPartition.code(),
            "{topic}/{partition}"
        );
        assert_eq!(p.offset, -1, "{topic}/{partition}");
    }
}

/// ⚠️ **A null topic name closes rather than being echoed.** The field is
/// nullable on the request wire and not nullable on the response wire, so
/// echoing it frames a body no client can parse — the Java client throws
/// in its response parser, librdkafka reports a protocol read error, and
/// the connection dies having been handed bytes it could not read. A close
/// is the answer every other unanswerable shape gets.
#[tokio::test]
async fn a_null_topic_name_closes_rather_than_framing_an_unparsable_reply() {
    let fixture = fixture(&["t"]).await;
    // The dependency's encoder will not write a null here, so the bytes
    // are hand-built: `replica_id`, `isolation_level`, then a compact
    // array of one topic whose name is the null marker.
    let mut out = Vec::new();
    out.extend_from_slice(&(-1_i32).to_be_bytes()); // replica_id
    out.push(0); // isolation_level
    out.push(2); // compact array length 1
    out.push(0); // compact nullable string: null
    out.push(1); // compact array of partitions: empty
    out.push(0); // tagged fields (topic)
    out.push(0); // tagged fields (request)
    let prelude = RequestPrelude {
        api_key: 2,
        api_version: VERSION,
        correlation_id: 11,
    };

    assert!(matches!(
        handle(
            &fixture.cluster,
            prelude,
            &out,
            &AuthzContext {
                principal: None,
                credentials_configured: false,
                topic_grants: &oqueue_core::TopicGrants::default(),
            }
        )
        .await,
        HandlerResponse::Close
    ));
}

/// ⚠️ **An undefined isolation level closes**, the dispatcher's policy for
/// every shape it cannot answer.
#[tokio::test]
async fn an_undefined_isolation_level_closes_the_connection() {
    let fixture = fixture(&["t"]).await;
    let mut request = ListOffsetsRequest::default();
    request.replica_id = kafka_protocol::messages::BrokerId(-1);
    request.isolation_level = 7;
    let mut out = Vec::new();
    request.encode(&mut out, VERSION).expect("encodes");
    let prelude = RequestPrelude {
        api_key: 2,
        api_version: VERSION,
        correlation_id: 11,
    };
    assert!(matches!(
        handle(
            &fixture.cluster,
            prelude,
            &out,
            &AuthzContext {
                principal: None,
                credentials_configured: false,
                topic_grants: &oqueue_core::TopicGrants::default(),
            }
        )
        .await,
        HandlerResponse::Close
    ));
}

/// `M9.12`'s own tests: per-principal scoping on `ListOffsets`, `Metadata`'s
/// `M9.9` shape reused rather than reinvented.
mod authorization {
    use super::{body, replied_as};
    use crate::authz::AuthzContext;
    use crate::testing::fixture;
    use oqueue_core::{Principal, TopicGrants, TopicId};

    fn alice() -> Principal {
        Principal::new("alice").expect("valid")
    }

    #[tokio::test]
    async fn an_unconfigured_broker_answers_every_topic_regardless_of_grants() {
        let fixture = fixture(&["t"]).await;
        let response = replied_as(
            &fixture,
            &body("t", 0, -1),
            &AuthzContext {
                principal: None,
                credentials_configured: false,
                topic_grants: &TopicGrants::default(),
            },
        )
        .await;
        assert_eq!(response.topics[0].partitions[0].error_code, 0);
    }

    #[tokio::test]
    async fn a_configured_broker_refuses_an_ungranted_topic() {
        let fixture = fixture(&["t"]).await;
        let grants = TopicGrants::new();
        let response = replied_as(
            &fixture,
            &body("t", 0, -1),
            &AuthzContext {
                principal: Some(&alice()),
                credentials_configured: true,
                topic_grants: &grants,
            },
        )
        .await;
        assert_eq!(
            response.topics[0].partitions[0].error_code,
            oqueue_codec::error_codes::TOPIC_AUTHORIZATION_FAILED,
        );
        assert_eq!(
            response.topics[0].partitions[0].offset, -1,
            "a refusal answers -1, never a plausible 0"
        );
    }

    #[tokio::test]
    async fn a_configured_broker_answers_a_granted_topic() {
        let fixture = fixture(&["t"]).await;
        let mut grants = TopicGrants::new();
        grants.grant(alice(), TopicId::new("t").expect("valid"));
        let response = replied_as(
            &fixture,
            &body("t", 0, -1),
            &AuthzContext {
                principal: Some(&alice()),
                credentials_configured: true,
                topic_grants: &grants,
            },
        )
        .await;
        assert_eq!(response.topics[0].partitions[0].error_code, 0);
    }
}
