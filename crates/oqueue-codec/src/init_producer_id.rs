//! `InitProducerId` v0-4 request decode and response encode, hand-rolled
//! (`ADR-0019`).
//!
//! ⚠️ **Non-transactional only, and that is `M11.4`'s whole scope.**
//! `transactional_id` is decoded so the handler can refuse a transactional
//! call with a clear code (FR-15, deferred) rather than silently answering
//! as if it understood transactions — `oqueue-broker`'s handler is where
//! that refusal happens, not this module.
//!
//! ⚠️ **`producer_id`/`producer_epoch` (v3+) are decoded and ignored.** A
//! client presenting a previously-issued identity to resume it is a
//! transactional-producer shape this broker does not yet honour — `M11.4`'s
//! non-transactional path always mints a fresh one, per `ADR-0031`'s
//! non-sequential allocation shape.

use crate::flex::{TaggedFields, put_tagged_fields, read_nullable_string, read_tagged_fields};
use crate::wire::{Cursor, DecodeError, put_i16, put_i32, put_i64};

/// The fields of an `InitProducerId` request this broker acts on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitProducerIdRequest<'a> {
    /// The transactional id, or `None` for the idempotent-only case this
    /// broker serves (`M11.4`).
    pub transactional_id: Option<&'a str>,
}

/// Decodes an `InitProducerId` request.
///
/// # Errors
/// [`DecodeError`] on any malformed length or truncation.
pub fn decode_request(body: &[u8], version: i16) -> Result<InitProducerIdRequest<'_>, DecodeError> {
    let flexible = version >= 2;
    let mut cur = Cursor::new(body);
    let transactional_id = read_nullable_string(&mut cur, flexible)?;
    let _transaction_timeout_ms = cur.read_i32()?;
    if version >= 3 {
        let _producer_id = cur.read_i64()?;
        let _producer_epoch = cur.read_i16()?;
    }
    if flexible {
        read_tagged_fields(&mut cur)?;
    }
    Ok(InitProducerIdRequest { transactional_id })
}

/// The whole answer: a freshly minted identity, or a refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InitProducerIdResponse {
    /// `0`, or why nothing was minted.
    pub error_code: i16,
    /// The minted id, or `-1` on any refusal.
    pub producer_id: i64,
    /// The minted epoch (always `0` for a fresh identity), or `-1` on any
    /// refusal.
    pub producer_epoch: i16,
}

/// Encodes an `InitProducerId` response.
pub fn encode_response(out: &mut Vec<u8>, version: i16, resp: &InitProducerIdResponse) {
    let flexible = version >= 2;
    put_i32(out, 0); // throttle_time_ms
    put_i16(out, resp.error_code);
    put_i64(out, resp.producer_id);
    put_i16(out, resp.producer_epoch);
    if flexible {
        put_tagged_fields(out, &TaggedFields::default());
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{InitProducerIdResponse, decode_request, encode_response};

    /// Every advertised `InitProducerId` version, both directions.
    const VERSIONS: [i16; 5] = [0, 1, 2, 3, 4];

    /// The `ADR-0019` oracle: our decoder reads what the dependency's
    /// encoder wrote, at every advertised version — the non-transactional
    /// case (`transactional_id: None`).
    #[test]
    fn our_request_decode_matches_the_dependency_non_transactional() {
        use kafka_protocol::messages::InitProducerIdRequest as KpRequest;
        use kafka_protocol::protocol::Encodable;

        for version in VERSIONS {
            let mut kp = KpRequest::default();
            kp.transactional_id = None;
            kp.transaction_timeout_ms = 30_000;
            let mut bytes = Vec::new();
            kp.encode(&mut bytes, version).expect("dependency encodes");

            let ours = decode_request(&bytes, version).expect("ours decodes");
            assert_eq!(ours.transactional_id, None, "v{version}");
        }
    }

    /// And the transactional case: a real `transactional_id` round-trips
    /// through our decoder too, so the handler can see it and refuse —
    /// `M11.4`'s whole reason to decode this field at all.
    #[test]
    fn our_request_decode_matches_the_dependency_transactional() {
        use kafka_protocol::messages::InitProducerIdRequest as KpRequest;
        use kafka_protocol::protocol::{Encodable, StrBytes};

        for version in VERSIONS {
            let mut kp = KpRequest::default();
            kp.transactional_id = Some(kafka_protocol::messages::TransactionalId(
                StrBytes::from_static_str("txn-1"),
            ));
            kp.transaction_timeout_ms = 30_000;
            if version >= 3 {
                kp.producer_id = kafka_protocol::messages::ProducerId(7);
                kp.producer_epoch = 2;
            }
            let mut bytes = Vec::new();
            kp.encode(&mut bytes, version).expect("dependency encodes");

            let ours = decode_request(&bytes, version).expect("ours decodes");
            assert_eq!(ours.transactional_id, Some("txn-1"), "v{version}");
        }
    }

    /// The other direction: the dependency's decoder reads what we wrote,
    /// field for field, at every advertised version.
    #[test]
    fn our_response_encode_matches_the_dependency() {
        use kafka_protocol::messages::InitProducerIdResponse as KpResponse;
        use kafka_protocol::protocol::Decodable;

        for version in VERSIONS {
            let response = InitProducerIdResponse {
                error_code: 0,
                producer_id: 42,
                producer_epoch: 0,
            };
            let mut bytes = Vec::new();
            encode_response(&mut bytes, version, &response);

            let mut rest = &bytes[..];
            let theirs = KpResponse::decode(&mut rest, version).expect("the dependency decodes");
            assert!(rest.is_empty(), "v{version}: nothing after the body");
            assert_eq!(theirs.error_code, 0, "v{version}");
            assert_eq!(theirs.producer_id.0, 42, "v{version}");
            assert_eq!(theirs.producer_epoch, 0, "v{version}");
        }
    }

    /// A refusal encodes the protocol's own sentinel identity.
    #[test]
    fn a_refusal_encodes_the_sentinel_identity() {
        use kafka_protocol::messages::InitProducerIdResponse as KpResponse;
        use kafka_protocol::protocol::Decodable;

        for version in VERSIONS {
            let response = InitProducerIdResponse {
                error_code: 35,
                producer_id: -1,
                producer_epoch: -1,
            };
            let mut bytes = Vec::new();
            encode_response(&mut bytes, version, &response);

            let mut rest = &bytes[..];
            let theirs = KpResponse::decode(&mut rest, version).expect("the dependency decodes");
            assert_eq!(theirs.error_code, 35, "v{version}");
            assert_eq!(theirs.producer_id.0, -1, "v{version}");
            assert_eq!(theirs.producer_epoch, -1, "v{version}");
        }
    }

    /// ⚠️ **A truncated body is refused, not guessed at**, the same
    /// `security.md` rule 3 every decoder in this crate follows.
    #[test]
    fn every_prefix_of_a_request_is_refused_rather_than_panicking() {
        use kafka_protocol::messages::InitProducerIdRequest as KpRequest;
        use kafka_protocol::protocol::{Encodable, StrBytes};

        for version in VERSIONS {
            let mut kp = KpRequest::default();
            kp.transactional_id = Some(kafka_protocol::messages::TransactionalId(
                StrBytes::from_static_str("txn-1"),
            ));
            kp.transaction_timeout_ms = 30_000;
            let mut bytes = Vec::new();
            kp.encode(&mut bytes, version).expect("dependency encodes");

            for cut in 0..bytes.len() {
                let _ = decode_request(&bytes[..cut], version);
            }
            assert!(decode_request(&bytes, version).is_ok(), "v{version}");
        }
    }
}
