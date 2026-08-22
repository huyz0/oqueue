//! `Fetch` v4-17 against the stub: whole batches, an honest watermark.
//!
//! ⚠️ **Every stored batch comes back, whatever the fetch offset** — the
//! stub keeps opaque batch bytes with no per-batch index (`M3`'s job), and
//! returning whole batches from the log's start is protocol-legal because
//! consumers already skip records below their fetch offset (batch
//! alignment forces that skill on every client). The watermark is real,
//! though: an offset past it is `OFFSET_OUT_OF_RANGE`, and a fetch *at* it
//! returns empty records rather than an error — the poll loop's idle case.
//!
//! Sessions (KIP-227) are declined, which a broker may always do: every
//! response carries `session_id` 0, so clients fall back to full fetches.
//! `max_wait_ms`/`min_bytes` are parsed and ignored — the stub answers
//! immediately, empty-handed or not, and long-polling arrives with the
//! milestone that has something to wait on.

use crate::connection::HandlerResponse;
use crate::stub::StubCluster;
use kafka_protocol::error::ResponseError;
use kafka_protocol::messages::fetch_response::{FetchableTopicResponse, PartitionData};
use kafka_protocol::messages::{ApiKey, FetchRequest, FetchResponse};
use kafka_protocol::protocol::{Decodable, Encodable};
use oqueue_codec::frame::{RequestPrelude, encode_response_header};

/// Decodes, resolves, reads, answers — or closes on a malformed body.
pub(crate) fn handle(
    cluster: &StubCluster,
    prelude: RequestPrelude,
    body: &[u8],
) -> HandlerResponse {
    let mut buf = body;
    let Ok(request) = FetchRequest::decode(&mut buf, prelude.api_version) else {
        return HandlerResponse::Close;
    };
    // The two isolation levels the protocol defines; anything else is a
    // malformed request from a client that negotiated fine, and closes —
    // the dispatcher's policy for every unanswerable shape. With no
    // transactions in the stub, both levels see the same log, so the
    // parsed value changes nothing yet (`M2.md`).
    if !matches!(request.isolation_level, 0 | 1) {
        return HandlerResponse::Close;
    }

    let mut response = FetchResponse::default();
    // Sessions declined: id 0 tells the client to keep full-fetching.
    response.session_id = 0;
    for topic in &request.topics {
        let mut topic_response = FetchableTopicResponse::default();
        // From v13 the wire addresses topics by id — same split as
        // produce: echo what this version carries, resolve the rest.
        let resolved_name = if prelude.api_version >= 13 {
            topic_response.topic_id = topic.topic_id;
            cluster.topic_name_by_id(topic.topic_id)
        } else {
            topic_response.topic = topic.topic.clone();
            Some(topic.topic.to_string())
        };
        // An id this broker never issued has its own error (100); a name
        // it does not host stays UNKNOWN_TOPIC_OR_PARTITION, matching what
        // real brokers answer on each addressing path.
        let unknown = if prelude.api_version >= 13 {
            ResponseError::UnknownTopicId
        } else {
            ResponseError::UnknownTopicOrPartition
        };
        for partition in &topic.partitions {
            topic_response.partitions.push(one_partition(
                cluster,
                resolved_name.as_deref(),
                unknown,
                partition.partition,
                partition.fetch_offset,
            ));
        }
        response.responses.push(topic_response);
    }

    let mut out = Vec::new();
    if encode_response_header(
        &mut out,
        ApiKey::Fetch,
        prelude.api_version,
        prelude.correlation_id,
    )
    .is_err()
        || response.encode(&mut out, prelude.api_version).is_err()
    {
        return HandlerResponse::Close;
    }
    HandlerResponse::Reply(out)
}

