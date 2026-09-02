//! The authorization decision point (FR-40, `M9.7`).

use crate::Principal;

/// Whether an API beyond the pre-authentication trio (`ApiVersions`,
/// `SaslHandshake`, `SaslAuthenticate`) may run on this connection.
///
/// `M9.2`'s own note named this the open question its `Principal` type left
/// behind: "the rule that every request carries one" belongs to whichever
/// seam sits between authentication and every handler, and this function is
/// that seam — the one place `M9.7`'s own backlog row asks for, so a new API
/// handler added to `oqueue-broker`'s dispatch match cannot skip it by
/// forgetting to call it, only by being routed outside the single call site
/// that already does.
///
/// ⚠️ **This is the seam, not the policy.** It does not yet scope by topic —
/// `M9.8`'s forward index and `M9.9`-`M9.12`'s per-API wiring own that. What
/// it decides today is narrower and already real: whether *a* principal is
/// attached to this connection at all, on a broker where one is required to
/// be.
///
/// ⚠️ **Fails open when no credential source is configured.** `SASL/PLAIN`
/// (`M9.4`) cannot succeed at all without configured credentials — requiring
/// an identity no connection could ever present would not be a security
/// property, it would be a deadlock every existing deployment already falls
/// into today (`M9.4`'s and `M9.5`'s own shipped default: nothing configured,
/// every real connection unauthenticated). Real Kafka's own authorizer
/// carries the same instinct: absent an authorizer, every request is allowed.
///
/// ⚠️ **Fails closed once one is.** A connection that has not completed
/// `SaslAuthenticate` (`principal` is `None`) is refused every other API the
/// moment a credential source exists to authenticate against — an
/// unauthenticated connection is never let through as anonymous once
/// authentication is a live requirement on this broker.
#[must_use]
pub const fn authorize(principal: Option<&Principal>, credentials_configured: bool) -> bool {
    !credentials_configured || principal.is_some()
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::authorize;
    use crate::Principal;

    #[test]
    fn unconfigured_allows_an_unauthenticated_connection() {
        assert!(authorize(None, false));
    }

    #[test]
    fn unconfigured_allows_an_authenticated_connection_too() {
        let alice = Principal::new("alice").expect("valid");
        assert!(authorize(Some(&alice), false));
    }

    #[test]
    fn configured_refuses_an_unauthenticated_connection() {
        assert!(!authorize(None, true));
    }

    #[test]
    fn configured_allows_an_authenticated_connection() {
        let alice = Principal::new("alice").expect("valid");
        assert!(authorize(Some(&alice), true));
    }
}
