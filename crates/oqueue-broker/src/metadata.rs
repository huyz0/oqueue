//! The `Metadata` answer, v0-v13, against the stub cluster.
//!
//! ⚠️ **`allow_auto_topic_creation` exists from v4** (`M2.md` task 16): at
//! v4+ a requested topic that does not exist is created only when the flag
//! says so, else answered `UNKNOWN_TOPIC_OR_PARTITION` for that topic
//! alone. Below v4 the field does not exist on the wire and the protocol's
//! historical behaviour — creation allowed, the broker's config deciding —
//! is what our decoder's default (`true`) expresses below v4
//! (`oqueue_codec::metadata`).
//!
//! ⚠️ **`M9.9`: an explicitly-named topic is scoped, the null-topic-array
//! case is not yet** (`M9.10`'s own row). `M9.1`'s verified finding against
//! real Kafka source split these into two distinct shapes on purpose — an
//! *explicitly-named* topic the principal cannot `DESCRIBE` answers
//! `TOPIC_AUTHORIZATION_FAILED` for that entry, while "every topic" (the
//! null array) still calls [`Cluster::topic_names`] unscoped until `M9.10`
//! points it at the same index instead. Scoping is a no-op — every name
//! resolves exactly as before `M9` — whenever `credentials_configured` is
//! `false`, `oqueue_core::authorize`'s own fail-open signal reused rather
//! than inventing a second one: no credential source configured means no
//! connection could ever be denied by `M9.7`'s gate either, so there is
//! nothing this handler could honestly refuse.

use crate::cluster::Cluster;
use oqueue_codec::apikey::ApiKey;
use oqueue_codec::error_codes;
use oqueue_codec::frame::{RequestPrelude, encode_response_header};
use oqueue_codec::metadata::{
    MetadataResponse, MetadataResponseTopic, decode_request, encode_response,
};
use oqueue_core::{Principal, TopicGrants, TopicId};

/// `M9.9`'s authorization inputs, bundled: `Metadata`'s own signature would
/// otherwise carry three parameters that only ever travel together (`M9.7`'s
/// principal/credentials-configured pair, plus `M9.8`'s index), past
/// `rust-style.md`'s argument-count limit.
pub(crate) struct AuthzContext<'a> {
    /// This connection's authenticated identity, if any (`M9.7`'s
    /// `Session::principal`).
    pub(crate) principal: Option<&'a Principal>,
    /// Whether authorization is even live on this broker — `M9.7`'s own
    /// fail-open signal, reused rather than a second one invented here.
    pub(crate) credentials_configured: bool,
    /// `M9.8`'s forward index.
    pub(crate) topic_grants: &'a TopicGrants,
}

/// Decodes, answers, encodes. `Close` only when the body cannot be
/// decoded — a malformed request from a client that negotiated fine is a
/// closed connection, same policy as the dispatcher's other unanswerables.
pub(crate) fn handle(
    cluster: &Cluster,
    prelude: RequestPrelude,
    body: &[u8],
    authz: &AuthzContext<'_>,
) -> crate::connection::HandlerResponse {
    answer(cluster, prelude, body, authz).map_or(
        crate::connection::HandlerResponse::Close,
        crate::connection::HandlerResponse::Reply,
    )
}

/// One topic resolved to what the response needs, owned so the borrowed
/// [`MetadataResponseTopic`] can point into it.
struct ResolvedTopic {
    name: String,
    topic_id: [u8; 16],
    error_code: i16,
    partition_count: usize,
}

