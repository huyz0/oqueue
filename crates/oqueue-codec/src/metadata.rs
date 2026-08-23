//! `Metadata` v0-13 request decode and response encode, hand-rolled
//! (`ADR-0019`).
//!
//! The most version-heavy message.
//!
//! The array/string encodings go flexible at v9, topic ids appear at v10,
//! `allow_auto_topic_creation` at v4, and the response nests brokers, topics,
//! and partitions, each with their own version-gated fields. Every one of
//! those transitions is data here, driven by the `flexible` flag and per-field
//! `version >=` guards, and the whole thing is byte-differentialed against
//! `kafka-protocol` below.
//!
//! ⚠️ **Topic ids are `[u8; 16]`, not `uuid::Uuid`.** The codec deals in
//! bytes; the broker owns the `Uuid` type and converts at the seam. That
//! keeps `oqueue-codec` free of a dependency it does not need.

use crate::flex::{
    TaggedFields, put_array_len, put_nullable_string, put_tagged_fields, read_array_len,
    read_nullable_string, read_tagged_fields,
};
use crate::wire::{Cursor, DecodeError, put_bool, put_i16, put_i32};

/// A topic id on the wire: 16 opaque bytes (nil below v10).
pub type TopicId = [u8; 16];

/// The fields of a `Metadata` request this broker acts on.
///
/// Which topics (`None` = all), and whether to auto-create missing ones. The
/// authorized-operations flags and tagged fields are not decoded — nothing
/// downstream reads them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataRequest {
    /// Requested topics, or `None` for "every topic".
    pub topics: Option<Vec<MetadataRequestTopic>>,
    /// Auto-create missing topics (v4+; `true` below, the historical
    /// default).
    pub allow_auto_topic_creation: bool,
}

/// One requested topic: by id (v10+) or name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MetadataRequestTopic {
    /// The topic id, nil below v10.
    pub topic_id: TopicId,
    /// The topic name, or `None` (a v10+ describe-by-id request).
    pub name: Option<String>,
}

/// Decodes a `Metadata` request. Stops after `allow_auto_topic_creation`;
/// the trailing authorized-operations flags and tagged fields do not change
/// the answer, so they are not read.
///
/// # Errors
/// [`DecodeError`] on any malformed length, string, or truncation — every
/// array count and string length is bounded against the input first.
pub fn decode_request(body: &[u8], version: i16) -> Result<MetadataRequest, DecodeError> {
    let flexible = version >= 9;
    let mut cur = Cursor::new(body);

    let topics = match read_array_len(&mut cur, flexible)? {
        None => None,
        Some(count) => {
            // Grown, not pre-sized: `count` is bounded by remaining bytes,
            // not by the per-element cost (`flex`'s discipline).
            let mut topics = Vec::new();
            for _ in 0..count {
                let topic_id = if version >= 10 {
                    read_topic_id(&mut cur)?
                } else {
                    [0u8; 16]
                };
                let name = read_nullable_string(&mut cur, flexible)?.map(str::to_owned);
                if flexible {
                    let _: TaggedFields = read_tagged_fields(&mut cur)?;
                }
                topics.push(MetadataRequestTopic { topic_id, name });
            }
            Some(topics)
        }
    };

    let allow_auto_topic_creation = if version >= 4 { cur.read_bool()? } else { true };

    Ok(MetadataRequest {
        topics,
        allow_auto_topic_creation,
    })
}

/// Reads a topic id: 16 raw bytes, shared with [`crate::produce`] (both
/// messages address topics by the same wire shape from their id-addressed
/// version).
pub(crate) fn read_topic_id(cur: &mut Cursor<'_>) -> Result<TopicId, DecodeError> {
    let bytes = cur.take(16)?;
    // `take(16)` returns exactly 16 bytes, so the conversion cannot fail.
    Ok(bytes.try_into().unwrap_or([0u8; 16]))
}

