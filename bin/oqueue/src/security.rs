//! The authentication, authorization and quota configuration this broker
//! runs with — `M4.18`, discharging `M9.21`.
//!
//! ⚠️ **`M9` built every mechanism and wired none of them.** `tls::acceptor`,
//! `Dispatcher::tls_terminated`, `with_credentials`, `with_topic_grants` and
//! `with_quota` all existed and all had exactly zero real call sites: the
//! only thing constructing a `Dispatcher` was `serve`, and it called plain
//! `new`. So a `serve` built from `M9`'s own crate answered every request
//! unauthenticated, over cleartext, with no grant checked and no quota
//! applied — `M9`'s closing milestone review found that and
//! `roadmap.md`'s deferred table carried it here. This module is the
//! reading half; `serve` is the wiring half.
//!
//! ⚠️ **Every source is named by an environment variable and never
//! sniffed**, and a malformed one is a startup failure rather than a
//! silent default — `behavior.md` rule 8, and `chosen_store`'s own
//! argument one file over. The failure mode this rule exists for is
//! sharper here than it is for a store: a typo'd credential path that fell
//! back to "no credentials" would start a broker that accepts every
//! client, which is the opposite of what the operator asked for and looks
//! identical from the outside until somebody reads the logs.

// ⚠️ **The same standoff `serve.rs` records, for the same reason.** In a
// binary crate `unreachable_pub` denies `pub` on anything not exported, and
// `redundant_pub_crate` denies `pub(crate)` inside a private module — so a
// type this module must share with `serve` and `main` cannot satisfy both.
// `serve.rs` resolved it this way first; a second answer here would be a
// second convention for one question.
#![allow(clippy::redundant_pub_crate)]

use oqueue_broker::Dispatcher;
use oqueue_broker::sasl_authenticate::PlainCredentials;
use oqueue_core::{AdminGrants, GroupGrants, PrincipalQuota, TopicGrants};
use std::sync::Arc;
use tokio_rustls::TlsAcceptor;

/// Why a security source could not be used.
///
/// ⚠️ **Every variant is fatal at startup.** None of them has a sensible
/// degraded reading: "the credential file is malformed" cannot mean "run
/// without credentials", because the operator who set the variable asked
/// for the opposite.
#[derive(Debug, thiserror::Error)]
pub(crate) enum SecurityError {
    /// A named file could not be read.
    #[error("{variable} names {path:?}, which could not be read: {source}")]
    Unreadable {
        variable: &'static str,
        path: String,
        #[source]
        source: std::io::Error,
    },
    /// Exactly one half of the TLS pair was named.
    #[error(
        "OQUEUE_TLS_CERT and OQUEUE_TLS_KEY must be set together; {set} is set and {unset} is not"
    )]
    HalfTls {
        set: &'static str,
        unset: &'static str,
    },
    /// The certificate/key pair was named but `rustls` refused it.
    #[error("the TLS certificate and key could not be used: {0}")]
    Tls(#[source] oqueue_broker::tls::TlsConfigError),
    /// A line in a `principal:secret` or `principal:topic` file is not one.
    #[error("{variable}: line {line} is not \"{shape}\": {reason}")]
    MalformedLine {
        variable: &'static str,
        line: usize,
        shape: &'static str,
        reason: String,
    },
    /// A source was named but contains nothing usable.
    #[error(
        "{variable} is set but contains no {shape} — an empty source is not the same as an unset \
         one, and running without what it configures is never what naming it asked for"
    )]
    EmptySource {
        variable: &'static str,
        shape: &'static str,
    },
    /// Credentials were configured without TLS.
    #[error(
        "OQUEUE_CREDENTIALS is set but OQUEUE_TLS_CERT/OQUEUE_TLS_KEY are not. SASL/PLAIN is \
         only ever accepted inside a TLS session (ADR-0032), so no client could authenticate \
         and every request would be refused — this broker would serve nobody"
    )]
    CredentialsWithoutTls,
    /// A source is not valid UTF-8.
    #[error("{variable} is not valid UTF-8; a password or name this broker cannot read is not one")]
    NotUtf8 { variable: &'static str },
    /// The quota is not a number, or is zero.
    #[error("OQUEUE_MAX_IN_FLIGHT must be a positive integer, not {value:?}")]
    Quota { value: String },
}

