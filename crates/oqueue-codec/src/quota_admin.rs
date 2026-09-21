//! Kafka client-quota administration, `DescribeClientQuotas` and
//! `AlterClientQuotas`, versions 0 and 1.
//!
//! Oqueue supports one deliberately narrow quota key: `in_flight_requests`
//! on a named `user` entity. The wire shapes remain Kafka-compatible while
//! unsupported entity kinds and quota keys are rejected by the broker.

#![allow(clippy::derive_partial_eq_without_eq)]

use crate::flex::{
    TaggedFields, put_array_len, put_nullable_string, put_string, put_tagged_fields,
    read_array_len, read_nullable_string, read_string, read_tagged_fields,
};
use crate::wire::{Cursor, DecodeError, put_f64, put_i16, put_i32};

/// The only quota key this broker enforces.
pub const IN_FLIGHT_REQUESTS: &str = "in_flight_requests";
/// The Kafka entity kind used for principal-scoped overrides.
pub const USER_ENTITY: &str = "user";

/// One `DescribeClientQuotas` filter component.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaFilter {
    /// Entity kind, supported as `user`.
    pub entity_type: String,
    /// Kafka match mode: exact (`0`) or any (`2`).
    pub match_type: i8,
    /// Exact principal name, or absent for an any-user filter.
    pub name: Option<String>,
}

/// A `DescribeClientQuotas` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescribeClientQuotasRequest {
    /// Entity filters.
    pub components: Vec<QuotaFilter>,
    /// Whether unspecified entity kinds must be excluded.
    pub strict: bool,
}

/// One described quota entity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuotaEntity {
    /// Entity kind.
    pub entity_type: String,
    /// Entity name, absent only for a default entity.
    pub name: Option<String>,
}

/// One described quota value.
#[derive(Debug, Clone, PartialEq)]
pub struct QuotaValue {
    /// Quota key.
    pub key: String,
    /// Quota value.
    pub value: f64,
}

/// One `DescribeClientQuotas` result entry.
#[derive(Debug, Clone, PartialEq)]
pub struct DescribeClientQuotasEntry {
    /// Entity identity.
    pub entity: Vec<QuotaEntity>,
    /// Values enforced for the entity.
    pub values: Vec<QuotaValue>,
}

/// A `DescribeClientQuotas` response.
#[derive(Debug, Clone, PartialEq)]
pub struct DescribeClientQuotasResponse {
    /// Throttle duration in milliseconds.
    pub throttle_time_ms: i32,
    /// Top-level error code.
    pub error_code: i16,
    /// Safe diagnostic for the top-level error.
    pub error_message: Option<String>,
    /// Matching quota entries.
    pub entries: Vec<DescribeClientQuotasEntry>,
}

/// One quota operation in an `AlterClientQuotas` request.
#[derive(Debug, Clone, PartialEq)]
pub struct QuotaOperation {
    /// Quota key.
    pub key: String,
    /// New value, ignored when removing.
    pub value: f64,
    /// Whether to remove the override.
    pub remove: bool,
}

/// One `AlterClientQuotas` request entry.
#[derive(Debug, Clone, PartialEq)]
pub struct AlterClientQuotasEntry {
    /// Entity identity.
    pub entity: Vec<QuotaEntity>,
    /// Requested operations.
    pub operations: Vec<QuotaOperation>,
}

/// An `AlterClientQuotas` request.
#[derive(Debug, Clone, PartialEq)]
pub struct AlterClientQuotasRequest {
    /// Entries to alter.
    pub entries: Vec<AlterClientQuotasEntry>,
    /// Validate without applying.
    pub validate_only: bool,
}

/// One `AlterClientQuotas` result entry.
#[derive(Debug, Clone, PartialEq)]
pub struct AlterClientQuotasResult {
    /// Entry error code.
    pub error_code: i16,
    /// Safe diagnostic for the entry error.
    pub error_message: Option<String>,
    /// Echoed entity identity.
    pub entity: Vec<QuotaEntity>,
}

/// An `AlterClientQuotas` response.
#[derive(Debug, Clone, PartialEq)]
pub struct AlterClientQuotasResponse {
    /// Throttle duration in milliseconds.
    pub throttle_time_ms: i32,
    /// Per-entry results.
    pub entries: Vec<AlterClientQuotasResult>,
}

