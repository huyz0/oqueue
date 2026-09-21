//! The `ApiVersions` response, hand-rolled (`ADR-0019`).
//!
//! This is the first message this broker answers and the one whose framing
//! is most load-bearing: its *response header* is always v0 (handled in
//! [`crate::frame`]), and its body advertises the version table every client
//! reads before it speaks anything else. ⚠️ **We encode only the response.**
//! The request body carries just the client's software name and version
//! (v3, informational); this broker answers from the negotiated version
//! alone, so decoding it would be unearned — a malformed request body cannot
//! change the table we send.
//!
//! Byte layout, from the protocol (pinned to the dependency below):
//! `error_code` (i16); `api_keys` — a compact array from v3, a regular
//! `i32`-length array below, each entry `api_key`/`min`/`max` (i16) plus a
//! tagged-fields byte at v3; `throttle_time_ms` (i32) from v1; a top-level
//! tagged-fields section at v3.

use crate::flex::{TaggedFields, put_compact_array_len, put_tagged_fields};
use crate::versions::{ADVERTISED, Advertised};
use crate::wire::{put_i16, put_i32};

/// Appends an `ApiVersions` response body at `body_version` with
/// `error_code`, advertising [`ADVERTISED`]. The response header (always v0)
/// is [`crate::frame::encode_response_header`]'s job.
pub fn encode_response(out: &mut Vec<u8>, body_version: i16, error_code: i16) {
    encode_response_for(out, body_version, error_code, &ADVERTISED);
}

/// Appends an `ApiVersions` body using an explicitly filtered table.
pub fn encode_response_for(
    out: &mut Vec<u8>,
    body_version: i16,
    error_code: i16,
    advertised: &[Advertised],
) {
    put_i16(out, error_code);

    if body_version >= 3 {
        put_compact_array_len(out, Some(advertised.len()));
    } else {
        put_i32(out, i32::try_from(advertised.len()).unwrap_or(i32::MAX));
    }
    for row in advertised {
        put_i16(out, row.api_key.as_i16());
        put_i16(out, row.min);
        put_i16(out, row.max);
        if body_version >= 3 {
            put_tagged_fields(out, &TaggedFields::default());
        }
    }

    if body_version >= 1 {
        put_i32(out, 0); // throttle_time_ms
    }
    if body_version >= 3 {
        put_tagged_fields(out, &TaggedFields::default());
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::encode_response;
    use crate::versions::ADVERTISED;

    /// The `ADR-0019` oracle: our bytes decode under `kafka-protocol`'s
    /// generated `ApiVersionsResponse` and carry the whole advertised table,
    /// at every advertised body version and at the v0 fallback.
    #[test]
    fn our_response_decodes_under_the_dependency() {
        use kafka_protocol::messages::ApiVersionsResponse;
        use kafka_protocol::protocol::Decodable;

        for body_version in [0i16, 1, 2, 3] {
            let mut out = Vec::new();
            encode_response(&mut out, body_version, 0);
            let mut rest = &out[..];
            let decoded =
                ApiVersionsResponse::decode(&mut rest, body_version).expect("oracle decodes ours");
            assert!(rest.is_empty(), "v{body_version}: whole body consumed");
            assert_eq!(decoded.error_code, 0);
            assert_eq!(decoded.api_keys.len(), ADVERTISED.len());
            for (entry, row) in decoded.api_keys.iter().zip(&ADVERTISED) {
                assert_eq!(entry.api_key, row.api_key.as_i16());
                assert_eq!(entry.min_version, row.min);
                assert_eq!(entry.max_version, row.max);
            }
        }
    }

    /// Byte-exact against the dependency's own encoder: build the same
    /// response both ways and demand identical bytes.
    #[test]
    fn our_bytes_are_byte_identical_to_the_dependency() {
        use kafka_protocol::messages::ApiVersionsResponse;
        use kafka_protocol::messages::api_versions_response::ApiVersion;
        use kafka_protocol::protocol::Encodable;

        for body_version in [0i16, 1, 2, 3] {
            let mut theirs = ApiVersionsResponse::default();
            theirs.error_code = 35;
            theirs.throttle_time_ms = 0;
            theirs.api_keys = ADVERTISED
                .iter()
                .map(|row| {
                    let mut v = ApiVersion::default();
                    v.api_key = row.api_key.as_i16();
                    v.min_version = row.min;
                    v.max_version = row.max;
                    v
                })
                .collect();
            let mut theirs_bytes = Vec::new();
            theirs
                .encode(&mut theirs_bytes, body_version)
                .expect("dependency encodes");

            let mut ours = Vec::new();
            encode_response(&mut ours, body_version, 35);
            assert_eq!(ours, theirs_bytes, "v{body_version}: byte-identical");
        }
    }
}
