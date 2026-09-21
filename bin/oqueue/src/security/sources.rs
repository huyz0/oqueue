//! Reading the configuration: which sources exist, and what refusing a
//! malformed one means.
//!
//! ⚠️ **Split from `security.rs` by concept, not size.** Its parent owns
//! what a configured deployment *is* — the type every connection is built
//! from, and what it tells an operator. This owns getting there from an
//! environment and a filesystem, which is where every way of being wrong
//! lives: a half-named TLS pair, a credential file that parses to nothing, a
//! source that is not UTF-8, a variable set to something undecodable.
//!
//! ⚠️ **Every function here takes its input as an argument**, including the
//! ones whose input is the environment. `store_for`/`chosen_store`'s
//! division two files over, for the same reason and with the same result:
//! reading an environment variable is process-global state, so a test that
//! set one would race every other test in the binary.

use super::{Security, SecurityError};
use oqueue_broker::sasl_authenticate::{PlainCredential, PlainCredentials};
use oqueue_core::{
    AdminGrants, AdminOperation, GroupGrants, GroupId, Principal, PrincipalQuota, Redacted,
    TopicGrants, TopicId,
};
use std::sync::Arc;

/// Reads every source this deployment names.
///
/// ⚠️ **Split from [`configured`] for the reason `store_for` is split from
/// `chosen_store`**: reading an environment variable is process-global
/// state, so a test that set one would race every other test in the
/// binary. Everything below this line takes its input as arguments.
///
/// # Errors
/// [`SecurityError`] if any named source is unreadable or malformed. Never
/// for a source that is simply absent — that is the operator declining a
/// feature, which [`Security::describe`] warns about rather than refusing.
#[cfg(test)]
pub(super) fn from_sources(
    tls: Option<(&[u8], &[u8])>,
    credentials: Option<&str>,
    topic_grants: Option<&str>,
    quota: Option<&str>,
) -> Result<Security, SecurityError> {
    from_sources_with_policies(tls, credentials, topic_grants, quota, None, None)
}

/// As [`from_sources`], with independent administrative and group policies.
// This test-only seam keeps each optional policy independently configurable.
#[allow(clippy::too_many_arguments)]
pub(super) fn from_sources_with_policies(
    tls: Option<(&[u8], &[u8])>,
    credentials: Option<&str>,
    topic_grants: Option<&str>,
    quota: Option<&str>,
    admin_grants: Option<&str>,
    group_grants: Option<&str>,
) -> Result<Security, SecurityError> {
    // ⚠️ **Credentials without TLS is a broker that serves nobody**, so it
    // is refused rather than warned about. `sasl_authenticate::handle`
    // matches a credential only when the connection is TLS-terminated
    // (`ADR-0032`: the password is protected by the session, not by the
    // mechanism), so without TLS no client can ever authenticate — and with
    // credentials configured, `authorize` stops failing open, so every
    // request from every client is refused on every API. Measured. ⚠️ The
    // only warning this used to print said every password "crosses the
    // network in the clear", which is the opposite of what happens: nothing
    // crosses at all. Found by review.
    if credentials.is_some() && tls.is_none() {
        return Err(SecurityError::CredentialsWithoutTls);
    }
    Ok(Security {
        grants_configured: topic_grants.is_some(),
        acceptor: tls
            .map(|(cert, key)| oqueue_broker::tls::acceptor(cert, key).map_err(SecurityError::Tls))
            .transpose()?,
        credentials: credentials
            .map(credentials_from)
            .transpose()?
            .unwrap_or_default(),
        topic_grants: Arc::new(std::sync::RwLock::new(
            topic_grants
                .map(topic_grants_from)
                .transpose()?
                .unwrap_or_default(),
        )),
        admin_grants: admin_grants
            .map(admin_grants_from)
            .transpose()?
            .unwrap_or_default(),
        admin_grants_configured: admin_grants.is_some(),
        group_grants: group_grants
            .map(group_grants_from)
            .transpose()?
            .unwrap_or_default(),
        group_grants_configured: group_grants.is_some(),
        quota: quota.map(quota_from).transpose()?,
    })
}

