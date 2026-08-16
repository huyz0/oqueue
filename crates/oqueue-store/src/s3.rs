//! An [`ObjectStore`] backed by S3 (or an S3-compatible endpoint, e.g. `MinIO`).

use crate::tls::install_ring_provider;
// ⚠️ Both traits, unaliased-but-unnamed: `object_store::ObjectStore` (the
// base trait, for `get_opts`/`put_opts`) and `object_store::ObjectStoreExt`
// (the blanket-implemented convenience trait, for `delete`/`bytes`) must
// both be in scope for their methods to resolve via dot-syntax below, but
// this module implements `oqueue_core::ObjectStore` (imported below,
// unaliased) — so both of `object_store`'s are brought in `as _` rather than
// under a name that would collide with it.
use object_store::ObjectStore as _;
use object_store::ObjectStoreExt as _;
use object_store::aws::AmazonS3Builder;
use object_store::path::Path as ObjectStorePath;
use object_store::{GetOptions, GetRange, PutMode, PutOptions, UpdateVersion};
use oqueue_core::{
    BoxFuture, ByteRange, Error, ObjectKey, ObjectMeta, ObjectStore, Precondition,
    PreconditionToken, Result,
};
use std::ops::Range;

/// An [`ObjectStore`] talking to S3 (or an S3-compatible endpoint) via the
/// crate ADR-0008 chose.
///
/// ⚠️ **Credentials, region, bucket and endpoint all come from `AWS_*`
/// environment variables** (`AmazonS3Builder::from_env`), never a literal in
/// this crate — `security.md`. The same mechanism reads `AWS_ENDPOINT` /
/// `AWS_ALLOW_HTTP`, which is how a test points this at a local `MinIO`
/// container instead of real S3.
pub struct S3Store {
    inner: object_store::aws::AmazonS3,
}

impl S3Store {
    /// Builds a store from `AWS_*` environment variables — nothing else.
    ///
    /// # Errors
    ///
    /// [`Error::Permanent`] if the environment does not describe a usable
    /// endpoint (missing bucket, unparsable region, and the like) — a
    /// configuration mistake retrying can never fix.
    pub fn from_env() -> Result<Self> {
        // ⚠️ Must run before the builder makes its first HTTPS connection —
        // see `crate::tls` and ADR-0012. Idempotent, so calling this once per
        // `S3Store` built in the same process is cheap and safe.
        install_ring_provider();
        let inner = AmazonS3Builder::from_env()
            .build()
            .map_err(|_source| Error::Permanent)?;
        Ok(Self { inner })
    }
}

