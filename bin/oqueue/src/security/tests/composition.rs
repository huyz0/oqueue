//! How sources combine into a `Security`, and what it tells an operator.
//!
//! ⚠️ **Its sibling asks whether one source's *text* parses; this asks what
//! a whole configuration means.** The two fail differently and that is the
//! point: a malformed line is caught by a parser, while "credentials without
//! TLS" and "a quota with no principal to charge" are only visible once the
//! sources are read together — and each of those was a real defect review
//! measured against a running broker.
//!
//! ⚠️ **Every warning is tested from both sides.** Each `describe()`
//! condition here had a one-sided test first, and `cargo mutants` flipped its
//! `&&` to `||` and passed both times. A warning that fires when it should
//! not is not harmless: it sends an operator to fix something already right.

#![allow(clippy::expect_used)]

use super::tls;
use crate::security::sources::{credentials_from, from_sources, topic_grants_from};

/// ⚠️ **The whole load fails, not the one bad source.** A `Security` built
/// with three good sources and one malformed one would run — authenticated,
/// but with no grants checked — which is a state no operator asked for.
#[test]
fn one_malformed_source_fails_the_whole_load() {
    from_sources(
        Some(tls()),
        Some("alice:secret\n"),
        Some("no-colon-here\n"),
        Some("64"),
    )
    .expect_err("a malformed grant file must stop startup, not be skipped");
}

/// ⚠️ **An absent source is a choice; a malformed one is not.** That is the
/// distinction this whole module turns on, so it has its own test.
#[test]
fn absent_sources_load_and_are_warned_about() {
    let security = from_sources(None, None, None, None).expect("absent is legal");
    let warnings = security.describe();
    assert!(
        warnings.iter().any(|w| w.contains("no TLS")),
        "cleartext must be said out loud: {warnings:?}"
    );
    assert!(
        warnings.iter().any(|w| w.contains("no credentials")),
        "so must unauthenticated: {warnings:?}"
    );
}

/// ⚠️ **`tls_terminated` is derived from the acceptor, never set beside
/// it.** Asserting it on a cleartext socket is what would let `SASL/PLAIN`
/// send a password in the clear, so the two cannot be set independently.
#[test]
fn tls_is_announced_only_when_an_acceptor_exists() {
    let cleartext = from_sources(None, None, None, None).expect("loads");
    assert!(cleartext.acceptor.is_none());
    assert!(
        cleartext.describe().iter().any(|w| w.contains("no TLS")),
        "and it says so"
    );
}

#[test]
fn a_malformed_certificate_is_refused_rather_than_ignored() {
    let error = from_sources(Some((b"not a pem", b"not a key")), None, None, None)
        .expect_err("must refuse");
    assert!(
        error.to_string().contains("TLS"),
        "the message names what failed: {error}"
    );
}

/// ⚠️ **The redacting `Debug` is tested because the point of it is what it
/// leaves out.** `cargo mutants` replaced the whole `fmt` body with
/// `Ok(Default::default())` — printing nothing at all — and nothing failed,
/// which is a fair complaint: an impl no test reads is an impl that can
/// quietly start printing anything, including the credential set it exists
/// to keep out of a log line.
#[test]
fn debug_says_what_is_configured_and_never_what_it_is() {
    let security =
        from_sources(Some(tls()), Some("alice:hunter2\n"), None, Some("64")).expect("loads");
    let rendered = format!("{security:?}");

    assert!(
        rendered.contains("credentials_configured: true"),
        "an operator reading a log must be able to tell configured from not: {rendered}"
    );
    assert!(
        rendered.contains("quota_configured: true"),
        "likewise the quota: {rendered}"
    );
    assert!(
        rendered.contains("tls: true"),
        "and whether this connection is protected: {rendered}"
    );
    assert!(
        !rendered.contains("hunter2"),
        "the password must never reach a log line: {rendered}"
    );
    assert!(
        !rendered.contains("alice"),
        "nor the principal — a set of valid names is worth having, and \
         `Redacted` protects the password but not the name beside it: {rendered}"
    );
}

