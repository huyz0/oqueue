//! `Produce` v3-13 against the real write path: verify, bundle, PUT, commit.
//!
//! ⚠️ **One request is one object and one metadata record** — FR-32. Every
//! partition that passes the ingest rule goes into a single
//! [`BundleBuilder`], so a produce spanning N topics costs one PUT rather than
//! N. Doc 12 prices a PUT far above the bytes in it; that ratio is the cost
//! model, and this handler is where it is spent.
//!
//! ⚠️ **The offset a client is told is the one the *commit* assigned**, not
//! one this handler picked. `ADR-0020` puts the serialization point at the log
//! append, so the base offsets arrive in the
//! [`CommitAck`](oqueue_coordinator::CommitAck) after the object is
//! durable — which is the whole of "no offset is externally visible before its
//! log record commits". Every refusal answers `-1`, never `0`: a
//! plausible-looking offset on an error path turns an availability bug into a
//! safety bug (doc 13 §8).
//!
//! ⚠️ **`acks=0` sends no response at all** — the protocol's fire-and-forget
//! mode, and the case that forced the [`crate::Handler`] seam's final shape:
//! answering it would desync the client's read stream, so the outcome is
//! `Silent`, not an empty reply. ⚠️ It still flushes, and still waits for the
//! commit before returning: the alternative is a handler that returns while a
//! flush it owns is unowned, which `async-concurrency.md` rule 13 refuses.

mod answer;

use crate::cluster::Cluster;
use crate::connection::HandlerResponse;
use crate::ingest::verify;
use answer::{PartitionSlot, Slot, answer};
use oqueue_codec::apikey::ApiKey;
use oqueue_codec::error_codes;
use oqueue_codec::frame::{RequestPrelude, encode_response_header};
use oqueue_codec::produce::{ProduceResponse, ProduceResponseTopic, decode_request};
use oqueue_core::{BundleBuilder, PartitionId, TopicId};
use std::collections::HashSet;

/// The bundle this request is filling, and what is already in it.
///
/// ⚠️ **The set is not bookkeeping.** A bundle's regions carry no offsets —
/// offsets are assigned at commit, after the object is written — so two
/// regions for one `(topic, partition)` in one object leave the read path
/// nothing to say which of them an `ObjectRef` names. `Cluster::read` refuses
/// such an object; this is what stops one being written. A `Produce` request
/// carrying a partition twice is malformed, and the second entry is what is
/// refused, so the first still lands.
struct Pending {
    bundle: BundleBuilder,
    seen: HashSet<(String, i32)>,
}

/// One topic's response, owned so the borrowed [`ProduceResponseTopic`] can
/// point into its echoed name.
struct TopicOutcome {
    name: Option<String>,
    topic_id: [u8; 16],
    partitions: Vec<PartitionSlot>,
}

/// Decodes, verifies, flushes, answers — or stays silent for `acks=0`.
pub(crate) async fn handle(
    cluster: &Cluster,
    prelude: RequestPrelude,
    body: &[u8],
) -> HandlerResponse {
    let version = prelude.api_version;
    let Ok(request) = decode_request(body, version) else {
        return HandlerResponse::Close;
    };
    // The protocol's three acks: 0, 1, -1. Anything else is answered
    // INVALID_REQUIRED_ACKS per partition with nothing stored.
    let acks_valid = matches!(request.acks, -1..=1);

    let mut pending = Pending {
        bundle: BundleBuilder::new(),
        seen: HashSet::new(),
    };
    let outcomes: Vec<TopicOutcome> = request
        .topics
        .iter()
        .map(|topic| one_topic(cluster, &mut pending, topic, version, acks_valid))
        .collect();

    // ⚠️ The one store call, and only when something passed. An empty bundle
    // cannot be sealed — `Error::EmptyBundle` — and committing no spans would
    // burn a `CommitVersion` on a record saying nothing happened.
    let flushed = if pending.bundle.is_empty() {
        Ok(Vec::new())
    } else {
        cluster.flush(pending.bundle).await.map(|ack| {
            ack.assignments()
                .iter()
                .map(|assignment| assignment.base_offset().get())
                .collect()
        })
    };

    if request.acks == 0 {
        return HandlerResponse::Silent;
    }
    let response = ProduceResponse {
        topics: outcomes
            .iter()
            .map(|topic| ProduceResponseTopic {
                name: topic.name.as_deref(),
                topic_id: topic.topic_id,
                partitions: topic
                    .partitions
                    .iter()
                    .map(|p| answer(p, flushed.as_ref()))
                    .collect(),
            })
            .collect(),
    };

    let mut out = Vec::new();
    if encode_response_header(&mut out, ApiKey::Produce, version, prelude.correlation_id).is_err() {
        return HandlerResponse::Close;
    }
    oqueue_codec::produce::encode_response(&mut out, version, &response);
    HandlerResponse::Reply(out)
}

