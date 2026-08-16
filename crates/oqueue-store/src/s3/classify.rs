//! Classifies a raw `object_store::Error` into `oqueue-core`'s taxonomy.

// ⚠️ `pub(crate)` on `classify` below is "redundant" only in the narrow sense
// this lint means: `s3` and this module are both private, so nothing outside
// the crate could reach it regardless. The qualifier still states the real,
// intended scope — visible to every one of `s3`'s pieces, not just this file
// — and `clippy::redundant_pub_crate`'s own suggestion (bare `pub`) is
// exactly wrong here, for the same reason `tls.rs` already gives.
#![allow(clippy::redundant_pub_crate)]

use oqueue_core::{Error, ObjectKey};

/// Classifies a raw `object_store::Error` this backend actually received
/// against `key` into `oqueue-core`'s taxonomy.
///
/// ⚠️ **Exactly the four classes this backend can produce** — `M1.4`'s
/// dissolution note (`backlog.md` `M1.4`): `SlowDown`/`Throttled`/`Transient`
/// already have their first producer in `M1.8`'s fault-injection storm, so
/// this backend does not need to invent a way to reach `SlowDown`/`Throttled`
/// specifically. `object_store`'s own `RetryConfig` already retries
/// 429/5xx internally with backoff before ever handing us an `Err`
/// (`object_store::client::retry`, not part of this crate's dependency
/// surface) — by the time this function runs, that budget is spent, so a
/// residual backend hiccup is `Transient` here, not `SlowDown`/`Throttled`.
///
/// ⚠️ **No string-matching on `to_string()`.** `error-handling.md` rule 8
/// bans exactly that, and it would have been the only way to recover the
/// HTTP status behind an unrecognized `object_store::Error::Generic` —
/// `object_store` 0.14.1 keeps the type that carries it (`RetryError`) in a
/// `pub(crate)` module, unreachable from here even by name. Every
/// `Generic` this backend cannot otherwise classify becomes `Transient`
/// instead: a bounded number of retries at this crate's own layer is a
/// reasonable response to "some hiccup happened," and never retrying forever
/// on what could be a permanent failure this function failed to recognize
/// would be the worse default.
pub(crate) fn classify(err: &object_store::Error, key: &ObjectKey) -> Error {
    use object_store::Error as ObjErr;
    match err {
        ObjErr::NotFound { .. } => Error::ObjectNotFound { key: key.clone() },
        // `AlreadyExists`: `PutMode::Create` (our `Precondition::IfAbsent`)
        // losing its race. `Precondition`: `PutMode::Update` (our
        // `Precondition::IfMatches`) losing its race, or a stale/absent
        // e_tag. `NotModified`: unreachable through any call this backend
        // makes today (nothing here sets `if_none_match` on a `get`), kept
        // in this arm anyway since it is the same failure shape and
        // `error-handling.md` rule 6 is about inventing a producer, not
        // about declining to classify a shape the SDK itself can return.
        ObjErr::AlreadyExists { .. } | ObjErr::Precondition { .. } | ObjErr::NotModified { .. } => {
            Error::PreconditionFailed { key: key.clone() }
        }
        // `Generic` is where a retried-and-still-failing request lands
        // (`object_store::client::retry::RetryError::error`) — see this
        // function's own doc comment for why nothing more specific is
        // reachable from here.
        ObjErr::Generic { .. } => Error::Transient,
        // Everything else — `PermissionDenied`, `Unauthenticated`,
        // `InvalidPath`, `NotImplemented`, `UnknownConfigurationKey`,
        // `NotSupported`, and anything a future `object_store` release adds
        // under its `#[non_exhaustive]` (which forces this wildcard; an
        // exhaustive match over a foreign non-exhaustive enum does not
        // compile) — is a misconfiguration or an unsupported request this
        // backend's own retrying cannot fix.
        _ => Error::Permanent,
    }
}

#[cfg(test)]
mod tests {
    // Same justification as `tls.rs`'s: every `expect` below is on a value
    // this module just constructed from a literal it controls.
    #![allow(clippy::expect_used)]

    use super::classify;
    use object_store::Error as ObjErr;
    use oqueue_core::{Error, ObjectKey};

    fn key() -> ObjectKey {
        ObjectKey::new("classify-test").expect("a non-empty key")
    }

    fn boxed(msg: &str) -> Box<dyn std::error::Error + Send + Sync> {
        msg.into()
    }

    /// ⚠️ **This is the unit test `backlog.md`'s `M1.15` row asks for**: one
    /// `object_store`-shaped raw error per class this backend can actually
    /// receive, each asserted against the classified variant — not a shape
    /// invented ahead of a real producer (`error-handling.md` rule 6).

    #[test]
    fn not_found_becomes_object_not_found() {
        let k = key();
        let raw = ObjErr::NotFound {
            path: k.as_str().to_owned(),
            source: boxed("missing"),
        };
        assert_eq!(classify(&raw, &k), Error::ObjectNotFound { key: k.clone() });
    }

    #[test]
    fn precondition_becomes_precondition_failed() {
        let k = key();
        let raw = ObjErr::Precondition {
            path: k.as_str().to_owned(),
            source: boxed("cas lost"),
        };
        assert_eq!(classify(&raw, &k), Error::PreconditionFailed { key: k });
    }

    /// `PutMode::Create` (our `Precondition::IfAbsent`) losing its race
    /// surfaces as `AlreadyExists`, not `Precondition` — `object_store`'s own
    /// `put_opts` remaps it (`object_store`'s `aws/mod.rs`) so R2 and real S3
    /// read the same way. This backend must classify both the same.
    #[test]
    fn already_exists_becomes_precondition_failed() {
        let k = key();
        let raw = ObjErr::AlreadyExists {
            path: k.as_str().to_owned(),
            source: boxed("exists"),
        };
        assert_eq!(classify(&raw, &k), Error::PreconditionFailed { key: k });
    }

    #[test]
    fn not_modified_becomes_precondition_failed() {
        let k = key();
        let raw = ObjErr::NotModified {
            path: k.as_str().to_owned(),
            source: boxed("not modified"),
        };
        assert_eq!(classify(&raw, &k), Error::PreconditionFailed { key: k });
    }

    /// A retried-and-still-failing request — `object_store`'s own
    /// `RetryConfig` already spent its backoff budget before handing this
    /// back — is `Transient`, this backend's own bounded-retry class.
    #[test]
    fn generic_becomes_transient() {
        let k = key();
        let raw = ObjErr::Generic {
            store: "S3",
            source: boxed("network blip"),
        };
        assert_eq!(classify(&raw, &k), Error::Transient);
    }

    /// Every shape this classifier does not name explicitly — credentials,
    /// unsupported operations, config, and anything a future `object_store`
    /// adds under its `#[non_exhaustive]` — is `Permanent`: retrying a
    /// backend's own configuration mistake never helps.
    #[test]
    fn unrecognized_shapes_become_permanent() {
        let k = key();
        let raw = ObjErr::PermissionDenied {
            path: k.as_str().to_owned(),
            source: boxed("denied"),
        };
        assert_eq!(classify(&raw, &k), Error::Permanent);
    }
}
