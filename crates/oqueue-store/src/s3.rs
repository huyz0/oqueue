//! An [`ObjectStore`] backed by S3 (or an S3-compatible endpoint, e.g. `MinIO`).
//!
//! The pure decision logic that is genuinely S3-specific lives in `put.rs`
//! (`code-structure.md` rule 9); the decision logic shared with `gcs.rs` —
//! error classification, `get`'s range logic, and the size/precondition
//! decision behind multipart — lives at the crate root (`classify.rs`,
//! `get.rs`, `multipart.rs`). See each module's own doc comment.
//!
//! ⚠️ **The adapter bodies below are deliberate near-copies of `gcs.rs`'s** —
//! `get`, `delete`, `put_multipart`, `object_store_path`, `Debug` and the
//! test fixture differ only where `put_options_for` already isolates the
//! backends (`e_tag` vs `version`). M1's closing review left the copies
//! standing rather than demanding a generic adapter, which it judged may
//! cost more clarity than it buys (`M1.58`). ⚠️ The risk that keeps is
//! asymmetric: an edit here alone is exercised by the `MinIO` conformance
//! run, while the same edit to `gcs.rs` alone is exercised by nothing until
//! GCS live verification lands (`M15`) — so change the twin in the same
//! commit, by hand.

mod put;

use crate::classify::classify;
use crate::get::{
    disambiguate_failed_ranged_get, get_options_for, requested_range, truncated_range_error,
};
use crate::multipart::{PutStrategy, put_strategy_for};
use crate::retry::retry_config_for;
use put::{S3_MULTIPART_LIMITS, put_options_for};
// ⚠️ Both traits, unaliased-but-unnamed: `object_store::ObjectStore` (the
// base trait, for `get_opts`/`put_opts`/`put_multipart_opts`) and
// `object_store::ObjectStoreExt` (the blanket-implemented convenience trait,
// for `delete` — its other methods, like `GetResult::bytes` below, are
// inherent and need no trait import at all) must both be in scope for their
// methods to resolve via dot-syntax below, but this module implements
// `oqueue_core::ObjectStore` (imported below, unaliased) — so both of
// `object_store`'s are brought in `as _` rather than under a name that would
// collide with it.
use crate::tls::install_ring_provider;
use object_store::ObjectStore as _;
use object_store::ObjectStoreExt as _;
use object_store::PutMultipartOptions;
use object_store::aws::AmazonS3Builder;
use object_store::path::Path as ObjectStorePath;
use oqueue_core::{
    BoxFuture, ByteRange, Error, MultipartLimits, ObjectKey, ObjectMeta, ObjectStore, Precondition,
    PreconditionToken, Result, RetryPolicy,
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
    multipart_limits: MultipartLimits,
}

impl S3Store {
    /// Builds a store from `AWS_*` environment variables — nothing else.
    ///
    /// Multipart bounds default to real S3's own numbers. Override with
    /// [`S3Store::with_multipart_limits`] to lower the threshold this
    /// backend switches to multipart at (never to raise it past what S3
    /// itself allows) — the only reason to is a test that wants to exercise
    /// the multipart path without an actual multi-gigabyte payload,
    /// `s3_minio.rs`'s own reason.
    ///
    /// # Errors
    ///
    /// [`Error::Permanent`] if the environment does not describe a usable
    /// endpoint (missing bucket, unparsable region, and the like) — a
    /// configuration mistake retrying can never fix.
    pub fn from_env() -> Result<Self> {
        Self::from_env_with_retry(RetryPolicy::DEFAULT)
    }

