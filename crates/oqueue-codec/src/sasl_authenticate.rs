//! `SaslAuthenticate` v0-2 request decode and response encode, hand-rolled
//! (`ADR-0019`).
//!
//! ⚠️ **The mechanism exchange, not the mechanism itself.** This module
//! carries `auth_bytes` as opaque bytes — whatever the negotiated
//! mechanism's own encoding is — the same way `flex.rs`'s primitives are
//! agnostic to what a string or a byte string *means*. `M9.4` is where the
//! `SASL/PLAIN` triple (`authzid\0authcid\0password`, RFC 4616) is actually
//! parsed out of `auth_bytes`; this module's job ends at the wire.

use crate::flex::{
    TaggedFields, put_bytes, put_nullable_string, put_tagged_fields, read_bytes, read_tagged_fields,
};
use crate::wire::{Cursor, DecodeError, put_i16, put_i64};

/// The bytes a client sent for the mechanism `SaslHandshake` already
/// negotiated.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaslAuthenticateRequest<'a> {
    /// The mechanism-defined exchange bytes.
    pub auth_bytes: &'a [u8],
}

/// Decodes a `SaslAuthenticate` request.
///
/// # Errors
/// [`DecodeError`] on any malformed length, truncation, or a null
/// `auth_bytes` — the schema declares it non-nullable.
pub fn decode_request(
    body: &[u8],
    version: i16,
) -> Result<SaslAuthenticateRequest<'_>, DecodeError> {
    let flexible = version >= 2;
    let mut cur = Cursor::new(body);
    let auth_bytes = read_bytes(&mut cur, flexible)?;
    if flexible {
        read_tagged_fields(&mut cur)?;
    }
    Ok(SaslAuthenticateRequest { auth_bytes })
}

/// The whole answer: success with the mechanism's own reply bytes, or a
/// refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaslAuthenticateResponse<'a> {
    /// `0` on success, or why the exchange failed.
    pub error_code: i16,
    /// A human-readable reason, only on failure.
    pub error_message: Option<&'a str>,
    /// The mechanism's own reply bytes — empty on failure.
    pub auth_bytes: &'a [u8],
    /// How long the resulting session is valid for, in milliseconds — `0`
    /// when there is no session (a failure, or a mechanism this broker
    /// does not track a lifetime for).
    pub session_lifetime_ms: i64,
}

/// Encodes a `SaslAuthenticate` response.
pub fn encode_response(out: &mut Vec<u8>, version: i16, resp: &SaslAuthenticateResponse<'_>) {
    let flexible = version >= 2;
    put_i16(out, resp.error_code);
    put_nullable_string(out, flexible, resp.error_message);
    put_bytes(out, flexible, resp.auth_bytes);
    if version >= 1 {
        put_i64(out, resp.session_lifetime_ms);
    }
    if flexible {
        put_tagged_fields(out, &TaggedFields::default());
    }
}

#[cfg(test)]
mod tests;
