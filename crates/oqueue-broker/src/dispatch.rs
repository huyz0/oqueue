//! The dispatcher: one decoded frame in, one routed response out — or the
//! decision to close.
//!
//! ⚠️ **`ApiVersions` is answered before anything else exists** (doc 02
//! §7.6): it is the first request every client sends, and its two special
//! behaviours both live here — the response header stays v0 at every body
//! version (the frame layer's golden-pinned case), and a request at a
//! version this broker does not advertise gets the **v0-bodied fallback**:
//! an `ApiVersionsResponse` encoded at v0, `error_code` 35
//! (`UNSUPPORTED_VERSION`), with the `api_keys` table still populated so
//! the client can pick a version and retry. That fallback is `ApiVersions`'
//! alone: any other unanswerable request — an unknown key, an unadvertised
//! version — has no response the client would parse, and the only safe
//! answer is closing the connection: [`HandlerResponse::Close`] from the
//! [`crate::Handler`] seam (ADR-0018's status notes).
//!
//! ⚠️ **`M9.7`'s authorization decision point answers the same way.** An API
//! beyond the pre-authentication trio (`ApiVersions`, `SaslHandshake`,
//! `SaslAuthenticate`) that `oqueue_core::authorize` refuses closes the
//! connection rather than answering with a per-API authorization error code —
//! a deliberate simplification, not an oversight: five heterogeneous response
//! shapes (`Metadata`, `Produce`, `Fetch`, `ListOffsets`, `InitProducerId`)
//! would each need their own encoded refusal, and this decision point's own
//! scope is the seam, not full Kafka error-code parity for a branch no
//! existing deployment reaches yet (`credentials` is empty everywhere until
//! `bin/oqueue`'s own composition-root wiring lands). A later task may trade
//! this for a friendlier per-API answer; recorded rather than silently
//! chosen.

use crate::connection::HandlerResponse;
use core::future::Future;
use oqueue_codec::apikey::ApiKey;
use oqueue_codec::error_codes;
use oqueue_codec::frame::{RequestPrelude, encode_response_header, read_request_prelude};
use oqueue_codec::versions::supports;

/// Routes decoded frames to API handlers over the cluster.
///
/// `M2.21` built the frame walk and `ApiVersions`; `M2.22` added `Metadata`;
/// `M2.23`/`M2.24` plugged produce and fetch in beside them; `M3.14` pointed
/// those two at the real coordinator, index and object store.
#[derive(Debug)]
pub struct Dispatcher {
    cluster: std::sync::Arc<crate::cluster::Cluster>,
    session: crate::session::Session,
    /// Whether this connection is TLS-terminated — `ADR-0032`'s prerequisite
    /// for `SASL/PLAIN` to ever succeed. `false` by construction
    /// ([`Dispatcher::new`]): the honest default until a composer that
    /// actually knows says otherwise ([`Dispatcher::tls_terminated`]).
    tls: bool,
    /// The `SASL/PLAIN` credentials this connection may authenticate
    /// against (`ADR-0032`, `M9.4`) — empty by construction, matching
    /// `M9.3`'s own shipped "no credential source configured" state.
    credentials: std::sync::Arc<crate::sasl_authenticate::PlainCredentials>,
}

impl Dispatcher {
    /// A dispatcher over `cluster`, and **one session**.
    ///
    /// ⚠️ **Build one per connection.** The session is what carries a
    /// producer's own commit watermark into its next fetch (hazard H2,
    /// `ADR-0023`), and a session is per connection because that is the scope
    /// a client understands: two connections are two clients as far as
    /// read-your-writes is concerned, and sharing one would let an unrelated
    /// client's produce raise the freshness bar for everybody. Sharing the
    /// `Cluster` is the point; sharing the `Dispatcher` is not.
    ///
    /// ⚠️ **Not TLS-terminated, no credentials, by default.** Every one of
    /// this workspace's existing call sites gets exactly `M9.3`'s own
    /// shipped behaviour unchanged: `SASL/PLAIN` refuses every attempt.
    /// [`Dispatcher::tls_terminated`]/[`Dispatcher::with_credentials`] are
    /// additive builder steps a composer that actually knows opts into,
    /// never a silent default this constructor could get wrong.
    #[must_use]
    pub fn new(cluster: std::sync::Arc<crate::cluster::Cluster>) -> Self {
        Self {
            cluster,
            session: crate::session::Session::default(),
            tls: false,
            credentials: std::sync::Arc::default(),
        }
    }

    /// Marks this dispatcher's connection as TLS-terminated.
    ///
    /// ⚠️ **The composer's claim, not this crate's to verify.** Whichever
    /// listener accepted the connection (`crate::tls`'s own capability,
    /// `M9.5`) is the one place that can honestly answer this — `M9.5`'s own
    /// backlog row named wiring a real TLS listener into `bin/oqueue serve`
    /// as separate, not-yet-scoped work; this method is where that answer
    /// will land once it exists.
    #[must_use]
    pub const fn tls_terminated(mut self) -> Self {
        self.tls = true;
        self
    }

