//! `Fetch` v4-17 request decode and response encode, hand-rolled
//! (`ADR-0019`).
//!
//! ⚠️ **This is the message whose owning by a dependency caused the `DoS`
//! `M2.26` found.** `kafka-protocol`'s generated array decoder allocated a
//! `Vec` from an untrusted element count with no bound against the input —
//! a 64-byte Fetch v16 frame demanded ~30 GB. Every array and string here
//! goes through [`crate::flex`]'s bound-before-allocate combinators instead,
//! so that shape of defect cannot exist in this decoder by construction.
//!
//! ⚠️ **Fetch goes flexible at v12, not v9** — the one message whose cutover
//! differs from Produce/Metadata/ApiVersions (`versions.rs`'s
//! `flexible_from`). Topics address by name through v12 and by id from v13;
//! `session_id`/`session_epoch` (v7+) and `forgotten_topics_data` (v7+, an
//! incremental-fetch-session feature this broker does not implement) are
//! decoded past and discarded — sessions are always declined, so nothing
//! downstream reads them.

use crate::flex::{
    TaggedFields, put_array_len, put_string, put_tagged_fields, read_array_len,
    read_nullable_string, read_tagged_fields,
};
use crate::metadata::{TopicId, read_topic_id};
use crate::wire::{Cursor, DecodeError, put_i16, put_i32, put_i64};

/// The fields of a `Fetch` request this broker acts on.
///
/// The isolation level and the per-topic, per-partition offsets requested.
/// ⚠️ ~~`max_wait_ms` and `min_bytes` are decoded past but not kept~~ — **kept
/// since `M3.20`**, the commit that gave the broker something to wait *on*.
/// They were skipped while nothing could park, which is a different thing from
/// their not being on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchRequest<'a> {
    /// How long the broker may hold this request waiting for records, in
    /// milliseconds. ⚠️ **The client's number, and the broker's whole timer.**
    /// `M3.20` wires it to a park on the coordinator's index; a broker that
    /// substituted a poll interval of its own would turn one wakeup into a
    /// choice between latency and wasted work.
    pub max_wait_ms: i32,
    /// How many bytes must accumulate before the broker answers early. A fetch
    /// holds until this is met or `max_wait_ms` expires, whichever is first.
    pub min_bytes: i32,
    /// The most bytes the whole response may carry (v3+; absent below).
    ///
    /// ⚠️ **Per *request*, and that is the point of keeping it** (`M3.22`). A
    /// `Fetch` may name many partitions, so a per-partition bound multiplied
    /// by a client-chosen count is not a bound at all — one frame becomes
    /// partitions × budget bytes of object-storage reads, all concatenated in
    /// memory before the response is framed.
    pub max_bytes: i32,
    /// `0` = read uncommitted, `1` = read committed.
    pub isolation_level: i8,
    /// The topics fetched from.
    pub topics: Vec<FetchTopic<'a>>,
}

/// One topic in a `Fetch` request: by name through v12, by id from v13.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchTopic<'a> {
    /// The topic name, or `None` (v13+ addresses by id).
    pub name: Option<&'a str>,
    /// The topic id, nil below v13.
    pub topic_id: TopicId,
    /// The partitions fetched from.
    pub partitions: Vec<FetchPartition>,
}

/// One partition in a `Fetch` request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FetchPartition {
    /// The partition index.
    pub index: i32,
    /// The offset to fetch from.
    pub fetch_offset: i64,
    /// The most bytes this partition may contribute to the response.
    ///
    /// ⚠️ **A per-partition share of the request's own bound, not a second
    /// bound.** Kafka's own broker treats it that way, and a reader that
    /// honoured only this one would be back to partitions × budget.
    pub partition_max_bytes: i32,
}

