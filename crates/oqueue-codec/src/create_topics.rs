//! `CreateTopics` v2-v7, hand-rolled (`ADR-0019`).
//!
//! The broker only acts on names and partition counts. Assignments and
//! configuration entries are nevertheless decoded completely so malformed or
//! trailing client input cannot be mistaken for a valid request.

use crate::flex::{
    TaggedFields, put_array_len, put_nullable_string, put_string, put_tagged_fields,
    read_array_len, read_nullable_string, read_string, read_tagged_fields,
};
use crate::wire::{Cursor, DecodeError, put_i16, put_i32};

/// A topic requested for creation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateTopicsRequest {
    /// Topics to create.
    pub topics: Vec<CreatableTopic>,
    /// Client timeout, carried for protocol compatibility.
    pub timeout_ms: i32,
    /// Validate without changing the catalog.
    pub validate_only: bool,
}

/// One requested topic.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatableTopic {
    /// Topic name.
    pub name: String,
    /// Requested partition count, or -1 for a broker default.
    pub num_partitions: i32,
    /// Requested replication factor, or -1 for a broker default.
    pub replication_factor: i16,
    /// Manual assignments, retained for validation.
    pub assignments: Vec<CreatableReplicaAssignment>,
    /// Configurations, retained for validation and future support.
    pub configs: Vec<CreatableTopicConfig>,
}

/// A manual partition assignment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatableReplicaAssignment {
    /// Partition index.
    pub partition_index: i32,
    /// Broker ids.
    pub broker_ids: Vec<i32>,
}

/// A requested topic configuration.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatableTopicConfig {
    /// Configuration name.
    pub name: String,
    /// Configuration value, where null means remove/default.
    pub value: Option<String>,
}

/// Decodes a request body at v2-v7.
///
/// # Errors
/// Returns [`DecodeError`] for a null or malformed array, invalid string,
/// truncation, or trailing bytes.
pub fn decode_request(body: &[u8], version: i16) -> Result<CreateTopicsRequest, DecodeError> {
    let flexible = version >= 5;
    let mut cur = Cursor::new(body);
    let count = read_array_len(&mut cur, flexible)?
        .ok_or_else(|| DecodeError::UnexpectedNull { at: cur.position() })?;
    let mut topics = Vec::new();
    for _ in 0..count {
        topics.push(decode_topic(&mut cur, flexible)?);
    }
    let timeout_ms = cur.read_i32()?;
    let validate_only = cur.read_bool()?;
    if flexible {
        let _ = read_tagged_fields(&mut cur)?;
    }
    if cur.remaining() != 0 {
        return Err(DecodeError::LengthOutOfBounds {
            length: cur.remaining() as u64,
            max: 0,
            at: cur.position(),
        });
    }
    Ok(CreateTopicsRequest {
        topics,
        timeout_ms,
        validate_only,
    })
}

fn decode_topic(cur: &mut Cursor<'_>, flexible: bool) -> Result<CreatableTopic, DecodeError> {
    let name = read_string(cur, flexible)?.to_owned();
    let num_partitions = cur.read_i32()?;
    let replication_factor = cur.read_i16()?;
    let assignments = decode_assignments(cur, flexible)?;
    let configs = decode_configs(cur, flexible)?;
    if flexible {
        let _ = read_tagged_fields(cur)?;
    }
    Ok(CreatableTopic {
        name,
        num_partitions,
        replication_factor,
        assignments,
        configs,
    })
}

fn decode_assignments(
    cur: &mut Cursor<'_>,
    flexible: bool,
) -> Result<Vec<CreatableReplicaAssignment>, DecodeError> {
    let count = read_array_len(cur, flexible)?
        .ok_or_else(|| DecodeError::UnexpectedNull { at: cur.position() })?;
    let mut assignments = Vec::new();
    for _ in 0..count {
        let partition_index = cur.read_i32()?;
        let broker_count = read_array_len(cur, flexible)?
            .ok_or_else(|| DecodeError::UnexpectedNull { at: cur.position() })?;
        let mut broker_ids = Vec::new();
        for _ in 0..broker_count {
            broker_ids.push(cur.read_i32()?);
        }
        if flexible {
            let _ = read_tagged_fields(cur)?;
        }
        assignments.push(CreatableReplicaAssignment {
            partition_index,
            broker_ids,
        });
    }
    Ok(assignments)
}

fn decode_configs(
    cur: &mut Cursor<'_>,
    flexible: bool,
) -> Result<Vec<CreatableTopicConfig>, DecodeError> {
    let count = read_array_len(cur, flexible)?
        .ok_or_else(|| DecodeError::UnexpectedNull { at: cur.position() })?;
    let mut configs = Vec::new();
    for _ in 0..count {
        let name = read_string(cur, flexible)?.to_owned();
        let value = read_nullable_string(cur, flexible)?.map(str::to_owned);
        if flexible {
            let _ = read_tagged_fields(cur)?;
        }
        configs.push(CreatableTopicConfig { name, value });
    }
    Ok(configs)
}

/// One topic's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateTopicsResponseTopic {
    /// Topic name.
    pub name: String,
    /// Topic id, present on v7.
    pub topic_id: [u8; 16],
    /// Result code.
    pub error_code: i16,
    /// Optional diagnostic.
    pub error_message: Option<String>,
    /// Created partition count, present from v5.
    pub num_partitions: i32,
    /// Single-node replication factor, present from v5.
    pub replication_factor: i16,
}

