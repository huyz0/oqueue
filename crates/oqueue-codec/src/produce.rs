//! `Produce` v3-13 request decode and response encode, hand-rolled
//! (`ADR-0019`).
//!
//! The hot path's request side.
//!
//! Decoding is zero-copy where it matters: each partition's `records` blob is
//! handed back as a slice of the caller's buffer, never copied here — the
//! handler verifies its CRC and rewrites its base offset in place (doc 18
//! §4.4). Topics are addressed by name below v13 and by id from v13, the
//! array/string encodings go flexible at v9, and `records` is nullable bytes.
//! Byte-differentialed against `kafka-protocol` below.
//!
//! ⚠️ **Topic ids are `[u8; 16]`.** The codec deals in bytes; the broker owns
//! the `Uuid`.

use crate::flex::{
    TaggedFields, put_array_len, put_nullable_string, put_string, put_tagged_fields,
    read_array_len, read_nullable_string, read_tagged_fields,
};
use crate::metadata::{TopicId, read_topic_id};
use crate::wire::{Cursor, DecodeError, put_i16, put_i32, put_i64};

/// The fields of a `Produce` request this broker acts on.
///
/// The ack mode and the per-topic, per-partition record blobs.
/// `transactional_id` and `timeout_ms` are decoded past but not kept —
/// nothing downstream reads them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProduceRequest<'a> {
    /// The ack mode: `-1` all, `0` none, `1` leader.
    pub acks: i16,
    /// The topics produced to.
    pub topics: Vec<ProduceTopic<'a>>,
}

/// One topic in a `Produce` request: by name below v13, by id from v13.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProduceTopic<'a> {
    /// The topic name, or `None` (v13+ addresses by id).
    pub name: Option<String>,
    /// The topic id, nil below v13.
    pub topic_id: TopicId,
    /// The partitions produced to.
    pub partitions: Vec<ProducePartition<'a>>,
}

/// One partition in a `Produce` request: an index and its opaque record
/// batch bytes, borrowed from the request buffer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProducePartition<'a> {
    /// The partition index.
    pub index: i32,
    /// The record batch bytes, or `None` — verified and stored by the
    /// handler, never decoded here.
    pub records: Option<&'a [u8]>,
}

/// Decodes a `Produce` request, borrowing each partition's records from
/// `body`.
///
/// # Errors
/// [`DecodeError`] on any malformed length or truncation; every count and
/// length is bounded against the input before anything is grown.
pub fn decode_request(body: &[u8], version: i16) -> Result<ProduceRequest<'_>, DecodeError> {
    let flexible = version >= 9;
    let mut cur = Cursor::new(body);

    // transactional_id (nullable string) — decoded past, not kept.
    let _ = read_nullable_string(&mut cur, flexible)?;
    let acks = cur.read_i16()?;
    let _timeout_ms = cur.read_i32()?;

    let topic_count = read_array_len(&mut cur, flexible)?.unwrap_or(0);
    let mut topics = Vec::new();
    for _ in 0..topic_count {
        let name = if version <= 12 {
            read_nullable_string(&mut cur, flexible)?.map(str::to_owned)
        } else {
            None
        };
        let topic_id = if version >= 13 {
            read_topic_id(&mut cur)?
        } else {
            [0u8; 16]
        };
        let partition_count = read_array_len(&mut cur, flexible)?.unwrap_or(0);
        let mut partitions = Vec::new();
        for _ in 0..partition_count {
            let index = cur.read_i32()?;
            let records = read_nullable_bytes(&mut cur, flexible)?;
            if flexible {
                let _: TaggedFields = read_tagged_fields(&mut cur)?;
            }
            partitions.push(ProducePartition { index, records });
        }
        if flexible {
            let _: TaggedFields = read_tagged_fields(&mut cur)?;
        }
        topics.push(ProduceTopic {
            name,
            topic_id,
            partitions,
        });
    }

    Ok(ProduceRequest { acks, topics })
}

/// Nullable bytes, compact when `flexible` else legacy (`i32` length, `-1`
/// null), bounded against the input before the slice is taken.
fn read_nullable_bytes<'a>(
    cur: &mut Cursor<'a>,
    flexible: bool,
) -> Result<Option<&'a [u8]>, DecodeError> {
    if flexible {
        crate::flex::read_compact_nullable_bytes(cur)
    } else {
        cur.read_nullable_length_prefixed(u64::from(u32::MAX))
    }
}

/// The data a `Produce` response carries: one entry per requested topic and
/// partition, with the outcome the handler assigned.
#[derive(Debug, Clone)]
pub struct ProduceResponse<'a> {
    /// One entry per topic, in request order.
    pub topics: Vec<ProduceResponseTopic<'a>>,
}

/// One topic in a `Produce` response.
#[derive(Debug, Clone)]
pub struct ProduceResponseTopic<'a> {
    /// The topic name, echoed below v13.
    pub name: Option<&'a str>,
    /// The topic id, echoed from v13.
    pub topic_id: TopicId,
    /// The per-partition outcomes.
    pub partitions: Vec<ProduceResponsePartition>,
}

/// One partition's outcome in a `Produce` response.
#[derive(Debug, Clone, Copy)]
pub struct ProduceResponsePartition {
    /// The partition index.
    pub index: i32,
    /// The error code (0 = ok).
    pub error_code: i16,
    /// The base offset assigned, or `-1` on error.
    pub base_offset: i64,
}

