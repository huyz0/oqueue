#![allow(clippy::expect_used)]

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
        "OQUEUE_ADMIN_GRANTS",
        "OQUEUE_GROUP_GRANTS",
        "OQUEUE_MAX_IN_FLIGHT",
    ] {
        let undecodable = std::env::VarError::NotUnicode(std::ffi::OsString::from("x"));
        crate::security::sources::from_var(variable, Err(undecodable))
            .expect_err("a set-but-undecodable value must never read as unset");
    }
}
