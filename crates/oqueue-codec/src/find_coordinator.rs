//! `FindCoordinator` v0-6, hand-rolled (`ADR-0019`).
//!
//! ⚠️ **One function, two frame shapes** — `M4.4`'s own acceptance
//! criterion, `M2`'s "one handler, version-gated encode/decode" precedent
//! applied here. Below v4 the wire carries exactly one key, flattened into
//! the top-level request/response fields; from v4 (KIP-699) it carries a
//! batch, nested under `coordinator_keys`/`coordinators`. Both shapes
//! normalize to the same in-memory type — [`FindCoordinatorRequest::keys`]
//! is always a `Vec`, of length exactly one below v4 — so a caller
//! (`crates/oqueue-broker/src/find_coordinator.rs`) writes one resolution
//! loop rather than two.
//!
//! ⚠️ **`key_type` is decoded here, never interpreted** — this module is
//! the wire layer only; whether `key_type == 1` (`TRANSACTION`) gets a real
//! answer or a refusal is `crates/oqueue-broker/src/find_coordinator.rs`'s
//! own decision, matching `InitProducerId`'s own precedent of refusing a
//! transactional call rather than answering as if this broker understood
//! one (FR-15 deferred, doc 02 §4.3, KIP-98, out of scope).
//!
//! ⚠️ **Per-key failure is not per-request failure** — `M4.4`'s own
//! acceptance criterion. Each [`Coordinator`] in a batched response carries
//! its own `error_code`/`error_message`, so one malformed or refused key
//! never fails the keys beside it.

use crate::flex::{
    TaggedFields, put_array_len, put_nullable_string, put_string, put_tagged_fields,
    read_array_len, read_string, read_tagged_fields,
};
use crate::wire::{Cursor, DecodeError, put_i16, put_i32};

/// Which coordinator(s) a client asked to find.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindCoordinatorRequest<'a> {
    /// Below v4: exactly one key, from the singular wire field. From v4:
    /// one or more, from `coordinator_keys` (KIP-699).
    pub keys: Vec<&'a str>,
    /// `0` (`GROUP`) or `1` (`TRANSACTION`) from v1; always `0` below it.
    pub key_type: i8,
}

/// Decodes a `FindCoordinator` request.
///
/// # Errors
/// [`DecodeError`] on any malformed length, truncation, or a null key —
/// the schema declares every key non-nullable.
pub fn decode_request(
    body: &[u8],
    version: i16,
) -> Result<FindCoordinatorRequest<'_>, DecodeError> {
    let flexible = version >= 3;
    let mut cur = Cursor::new(body);
    // Wire order: the singular `key` (<=3), then `key_type` (>=1), then
    // the batched `coordinator_keys` (>=4) -- the dependency's own encode
    // order, not alphabetical or declaration order.
    let single_key = if version <= 3 {
        Some(read_string(&mut cur, flexible)?)
    } else {
        None
    };
    let key_type = if version >= 1 { cur.read_i8()? } else { 0 };
    let keys = if version >= 4 {
        let count = read_array_len(&mut cur, flexible)?.unwrap_or(0);
        let mut keys = Vec::with_capacity(count);
        for _ in 0..count {
            keys.push(read_string(&mut cur, flexible)?);
        }
        keys
    } else {
        // `single_key` is always `Some` here (version <= 3 implies version < 4).
        single_key.into_iter().collect()
    };
    if flexible {
        read_tagged_fields(&mut cur)?;
    }
    Ok(FindCoordinatorRequest { keys, key_type })
}

/// One key's own answer: which node coordinates it, or why not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Coordinator<'a> {
    /// The key this entry resolves — echoed back only in the batched (v4+)
    /// shape; below v4 there is exactly one entry and no per-entry key
    /// field on the wire.
    pub key: &'a str,
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

/// The whole answer: one [`Coordinator`] per requested key, in the same
/// order.
///
/// ⚠️ **Meant to carry exactly one entry below v4** — a caller answering a
/// single-key request should supply exactly the one entry the wire shape
/// below v4 has room for. [`encode_response`] does not assert this: zero
/// entries falls back to the protocol's own "no coordinator" sentinel
/// rather than panicking (a caller mistake, not untrusted wire bytes, but
/// `security.md` rule 3's instinct against panicking still applies), and
/// more than one silently uses only the first — a caller bug either way,
/// caught by review rather than by a debug assertion, since asserting it
/// would make that fallback itself untestable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindCoordinatorResponse<'a> {
    /// One entry per requested key, in request order.
    pub coordinators: Vec<Coordinator<'a>>,
}

/// Encodes a `FindCoordinator` response.
///
/// Below v4 this flattens `coordinators[0]` into the top-level fields real
/// Kafka's own single-key response shape uses; from v4 it encodes the full
/// batch under `coordinators`, KIP-699's own nested shape.
pub fn encode_response(out: &mut Vec<u8>, version: i16, resp: &FindCoordinatorResponse<'_>) {
    let flexible = version >= 3;
    if version >= 1 {
        // throttle_time_ms: this broker has no quota mechanism to charge
        // against yet (M9.16's own quota is per-principal in-flight, not a
        // throttle duration a client would wait out) -- zero, honestly,
        // rather than a number nothing computes.
        put_i32(out, 0);
    }
    if version <= 3 {
        // A single-key response should carry exactly one answer; a caller
        // that supplies zero (a bug, since the broker handler always
        // builds exactly one for a single-key request) still gets a
        // well-formed refusal rather than a panic -- `security.md` rule 3's
        // instinct applied to a caller mistake rather than untrusted bytes.
        let c = resp.coordinators.first().copied().unwrap_or(Coordinator {
            key: "",
            error_code: crate::error_codes::UNKNOWN_SERVER_ERROR,
            error_message: None,
            node_id: -1,
            host: "",
            port: -1,
        });
        put_i16(out, c.error_code);
        if version >= 1 {
            put_nullable_string(out, flexible, c.error_message);
        }
        put_i32(out, c.node_id);
        put_string(out, flexible, c.host);
        put_i32(out, c.port);
    } else {
        put_array_len(out, flexible, Some(resp.coordinators.len()));
        for c in &resp.coordinators {
            put_string(out, flexible, c.key);
            put_i32(out, c.node_id);
            put_string(out, flexible, c.host);
            put_i32(out, c.port);
            put_i16(out, c.error_code);
            put_nullable_string(out, flexible, c.error_message);
            put_tagged_fields(out, &TaggedFields::default());
        }
    }
    if flexible {
        put_tagged_fields(out, &TaggedFields::default());
    }
}

#[cfg(test)]
mod tests;
