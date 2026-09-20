//! ⚠️ **Every test here is about failing *closed*.** `M9` shipped each
//! mechanism with a permissive default, which is right for a library —
//! `PlainCredentials::is_empty` exists so `authorize` can fail open when no
//! credential could ever be presented. It is wrong for a composition root
//! reading an operator's configuration: the operator who set
//! `OQUEUE_CREDENTIALS` asked for authentication, and a broker that starts
//! without it because the file had a typo accepts everybody while looking,
//! from outside, exactly like one that does not.

#![allow(clippy::expect_used)]

mod composition;

use super::sources::{admin_grants_from, credentials_from, quota_from, topic_grants_from};

#[test]
fn an_admin_grant_names_a_supported_operation() {
    let alice = oqueue_core::Principal::new("alice").expect("valid principal");
    for (wire_name, operation) in [
        ("create_topics", oqueue_core::AdminOperation::CreateTopics),
        ("delete_topics", oqueue_core::AdminOperation::DeleteTopics),
        (
            "describe_configs",
            oqueue_core::AdminOperation::DescribeConfigs,
        ),
        ("alter_configs", oqueue_core::AdminOperation::AlterConfigs),
        (
            "describe_groups",
            oqueue_core::AdminOperation::DescribeGroups,
        ),
        ("list_groups", oqueue_core::AdminOperation::ListGroups),
        ("alter_quotas", oqueue_core::AdminOperation::AlterQuotas),
    ] {
        let grants = admin_grants_from(&format!("alice:{wire_name}\n")).expect("parses");
        assert!(
            grants.allows(&alice, operation),
            "the supported operation {wire_name} must be granted"
        );
    }
}

#[test]
fn an_unknown_admin_operation_is_refused() {
    let error = admin_grants_from("alice:make_everything\n").expect_err("must refuse");
    assert!(
        error.to_string().contains("OQUEUE_ADMIN_GRANTS"),
        "the error names the configured source: {error}"
    );
}

#[test]
fn an_empty_admin_grant_source_is_refused() {
    admin_grants_from("# no authority\n").expect_err("empty authority must not silently deny all");
}

/// A valid self-signed pair, so a test can configure credentials — which
/// `from_sources` refuses without TLS, deliberately.
///
/// ⚠️ **Copied from `oqueue-broker`'s `tls::tests`, because a
/// `#[cfg(test)] const` is not reachable across a crate boundary.** Its
/// provenance and the two properties that matter are documented there: the
/// `subjectAltName` (a modern `rustls`/`webpki` client refuses a bare CN,
/// RFC 6125) and `basicConstraints=CA:FALSE` (without it the self-signed
/// output verifies as a CA certificate, which `webpki` refuses as an
/// end-entity leaf). Nothing here depends on either — `server_config` only
/// has to accept the pair — but a reader comparing the two copies should
/// know neither is arbitrary.
pub(super) const TEST_CERT_PEM: &[u8] = br"-----BEGIN CERTIFICATE-----
MIIDSTCCAjGgAwIBAgIUJabZOBs1YBF491KjE6CpxEJ3rzgwDQYJKoZIhvcNAQEL
BQAwFjEUMBIGA1UEAwwLb3F1ZXVlLXRlc3QwHhcNMjYwOTAyMTExNjUwWhcNMzYw
ODMwMTExNjUwWjAWMRQwEgYDVQQDDAtvcXVldWUtdGVzdDCCASIwDQYJKoZIhvcN
AQEBBQADggEPADCCAQoCggEBALmYDUsU9BU1H+pech38TE2crKX4744RL99AjZAE
RuI114sPRIgtKQleE5uyz7eDCwcMYZKQAvvWBjP5uy2eVOqbDB6p7Z/ZQCHrcSr6
ST32Wz9Gmgbr4y6e0HkxHlL2hjSQ57WibU3pyQZ/a7EQcFVVnbeVDAtYTKvDbFXc
OEr92ZuSw+lvCqk1XZyoWpYWSJQPdj+eAh6SKkQ2XEZez3UtqDQUiNlf7AnEstRN
WtIwpJyTfW7k9gnF09iyzoGSTc50hGg7WqlX+lTbjFrv1hOYN/gG1hnTW+8YROFa
crkSod+K4syMnQ/vLE8S8EgMq/4QCt8VHXLMoAp0wDuM6jECAwEAAaOBjjCBizAd
BgNVHQ4EFgQUWLYViLeMeOAXo7OXQ7IVQAV5XTIwHwYDVR0jBBgwFoAUWLYViLeM
eOAXo7OXQ7IVQAV5XTIwFgYDVR0RBA8wDYILb3F1ZXVlLXRlc3QwDAYDVR0TAQH/
BAIwADAOBgNVHQ8BAf8EBAMCBaAwEwYDVR0lBAwwCgYIKwYBBQUHAwEwDQYJKoZI
hvcNAQELBQADggEBACF+t70iWvSBgXs5RbwhIv+4uu9X7uE7BTCDZ3mcpd6DopbY
mSrGqAbDx5J4GdstOcFtrxKpUl8AIDTJ7FnP8ZyTLaPw5hJtLLWUe+EGXCduqOcq
P9ByBbYzOeheN5pxasCaP3WffkIJkyiONngxuxOCP9wd+vNBRoL9tUXbLtObN3Pr
x1UsBHBve3yaMdzIjn/E4QbloHjPL3NpwpqEuvnZPzZW2b5xaRtXfeiSwE536tnY
UhqHHOZZe5/aIJeTr/ZqYCkXNcCKNsecLD4vT9JRh/ckTGg5E/RhrrQCwHqjF6p2
f1uwMnSotmiPF0Z5XBAvMnoJhMFJrxBsYR07dwg=
-----END CERTIFICATE-----
";

