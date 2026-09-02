//! `SaslAuthenticate` (36): the exchange itself — `M9.3`, `SASL/PLAIN`
//! (`M9.4`, `ADR-0032`).
//!
//! ⚠️ **No credential, however correct, succeeds without TLS.** `ADR-0032`'s
//! own load-bearing property: `PLAIN`'s one missing guarantee (credential
//! confidentiality in transit) is supplied by the TLS layer beneath it, so a
//! connection this broker cannot confirm is TLS-terminated is refused before
//! the credential is even checked — the caller passes `tls: bool`, and there
//! is no path from `false` to a success.
//!
//! ⚠️ **Credentials are configured, never invented.** With none configured
//! (`M9.3`'s own shipped state, still real once `M9.4` lands), every attempt
//! is refused the same way it always was — an unconfigured broker fails
//! closed, `security.md` rule 3's instinct applied here.
//!
//! ⚠️ **Ordinary string equality, not constant-time.** [`Redacted`]'s own
//! documentation names this exact gap: comparing two wrapped secrets leaks
//! timing, and `M8` owns closing it before anything actually compares them.
//! This is that anything, and the gap is inherited deliberately rather than
//! solved out of scope here.

#![allow(clippy::redundant_pub_crate)]

use crate::connection::HandlerResponse;
use oqueue_codec::apikey::ApiKey;
use oqueue_codec::error_codes;
use oqueue_codec::frame::{RequestPrelude, encode_response_header};
use oqueue_codec::sasl_authenticate::{SaslAuthenticateResponse, decode_request, encode_response};
use oqueue_core::{Principal, Redacted};

/// One principal's `SASL/PLAIN` credential — `ADR-0032`'s flat model, no
/// salt, no per-mechanism variant.
#[derive(Debug, Clone)]
pub struct PlainCredential {
    /// The principal this credential authenticates.
    pub principal: Principal,
    /// The password checked against a client's own `authcid`/`passwd`.
    pub password: Redacted<String>,
}

/// The `SASL/PLAIN` credentials this broker's mechanism checks against —
/// "from configuration" (`ADR-0032`); `bin/oqueue`'s job to read that
/// configuration and hand the result here, never this crate's own.
#[derive(Debug, Clone, Default)]
pub struct PlainCredentials(Vec<PlainCredential>);

impl PlainCredentials {
    /// Builds a credential set from its members.
    #[must_use]
    pub const fn new(credentials: Vec<PlainCredential>) -> Self {
        Self(credentials)
    }

    /// Whether this broker has any credential to check against at all —
    /// `M9.7`'s own question, distinct from whether any one exchange
    /// succeeds: an empty set is `M9.3`'s and `M9.4`'s own shipped default,
    /// and the one case `oqueue_core::authorize` must fail open for rather
    /// than lock every connection out of an API no credential could ever be
    /// presented to unlock.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// The principal `authcid`/`password` authenticates as, or `None` for
    /// no match — an unknown principal and a wrong password are the same
    /// outcome here, deliberately: distinguishing them would let a client
    /// enumerate valid names.
    fn verify(&self, authcid: &str, password: &str) -> Option<&Principal> {
        self.0
            .iter()
            .find(|c| c.principal.as_str() == authcid && c.password.expose() == password)
            .map(|c| &c.principal)
    }
}

/// Parses `SASL/PLAIN`'s RFC 4616 wire format — `[authzid] NUL authcid NUL
/// passwd` — into `(authcid, passwd)`. `authzid` is decoded but not
/// returned: this broker authenticates the connection as `authcid`, the
/// identity that actually presented a password, not an unauthenticated
/// authorization-identity override.
///
/// `None` for anything that is not exactly three NUL-separated fields, or
/// whose `authcid`/`passwd` fields are not valid UTF-8.
fn parse_plain(auth_bytes: &[u8]) -> Option<(&str, &str)> {
    let mut parts = auth_bytes.split(|&b| b == 0);
    let _authzid = parts.next()?;
    let authcid = parts.next()?;
    let passwd = parts.next()?;
    if parts.next().is_some() {
        return None; // more than three fields
    }
    Some((
        std::str::from_utf8(authcid).ok()?,
        std::str::from_utf8(passwd).ok()?,
    ))
}

/// One error message for every refusal this mechanism can produce — no
/// SASL exchange, wrong TLS state, malformed bytes, or a real credential
/// mismatch all answer identically, so nothing here becomes an oracle for
/// which failure a client hit.
const REFUSED: &str = "SASL/PLAIN authentication failed";

const fn refused() -> SaslAuthenticateResponse<'static> {
    SaslAuthenticateResponse {
        error_code: error_codes::SASL_AUTHENTICATION_FAILED,
        error_message: Some(REFUSED),
        auth_bytes: Redacted::new(b""),
        session_lifetime_ms: 0,
    }
}

/// Decodes, authenticates (or refuses), and encodes — or closes on a
/// malformed body.
///
/// `tls` names whether this connection is already TLS-terminated —
/// `ADR-0032`'s prerequisite, decided by whichever composer accepted the
/// connection (a TLS listener, once that wiring exists), never guessed at
/// here. `credentials` is the configured set to check against. On success,
/// `session` records the matched principal (`Session::authenticate`) —
/// `M9.7`'s "the rule that every request carries one" made real, since this
/// is the one place that principal is ever established.
pub(crate) fn handle(
    prelude: RequestPrelude,
    body: &[u8],
    tls: bool,
    credentials: &PlainCredentials,
    session: &crate::session::Session,
) -> HandlerResponse {
    let version = prelude.api_version;
    let Ok(request) = decode_request(body, version) else {
        return HandlerResponse::Close;
    };

    let matched = if tls {
        parse_plain(request.auth_bytes.expose())
            .and_then(|(authcid, password)| credentials.verify(authcid, password).cloned())
    } else {
        None
    };
    // ⚠️ **`session.authenticate` decides "first" atomically.** Verifying the
    // credential above has no side effect, so two `SaslAuthenticate` frames
    // pipelined on the same connection (`connection.rs`'s per-frame
    // `tokio::spawn`, `bin/oqueue`'s deeply pipelined default) can both reach
    // this point having both matched a real credential — `Session::authenticate`
    // itself is the one place that decides which principal, if either, actually
    // wins, under a single lock acquisition rather than a separate check and
    // write here that a race could slip between. A matched credential that
    // loses that race is refused exactly like one that never matched at all —
    // `ADR-0032`'s "no re-authentication story yet" held under concurrency,
    // not just in sequence.
    let response = matched
        .filter(|principal| session.authenticate(principal.clone()))
        .map_or_else(refused, |_principal| SaslAuthenticateResponse {
            error_code: error_codes::NONE,
            error_message: None,
            auth_bytes: Redacted::new(b""),
            // ⚠️ No expiry: this broker does not yet re-authenticate a
            // long-lived connection (a real feature, not an omission to
            // fix here) — Kafka's own convention for "no bound" is the
            // field's own max representable value, not `0`, which real
            // Kafka clients treat as "expires immediately."
            session_lifetime_ms: i64::MAX,
        });

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