/// [`from_sources`] over what the environment names.
///
/// # Errors
/// As [`from_sources`], plus [`SecurityError::Unreadable`] for a named file
/// that cannot be read and [`SecurityError::HalfTls`] for one half of a
/// pair.
pub(crate) fn configured() -> Result<Security, SecurityError> {
    // ⚠️ A closure, not a helper function: a one-line wrapper around
    // `std::env::var` is something no test can reach, and `cargo mutants`
    // rightly refuses to let one stand — it replaced two such wrappers with
    // `Ok(None)` over this task and nothing failed either time. The decision
    // that matters lives in `from_var`, which takes the read as an argument.
    let var = |variable: &'static str| from_var(variable, std::env::var(variable));

    let cert = read_source("OQUEUE_TLS_CERT", var("OQUEUE_TLS_CERT")?)?;
    let key = read_source("OQUEUE_TLS_KEY", var("OQUEUE_TLS_KEY")?)?;
    let tls = match (cert, key) {
        (Some(cert), Some(key)) => Some((cert, key)),
        (None, None) => None,
        // ⚠️ **One half is a mistake, not a choice.** Falling back to
        // cleartext because the key path was typo'd starts exactly the
        // broker the operator was trying not to start.
        (Some(_), None) => {
            return Err(SecurityError::HalfTls {
                set: "OQUEUE_TLS_CERT",
                unset: "OQUEUE_TLS_KEY",
            });
        }
        (None, Some(_)) => {
            return Err(SecurityError::HalfTls {
                set: "OQUEUE_TLS_KEY",
                unset: "OQUEUE_TLS_CERT",
            });
        }
    };
    let credentials = read_source("OQUEUE_CREDENTIALS", var("OQUEUE_CREDENTIALS")?)?;
    let topic_grants = read_source("OQUEUE_TOPIC_GRANTS", var("OQUEUE_TOPIC_GRANTS")?)?;
    let admin_grants = read_source("OQUEUE_ADMIN_GRANTS", var("OQUEUE_ADMIN_GRANTS")?)?;
    let group_grants = read_source("OQUEUE_GROUP_GRANTS", var("OQUEUE_GROUP_GRANTS")?)?;
    from_sources_with_policies(
        tls.as_ref().map(|(c, k)| (c.as_slice(), k.as_slice())),
        decode("OQUEUE_CREDENTIALS", credentials)?.as_deref(),
        decode("OQUEUE_TOPIC_GRANTS", topic_grants)?.as_deref(),
        var("OQUEUE_MAX_IN_FLIGHT")?.as_deref(),
        decode("OQUEUE_ADMIN_GRANTS", admin_grants)?.as_deref(),
        decode("OQUEUE_GROUP_GRANTS", group_grants)?.as_deref(),
    )
}

/// A source's bytes as text.
///
/// ⚠️ **Refused, not rewritten.** `from_utf8_lossy` turns an invalid byte
/// into U+FFFD, so a password file saved in the wrong encoding would load as
/// a password nobody can type: the broker starts, looks configured, and
/// rejects the operator's own client. ⚠️ **A free function rather than a
/// closure inside `configured`**, so a test can reach it — as a closure the
/// only test of it asserted a property of its own fixture and stayed green
/// when the decode was reverted to lossy. Found by review.
pub(super) fn decode(
    variable: &'static str,
    bytes: Option<Vec<u8>>,
) -> Result<Option<String>, SecurityError> {
    bytes
        .map(|b| String::from_utf8(b).map_err(|_| SecurityError::NotUtf8 { variable }))
        .transpose()
}

/// The value of `variable`, or `None` if it is unset.
///
/// ⚠️ **Takes the read rather than performing it, so it can be tested** —
/// `read_source`'s own division, and `store_for`/`chosen_store`'s before it.
/// A wrapper that only called `std::env::var` would be one more function no
/// test could reach, which `cargo mutants` says plainly by replacing it with
/// `Ok(None)` and watching nothing fail.
///
/// ⚠️ **Split so it can be tested at all** — `read_source`'s own division,
/// and `store_for`/`chosen_store`'s before it. The distinction it draws is
/// the one that matters: `NotPresent` is an operator declining a feature and
/// `NotUnicode` is a mistake, and `std::env::var(..).ok()` collapses them
/// into each other.
pub(super) fn from_var(
    variable: &'static str,
    read: Result<String, std::env::VarError>,
) -> Result<Option<String>, SecurityError> {
    match read {
        Ok(value) => Ok(Some(value)),
        Err(std::env::VarError::NotPresent) => Ok(None),
        Err(std::env::VarError::NotUnicode(_)) => Err(SecurityError::NotUtf8 { variable }),
    }
}

