//! `DescribeConfigs` v0-v4, hand-rolled (`ADR-0019`).

use crate::flex::{
    TaggedFields, put_array_len, put_nullable_string, put_string, put_tagged_fields,
    read_array_len, read_string, read_tagged_fields,
};
use crate::wire::{Cursor, DecodeError, put_bool, put_i8, put_i16, put_i32};

/// A resource named by a `DescribeConfigs` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigResource {
    /// Kafka resource type. Topic is `2`; other types are refused by the
    /// broker because this single-node implementation has no broker config
    /// surface.
    pub resource_type: i8,
    /// The resource name. Topic names are non-null on the wire.
    pub resource_name: String,
    /// Requested keys, or `None` to request every supported key.
    pub config_names: Option<Vec<String>>,
}

/// A `DescribeConfigs` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescribeConfigsRequest {
    /// Resources to describe.
    pub resources: Vec<ConfigResource>,
    /// Kafka synonym flag, decoded for wire compatibility but unsupported.
    pub include_synonyms: bool,
    /// Kafka documentation flag, decoded for wire compatibility but ignored.
    pub include_documentation: bool,
}

/// Decodes a v0-v4 request body.
///
/// # Errors
/// Returns [`DecodeError`] for malformed, truncated, or trailing input.
pub fn decode_request(body: &[u8], version: i16) -> Result<DescribeConfigsRequest, DecodeError> {
    let flexible = version >= 4;
    let mut cur = Cursor::new(body);
    let count = read_array_len(&mut cur, flexible)?
        .ok_or_else(|| DecodeError::UnexpectedNull { at: cur.position() })?;
    let mut resources = Vec::new();
    for _ in 0..count {
        resources.push(decode_resource(&mut cur, flexible)?);
    }
    let include_synonyms = if version >= 1 {
        cur.read_bool()?
    } else {
        false
    };
    let include_documentation = if version >= 3 {
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
    Ok(DescribeConfigsRequest {
        resources,
        include_synonyms,
        include_documentation,
    })
}

fn decode_resource(cur: &mut Cursor<'_>, flexible: bool) -> Result<ConfigResource, DecodeError> {
    let resource_type = cur.read_i8()?;
    let resource_name = read_string(cur, flexible)?.to_owned();
    let config_names = read_array_len(cur, flexible)?
        .map(|count| {
            let mut names = Vec::new();
            for _ in 0..count {
                names.push(read_string(cur, flexible).map(str::to_owned)?);
            }
            Ok::<_, DecodeError>(names)
        })
        .transpose()?;
    if flexible {
        let _ = read_tagged_fields(cur)?;
    }
    Ok(ConfigResource {
        resource_type,
        resource_name,
        config_names,
    })
}

/// One configuration entry in a response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigEntry {
    /// Configuration key.
    pub name: String,
    /// Effective value, or null when Kafka's config model has no value.
    pub value: Option<String>,
    /// Whether the broker permits this setting to be changed.
    pub read_only: bool,
    /// Kafka v1+ config source. `5` is the protocol's default source.
    pub config_source: i8,
    /// Whether the value is secret. Supported entries are never sensitive.
    pub is_sensitive: bool,
}

/// One resource's result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescribeConfigsResponseResource {
    /// Resource-level result code.
    pub error_code: i16,
    /// Safe diagnostic, never containing a requested secret or key value.
    pub error_message: Option<String>,
    /// Resource type echoed from the request.
    pub resource_type: i8,
    /// Resource name echoed from the request.
    pub resource_name: String,
    /// Supported entries, empty on a resource error.
    pub configs: Vec<ConfigEntry>,
}

/// A `DescribeConfigs` response body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescribeConfigsResponse {
    /// Throttle duration.
    pub throttle_time_ms: i32,
    /// Per-resource results.
    pub resources: Vec<DescribeConfigsResponseResource>,
}