/// Decodes a `Fetch` request.
///
/// # Errors
/// [`DecodeError`] on any malformed length or truncation — every count and
/// length is bounded against the input before anything is grown, the
/// discipline that dissolves the class of `DoS` `M2.26` found.
pub fn decode_request(body: &[u8], version: i16) -> Result<FetchRequest<'_>, DecodeError> {
    let flexible = version >= 12;
    let mut cur = Cursor::new(body);

    // `replica_id` sits on the wire only through v14; from v15 the client
    // is always a real consumer (`-1`) and the field is dropped, replaced
    // by a per-partition `replica_state` this broker does not read.
    if version <= 14 {
        let _replica_id = cur.read_i32()?;
    }
    let max_wait_ms = cur.read_i32()?;
    let min_bytes = cur.read_i32()?;
    // ⚠️ **Unconditional, and the version gate is in `ADVERTISED` instead.**
    // The field arrives at v3 and this broker's Fetch floor is v4, so there is
    // no version reaching here without it — a `version >= 3` guard would be a
    // branch nothing can take, which is worse than no guard: it reads as a
    // case that has been handled.
    let max_bytes = cur.read_i32()?;
    let isolation_level = cur.read_i8()?;
    if version >= 7 {
        let _session_id = cur.read_i32()?;
        let _session_epoch = cur.read_i32()?;
    }

    let topic_count = read_array_len(&mut cur, flexible)?.unwrap_or(0);
    let mut topics = Vec::new();
    for _ in 0..topic_count {
        topics.push(read_fetch_topic(&mut cur, version, flexible)?);
    }

    // forgotten_topics_data (v7+): an incremental-fetch-session list this
    // broker never populates a session against, decoded past to reach
    // rack_id and the top-level tagged fields correctly.
    if version >= 7 {
        skip_forgotten_topics(&mut cur, version, flexible)?;
    }
    if version >= 11 {
        let _rack_id = read_nullable_string(&mut cur, flexible)?;
    }
    if flexible {
        let _: TaggedFields = read_tagged_fields(&mut cur)?;
    }

    Ok(FetchRequest {
        max_wait_ms,
        min_bytes,
        max_bytes,
        isolation_level,
        topics,
    })
}

/// Reads one `FetchTopic` entry: its addressing, then every partition.
fn read_fetch_topic<'a>(
    cur: &mut Cursor<'a>,
    version: i16,
    flexible: bool,
) -> Result<FetchTopic<'a>, DecodeError> {
    let name = if version <= 12 {
        read_nullable_string(cur, flexible)?
    } else {
        None
    };
    let topic_id = if version >= 13 {
        read_topic_id(cur)?
    } else {
        [0u8; 16]
    };
    let partition_count = read_array_len(cur, flexible)?.unwrap_or(0);
    let mut partitions = Vec::new();
    for _ in 0..partition_count {
        let index = cur.read_i32()?;
        if version >= 9 {
            let _current_leader_epoch = cur.read_i32()?;
        }
        let fetch_offset = cur.read_i64()?;
        if version >= 12 {
            let _last_fetched_epoch = cur.read_i32()?;
        }
        if version >= 5 {
            let _log_start_offset = cur.read_i64()?;
        }
        let partition_max_bytes = cur.read_i32()?;
        if flexible {
            let _: TaggedFields = read_tagged_fields(cur)?;
        }
        partitions.push(FetchPartition {
            index,
            fetch_offset,
            partition_max_bytes,
        });
    }
    if flexible {
        let _: TaggedFields = read_tagged_fields(cur)?;
    }
    Ok(FetchTopic {
        name,
        topic_id,
        partitions,
    })
}

/// Skips one `forgotten_topics_data` entry list (v7+): each entry names a
/// topic (by name through v12, by id from v13) and a plain `i32` partition
/// index array — a fetch-session feature this broker never establishes a
/// session for, so nothing is kept.
fn skip_forgotten_topics(
    cur: &mut Cursor<'_>,
    version: i16,
    flexible: bool,
) -> Result<(), DecodeError> {
    let count = read_array_len(cur, flexible)?.unwrap_or(0);
    for _ in 0..count {
        if version <= 12 {
            let _ = read_nullable_string(cur, flexible)?;
        }
        if version >= 13 {
            let _ = read_topic_id(cur)?;
        }
        let partition_count = read_array_len(cur, flexible)?.unwrap_or(0);
        for _ in 0..partition_count {
            let _ = cur.read_i32()?;
        }
        if flexible {
            let _: TaggedFields = read_tagged_fields(cur)?;
        }
    }
    Ok(())
}

/// The data a `Fetch` response carries: one entry per requested topic and
/// partition, with the outcome the handler resolved.
#[derive(Debug, Clone)]
pub struct FetchResponse<'a> {
    /// One entry per topic, in request order.
    pub topics: Vec<FetchResponseTopic<'a>>,
}

/// One topic in a `Fetch` response.
#[derive(Debug, Clone)]
pub struct FetchResponseTopic<'a> {
    /// The topic name, echoed through v12.
    pub name: Option<&'a str>,
    /// The topic id, echoed from v13.
    pub topic_id: TopicId,
    /// The per-partition outcomes.
    pub partitions: Vec<FetchResponsePartition<'a>>,
}