/// ⚠️ **A named file that cannot be read is fatal, and an unnamed one is
/// not.** That distinction is the module's whole thesis applied to the
/// filesystem: an operator who set `OQUEUE_CREDENTIALS` to a path that does
/// not exist has made a mistake, and starting unauthenticated because of it
/// is the failure this refuses.
#[test]
fn a_named_source_that_cannot_be_read_is_fatal() {
    let error = crate::security::sources::read_source(
        "OQUEUE_CREDENTIALS",
        Some("/nonexistent/credentials".into()),
    )
    .expect_err("a named file that is not there must stop startup");
    assert!(
        error.to_string().contains("OQUEUE_CREDENTIALS"),
        "the message names the variable so the operator knows which: {error}"
    );
    assert!(
        error.to_string().contains("/nonexistent/credentials"),
        "and the path it tried: {error}"
    );
}

#[test]
fn an_unnamed_source_is_absent_rather_than_an_error() {
    let absent =
        crate::security::sources::read_source("OQUEUE_CREDENTIALS", None).expect("unset is legal");
    assert!(absent.is_none(), "nobody asked for one");
}

#[test]
fn a_named_source_that_exists_is_read() {
    let file = std::env::temp_dir().join("oqueue-security-read-source");
    std::fs::write(&file, b"alice:secret\n").expect("writes");
    let read = crate::security::sources::read_source(
        "OQUEUE_CREDENTIALS",
        Some(file.display().to_string()),
    )
    .expect("readable")
    .expect("named");
    assert_eq!(read, b"alice:secret\n", "the bytes, verbatim");
    std::fs::remove_file(&file).ok();
}

/// ⚠️ **A named source that yields nothing is refused, and this is the most
/// important test in the file.** An empty or comments-only credential file
/// produced an empty `PlainCredentials`, which `is_empty` makes
/// indistinguishable from "no credentials configured" — and `authorize`
/// deliberately fails *open* there, so every request was allowed. Review
/// reproduced it end to end: a comments-only file let an unauthenticated
/// librdkafka client produce. The operator who names a credential file is
/// the one who least wants that.
#[test]
fn a_named_but_empty_credentials_source_is_refused() {
    for empty in ["", "\n\n", "# only a comment\n"] {
        let error = credentials_from(empty)
            .expect_err("an empty credentials source must not mean 'no authentication'");
        assert!(
            error.to_string().contains("OQUEUE_CREDENTIALS"),
            "named against the variable: {error}"
        );
    }
}

/// The same rule, the opposite failure: an empty grant set fails *closed*,
/// refusing every authenticated principal. Safer, still not what naming the
/// variable asked for.
#[test]
fn a_named_but_empty_grants_source_is_refused() {
    topic_grants_from("# nothing here\n").expect_err("an empty grant source is not a grant set");
}

/// ⚠️ **Refused, not rewritten.** `from_utf8_lossy` would turn an invalid
/// byte into U+FFFD, so a password file in the wrong encoding would load as
/// a password nobody can type: the broker starts, looks configured, and
/// rejects the operator's own client.
///
/// ⚠️ **This asserts `decode`'s own behaviour.** An earlier version asserted
/// only that its fixture was invalid UTF-8 — a property of the fixture, not
/// of the code — and stayed green with the decode reverted to lossy. Found
/// by review.
#[test]
fn a_source_that_is_not_utf8_is_refused() {
    let error = crate::security::sources::decode(
        "OQUEUE_CREDENTIALS",
        Some(b"alice:se\xffcret\n".to_vec()),
    )
    .expect_err("an undecodable password is not a password");
    assert!(
        error.to_string().contains("OQUEUE_CREDENTIALS"),
        "named against the variable the operator set: {error}"
    );
    assert!(
        crate::security::sources::decode("OQUEUE_CREDENTIALS", Some(b"alice:secret\n".to_vec()))
            .expect("valid UTF-8 decodes")
            .is_some(),
        "and a legal source still loads"
    );
}

/// ⚠️ **Credentials without grants refuses every authenticated client**, and
/// says so — it is the one absence that fails closed, and so the one most
/// likely to be mistaken for a broken broker.
#[test]
fn credentials_without_grants_are_warned_about() {
    let security = from_sources(Some(tls()), Some("alice:secret\n"), None, None).expect("loads");
    assert!(
        security
            .describe()
            .iter()
            .any(|w| w.contains("no topic grants")),
        "the warning names the gap: {:?}",
        security.describe()
    );
}

