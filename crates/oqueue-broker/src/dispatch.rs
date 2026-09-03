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
//! a deliberate simplification, not an oversight: eleven heterogeneous
//! response shapes (`Metadata`, `Produce`, `Fetch`, `ListOffsets`,
//! `InitProducerId`, `FindCoordinator`, `JoinGroup`, `SyncGroup`,
//! `Heartbeat`, `LeaveGroup`, `OffsetCommit`) would each need their own
//! encoded refusal,
//! and this decision point's own scope is the seam, not full Kafka error-code
//! parity for a branch no
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

/// The result of [`Dispatcher::admit_quota`] — not `Option<Option<InFlight>>`
/// (clippy's `option_option`, `-D pedantic`): a refusal and "admitted,
/// nothing to release" are genuinely different outcomes, not two spellings
/// of `None`.
enum QuotaAdmission {
    /// The request is over its principal's bound; close the connection.
    Refused,
    /// The request may proceed, holding `InFlight` for the rest of it if a
    /// quota is actually configured for an authenticated principal.
    Admitted(Option<oqueue_core::InFlight>),
}

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
    /// Which topics an authenticated principal may see in `Metadata`
    /// (`M9.8`, `M9.9`) — empty by construction, matching `credentials`'
    /// own default: nothing configured means nothing to consult, and
    /// `Metadata`'s own fail-open rule (`credentials` empty) means this
    /// field is not even read until a credential source exists.
    topic_grants: std::sync::Arc<oqueue_core::TopicGrants>,
    /// The in-flight-request bound each authenticated principal shares
    /// across every connection it opens (`security.md` rule 13, FR-45,
    /// `M9.16`) — `None` by construction, matching `credentials`' own
    /// "nothing configured" default: no quota is enforced until a composer
    /// that actually knows opts one in.
    quota: Option<std::sync::Arc<oqueue_core::PrincipalQuota>>,
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
            topic_grants: std::sync::Arc::default(),
            quota: None,
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

    /// Which topics an authenticated principal may see in `Metadata`
    /// (`M9.8`, `M9.9`).
    ///
    /// ⚠️ **"From configuration," never invented here** — `with_credentials`'
    /// own rule applied to a second value: reading real grants (a file, an
    /// environment variable) is `bin/oqueue`'s job, the composition root;
    /// this crate only carries whatever result it is handed. v1 ships no
    /// `CreateTopics`/`DescribeAcls`/`CreateAcls`/`DeleteAcls` wire API, so
    /// there is no live path that calls `TopicGrants::grant`/`revoke` yet —
    /// a composer would build one once, at startup, from static
    /// configuration.
    #[must_use]
    pub fn with_topic_grants(mut self, topic_grants: oqueue_core::TopicGrants) -> Self {
        self.topic_grants = std::sync::Arc::new(topic_grants);
        self
    }

    /// The in-flight-request bound this connection's authenticated
    /// principal shares with every other connection it opens
    /// (`security.md` rule 13, FR-45, `M9.16`).
    ///
    /// ⚠️ **Takes the shared `Arc` itself, not a value to wrap.**
    /// `with_credentials`/`with_topic_grants` each take an owned value and
    /// wrap it in a fresh `Arc` — correct for read-only configuration, where
    /// a clone of the *data* behaves identically to sharing it. A quota's
    /// count has to be the *same* counter across every dispatcher for one
    /// broker, or each connection gets its own independent bound and a
    /// principal escapes it by opening more connections — the composer
    /// builds one `Arc<PrincipalQuota>` and clones the `Arc` into every
    /// dispatcher sharing the `Cluster`, `Cluster`'s own sharing shape
    /// applied to a second value.
    #[must_use]
    pub fn with_quota(mut self, quota: std::sync::Arc<oqueue_core::PrincipalQuota>) -> Self {
        self.quota = Some(quota);
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
        if self.unauthorized(api_key) {
            return HandlerResponse::Close;
        }
        // `security.md` rule 13, FR-45, `M9.16`: the second bound every API
        // beyond the pre-authentication trio passes through, right beside
        // `M9.7`'s own authorization check rather than folded into it —
        // "may this principal run at all" and "has this principal already
        // got too much running" are separate questions, and `authorize`'s
        // own doc says plainly it is the seam, not the policy. Held for the
        // rest of this call, including the `.await` below: `_in_flight`
        // releases when `dispatch` returns, not before.
        let _in_flight = match self.admit_quota(self.session.principal().as_ref()) {
            QuotaAdmission::Admitted(in_flight) => in_flight,
            QuotaAdmission::Refused => return HandlerResponse::Close,
        };
        // The message body starts after the full request header, whose
        // per-API shape `oqueue_codec::frame` now owns (`ADR-0019`).
        let Some(body) = oqueue_codec::frame::decode_request_header(request)
            .ok()
            .map(|(_, consumed)| &request[consumed..])
        else {
            return HandlerResponse::Close;
        };
        match api_key {
            ApiKey::ListOffsets => self.listoffsets_handle(prelude, body),
            ApiKey::Metadata => self.metadata_handle(prelude, body),
            ApiKey::OffsetCommit => self.offset_commit_handle(prelude, body),
            ApiKey::FindCoordinator => self.find_coordinator_handle(prelude, body),
            ApiKey::JoinGroup => crate::join_group::handle(&self.cluster, prelude, body).await,
            ApiKey::SyncGroup => crate::sync_group::handle(&self.cluster, prelude, body).await,
            ApiKey::Heartbeat => crate::heartbeat::handle(&self.cluster, prelude, body),
            ApiKey::LeaveGroup => crate::leave_group::handle(&self.cluster, prelude, body),
            ApiKey::Produce => self.produce_handle(prelude, body).await,
            ApiKey::Fetch => self.fetch_handle(prelude, body).await,
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

    /// `M9.7`'s authorization decision point: one call, ahead of `dispatch`'s
    /// own routing match, that every API beyond the pre-authentication trio
    /// (`ApiVersions`, answered before this is ever reached; `SaslHandshake`
    /// and `SaslAuthenticate`, exempted here since they are how a principal
    /// gets attached in the first place) passes through before its handler
    /// runs. A new arm added to that match cannot skip this by forgetting to
    /// call it — only by being routed outside this one shared guard, which
    /// `M9.11`'s structural gate is free to check for once there is more
    /// than one call site to compare against. Pulled into its own method
    /// purely to keep `dispatch` under the fifty-line limit.
    fn unauthorized(&self, api_key: ApiKey) -> bool {
        !matches!(api_key, ApiKey::SaslHandshake | ApiKey::SaslAuthenticate)
            && !oqueue_core::authorize(
                self.session.principal().as_ref(),
                !self.credentials.is_empty(),
            )
    }

    /// Admits one in-flight request against this connection's quota, if one
    /// is configured (`security.md` rule 13, FR-45, `M9.16`).
    fn admit_quota(&self, principal: Option<&oqueue_core::Principal>) -> QuotaAdmission {
        match (&self.quota, principal) {
            (Some(quota), Some(principal)) => oqueue_core::PrincipalQuota::admit(quota, principal)
                .map_or(QuotaAdmission::Refused, |in_flight| {
                    QuotaAdmission::Admitted(Some(in_flight))
                }),
            // No quota configured, or no principal yet: the same fail-open
            // shape `oqueue_core::authorize` already uses — nothing to bound
            // is not a refusal.
            _ => QuotaAdmission::Admitted(None),
        }
    }

    /// [`crate::authz::AuthzContext`] for this connection, right now —
    /// `M9.12`'s shared builder, `metadata_handle`'s own original pattern
    /// generalized to every handler that scopes by topic. `principal` is
    /// borrowed rather than read again, because `Session::principal` returns
    /// an owned value and every caller needs its own local to borrow from
    /// for the lifetime this context carries.
    fn authz_context<'a>(
        &'a self,
        principal: Option<&'a oqueue_core::Principal>,
    ) -> crate::authz::AuthzContext<'a> {
        crate::authz::AuthzContext {
            principal,
            credentials_configured: !self.credentials.is_empty(),
            topic_grants: &self.topic_grants,
        }
    }

    /// `Metadata`'s own arm, pulled out of `dispatch`'s `match` purely to
    /// keep that function under the fifty-line limit — building
    /// [`crate::authz::AuthzContext`] needs the session's current
    /// principal bound to a local first, which the match arm's own line
    /// budget could not absorb alongside every other API.
    fn metadata_handle(&self, prelude: RequestPrelude, body: &[u8]) -> HandlerResponse {
        let principal = self.session.principal();
        crate::metadata::handle(
            &self.cluster,
            prelude,
            body,
            &self.authz_context(principal.as_ref()),
        )
    }

    /// `FindCoordinator`'s own arm, pulled out of `dispatch`'s `match` for
    /// the same fifty-line-limit reason `metadata_handle` is.
    fn find_coordinator_handle(&self, prelude: RequestPrelude, body: &[u8]) -> HandlerResponse {
        crate::find_coordinator::handle(&self.cluster, prelude, body)
    }

    /// `ListOffsets`'s own arm — `M9.12`'s per-principal scoping, same
    /// pattern as `metadata_handle`.
    fn listoffsets_handle(&self, prelude: RequestPrelude, body: &[u8]) -> HandlerResponse {
        let principal = self.session.principal();
        crate::listoffsets::handle(
            &self.cluster,
            prelude,
            body,
            &self.authz_context(principal.as_ref()),
        )
    }

    /// `Produce`'s own arm — `M9.12`'s per-principal scoping, same pattern
    /// as `metadata_handle`.
    async fn produce_handle(&self, prelude: RequestPrelude, body: &[u8]) -> HandlerResponse {
        let principal = self.session.principal();
        crate::produce::handle(
            &self.cluster,
            &self.session,
            prelude,
            body,
            &self.authz_context(principal.as_ref()),
        )
        .await
    }

    /// `Fetch`'s own arm — `M9.12`'s per-principal scoping, same pattern as
    /// `metadata_handle`.
    async fn fetch_handle(&self, prelude: RequestPrelude, body: &[u8]) -> HandlerResponse {
        let principal = self.session.principal();
        crate::fetch::handle(
            &self.cluster,
            &self.session,
            prelude,
            body,
            &self.authz_context(principal.as_ref()),
        )
        .await
    }

    /// `OffsetCommit`'s own arm — `M4.12`'s own per-principal scoping,
    /// `M9.12`'s pattern reused for a group-protocol handler rather than
    /// invented again. Not `async`: `crate::offset_commit::handle`'s own
    /// precedent, nothing here parks.
    fn offset_commit_handle(&self, prelude: RequestPrelude, body: &[u8]) -> HandlerResponse {
        let principal = self.session.principal();
        crate::offset_commit::handle(
            &self.cluster,
            prelude,
            body,
            &self.authz_context(principal.as_ref()),
        )
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
