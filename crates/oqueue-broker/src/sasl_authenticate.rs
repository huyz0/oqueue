//! `SaslAuthenticate` (36): the exchange itself — `M9.3`.
//!
//! ⚠️ **Refuses every attempt, and that is the correct v1 answer, not a
//! stub.** No credential source is wired yet — `M9.4` is what adds one — and
//! a broker with nothing configured to check against must fail closed, the
//! same instinct `security.md` rule 3 states generally: an authentication
//! path with no real answer must never invent a success. `M9.4` changes
//! this from "always refuses" to "a correctly-presented `SASL/PLAIN`
//! credential succeeds"; both states are real, tested behaviour, not one
//! finished feature and one placeholder.

#![allow(clippy::redundant_pub_crate)]

use crate::connection::HandlerResponse;
use oqueue_codec::apikey::ApiKey;
use oqueue_codec::error_codes;
use oqueue_codec::frame::{RequestPrelude, encode_response_header};
use oqueue_codec::sasl_authenticate::{SaslAuthenticateResponse, decode_request, encode_response};

/// Decodes and refuses — or closes on a malformed body.
pub(crate) fn handle(prelude: RequestPrelude, body: &[u8]) -> HandlerResponse {
    let version = prelude.api_version;
    let Ok(_request) = decode_request(body, version) else {
        return HandlerResponse::Close;
    };

    // ⚠️ `_request.auth_bytes` is read (decoded, bounds-checked) but not
    // yet interpreted — `M9.4`'s own scope is parsing the SASL/PLAIN triple
    // out of it. Decoding without acting on it is not wasted: it is what
    // proves a malformed exchange still closes cleanly rather than the
    // refusal below masking a parse failure as an ordinary auth failure.
    let response = SaslAuthenticateResponse {
        error_code: error_codes::SASL_AUTHENTICATION_FAILED,
        error_message: Some("no SASL/PLAIN credential is configured"),
        auth_bytes: b"",
        session_lifetime_ms: 0,
    };

    let mut out = Vec::new();
    if encode_response_header(
        &mut out,
        ApiKey::SaslAuthenticate,
        version,
        prelude.correlation_id,
    )
    .is_err()
    {
        return HandlerResponse::Close;
    }
    encode_response(&mut out, version, &response);
    HandlerResponse::Reply(out)
}

#[cfg(test)]
mod tests;
