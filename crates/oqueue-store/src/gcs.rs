//! An [`ObjectStore`] backed by GCS.
//!
//! ⚠️ **No compatible emulator yet, unlike `S3Store`'s `MinIO`.** ADR-0014:
//! neither `fake-gcs-server` nor Google's own `storage-testbench` round-trips
//! `object_store`'s actual GCS requests out of the box, so this backend is
//! verified at T0 only for now — see that ADR before assuming a container
//! this crate's own tests could point `GOOGLE_BASE_URL` at.
//!
//! The pure decision logic that is genuinely GCS-specific lives in `put.rs`
//! (`code-structure.md` rule 9); the decision logic shared with `s3.rs` —
//! error classification, `get`'s range logic, and the size/precondition
//! decision behind multipart — lives at the crate root (`classify.rs`,
//! `get.rs`, `multipart.rs`). See each module's own doc comment, and
//! `s3.rs`'s own module doc for why the two backends are this similar in
//! shape: both are `object_store` adapters over the same generic
//! `ObjectStore` trait.
//!
//! ⚠️ **The adapter bodies below are deliberate near-copies of `s3.rs`'s**
//! (`M1.58` — see the matching warning there). The asymmetry lands on this
//! file: an edit to `s3.rs` alone is exercised by the `MinIO` conformance run,
//! while an edit here alone is exercised by nothing until GCS live
//! verification lands (`M15`) — so change the twin in the same commit, by
//! hand.

mod put;

use crate::classify::classify;
use crate::get::{
    disambiguate_failed_ranged_get, get_options_for, requested_range, truncated_range_error,
};
use crate::multipart::{PutStrategy, put_strategy_for};
use crate::retry::retry_config_for;
use put::{GCS_MULTIPART_LIMITS, put_options_for};
// Same reasoning `s3.rs`'s matching comment gives for its own trait imports.
use crate::tls::install_ring_provider;
use object_store::ObjectStore as _;
use object_store::ObjectStoreExt as _;
use object_store::PutMultipartOptions;
use object_store::gcp::GoogleCloudStorageBuilder;
use object_store::path::Path as ObjectStorePath;
use oqueue_core::{
    BoxFuture, ByteRange, Error, MultipartLimits, MultipartSession, ObjectKey, ObjectMeta,
    ObjectStore, Precondition, PreconditionToken, Result, RetryPolicy,
};
use std::ops::Range;

/// An [`ObjectStore`] talking to GCS via the crate ADR-0008 chose.
///
/// ⚠️ **Credentials, bucket and endpoint all come from `GOOGLE_*`
/// environment variables** (`GoogleCloudStorageBuilder::from_env`), never a
/// literal in this crate — `security.md`. The same mechanism reads
/// `GOOGLE_BASE_URL`, which would be how a test points this at a local
/// emulator instead of real GCS once one exists that this crate's requests
/// actually work against — ADR-0014, not yet true of either emulator tried.
/// `GoogleCloudStorageBuilder` already defaults to allowing plain HTTP
/// (`object_store`'s own default, not something this crate has to ask for),
/// which every real GCS endpoint still rejects since it only ever answers on
/// HTTPS.
pub struct GcsStore {
    inner: object_store::gcp::GoogleCloudStorage,
    multipart_limits: MultipartLimits,
}

