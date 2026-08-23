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
use oqueue_codec::apikey::ApiKey;
use oqueue_codec::error_codes;
use oqueue_codec::fetch::{
    FetchResponse, FetchResponsePartition, FetchResponseTopic, decode_request,
};
use oqueue_codec::frame::{RequestPrelude, encode_response_header};

/// One topic's response, owned so the borrowed [`FetchResponseTopic`] can
/// point into its echoed name and its concatenated batch bytes.
struct TopicOutcome {
    name: Option<String>,
    topic_id: [u8; 16],
    partitions: Vec<PartitionOutcome>,
}

/// One partition's outcome, owned so the borrowed [`FetchResponsePartition`]
/// can point into the concatenated batch bytes.
struct PartitionOutcome {
    index: i32,
    error_code: i16,
    high_watermark: i64,
    last_stable_offset: i64,
    log_start_offset: i64,
    records: Vec<u8>,
}

/// Decodes, resolves, reads, answers — or closes on a malformed body.
pub(crate) fn handle(
    cluster: &StubCluster,
    prelude: RequestPrelude,
    body: &[u8],
) -> HandlerResponse {
    let version = prelude.api_version;
    let Ok(request) = decode_request(body, version) else {
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

    let outcomes: Vec<TopicOutcome> = request
        .topics
        .iter()
        .map(|topic| one_topic(cluster, topic, version))
        .collect();

    let response = FetchResponse {
        topics: outcomes
            .iter()
            .map(|t| FetchResponseTopic {
                name: t.name.as_deref(),
                topic_id: t.topic_id,
                partitions: t
                    .partitions
                    .iter()
                    .map(|p| FetchResponsePartition {
                        index: p.index,
                        error_code: p.error_code,
                        high_watermark: p.high_watermark,
                        last_stable_offset: p.last_stable_offset,
                        log_start_offset: p.log_start_offset,
                        records: Some(&p.records),
                    })
                    .collect(),
            })
            .collect(),
    };

    let mut out = Vec::new();
    if encode_response_header(&mut out, ApiKey::Fetch, version, prelude.correlation_id).is_err() {
        return HandlerResponse::Close;
    }
    // Sessions declined: id 0 tells the client to keep full-fetching
    // (`encode_response` writes it).
    oqueue_codec::fetch::encode_response(&mut out, version, &response);
    HandlerResponse::Reply(out)
}

/// One topic's outcome: resolve its addressing, then every partition.
fn one_topic(
    cluster: &StubCluster,
    topic: &oqueue_codec::fetch::FetchTopic<'_>,
    version: i16,
) -> TopicOutcome {
    // From v13 the wire addresses topics by id — same split as produce:
    // echo what this version carries, resolve the rest.
    let (name, resolved_name) = if version >= 13 {
        (
            None,
            cluster.topic_name_by_id(uuid::Uuid::from_bytes(topic.topic_id)),
        )
    } else {
        (topic.name.map(str::to_owned), topic.name.map(str::to_owned))
    };
    // An id this broker never issued has its own error (100); a name it
    // does not host stays UNKNOWN_TOPIC_OR_PARTITION, matching what real
    // brokers answer on each addressing path.
    let unknown = if version >= 13 {
        error_codes::UNKNOWN_TOPIC_ID
    } else {
        error_codes::UNKNOWN_TOPIC_OR_PARTITION
    };
    let partitions = topic
        .partitions
        .iter()
        .map(|p| {
            one_partition(
                cluster,
                resolved_name.as_deref(),
                unknown,
                p.index,
                p.fetch_offset,
            )
        })
        .collect();
    TopicOutcome {
        name,
        topic_id: topic.topic_id,
        partitions,
    }
}

/// One partition's answer: every batch and the watermark, or a refusal.
///
/// ⚠️ **The unset-offset sentinel is `-1`, not `0`** — the protocol's own
/// convention (`kafka-protocol`'s generated `PartitionData::default()`
/// agrees), and every refusal path below leaves `last_stable_offset` and
/// `log_start_offset` there rather than guessing a real value for a
/// partition that was never read. `high_watermark` is the one field a
/// refusal may still answer honestly (`OFFSET_OUT_OF_RANGE` reports the
/// real watermark the client overshot); the other two refusals report a
/// partition this broker never touched, so `0` would be as much a fiction
/// as `-1` is a confession.
fn one_partition(
    cluster: &StubCluster,
    topic: Option<&str>,
    unknown: i16,
    index: i32,
    fetch_offset: i64,
) -> PartitionOutcome {
    const OFFSET_UNSET: i64 = -1;
    let refused = |error_code: i16, high_watermark: i64| PartitionOutcome {
        index,
        error_code,
        high_watermark,
        last_stable_offset: OFFSET_UNSET,
        log_start_offset: OFFSET_UNSET,
        records: Vec::new(),
    };

    let Some(topic) = topic else {
        return refused(unknown, 0);
    };
    let read = usize::try_from(index)
        .ok()
        .and_then(|partition| cluster.read(topic, partition, fetch_offset));
    let Some((batches, high)) = read else {
        return refused(error_codes::UNKNOWN_TOPIC_OR_PARTITION, 0);
    };
    if fetch_offset < 0 || fetch_offset > high {
        // The watermark is real even though the read below is not
        // filtered by it: past-the-end is the client's bug to hear about.
        return refused(error_codes::OFFSET_OUT_OF_RANGE, high);
    }

    // The idle-poll case the module doc promises: a consumer already at
    // the watermark gets empty records, not the whole log again. No
    // transactions yet, so the stable offset IS the watermark.
    let records = if fetch_offset == high {
        Vec::new()
    } else {
        batches.concat()
    };
    PartitionOutcome {
        index,
        error_code: error_codes::NONE,
        high_watermark: high,
        last_stable_offset: high,
        log_start_offset: 0,
        records,
    }
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
            let p = &response.responses[0].partitions[0];
            assert_eq!(p.high_watermark, 0, "no partition was ever read");
            assert_eq!(
                p.last_stable_offset, -1,
                "-1 is the protocol's unset sentinel, never guessed"
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
        let past_partition = &past.responses[0].partitions[0];
        assert_eq!(
            past_partition.error_code,
            kafka_protocol::error::ResponseError::OffsetOutOfRange.code()
        );
        assert_eq!(
            past_partition.high_watermark, 2,
            "the watermark is reported honestly even on refusal"
        );
        assert_eq!(
            past_partition.last_stable_offset, -1,
            "unlike the watermark, the stable offset is not guessed on refusal"
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
        use kafka_protocol::messages::RequestHeader;
        use oqueue_codec::apikey::ApiKey;
        let cluster = std::sync::Arc::new(StubCluster::new("h", 1));
        cluster.ensure_topic("t");
        let dispatcher = crate::Dispatcher::new(std::sync::Arc::clone(&cluster));

        let framed = |api_key: i16, version: i16, body: &[u8]| {
            let mut request = Vec::new();
            let mut header = RequestHeader::default();
            header.request_api_key = api_key;
            header.request_api_version = version;
            header.correlation_id = 7;
            let header_version = ApiKey::from_i16(api_key)
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
