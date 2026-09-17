//! Op-class accounting: cost as a testable property of the seam, not a
//! review convention.

use crate::{ByteRange, ObjectKey, ObjectMeta, ObjectStore, Precondition, Result};
use core::sync::atomic::{AtomicU64, Ordering};

/// One kind of call through [`ObjectStore`].
///
/// ⚠️ **Exactly the trait's three methods, no more.** `list()` is
/// permanently absent — ADR-0009 keeps it off `ObjectStore` entirely, so
/// there is no call to count.
///
/// ⚠️ **No multipart-specific variant, even though `M1.16` now issues
/// `UploadPart`-shaped calls.** This comment used to defer one to exactly
/// that commit, on the assumption that adding a variant would be enough —
/// found wrong writing `M1.16`: [`CountingObjectStore`] is a decorator around
/// the *trait*, composing with any implementor (the fake, S3, later GCS)
/// alike, and from that vantage point a multipart `put` is still one
/// [`ObjectStore::put`] call — the same shape whether the backend sent one
/// `PutObject` or a `CreateMultipartUpload` plus N `UploadPart`s plus a
/// `CompleteMultipartUpload`. Counting the true per-request cost needs
/// either a backend reporting it back through the trait (a contract change —
/// `contracts.md` rule 12, non-negotiable 6) or accounting living inside
/// each backend instead of as a generic decorator; either is a bigger design
/// question than "add a variant" and belongs to `M14`'s API-cost model
/// (NFR-31), the milestone that already owns turning this into a bound —
/// `roadmap.md`'s deferred-into-a-later-milestone table, `M14.md` task 5.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Operation {
    /// [`ObjectStore::get`].
    Get,
    /// [`ObjectStore::put`].
    Put,
    /// [`ObjectStore::delete`].
    Delete,
}

/// How many times each [`Operation`] has been called.
#[derive(Debug, Default)]
pub struct OpCounts {
    get: AtomicU64,
    put: AtomicU64,
    delete: AtomicU64,
}

impl OpCounts {
    /// An empty set of counters.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many times `op` has been called.
    #[must_use]
    pub fn count(&self, op: Operation) -> u64 {
        // ⚠️ `Relaxed` throughout this module, deliberately: each counter is
        // independent and the only property that matters is that
        // `fetch_add` itself is atomic (no lost updates under concurrent
        // callers) — nothing here orders one counter's value against
        // another's or against unrelated memory, which is the property a
        // stronger ordering would buy and nothing here needs.
        self.counter(op).load(Ordering::Relaxed)
    }

    const fn counter(&self, op: Operation) -> &AtomicU64 {
        match op {
            Operation::Get => &self.get,
            Operation::Put => &self.put,
            Operation::Delete => &self.delete,
        }
    }

    fn increment(&self, op: Operation) {
        self.counter(op).fetch_add(1, Ordering::Relaxed);
    }
}

/// Wraps any [`ObjectStore`], counting how many calls of each [`Operation`]
/// pass through it.
///
/// The same trick as any decorator, so it composes with the fake today and a
/// real backend once `M1.15`/`M1.17` build one, with no change to either.
///
/// ⚠️ **Counts on invocation, not on success.** A call is charged the moment
/// it is made, matching how S3 and GCS bill: a failed conditional `put`
/// still issued a request and still costs one, so counting only successful
/// calls would undercount exactly the case op-class accounting exists to
/// make visible.
#[derive(Debug, Default)]
pub struct CountingObjectStore<S> {
    inner: S,
    counts: OpCounts,
}

impl<S> CountingObjectStore<S> {
    /// Wraps `inner`, starting from zero counts.
    #[must_use]
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            counts: OpCounts::new(),
        }
    }

    /// The counts accumulated so far.
    #[must_use]
    pub const fn counts(&self) -> &OpCounts {
        &self.counts
    }
}

impl<S> CountingObjectStore<S> {
    /// The store underneath, so a caller that wrapped one to count it can
    /// still reach what it wrapped.
    pub const fn inner(&self) -> &S {
        &self.inner
    }
}

impl<S: ObjectStore> ObjectStore for CountingObjectStore<S> {
    fn get<'a>(
        &'a self,
        key: &'a ObjectKey,
        range: ByteRange,
    ) -> crate::BoxFuture<'a, Result<Vec<u8>>> {
        self.counts.increment(Operation::Get);
        self.inner.get(key, range)
    }

    fn put<'a>(
        &'a self,
        key: &'a ObjectKey,
        payload: Vec<u8>,
        precondition: Option<Precondition>,
    ) -> crate::BoxFuture<'a, Result<ObjectMeta>> {
        self.counts.increment(Operation::Put);
        self.inner.put(key, payload, precondition)
    }

    /// ⚠️ **Counted as one write**, whatever the part count: the number this
    /// wrapper exists to report is object operations, and a multipart upload
    /// is one object however many requests carry it — ⚠️ which is *not* the
    /// same as one request, and a cost model that needs the request count
    /// needs a second counter rather than this one reinterpreted.
    fn open_multipart<'a>(
        &'a self,
        key: &'a ObjectKey,
    ) -> crate::BoxFuture<'a, Result<Box<dyn crate::MultipartWriter<'a> + 'a>>> {
        Box::pin(async move {
            self.counts.increment(Operation::Put);
            self.inner.open_multipart(key).await
        })
    }

    fn delete<'a>(&'a self, keys: &'a [ObjectKey]) -> crate::BoxFuture<'a, Result<()>> {
        self.counts.increment(Operation::Delete);
        self.inner.delete(keys)
    }
}