/// The bytes at `path`, or `None` if nothing named one.
///
/// ⚠️ **Split from the environment lookup so it can be tested at all** —
/// `store_for`/`chosen_store`'s own division one file over. An unset
/// variable is `None` and legal; a named file that cannot be read is fatal,
/// because the operator who named it asked for it.
pub(super) fn read_source(
    variable: &'static str,
    path: Option<String>,
) -> Result<Option<Vec<u8>>, SecurityError> {
    let Some(path) = path else {
        return Ok(None);
    };
    std::fs::read(&path)
        .map(Some)
        .map_err(|source| SecurityError::Unreadable {
            variable,
            path,
            source,
        })
}

/// One `name:value` line per entry; **whole-line** `#` comments and blank
/// lines ignored.
///
/// ⚠️ **A `#` after a value belongs to the value**, and must: a password may
/// legitimately contain one, so there is no safe place to start stripping.
/// `alice:secret # prod` therefore sets the password to `secret # prod` and
/// the broker refuses the operator's own client while the file reads
/// correctly — documented in `bin/oqueue/README.md` because nothing can
/// detect it. Found by review.
///
/// ⚠️ **Split on the *first* colon, not every colon.** A password may
/// contain one and a principal may not, so anything after the first
/// separator belongs to the value — splitting on all of them would refuse
/// a legal password and, worse, would do it only for some operators.
fn pairs<'a>(
    variable: &'static str,
    shape: &'static str,
    text: &'a str,
) -> Result<Vec<(usize, &'a str, &'a str)>, SecurityError> {
    let mut out = Vec::new();
    for (index, raw) in text.lines().enumerate() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((name, value)) = line.split_once(':') else {
            return Err(SecurityError::MalformedLine {
                variable,
                line: index + 1,
                shape,
                reason: "no ':' separator".to_owned(),
            });
        };
        // ⚠️ **Trimmed, which a password notices.** `alice:s3cret ` stores
        // `s3cret`, and the operator's own client is then refused with
        // `SASL/PLAIN authentication failed` — deliberately uninformative,
        // so nothing says why. The trim is still right (a trailing space in
        // a config file is almost always an accident, and a leading one
        // always is), but it is undetectable from the file, so
        // `bin/oqueue/README.md` says so beside the `#` trap it resembles.
        // Found by review.
        let (name, value) = (name.trim(), value.trim());
        // ⚠️ **Only the value is checked here; the name is the
        // constructor's business.** `Principal::new` and `TopicId::new`
        // already reject the empty string, and duplicating that check here
        // made their error arms unreachable from any input — `cargo mutants`
        // found it, by rewriting the line number inside an arm no test could
        // enter. A value has no such constructor (a password is a `String`),
        // so its emptiness has nowhere else to be caught.
        if value.is_empty() {
            return Err(SecurityError::MalformedLine {
                variable,
                line: index + 1,
                shape,
                reason: "an empty value".to_owned(),
            });
        }
        out.push((index + 1, name, value));
    }
    Ok(out)
}

/// `principal:password` lines.
pub(super) fn credentials_from(text: &str) -> Result<PlainCredentials, SecurityError> {
    const VARIABLE: &str = "OQUEUE_CREDENTIALS";
    let mut out = Vec::new();
    for (line, name, password) in pairs(VARIABLE, "principal:password", text)? {
        let principal = Principal::new(name).map_err(|error| SecurityError::MalformedLine {
            variable: VARIABLE,
            line,
            shape: "principal:password",
            reason: error.to_string(),
        })?;
        out.push(PlainCredential {
            principal,
            password: Redacted::new(password.to_owned()),
        });
    }
    // ⚠️ **A named source that yields nothing is refused.** An empty or
    // comments-only credential file would otherwise produce an empty
    // `PlainCredentials`, which `is_empty` makes indistinguishable from "no
    // credentials configured" — and `authorize` deliberately fails *open*
    // there, so every request would be allowed. That is the exact outcome
    // this module exists to prevent, reached by the operator who most
    // clearly meant the opposite. Found by review, end to end: a
    // comments-only file let an unauthenticated client produce.
    if out.is_empty() {
        return Err(SecurityError::EmptySource {
            variable: VARIABLE,
            shape: "principal:password pairs",
        });
    }
    Ok(PlainCredentials::new(out))
}

