//! `IncrementalAlterConfigs` v0-v1, hand-rolled (`ADR-0019`).

use crate::alter_configs::{AlterConfigsResponse, AlterConfigsResponseResource};
use crate::flex::{
    TaggedFields, put_array_len, put_nullable_string, put_string, put_tagged_fields,
    read_array_len, read_nullable_string, read_string, read_tagged_fields,
};
use crate::wire::{Cursor, DecodeError, put_i8, put_i16, put_i32};

/// Kafka's incremental configuration operation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i8)]
pub enum ConfigOperation {
    /// Replace the scalar value.
    Set = 0,
    /// Restore the effective default.
    Delete = 1,
    /// List append, unsupported for `retention.ms`.
    Append = 2,
    /// List subtraction, unsupported for `retention.ms`.
    Subtract = 3,
}

impl ConfigOperation {
    fn from_i8(value: i8) -> Result<Self, DecodeError> {
        match value {
            0 => Ok(Self::Set),
            1 => Ok(Self::Delete),
            2 => Ok(Self::Append),
            3 => Ok(Self::Subtract),
            other => Err(DecodeError::LengthOutOfBounds {
                length: u64::from(other.cast_unsigned()),
                max: 3,
                at: 0,
            }),
        }
    }
}

/// One incremental configuration entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncrementalConfig {
    /// Configuration key.
    pub name: String,
    /// Operation to apply.
    pub operation: ConfigOperation,
    /// Value for `SET` or `APPEND`; null for `DELETE`.
    pub value: Option<String>,
}

/// One incremental resource.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncrementalConfigsResource {
    /// Kafka resource type.
    pub resource_type: i8,
    /// Resource name.
    pub resource_name: String,
    /// Changes to apply.
    pub configs: Vec<IncrementalConfig>,
}

/// An incremental request body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncrementalAlterConfigsRequest {
    /// Resources to update.
    pub resources: Vec<IncrementalConfigsResource>,
    /// Validate without persisting.
    pub validate_only: bool,
}

/// The response body reuses `AlterConfigs`' resource result shape.
pub type IncrementalAlterConfigsResponse = AlterConfigsResponse;
/// One resource result in an incremental alteration response.
pub type IncrementalAlterConfigsResponseResource = AlterConfigsResponseResource;

/// Decodes an incremental request at v0-v1.
///
/// # Errors
///
/// Returns a decode error when the request is truncated, malformed, or has
/// trailing bytes.
pub fn decode_request(
    body: &[u8],
    version: i16,
) -> Result<IncrementalAlterConfigsRequest, DecodeError> {
    let flexible = version >= 1;
    let mut cur = Cursor::new(body);
    let count = read_array_len(&mut cur, flexible)?
        .ok_or_else(|| DecodeError::UnexpectedNull { at: cur.position() })?;
    let mut resources = Vec::new();
    for _ in 0..count {
        let resource_type = cur.read_i8()?;
        let resource_name = read_string(&mut cur, flexible)?.to_owned();
        let config_count = read_array_len(&mut cur, flexible)?
            .ok_or_else(|| DecodeError::UnexpectedNull { at: cur.position() })?;
        let mut configs = Vec::new();
        for _ in 0..config_count {
            let name = read_string(&mut cur, flexible)?.to_owned();
            let operation = ConfigOperation::from_i8(cur.read_i8()?)?;
            let value = read_nullable_string(&mut cur, flexible)?.map(str::to_owned);
            if flexible {
                let _ = read_tagged_fields(&mut cur)?;
            }
            configs.push(IncrementalConfig {
                name,
                operation,
                value,
            });
        }
        if flexible {
            let _ = read_tagged_fields(&mut cur)?;
        }
        resources.push(IncrementalConfigsResource {
            resource_type,
            resource_name,
            configs,
        });
    }
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
    Ok(IncrementalAlterConfigsRequest {
        resources,
        validate_only,
    })
}

/// Encodes the shared response shape at v0-v1.
pub fn encode_response(
    out: &mut Vec<u8>,
    version: i16,
    response: &IncrementalAlterConfigsResponse,
) {
    let flexible = version >= 1;
    let empty = TaggedFields::default();
    put_i32(out, response.throttle_time_ms);
    put_array_len(out, flexible, Some(response.resources.len()));
    for resource in &response.resources {
        put_i16(out, resource.error_code);
        put_nullable_string(out, flexible, resource.error_message.as_deref());
        put_i8(out, resource.resource_type);
        put_string(out, flexible, &resource.resource_name);
        if flexible {
            put_tagged_fields(out, &empty);
        }
    }
    if flexible {
        put_tagged_fields(out, &empty);
    }
}

#[cfg(test)]
mod tests;
