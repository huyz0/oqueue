//! `FindCoordinator` v0-3, single-key, hand-rolled (`ADR-0019`).
//!
//! ⚠️ **Not the batched `coordinator_keys` shape** — the dependency's own
//! schema adds that at v4 (KIP-699); `M4.4` is where this module gains a
//! second decode/encode pair for it. This module's own `key`/`key_type`
//! fields are exactly the v0-3 request's supported-version ranges: `key` at
//! 0-3, `key_type` at 1-3 (absent, and read as `0` — `GROUP` — at v0).
//!
//! ⚠️ **`key_type` is decoded here, never interpreted** — this module is
//! the wire layer only; whether `key_type == 1` (`TRANSACTION`) gets a real
//! answer or a refusal is `crates/oqueue-broker/src/find_coordinator.rs`'s
//! own decision, matching `InitProducerId`'s own precedent of refusing a
//! transactional call rather than answering as if this broker understood
//! one (FR-15 deferred, doc 02 §4.3, KIP-98, out of scope).

use crate::flex::{
    TaggedFields, put_nullable_string, put_string, put_tagged_fields, read_string,
    read_tagged_fields,
};
use crate::wire::{Cursor, DecodeError, put_i16, put_i32};

/// Which coordinator a client asked to find.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FindCoordinatorRequest<'a> {
    /// The group (or, at the wire level, transaction) id to resolve.
    pub key: &'a str,
    /// `0` (`GROUP`) or `1` (`TRANSACTION`) from v1; always `0` below it.
    pub key_type: i8,
}

/// Decodes a `FindCoordinator` request.
///
/// # Errors
/// [`DecodeError`] on any malformed length, truncation, or a null `key` —
/// the schema declares it non-nullable.
pub fn decode_request(
    body: &[u8],
    version: i16,
) -> Result<FindCoordinatorRequest<'_>, DecodeError> {
    let flexible = version >= 3;
    let mut cur = Cursor::new(body);
    let key = read_string(&mut cur, flexible)?;
    let key_type = if version >= 1 { cur.read_i8()? } else { 0 };
    if flexible {
        read_tagged_fields(&mut cur)?;
    }
    Ok(FindCoordinatorRequest { key, key_type })
}

/// The whole answer: which node coordinates `key`, or why not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FindCoordinatorResponse<'a> {
    /// `0` on success, or the reason resolution failed.
    pub error_code: i16,
    /// A human-readable reason, only on failure — absent below v1.
    pub error_message: Option<&'a str>,
    /// The coordinator's node id.
    pub node_id: i32,
    /// The coordinator's advertised host.
    pub host: &'a str,
    /// The coordinator's advertised port.
    pub port: i32,
}

/// Encodes a `FindCoordinator` response.
pub fn encode_response(out: &mut Vec<u8>, version: i16, resp: &FindCoordinatorResponse<'_>) {
    let flexible = version >= 3;
    if version >= 1 {
        // throttle_time_ms: this broker has no quota mechanism to charge
        // against yet (M9.16's own quota is per-principal in-flight, not a
        // throttle duration a client would wait out) -- zero, honestly,
        // rather than a number nothing computes.
        put_i32(out, 0);
    }
    put_i16(out, resp.error_code);
    if version >= 1 {
        put_nullable_string(out, flexible, resp.error_message);
    }
    put_i32(out, resp.node_id);
    put_string(out, flexible, resp.host);
    put_i32(out, resp.port);
    if flexible {
        put_tagged_fields(out, &TaggedFields::default());
    }
}

#[cfg(test)]
mod tests;