    /// The `SASL/PLAIN` credentials this dispatcher's connection may
    /// authenticate against (`ADR-0032`).
    ///
    /// ⚠️ **"From configuration," never invented here.** Reading that
    /// configuration (a file, an environment variable) is `bin/oqueue`'s
    /// job, the composition root — this crate only carries whatever result
    /// it is handed.
    #[must_use]
    pub fn with_credentials(
        mut self,
        credentials: crate::sasl_authenticate::PlainCredentials,
    ) -> Self {
        self.credentials = std::sync::Arc::new(credentials);
        self
    }
}

impl crate::Handler for Dispatcher {
    fn handle(&self, request: Vec<u8>) -> impl Future<Output = HandlerResponse> + Send {
        self.dispatch(request)
    }
}

impl Dispatcher {
    /// The body. ⚠️ **Async because produce and fetch now do I/O**: a produce
    /// PUTs an object and waits for its commit, and a fetch reads objects the
    /// index named. `M2`'s version of this was synchronous with a `ready`
    /// future, and the seam was async for exactly this milestone.
    ///
    /// ⚠️ **It takes the frame by value**, because the future it returns
    /// outlives this call and the connection task owns the buffer.
    pub(crate) async fn dispatch(&self, frame: Vec<u8>) -> HandlerResponse {
        let request: &[u8] = &frame;
        let Ok(prelude) = read_request_prelude(request) else {
            return HandlerResponse::Close;
        };
        let Some(api_key) = ApiKey::from_i16(prelude.api_key) else {
            // An unknown key has no parsable response; close (module doc).
            return HandlerResponse::Close;
        };
        if api_key != ApiKey::ApiVersions && !supports(api_key, prelude.api_version) {
            // Outside ApiVersions' fallback, an unadvertised version has no
            // response the client would parse.
            return HandlerResponse::Close;
        }
        if api_key == ApiKey::ApiVersions {
            return HandlerResponse::Reply(api_versions_response(prelude));
        }
        // `M9.7`'s authorization decision point: one call, ahead of the
        // routing match below, that every API beyond the pre-authentication
        // trio (`ApiVersions`, already answered above; `SaslHandshake` and
        // `SaslAuthenticate`, exempted here since they are how a principal
        // gets attached in the first place) passes through before its
        // handler runs. A new arm added to the match cannot skip this by
        // forgetting to call it — only by being routed outside this one
        // shared guard, which `M9.11`'s structural gate is free to check for
        // once there is more than one call site to compare against.
        if !matches!(api_key, ApiKey::SaslHandshake | ApiKey::SaslAuthenticate)
            && !oqueue_core::authorize(
                self.session.principal().as_ref(),
                !self.credentials.is_empty(),
            )
        {
            return HandlerResponse::Close;
        }
        // The message body starts after the full request header, whose
        // per-API shape `oqueue_codec::frame` now owns (`ADR-0019`).
        let Some(body) = oqueue_codec::frame::decode_request_header(request)
            .ok()
            .map(|(_, consumed)| &request[consumed..])
        else {
            return HandlerResponse::Close;
        };
        match api_key {
            ApiKey::ListOffsets => crate::listoffsets::handle(&self.cluster, prelude, body),
            ApiKey::Metadata => crate::metadata::handle(&self.cluster, prelude, body),
            ApiKey::Produce => {
                crate::produce::handle(&self.cluster, &self.session, prelude, body).await
            }
            ApiKey::Fetch => {
                crate::fetch::handle(&self.cluster, &self.session, prelude, body).await
            }
            ApiKey::InitProducerId => crate::init_producer_id::handle(prelude, body),
            ApiKey::SaslHandshake => crate::sasl_handshake::handle(prelude, body),
            ApiKey::SaslAuthenticate => crate::sasl_authenticate::handle(
                prelude,
                body,
                self.tls,
                &self.credentials,
                &self.session,
            ),
            // Answered above by the early return; named rather than a
            // wildcard so an eighth API cannot be silently swallowed here.
            ApiKey::ApiVersions => HandlerResponse::Close,
        }
    }
}

/// The `ApiVersions` answer at any request version, fallback included.
fn api_versions_response(prelude: RequestPrelude) -> Vec<u8> {
    let supported = supports(ApiKey::ApiVersions, prelude.api_version);
    // ⚠️ The fallback's whole point: an unsupported version is answered at
    // v0 — parsable by every client ever shipped — with the error code set
    // and the table still present, so the client retries at a version this
    // broker named (doc 02 §1.4).
    let (body_version, error_code) = if supported {
        (prelude.api_version, error_codes::NONE)
    } else {
        (0, error_codes::UNSUPPORTED_VERSION)
    };

    let mut out = Vec::new();
    if encode_response_header(
        &mut out,
        ApiKey::ApiVersions,
        body_version,
        prelude.correlation_id,
    )
    .is_err()
    {
        // Unreachable for a correlation-id header; an empty body would at
        // least carry the frame. Kept non-panicking per error-handling.md.
        out.clear();
    }
    oqueue_codec::apiversions::encode_response(&mut out, body_version, error_code);
    out
}

#[cfg(test)]
mod tests;