/// Decodes a v0-v1 `DescribeClientQuotas` body.
///
/// # Errors
///
/// Returns [`DecodeError`] when a field is malformed, truncated, or trailing
/// bytes remain after the versioned request shape.
pub fn decode_describe_request(
    body: &[u8],
    version: i16,
) -> Result<DescribeClientQuotasRequest, DecodeError> {
    let flexible = version >= 1;
    let mut cur = Cursor::new(body);
    let count = read_array_len(&mut cur, flexible)?
        .ok_or_else(|| DecodeError::UnexpectedNull { at: cur.position() })?;
    let mut components = Vec::new();
    for _ in 0..count {
        let entity_type = read_string(&mut cur, flexible)?.to_owned();
        let match_type = cur.read_i8()?;
        let name = read_nullable_string(&mut cur, flexible)?.map(str::to_owned);
        if flexible {
            read_tagged_fields(&mut cur)?;
        }
        components.push(QuotaFilter {
            entity_type,
            match_type,
            name,
        });
    }
    let strict = cur.read_bool()?;
    if flexible {
        read_tagged_fields(&mut cur)?;
    }
    finish(cur, DescribeClientQuotasRequest { components, strict })
}

/// Decodes a v0-v1 `AlterClientQuotas` body.
///
/// # Errors
///
/// Returns [`DecodeError`] when a field is malformed, truncated, or trailing
/// bytes remain after the versioned request shape.
pub fn decode_alter_request(
    body: &[u8],
    version: i16,
) -> Result<AlterClientQuotasRequest, DecodeError> {
    let flexible = version >= 1;
    let mut cur = Cursor::new(body);
    let count = read_array_len(&mut cur, flexible)?
        .ok_or_else(|| DecodeError::UnexpectedNull { at: cur.position() })?;
    let mut entries = Vec::new();
    for _ in 0..count {
        let entity = decode_entities(&mut cur, flexible)?;
        let op_count = read_array_len(&mut cur, flexible)?
            .ok_or_else(|| DecodeError::UnexpectedNull { at: cur.position() })?;
        let mut operations = Vec::new();
        for _ in 0..op_count {
            let key = read_string(&mut cur, flexible)?.to_owned();
            let value = cur.read_f64()?;
            let remove = cur.read_bool()?;
            if flexible {
                read_tagged_fields(&mut cur)?;
            }
            operations.push(QuotaOperation { key, value, remove });
        }
        if flexible {
            read_tagged_fields(&mut cur)?;
        }
        entries.push(AlterClientQuotasEntry { entity, operations });
    }
    let validate_only = cur.read_bool()?;
    if flexible {
        read_tagged_fields(&mut cur)?;
    }
    finish(
        cur,
        AlterClientQuotasRequest {
            entries,
            validate_only,
        },
    )
}

