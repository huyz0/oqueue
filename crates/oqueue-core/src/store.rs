//! The object-storage seam: the contract, and the streaming half `ADR-0037`
//! adds to it.
//!
//! ⚠️ **The fake lives in [`fake`]**, its own module since `M5.6` — the trait's
//! documented guarantees and the double that stands in for a backend are two
//! concepts, and this file reached `code-structure.md`'s 500-line limit holding
//! both.

use crate::{ByteRange, ObjectKey, ObjectMeta, Precondition, Result};
use std::future::Future;
use std::pin::Pin;

/// A boxed, `Send` future — the shape ADR-0002 gives every async seam.
///
/// ⚠️ Written out rather than reached for via a dependency: `async fn` in trait
/// is not `dyn`-compatible on this toolchain, and `bin/oqueue` must hold an
/// `Arc<dyn ObjectStore>` to choose a backend at startup (FR-50).
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Reads, writes and deletes objects.
///
/// ⚠️ **This is the only way anything in this workspace reaches object storage**
/// (NFR-51). It names no S3 or GCS type, and no vendor SDK appears in
/// `oqueue-core`; the backends live in `oqueue-store`, which `M1` writes.
///
/// ⚠️ **Carries no `list()`** — ADR-0009. Listing lives on a separate,
/// not-yet-built `MaintenanceStore` seam, so "never LIST on the read path"
/// (NFR-30) is a property nothing holding only this trait can violate.
///
/// # What an implementor must guarantee
///
/// See ADR-0005 and ADR-0009. In short: `put` is durable when its future
/// resolves `Ok`, a `get` of a key that was put returns exactly those bytes
/// (or the requested slice of them), and an object is never partially
/// visible.
///
/// ⚠️ Two that are easy to miss and are the reason the ADRs exist. **A `put`
/// that resolves `Err` leaves the key in an unknown state** — the object may
/// have landed and only the acknowledgement been lost, so a failed `put` is
/// not proof of absence. And **visibility is strong per key**: absent a later
/// `put`, a `get` after a successful `put` never returns `ObjectNotFound`.
/// ⚠️ [`FakeObjectStore`] cannot catch a violation of the *second* one — it is
/// strongly consistent by construction, and nothing in `M1.8`'s fault
/// injection changes that. It **can** now demonstrate the *first*: with
/// [`FaultConfig::crash_after_put_before_ack`] installed, a `put` durably
/// writes the object and still resolves `Err`, exactly the ambiguity this ADR
/// documents — see that field's own doc comment.
///
/// ADR-0005 is `docs/internal/product/decisions/0005-object-store-seam.md`,
/// ADR-0009 is `docs/internal/product/decisions/0009-object-store-error-home-and-list-placement.md`,
/// both in this repository.
pub trait ObjectStore: Send + Sync + core::fmt::Debug {
    /// Fetches an object, or a byte range of it.
    ///
    /// # Errors
    ///
    /// [`Error::ObjectNotFound`] if no object is stored under `key`.
    /// [`Error::ByteRangeOutOfBounds`] if `range` does not fit inside the
    /// object's actual size.
    fn get<'a>(&'a self, key: &'a ObjectKey, range: ByteRange) -> BoxFuture<'a, Result<Vec<u8>>>;

