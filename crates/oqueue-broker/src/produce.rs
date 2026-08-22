//! `Produce` v3-13 against the stub: verify, assign, append, answer.
//!
//! The write path in miniature, in exactly the shape doc 18 §4.4 designed
//! the batch layout for: a partition's records must be **exactly one
//! batch** — v3+ produce carries one per partition, so a trailing byte or
//! a second batch is `INVALID_RECORD`, never stored unverified — whose
//! CRC is verified through the one-line ingest rule
//! (`crc32c(crc_coverage(b)) == stored_crc(b)` — `M2.19`'s composition,
//! with `oqueue-checksum` joining this crate on schedule), the stub
//! assigns a base offset, and the assignment is a twelve-byte in-place
//! rewrite that recomputes nothing.
//!
//! ⚠️ **`acks=0` sends no response at all** — the protocol's fire-and-forget
//! mode, and the case that forced the [`crate::Handler`] seam's final
//! shape: answering it would desync the client's read stream, so the
//! outcome is `Silent`, not an empty reply.

// The test module is pub(crate) so fetch's tests can share the golden
// fixtures; clippy calls the inner pub(crate) redundant while
// `unreachable_pub` refuses the alternative -- codec's batch.rs precedent.
#![allow(clippy::redundant_pub_crate)]

use crate::connection::HandlerResponse;
use crate::stub::StubCluster;
use kafka_protocol::error::ResponseError;
use kafka_protocol::messages::produce_response::{PartitionProduceResponse, TopicProduceResponse};
use kafka_protocol::messages::{ProduceRequest, ProduceResponse};
use kafka_protocol::protocol::{Decodable, Encodable};
use oqueue_codec::apikey::ApiKey;
use oqueue_codec::batch::{crc_coverage, decode_batch_header, rewrite_base_offset, stored_crc};
use oqueue_codec::frame::{RequestPrelude, encode_response_header};

