//! `M9.7`'s authorization inputs and `M9.9`'s per-topic gate, shared across
//! every handler that names a topic explicitly (`M9.9` built it for
//! `Metadata`; `M9.12` reuses it for `Produce`, `Fetch`, and `ListOffsets`
//! rather than each growing its own copy).

// ⚠️ `pub(crate)` inside a private module: `unreachable_pub` denies the `pub`
// clippy's `redundant_pub_crate` asks for. `pub(crate)` is the visibility
// that is actually true here — every handler crate-wide reaches these — so
// the lint that disagrees is the one allowed, `init_producer_id.rs`'s own
// precedent for the same standoff.
#![allow(clippy::redundant_pub_crate)]

use oqueue_core::{Principal, TopicGrants, TopicId};

/// `M9.7`'s authorization inputs, bundled: a handler's own signature would
/// otherwise carry three parameters that only ever travel together (the
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

/// Whether an explicitly-named topic may be resolved at all, before a
/// handler's own per-topic work runs — `M9.9`'s original gate for
/// `Metadata`, the explicitly-named half of `M9.1`'s verified Kafka finding;
/// `M9.12` calls this same function from `Produce`, `Fetch`, and
/// `ListOffsets`.
///
/// ⚠️ **Fails open, reusing `oqueue_core::authorize`'s own signal.** With no
/// credential source configured, `M9.7`'s dispatcher gate could never have
/// refused this connection either — scoping one handler without scoping
/// `SaslAuthenticate` would be inconsistent, not more careful.
/// ⚠️ **Fails closed on everything else**, deliberately conservative: no
/// principal (should be unreachable — `M9.7`'s own gate already refuses an
/// unauthenticated connection once credentials are configured) and an
/// unconstructible topic name (an empty string; `TopicId`'s own invariant)
/// both answer "no," never a panic or an unwrap.
pub(crate) fn topic_authorized(name: &str, authz: &AuthzContext<'_>) -> bool {
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
