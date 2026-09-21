//! `DescribeGroups` v0-v5, hand-rolled (`ADR-0019`).

use crate::flex::{
    TaggedFields, put_array_len, put_bytes, put_nullable_string, put_string, put_tagged_fields,
    read_array_len, read_string, read_tagged_fields,
};
use crate::wire::{Cursor, DecodeError, put_i16, put_i32};

/// A `DescribeGroups` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescribeGroupsRequest {
    /// Group ids to describe.
    pub groups: Vec<String>,
    /// Whether to include the authorized-operations bitfield.
    pub include_authorized_operations: bool,
}

/// Decodes a v0-v5 request body.
///
/// # Errors
///
/// Returns [`DecodeError`] when a field is malformed, truncated, or trailing
/// bytes remain after the versioned request shape.
pub fn decode_request(body: &[u8], version: i16) -> Result<DescribeGroupsRequest, DecodeError> {
    let flexible = version >= 5;
    let mut cur = Cursor::new(body);
    let count = read_array_len(&mut cur, flexible)?
        .ok_or_else(|| DecodeError::UnexpectedNull { at: cur.position() })?;
    let mut groups = Vec::with_capacity(count);
    for _ in 0..count {
        groups.push(read_string(&mut cur, flexible)?.to_owned());
    }
    let include_authorized_operations = if version >= 3 {
        cur.read_bool()?
    } else {
        false
    };
    if flexible {
        read_tagged_fields(&mut cur)?;
    }
    if cur.remaining() != 0 {
        return Err(DecodeError::LengthOutOfBounds {
            length: cur.remaining() as u64,
            max: 0,
            at: cur.position(),
        });
    }
    Ok(DescribeGroupsRequest {
        groups,
        include_authorized_operations,
    })
}

/// One member in a group description.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescribeGroupMember {
    /// Coordinator-assigned member id.
    pub member_id: String,
    /// Static membership id, when one exists.
    pub group_instance_id: Option<String>,
    /// Client id. This broker does not persist it, so handlers leave it empty.
    pub client_id: String,
    /// Client host. This broker does not persist it, so handlers leave it empty.
    pub client_host: String,
    /// Opaque subscription metadata. Deliberately empty in admin responses.
    pub member_metadata: Vec<u8>,
    /// Assignment bytes. Deliberately empty in admin responses.
    pub member_assignment: Vec<u8>,
}

/// One group in a `DescribeGroups` response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescribeGroup {
    /// Per-group error.
    pub error_code: i16,
    /// Group id.
    pub group_id: String,
    /// Kafka group-state name.
    pub group_state: String,
    /// Group protocol type.
    pub protocol_type: String,
    /// Negotiated protocol name.
    pub protocol_data: String,
    /// Members visible to the authorized caller.
    pub members: Vec<DescribeGroupMember>,
    /// Authorized operations, or zero when not requested.
    pub authorized_operations: i32,
}

/// A `DescribeGroups` response body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DescribeGroupsResponse {
    /// Throttle duration.
    pub throttle_time_ms: i32,
    /// Descriptions in request order.
    pub groups: Vec<DescribeGroup>,
}