/// ⚠️ **The grants warning must not fire when there are no credentials**,
/// which is the other half of its condition and the half a one-sided test
/// leaves unpinned — `cargo mutants` flipped the `&&` to `||` and nothing
/// failed. An unconfigured broker already gets the "no credentials" warning;
/// adding "every authenticated client will be refused" beside it would be
/// advice about a situation that does not exist, in the output an operator
/// reads to find out what *is* wrong.
#[test]
fn the_grants_warning_is_silent_without_credentials() {
    let security = from_sources(None, None, None, None).expect("loads");
    let warnings = security.describe();
    assert!(
        !warnings.iter().any(|w| w.contains("no topic grants")),
        "there are no authenticated clients to refuse: {warnings:?}"
    );
    assert!(
        warnings.iter().any(|w| w.contains("no credentials")),
        "the warning that does apply is still there: {warnings:?}"
    );
}

/// ⚠️ **Credentials without TLS is a broker that serves nobody**, so it is
/// refused at startup rather than warned about. `SASL/PLAIN` is only ever
/// accepted inside a TLS session (`ADR-0032`), so without TLS no client can
/// authenticate — and with credentials configured `authorize` stops failing
/// open, so every request from every client is refused on every API. The
/// only warning this used to print said passwords "cross the network in the
/// clear", which is the opposite of what happens: nothing crosses at all.
#[test]
fn credentials_without_tls_are_refused() {
    let error = from_sources(None, Some("alice:secret\n"), None, None)
        .expect_err("a broker that can serve nobody must not start");
    assert!(
        error.to_string().contains("OQUEUE_TLS_CERT"),
        "the message names what is missing: {error}"
    );
    assert!(
        from_sources(Some(tls()), Some("alice:secret\n"), None, None).is_ok(),
        "and the same credentials with TLS are fine"
    );
}

/// ⚠️ **`NotPresent` and `NotUnicode` are different answers**, and
/// `std::env::var(..).ok()` collapses them: a *set* `OQUEUE_CREDENTIALS`
/// whose value has an undecodable byte would read as "nobody asked for
/// credentials", so the broker starts, prints "no credentials configured",
/// and serves everybody. Measured before the fix. Found by review.
#[test]
fn an_unset_variable_is_absent_and_an_undecodable_one_is_refused() {
    assert!(
        crate::security::sources::from_var(
            "OQUEUE_CREDENTIALS",
            Err(std::env::VarError::NotPresent)
        )
        .expect("unset is legal")
        .is_none(),
        "nobody asked"
    );
    assert_eq!(
        crate::security::sources::from_var("OQUEUE_CREDENTIALS", Ok("/etc/creds".to_owned()))
            .expect("a set value is legal")
            .as_deref(),
        Some("/etc/creds"),
    );
    let undecodable = std::env::VarError::NotUnicode(std::ffi::OsString::from("x"));
    crate::security::sources::from_var("OQUEUE_CREDENTIALS", Err(undecodable))
        .expect_err("a set-but-undecodable variable must not read as unset");
}

/// ⚠️ **A quota with no credentials is inert**, because `admit_quota` keys
/// on the authenticated principal and without credentials there is never
/// one. The broker still serves correctly, so this warns rather than
/// refusing — but silently not applying a limit an operator configured is
/// the failure this whole module is about.
#[test]
fn a_quota_without_credentials_is_warned_about() {
    let security = from_sources(None, None, None, Some("64")).expect("loads");
    assert!(
        security
            .describe()
            .iter()
            .any(|w| w.contains("quota is keyed on a principal that never exists")),
        "the warning names why it does nothing: {:?}",
        security.describe()
    );
}