/// `principal:topic` lines, one grant each.
pub(super) fn topic_grants_from(text: &str) -> Result<TopicGrants, SecurityError> {
    const VARIABLE: &str = "OQUEUE_TOPIC_GRANTS";
    let mut grants = TopicGrants::new();
    let mut empty = true;
    for (line, name, topic) in pairs(VARIABLE, "principal:topic", text)? {
        empty = false;
        let malformed = |error: oqueue_core::Error| SecurityError::MalformedLine {
            variable: VARIABLE,
            line,
            shape: "principal:topic",
            reason: error.to_string(),
        };
        grants.grant(
            Principal::new(name).map_err(malformed)?,
            TopicId::new(topic).map_err(malformed)?,
        );
    }
    // ⚠️ Same rule as credentials, opposite failure: an empty grant set
    // fails *closed*, refusing every request from an authenticated
    // principal. Safer, and still not what naming the variable asked for.
    if empty {
        return Err(SecurityError::EmptySource {
            variable: VARIABLE,
            shape: "principal:topic pairs",
        });
    }
    Ok(grants)
}

/// `principal:operation` lines for administrative authority.
pub(super) fn admin_grants_from(text: &str) -> Result<AdminGrants, SecurityError> {
    const VARIABLE: &str = "OQUEUE_ADMIN_GRANTS";
    let mut grants = AdminGrants::new();
    let mut empty = true;
    for (line, name, operation) in pairs(VARIABLE, "principal:operation", text)? {
        let operation = match operation {
            "create_topics" => AdminOperation::CreateTopics,
            "delete_topics" => AdminOperation::DeleteTopics,
            "describe_configs" => AdminOperation::DescribeConfigs,
            "alter_configs" => AdminOperation::AlterConfigs,
            "describe_groups" => AdminOperation::DescribeGroups,
            "list_groups" => AdminOperation::ListGroups,
            "alter_quotas" => AdminOperation::AlterQuotas,
            _ => {
                return Err(SecurityError::MalformedLine {
                    variable: VARIABLE,
                    line,
                    shape: "principal:operation",
                    reason: format!("unsupported administrative operation {operation:?}"),
                });
            }
        };
        grants.grant(
            Principal::new(name).map_err(|error| SecurityError::MalformedLine {
                variable: VARIABLE,
                line,
                shape: "principal:operation",
                reason: error.to_string(),
            })?,
            operation,
        );
        empty = false;
    }
    if empty {
        return Err(SecurityError::EmptySource {
            variable: VARIABLE,
            shape: "principal:operation pairs",
        });
    }
    Ok(grants)
}

/// `principal:group` lines for consumer-group ownership.
pub(super) fn group_grants_from(text: &str) -> Result<GroupGrants, SecurityError> {
    const VARIABLE: &str = "OQUEUE_GROUP_GRANTS";
    let mut grants = GroupGrants::new();
    let mut empty = true;
    for (line, name, group) in pairs(VARIABLE, "principal:group", text)? {
        let principal = Principal::new(name).map_err(|error| SecurityError::MalformedLine {
            variable: VARIABLE,
            line,
            shape: "principal:group",
            reason: error.to_string(),
        })?;
        let group = GroupId::new(group).map_err(|error| SecurityError::MalformedLine {
            variable: VARIABLE,
            line,
            shape: "principal:group",
            reason: error.to_string(),
        })?;
        grants.grant(principal, group);
        empty = false;
    }
    if empty {
        return Err(SecurityError::EmptySource {
            variable: VARIABLE,
            shape: "principal:group pairs",
        });
    }
    Ok(grants)
}

/// A positive `max_in_flight`.
///
/// ⚠️ **Zero is refused rather than meaning "unlimited".** A quota of zero
/// would admit nothing at all, and an operator who types it has made a
/// mistake in either reading; "unlimited" is what leaving the variable
/// unset already means.
pub(super) fn quota_from(value: &str) -> Result<Arc<PrincipalQuota>, SecurityError> {
    match value.trim().parse::<u32>() {
        Ok(max) if max > 0 => Ok(Arc::new(PrincipalQuota::new(max))),
        _ => Err(SecurityError::Quota {
            value: value.to_owned(),
        }),
    }
}
