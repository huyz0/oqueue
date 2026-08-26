//! `Fetch` v4-17 against the real read path: index lookup, whole batches, a
//! derived watermark.
//!
//! ⚠️ **The fetch offset is honoured now**, which under `M2`'s stub it could
//! not be: the stub held opaque bytes with no per-batch index and returned
//! every batch it had, from the beginning. `IndexReader::find_batches` names
//! exactly the batches at or past the offset asked for, so a consumer that has
//! read 10,000 records no longer re-reads them.
//!
//! ⚠️ **The high watermark is derived, and nothing can set it** (`M3.md` task
//! 13): it is `IndexReader::end_offset`, folded from the metadata log. A
//! settable watermark is how a broker reports a position it cannot serve.
//!
//! ⚠️ **A fetch *at* the watermark issues zero GETs** — FR-12, and the reason
//! it holds is that the index names no batch there, so the read path's loop
//! never runs. The idle poll costs one index lookup and no object storage.
//!
//! Sessions (KIP-227) are declined, which a broker may always do: every
//! response carries `session_id` 0, so clients fall back to full fetches.
//!
//! ⚠️ ~~**`max_wait_ms` and `min_bytes` are still parsed and ignored**~~ —
//! **wired at `M3.20`** (`M3.md` task 17). A fetch with nothing to return
//! parks on the coordinator's index rather than answering empty, so a consumer
//! polling an idle partition costs one wakeup instead of a request per
//! interval, and a fetch racing a concurrent produce sees it. ⚠️ **No new
//! semantics**: the deadline is the client's `max_wait_ms` and the timer is
//! the client's too — the broker contributes a wakeup, not a poll interval.

mod park;
mod partition;
mod target;

use crate::cluster::Cluster;
use crate::connection::HandlerResponse;
use oqueue_codec::apikey::ApiKey;
use oqueue_codec::error_codes;
use oqueue_codec::fetch::{
    FetchResponse, FetchResponsePartition, FetchResponseTopic, decode_request,
};
use oqueue_codec::frame::{RequestPrelude, encode_response_header};
pub use park::MAX_PARK_MS;
use park::read_or_park;
use partition::{PartitionOutcome, one_partition};

/// One topic's response, owned so the borrowed [`FetchResponseTopic`] can
/// point into its echoed name and its concatenated batch bytes.
struct TopicOutcome {
    name: Option<String>,
    topic_id: [u8; 16],
    partitions: Vec<PartitionOutcome>,
}