/// The data a `Metadata` response carries.
///
/// One broker (this node), a cluster id, and the topics with their partition
/// counts. Every partition is led by the single broker with it as the sole
/// replica and in-sync replica — the stub's shape, and all a single-node
/// broker can honestly report.
#[derive(Debug, Clone)]
pub struct MetadataResponse<'a> {
    /// This broker's node id.
    pub node_id: i32,
    /// This broker's advertised host.
    pub host: &'a str,
    /// This broker's advertised port.
    pub port: i32,
    /// The cluster id (v2+).
    pub cluster_id: Option<&'a str>,
    /// The controller's node id (v1+).
    pub controller_id: i32,
    /// The topics to report.
    pub topics: Vec<MetadataResponseTopic<'a>>,
}

/// One topic in a `Metadata` response.
#[derive(Debug, Clone)]
pub struct MetadataResponseTopic<'a> {
    /// The per-topic error code (0 = ok).
    pub error_code: i16,
    /// The topic name, or `None`.
    pub name: Option<&'a str>,
    /// The topic id (v10+; nil below).
    pub topic_id: TopicId,
    /// How many partitions to report, `0..count`, each led by this broker.
    pub partition_count: usize,
}

/// The "not requested" sentinel for the authorized-operations fields.
const AUTHORIZED_OPERATIONS_OMITTED: i32 = i32::MIN;

/// Appends a `Metadata` response body at `version`. The response header is
/// [`crate::frame::encode_response_header`]'s job.
#[allow(clippy::too_many_lines)]
pub fn encode_response(out: &mut Vec<u8>, version: i16, resp: &MetadataResponse<'_>) {
    let flexible = version >= 9;
    let empty = TaggedFields::default();

    if version >= 3 {
        put_i32(out, 0); // throttle_time_ms
    }

    // brokers: exactly this node.
    put_array_len(out, flexible, Some(1));
    put_i32(out, resp.node_id);
    put_nullable_string(out, flexible, Some(resp.host));
    put_i32(out, resp.port);
    if version >= 1 {
        put_nullable_string(out, flexible, None); // rack
    }
    if flexible {
        put_tagged_fields(out, &empty);
    }

    if version >= 2 {
        put_nullable_string(out, flexible, resp.cluster_id);
    }
    if version >= 1 {
        put_i32(out, resp.controller_id);
    }

    // topics.
    put_array_len(out, flexible, Some(resp.topics.len()));
    for topic in &resp.topics {
        put_i16(out, topic.error_code);
        put_nullable_string(out, flexible, topic.name);
        if version >= 10 {
            out.extend_from_slice(&topic.topic_id);
        }
        if version >= 1 {
            put_bool(out, false); // is_internal
        }

        put_array_len(out, flexible, Some(topic.partition_count));
        for index in 0..topic.partition_count {
            put_i16(out, 0); // error_code
            put_i32(out, i32::try_from(index).unwrap_or(i32::MAX));
            put_i32(out, resp.node_id); // leader_id
            if version >= 7 {
                put_i32(out, 0); // leader_epoch
            }
            put_int32_array(out, flexible, &[resp.node_id]); // replica_nodes
            put_int32_array(out, flexible, &[resp.node_id]); // isr_nodes
            if version >= 5 {
                put_int32_array(out, flexible, &[]); // offline_replicas
            }
            if flexible {
                put_tagged_fields(out, &empty);
            }
        }

        if version >= 8 {
            put_i32(out, AUTHORIZED_OPERATIONS_OMITTED); // topic_authorized_operations
        }
        if flexible {
            put_tagged_fields(out, &empty);
        }
    }

    if (8..=10).contains(&version) {
        put_i32(out, AUTHORIZED_OPERATIONS_OMITTED); // cluster_authorized_operations
    }
    if version >= 13 {
        put_i16(out, 0); // top-level error_code (added v13); 0 = no error
    }
    if flexible {
        put_tagged_fields(out, &empty);
    }
}