/// The `Option` body `handle` wraps: `None` is the close decision.
fn answer(
    cluster: &Cluster,
    prelude: RequestPrelude,
    body: &[u8],
    authz: &AuthzContext<'_>,
) -> Option<Vec<u8>> {
    let version = prelude.api_version;
    let request = decode_request(body, version).ok()?;

    // Null asks for every topic this broker has — and so does v0's empty
    // array, which is that version's only all-topics spelling (empty means
    // none only from v1; round 1's review caught the inversion).
    let all_topics = request.topics.is_none()
        || (version == 0 && request.topics.as_ref().is_some_and(Vec::is_empty));
    let names: Option<Vec<String>> = if all_topics {
        // ⚠️ **Not yet scoped — `M9.10`'s own case.** "Every topic" still
        // means every topic that exists, not yet every topic this
        // principal can see; the module doc names this a known, temporary
        // gap rather than an oversight.
        Some(cluster.topic_names())
    } else {
        // ⚠️ A null NAME inside an entry (v10+ describe-by-topic-id) is a
        // request this id-less stub cannot serve — refusing beats creating
        // a topic literally named "" and polluting every later response.
        request.topics.as_ref().map_or(Some(Vec::new()), |topics| {
            topics
                .iter()
                .map(|t| t.name.clone())
                .collect::<Option<Vec<String>>>()
        })
    };
    // Same policy as every other unanswerable shape: close.
    let names = names?;

    let mode = ResolveMode {
        version,
        all_topics,
        allow_auto_topic_creation: request.allow_auto_topic_creation,
    };
    let resolved = resolve_all(cluster, names, &mode, authz);

    let response = MetadataResponse {
        node_id: cluster.node_id,
        host: &cluster.host,
        port: cluster.port,
        cluster_id: Some("oqueue"),
        controller_id: cluster.node_id,
        topics: resolved
            .iter()
            .map(|t| MetadataResponseTopic {
                error_code: t.error_code,
                name: Some(&t.name),
                topic_id: t.topic_id,
                partition_count: t.partition_count,
            })
            .collect(),
    };

    let mut out = Vec::new();
    encode_response_header(&mut out, ApiKey::Metadata, version, prelude.correlation_id).ok()?;
    encode_response(&mut out, version, &response);
    Some(out)
}

/// The per-response scalars `resolve_all` needs beyond `names` itself,
/// bundled for the same argument-count reason as [`AuthzContext`].
struct ResolveMode {
    version: i16,
    /// Whether `names` came from the null-topic-array case — `M9.9`'s own
    /// authorization gate does not run for it yet (`M9.10`'s row).
    all_topics: bool,
    allow_auto_topic_creation: bool,
}

/// Resolves every requested name — authorized first for the
/// explicitly-named case, `resolve_topic`'s own answer otherwise.
fn resolve_all(
    cluster: &Cluster,
    names: Vec<String>,
    mode: &ResolveMode,
    authz: &AuthzContext<'_>,
) -> Vec<ResolvedTopic> {
    names
        .into_iter()
        .map(|name| {
            if mode.all_topics || topic_authorized(&name, authz) {
                resolve_topic(cluster, name, mode.version, mode.allow_auto_topic_creation)
            } else {
                ResolvedTopic {
                    name,
                    topic_id: [0u8; 16],
                    error_code: error_codes::TOPIC_AUTHORIZATION_FAILED,
                    partition_count: 0,
                }
            }
        })
        .collect()
}

/// Whether an explicitly-named topic may be resolved at all, before
/// `resolve_topic` ever runs — `M9.9`'s own gate, the explicitly-named half
/// of `M9.1`'s verified Kafka finding.
///
/// ⚠️ **Fails open, reusing `oqueue_core::authorize`'s own signal.** With no
/// credential source configured, `M9.7`'s dispatcher gate could never have
/// refused this connection either — scoping `Metadata` here without
/// scoping `SaslAuthenticate` would be inconsistent, not more careful.
/// ⚠️ **Fails closed on everything else**, deliberately conservative: no
/// principal (should be unreachable — `M9.7`'s own gate already refuses an
/// unauthenticated connection once credentials are configured) and an
/// unconstructible topic name (an empty string; `TopicId`'s own invariant)
/// both answer "no," never a panic or an unwrap.
fn topic_authorized(name: &str, authz: &AuthzContext<'_>) -> bool {
    if !authz.credentials_configured {
        return true;
    }
    let Some(principal) = authz.principal else {
        return false;
    };
    let Ok(topic_id) = TopicId::new(name) else {
        return false;
    };
    authz.topic_grants.can_see(principal, &topic_id)
}

/// Resolves one topic: existing topics report their partitions; a missing
/// one is created or refused by the flag. Topic ids ride the wire from v10.
fn resolve_topic(
    cluster: &Cluster,
    name: String,
    version: i16,
    allow_auto_topic_creation: bool,
) -> ResolvedTopic {
    let exists = cluster.partition_count(&name).is_some();
    if !exists && !allow_auto_topic_creation {
        return ResolvedTopic {
            name,
            topic_id: [0u8; 16],
            error_code: error_codes::UNKNOWN_TOPIC_OR_PARTITION,
            partition_count: 0,
        };
    }
    if !exists {
        cluster.ensure_topic(&name);
    }
    let partition_count = cluster.partition_count(&name).unwrap_or(1);
    let topic_id = if version >= 10 {
        cluster
            .topic_id(&name)
            .map_or([0u8; 16], uuid::Uuid::into_bytes)
    } else {
        [0u8; 16]
    };
    ResolvedTopic {
        name,
        topic_id,
        error_code: error_codes::NONE,
        partition_count,
    }
}

#[cfg(test)]
mod tests;
