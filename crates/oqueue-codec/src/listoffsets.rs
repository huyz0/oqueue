//! `ListOffsets` v1-9 request decode and response encode, hand-rolled
//! (`ADR-0019`).
//!
//! ⚠️ **The API a stale answer turns into negative consumer lag.** A consumer
//! computes lag as `log_end_offset - position`; if `ListOffsets` reports an end
//! below where the consumer already is, the subtraction goes negative and every
//! dashboard built on it is wrong in the direction that looks like nothing is
//! happening. Doc 12 §4.6 lists it as hazard H1, and two of five bugs an
//! independent audit found in a peer system were of this shape. So the answer
//! is never derived from a cache — see `oqueue-broker`'s handler.
//!
//! ⚠️ **From v1, and v0 is not a version this broker declines to serve — it is
//! a different message.** Before KIP-79, a partition's entry carried an
//! *array* of offsets; from v1 it carries one, with a timestamp beside it.
//! Advertising a version means serving it (FR-2), so the floor is where the
//! shape this code writes begins.
//!
//! ⚠️ **Two timestamps are sentinels, not times**: `-1` is LATEST and `-2` is
//! EARLIEST. A real timestamp asks "the first offset at or after this time",
//! which needs an index this broker does not keep — the handler says what it
//! answers instead.

use crate::flex::{
    TaggedFields, put_array_len, put_string, put_tagged_fields, read_array_len,
    read_nullable_string, read_tagged_fields,
};
use crate::wire::{Cursor, DecodeError, put_i16, put_i32, put_i64};

/// The `timestamp` asking for the offset after the last record.
pub const LATEST_TIMESTAMP: i64 = -1;
/// The `timestamp` asking for the offset of the first record still held.
pub const EARLIEST_TIMESTAMP: i64 = -2;

/// The fields of a `ListOffsets` request this broker acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListOffsetsRequest<'a> {
    /// `0` = read uncommitted, `1` = read committed (v2+; `0` below).
    pub isolation_level: i8,
    /// The topics asked about.
    pub topics: Vec<ListOffsetsTopic<'a>>,
}

/// One topic in a `ListOffsets` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListOffsetsTopic<'a> {
    /// The topic name. ⚠️ **Always by name**: unlike `Produce` and `Fetch`,
    /// `ListOffsets` never gained id addressing.
    pub name: Option<&'a str>,
    /// The partitions asked about.
    pub partitions: Vec<ListOffsetsPartition>,
}

/// One partition in a `ListOffsets` request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListOffsetsPartition {
    /// The partition index.
    pub index: i32,
    /// [`LATEST_TIMESTAMP`], [`EARLIEST_TIMESTAMP`], or a real millisecond
    /// timestamp.
    pub timestamp: i64,
}

/// Decodes a `ListOffsets` request.
///
/// # Errors
/// [`DecodeError`] on any malformed length or truncation — every count and
/// length is bounded against the input before anything is grown.
pub fn decode_request(body: &[u8], version: i16) -> Result<ListOffsetsRequest<'_>, DecodeError> {
    let flexible = version >= 6;
    let mut cur = Cursor::new(body);
    let _replica_id = cur.read_i32()?;
    // ⚠️ v2 is where the isolation level appears. Below it every read is
    // uncommitted, which is the same thing this broker serves at either level
    // until transactions exist.
    let isolation_level = if version >= 2 { cur.read_i8()? } else { 0 };

    let topic_count = read_array_len(&mut cur, flexible)?.unwrap_or(0);
    let mut topics = Vec::new();
    for _ in 0..topic_count {
        let name = read_nullable_string(&mut cur, flexible)?;
        let partition_count = read_array_len(&mut cur, flexible)?.unwrap_or(0);
        let mut partitions = Vec::new();
        for _ in 0..partition_count {
            let index = cur.read_i32()?;
            if version >= 4 {
                let _current_leader_epoch = cur.read_i32()?;
            }
            let timestamp = cur.read_i64()?;
            if version == 0 {
                let _max_num_offsets = cur.read_i32()?;
            }
            if flexible {
                read_tagged_fields(&mut cur)?;
            }
            partitions.push(ListOffsetsPartition { index, timestamp });
        }
        if flexible {
            read_tagged_fields(&mut cur)?;
        }
        topics.push(ListOffsetsTopic { name, partitions });
    }
    if flexible {
        read_tagged_fields(&mut cur)?;
    }
    Ok(ListOffsetsRequest {
        isolation_level,
        topics,
    })
}

/// One topic's answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListOffsetsResponseTopic<'a> {
    /// The topic name, echoed. ⚠️ **`&'a str`, not `Option`** (`M3.41`): the
    /// response schema writes it at *every* advertised version, so there is
    /// no version at which a null is legal and no fallback for the encoder
    /// to need. A caller with no name to echo is a caller that should not be
    /// building this response at all.
    pub name: &'a str,
    /// One entry per requested partition, in order.
    pub partitions: Vec<ListOffsetsResponsePartition>,
}