/// ⚠️ Prints nothing backend-specific — a `Debug` derive on the vendor
/// client would risk dumping request-signing state; this crate has no reason
/// to know what that state is, so it does not try to render it.
impl core::fmt::Debug for S3Store {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("S3Store").finish_non_exhaustive()
    }
}

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
fn classify(err: &object_store::Error, key: &ObjectKey) -> Error {
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

/// Converts an [`ObjectKey`] into the `Path` `object_store` addresses by.
///
/// # Errors
///
/// [`Error::Permanent`] if the key is not a valid `object_store` path (an
/// empty segment from a doubled `/`, for instance) — `oqueue-core` imposes no
/// backend-specific character limit on a key (`ObjectKey`'s own doc comment),
/// so this is the first point anything actually validates against S3's.
fn object_store_path(key: &ObjectKey) -> Result<ObjectStorePath> {
    ObjectStorePath::parse(key.as_str()).map_err(|_source| Error::Permanent)
}

/// The `GetOptions` a `get_opts` call sends for `requested` — `None` for
/// [`ByteRange::Full`], `Some` for [`ByteRange::Bounded`].
///
/// ⚠️ **Pulled out of `get` on its own**, not merely for readability: the
/// rest of `get` only runs against a live backend, which this crate's own
/// mutation-testing budget cannot afford per mutant (`scripts/mutants.sh`'s
/// own header) — a struct-literal field silently going missing there is
/// only observable over the network, `s3_minio.rs`'s job, not this gate's.
/// A pure function with the same struct literal keeps the mutation site
/// reachable by a T0 unit test instead.
fn get_options_for(requested: Option<&Range<u64>>) -> GetOptions {
    GetOptions {
        range: requested.cloned().map(GetRange::Bounded),
        ..GetOptions::default()
    }
}

/// If `actual` (what `object_store` reports it actually returned) is not
/// exactly `requested`, the [`Error::ByteRangeOutOfBounds`] this backend
/// raises instead of silently accepting a truncated read — see `get`'s own
/// doc comment for why `object_store`'s own, more lenient contract is not
/// enough here. `None` if `actual` matches `requested` exactly.
fn truncated_range_error(
    key: &ObjectKey,
    requested: &Range<u64>,
    actual: &Range<u64>,
    object_size: u64,
) -> Option<Error> {
    if actual == requested {
        return None;
    }
    Some(Error::ByteRangeOutOfBounds {
        key: key.clone(),
        offset: requested.start,
        length: requested.end - requested.start,
        object_size,
    })
}

/// The `PutOptions` a `put_opts` call sends for `precondition`.
///
/// ⚠️ Pulled out for the same reason `get_options_for` is: the field it
/// assembles is only observable against a live backend, which
/// `s3_minio.rs`'s `conditional_write_*` cases already exercise — this
/// function exists so the assembly itself is also reachable by a T0 unit
/// test, independent of that network round trip.
fn put_options_for(precondition: Option<&Precondition>) -> PutOptions {
    let mode = match precondition {
        None => PutMode::Overwrite,
        Some(Precondition::IfAbsent) => PutMode::Create,
        Some(Precondition::IfMatches(token)) => PutMode::Update(UpdateVersion {
            e_tag: Some(token.as_str().to_owned()),
            version: None,
        }),
    };
    PutOptions {
        mode,
        ..PutOptions::default()
    }
}

impl ObjectStore for S3Store {
    fn get<'a>(&'a self, key: &'a ObjectKey, range: ByteRange) -> BoxFuture<'a, Result<Vec<u8>>> {
        Box::pin(async move {
            let path = object_store_path(key)?;
            let requested = match range {
                ByteRange::Full => None,
                ByteRange::Bounded(bounded) => {
                    let start = bounded.offset();
                    let end = start.checked_add(bounded.length()).ok_or_else(|| {
                        Error::ByteRangeOutOfBounds {
                            key: key.clone(),
                            offset: bounded.offset(),
                            length: bounded.length(),
                            object_size: u64::MAX,
                        }
                    })?;
                    Some(start..end)
                }
            };
            let options = get_options_for(requested.as_ref());
            let result = self
                .inner
                .get_opts(&path, options)
                .await
                .map_err(|source| classify(&source, key))?;
            let object_size = result.meta.size;
            // ⚠️ **Fidelity to `FakeObjectStore`'s strict contract, at no
            // extra round trip.** `object_store`'s own `GetRange::Bounded`
            // doc comment says a range whose *end* runs past the object's
            // actual size is silently truncated to what exists, not an
            // error — `oqueue-core`'s `ByteRange` contract (`slice_range`,
            // `M1.3`) is stricter: any requested end past the true size is
            // `Error::ByteRangeOutOfBounds`. `GetResult::range` already
            // reports what was actually returned in the same response this
            // `get_opts` call already paid for, so checking it here costs
            // nothing extra and closes that gap for the truncation case.
            if let Some(requested) = &requested
                && let Some(err) = truncated_range_error(key, requested, &result.range, object_size)
            {
                return Err(err);
            }
            // ⚠️ **Not fully symmetric.** A range whose *start* is at or past
            // the object's actual size is rejected by `object_store` itself
            // before this function ever sees a `GetResult` to inspect — it
            // surfaces as the `Generic` arm of `classify`, i.e.
            // `Error::Transient`, not `Error::ByteRangeOutOfBounds`. Closing
            // that gap needs either a `HEAD` before every ranged `get` (an
            // extra round trip on every call, for a genuinely rare shape) or
            // reaching into `object_store`'s own `pub(crate)` error internals
            // (which `error-handling.md` rule 8 already rules out) — neither
            // is `M1.15`'s to spend on "core ops."
            let bytes = result
                .bytes()
                .await
                .map_err(|source| classify(&source, key))?;
            Ok(bytes.to_vec())
        })
    }

    fn put<'a>(
        &'a self,
        key: &'a ObjectKey,
        payload: Vec<u8>,
        precondition: Option<Precondition>,
    ) -> BoxFuture<'a, Result<ObjectMeta>> {
        Box::pin(async move {
            let path = object_store_path(key)?;
            let size = payload.len();
            let options = put_options_for(precondition.as_ref());
            let result = self
                .inner
                .put_opts(&path, payload.into(), options)
                .await
                .map_err(|source| classify(&source, key))?;
            let e_tag = result.e_tag.ok_or(Error::Permanent)?;
            Ok(ObjectMeta {
                size: u64::try_from(size).unwrap_or(u64::MAX),
                precondition_token: PreconditionToken::new(e_tag),
            })
        })
    }

    fn delete<'a>(&'a self, keys: &'a [ObjectKey]) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            for key in keys {
                let path = object_store_path(key)?;
                match self.inner.delete(&path).await {
                    // ⚠️ **Idempotent, per the trait's own contract** — same
                    // as `FakeObjectStore::delete`. Real S3 already returns
                    // success for deleting an absent key, so this arm is
                    // defensive rather than load-bearing against S3 itself,
                    // but keeps an S3-compatible endpoint that does surface
                    // `NotFound` (some are stricter) from violating the
                    // contract this backend promises.
                    Err(object_store::Error::NotFound { .. }) | Ok(()) => {}
                    Err(source) => return Err(classify(&source, key)),
                }
            }
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    // Same justification as `tls.rs`'s: every `expect` below is on a value
    // this module just constructed from a literal it controls.
    #![allow(clippy::expect_used)]

    use super::{
        S3Store, classify, get_options_for, object_store_path, put_options_for,
        truncated_range_error,
    };
    use object_store::aws::AmazonS3Builder;
    use object_store::{Error as ObjErr, GetRange, PutMode, UpdateVersion};
    use oqueue_core::{Error, ObjectKey, Precondition, PreconditionToken};

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

    #[test]
    fn a_malformed_key_is_rejected_before_any_request() {
        // A doubled separator is an empty path segment, which
        // `object_store::path::Path::parse` rejects outright.
        let malformed = ObjectKey::new("a//b").expect("a non-empty key");
        assert_eq!(object_store_path(&malformed), Err(Error::Permanent));
    }

    #[test]
    fn a_well_formed_key_converts_to_the_same_path() {
        let k = key();
        let path = object_store_path(&k).expect("a well-formed key converts");
        assert_eq!(path.as_ref(), k.as_str());
    }

    /// `AmazonS3Builder::build` does no I/O — it only validates local
    /// config — so a store built from nothing but a bucket name is enough
    /// to exercise `Debug` without touching the network or a literal
    /// credential (`security.md`: never a literal, even a fake one, in a
    /// test fixture).
    fn store_with_no_credentials() -> S3Store {
        // `build()` constructs its HTTP client eagerly (found running this
        // very test — it is not the pure config validation its doc comment
        // implies), so it needs the same provider install every real
        // constructor does.
        super::install_ring_provider();
        let inner = AmazonS3Builder::new()
            .with_bucket_name("unused-in-this-test")
            .build()
            .expect("building against a bucket name alone needs no network");
        S3Store { inner }
    }

    #[test]
    fn debug_names_the_type_and_nothing_backend_specific() {
        let store = store_with_no_credentials();
        assert_eq!(format!("{store:?}"), "S3Store { .. }");
    }

    #[test]
    fn get_options_for_a_bounded_range_carries_it() {
        let requested = 2..5;
        assert_eq!(
            get_options_for(Some(&requested)).range,
            Some(GetRange::Bounded(requested))
        );
    }

    #[test]
    fn get_options_for_a_full_read_has_no_range() {
        assert_eq!(get_options_for(None).range, None);
    }

    #[test]
    fn a_range_matching_what_was_requested_is_not_an_error() {
        let k = key();
        assert_eq!(truncated_range_error(&k, &(2..10), &(2..10), 20), None);
    }

    #[test]
    fn a_truncated_range_becomes_out_of_bounds() {
        let k = key();
        let err = truncated_range_error(&k, &(2..10), &(2..7), 7)
            .expect("a range object_store did not fully honour is reported");
        assert_eq!(
            err,
            Error::ByteRangeOutOfBounds {
                key: k,
                offset: 2,
                length: 8,
                object_size: 7,
            }
        );
    }

    #[test]
    fn put_options_for_no_precondition_overwrites() {
        assert_eq!(put_options_for(None).mode, PutMode::Overwrite);
    }

    #[test]
    fn put_options_for_if_absent_creates() {
        assert_eq!(
            put_options_for(Some(&Precondition::IfAbsent)).mode,
            PutMode::Create
        );
    }

    #[test]
    fn put_options_for_if_matches_updates_with_the_tokens_etag() {
        let precondition = Precondition::IfMatches(PreconditionToken::new("etag-123"));
        assert_eq!(
            put_options_for(Some(&precondition)).mode,
            PutMode::Update(UpdateVersion {
                e_tag: Some("etag-123".to_owned()),
                version: None,
            })
        );
    }
}