/// The response body to encode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreateTopicsResponse {
    /// Throttle duration.
    pub throttle_time_ms: i32,
    /// Per-topic results.
    pub topics: Vec<CreateTopicsResponseTopic>,
}

/// Encodes a response body at v2-v7. Unsupported config values are omitted;
/// the broker never claims to have applied one.
pub fn encode_response(out: &mut Vec<u8>, version: i16, response: &CreateTopicsResponse) {
    let flexible = version >= 5;
    put_i32(out, response.throttle_time_ms);
    put_array_len(out, flexible, Some(response.topics.len()));
    let empty = TaggedFields::default();
    for topic in &response.topics {
        put_string(out, flexible, &topic.name);
        if version >= 7 {
            out.extend_from_slice(&topic.topic_id);
        }
        put_i16(out, topic.error_code);
        put_nullable_string(out, flexible, topic.error_message.as_deref());
        if version >= 5 {
            put_i32(out, topic.num_partitions);
            put_i16(out, topic.replication_factor);
            // Kafka clients decode topic-config errors as a non-null array;
            // an empty array means the broker accepted no per-config errors.
            put_array_len(out, flexible, Some(0));
            put_tagged_fields(out, &empty);
        }
        // v5+ topic_config_error_code is tag 0; the broker returns no config
        // data and therefore emits an empty tag buffer for the result.
    }
    if flexible {
        put_tagged_fields(out, &empty);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]
    use super::{CreateTopicsRequest, decode_request};
    use crate::flex::{
        TaggedFields, put_array_len, put_nullable_string, put_string, put_tagged_fields,
    };
    use crate::wire::{put_i16, put_i32};

    #[test]
    fn decodes_the_non_flexible_request_shape() {
        let mut body = Vec::new();
        put_array_len(&mut body, false, Some(1));
        put_string(&mut body, false, "orders");
        put_i32(&mut body, 3);
        put_i16(&mut body, -1);
        put_array_len(&mut body, false, Some(0));
        put_array_len(&mut body, false, Some(0));
        put_i32(&mut body, 10_000);
        body.push(0);
        let request = decode_request(&body, 2).expect("v2 request");
        assert_eq!(
            request,
            CreateTopicsRequest {
                topics: vec![super::CreatableTopic {
                    name: "orders".to_owned(),
                    num_partitions: 3,
                    replication_factor: -1,
                    assignments: Vec::new(),
                    configs: Vec::new(),
                }],
                timeout_ms: 10_000,
                validate_only: false,
            }
        );
    }

    #[test]
    fn decodes_the_flexible_request_shape_and_tagged_fields() {
        let mut body = Vec::new();
        put_array_len(&mut body, true, Some(1));
        put_string(&mut body, true, "orders");
        put_i32(&mut body, 3);
        put_i16(&mut body, 1);
        put_array_len(&mut body, true, Some(0));
        put_array_len(&mut body, true, Some(1));
        put_string(&mut body, true, "cleanup.policy");
        put_nullable_string(&mut body, true, Some("compact"));
        put_tagged_fields(&mut body, &TaggedFields::default());
        put_tagged_fields(&mut body, &TaggedFields::default());
        put_i32(&mut body, 10_000);
        body.push(1);
        put_tagged_fields(&mut body, &TaggedFields::default());
        let request = decode_request(&body, 5).expect("v5 request");
        assert_eq!(
            request.topics[0].configs[0].value.as_deref(),
            Some("compact")
        );
        assert!(request.validate_only);
    }

    #[test]
    fn encodes_responses_for_each_advertised_shape() {
        use kafka_protocol::messages::CreateTopicsResponse as KpResponse;
        use kafka_protocol::protocol::Decodable;

        for version in [2, 5, 7] {
            let response = super::CreateTopicsResponse {
                throttle_time_ms: 17,
                topics: vec![super::CreateTopicsResponseTopic {
                    name: "orders".to_owned(),
                    topic_id: [7; 16],
                    error_code: 36,
                    error_message: Some("already exists".to_owned()),
                    num_partitions: 3,
                    replication_factor: 1,
                }],
            };
            let mut bytes = Vec::new();
            super::encode_response(&mut bytes, version, &response);
            let mut rest = &bytes[..];
            let decoded = KpResponse::decode(&mut rest, version).expect("oracle decodes ours");
            assert!(rest.is_empty(), "v{version}: whole body consumed");
            assert_eq!(decoded.throttle_time_ms, 17, "v{version}");
            assert_eq!(decoded.topics.len(), 1, "v{version}");
            assert_eq!(decoded.topics[0].name.as_str(), "orders", "v{version}");
            assert_eq!(decoded.topics[0].error_code, 36, "v{version}");
            assert_eq!(
                decoded.topics[0].error_message.as_deref(),
                Some("already exists"),
                "v{version}"
            );
            if version >= 5 {
                assert_eq!(decoded.topics[0].num_partitions, 3, "v{version}");
                assert_eq!(decoded.topics[0].replication_factor, 1, "v{version}");
            }
            if version >= 7 {
                assert_eq!(
                    decoded.topics[0].topic_id.as_bytes(),
                    &[7; 16],
                    "v{version}"
                );
            }
        }
    }
}
