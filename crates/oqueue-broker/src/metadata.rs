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
//! ⚠️ **`M9.9`/`M9.10`: two distinct shapes, not the same rule twice**
//! (`M9.1`'s verified finding against real Kafka source). An
//! *explicitly-named* topic the principal cannot `DESCRIBE` answers
//! `TOPIC_AUTHORIZATION_FAILED` for that entry (`M9.9`) — the client already
//! named it, so existence is not what is being protected. The
//! *null-topic-array* case ("every topic") instead **silently omits**
//! whatever the principal cannot see (`M9.10`): it is served directly from
//! [`oqueue_core::TopicGrants::topics_for`] rather than [`Cluster::topic_names`], so an
//! unauthorized topic is never enumerated in the first place, not answered
//! and then hidden. Scoping is a no-op for both shapes — every name
//! resolves exactly as before `M9` — whenever `credentials_configured` is
//! `false`, `oqueue_core::authorize`'s own fail-open signal reused rather
//! than inventing a second one: no credential source configured means no
//! connection could ever be denied by `M9.7`'s gate either, so there is
//! nothing this handler could honestly refuse.

use crate::authz::{AuthzContext, topic_authorized};
use crate::cluster::Cluster;
use oqueue_codec::apikey::ApiKey;
use oqueue_codec::error_codes;
use oqueue_codec::frame::{RequestPrelude, encode_response_header};
use oqueue_codec::metadata::{
    MetadataResponse, MetadataResponseTopic, decode_request, encode_response,
};
use oqueue_core::TopicId;

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
        Some(all_topics_names(cluster, authz))
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
    /// Whether `names` came from the null-topic-array case — `topic_authorized`
    /// does not run per-entry for it, because `all_topics_names` (`M9.10`)
    /// already scoped the list at its source; running it again per name
    /// would be redundant, not incorrect.
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

/// The topic names a null-topic-array ("every topic") request answers with
/// — `M9.10`'s own gate, the silent-omission half of `M9.1`'s verified
/// Kafka finding.
///
/// ⚠️ **Served from [`oqueue_core::TopicGrants::topics_for`] directly, never from
/// [`Cluster::topic_names`], once configured.** This is doc 15 §4's own
/// architectural claim made real for this path specifically: the cost is
/// O(topics this principal can see), not O(topics that exist) filtered
/// down afterward — the anti-pattern `M9.8`'s whole index exists to avoid,
/// which computing this list by enumerating every topic and checking each
/// one against the index would silently reintroduce.
///
/// ⚠️ **Fails open when unconfigured**, `topic_authorized`'s own signal.
/// **No principal is an empty list**, not a panic — the same defensive
/// shape as `topic_authorized`, for a case that should be equally
/// unreachable past `M9.7`'s own gate.
fn all_topics_names(cluster: &Cluster, authz: &AuthzContext<'_>) -> Vec<String> {
    if !authz.credentials_configured {
        return cluster.topic_names();
    }
    let Some(principal) = authz.principal else {
        return Vec::new();
    };
    // ⚠️ **Existence filtered here, not left to `resolve_topic`.** A grant
    // naming a topic that does not (yet) exist must not appear at all — real
    // Kafka's own null-array path never invents or reports on a topic
    // outside its existing metadata cache, only an *explicitly-named* one
    // gets `allow_auto_topic_creation`'s treatment (`M9.9`'s own case, a
    // request the client actually made). Without this, `resolve_all`'s
    // `mode.all_topics` short-circuit would hand a phantom name straight to
    // `resolve_topic`, which would either silently create it or answer
    // `UNKNOWN_TOPIC_OR_PARTITION` for a topic nobody named — round 1
    // review's own finding. `partition_count` is one lookup per granted
    // topic (O(this principal's own grant count)), never a scan of
    // `Cluster`'s own topics — the cost claim this whole path exists for
    // stays intact.
    authz
        .topic_grants
        .topics_for(principal)
        .map(TopicId::as_str)
        .filter(|name| cluster.partition_count(name).is_some())
        .map(str::to_owned)
        .collect()
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