/// One partition's answer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListOffsetsResponsePartition {
    /// The partition index, echoed.
    pub index: i32,
    /// `0`, or why this partition could not be answered.
    pub error_code: i16,
    /// The timestamp of the record at `offset`, or `-1` when none applies.
    pub timestamp: i64,
    /// The offset asked for, or `-1` on any refusal.
    pub offset: i64,
}

/// The whole answer.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListOffsetsResponse<'a> {
    /// One entry per requested topic, in order.
    pub topics: Vec<ListOffsetsResponseTopic<'a>>,
}

/// Encodes a `ListOffsets` response.
pub fn encode_response(out: &mut Vec<u8>, version: i16, resp: &ListOffsetsResponse<'_>) {
    let flexible = version >= 6;
    let empty = TaggedFields::default();

    if version >= 2 {
        put_i32(out, 0); // throttle_time_ms
    }
    put_array_len(out, flexible, Some(resp.topics.len()));
    for topic in &resp.topics {
        // ⚠️ **Non-nullable in the response schema, and now in the type**
        // (`M3.40`, `M3.41`). `topic.name` is `&'a str`, not `Option`, so
        // there is nothing here to resolve and nothing to default.
        put_string(out, flexible, topic.name);
        put_array_len(out, flexible, Some(topic.partitions.len()));
        for partition in &topic.partitions {
            put_i32(out, partition.index);
            put_i16(out, partition.error_code);
            put_i64(out, partition.timestamp);
            put_i64(out, partition.offset);
            if version >= 4 {
                // ⚠️ `leader_epoch`, and `-1` is the honest answer rather than
                // a placeholder: this broker has no partition leadership at
                // all (`ADR-0020`), so there is no epoch to report and `-1` is
                // the protocol's "unknown".
                put_i32(out, -1);
            }
            if flexible {
                put_tagged_fields(out, &empty);
            }
        }
        if flexible {
            put_tagged_fields(out, &empty);
        }
    }
    if flexible {
        put_tagged_fields(out, &empty);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{
        EARLIEST_TIMESTAMP, LATEST_TIMESTAMP, ListOffsetsResponse, ListOffsetsResponsePartition,
        ListOffsetsResponseTopic, decode_request, encode_response,
    };

    /// Every advertised `ListOffsets` version, both directions.
    const VERSIONS: [i16; 9] = [1, 2, 3, 4, 5, 6, 7, 8, 9];

    /// ⚠️ **The sentinels are protocol constants, so their *values* are the
    /// contract.** Every test here that used them symbolically would keep
    /// passing if they were changed together, and a broker whose `LATEST` was
    /// `1` would refuse every real consumer's request as an unsupported
    /// timestamp. `kafka-protocol` is the oracle, as `ADR-0019` requires.
    #[test]
    fn the_timestamp_sentinels_are_the_protocols_own() {
        use kafka_protocol::messages::list_offsets_request::ListOffsetsPartition as KpPartition;

        assert_eq!(LATEST_TIMESTAMP, -1);
        assert_eq!(EARLIEST_TIMESTAMP, -2);
        // And the dependency agrees they are legal values of the field.
        let mut partition = KpPartition::default();
        partition.timestamp = LATEST_TIMESTAMP;
        assert_eq!(partition.timestamp, -1);
        partition.timestamp = EARLIEST_TIMESTAMP;
        assert_eq!(partition.timestamp, -2);
    }

    /// The `ADR-0019` oracle: our decoder reads what the dependency's encoder
    /// wrote, at every advertised version.
    #[test]
    fn our_request_decode_matches_the_dependency() {
        use kafka_protocol::messages::list_offsets_request::{
            ListOffsetsPartition as KpPartition, ListOffsetsTopic as KpTopic,
        };
        use kafka_protocol::messages::{ListOffsetsRequest as KpRequest, TopicName};
        use kafka_protocol::protocol::{Encodable, StrBytes};

        for version in VERSIONS {
            for timestamp in [LATEST_TIMESTAMP, EARLIEST_TIMESTAMP, 1_700_000_000_000] {
                let mut kp = KpRequest::default();
                kp.replica_id = kafka_protocol::messages::BrokerId(-1);
                let mut topic = KpTopic::default();
                topic.name = TopicName(StrBytes::from_static_str("orders"));
                let mut partition = KpPartition::default();
                partition.partition_index = 7;
                partition.timestamp = timestamp;
                topic.partitions.push(partition);
                kp.topics.push(topic);
                let mut bytes = Vec::new();
                kp.encode(&mut bytes, version).expect("dependency encodes");

                let ours = decode_request(&bytes, version).expect("ours decodes");
                assert_eq!(ours.topics.len(), 1, "v{version}");
                assert_eq!(ours.topics[0].name, Some("orders"), "v{version}");
                assert_eq!(ours.topics[0].partitions.len(), 1, "v{version}");
                assert_eq!(ours.topics[0].partitions[0].index, 7, "v{version}");
                assert_eq!(
                    ours.topics[0].partitions[0].timestamp, timestamp,
                    "v{version}"
                );
            }
        }
    }

    /// ⚠️ **The isolation level is on the wire from v2**, and below it there is
    /// nothing to read — a decoder that looked anyway would consume a byte of
    /// the topic array and mis-frame everything after it.
    #[test]
    fn the_isolation_level_appears_at_v2_and_not_before() {
        use kafka_protocol::messages::ListOffsetsRequest as KpRequest;
        use kafka_protocol::protocol::Encodable;

        for version in VERSIONS {
            let mut kp = KpRequest::default();
            kp.replica_id = kafka_protocol::messages::BrokerId(-1);
            kp.isolation_level = i8::from(version >= 2);
            let mut bytes = Vec::new();
            kp.encode(&mut bytes, version).expect("dependency encodes");

            let ours = decode_request(&bytes, version).expect("ours decodes");
            assert_eq!(ours.isolation_level, i8::from(version >= 2), "v{version}");
            assert!(ours.topics.is_empty(), "v{version}");
        }
    }

    /// And the other direction: the dependency's decoder reads what we wrote,
    /// field for field, at every advertised version.
    #[test]
    fn our_response_encode_matches_the_dependency() {
        use kafka_protocol::messages::ListOffsetsResponse as KpResponse;
        use kafka_protocol::protocol::Decodable;

        for version in VERSIONS {
            let response = ListOffsetsResponse {
                topics: vec![ListOffsetsResponseTopic {
                    name: "orders",
                    partitions: vec![
                        ListOffsetsResponsePartition {
                            index: 0,
                            error_code: 0,
                            timestamp: -1,
                            offset: 42,
                        },
                        ListOffsetsResponsePartition {
                            index: 1,
                            error_code: 3,
                            timestamp: -1,
                            offset: -1,
                        },
                    ],
                }],
            };
            let mut bytes = Vec::new();
            encode_response(&mut bytes, version, &response);

            let mut rest = &bytes[..];
            let theirs = KpResponse::decode(&mut rest, version).expect("the dependency decodes");
            assert!(rest.is_empty(), "v{version}: nothing after the body");
            assert_eq!(theirs.topics.len(), 1, "v{version}");
            assert_eq!(theirs.topics[0].name.0.as_str(), "orders", "v{version}");
            let partitions = &theirs.topics[0].partitions;
            assert_eq!(partitions.len(), 2, "v{version}");
            assert_eq!(partitions[0].offset, 42, "v{version}");
            assert_eq!(partitions[0].error_code, 0, "v{version}");
            assert_eq!(partitions[1].offset, -1, "v{version}");
            assert_eq!(partitions[1].error_code, 3, "v{version}");
            if version >= 4 {
                // ⚠️ **`-1`, the protocol's "unknown"**, and it is the honest
                // answer rather than a placeholder: this broker has no
                // partition leadership at all (`ADR-0020`), so there is no
                // epoch. A `0` or a `1` here would be a claim about a leader
                // that does not exist, and a client fencing on it would fence
                // against a number nothing maintains.
                assert_eq!(partitions[0].leader_epoch, -1, "v{version}");
                assert_eq!(partitions[1].leader_epoch, -1, "v{version}");
            }
        }
    }

    /// ⚠️ **A truncated body is refused, not guessed at.** These are bytes off
    /// a socket, so every prefix of a legal request must decode or fail —
    /// never panic, and never allocate from a length it has not checked
    /// (`security.md` rule 3, and the `M2.26` `DoS` this layer exists to
    /// dissolve).
    #[test]
    fn every_prefix_of_a_request_is_refused_rather_than_panicking() {
        use kafka_protocol::messages::list_offsets_request::{
            ListOffsetsPartition as KpPartition, ListOffsetsTopic as KpTopic,
        };
        use kafka_protocol::messages::{ListOffsetsRequest as KpRequest, TopicName};
        use kafka_protocol::protocol::{Encodable, StrBytes};

        for version in VERSIONS {
            let mut kp = KpRequest::default();
            kp.replica_id = kafka_protocol::messages::BrokerId(-1);
            let mut topic = KpTopic::default();
            topic.name = TopicName(StrBytes::from_static_str("orders"));
            topic.partitions.push(KpPartition::default());
            kp.topics.push(topic);
            let mut bytes = Vec::new();
            kp.encode(&mut bytes, version).expect("dependency encodes");

            for cut in 0..bytes.len() {
                let _ = decode_request(&bytes[..cut], version);
            }
            assert!(decode_request(&bytes, version).is_ok(), "v{version}");
        }
    }
}