/// `LogAppendTime` is not used by this broker; `-1` is the protocol's
/// "not set" value.
const LOG_APPEND_TIME_UNSET: i64 = -1;

/// Appends a `Produce` response body at `version`.
///
/// The response header is [`crate::frame::encode_response_header`]'s job.
/// Never called for `acks == 0` — that is fire-and-forget, and the handler
/// stays silent.
pub fn encode_response(out: &mut Vec<u8>, version: i16, resp: &ProduceResponse<'_>) {
    let flexible = version >= 9;
    let empty = TaggedFields::default();

    put_array_len(out, flexible, Some(resp.topics.len()));
    for topic in &resp.topics {
        if version <= 12 {
            // ⚠️ **Non-nullable in the response schema** (`M3.40`), so
            // the writer is the one that cannot express a null. What
            // reaches here is always `Some`, because the handler refuses a
            // null request name — three handlers had to learn that
            // separately. If one ever forgets, this frames a topic no
            // client will match rather than a body no client can read.
            put_string(out, flexible, topic.name.unwrap_or_default());
        }
        if version >= 13 {
            out.extend_from_slice(&topic.topic_id);
        }

        put_array_len(out, flexible, Some(topic.partitions.len()));
        for p in &topic.partitions {
            put_i32(out, p.index);
            put_i16(out, p.error_code);
            put_i64(out, p.base_offset);
            put_i64(out, LOG_APPEND_TIME_UNSET);
            if version >= 5 {
                put_i64(out, 0); // log_start_offset
            }
            if version >= 8 {
                put_array_len(out, flexible, Some(0)); // record_errors (empty)
                put_nullable_string(out, flexible, None); // error_message
            }
            if flexible {
                put_tagged_fields(out, &empty);
            }
        }

        if flexible {
            put_tagged_fields(out, &empty);
        }
    }

    put_i32(out, 0); // throttle_time_ms
    if flexible {
        // Top-level tagged fields: `node_endpoints` (v10+, KIP-951) rides
        // here, but only when non-empty — a single-node broker sends none.
        put_tagged_fields(out, &empty);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{
        ProduceResponse, ProduceResponsePartition, ProduceResponseTopic, decode_request,
        encode_response,
    };

    const VERSIONS: [i16; 11] = [3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13];

    /// The `ADR-0019` oracle: our request decoder reads what the dependency's
    /// encoder wrote — acks, the topic addressing, and each partition's
    /// records blob byte-for-byte — at every advertised version.
    #[test]
    fn our_request_decode_matches_the_dependency() {
        use kafka_protocol::messages::produce_request::{PartitionProduceData, TopicProduceData};
        use kafka_protocol::messages::{ProduceRequest as KpRequest, TopicName};
        use kafka_protocol::protocol::{Encodable, StrBytes};

        let records = b"\x01\x02\x03opaque-batch-bytes";
        for version in VERSIONS {
            let mut kp = KpRequest::default();
            kp.acks = -1;
            let mut topic = TopicProduceData::default();
            if version >= 13 {
                topic.topic_id = uuid::Uuid::from_u128(0x1234);
            } else {
                topic.name = TopicName(StrBytes::from_static_str("t"));
            }
            let mut partition = PartitionProduceData::default();
            partition.index = 2;
            partition.records = Some(bytes::Bytes::from_static(records));
            topic.partition_data.push(partition);
            kp.topic_data.push(topic);
            let mut bytes = Vec::new();
            kp.encode(&mut bytes, version).expect("dependency encodes");

            let ours = decode_request(&bytes, version).expect("ours decodes");
            assert_eq!(ours.acks, -1, "v{version}");
            assert_eq!(ours.topics.len(), 1, "v{version}");
            assert_eq!(ours.topics[0].partitions.len(), 1, "v{version}");
            let part = &ours.topics[0].partitions[0];
            assert_eq!(part.index, 2, "v{version}");
            assert_eq!(
                part.records,
                Some(&records[..]),
                "v{version}: records verbatim"
            );
            if version >= 13 {
                assert_eq!(
                    ours.topics[0].topic_id,
                    uuid::Uuid::from_u128(0x1234).into_bytes(),
                    "v{version}"
                );
            } else {
                assert_eq!(ours.topics[0].name.as_deref(), Some("t"), "v{version}");
            }
        }
    }

    /// The `ADR-0019` oracle: our response bytes decode under the dependency's
    /// `ProduceResponse` at every version, carrying the assigned offset.
    #[test]
    fn our_response_decodes_under_the_dependency() {
        use kafka_protocol::messages::ProduceResponse as KpResponse;
        use kafka_protocol::protocol::Decodable;

        let resp = ProduceResponse {
            topics: vec![ProduceResponseTopic {
                name: Some("t"),
                topic_id: [9u8; 16],
                partitions: vec![ProduceResponsePartition {
                    index: 2,
                    error_code: 0,
                    base_offset: 42,
                }],
            }],
        };

        for version in VERSIONS {
            let mut out = Vec::new();
            encode_response(&mut out, version, &resp);
            let mut cursor = &out[..];
            let decoded = KpResponse::decode(&mut cursor, version)
                .unwrap_or_else(|e| panic!("v{version}: oracle decode failed: {e}"));
            assert!(cursor.is_empty(), "v{version}: whole body consumed");
            let part = &decoded.responses[0].partition_responses[0];
            assert_eq!(part.index, 2, "v{version}");
            assert_eq!(part.error_code, 0, "v{version}");
            assert_eq!(part.base_offset, 42, "v{version}");
        }
    }
}