/// Encodes a v0-v4 response body.
pub fn encode_response(out: &mut Vec<u8>, version: i16, response: &DescribeConfigsResponse) {
    let flexible = version >= 4;
    put_i32(out, response.throttle_time_ms);
    put_array_len(out, flexible, Some(response.resources.len()));
    let empty = TaggedFields::default();
    for resource in &response.resources {
        put_i16(out, resource.error_code);
        put_nullable_string(out, flexible, resource.error_message.as_deref());
        put_i8(out, resource.resource_type);
        put_string(out, flexible, &resource.resource_name);
        put_array_len(out, flexible, Some(resource.configs.len()));
        for config in &resource.configs {
            put_string(out, flexible, &config.name);
            put_nullable_string(out, flexible, config.value.as_deref());
            put_bool(out, config.read_only);
            if version == 0 {
                put_bool(out, config.config_source == 5);
                put_bool(out, config.is_sensitive);
            } else {
                put_i8(out, config.config_source);
                put_bool(out, config.is_sensitive);
                put_array_len(out, flexible, Some(0));
                if version >= 3 {
                    // Kafka's LONG type is 4. The broker does not maintain
                    // per-setting documentation yet, so the field is null.
                    put_i8(out, 4);
                    put_nullable_string(out, flexible, None);
                }
                if flexible {
                    put_tagged_fields(out, &empty);
                }
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

    use super::{ConfigEntry, DescribeConfigsResponse, DescribeConfigsResponseResource};
    use crate::flex::{TaggedFields, put_array_len, put_string, put_tagged_fields};
    use crate::wire::{put_bool, put_i8};

    #[test]
    fn decodes_a_flexible_topic_request_with_selected_keys() {
        let mut body = Vec::new();
        put_array_len(&mut body, true, Some(1));
        put_i8(&mut body, 2);
        put_string(&mut body, true, "orders");
        put_array_len(&mut body, true, Some(1));
        put_string(&mut body, true, "retention.ms");
        put_tagged_fields(&mut body, &TaggedFields::default());
        put_bool(&mut body, false);
        put_bool(&mut body, false);
        put_tagged_fields(&mut body, &TaggedFields::default());

        let request = super::decode_request(&body, 4).expect("v4 request");
        assert_eq!(request.resources[0].resource_type, 2);
        assert_eq!(request.resources[0].resource_name, "orders");
        assert_eq!(
            request.resources[0].config_names.as_deref(),
            Some([String::from("retention.ms")].as_slice())
        );
        assert!(!request.include_synonyms);
        assert!(!request.include_documentation);
    }

    #[test]
    fn encodes_defaults_in_every_response_shape() {
        for version in 0..=4 {
            let response = DescribeConfigsResponse {
                throttle_time_ms: 17,
                resources: vec![DescribeConfigsResponseResource {
                    error_code: 0,
                    error_message: None,
                    resource_type: 2,
                    resource_name: "orders".to_owned(),
                    configs: vec![ConfigEntry {
                        name: "retention.ms".to_owned(),
                        value: Some("604800000".to_owned()),
                        config_source: 5,
                        read_only: true,
                        is_sensitive: false,
                    }],
                }],
            };
            let mut bytes = Vec::new();
            super::encode_response(&mut bytes, version, &response);
            assert!(!bytes.is_empty(), "v{version}");
        }
    }

    #[test]
    fn responses_decode_under_the_protocol_oracle() {
        use kafka_protocol::messages::DescribeConfigsResponse as KpResponse;
        use kafka_protocol::protocol::Decodable;

        for version in 1..=4 {
            let response = DescribeConfigsResponse {
                throttle_time_ms: 17,
                resources: vec![DescribeConfigsResponseResource {
                    error_code: 0,
                    error_message: None,
                    resource_type: 2,
                    resource_name: "orders".to_owned(),
                    configs: vec![ConfigEntry {
                        name: "retention.ms".to_owned(),
                        value: Some("604800000".to_owned()),
                        config_source: 5,
                        read_only: true,
                        is_sensitive: false,
                    }],
                }],
            };
            let mut bytes = Vec::new();
            super::encode_response(&mut bytes, version, &response);
            let mut rest = &bytes[..];
            let decoded = KpResponse::decode(&mut rest, version).expect("oracle decodes ours");
            assert!(rest.is_empty(), "v{version}: whole body consumed");
            assert_eq!(decoded.throttle_time_ms, 17, "v{version}");
            assert_eq!(decoded.results.len(), 1, "v{version}");
            assert_eq!(
                decoded.results[0].resource_name.as_str(),
                "orders",
                "v{version}"
            );
            assert_eq!(decoded.results[0].configs.len(), 1, "v{version}");
            assert_eq!(
                decoded.results[0].configs[0].name.as_str(),
                "retention.ms",
                "v{version}"
            );
        }
    }

    #[test]
    fn malformed_request_with_trailing_bytes_is_rejected() {
        let mut body = Vec::new();
        put_array_len(&mut body, false, Some(0));
        body.push(0);
        body.push(0);
        body.push(7);
        assert!(super::decode_request(&body, 3).is_err());
    }
}