/// One topic's outcome: resolve its addressing, then every partition.
fn one_topic(
    cluster: &Cluster,
    pending: &mut Pending,
    topic: &oqueue_codec::produce::ProduceTopic<'_>,
    version: i16,
    acks_valid: bool,
) -> TopicOutcome {
    // From v13 the wire addresses topics by id, not name — echo what the wire
    // carries at this version, and resolve the id against the registry.
    let (name, resolved_name) = if version >= 13 {
        (
            None,
            cluster.topic_name_by_id(uuid::Uuid::from_bytes(topic.topic_id)),
        )
    } else {
        (topic.name.clone(), topic.name.clone())
    };
    let partitions = topic
        .partitions
        .iter()
        .map(|p| PartitionSlot {
            index: p.index,
            slot: match (&resolved_name, acks_valid) {
                (_, false) => Slot::Refused(error_codes::INVALID_REQUIRED_ACKS),
                // An id this broker never issued: its own error (100), mapped
                // back through the echoed id.
                (None, true) => Slot::Refused(error_codes::UNKNOWN_TOPIC_ID),
                (Some(topic_name), true) => {
                    one_partition(cluster, pending, topic_name, p.index, p.records)
                }
            },
        })
        .collect();
    TopicOutcome {
        name,
        topic_id: topic.topic_id,
        partitions,
    }
}

/// One partition's verdict: the ingest rule, then a region in the bundle.
fn one_partition(
    cluster: &Cluster,
    pending: &mut Pending,
    topic: &str,
    index: i32,
    records: Option<&[u8]>,
) -> Slot {
    let Ok(partition) = PartitionId::new(index) else {
        return Slot::Refused(error_codes::UNKNOWN_TOPIC_OR_PARTITION);
    };
    let hosted = usize::try_from(index)
        .ok()
        .zip(cluster.partition_count(topic))
        .is_some_and(|(index, count)| index < count);
    if !hosted {
        return Slot::Refused(error_codes::UNKNOWN_TOPIC_OR_PARTITION);
    }
    let Some(records) = records else {
        // A produce with no records is a client bug; nothing to store.
        return Slot::Refused(error_codes::INVALID_RECORD);
    };
    let verified = match verify(records) {
        Ok(verified) => verified,
        Err(code) => return Slot::Refused(code),
    };
    let Ok(topic_id) = TopicId::new(topic.to_owned()) else {
        return Slot::Refused(error_codes::UNKNOWN_TOPIC_OR_PARTITION);
    };
    if !pending.seen.insert((topic.to_owned(), index)) {
        // A partition named twice in one request — see `Pending`.
        return Slot::Refused(error_codes::INVALID_RECORD);
    }
    let nth = pending.bundle.len();
    // ⚠️ The bytes go in **unstamped**: the offset is not known until the
    // commit, which is after this object is written. `Cluster::read` is where
    // the assigned offset is stamped in, and the CRC never covered those
    // twelve bytes (doc 18 §4.4), so neither end recomputes anything.
    match pending
        .bundle
        .push(topic_id, partition, verified.records, records)
    {
        Ok(()) => Slot::Pushed(nth),
        // Only a region this handler could not have built: an empty range, a
        // zero record count, or a topic name past the format's ceiling. None
        // is the client's fault in a way a retry mends.
        Err(_) => Slot::Refused(error_codes::INVALID_RECORD),
    }
}

#[cfg(test)]
pub(crate) mod tests {
    #![allow(clippy::expect_used)]
    #![allow(clippy::redundant_pub_crate)]

    use super::handle;
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

    fn body_for(version: i16, topics: Vec<(TopicProduceData, Vec<u8>)>, acks: i16) -> Vec<u8> {
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
        let HandlerResponse::Reply(out) = handle(&fixture.cluster, prelude(version), body).await
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
            handle(&fixture.cluster, prelude(9), &body).await,
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
}