/// ⚠️ **Every source's path must refuse a set-but-undecodable value**, not
/// just the ones that happened to be rewritten first. Round 2 fixed this for
/// `OQUEUE_TLS_CERT` and left `OQUEUE_CREDENTIALS` calling
/// `std::env::var(..).ok()`, so the *credentials* variable — the one whose
/// absence fails open — still read as unset. Review measured it: the broker
/// started and served every client unauthenticated. This asserts the shared
/// decision directly, so a future call site that skips it is the only way
/// back in.
#[test]
fn every_source_refuses_a_set_but_undecodable_value() {
    for variable in [
        "OQUEUE_TLS_CERT",
        "OQUEUE_TLS_KEY",
        "OQUEUE_CREDENTIALS",
        "OQUEUE_TOPIC_GRANTS",
        "OQUEUE_MAX_IN_FLIGHT",
    ] {
        let undecodable = std::env::VarError::NotUnicode(std::ffi::OsString::from("x"));
        crate::security::sources::from_var(variable, Err(undecodable))
            .expect_err("a set-but-undecodable value must never read as unset");
    }
}

/// ⚠️ **And silent when the quota can take effect**, which is the other
/// half of its condition — `cargo mutants` flipped the `&&` to `||` and
/// nothing failed, the same one-sided test the grants warning had. A
/// configured broker reading "the quota admits everything" when it does not
/// is worse than no warning: it sends an operator to fix something that is
/// already right.
#[test]
fn the_quota_warning_is_silent_when_credentials_exist() {
    let security =
        from_sources(Some(tls()), Some("alice:secret\n"), None, Some("64")).expect("loads");
    assert!(
        !security
            .describe()
            .iter()
            .any(|w| w.contains("quota is keyed on a principal that never exists")),
        "the quota has a principal to charge: {:?}",
        security.describe()
    );
}

/// ⚠️ **The wiring half of this whole task, and until this test nothing
/// pinned it.** `M9` built `tls_terminated`, `with_credentials`,
/// `with_topic_grants` and `with_quota` and left them with zero call sites;
/// `M4.18` added the one call site. Review found that deleting any of those
/// calls from `Security::dispatcher` left **every** gate green — no test
/// built a dispatcher, and `cargo mutants` generates nothing for a builder
/// chain — while producing a broker that prints "TLS enabled", warns about
/// nothing, and serves every unauthenticated client with full topic access.
/// That is M9.21's original defect exactly, restored by a one-line deletion
/// nothing would have caught.
///
/// ⚠️ **Asserted through `Debug` because that is what is observable.** The
/// fields are private to `oqueue-broker` and the builder returns `Self`, so
/// a composition root cannot read back what it set; `Dispatcher` derives
/// `Debug`, which is enough to see that a credential set and a grant map
/// arrived and that TLS was asserted.
#[tokio::test]
async fn every_configured_source_reaches_the_dispatcher() {
    // ⚠️ The same cluster the binary composes, with fakes —
    // `serve::tests`'s own route to one, because `oqueue-broker`'s
    // `testing::fixture` is `#[cfg(test)]` and so not reachable from here.
    let store: std::sync::Arc<dyn oqueue_core::ObjectStore> =
        std::sync::Arc::new(oqueue_core::FakeObjectStore::new());
    let (cluster, _serving, _retention, _log) =
        crate::compose::build_cluster("h".to_owned(), 1, store)
            .await
            .expect("an empty fixture composes");
    let security = from_sources(
        Some(tls()),
        Some("alice:secret\n"),
        // ⚠️ **A different principal in each source, deliberately.** With
        // `alice` in both, dropping `.with_credentials(...)` left this test
        // green — "alice" still appeared, via the *grants*. Two names make
        // each assertion name one source and only that source. A test that
        // passes for a reason other than the one it plants is not a test.
        Some("bob:orders\n"),
        Some("64"),
    )
    .expect("a fully configured deployment");

    let rendered = format!("{:?}", security.dispatcher(std::sync::Arc::new(cluster)));

    assert!(
        rendered.contains("tls: true"),
        "TLS must be asserted, or SASL/PLAIN refuses every credential: {rendered}"
    );
    assert!(
        rendered.contains("alice"),
        "the credential set must reach the dispatcher, or every client is \
         unauthenticated and authorization fails open: {rendered}"
    );
    assert!(
        rendered.contains("bob") && rendered.contains("orders"),
        "the grants must reach it, or an authenticated client sees every \
         topic: {rendered}"
    );
    assert!(
        rendered.contains("64"),
        "and the quota, or one principal can hold every in-flight slot: \
         {rendered}"
    );
}