    /// The same, with the retry policy chosen rather than defaulted.
    ///
    /// ⚠️ **Before the builder is built, because it has to be.** The vendor
    /// applies retry configuration when it constructs its client, so a
    /// `with_retry_policy` that ran afterwards would silently do nothing —
    /// which is why this is a constructor rather than the builder-style setter
    /// `with_multipart_limits` can afford to be.
    ///
    /// # Errors
    ///
    /// As [`from_env`](Self::from_env).
    pub fn from_env_with_retry(policy: RetryPolicy) -> Result<Self> {
        // ⚠️ Must run before the builder makes its first HTTPS connection —
        // see `crate::tls` and ADR-0012. Idempotent, so calling this once per
        // `S3Store` built in the same process is cheap and safe.
        install_ring_provider();
        let inner = AmazonS3Builder::from_env()
            // ⚠️ `ADR-0008`'s premise, discharged at `M3.13`: the vendor retry
            // is configured from this project's own `RetryPolicy` rather than
            // left at whatever the crate defaults to. See
            // `crate::retry_config_for` for why only one of the two retry
            // layers this sets.
            .with_retry(retry_config_for(policy))
            .build()
            .map_err(|_source| Error::Permanent)?;
        Ok(Self {
            inner,
            multipart_limits: S3_MULTIPART_LIMITS,
        })
    }

    /// Wraps a client the caller built.
    ///
    /// ⚠️ **A seam, not a test hook**, and the distinction is `ADR-0027`'s: a
    /// caller that has configured its own [`AmazonS3`](object_store::aws::AmazonS3)
    /// — a different credential source, a different endpoint, or a
    /// deterministic transport through
    /// [`with_http_connector`](object_store::aws::AmazonS3Builder::with_http_connector)
    /// — gets exactly the `put`/`get`/`delete` behaviour [`from_env`] gives,
    /// because it is the same code over a different client.
    ///
    /// ⚠️ **It carries no retry configuration**, which [`from_env_with_retry`]
    /// does: whatever the caller set on the builder is what this store uses,
    /// and `ADR-0008`'s premise is discharged by the caller rather than here.
    /// Multipart bounds start at the S3 defaults — see
    /// [`with_multipart_limits`].
    ///
    /// ⚠️ **The caller must build over HTTP, or install the crypto provider
    /// before building.** `ADR-0012` picks `ring`, and every constructor here
    /// calls `install_ring_provider` *before* the builder makes its first
    /// HTTPS connection — but this one is handed a client that is already
    /// built, so the call would be too late to matter. Review found the
    /// consequence by running it: an `AmazonS3Builder` with an `https://`
    /// endpoint **panics inside `build()`** with "No rustls crypto provider is
    /// configured", and the remedy it suggests is `aws_lc_rs`, which is the
    /// provider `ADR-0012` exists to avoid. A caller supplying its own
    /// transport — the case this constructor is for — never reaches rustls at
    /// all. ⚠️ **The provider installer is deliberately not public**: exposing
    /// it would be a second way to satisfy `ADR-0012`, and this doc is the
    /// first list of differences that is meant to be exhaustive.
    ///
    /// [`from_env`]: Self::from_env
    /// [`from_env_with_retry`]: Self::from_env_with_retry
    /// [`with_multipart_limits`]: Self::with_multipart_limits
    #[must_use]
    pub const fn from_client(inner: object_store::aws::AmazonS3) -> Self {
        Self {
            inner,
            multipart_limits: S3_MULTIPART_LIMITS,
        }
    }

    /// Overrides the multipart bounds `put` uses — see [`S3Store::from_env`].
    #[must_use]
    pub const fn with_multipart_limits(mut self, limits: MultipartLimits) -> Self {
        self.multipart_limits = limits;
        self
    }

    /// Test-only: what `put` would actually decide against a payload of
    /// `payload_len` bytes with no precondition, given this store's current
    /// `multipart_limits` — the only externally observable trace of
    /// `with_multipart_limits`'s effect that does not require a live
    /// network call.
    #[cfg(test)]
    fn strategy_for(&self, payload_len: usize) -> Result<PutStrategy> {
        put_strategy_for(payload_len, None, self.multipart_limits)
    }