/// One partition's outcome in a `Fetch` response.
#[derive(Debug, Clone, Copy)]
pub struct FetchResponsePartition<'a> {
    /// The partition index.
    pub index: i32,
    /// The error code (0 = ok).
    pub error_code: i16,
    /// The high watermark.
    pub high_watermark: i64,
    /// The stable offset (no transactions, so this equals the watermark on
    /// success; the caller's call on a refusal — real Kafka brokers leave
    /// it at the protocol's `-1` unset value there, which this type does
    /// not default for the caller).
    pub last_stable_offset: i64,
    /// The log's earliest retained offset, likewise the caller's call.
    pub log_start_offset: i64,
    /// The record batches, concatenated verbatim, or `None`.
    pub records: Option<&'a [u8]>,
}

/// Appends a `Fetch` response body at `version`. The response header is
/// [`crate::frame::encode_response_header`]'s job.
pub fn encode_response(out: &mut Vec<u8>, version: i16, resp: &FetchResponse<'_>) {
    let flexible = version >= 12;
    let empty = TaggedFields::default();

    put_i32(out, 0); // throttle_time_ms
    if version >= 7 {
        put_i16(out, 0); // error_code (top-level; sessions declined, never an error)
        put_i32(out, 0); // session_id: sessions declined
    }

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
            put_i64(out, p.high_watermark);
            put_i64(out, p.last_stable_offset);
            if version >= 5 {
                put_i64(out, p.log_start_offset);
            }
            put_array_len(out, flexible, Some(0)); // aborted_transactions (empty)
            if version >= 11 {
                put_i32(out, -1); // preferred_read_replica: none
            }
            put_nullable_bytes(out, flexible, p.records);
            if flexible {
                put_tagged_fields(out, &empty);
            }
        }

        if flexible {
            put_tagged_fields(out, &empty);
        }
    }

    if flexible {
        // Top-level tagged fields: `node_endpoints` (v16+, KIP-951) rides
        // here, but only when non-empty — a single-node broker sends none.
        put_tagged_fields(out, &empty);
    }
}

