//! Classifies a raw `object_store::Error` into `oqueue-core`'s taxonomy.
//!
//! ⚠️ **Shared across every `object_store`-backed backend** (S3 `M1.15`, GCS
//! `M1.17`), not duplicated per backend: `object_store::Error` is the same
//! enum regardless of which client produced it, and both backends' `PutMode`
//! handling remaps a lost `PutMode::Create` race to `AlreadyExists` the same
//! way (confirmed against `object_store`'s `aws/client.rs` and
//! `gcp/client.rs`) — nothing in this classification is actually S3- or
//! GCS-specific, so a shared function is the accurate model, not premature
//! abstraction.

// ⚠️ `pub(crate)` on `classify` below is "redundant" only in the narrow sense
// this lint means: this module is private, so nothing outside the crate could
// reach it regardless. The qualifier still states the real, intended scope —
// visible to every backend this crate holds, not just one — and
// `clippy::redundant_pub_crate`'s own suggestion (bare `pub`) is exactly
// wrong here, for the same reason `tls.rs` already gives.
#![allow(clippy::redundant_pub_crate)]

use oqueue_core::{Error, ObjectKey};

/// Classifies a raw `object_store::Error` a backend actually received
/// against `key` into `oqueue-core`'s taxonomy.
///
/// ⚠️ **Exactly the four classes an `object_store`-backed backend can
/// produce** — `M1.4`'s dissolution note (`backlog.md` `M1.4`):
/// `SlowDown`/`Throttled`/`Transient` already have their first producer in
/// `M1.8`'s fault-injection storm, so no backend needs to invent a way to
/// reach `SlowDown`/`Throttled` specifically. `object_store`'s own
/// `RetryConfig` already retries 429/5xx internally with backoff before ever
/// handing back an `Err` (`object_store::client::retry`, not part of this
/// crate's dependency surface) — by the time this function runs, that budget
/// is spent, so a residual backend hiccup is `Transient` here, not
/// `SlowDown`/`Throttled`.
///
/// ⚠️ **No string-matching on `to_string()`.** `error-handling.md` rule 8
/// bans exactly that, and it would have been the only way to recover the
/// HTTP status behind an unrecognized `object_store::Error::Generic` —
/// `object_store` 0.14.1 keeps the type that carries it (`RetryError`) in a
/// `pub(crate)` module, unreachable from here even by name. Every
/// `Generic` this cannot otherwise classify becomes `Transient` instead: a
/// bounded number of retries at this crate's own layer is a reasonable
/// response to "some hiccup happened," and never retrying forever on what
/// could be a permanent failure this function failed to recognize would be
/// the worse default.
pub(crate) fn classify(err: &object_store::Error, key: &ObjectKey) -> Error {
    use object_store::Error as ObjErr;
    match err {
        ObjErr::NotFound { .. } => Error::ObjectNotFound { key: key.clone() },
        // `AlreadyExists`: `PutMode::Create` (our `Precondition::IfAbsent`)
        // losing its race — both `aws/mod.rs` and `gcp/client.rs` remap a
        // failed `Create` from `Precondition` to `AlreadyExists`, so R2, S3
        // and GCS all read the same way. `Precondition`: `PutMode::Update`
        // (our `Precondition::IfMatches`) losing its race, or a stale/absent
        // token. `NotModified`: unreachable through any call either backend
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
        // compile) — is a misconfiguration or an unsupported request no
        // backend's own retrying can fix.
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
    /// surfaces as `AlreadyExists`, not `Precondition` — confirmed for both
    /// S3 (`object_store`'s `aws/mod.rs`) and GCS (`gcp/client.rs`), so both
    /// backends' `classify` calls must land here the same way.
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
    /// back — is `Transient`, the bounded-retry class.
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