/// What `serve` needs to hand every connection.
///
/// ⚠️ **`Debug` is written by hand and redacts.** A derived one is not
/// available anyway (`TlsAcceptor` has none), and writing one that printed
/// the credential set would put every configured principal within one
/// `{:?}` of a log line — the passwords inside are `Redacted`, but the
/// principal names are not, and a set of valid names is worth having. This
/// prints shapes and counts only. [`Security::describe`] is the operator
/// surface; this exists so a `Result<Security, _>` can be unwrapped in a
/// test.
pub(crate) struct Security {
    /// `None` means cleartext — see [`Security::describe`].
    pub(crate) acceptor: Option<TlsAcceptor>,
    credentials: PlainCredentials,
    topic_grants: TopicGrants,
    group_grants: GroupGrants,
    admin_grants: AdminGrants,
    /// ⚠️ Recorded rather than asked of `TopicGrants`, which has no
    /// `is_empty` — and adding one to `oqueue-core` for a composition
    /// root's warning would be a library change made for the wrong reason.
    grants_configured: bool,
    group_grants_configured: bool,
    admin_grants_configured: bool,
    quota: Option<Arc<PrincipalQuota>>,
}

impl Security {
    /// No TLS, no credentials, no grants, no quota — what an unconfigured
    /// deployment runs, and what [`Security::describe`] warns about.
    ///
    /// ⚠️ **A named constructor rather than `Default`.** It says which of
    /// the many possible "empty" states this is, and it denies `cargo
    /// mutants` the `Ok(Default::default())` it otherwise substitutes for
    /// every fallible function returning one — a mutant that turns a
    /// configuration *failure* into a silently unconfigured broker, which
    /// is precisely the outcome this module exists to prevent and which no
    /// test of a pure function can catch.
    #[cfg(test)]
    pub(crate) fn cleartext() -> Self {
        Self {
            acceptor: None,
            credentials: PlainCredentials::default(),
            topic_grants: TopicGrants::new(),
            group_grants: GroupGrants::new(),
            admin_grants: AdminGrants::new(),
            grants_configured: false,
            group_grants_configured: false,
            admin_grants_configured: false,
            quota: None,
        }
    }
}

impl std::fmt::Debug for Security {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Security")
            .field("tls", &self.acceptor.is_some())
            .field("credentials_configured", &!self.credentials.is_empty())
            .field("quota_configured", &self.quota.is_some())
            .field("admin_grants_configured", &self.admin_grants_configured)
            .field("group_grants_configured", &self.group_grants_configured)
            .finish_non_exhaustive()
    }
}

impl Security {
    /// Applies everything read here to one connection's own dispatcher.
    ///
    /// ⚠️ **`tls_terminated` is asserted by the composer and by nobody
    /// else**, which is the whole reason it is a separate call from
    /// `with_credentials`: the `Dispatcher` cannot see whether the `S` it
    /// was handed is a `TlsStream`, so the one place that *does* know has
    /// to say so. Getting this wrong in the permissive direction — calling
    /// it on a cleartext socket — would let `SASL/PLAIN` send a password in
    /// the clear, so it is derived from the acceptor rather than from a
    /// second flag somebody could set independently.
    /// ⚠️ **Called on the connection's own task, never in the accept loop.**
    /// It clones the credential set and the grant map, which is work
    /// proportional to the configuration and paid for *every* TCP connect —
    /// including peers that never finish a handshake. Doing it before
    /// returning to `accept` is the same head-of-line cost
    /// `spawn_connection` exists to avoid, reached by a different route.
    /// Found by review.
    pub(crate) fn dispatcher(&self, cluster: Arc<oqueue_broker::Cluster>) -> Dispatcher {
        let mut dispatcher = Dispatcher::new(cluster);
        if self.acceptor.is_some() {
            dispatcher = dispatcher.tls_terminated();
        }
        dispatcher = dispatcher
            .with_credentials(self.credentials.clone())
            .with_topic_grants(self.topic_grants.clone())
            .with_group_grants(self.group_grants.clone())
            .with_admin_grants(self.admin_grants.clone());
        if let Some(quota) = self.quota.as_ref() {
            dispatcher = dispatcher.with_quota(Arc::clone(quota));
        }
        dispatcher
    }

