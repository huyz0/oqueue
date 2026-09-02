//! `SaslHandshake` (17): mechanism negotiation — `M9.3`.
//!
//! ⚠️ **`PLAIN`, and only `PLAIN`.** `ADR-0032` is the one v1 mechanism this
//! broker enables; a client naming anything else is refused with
//! `UNSUPPORTED_SASL_MECHANISM`, but the mechanism list still comes back
//! (real Kafka's own shape — `crates/oqueue-codec/src/sasl_handshake.rs`'s
//! own doc), so the client can retry with a mechanism this broker actually
//! serves rather than being told nothing.

#![allow(clippy::redundant_pub_crate)]

use crate::connection::HandlerResponse;
use oqueue_codec::apikey::ApiKey;
use oqueue_codec::error_codes;
use oqueue_codec::frame::{RequestPrelude, encode_response_header};
use oqueue_codec::sasl_handshake::{SaslHandshakeResponse, decode_request, encode_response};

/// The one mechanism this broker enables (`ADR-0032`).
const ENABLED_MECHANISMS: [&str; 1] = ["PLAIN"];

/// Decodes, checks the mechanism, and encodes — or closes on a malformed
/// body.
pub(crate) fn handle(prelude: RequestPrelude, body: &[u8]) -> HandlerResponse {
    let version = prelude.api_version;
    let Ok(request) = decode_request(body) else {
        return HandlerResponse::Close;
    };

    let error_code = if request.mechanism == "PLAIN" {
        error_codes::NONE
    } else {
        error_codes::UNSUPPORTED_SASL_MECHANISM
    };
    let response = SaslHandshakeResponse {
        error_code,
        mechanisms: &ENABLED_MECHANISMS,
    };

    let mut out = Vec::new();
    if encode_response_header(
        &mut out,
        ApiKey::SaslHandshake,
        version,
        prelude.correlation_id,
    )
    .is_err()
    {
        return HandlerResponse::Close;
    }
    encode_response(&mut out, &response);
    HandlerResponse::Reply(out)
}

#[cfg(test)]
mod tests;