/// Encodes a v0-v1 `DescribeClientQuotas` response body.
pub fn encode_describe_response(
    out: &mut Vec<u8>,
    version: i16,
    response: &DescribeClientQuotasResponse,
) {
    let flexible = version >= 1;
    let empty = TaggedFields::default();
    put_i32(out, response.throttle_time_ms);
    put_i16(out, response.error_code);
    put_nullable_string(out, flexible, response.error_message.as_deref());
    put_array_len(out, flexible, Some(response.entries.len()));
    for entry in &response.entries {
        encode_entities(out, flexible, &entry.entity);
        put_array_len(out, flexible, Some(entry.values.len()));
        for value in &entry.values {
            put_string(out, flexible, &value.key);
            put_f64(out, value.value);
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

/// Encodes a v0-v1 `AlterClientQuotas` response body.
pub fn encode_alter_response(
    out: &mut Vec<u8>,
    version: i16,
    response: &AlterClientQuotasResponse,
) {
    let flexible = version >= 1;
    let empty = TaggedFields::default();
    put_i32(out, response.throttle_time_ms);
    put_array_len(out, flexible, Some(response.entries.len()));
    for entry in &response.entries {
        put_i16(out, entry.error_code);
        put_nullable_string(out, flexible, entry.error_message.as_deref());
        encode_entities(out, flexible, &entry.entity);
        if flexible {
            put_tagged_fields(out, &empty);
        }
    }
    if flexible {
        put_tagged_fields(out, &empty);
    }
}

fn decode_entities(cur: &mut Cursor<'_>, flexible: bool) -> Result<Vec<QuotaEntity>, DecodeError> {
    let count = read_array_len(cur, flexible)?
        .ok_or_else(|| DecodeError::UnexpectedNull { at: cur.position() })?;
    let mut entities = Vec::new();
    for _ in 0..count {
        let entity_type = read_string(cur, flexible)?.to_owned();
        let name = read_nullable_string(cur, flexible)?.map(str::to_owned);
        if flexible {
            read_tagged_fields(cur)?;
        }
        entities.push(QuotaEntity { entity_type, name });
    }
    Ok(entities)
}

fn encode_entities(out: &mut Vec<u8>, flexible: bool, entities: &[QuotaEntity]) {
    let empty = TaggedFields::default();
    put_array_len(out, flexible, Some(entities.len()));
    for entity in entities {
        put_string(out, flexible, &entity.entity_type);
        put_nullable_string(out, flexible, entity.name.as_deref());
        if flexible {
            put_tagged_fields(out, &empty);
        }
    }
}

fn finish<T>(cur: Cursor<'_>, value: T) -> Result<T, DecodeError> {
    if cur.remaining() != 0 {
        return Err(DecodeError::LengthOutOfBounds {
            length: cur.remaining() as u64,
            max: 0,
            at: cur.position(),
        });
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::*;
    use crate::flex::{put_array_len, put_tagged_fields};
    use crate::wire::{put_bool, put_i8};

    fn entity(name: Option<&str>) -> QuotaEntity {
        QuotaEntity {
            entity_type: USER_ENTITY.to_owned(),
            name: name.map(str::to_owned),
        }
    }

    #[test]
    fn v0_request_and_responses_match_the_protocol_oracle() {
        use kafka_protocol::messages::{
            AlterClientQuotasResponse as KpAlterResponse,
            DescribeClientQuotasResponse as KpDescribeResponse,
        };
        use kafka_protocol::protocol::Decodable;

        let mut describe = Vec::new();
        put_array_len(&mut describe, false, Some(1));
        put_string(&mut describe, false, USER_ENTITY);
        put_i8(&mut describe, 0);
        put_nullable_string(&mut describe, false, Some("alice"));
        put_bool(&mut describe, true);
        let request = decode_describe_request(&describe, 0).expect("describe request");
        assert_eq!(request.components[0].name.as_deref(), Some("alice"));

        let response = DescribeClientQuotasResponse {
            throttle_time_ms: 0,
            error_code: 0,
            error_message: None,
            entries: vec![DescribeClientQuotasEntry {
                entity: vec![entity(Some("alice"))],
                values: vec![QuotaValue {
                    key: IN_FLIGHT_REQUESTS.to_owned(),
                    value: 3.0,
                }],
            }],
        };
        let mut bytes = Vec::new();
        encode_describe_response(&mut bytes, 0, &response);
        let mut rest = &bytes[..];
        let decoded = KpDescribeResponse::decode(&mut rest, 0).expect("describe response");
        assert!(rest.is_empty());
        assert_eq!(decoded.entries.expect("entries").len(), 1);

        let alter = AlterClientQuotasResponse {
            throttle_time_ms: 0,
            entries: vec![AlterClientQuotasResult {
                error_code: 0,
                error_message: None,
                entity: vec![entity(Some("alice"))],
            }],
        };
        bytes.clear();
        encode_alter_response(&mut bytes, 0, &alter);
        let mut rest = &bytes[..];
        let decoded = KpAlterResponse::decode(&mut rest, 0).expect("alter response");
        assert!(rest.is_empty());
        assert_eq!(decoded.entries.len(), 1);
    }

    #[test]
    fn flexible_alter_request_preserves_the_version_one_shape() {
        let mut body = Vec::new();
        put_array_len(&mut body, true, Some(1));
        put_array_len(&mut body, true, Some(1));
        put_string(&mut body, true, USER_ENTITY);
        put_nullable_string(&mut body, true, Some("alice"));
        put_tagged_fields(&mut body, &TaggedFields::default());
        put_array_len(&mut body, true, Some(1));
        put_string(&mut body, true, IN_FLIGHT_REQUESTS);
        put_f64(&mut body, 2.0);
        put_bool(&mut body, false);
        put_tagged_fields(&mut body, &TaggedFields::default());
        put_tagged_fields(&mut body, &TaggedFields::default());
        put_bool(&mut body, false);
        put_tagged_fields(&mut body, &TaggedFields::default());
        let request = decode_alter_request(&body, 1).expect("alter request");
        assert!((request.entries[0].operations[0].value - 2.0).abs() < f64::EPSILON);
    }
}
