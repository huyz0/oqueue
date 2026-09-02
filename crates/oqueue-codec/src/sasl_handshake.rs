//! `SaslHandshake` v0-1 request decode and response encode, hand-rolled
//! (`ADR-0019`).
//!
//! ⚠️ **Never flexible, at any version** — `apikey.rs`'s own doc names this
//! message "the protocol's standing example": both `SaslHandshakeRequest`
//! and `SaslHandshakeResponse` report header version 1/0 unconditionally,
//! independent of the body version, since a client cannot yet know what a
//! flexible encoding would even mean before this exchange tells it which
//! mechanism it is using.

use crate::flex::{put_array_len, put_string, read_string};
use crate::wire::{Cursor, DecodeError, put_i16};

/// The mechanism a client asked to use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaslHandshakeRequest<'a> {
    /// The SASL mechanism name, e.g. `"PLAIN"`.
    pub mechanism: &'a str,
}

/// Decodes a `SaslHandshake` request.
///
/// # Errors
/// [`DecodeError`] on any malformed length, truncation, or a null
/// `mechanism` — the schema declares it non-nullable.
pub fn decode_request(body: &[u8]) -> Result<SaslHandshakeRequest<'_>, DecodeError> {
    // Never flexible at any version (this module's own doc), so `flexible`
    // is a literal here rather than threaded in from the caller.
    let mut cur = Cursor::new(body);
    let mechanism = read_string(&mut cur, false)?;
    Ok(SaslHandshakeRequest { mechanism })
}

/// The whole answer: which mechanisms this broker enables, and whether the
/// one the client asked for is among them.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SaslHandshakeResponse<'a> {
    /// `0` if `mechanism` is supported, or the error naming why not.
    pub error_code: i16,
    /// Every mechanism this broker enables — sent regardless of
    /// `error_code`, so a client refused for one can retry with another
    /// without a second round trip.
    pub mechanisms: &'a [&'a str],
}

/// Encodes a `SaslHandshake` response.
pub fn encode_response(out: &mut Vec<u8>, resp: &SaslHandshakeResponse<'_>) {
    put_i16(out, resp.error_code);
    put_array_len(out, false, Some(resp.mechanisms.len()));
    for mechanism in resp.mechanisms {
        put_string(out, false, mechanism);
    }
}

#[cfg(test)]
mod tests;