/// One partition's answer: every batch and the watermark, or a refusal.
fn one_partition(
    cluster: &StubCluster,
    topic: Option<&str>,
    unknown: ResponseError,
    index: i32,
    fetch_offset: i64,
) -> PartitionData {
    let mut p = PartitionData::default();
    p.partition_index = index;

    let Some(topic) = topic else {
        p.error_code = unknown.code();
        return p;
    };
    let read = usize::try_from(index)
        .ok()
        .and_then(|partition| cluster.read(topic, partition, fetch_offset));
    let Some((batches, high)) = read else {
        p.error_code = ResponseError::UnknownTopicOrPartition.code();
        return p;
    };
    if fetch_offset < 0 || fetch_offset > high {
        // The watermark is real even though the read below is not
        // filtered by it: past-the-end is the client's bug to hear about.
        p.error_code = ResponseError::OffsetOutOfRange.code();
        p.high_watermark = high;
        return p;
    }

    p.high_watermark = high;
    // No transactions yet, so the stable offset IS the watermark and no
    // aborted-transaction list rides along.
    p.last_stable_offset = high;
    p.log_start_offset = 0;
    // The idle-poll case the module doc promises: a consumer already at
    // the watermark gets empty records, not the whole log again.
    let records: Vec<u8> = if fetch_offset == high {
        Vec::new()
    } else {
        batches.concat()
    };
    p.records = Some(bytes::Bytes::from(records));
    p
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::handle;
    use crate::connection::HandlerResponse;
    use crate::stub::StubCluster;
    use kafka_protocol::messages::fetch_request::{FetchPartition, FetchTopic};
    use kafka_protocol::messages::{FetchRequest, FetchResponse, TopicName};
    use kafka_protocol::protocol::{Decodable, Encodable, StrBytes};
    use oqueue_codec::frame::RequestPrelude;

    fn fetch_body(version: i16, t: FetchTopic, fetch_offset: i64, isolation: i8) -> Vec<u8> {
        let mut request = FetchRequest::default();
        request.max_wait_ms = 500;
        request.isolation_level = isolation;
        let mut t = t;
        let mut p = FetchPartition::default();
        p.partition = 0;
        p.fetch_offset = fetch_offset;
        p.partition_max_bytes = 1 << 20;
        t.partitions.push(p);
        request.topics.push(t);
        let mut out = Vec::new();
        request.encode(&mut out, version).expect("encodes");
        out
    }

    fn by_name(name: &str) -> FetchTopic {
        let mut t = FetchTopic::default();
        t.topic = TopicName(StrBytes::from_string(name.to_owned()));
        t
    }

    fn by_id(id: uuid::Uuid) -> FetchTopic {
        let mut t = FetchTopic::default();
        t.topic_id = id;
        t
    }

    fn prelude(version: i16) -> RequestPrelude {
        RequestPrelude {
            api_key: 1,
            api_version: version,
            correlation_id: 5,
        }
    }

    fn decode(bytes: &[u8], version: i16) -> FetchResponse {
        // Fetch goes flexible (tagged response header) at v12.
        let header_len = if version >= 12 { 5 } else { 4 };
        let mut rest = &bytes[header_len..];
        let r = FetchResponse::decode(&mut rest, version).expect("decodes");
        assert!(rest.is_empty());
        r
    }

    /// A cluster with one produced batch: the produce path is the fixture,
    /// so what fetch returns is exactly what produce stored.
    fn produced_cluster() -> StubCluster {
        let cluster = StubCluster::new("h", 1);
        cluster.ensure_topic("t");
        let batch = crate::produce::tests::golden_batch();
        let body = crate::produce::tests::produce_body(9, "t", -1, batch);
        let HandlerResponse::Reply(_) =
            crate::produce::handle(&cluster, prelude_for_produce(), &body)
        else {
            panic!("the fixture produce replies");
        };
        cluster
    }

    fn prelude_for_produce() -> RequestPrelude {
        RequestPrelude {
            api_key: 0,
            api_version: 9,
            correlation_id: 1,
        }
    }

    #[test]
    fn a_fetch_returns_what_produce_stored_with_the_watermark() {
        let cluster = produced_cluster();
        for version in [4i16, 12] {
            let body = fetch_body(version, by_name("t"), 0, 0);
            let HandlerResponse::Reply(out) = handle(&cluster, prelude(version), &body) else {
                panic!("replies");
            };
            let response = decode(&out, version);
            let p = &response.responses[0].partitions[0];
            assert_eq!(p.error_code, 0);
            assert_eq!(p.high_watermark, 2);
            assert_eq!(p.last_stable_offset, 2);
            let (stored, _) = cluster.read("t", 0, 0).expect("exists");
            assert_eq!(
                p.records.as_deref().expect("records ride along"),
                stored.concat().as_slice(),
                "the bytes are the stored batches, verbatim"
            );
        }
    }

    #[test]
    fn v17_addresses_the_topic_by_id_and_echoes_it() {
        let cluster = produced_cluster();
        let id = cluster.topic_id("t").expect("an id");
        let body = fetch_body(17, by_id(id), 0, 0);
        let HandlerResponse::Reply(out) = handle(&cluster, prelude(17), &body) else {
            panic!("replies");
        };
        let response = decode(&out, 17);
        assert_eq!(response.responses[0].topic_id, id);
        let p = &response.responses[0].partitions[0];
        assert_eq!(p.error_code, 0);
        assert!(p.records.as_deref().is_some_and(|r| !r.is_empty()));
    }

    #[test]
    fn an_unknown_topic_or_id_is_that_partitions_error_not_a_close() {
        let cluster = StubCluster::new("h", 1);
        // Each addressing path has its own refusal: names answer 3, ids
        // answer 100 (UNKNOWN_TOPIC_ID), as real brokers do.
        let cases = [
            (
                12i16,
                by_name("ghost"),
                kafka_protocol::error::ResponseError::UnknownTopicOrPartition,
            ),
            (
                17,
                by_id(uuid::Uuid::from_u128(0xBAD)),
                kafka_protocol::error::ResponseError::UnknownTopicId,
            ),
        ];
        for (version, topic, expected) in cases {
            let body = fetch_body(version, topic, 0, 0);
            let HandlerResponse::Reply(out) = handle(&cluster, prelude(version), &body) else {
                panic!("replies");
            };
            let response = decode(&out, version);
            assert_eq!(
                response.responses[0].partitions[0].error_code,
                expected.code()
            );
        }
    }

    #[test]
    fn past_the_watermark_is_out_of_range_but_at_it_is_empty_success() {
        let cluster = produced_cluster();
        let body = fetch_body(12, by_name("t"), 3, 0);
        let HandlerResponse::Reply(out) = handle(&cluster, prelude(12), &body) else {
            panic!("replies");
        };
        let past = decode(&out, 12);
        assert_eq!(
            past.responses[0].partitions[0].error_code,
            kafka_protocol::error::ResponseError::OffsetOutOfRange.code()
        );

        let body = fetch_body(12, by_name("t"), 2, 0);
        let HandlerResponse::Reply(out) = handle(&cluster, prelude(12), &body) else {
            panic!("replies");
        };
        let at = decode(&out, 12);
        let p = &at.responses[0].partitions[0];
        assert_eq!(p.error_code, 0, "the idle poll case is not an error");
        assert_eq!(
            p.records.as_deref().map(<[u8]>::len),
            Some(0),
            "and it is empty -- not the whole log again"
        );
        assert_eq!(p.high_watermark, 2);
    }

    /// The whole walk `M2.25`'s harness will drive with a real client:
    /// produce through the dispatcher, then fetch the same records back
    /// through it — header slicing, `supports()` gate and all.
    #[test]
    fn a_produce_then_fetch_round_trips_through_the_dispatcher() {
        use kafka_protocol::messages::{ApiKey, RequestHeader};
        let cluster = std::sync::Arc::new(StubCluster::new("h", 1));
        cluster.ensure_topic("t");
        let dispatcher = crate::Dispatcher::new(std::sync::Arc::clone(&cluster));

        let framed = |api_key: i16, version: i16, body: &[u8]| {
            let mut request = Vec::new();
            let mut header = RequestHeader::default();
            header.request_api_key = api_key;
            header.request_api_version = version;
            header.correlation_id = 7;
            let header_version = ApiKey::try_from(api_key)
                .expect("a known key")
                .request_header_version(version);
            header
                .encode(&mut request, header_version)
                .expect("header encodes");
            request.extend_from_slice(body);
            request
        };

        let produce =
            crate::produce::tests::produce_body(9, "t", -1, crate::produce::tests::golden_batch());
        let HandlerResponse::Reply(_) = dispatcher.dispatch(&framed(0, 9, &produce)) else {
            panic!("produce replies");
        };

        let fetch = fetch_body(12, by_name("t"), 0, 0);
        let HandlerResponse::Reply(out) = dispatcher.dispatch(&framed(1, 12, &fetch)) else {
            panic!("fetch replies");
        };
        let response = decode(&out, 12);
        let p = &response.responses[0].partitions[0];
        assert_eq!(p.error_code, 0);
        assert_eq!(p.high_watermark, 2);
        assert!(p.records.as_deref().is_some_and(|r| !r.is_empty()));
    }

    #[test]
    fn sessions_are_declined_and_isolation_is_parsed() {
        let cluster = produced_cluster();
        // read_committed sees the same log -- no transactions to hide.
        let body = fetch_body(12, by_name("t"), 0, 1);
        let HandlerResponse::Reply(out) = handle(&cluster, prelude(12), &body) else {
            panic!("replies");
        };
        let response = decode(&out, 12);
        assert_eq!(response.session_id, 0, "no session is ever created");
        assert_eq!(response.responses[0].partitions[0].error_code, 0);

        // A third isolation level does not exist; same close policy as
        // every unanswerable shape.
        let body = fetch_body(12, by_name("t"), 0, 2);
        assert!(matches!(
            handle(&cluster, prelude(12), &body),
            HandlerResponse::Close
        ));
    }
}
