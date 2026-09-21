//! `DeleteTopics` v0-v6, hand-rolled (`ADR-0019`).

use crate::flex::{
    TaggedFields, put_array_len, put_nullable_string, put_string, put_tagged_fields,
    read_array_len, read_nullable_string, read_string, read_tagged_fields,
};
use crate::wire::{Cursor, DecodeError, put_i16, put_i32};

/// One topic named by a `DeleteTopics` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletableTopic {
    /// The name, nullable only in v6 when the UUID is the address.
    pub name: Option<String>,
    /// The UUID, meaningful in v6 and zero in older versions.
    pub topic_id: [u8; 16],
}

/// A `DeleteTopics` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteTopicsRequest {
    /// Topics to delete.
    pub topics: Vec<DeletableTopic>,
    /// Client timeout, carried for protocol compatibility.
    pub timeout_ms: i32,
}

/// Decodes a v0-v6 request body.
///
/// # Errors
///
/// Returns a decode error when the body is truncated, malformed, or contains
/// trailing bytes.
pub fn decode_request(body: &[u8], version: i16) -> Result<DeleteTopicsRequest, DecodeError> {
    let flexible = version >= 4;
    let mut cur = Cursor::new(body);
    let count = read_array_len(&mut cur, flexible)?
        .ok_or_else(|| DecodeError::UnexpectedNull { at: cur.position() })?;
    let mut topics = Vec::new();
    for _ in 0..count {
        if version >= 6 {
            let name = read_nullable_string(&mut cur, true)?.map(str::to_owned);
            let topic_id = cur
                .take(16)?
                .try_into()
                .map_err(|_| DecodeError::UnexpectedEof {
                    needed: 16,
                    remaining: 0,
                    at: cur.position(),
                })?;
            let _ = read_tagged_fields(&mut cur)?;
            topics.push(DeletableTopic { name, topic_id });
        } else {
            topics.push(DeletableTopic {
                name: Some(read_string(&mut cur, flexible)?.to_owned()),
                topic_id: [0; 16],
            });
        }
    }
    let timeout_ms = cur.read_i32()?;
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
    Ok(DeleteTopicsRequest { topics, timeout_ms })
}

/// One topic's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteTopicsResponseTopic {
    /// The returned name, nullable in v6.
    pub name: Option<String>,
    /// The returned UUID, present in v6.
    pub topic_id: [u8; 16],
    /// Result code.
    pub error_code: i16,
    /// Optional diagnostic, present in v5+.
    pub error_message: Option<String>,
}

/// A `DeleteTopics` response body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeleteTopicsResponse {
    /// Throttle duration.
    pub throttle_time_ms: i32,
    /// Per-topic results.
    pub responses: Vec<DeleteTopicsResponseTopic>,
}

/// Encodes a v0-v6 response body.
pub fn encode_response(out: &mut Vec<u8>, version: i16, response: &DeleteTopicsResponse) {
    let flexible = version >= 4;
    if version >= 1 {
        put_i32(out, response.throttle_time_ms);
    }
    put_array_len(out, flexible, Some(response.responses.len()));
    let empty = TaggedFields::default();
    for topic in &response.responses {
        if version >= 6 {
            put_nullable_string(out, true, topic.name.as_deref());
            out.extend_from_slice(&topic.topic_id);
            put_i16(out, topic.error_code);
            put_nullable_string(out, true, topic.error_message.as_deref());
            put_tagged_fields(out, &empty);
        } else {
            put_string(out, flexible, topic.name.as_deref().unwrap_or_default());
            put_i16(out, topic.error_code);
            if version >= 5 {
                put_nullable_string(out, true, topic.error_message.as_deref());
            }
            if version >= 4 {
                put_tagged_fields(out, &empty);
            }
        }
    }
    if flexible {
        put_tagged_fields(out, &empty);
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{DeleteTopicsResponse, DeleteTopicsResponseTopic, decode_request, encode_response};
    use crate::flex::{
        TaggedFields, put_array_len, put_nullable_string, put_string, put_tagged_fields,
    };
    use crate::wire::put_i32;

    #[test]
    fn decodes_name_requests_before_and_after_flexible_cutover() {
        for version in [0, 3, 4, 5] {
            let flexible = version >= 4;
            let mut body = Vec::new();
            put_array_len(&mut body, flexible, Some(1));
            put_string(&mut body, flexible, "orders");
            put_i32(&mut body, 5_000);
            if flexible {
                put_tagged_fields(&mut body, &TaggedFields::default());
            }
            let request = decode_request(&body, version).expect("request");
            assert_eq!(request.timeout_ms, 5_000);
            assert_eq!(request.topics[0].name.as_deref(), Some("orders"));
        }
    }

    #[test]
    fn decodes_uuid_addressed_v6_request() {
        let mut body = Vec::new();
        put_array_len(&mut body, true, Some(1));
        put_nullable_string(&mut body, true, None);
        body.extend_from_slice(&[7; 16]);
        put_tagged_fields(&mut body, &TaggedFields::default());
        put_i32(&mut body, 1_000);
        put_tagged_fields(&mut body, &TaggedFields::default());
        let request = decode_request(&body, 6).expect("v6 request");
        assert_eq!(request.topics[0].name, None);
        assert_eq!(request.topics[0].topic_id, [7; 16]);
    }

    #[test]
    fn response_round_trips_every_protocol_shape() {
        for version in 0..=6 {
            let response = DeleteTopicsResponse {
                throttle_time_ms: 17,
                responses: vec![DeleteTopicsResponseTopic {
                    name: Some("orders".to_owned()),
                    topic_id: [9; 16],
                    error_code: 0,
                    error_message: (version >= 5).then(|| "gone".to_owned()),
                }],
            };
            let mut bytes = Vec::new();
            encode_response(&mut bytes, version, &response);
            assert!(!bytes.is_empty(), "v{version}");
        }
    }

    #[test]
    fn responses_decode_under_the_protocol_oracle() {
        use kafka_protocol::messages::DeleteTopicsResponse as KpResponse;
        use kafka_protocol::protocol::Decodable;

        for version in 1..=6 {
            let response = DeleteTopicsResponse {
                throttle_time_ms: 17,
                responses: vec![DeleteTopicsResponseTopic {
                    name: Some("orders".to_owned()),
                    topic_id: [9; 16],
                    error_code: 3,
                    error_message: (version >= 5).then(|| "missing".to_owned()),
                }],
            };
            let mut bytes = Vec::new();
            encode_response(&mut bytes, version, &response);
            let mut rest = &bytes[..];
            let decoded = KpResponse::decode(&mut rest, version).expect("oracle decodes");
            assert!(rest.is_empty(), "v{version}: whole body consumed");
            assert_eq!(decoded.throttle_time_ms, 17, "v{version}");
            assert_eq!(decoded.responses.len(), 1, "v{version}");
            assert_eq!(decoded.responses[0].error_code, 3, "v{version}");
        }
    }
}