    /// Uploads `payload` as a multipart object, `chunks` (from
    /// `put_strategy_for`) giving the slice boundaries in upload order.
    ///
    /// ⚠️ **Unconditional only** — see this module's doc comment and
    /// ADR-0013; `put_strategy_for` already refuses this path when a
    /// `Precondition` was given, so this method never receives one to drop.
    ///
    /// ⚠️ **Sequential, not concurrent**, deliberately: nothing in this
    /// project has yet measured a payload large enough to reach this path at
    /// all (`M1.20`'s 4 MiB-aligned chunks are far below
    /// `S3_MULTIPART_LIMITS::max_part_size`), so parallel part uploads would
    /// be an unmeasured optimization — `performance.md`'s own rule against
    /// optimizing ahead of a benchmark.
    async fn put_multipart(
        &self,
        key: &ObjectKey,
        path: &ObjectStorePath,
        payload: &[u8],
        chunks: Vec<Range<usize>>,
    ) -> Result<ObjectMeta> {
        let size = payload.len();
        let mut upload = self
            .inner
            .put_multipart_opts(path, PutMultipartOptions::default())
            .await
            .map_err(|source| classify(&source, key))?;
        for chunk in &chunks {
            if let Err(source) = upload
                .put_part(payload[chunk.clone()].to_vec().into())
                .await
            {
                // ⚠️ Best-effort cleanup, same trade `object_store`'s own
                // `copy_if_not_exists` multipart path makes for the same
                // reason (`aws/mod.rs`): a second failure here (a crash, a
                // dropped connection) is not this call's to guarantee against
                // — an operator relies on a bucket lifecycle rule for
                // orphaned-part cleanup, not this return path.
                let _ = upload.abort().await;
                return Err(classify(&source, key));
            }
        }
        match upload.complete().await {
            Ok(result) => {
                let e_tag = result.e_tag.ok_or(Error::Permanent)?;
                Ok(ObjectMeta {
                    size: u64::try_from(size).unwrap_or(u64::MAX),
                    precondition_token: PreconditionToken::new(e_tag),
                })
            }
            Err(source) => {
                let _ = upload.abort().await;
                Err(classify(&source, key))
            }
        }
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

impl ObjectStore for S3Store {
    fn get<'a>(&'a self, key: &'a ObjectKey, range: ByteRange) -> BoxFuture<'a, Result<Vec<u8>>> {
        Box::pin(async move {
            let path = object_store_path(key)?;
            let requested = requested_range(range, key)?;
            let options = get_options_for(requested.as_ref());
            let result = match self.inner.get_opts(&path, options).await {
                Ok(result) => result,
                Err(source) => {
                    // `M2.4`: a ranged request that failed as transient gets
                    // one `HEAD` to tell "can never satisfy" apart from a
                    // network blip — see `get.rs`'s
                    // `disambiguate_failed_ranged_get` for the measured
                    // reasoning, including why the 416 status itself is
                    // deliberately not inspected.
                    let classified = classify(&source, key);
                    if let (Some(requested), Error::Transient) = (&requested, &classified)
                        && let Some(err) =
                            disambiguate_failed_ranged_get(&self.inner, key, &path, requested).await
                    {
                        return Err(err);
                    }
                    return Err(classified);
                }
            };
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
            // Symmetric since `M2.4`: the start-past-size half is handled
            // on the error path above, so both halves of the range contract
            // now hold against a live backend, not only against the fake.
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
            match put_strategy_for(size, precondition.as_ref(), self.multipart_limits)? {
                PutStrategy::Single => {
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
                }
                PutStrategy::Multipart(chunks) => {
                    self.put_multipart(key, &path, &payload, chunks).await
                }
            }
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

    use super::{S3_MULTIPART_LIMITS, S3Store, install_ring_provider, object_store_path};
    use object_store::aws::AmazonS3Builder;
    use oqueue_core::{Error, ObjectKey};

    fn key() -> ObjectKey {
        ObjectKey::new("s3-store-test").expect("a non-empty key")
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
        install_ring_provider();
        let inner = AmazonS3Builder::new()
            .with_bucket_name("unused-in-this-test")
            .build()
            .expect("building against a bucket name alone needs no network");
        S3Store {
            inner,
            multipart_limits: S3_MULTIPART_LIMITS,
        }
    }

    #[test]
    fn debug_names_the_type_and_nothing_backend_specific() {
        let store = store_with_no_credentials();
        assert_eq!(format!("{store:?}"), "S3Store { .. }");
    }
}