impl GcsStore {
    /// Builds a store from `GOOGLE_*` environment variables — nothing else.
    ///
    /// Multipart bounds default to real GCS's own numbers. Override with
    /// [`GcsStore::with_multipart_limits`] to lower the threshold this
    /// backend switches to multipart at — the only reason to is a test that
    /// wants to exercise the multipart path without an actual large
    /// payload, same reason `S3Store::with_multipart_limits`'s doc comment
    /// gives (`s3_minio.rs`'s own T2 test uses it that way; this backend has
    /// no equivalent live test yet — ADR-0014).
    ///
    /// # Errors
    ///
    /// [`Error::Permanent`] if the environment does not describe a usable
    /// endpoint (missing bucket, malformed credentials, and the like) — a
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
        // `GcsStore` built in the same process is cheap and safe.
        install_ring_provider();
        let inner = GoogleCloudStorageBuilder::from_env()
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
            multipart_limits: GCS_MULTIPART_LIMITS,
        })
    }

    /// Overrides the multipart bounds `put` uses — see [`GcsStore::from_env`].
    #[must_use]
    pub const fn with_multipart_limits(mut self, limits: MultipartLimits) -> Self {
        self.multipart_limits = limits;
        self
    }

    /// Test-only: what `put` would actually decide against a payload of
    /// `payload_len` bytes with no precondition, given this store's current
    /// `multipart_limits` — same reasoning as `S3Store`'s matching method.
    #[cfg(test)]
    fn strategy_for(&self, payload_len: usize) -> Result<PutStrategy> {
        put_strategy_for(payload_len, None, self.multipart_limits)
    }

    /// Uploads `payload` as a multipart (resumable) object, `chunks` (from
    /// `put_strategy_for`) giving the slice boundaries in upload order.
    ///
    /// ⚠️ **Unconditional only** — see this module's doc comment and
    /// ADR-0013 (written against S3, confirmed to apply here too:
    /// `object_store`'s `GoogleCloudStorage`'s own `MultipartStore::
    /// complete_multipart` takes no precondition parameter either,
    /// `gcp/mod.rs`); `put_strategy_for` already refuses this path when a
    /// `Precondition` was given, so this method never receives one to drop.
    ///
    /// ⚠️ **Sequential, not concurrent** — same reasoning `S3Store`'s
    /// matching method gives; nothing here has a measured payload large
    /// enough to justify parallel part uploads.
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
                // ⚠️ Best-effort cleanup — same trade `S3Store::put_multipart`
                // makes, for the same reason.
                let _ = upload.abort().await;
                return Err(classify(&source, key));
            }
        }
        match upload.complete().await {
            Ok(result) => {
                // ⚠️ **`version`, not `e_tag`** — the opposite of
                // `S3Store::put`'s single-shot path, and for the same reason
                // `put.rs::put_options_for`'s doc comment gives: GCS's own
                // conditional-write encoding reads back through
                // `UpdateVersion::version`, so the token this backend hands
                // a later conditional write must be built from the same
                // field or a round-trip through `Precondition::IfMatches`
                // would present a value GCS never agreed to compare against.
                let generation = result.version.ok_or(Error::Permanent)?;
                Ok(ObjectMeta {
                    size: u64::try_from(size).unwrap_or(u64::MAX),
                    precondition_token: PreconditionToken::new(generation),
                })
            }
            Err(source) => {
                let _ = upload.abort().await;
                Err(classify(&source, key))
            }
        }
    }
}

/// ⚠️ Prints nothing backend-specific — same reasoning `S3Store`'s `Debug`
/// impl gives.
impl core::fmt::Debug for GcsStore {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("GcsStore").finish_non_exhaustive()
    }
}

/// Converts an [`ObjectKey`] into the `Path` `object_store` addresses by —
/// same reasoning and same failure mode as `s3.rs`'s matching function.
///
/// # Errors
///
/// [`Error::Permanent`] if the key is not a valid `object_store` path.
fn object_store_path(key: &ObjectKey) -> Result<ObjectStorePath> {
    ObjectStorePath::parse(key.as_str()).map_err(|_source| Error::Permanent)
}

/// A streaming write against GCS, over `object_store`'s own multipart
/// upload.
///
/// ⚠️ **Unconditional at the seal** (`ADR-0037`): `object_store`'s public API
/// carries no `PutMode` to `CompleteMultipartUpload`, and the caller's answer
/// is a key nothing else will name rather than a precondition here.
struct StreamingWriter<'store> {
    key: &'store ObjectKey,
    upload: Box<dyn object_store::MultipartUpload>,
    /// ⚠️ **The same validator the whole-payload path uses.** A part over the
    /// backend's maximum, or one part too many, is refused here rather than at
    /// `complete()` — after every part has been uploaded and paid for.
    session: MultipartSession,
    bytes: u64,
    /// ⚠️ **A failed part poisons the writer**, because the upload it belonged
    /// to has already been aborted: a retried part would be sent against an
    /// upload id the backend has forgotten, and the error it came back with
    /// would describe that rather than the original failure.
    poisoned: bool,
}

/// ⚠️ Prints nothing the vendor client holds, for `S3Store`'s own reason.
impl core::fmt::Debug for StreamingWriter<'_> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("StreamingWriter")
            .field("bytes", &self.bytes)
            .finish_non_exhaustive()
    }
}

impl<'store> oqueue_core::MultipartWriter<'store> for StreamingWriter<'store> {
    fn write_part(&mut self, part: Vec<u8>) -> BoxFuture<'_, Result<()>> {
        Box::pin(async move {
            if self.poisoned {
                return Err(Error::Permanent);
            }
            let len = u64::try_from(part.len()).unwrap_or(u64::MAX);
            self.session.add_part(len)?;
            match self.upload.put_part(part.into()).await {
                Ok(()) => {
                    self.bytes = self.bytes.saturating_add(len);
                    Ok(())
                }
                Err(source) => {
                    // ⚠️ Best-effort abort, the same trade the whole-payload
                    // path records: a second failure here is a bucket
                    // lifecycle rule's problem, not this return path's.
                    let _ = self.upload.abort().await;
                    self.poisoned = true;
                    Err(classify(&source, self.key))
                }
            }
        })
    }

    fn finish(mut self: Box<Self>) -> BoxFuture<'store, Result<ObjectMeta>> {
        Box::pin(async move {
            if self.poisoned {
                return Err(Error::Permanent);
            }
            if self.bytes == 0 {
                let _ = self.upload.abort().await;
                return Err(Error::EmptyBundle);
            }
            match self.upload.complete().await {
                Ok(result) => {
                    let e_tag = result.e_tag.ok_or(Error::Permanent)?;
                    Ok(ObjectMeta {
                        size: self.bytes,
                        precondition_token: PreconditionToken::new(e_tag),
                    })
                }
                Err(source) => {
                    let _ = self.upload.abort().await;
                    Err(classify(&source, self.key))
                }
            }
        })
    }
}