/// Appends nullable bytes: compact when `flexible`, else `i32`-length legacy
/// (`-1` null).
fn put_nullable_bytes(out: &mut Vec<u8>, flexible: bool, value: Option<&[u8]>) {
    if flexible {
        crate::flex::put_compact_nullable_bytes(out, value);
    } else {
        match value {
            None => put_i32(out, -1),
            Some(bytes) => {
                put_i32(out, i32::try_from(bytes.len()).unwrap_or(i32::MAX));
                out.extend_from_slice(bytes);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{
        FetchResponse, FetchResponsePartition, FetchResponseTopic, decode_request, encode_response,
    };

    const VERSIONS: [i16; 14] = [4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17];

    /// The `ADR-0019` oracle: our request decoder reads what the dependency's
    /// encoder wrote — isolation level, topic addressing, and each
    /// partition's fetch offset — at every advertised version, including
    /// versions that carry a fetch session and forgotten-topics list.
    #[test]
    fn our_request_decode_matches_the_dependency() {
        use kafka_protocol::messages::fetch_request::{FetchPartition, FetchTopic};
        use kafka_protocol::messages::{FetchRequest as KpRequest, TopicName};
        use kafka_protocol::protocol::{Encodable, StrBytes};

        for version in VERSIONS {
            let mut kp = KpRequest::default();
            kp.isolation_level = 1;
            kp.session_id = 7; // exercised, then ignored -- sessions declined
            let mut topic = FetchTopic::default();
            if version >= 13 {
                topic.topic_id = uuid::Uuid::from_u128(0x1234);
            } else {
                topic.topic = TopicName(StrBytes::from_static_str("t"));
            }
            let mut partition = FetchPartition::default();
            partition.partition = 3;
            partition.fetch_offset = 100;
            partition.partition_max_bytes = 1 << 20;
            topic.partitions.push(partition);
            kp.topics.push(topic);
            let mut bytes = Vec::new();
            kp.encode(&mut bytes, version).expect("dependency encodes");

            let ours = decode_request(&bytes, version).expect("ours decodes");
            assert_eq!(ours.isolation_level, 1, "v{version}");
            assert_eq!(ours.topics.len(), 1, "v{version}");
            assert_eq!(ours.topics[0].partitions.len(), 1, "v{version}");
            let part = &ours.topics[0].partitions[0];
            assert_eq!(part.index, 3, "v{version}");
            assert_eq!(part.fetch_offset, 100, "v{version}");
            assert_eq!(part.partition_max_bytes, 1 << 20, "v{version}");
            if version >= 13 {
                assert_eq!(
                    ours.topics[0].topic_id,
                    uuid::Uuid::from_u128(0x1234).into_bytes(),
                    "v{version}"
                );
            } else {
                assert_eq!(ours.topics[0].name, Some("t"), "v{version}");
            }
        }
    }

    /// The same, with a non-empty `forgotten_topics_data` (v7+): the decoder
    /// must walk past it correctly to reach the real topics and the tail.
    #[test]
    fn a_forgotten_topics_list_is_correctly_skipped() {
        use kafka_protocol::messages::fetch_request::{FetchPartition, FetchTopic, ForgottenTopic};
        use kafka_protocol::messages::{FetchRequest as KpRequest, TopicName};
        use kafka_protocol::protocol::{Encodable, StrBytes};

        for version in [7i16, 12, 17] {
            let mut kp = KpRequest::default();
            kp.isolation_level = 0;
            let mut forgotten = ForgottenTopic::default();
            if version >= 13 {
                forgotten.topic_id = uuid::Uuid::from_u128(0xDEAD);
            } else {
                forgotten.topic = TopicName(StrBytes::from_static_str("old"));
            }
            forgotten.partitions = vec![1, 2];
            kp.forgotten_topics_data.push(forgotten);

            let mut topic = FetchTopic::default();
            if version >= 13 {
                topic.topic_id = uuid::Uuid::from_u128(0x1234);
            } else {
                topic.topic = TopicName(StrBytes::from_static_str("t"));
            }
            let mut partition = FetchPartition::default();
            partition.partition = 0;
            partition.fetch_offset = 5;
            partition.partition_max_bytes = 1 << 20;
            topic.partitions.push(partition);
            kp.topics.push(topic);

            let mut bytes = Vec::new();
            kp.encode(&mut bytes, version).expect("encodes");
            let ours = decode_request(&bytes, version).expect("decodes past forgotten_topics");
            assert_eq!(ours.topics.len(), 1, "v{version}");
            assert_eq!(ours.topics[0].partitions[0].fetch_offset, 5, "v{version}");
        }
    }

    /// The `ADR-0019` oracle: our response bytes decode under the
    /// dependency's `FetchResponse` at every version, carrying the records
    /// bytes verbatim.
    #[test]
    fn our_response_decodes_under_the_dependency() {
        use kafka_protocol::messages::FetchResponse as KpResponse;
        use kafka_protocol::protocol::Decodable;

        let records = b"opaque-batch-bytes";
        let resp = FetchResponse {
            topics: vec![FetchResponseTopic {
                name: Some("t"),
                topic_id: [9u8; 16],
                partitions: vec![FetchResponsePartition {
                    index: 0,
                    error_code: 0,
                    high_watermark: 42,
                    last_stable_offset: 42,
                    log_start_offset: 0,
                    records: Some(records),
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
            let part = &decoded.responses[0].partitions[0];
            assert_eq!(part.error_code, 0, "v{version}");
            assert_eq!(part.high_watermark, 42, "v{version}");
            assert_eq!(part.last_stable_offset, 42, "v{version}");
            assert_eq!(part.records.as_deref(), Some(&records[..]), "v{version}");
        }
    }

    /// ⚠️ **The request's own `max_bytes` is read at every advertised version**,
    /// and the assertion that matters is the *second* one: a decoder that got
    /// this field's position wrong would still return a plausible number here
    /// and mis-frame the isolation level after it.
    #[test]
    fn the_request_max_bytes_is_read_and_leaves_the_next_field_framed() {
        use kafka_protocol::messages::FetchRequest as KpRequest;
        use kafka_protocol::protocol::Encodable;

        for version in VERSIONS {
            let mut kp = KpRequest::default();
            if version <= 14 {
                kp.replica_id = kafka_protocol::messages::BrokerId(-1);
            }
            kp.isolation_level = i8::from(version >= 4);
            kp.max_bytes = 4_096;
            let mut bytes = Vec::new();
            kp.encode(&mut bytes, version).expect("dependency encodes");

            let ours = decode_request(&bytes, version).expect("ours decodes");
            assert_eq!(ours.max_bytes, 4_096, "v{version}");
            assert_eq!(
                ours.isolation_level,
                i8::from(version >= 4),
                "v{version}: the field after it is still framed right"
            );
        }
    }
}
