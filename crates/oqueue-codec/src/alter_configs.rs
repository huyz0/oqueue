//! `AlterConfigs` v0-v3, hand-rolled (`ADR-0019`).

use crate::flex::{
    TaggedFields, put_array_len, put_nullable_string, put_string, put_tagged_fields,
    read_array_len, read_nullable_string, read_string, read_tagged_fields,
};
use crate::wire::{Cursor, DecodeError, put_i8, put_i16, put_i32};

/// One resource in an `AlterConfigs` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlterConfigsResource {
    /// Kafka resource type.
    pub resource_type: i8,
    /// Resource name.
    pub resource_name: String,
    /// Full replacement configuration entries.
    pub configs: Vec<AlterConfig>,
}

/// One full-replacement configuration entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlterConfig {
    /// Configuration key.
    pub name: String,
    /// New value, or null to reset this key.
    pub value: Option<String>,
}

/// An `AlterConfigs` request body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlterConfigsRequest {
    /// Resources to replace.
    pub resources: Vec<AlterConfigsResource>,
    /// Validate without persisting.
    pub validate_only: bool,
}

/// One resource result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlterConfigsResponseResource {
    /// Resource-level error code.
    pub error_code: i16,
    /// Safe diagnostic.
    pub error_message: Option<String>,
    /// Resource type echoed from the request.
    pub resource_type: i8,
    /// Resource name echoed from the request.
    pub resource_name: String,
}

/// An `AlterConfigs` response body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlterConfigsResponse {
    /// Throttle duration.
    pub throttle_time_ms: i32,
    /// Per-resource results.
    pub resources: Vec<AlterConfigsResponseResource>,
}

/// Decodes an `AlterConfigs` request at v0-v3.
///
/// # Errors
///
/// Returns a decode error when the request is truncated, malformed, or has
/// trailing bytes.
pub fn decode_request(body: &[u8], version: i16) -> Result<AlterConfigsRequest, DecodeError> {
    let flexible = version >= 2;
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
            let value = read_nullable_string(&mut cur, flexible)?.map(str::to_owned);
            if flexible {
                let _ = read_tagged_fields(&mut cur)?;
            }
            configs.push(AlterConfig { name, value });
        }
        if flexible {
            let _ = read_tagged_fields(&mut cur)?;
        }
        resources.push(AlterConfigsResource {
            resource_type,
            resource_name,
            configs,
        });
    }
    let validate_only = if version >= 1 {
        cur.read_bool()?
    } else {
        false
    };
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
    Ok(AlterConfigsRequest {
        resources,
        validate_only,
    })
}

/// Encodes an `AlterConfigs` response at v0-v3.
pub fn encode_response(out: &mut Vec<u8>, version: i16, response: &AlterConfigsResponse) {
    let flexible = version >= 2;
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
mod tests {
    #![allow(clippy::expect_used)]

    use super::{
        AlterConfigsResponse, AlterConfigsResponseResource, decode_request, encode_response,
    };
    use crate::flex::{
        TaggedFields, put_array_len, put_nullable_string, put_string, put_tagged_fields,
    };
    use crate::wire::{put_bool, put_i8};

    #[test]
    fn decodes_replacement_and_reset_entries_in_flexible_version() {
        let mut body = Vec::new();
        put_array_len(&mut body, true, Some(1));
        put_i8(&mut body, 2);
        put_string(&mut body, true, "orders");
        put_array_len(&mut body, true, Some(2));
        put_string(&mut body, true, "retention.ms");
        put_nullable_string(&mut body, true, Some("900000"));
        put_tagged_fields(&mut body, &TaggedFields::default());
        put_string(&mut body, true, "cleanup.policy");
        put_nullable_string(&mut body, true, None);
        put_tagged_fields(&mut body, &TaggedFields::default());
        put_tagged_fields(&mut body, &TaggedFields::default());
        put_bool(&mut body, true);
        put_tagged_fields(&mut body, &TaggedFields::default());
        let request = decode_request(&body, 2).expect("request");
        assert!(request.validate_only);
        assert_eq!(
            request.resources[0].configs[0].value.as_deref(),
            Some("900000")
        );
        assert_eq!(request.resources[0].configs[1].value, None);
    }

    #[test]
    fn response_encoding_preserves_the_flexible_tag_section() {
        let response = AlterConfigsResponse {
            throttle_time_ms: 7,
            resources: vec![AlterConfigsResponseResource {
                error_code: 0,
                error_message: Some("ok".to_owned()),
                resource_type: 2,
                resource_name: "orders".to_owned(),
            }],
        };
        let mut legacy = Vec::new();
        encode_response(&mut legacy, 1, &response);
        let mut flexible = Vec::new();
        encode_response(&mut flexible, 2, &response);
        assert!(flexible.len() < legacy.len());
        assert_eq!(&flexible[..4], &legacy[..4]);
    }
}