/// Decodes, verifies, appends, answers — or stays silent for `acks=0`.
pub(crate) fn handle(
    cluster: &StubCluster,
    prelude: RequestPrelude,
    body: &[u8],
) -> HandlerResponse {
    let mut buf = body;
    let Ok(request) = ProduceRequest::decode(&mut buf, prelude.api_version) else {
        return HandlerResponse::Close;
    };

    // The protocol's three acks: 0, 1, -1. Anything else is answered
    // INVALID_REQUIRED_ACKS per partition with nothing stored.
    let acks_valid = matches!(request.acks, -1..=1);

    let mut response = ProduceResponse::default();
    for topic in &request.topic_data {
        let mut topic_response = TopicProduceResponse::default();
        // From v13 the wire addresses topics by id, not name — echo what
        // the wire carries at this version (the encoder refuses the other
        // field), and resolve the id against the stub's registry.
        let resolved_name = if prelude.api_version >= 13 {
            topic_response.topic_id = topic.topic_id;
            cluster.topic_name_by_id(topic.topic_id)
        } else {
            topic_response.name = topic.name.clone();
            Some(topic.name.to_string())
        };
        for partition in &topic.partition_data {
            let entry = match (&resolved_name, acks_valid) {
                (_, false) => refused(partition.index, ResponseError::InvalidRequiredAcks),
                // An id this broker never issued: its own error (100),
                // mapped back through the echoed id — the name path's
                // UNKNOWN_TOPIC_OR_PARTITION stays in one_partition.
                (None, true) => refused(partition.index, ResponseError::UnknownTopicId),
                (Some(name), true) => {
                    one_partition(cluster, name, partition.index, partition.records.as_deref())
                }
            };
            topic_response.partition_responses.push(entry);
        }
        response.responses.push(topic_response);
    }

    if request.acks == 0 {
        return HandlerResponse::Silent;
    }
    let mut out = Vec::new();
    if encode_response_header(
        &mut out,
        ApiKey::Produce,
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

/// A partition entry carrying only a refusal.
fn refused(index: i32, error: ResponseError) -> PartitionProduceResponse {
    let mut p = PartitionProduceResponse::default();
    p.index = index;
    p.base_offset = -1;
    p.error_code = error.code();
    p
}

/// One partition's verdict: the ingest rule, then assignment.
fn one_partition(
    cluster: &StubCluster,
    topic: &str,
    index: i32,
    records: Option<&[u8]>,
) -> PartitionProduceResponse {
    let partition = usize::try_from(index).unwrap_or(usize::MAX);
    if cluster
        .partition_count(topic)
        .is_none_or(|n| partition >= n)
    {
        return refused(index, ResponseError::UnknownTopicOrPartition);
    }
    let Some(records) = records else {
        // A produce with no records is a client bug; nothing to store.
        return refused(index, ResponseError::InvalidRecord);
    };

    let batch = records.to_vec();
    let header = match decode_batch_header(&batch) {
        Ok(h) => h,
        // v0/v1 batches are refused, never converted — KIP-110's own
        // precedent, and the error names the format problem rather than
        // steering the client at a compression setting (`M2.md`'s risks).
        Err(oqueue_codec::batch::BatchError::WrongMagic { .. }) => {
            return refused(index, ResponseError::UnsupportedForMessageFormat);
        }
        Err(_) => return refused(index, ResponseError::CorruptMessage),
    };
    // Exactly one batch, whole: the header's declared span must be the
    // blob. Longer is a second batch or trailing garbage — bytes the CRC
    // below would never cover, refused rather than stored unverified
    // (real brokers answer multi-batch v3+ produce the same way).
    // Shorter never reaches here: `decode_batch_header` needs 61 bytes and
    // `crc_coverage` refuses a declared end past the blob.
    let declared = usize::try_from(header.batch_length)
        .ok()
        .and_then(|l| l.checked_add(12));
    if declared != Some(batch.len()) {
        return refused(index, ResponseError::InvalidRecord);
    }
    // The ingest rule — one line, exactly as `M2.19` shaped it.
    let verified = crc_coverage(&batch)
        .ok()
        .zip(stored_crc(&batch).ok())
        .is_some_and(|(coverage, stored)| oqueue_checksum::crc32c(coverage) == stored);
    if !verified {
        return refused(index, ResponseError::CorruptMessage);
    }

    let records_in_batch = i64::from(header.record_count.max(1));
    // Assignment is the twelve-byte rewrite, done inside the reservation's
    // critical section so concurrent produces cannot stamp each other's
    // offsets; the CRC needs nothing (doc 18 §4.4).
    let Some(base) = cluster.append_with(topic, partition, records_in_batch, |base| {
        let mut batch = batch;
        // Infallible here: decode_batch_header above proved the 61-byte
        // header is present, which is all the rewrite touches.
        let _ = rewrite_base_offset(&mut batch, base, 0);
        batch
    }) else {
        return refused(index, ResponseError::UnknownTopicOrPartition);
    };
    let mut p = PartitionProduceResponse::default();
    p.index = index;
    p.base_offset = base;
    p
}

#[cfg(test)]
pub(crate) mod tests {
    #![allow(clippy::expect_used)]

    use super::handle;
    use crate::connection::HandlerResponse;
    use crate::stub::StubCluster;
    use kafka_protocol::messages::produce_request::{PartitionProduceData, TopicProduceData};
    use kafka_protocol::messages::{ProduceRequest, ProduceResponse, TopicName};
    use kafka_protocol::protocol::{Decodable, Encodable, StrBytes};
    use oqueue_codec::frame::RequestPrelude;

    pub(crate) fn produce_body(version: i16, topic: &str, acks: i16, records: Vec<u8>) -> Vec<u8> {
        let mut t = TopicProduceData::default();
        t.name = TopicName(StrBytes::from_string(topic.to_owned()));
        body_for(version, t, acks, records)
    }

    /// v13's addressing: the topic id, no name (the encoder refuses one).
    fn produce_body_by_id(version: i16, id: uuid::Uuid, acks: i16, records: Vec<u8>) -> Vec<u8> {
        let mut t = TopicProduceData::default();
        t.topic_id = id;
        body_for(version, t, acks, records)
    }

    fn body_for(version: i16, mut t: TopicProduceData, acks: i16, records: Vec<u8>) -> Vec<u8> {
        let mut request = ProduceRequest::default();
        request.acks = acks;
        let mut p = PartitionProduceData::default();
        p.index = 0;
        p.records = Some(bytes::Bytes::from(records));
        t.partition_data.push(p);
        request.topic_data.push(t);
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

    /// The dependency's own `RecordBatchEncoder` producing a two-record
    /// batch — the same authority `oqueue-codec`'s golden tests use
    /// (`ADR-0017`), rebuilt here because a `#[cfg(test)]` helper is
    /// invisible across crates.
    pub(crate) fn golden_batch() -> Vec<u8> {
        use kafka_protocol::records::{
            Compression, Record, RecordBatchEncoder, RecordEncodeOptions, TimestampType,
        };
        // offset - sequence must match across records or the encoder
        // splits them into separate batches (its grouping predicate).
        fn record(offset: i64, value: &'static [u8]) -> Record {
            Record {
                transactional: false,
                control: false,
                partition_leader_epoch: 0,
                producer_id: -1,
                producer_epoch: -1,
                timestamp_type: TimestampType::Creation,
                offset,
                sequence: i32::try_from(offset).unwrap_or(0),
                delete_horizon: false,
                timestamp: 1_700_000_000_000 + offset,
                key: None,
                value: Some(bytes::Bytes::from_static(value)),
                headers: kafka_protocol::indexmap::IndexMap::default(),
            }
        }
        let records = vec![record(0, b"hello"), record(1, b"world")];
        let mut buf = bytes::BytesMut::new();
        RecordBatchEncoder::encode(
            &mut buf,
            &records,
            &RecordEncodeOptions {
                version: 2,
                compression: Compression::None,
            },
        )
        .expect("the dependency encodes its own records");
        buf.to_vec()
    }

    #[test]
    fn a_valid_batch_is_verified_assigned_and_stored_rewritten() {
        let cluster = StubCluster::new("h", 1);
        cluster.ensure_topic("t");
        // Two batches: offsets advance by record count.
        for expected_base in [0i64, 2] {
            let body = produce_body(9, "t", -1, golden_batch());
            let HandlerResponse::Reply(out) = handle(&cluster, prelude(9), &body) else {
                panic!("acks=-1 replies");
            };
            let response = decode(&out, 9);
            let p = &response.responses[0].partition_responses[0];
            assert_eq!(p.error_code, 0);
            assert_eq!(p.base_offset, expected_base);
        }
        // The stored batch carries the assigned offset and still verifies.
        let (batches, high) = cluster.read("t", 0, 0).expect("exists");
        assert_eq!(high, 4);
        let stored = &batches[1];
        let header = oqueue_codec::batch::decode_batch_header(stored).expect("stored decodes");
        assert_eq!(header.base_offset, 2);
        let coverage = oqueue_codec::batch::crc_coverage(stored).expect("spans");
        assert_eq!(
            oqueue_checksum::crc32c(coverage),
            oqueue_codec::batch::stored_crc(stored).expect("crc"),
            "the rewrite needed no recompute"
        );
    }

    #[test]
    fn a_corrupt_crc_is_refused_and_nothing_is_stored() {
        let cluster = StubCluster::new("h", 1);
        cluster.ensure_topic("t");
        let mut bad = golden_batch();
        let last = bad.len() - 1;
        bad[last] ^= 0xFF;
        let body = produce_body(9, "t", -1, bad);
        let HandlerResponse::Reply(out) = handle(&cluster, prelude(9), &body) else {
            panic!("replies");
        };
        let response = decode(&out, 9);
        assert_eq!(
            response.responses[0].partition_responses[0].error_code,
            kafka_protocol::error::ResponseError::CorruptMessage.code()
        );
        let (batches, _) = cluster.read("t", 0, 0).expect("exists");
        assert!(batches.is_empty(), "a refused batch is not stored");
    }

    #[test]
    fn an_old_format_batch_names_the_format_problem() {
        let cluster = StubCluster::new("h", 1);
        cluster.ensure_topic("t");
        let mut old = golden_batch();
        old[16] = 1; // magic v1
        let body = produce_body(9, "t", -1, old);
        let HandlerResponse::Reply(out) = handle(&cluster, prelude(9), &body) else {
            panic!("replies");
        };
        let response = decode(&out, 9);
        assert_eq!(
            response.responses[0].partition_responses[0].error_code,
            kafka_protocol::error::ResponseError::UnsupportedForMessageFormat.code()
        );
    }

    #[test]
    fn an_unknown_topic_is_that_partitions_error_not_a_close() {
        let cluster = StubCluster::new("h", 1);
        let body = produce_body(9, "ghost", -1, golden_batch());
        let HandlerResponse::Reply(out) = handle(&cluster, prelude(9), &body) else {
            panic!("replies");
        };
        let response = decode(&out, 9);
        assert_eq!(
            response.responses[0].partition_responses[0].error_code,
            kafka_protocol::error::ResponseError::UnknownTopicOrPartition.code()
        );
    }

    #[test]
    fn a_trailing_byte_or_second_batch_is_refused_whole() {
        let cluster = StubCluster::new("h", 1);
        cluster.ensure_topic("t");
        let mut trailing = golden_batch();
        trailing.push(0xAB);
        let mut doubled = golden_batch();
        doubled.extend_from_slice(&golden_batch());
        for blob in [trailing, doubled] {
            let body = produce_body(9, "t", -1, blob);
            let HandlerResponse::Reply(out) = handle(&cluster, prelude(9), &body) else {
                panic!("replies");
            };
            let response = decode(&out, 9);
            assert_eq!(
                response.responses[0].partition_responses[0].error_code,
                kafka_protocol::error::ResponseError::InvalidRecord.code(),
                "bytes the CRC never covered must not be stored"
            );
        }
        let (batches, _) = cluster.read("t", 0, 0).expect("exists");
        assert!(batches.is_empty(), "nothing landed");
    }

    #[test]
    fn invalid_acks_is_refused_and_stores_nothing() {
        let cluster = StubCluster::new("h", 1);
        cluster.ensure_topic("t");
        let body = produce_body(9, "t", 2, golden_batch());
        let HandlerResponse::Reply(out) = handle(&cluster, prelude(9), &body) else {
            panic!("acks=2 is not fire-and-forget; it is an error reply");
        };
        let response = decode(&out, 9);
        assert_eq!(
            response.responses[0].partition_responses[0].error_code,
            kafka_protocol::error::ResponseError::InvalidRequiredAcks.code()
        );
        let (batches, _) = cluster.read("t", 0, 0).expect("exists");
        assert!(batches.is_empty());
    }

    #[test]
    fn v13_addresses_the_topic_by_id_and_echoes_it() {
        let cluster = StubCluster::new("h", 1);
        cluster.ensure_topic("t");
        let id = cluster.topic_id("t").expect("an id");
        let body = produce_body_by_id(13, id, -1, golden_batch());
        let HandlerResponse::Reply(out) = handle(&cluster, prelude(13), &body) else {
            panic!("replies");
        };
        let response = decode(&out, 13);
        assert_eq!(response.responses[0].topic_id, id, "the id is the echo");
        let p = &response.responses[0].partition_responses[0];
        assert_eq!(p.error_code, 0);
        assert_eq!(p.base_offset, 0);
        let (batches, _) = cluster.read("t", 0, 0).expect("exists");
        assert_eq!(batches.len(), 1);
    }

    #[test]
    fn v13_with_an_unknown_id_refuses_per_partition_echoing_the_id() {
        let cluster = StubCluster::new("h", 1);
        let ghost = uuid::Uuid::from_u128(0xDEAD);
        let body = produce_body_by_id(13, ghost, -1, golden_batch());
        let HandlerResponse::Reply(out) = handle(&cluster, prelude(13), &body) else {
            panic!("replies");
        };
        let response = decode(&out, 13);
        assert_eq!(response.responses[0].topic_id, ghost);
        assert_eq!(
            response.responses[0].partition_responses[0].error_code,
            kafka_protocol::error::ResponseError::UnknownTopicId.code(),
            "ids have their own refusal, echoed through the id"
        );
    }

    #[test]
    fn the_nonflexible_low_versions_answer_too() {
        // v3: legacy header, v0 response header -- the half librdkafka
        // will not negotiate, pinned here instead.
        let cluster = StubCluster::new("h", 1);
        cluster.ensure_topic("t");
        let body = produce_body(3, "t", -1, golden_batch());
        let HandlerResponse::Reply(out) = handle(&cluster, prelude(3), &body) else {
            panic!("replies");
        };
        let response = decode(&out, 3);
        let p = &response.responses[0].partition_responses[0];
        assert_eq!(p.error_code, 0);
        assert_eq!(p.base_offset, 0);
    }

    #[test]
    fn acks_zero_is_silent_but_still_stores() {
        let cluster = StubCluster::new("h", 1);
        cluster.ensure_topic("t");
        let body = produce_body(9, "t", 0, golden_batch());
        assert!(matches!(
            handle(&cluster, prelude(9), &body),
            HandlerResponse::Silent
        ));
        let (batches, high) = cluster.read("t", 0, 0).expect("exists");
        assert_eq!(batches.len(), 1, "fire-and-forget still lands");
        assert_eq!(high, 2);
    }
}
