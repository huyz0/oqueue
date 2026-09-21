//! `ListGroups` v0-v4, hand-rolled (`ADR-0019`).

use crate::flex::{
    TaggedFields, put_array_len, put_string, put_tagged_fields, read_array_len, read_string,
    read_tagged_fields,
};
use crate::wire::{Cursor, DecodeError, put_i16, put_i32};

/// A `ListGroups` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListGroupsRequest {
    /// Optional state names to include. `None` means no filter.
    pub states_filter: Option<Vec<String>>,
}

/// Decodes a v0-v4 request body.
///
/// # Errors
///
/// Returns [`DecodeError`] when a field is malformed, truncated, or trailing
/// bytes remain after the versioned request shape.
pub fn decode_request(body: &[u8], version: i16) -> Result<ListGroupsRequest, DecodeError> {
    let flexible = version >= 3;
    let mut cur = Cursor::new(body);
    let states_filter = if version >= 4 {
        let count = read_array_len(&mut cur, true)?;
        count
            .map(|count| {
                (0..count)
                    .map(|_| read_string(&mut cur, true).map(str::to_owned))
                    .collect()
            })
            .transpose()?
    } else {
        None
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
    Ok(ListGroupsRequest { states_filter })
}

/// One group in a `ListGroups` response.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListedGroup {
    /// Group id.
    pub group_id: String,
    /// Group protocol type.
    pub protocol_type: String,
    /// Group-state name, present from v4.
    pub group_state: String,
}

/// A `ListGroups` response body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ListGroupsResponse {
    /// Throttle duration.
    pub throttle_time_ms: i32,
    /// Top-level error.
    pub error_code: i16,
    /// Visible groups.
    pub groups: Vec<ListedGroup>,
}

/// Encodes a v0-v4 response body.
pub fn encode_response(out: &mut Vec<u8>, version: i16, response: &ListGroupsResponse) {
    let flexible = version >= 3;
    if version >= 1 {
        put_i32(out, response.throttle_time_ms);
    }
    put_i16(out, response.error_code);
    put_array_len(out, flexible, Some(response.groups.len()));
    let empty = TaggedFields::default();
    for group in &response.groups {
        put_string(out, flexible, &group.group_id);
        put_string(out, flexible, &group.protocol_type);
        if version >= 4 {
            put_string(out, true, &group.group_state);
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

    use super::{ListGroupsResponse, ListedGroup, decode_request, encode_response};
    use crate::flex::{TaggedFields, put_array_len, put_string, put_tagged_fields};

    #[test]
    fn decodes_empty_and_state_filtered_requests() {
        for version in 0..=4 {
            let flexible = version >= 3;
            let mut body = Vec::new();
            if version >= 4 {
                put_array_len(&mut body, true, Some(1));
                put_string(&mut body, true, "Stable");
            }
            if flexible {
                put_tagged_fields(&mut body, &TaggedFields::default());
            }
            let request = decode_request(&body, version).expect("request");
            assert_eq!(
                request.states_filter,
                (version >= 4).then(|| vec!["Stable".into()])
            );
        }
    }

    #[test]
    fn response_is_decodable_by_the_protocol_oracle() {
        use kafka_protocol::messages::ListGroupsResponse as KpResponse;
        use kafka_protocol::protocol::Decodable;

        for version in 0..=4 {
            let response = ListGroupsResponse {
                throttle_time_ms: 7,
                error_code: 0,
                groups: vec![ListedGroup {
                    group_id: "orders".to_owned(),
                    protocol_type: "consumer".to_owned(),
                    group_state: "Stable".to_owned(),
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
}