    /// Stores an object whole.
    ///
    /// With `precondition: None`, overwrites any previous value
    /// unconditionally — racing unconditioned writers are last-writer-wins.
    /// With `Some(precondition)`, the write only happens if `precondition`
    /// holds against the key's current state.
    ///
    /// Returns the [`ObjectMeta`] of what was written, for a later
    /// conditional write to present.
    ///
    /// # Errors
    ///
    /// [`Error::PreconditionFailed`] if `precondition` is `Some` and does not
    /// hold. Otherwise implementation-defined — the fake here does not fail
    /// unconditionally *by default*, but can be made to with a [`FaultConfig`]
    /// installed (`M1.8`): [`Error::SlowDown`]/[`Throttled`][Error::Throttled]/
    /// [`Transient`][Error::Transient] from a storm, or a durable write that
    /// still resolves [`Error::Transient`] from
    /// [`FaultConfig::crash_after_put_before_ack`].
    fn put<'a>(
        &'a self,
        key: &'a ObjectKey,
        payload: Vec<u8>,
        precondition: Option<Precondition>,
    ) -> BoxFuture<'a, Result<ObjectMeta>>;

    /// Deletes zero or more objects.
    ///
    /// ⚠️ **Idempotent, per key.** A key with no object under it is not an
    /// error — batch delete on S3 and GCS both treat "already absent" as
    /// success, and this seam matches that rather than surprising a caller
    /// who retries a partially-applied delete.
    ///
    /// # Errors
    ///
    /// Implementation-defined; the fake here does not fail *by default* —
    /// same storm-driven exception as [`ObjectStore::put`]'s, with a
    /// [`FaultConfig`] installed (`M1.8`).
    fn delete<'a>(&'a self, keys: &'a [ObjectKey]) -> BoxFuture<'a, Result<()>>;

    /// Opens a streaming write, for an object whose size is not known yet.
    ///
    /// ⚠️ **The seal is unconditional, and a unique key is what makes that
    /// safe** (`ADR-0037`). `object_store`'s public API cannot condition a
    /// `CompleteMultipartUpload` — `ADR-0013` traced why, and `M5.6`
    /// re-checked it against the pinned 0.14.1 — so a caller that needs a
    /// write not to overwrite anything names a key nothing else will ever
    /// name. Non-reusable object IDs are the GC safety inequality's own first
    /// enabler, so this is the property the system already owes rather than a
    /// new one.
    ///
    /// ⚠️ **A failed stream leaves an incomplete upload**, which this seam does
    /// not promise to clean up: a part that fails mid-object aborts what it
    /// can and an operator relies on a bucket lifecycle rule for the rest —
    /// the same trade `ADR-0013` point 1 records for the whole-payload path.
    ///
    /// # Errors
    ///
    /// Implementation-defined; whatever the backend says about opening a
    /// multipart upload.
    fn open_multipart<'a>(
        &'a self,
        key: &'a ObjectKey,
    ) -> BoxFuture<'a, Result<Box<dyn MultipartWriter<'a> + 'a>>>;
}

/// A streaming write in progress.
///
/// ⚠️ **Parts, not bytes.** Every backend this seam covers bounds a part's size
/// from below as well as above (S3: 5 MiB minimum except the last), so a writer
/// that accepted arbitrary byte runs would have to buffer to reach the minimum
/// — and where that buffer lives is the whole question a streaming writer
/// exists to answer. The caller decides what a part is;
/// [`BUNDLE_PART_BYTES`](crate::BUNDLE_PART_BYTES) is what this workspace's own
/// writer uses, chosen clear of that floor.
///
/// ⚠️ **An implementation checks its own limits and the seam exposes none.**
/// `MultipartLimits` is a backend's own numbers — part count, part size, object
/// size — and a caller cannot ask for them across this seam, deliberately: a
/// limit a caller could read is one it would branch on, and the branch belongs
/// where the numbers are. [`write_part`](MultipartWriter::write_part) returns
/// [`Error::TooManyParts`](crate::Error::TooManyParts),
/// [`PartTooLarge`](crate::Error::PartTooLarge) or
/// [`ObjectTooLarge`](crate::Error::ObjectTooLarge) before sending anything,
/// which is the same refusal the whole-payload path makes and at the same
/// point.
///
/// ⚠️ **`'store` is the store it writes to.** A writer cannot outlive the store
/// that opened it, which is what lets an in-memory fake hand back a writer over
/// its own state rather than an owned copy of it.
pub trait MultipartWriter<'store>: Send + core::fmt::Debug {
    /// Appends one part.
    ///
    /// # Errors
    ///
    /// Implementation-defined. On any error the upload is aborted where the
    /// backend allows it, and the writer must not be used again.
    fn write_part(&mut self, part: Vec<u8>) -> BoxFuture<'_, Result<()>>;

    /// Seals the object, unconditionally.
    ///
    /// # Errors
    ///
    /// Implementation-defined, plus [`Error::EmptyBundle`] for a stream with no
    /// parts — an object of no bytes costs a request and carries nothing.
    fn finish(self: Box<Self>) -> BoxFuture<'store, Result<ObjectMeta>>;
}

mod fake;

pub use fake::{FakeMultipartWriter, FakeObjectStore};