impl ObjectStore for GcsStore {
    fn get<'a>(&'a self, key: &'a ObjectKey, range: ByteRange) -> BoxFuture<'a, Result<Vec<u8>>> {
        Box::pin(async move {
            let path = object_store_path(key)?;
            let requested = requested_range(range, key)?;
            let options = get_options_for(requested.as_ref());
            let result = match self.inner.get_opts(&path, options).await {
                Ok(result) => result,
                Err(source) => {
                    // `M2.4` — same reasoning as `s3.rs`'s matching branch
                    // and `get.rs`'s `disambiguate_failed_ranged_get`: one
                    // `HEAD` after a transient ranged failure tells "can
                    // never satisfy" apart from a network blip.
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
            // ⚠️ Same fidelity gap and the same fix — see `s3.rs`'s
            // matching comment on `get`, which this reasoning is identical
            // to: `GetRange::Bounded` is `object_store`'s own generic type,
            // not something either backend's client reinterprets.
            if let Some(requested) = &requested
                && let Some(err) = truncated_range_error(key, requested, &result.range, object_size)
            {
                return Err(err);
            }
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
                    // `version`, not `e_tag` — see `put_multipart`'s own
                    // comment on the same choice.
                    let generation = result.version.ok_or(Error::Permanent)?;
                    Ok(ObjectMeta {
                        size: u64::try_from(size).unwrap_or(u64::MAX),
                        precondition_token: PreconditionToken::new(generation),
                    })
                }
                PutStrategy::Multipart(chunks) => {
                    self.put_multipart(key, &path, &payload, chunks).await
                }
            }
        })
    }

    fn open_multipart<'a>(
        &'a self,
        key: &'a ObjectKey,
    ) -> BoxFuture<'a, Result<Box<dyn oqueue_core::MultipartWriter<'a> + 'a>>> {
        Box::pin(async move {
            let path = object_store_path(key)?;
            let upload = self
                .inner
                .put_multipart_opts(&path, PutMultipartOptions::default())
                .await
                .map_err(|source| classify(&source, key))?;
            let writer: Box<dyn oqueue_core::MultipartWriter<'a> + 'a> =
                Box::new(StreamingWriter {
                    key,
                    upload,
                    session: MultipartSession::new(GCS_MULTIPART_LIMITS),
                    bytes: 0,
                    poisoned: false,
                });
            Ok(writer)
        })
    }

    fn delete<'a>(&'a self, keys: &'a [ObjectKey]) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            for key in keys {
                let path = object_store_path(key)?;
                match self.inner.delete(&path).await {
                    // ⚠️ Idempotent, per the trait's own contract — same
                    // reasoning `S3Store::delete`'s matching arm gives.
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

    use super::{GCS_MULTIPART_LIMITS, GcsStore, install_ring_provider, object_store_path};
    use object_store::gcp::GoogleCloudStorageBuilder;
    use oqueue_core::{Error, ObjectKey};

    fn key() -> ObjectKey {
        ObjectKey::new("gcs-store-test").expect("a non-empty key")
    }

    #[test]
    fn a_malformed_key_is_rejected_before_any_request() {
        let malformed = ObjectKey::new("a//b").expect("a non-empty key");
        assert_eq!(object_store_path(&malformed), Err(Error::Permanent));
    }

    #[test]
    fn a_well_formed_key_converts_to_the_same_path() {
        let k = key();
        let path = object_store_path(&k).expect("a well-formed key converts");
        assert_eq!(path.as_ref(), k.as_str());
    }

    /// `GoogleCloudStorageBuilder::build` does no I/O for a plain bucket
    /// name plus no credentials (`gcs::put`'s matching test explains why),
    /// so this needs no network and no literal credential (`security.md`).
    fn store_with_no_credentials() -> GcsStore {
        install_ring_provider();
        let inner = GoogleCloudStorageBuilder::new()
            .with_bucket_name("unused-in-this-test")
            .build()
            .expect("building against a bucket name alone needs no network");
        GcsStore {
            inner,
            multipart_limits: GCS_MULTIPART_LIMITS,
        }
    }

    #[test]
    fn debug_names_the_type_and_nothing_backend_specific() {
        let store = store_with_no_credentials();
        assert_eq!(format!("{store:?}"), "GcsStore { .. }");
    }
}