pub(super) const TEST_KEY_PEM: &[u8] = br"-----BEGIN PRIVATE KEY-----
MIIEvQIBADANBgkqhkiG9w0BAQEFAASCBKcwggSjAgEAAoIBAQC5mA1LFPQVNR/q
XnId/ExNnKyl+O+OES/fQI2QBEbiNdeLD0SILSkJXhObss+3gwsHDGGSkAL71gYz
+bstnlTqmwweqe2f2UAh63Eq+kk99ls/RpoG6+MuntB5MR5S9oY0kOe1om1N6ckG
f2uxEHBVVZ23lQwLWEyrw2xV3DhK/dmbksPpbwqpNV2cqFqWFkiUD3Y/ngIekipE
NlxGXs91Lag0FIjZX+wJxLLUTVrSMKSck31u5PYJxdPYss6Bkk3OdIRoO1qpV/pU
24xa79YTmDf4BtYZ01vvGEThWnK5EqHfiuLMjJ0P7yxPEvBIDKv+EArfFR1yzKAK
dMA7jOoxAgMBAAECggEADntKmD/1jr0TNTSy51uTMaAmyZmbyaRWLa+qDCGFTWvh
mnhxwsVxVQmJ8qV4d0uKnf1tlKPXk8J+v+n93MCkxByejLr6L3WifzMRpMacVfEl
7BFMfftEghQC1N4MDXGuhaYD6oSWzlROawsgwlNzzHjOgm9nHfCBJQrt5mI1Y0Z6
s0xg4j5oc3Z7elcmculXlksjsSuq64oI3ci+r0YR5dwU3mXZk0MeV1of876St7Pt
YznTFYx41Dbwb6+xfaAIMrHZFODPpoidZ9V+2Y16s3F5qQEUsBz4Z7aZCQlwL6qj
/Qc9SzBg67Wg41MMMuinGjApeROBpfAwsG4N05jrBQKBgQD5folRLcLUzbH0uSMm
8z2Obueu+IBEl6EkzXdX5iM2qn4Opa6xU2mYiOswdXVk8th4+AxsVc8Cfu8SN88r
rYfAmVvR47ExRXAg8pEm3nbgSh/m+r0S1CySkz9jON6h6OHguqf9nnDlmTQF4pfo
ecpdv7qAoKAKPLZEFIRkXVq9lQKBgQC+bvU9SWf3o1K0Bt6b325VtzwhBZfcIQ6P
54c88awwLhiHbDH0c/fSpcJHN6HIoN+YATrru9Tj+fx1FCkzGdms/1QbJYql+aAK
HvnTjkC0KScpgTZN7n/s+wdjaOhJdIy753zKwF2d2mZ3gzlclhxCNjYEu0klz87d
loKf5Ld7LQKBgQCxw6XFUGycQT8FVhAkxXTbkkvDUE3cEYmAdmENIO2AGrQcbZJt
yDfZtdyVN2uAlMMGVf5MBkurxJNEkL0sqsSpxts0Th5HM+lzoEEpx6I9prLaWVb0
HnbvrLiiUrfV9t9RxszBGO3puWHmu49u1bAJYf1ZfpjpEl7vXQsDk7x+jQKBgC9x
5ZfHWifQgSJpM70SBaNFa62ufw9RDRe9T2xXqda3JVVYF3oYCn5o3eZwbdZWfl6Y
r91bhsbl2Ygx5bHdluYLFyFMUSbY8o6S+RtELcq1FhS5JJZ1/VlFkamq0XS7nPST
z/uTwb86Up0kDH6Mx62XZA35u1e4VonOnezIRw5hAoGAdBn2wSOxjYk6z9JfuP26
op89S+ZhRsFbCtnyOJhrG94guGrFgalinykf6qmavnqepwlFzYcbS6jtmAKMqNym
/k+tM2w0VABASirVmzvns2qV8RblR5U8qfHBayZc3TVLXnQhlZyWH5rbqUWxqKMs
QSae45MjrYYeDRpj1qmCRtw=
-----END PRIVATE KEY-----
";