/// Encodes a v0-v5 response body.
pub fn encode_response(out: &mut Vec<u8>, version: i16, response: &DescribeGroupsResponse) {
    let flexible = version >= 5;
    if version >= 1 {
        put_i32(out, response.throttle_time_ms);
    }
    put_array_len(out, flexible, Some(response.groups.len()));
    let empty = TaggedFields::default();
    for group in &response.groups {
        put_i16(out, group.error_code);
        put_string(out, flexible, &group.group_id);
        put_string(out, flexible, &group.group_state);
        put_string(out, flexible, &group.protocol_type);
        put_string(out, flexible, &group.protocol_data);
        put_array_len(out, flexible, Some(group.members.len()));
        for member in &group.members {
            put_string(out, flexible, &member.member_id);
            if version >= 4 {
                put_nullable_string(out, flexible, member.group_instance_id.as_deref());
            }
            put_string(out, flexible, &member.client_id);
            put_string(out, flexible, &member.client_host);
            put_bytes(out, flexible, &member.member_metadata);
            put_bytes(out, flexible, &member.member_assignment);
            if flexible {
                put_tagged_fields(out, &empty);
            }
        }
        if version >= 3 {
            put_i32(out, group.authorized_operations);
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

    use super::{DescribeGroup, DescribeGroupsResponse, decode_request, encode_response};
    use crate::flex::{TaggedFields, put_array_len, put_string, put_tagged_fields};
    use crate::wire::put_bool;

    #[test]
    fn decodes_requests_before_and_after_flexible_cutover() {
        for version in 0..=5 {
            let flexible = version >= 5;
            let mut body = Vec::new();
            put_array_len(&mut body, flexible, Some(1));
            put_string(&mut body, flexible, "orders");
            if version >= 3 {
                put_bool(&mut body, true);
            }
            if flexible {
                put_tagged_fields(&mut body, &TaggedFields::default());
            }
            let request = decode_request(&body, version).expect("request");
            assert_eq!(request.groups, vec!["orders"]);
            assert_eq!(request.include_authorized_operations, version >= 3);
        }
    }

    #[test]
    fn responses_have_the_expected_shapes_at_every_version() {
        for version in 0..=5 {
            let response = DescribeGroupsResponse {
                throttle_time_ms: 7,
                groups: vec![DescribeGroup {
                    error_code: 0,
                    group_id: "orders".to_owned(),
                    group_state: "Stable".to_owned(),
                    protocol_type: "consumer".to_owned(),
                    protocol_data: "range".to_owned(),
                    members: Vec::new(),
                    authorized_operations: 0,
                }],
            };
            let mut bytes = Vec::new();
            encode_response(&mut bytes, version, &response);
            assert!(!bytes.is_empty(), "v{version}");
        }
    }

    #[test]
    fn response_is_decodable_by_the_protocol_oracle() {
        use kafka_protocol::messages::DescribeGroupsResponse as KpResponse;
        use kafka_protocol::protocol::Decodable;

        for version in 0..=5 {
            let response = DescribeGroupsResponse {
                throttle_time_ms: 7,
                groups: vec![DescribeGroup {
                    error_code: 0,
                    group_id: "orders".to_owned(),
                    group_state: "Stable".to_owned(),
                    protocol_type: "consumer".to_owned(),
                    protocol_data: "range".to_owned(),
                    members: Vec::new(),
                    authorized_operations: 0,
                }],
            };
            let mut bytes = Vec::new();
            encode_response(&mut bytes, version, &response);
            let mut rest = &bytes[..];
            let decoded = KpResponse::decode(&mut rest, version).expect("oracle decodes");
            assert!(rest.is_empty(), "v{version}: whole body consumed");
            assert_eq!(decoded.groups.len(), 1, "v{version}");
            let group_id: &str = decoded.groups[0].group_id.0.as_ref();
            assert_eq!(group_id, "orders", "v{version}");
        }
    }

    #[test]
    fn only_versions_one_and_later_start_with_throttle_time() {
        let response = DescribeGroupsResponse {
            throttle_time_ms: 7,
            groups: Vec::new(),
        };
        let mut v0 = Vec::new();
        encode_response(&mut v0, 0, &response);
        let mut v1 = Vec::new();
        encode_response(&mut v1, 1, &response);
        assert_eq!(v0, [0, 0, 0, 0]);
        assert_eq!(v1, [0, 0, 0, 7, 0, 0, 0, 0]);
    }

    #[test]
    fn group_instance_id_is_present_only_from_version_four() {
        use kafka_protocol::messages::DescribeGroupsResponse as KpResponse;
        use kafka_protocol::protocol::Decodable;

        let response = DescribeGroupsResponse {
            throttle_time_ms: 0,
            groups: vec![DescribeGroup {
                error_code: 0,
                group_id: "orders".to_owned(),
                group_state: "Stable".to_owned(),
                protocol_type: "consumer".to_owned(),
                protocol_data: "range".to_owned(),
                members: vec![super::DescribeGroupMember {
                    member_id: "member".to_owned(),
                    group_instance_id: Some("instance".to_owned()),
                    client_id: "client".to_owned(),
                    client_host: "host".to_owned(),
                    member_metadata: Vec::new(),
                    member_assignment: Vec::new(),
                }],
                authorized_operations: 0,
            }],
        };

        let mut v3 = Vec::new();
        encode_response(&mut v3, 3, &response);
        let mut v3_rest = &v3[..];
        let v3_decoded = KpResponse::decode(&mut v3_rest, 3).expect("v3 response");
        assert!(v3_rest.is_empty());
        let v3_member_id: &str = v3_decoded.groups[0].members[0].member_id.as_ref();
        assert_eq!(v3_member_id, "member");
        assert!(v3_decoded.groups[0].members[0].group_instance_id.is_none());

        let mut v4 = Vec::new();
        encode_response(&mut v4, 4, &response);
        let mut v4_rest = &v4[..];
        let v4_decoded = KpResponse::decode(&mut v4_rest, 4).expect("v4 response");
        assert!(v4_rest.is_empty());
        let v4_instance = v4_decoded.groups[0].members[0]
            .group_instance_id
            .as_ref()
            .map(|id| {
                let id: &str = id.as_ref();
                id
            });
        assert_eq!(v4_instance, Some("instance"));
    }
}