/// Decodes, resolves, reads, answers — or closes on a malformed body.
pub(crate) async fn handle(
    cluster: &Cluster,
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

    let outcomes = read_or_park(cluster, &request, version).await;

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

/// The topic this entry names, whichever way this version addresses topics.
///
/// From v13 the wire carries an id and this resolves it against the registry;
/// below that it carries the name itself.
fn resolved_name(
    cluster: &Cluster,
    topic: &oqueue_codec::fetch::FetchTopic<'_>,
    version: i16,
) -> Option<String> {
    if version >= 13 {
        cluster.topic_name_by_id(uuid::Uuid::from_bytes(topic.topic_id))
    } else {
        topic.name.map(str::to_owned)
    }
}

/// One topic's outcome: resolve its addressing, then every partition.
async fn one_topic(
    cluster: &Cluster,
    topic: &oqueue_codec::fetch::FetchTopic<'_>,
    version: i16,
) -> TopicOutcome {
    // From v13 the wire addresses topics by id — same split as produce:
    // echo what this version carries, resolve the rest.
    // ⚠️ At v13 the wire carries no name to echo, so the response repeats the
    // id and the name is resolved only to find the partition.
    let name = if version >= 13 {
        None
    } else {
        topic.name.map(str::to_owned)
    };
    let resolved_name = resolved_name(cluster, topic, version);
    // An id this broker never issued has its own error (100); a name it
    // does not host stays UNKNOWN_TOPIC_OR_PARTITION, matching what real
    // brokers answer on each addressing path.
    let unknown = if version >= 13 {
        error_codes::UNKNOWN_TOPIC_ID
    } else {
        error_codes::UNKNOWN_TOPIC_OR_PARTITION
    };
    let mut partitions = Vec::with_capacity(topic.partitions.len());
    for p in &topic.partitions {
        partitions.push(
            one_partition(
                cluster,
                resolved_name.as_deref(),
                unknown,
                p.index,
                p.fetch_offset,
            )
            .await,
        );
    }
    TopicOutcome {
        name,
        topic_id: topic.topic_id,
        partitions,
    }
}

#[cfg(test)]
pub(crate) mod tests {
    #![allow(clippy::expect_used)]
    #![allow(clippy::redundant_pub_crate)]

    use super::handle;
    use crate::connection::HandlerResponse;
    use crate::testing::{Fixture, fixture, golden_batch, produce_one};
    use kafka_protocol::messages::fetch_request::{FetchPartition, FetchTopic};
    use kafka_protocol::messages::{FetchRequest, FetchResponse, TopicName};
    use kafka_protocol::protocol::{Decodable, Encodable, StrBytes};
    use oqueue_codec::frame::RequestPrelude;

    /// ⚠️ **`max_wait_ms` of zero**, so every case using this asserts the
    /// *immediate* answer and none of them sleeps. The park has its own tests
    /// under `start_paused`; a suite where each fetch case waited half a second
    /// would be one nobody runs (`testing.md`'s budget).
    pub(crate) fn fetch_body(
        version: i16,
        t: FetchTopic,
        fetch_offset: i64,
        isolation: i8,
    ) -> Vec<u8> {
        hungry_fetch_body(
            version,
            t,
            fetch_offset,
            isolation,
            Poll {
                max_wait_ms: 0,
                min_bytes: 1,
            },
        )
    }

    /// The same, with the client's `max_wait_ms` chosen and `min_bytes` at
    /// **1** — every real client's default, and the value that makes a park
    /// happen at all. ⚠️ `min_bytes = 0` means "answer now, empty is fine",
    /// which `hungry_fetch_body` and its own test cover.
    pub(crate) fn waiting_fetch_body(
        version: i16,
        t: FetchTopic,
        fetch_offset: i64,
        isolation: i8,
        max_wait_ms: i32,
    ) -> Vec<u8> {
        hungry_fetch_body(
            version,
            t,
            fetch_offset,
            isolation,
            Poll {
                max_wait_ms,
                min_bytes: 1,
            },
        )
    }

    /// What a client asks the broker to wait for.
    #[derive(Debug, Clone, Copy)]
    pub(crate) struct Poll {
        pub(crate) max_wait_ms: i32,
        pub(crate) min_bytes: i32,
    }

    /// The same again, with the whole long-poll shape chosen.
    pub(crate) fn hungry_fetch_body(
        version: i16,
        t: FetchTopic,
        fetch_offset: i64,
        isolation: i8,
        poll: Poll,
    ) -> Vec<u8> {
        let mut request = FetchRequest::default();
        request.max_wait_ms = poll.max_wait_ms;
        request.min_bytes = poll.min_bytes;
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

    /// The topic this fixture hosts, addressed by id.
    pub(crate) fn by_id(fixture: &Fixture) -> FetchTopic {
        by_id_of(hosted(fixture))
    }

    pub(crate) fn by_id_of(id: uuid::Uuid) -> FetchTopic {
        let mut t = FetchTopic::default();
        t.topic_id = id;
        t
    }

    pub(crate) fn prelude(version: i16) -> RequestPrelude {
        RequestPrelude {
            api_key: 1,
            api_version: version,
            correlation_id: 5,
        }
    }

    pub(crate) fn decode(bytes: &[u8], version: i16) -> FetchResponse {
        // Fetch goes flexible (tagged response header) at v12.
        let header_len = if version >= 12 { 5 } else { 4 };
        let mut rest = &bytes[header_len..];
        let r = FetchResponse::decode(&mut rest, version).expect("decodes");
        assert!(rest.is_empty());
        r
    }

    pub(crate) async fn replied(fixture: &Fixture, version: i16, body: &[u8]) -> FetchResponse {
        let HandlerResponse::Reply(out) = handle(&fixture.cluster, prelude(version), body).await
        else {
            panic!("a fetch with a legal isolation level replies");
        };
        decode(&out, version)
    }

    /// ⚠️ **From v13 the wire addresses topics by id**, so a v13 request built
    /// with a name reaches no topic — the refusal a client would get, not the
    /// one a test means to exercise.
    pub(crate) fn hosted(fixture: &Fixture) -> uuid::Uuid {
        fixture.cluster.topic_id("t").expect("the fixture's topic")
    }

    /// A cluster with one produced batch: the produce path is the fixture, so
    /// what fetch returns is exactly what produce stored.
    pub(crate) async fn produced() -> Fixture {
        let fixture = fixture(&["t"]).await;
        produce_one(&fixture, "t", golden_batch()).await;
        fixture
    }

    #[tokio::test]
    async fn a_fetch_from_zero_returns_the_batch_and_the_derived_watermark() {
        let fixture = produced().await;
        let response = replied(&fixture, 13, &fetch_body(13, by_id(&fixture), 0, 0)).await;
        let p = &response.responses[0].partitions[0];
        assert_eq!(p.error_code, 0);
        assert_eq!(p.high_watermark, 2, "two records were produced");
        assert_eq!(
            p.last_stable_offset, p.high_watermark,
            "H3: the stable offset is never ahead of the watermark"
        );
        let records = p.records.as_ref().expect("records rode along");
        assert!(!records.is_empty());
    }

    /// ⚠️ **The stamp**: the bytes in object storage carry whatever base offset
    /// the producer sent, because they were written before the commit assigned
    /// one. What comes back has the assigned offset, and still verifies —
    /// the CRC never covered those twelve bytes (doc 18 §4.4).
    #[tokio::test]
    async fn a_fetched_batch_carries_its_assigned_offset_and_still_verifies() {
        let fixture = fixture(&["t"]).await;
        produce_one(&fixture, "t", golden_batch()).await;
        produce_one(&fixture, "t", golden_batch()).await;

        let response = replied(&fixture, 13, &fetch_body(13, by_id(&fixture), 2, 0)).await;

        let records = response.responses[0].partitions[0]
            .records
            .as_ref()
            .expect("records");
        let header = oqueue_codec::batch::decode_batch_header(records).expect("a batch came back");
        assert_eq!(header.base_offset, 2, "the second produce's assignment");
        let coverage = oqueue_codec::batch::crc_coverage(records).expect("spans");
        assert_eq!(
            oqueue_checksum::crc32c(coverage),
            oqueue_codec::batch::stored_crc(records).expect("crc"),
            "the stamp needed no recompute"
        );
    }

    #[tokio::test]
    async fn an_offset_past_the_watermark_is_out_of_range_with_the_real_watermark() {
        let fixture = produced().await;
        let response = replied(&fixture, 13, &fetch_body(13, by_id(&fixture), 99, 0)).await;
        let p = &response.responses[0].partitions[0];
        assert_eq!(
            p.error_code,
            kafka_protocol::error::ResponseError::OffsetOutOfRange.code()
        );
        assert_eq!(p.high_watermark, 2, "the client hears where the log is");
    }

    #[tokio::test]
    async fn an_unknown_topic_is_refused_by_the_addressing_the_request_used() {
        let fixture = fixture(&[]).await;
        let by_name_response = replied(&fixture, 11, &fetch_body(11, by_name("ghost"), 0, 0)).await;
        assert_eq!(
            by_name_response.responses[0].partitions[0].error_code,
            kafka_protocol::error::ResponseError::UnknownTopicOrPartition.code()
        );
        let ghost = uuid::Uuid::from_u128(0xBEEF);
        let by_id_response = replied(&fixture, 13, &fetch_body(13, by_id_of(ghost), 0, 0)).await;
        assert_eq!(
            by_id_response.responses[0].partitions[0].error_code,
            kafka_protocol::error::ResponseError::UnknownTopicId.code(),
            "ids have their own refusal"
        );
    }

    #[tokio::test]
    async fn an_undefined_isolation_level_closes_the_connection() {
        let fixture = fixture(&["t"]).await;
        assert!(matches!(
            handle(
                &fixture.cluster,
                prelude(13),
                &fetch_body(13, by_id(&fixture), 0, 7)
            )
            .await,
            HandlerResponse::Close
        ));
    }

    /// v4: the oldest version advertised, legacy headers both ways.
    #[tokio::test]
    async fn the_nonflexible_low_versions_answer_too() {
        let fixture = produced().await;
        let response = replied(&fixture, 4, &fetch_body(4, by_name("t"), 0, 0)).await;
        let p = &response.responses[0].partitions[0];
        assert_eq!(p.error_code, 0);
        assert_eq!(p.high_watermark, 2);
    }
}
