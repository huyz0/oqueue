//! The object-storage seam, and the trivial fake that makes it testable.

use crate::{ObjectKey, Result};
use std::collections::HashMap;
use std::future::Future;
use std::pin::Pin;
use std::sync::Mutex;

/// A boxed, `Send` future — the shape ADR-0002 gives every async seam.
///
/// ⚠️ Written out rather than reached for via a dependency: `async fn` in trait
/// is not `dyn`-compatible on this toolchain, and `bin/oqueue` must hold an
/// `Arc<dyn ObjectStore>` to choose a backend at startup (FR-50).
pub type BoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// Reads and writes whole objects.
///
/// ⚠️ **This is the only way anything in this workspace reaches object storage**
/// (NFR-51). It names no S3 or GCS type, and no vendor SDK appears in
/// `oqueue-core`; the backends live in `oqueue-store`, which `M1` writes.
///
/// # What an implementor must guarantee
///
/// See [ADR-0005]. In short: `put` is durable when its future resolves `Ok`, a
/// `get` of a key that was put returns exactly those bytes, and an object is
/// never partially visible.
///
/// ⚠️ Two that are easy to miss and are the reason the ADR exists. **A `put`
/// that resolves `Err` leaves the key in an unknown state** — the object may
/// have landed and only the acknowledgement been lost, so a failed `put` is
/// not proof of absence. And **visibility is strong per key**: absent a later
/// `put`, a `get` after a successful `put` never returns `ObjectNotFound`.
/// ⚠️ [`FakeObjectStore`] cannot catch a violation of either, because it is
/// strongly consistent and infallible by construction.
///
/// ADR-0005 is `docs/internal/product/decisions/0005-object-store-seam.md` in
/// this repository.
pub trait ObjectStore: Send + Sync + core::fmt::Debug {
    /// Fetches an object whole.
    ///
    /// # Errors
    ///
    /// [`crate::Error::ObjectNotFound`] if no object is stored under `key`.
    fn get<'a>(&'a self, key: &'a ObjectKey) -> BoxFuture<'a, Result<Vec<u8>>>;

    /// Stores an object whole, overwriting any previous value.
    ///
    /// # Errors
    ///
    /// Implementation-defined; the fake here cannot fail.
    fn put<'a>(&'a self, key: &'a ObjectKey, payload: Vec<u8>) -> BoxFuture<'a, Result<()>>;
}

/// An in-memory [`ObjectStore`] holding whatever was put into it.
///
/// ⚠️ **This is the fake, and it lives here beside the trait** —
/// `contracts.md` rule 9. It is *not* `oqueue-store`'s in-memory backend, which
/// is a real implementation that must pass the same conformance suite as S3
/// (`testing.md` rule 6). ⚠️ **Exactly one `ObjectStore` fake exists in this
/// tree**: `M1` rewrites this one rather than adding another beside it, because
/// two fakes with divergent semantics is the highest-risk defect class in the
/// project (doc 10 #33).
///
/// # Fidelity
///
/// Deliberately trivial. It stores bytes and returns them, and models no
/// latency, no failure and no conditional write — ⚠️ which means a test passing
/// against it has shown nothing about how the code behaves when a PUT fails
/// after the object landed. `M1`'s conformance suite is where that is bought.
#[derive(Default)]
pub struct FakeObjectStore {
    objects: Mutex<HashMap<ObjectKey, Vec<u8>>>,
}

impl FakeObjectStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many objects it holds.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().len()
    }

    /// Whether it holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lock().is_empty()
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<ObjectKey, Vec<u8>>> {
        // ⚠️ Poison is recovered rather than propagated, deliberately. A
        // poisoned lock here means some *other* test panicked while holding
        // it; the map is a plain `HashMap` with no invariant a panic could
        // have broken mid-update, so the recovered state is sound. Panicking
        // instead would turn one failing test into a cascade of unrelated
        // ones, hiding the original.
        self.objects
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// ⚠️ Prints how many objects are held, never their bytes. The derived `Debug`
/// dumped every payload — and a payload is customer data, which is the one
/// thing a `Debug` on a shared test double is most likely to end up in a log.
impl core::fmt::Debug for FakeObjectStore {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("FakeObjectStore")
            .field("objects", &self.len())
            .finish()
    }
}

impl ObjectStore for FakeObjectStore {
    fn get<'a>(&'a self, key: &'a ObjectKey) -> BoxFuture<'a, Result<Vec<u8>>> {
        Box::pin(async move {
            self.lock()
                .get(key)
                .cloned()
                .ok_or_else(|| crate::Error::ObjectNotFound { key: key.clone() })
        })
    }

    fn put<'a>(&'a self, key: &'a ObjectKey, payload: Vec<u8>) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            self.lock().insert(key.clone(), payload);
            Ok(())
        })
    }
}
