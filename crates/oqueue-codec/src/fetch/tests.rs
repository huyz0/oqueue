//! `fetch.rs`'s tests, split out at the 500-line limit.
//!
//! ⚠️ **Split rather than trimmed**, on `oqueue-broker/src/produce/`'s own
//! precedent (`mod.rs` + `tests.rs`): the module boundary the line limit is
//! pointing at is request/response *code* versus the tests that pin it, not
//! any one test being cuttable.

#![allow(clippy::expect_used)]

use super::{
    FetchResponse, FetchResponsePartition, FetchResponseTopic, decode_request, encode_response,
};
use crate::metadata::TopicIdentity;

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
///
/// ⚠️ **The identity is built per version** — `M3.41`, same reasoning as
/// `produce.rs`'s identical test: one shared `name`/`topic_id` pair
/// reused across the version loop is what let the old encoder's
/// `unwrap_or_default()` go untested, because the value was always
/// present either way.
#[test]
fn our_response_decodes_under_the_dependency() {
    use kafka_protocol::messages::FetchResponse as KpResponse;
    use kafka_protocol::protocol::Decodable;

    let records = b"opaque-batch-bytes";
    for version in VERSIONS {
        let identity = if version <= 12 {
            TopicIdentity::Name("t")
        } else {
            TopicIdentity::Id([9u8; 16])
        };
        let resp = FetchResponse {
            topics: vec![FetchResponseTopic {
                identity,
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
        let mut out = Vec::new();
        encode_response(&mut out, version, &resp);
        let mut cursor = &out[..];
        let decoded = KpResponse::decode(&mut cursor, version)
            .unwrap_or_else(|e| panic!("v{version}: oracle decode failed: {e}"));
        assert!(cursor.is_empty(), "v{version}: whole body consumed");
        if version <= 12 {
            assert_eq!(
                decoded.responses[0].topic.as_str(),
                "t",
                "v{version}: the name this test sent, not a default"
            );
        } else {
            assert_eq!(
                decoded.responses[0].topic_id,
                uuid::Uuid::from_bytes([9u8; 16]),
                "v{version}: the id this test sent"
            );
        }
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