    /// One line per thing an operator would want to see confirmed, and one
    /// warning per thing they may not have meant to leave off.
    pub(crate) fn describe(&self) -> Vec<String> {
        let mut lines = Vec::new();
        match self.acceptor {
            Some(_) => {
                lines.push("oqueue: TLS enabled (OQUEUE_TLS_CERT/OQUEUE_TLS_KEY)".to_owned());
            }
            None => lines.push(
                "oqueue: WARNING -- no TLS. Every byte, including any SASL/PLAIN password, \
                 crosses the network in the clear. Set OQUEUE_TLS_CERT and OQUEUE_TLS_KEY."
                    .to_owned(),
            ),
        }
        if self.credentials.is_empty() {
            lines.push(
                "oqueue: WARNING -- no credentials configured, so every connection is \
                 unauthenticated and authorization fails open. Set OQUEUE_CREDENTIALS."
                    .to_owned(),
            );
        }
        self.append_authorization_warnings(&mut lines);
        // ⚠️ **A quota with nobody to charge it to does nothing.**
        // `admit_quota` keys on the authenticated principal, so without
        // credentials it sees none and admits everything — the same
        // "configured but can never take effect" shape that
        // `CredentialsWithoutTls` makes fatal. Warned rather than refused,
        // because unlike that case the broker still serves its clients
        // correctly; what the operator loses is only the protection they
        // asked for. Found by review.
        if self.quota.is_some() && self.credentials.is_empty() {
            lines.push(
                "oqueue: WARNING -- OQUEUE_MAX_IN_FLIGHT is set but no credentials are, so the \
                 quota is keyed on a principal that never exists and admits everything. Set \
                 OQUEUE_CREDENTIALS."
                    .to_owned(),
            );
        }
        if self.quota.is_none() {
            lines.push(
                "oqueue: WARNING -- no per-principal quota, so one client can hold every \
                 in-flight slot. Set OQUEUE_MAX_IN_FLIGHT."
                    .to_owned(),
            );
        }
        lines
    }

    fn append_authorization_warnings(&self, lines: &mut Vec<String>) {
        // ⚠️ **The one absence that fails *closed*, and so the one most
        // likely to look like a broker bug.** With credentials configured
        // and no grants, every authenticated client is refused every topic —
        // safe, but indistinguishable from a broken deployment unless this
        // says so. Found by review.
        if !self.credentials.is_empty() && !self.grants_configured {
            lines.push(
                "oqueue: WARNING -- credentials are configured but no topic grants are, so \
                 every authenticated client will be refused every topic. Set \
                 OQUEUE_TOPIC_GRANTS."
                    .to_owned(),
            );
        }
        if !self.credentials.is_empty() && !self.admin_grants_configured {
            lines.push(
                "oqueue: WARNING -- no admin grants configured, so administrative operations deny by default. Set OQUEUE_ADMIN_GRANTS."
                    .to_owned(),
            );
        }
        if !self.credentials.is_empty() && !self.group_grants_configured {
            lines.push(
                "oqueue: WARNING -- no group grants configured, so group operations deny by default. Set OQUEUE_GROUP_GRANTS."
                    .to_owned(),
            );
        }
    }
}

mod sources;
pub(crate) use sources::configured;

#[cfg(test)]
mod tests;
