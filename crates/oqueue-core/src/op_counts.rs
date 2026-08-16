//! Op-class accounting: cost as a testable property of the seam, not a
//! review convention.

use crate::{ByteRange, ObjectKey, ObjectMeta, ObjectStore, Precondition, Result};
use core::sync::atomic::{AtomicU64, Ordering};

/// One kind of call through [`ObjectStore`].
///
/// ⚠️ **Exactly the trait's three methods, no more.** `list()` is
/// permanently absent — ADR-0009 keeps it off `ObjectStore` entirely, so
/// there is no call to count. A multipart-specific variant is deferred to
/// `M1.16`, the first commit that actually issues an `UploadPart`-shaped
/// call anywhere; adding one now would be counting a call nothing makes yet
/// (`error-handling.md` rule 6's reasoning, applied to a counter instead of
/// an error variant).
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

    fn delete<'a>(&'a self, keys: &'a [ObjectKey]) -> crate::BoxFuture<'a, Result<()>> {
        self.counts.increment(Operation::Delete);
        self.inner.delete(keys)
    }
}