/// Appends a non-nullable `i32` array: the length (compact or legacy by
/// `flexible`) then each value.
fn put_int32_array(out: &mut Vec<u8>, flexible: bool, values: &[i32]) {
    put_array_len(out, flexible, Some(values.len()));
    for &v in values {
        put_i32(out, v);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{MetadataResponse, MetadataResponseTopic, decode_request, encode_response};

    /// Every advertised Metadata version, both directions.
    const VERSIONS: [i16; 14] = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13];

    /// The `ADR-0019` oracle: our request decoder reads what the dependency's
    /// encoder wrote, at every version — topics and the auto-create flag.
    #[test]
    fn our_request_decode_matches_the_dependency() {
        use kafka_protocol::messages::metadata_request::MetadataRequestTopic as KpTopic;
        use kafka_protocol::messages::{MetadataRequest as KpRequest, TopicName};
        use kafka_protocol::protocol::{Encodable, StrBytes};

        for version in VERSIONS {
            let mut kp = KpRequest::default();
            let mut topic = KpTopic::default();
            topic.name = Some(TopicName(StrBytes::from_static_str("orders")));
            kp.topics = Some(vec![topic]);
            // `true` is the field's default, so the encoder accepts it at
            // every version (below v4 the field is absent and our decoder
            // defaults it true too).
            kp.allow_auto_topic_creation = true;
            let mut bytes = Vec::new();
            kp.encode(&mut bytes, version).expect("dependency encodes");

            let ours = decode_request(&bytes, version).expect("ours decodes");
            let topics = ours.topics.expect("a topic list");
            assert_eq!(topics.len(), 1, "v{version}");
            assert_eq!(topics[0].name.as_deref(), Some("orders"), "v{version}");
            assert!(ours.allow_auto_topic_creation, "v{version}");
        }

        // The flag is on the wire from v4, so a `false` there is read back.
        for version in [4i16, 9, 12] {
            let mut kp = KpRequest::default();
            kp.topics = Some(vec![KpTopic::default()]);
            kp.allow_auto_topic_creation = false;
            let mut bytes = Vec::new();
            kp.encode(&mut bytes, version).expect("encodes");
            assert!(
                !decode_request(&bytes, version)
                    .expect("decodes")
                    .allow_auto_topic_creation,
                "v{version}: false is read from the wire"
            );
        }
    }

    /// A null topic list decodes as `None` (all topics) at every version.
    #[test]
    fn a_null_topic_list_decodes_as_none() {
        use kafka_protocol::messages::MetadataRequest as KpRequest;
        use kafka_protocol::protocol::Encodable;

        for version in VERSIONS {
            let mut kp = KpRequest::default();
            kp.topics = None;
            kp.allow_auto_topic_creation = true;
            let mut bytes = Vec::new();
            kp.encode(&mut bytes, version).expect("encodes");
            assert_eq!(
                decode_request(&bytes, version).expect("decodes").topics,
                None
            );
        }
    }

    /// The `ADR-0019` oracle: our response bytes decode under the
    /// dependency's `MetadataResponse` at every version, carrying the broker,
    /// the cluster id, and the topic with its partitions.
    #[test]
    fn our_response_decodes_under_the_dependency() {
        use kafka_protocol::messages::MetadataResponse as KpResponse;
        use kafka_protocol::protocol::Decodable;

        let resp = MetadataResponse {
            node_id: 0,
            host: "broker.example",
            port: 9092,
            cluster_id: Some("oqueue"),
            controller_id: 0,
            topics: vec![MetadataResponseTopic {
                error_code: 0,
                name: Some("orders"),
                topic_id: [7u8; 16],
                partition_count: 3,
            }],
        };

        for version in VERSIONS {
            let mut out = Vec::new();
            encode_response(&mut out, version, &resp);
            let mut cursor = &out[..];
            let decoded = KpResponse::decode(&mut cursor, version)
                .unwrap_or_else(|e| panic!("v{version}: oracle decode failed: {e}"));
            assert!(cursor.is_empty(), "v{version}: whole body consumed");
            assert_eq!(decoded.brokers.len(), 1, "v{version}");
            assert_eq!(decoded.brokers[0].port, 9092, "v{version}");
            assert_eq!(decoded.topics.len(), 1, "v{version}");
            assert_eq!(decoded.topics[0].error_code, 0, "v{version}");
            assert_eq!(decoded.topics[0].partitions.len(), 3, "v{version}");
            if version >= 10 {
                assert_eq!(
                    decoded.topics[0].topic_id.as_bytes(),
                    &[7u8; 16],
                    "v{version}"
                );
            }
        }
    }
}