/// The TLS half of a configured deployment, for the tests that are about
/// something else.
pub(super) fn tls() -> (&'static [u8], &'static [u8]) {
    (TEST_CERT_PEM, TEST_KEY_PEM)
}

#[test]
fn a_credential_line_without_a_separator_is_refused() {
    let error = credentials_from("alice-has-no-colon\n").expect_err("must refuse");
    assert!(
        error.to_string().contains("line 1"),
        "the line number is what makes this actionable: {error}"
    );
}

#[test]
fn a_credential_line_with_an_empty_password_is_refused() {
    let error =
        credentials_from("alice:\n").expect_err("an empty password must not authenticate anybody");
    assert!(
        error.to_string().contains("line 1"),
        "and names the line, which is the whole value of the message: {error}"
    );
}

/// ⚠️ **An empty principal is `Principal::new`'s refusal, not this
/// module's.** `pairs` used to reject it too, which made the constructor's
/// error arm unreachable from any input — `cargo mutants` found that by
/// rewriting the line number inside an arm no test could enter. Validation
/// belongs to the type that has one.
#[test]
fn a_credential_line_with_an_empty_principal_is_refused() {
    let error = credentials_from("  :secret\n").expect_err("an empty principal is nobody");
    assert!(
        error.to_string().contains("line 1"),
        "and names the line: {error}"
    );
}

#[test]
fn a_grant_line_with_an_empty_principal_is_refused() {
    let error = topic_grants_from(":orders\n").expect_err("an empty principal is nobody");
    assert!(
        error.to_string().contains("OQUEUE_TOPIC_GRANTS"),
        "named against the variable the operator set: {error}"
    );
}

/// ⚠️ **The line number is the line in the *file*, not the index among the
/// lines that parsed.** Comments and blanks are skipped, so an operator
/// counting down their own file has to land on the same line this names —
/// which is only true if the number is carried from `lines().enumerate()`
/// rather than re-derived after filtering.
#[test]
fn the_reported_line_is_the_line_in_the_file() {
    let error = credentials_from("# a note\n\nalice:secret\nbroken-line\n")
        .expect_err("line four is malformed");
    assert!(
        error.to_string().contains("line 4"),
        "the fourth line of the file, not the second entry: {error}"
    );
}

/// ⚠️ **A password may contain a colon and a principal may not**, so the
/// split is on the *first* separator only. Splitting on every one would
/// refuse a legal password, and would do it only for the operators who
/// happened to choose one.
#[test]
fn a_password_may_contain_a_colon() {
    let credentials = credentials_from("alice:pa:ss:word\n").expect("a legal password");
    assert!(!credentials.is_empty(), "the credential is kept");
}

#[test]
fn comments_and_blank_lines_are_ignored_not_refused() {
    let credentials =
        credentials_from("# the ops team's own note\n\nalice:secret\n\n").expect("parses");
    assert!(!credentials.is_empty(), "the one real line is read");
}

/// ⚠️ **A grant with no separator, not a grant with an invalid topic.** A
/// first version of this test used `alice:a topic with spaces` and failed,
/// because `TopicId::new` rejects only the empty string — spaces are a legal
/// topic name here. The reachable malformed shape is the one `pairs` itself
/// refuses, and testing the other would have been testing a validation this
/// tree does not perform.
#[test]
fn a_grant_line_that_is_not_a_pair_is_refused() {
    let error = topic_grants_from("alice-and-no-topic\n").expect_err("must refuse");
    assert!(
        error.to_string().contains("OQUEUE_TOPIC_GRANTS"),
        "the message names the variable the operator set: {error}"
    );
}

/// ⚠️ **Zero is a mistake, not "unlimited".** A zero quota admits nothing,
/// so neither reading of it is what an operator meant; "unlimited" is
/// already what leaving the variable unset says.
#[test]
fn a_zero_or_unparsable_quota_is_refused() {
    quota_from("0").expect_err("zero admits nothing at all");
    quota_from("lots").expect_err("not a number");
    quota_from("-1").expect_err("not a u32");
    quota_from("64").expect("a positive integer is the one legal shape");
}
