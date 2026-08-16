//! The object-storage seam, and the trivial fake that makes it testable.

use crate::{ByteRange, Error, ObjectKey, ObjectMeta, Precondition, PreconditionToken, Result};
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
/// ⚠️ [`FakeObjectStore`] cannot catch a violation of either, because it is
/// strongly consistent and infallible by construction.
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
    /// hold. Otherwise implementation-defined; the fake here cannot fail
    /// unconditionally.
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
    /// Implementation-defined; the fake here cannot fail.
    fn delete<'a>(&'a self, keys: &'a [ObjectKey]) -> BoxFuture<'a, Result<()>>;
}

/// One key's state as the fake holds it: its bytes if any are currently
/// stored, and a generation that increments on every write and **survives
/// deletion**.
///
/// ⚠️ **The generation, not a hash of `payload`, backs
/// [`PreconditionToken`].** The type's own documentation says a precondition
/// token is never a content hash — multipart and SSE-KMS break that
/// assumption on a real backend — and the fake is written to the same rule
/// so a test cannot pass here for a reason a real backend would not share.
///
/// ⚠️ **`payload: None` after a `delete`, not a removed map entry.** A real
/// backend's generation counter (GCS) or version chain (S3) is never reused
/// once a key is deleted and later recreated — deleting the map entry
/// outright would let a fresh `put` on a reused key start its generation
/// back at 1, so a token issued before the delete could compare equal to one
/// issued for an entirely different object that happens to reuse the key
/// afterward. Keeping the slot (with no payload) is what stops that.
#[derive(Debug, Clone)]
struct KeySlot {
    payload: Option<Vec<u8>>,
    generation: u64,
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
/// Deliberately trivial. It stores bytes, slices them on a ranged `get`,
/// tracks a per-key generation for [`ObjectMeta::precondition_token`], and
/// checks a [`Precondition`] atomically against that generation on `put` —
/// but models no latency and no failure. ⚠️ Which means a test passing
/// against it has shown nothing about how the code behaves when a `put`
/// fails after the object landed, or how a conditional write behaves under
/// real concurrency rather than a single `Mutex`'s serialization. `M1.8` is
/// where that is bought.
#[derive(Default)]
pub struct FakeObjectStore {
    slots: Mutex<HashMap<ObjectKey, KeySlot>>,
}

impl FakeObjectStore {
    /// An empty store.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// How many objects currently have a payload — a key whose slot survives
    /// only to remember its generation, after a `delete`, does not count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.lock().values().filter(|s| s.payload.is_some()).count()
    }

    /// Whether it holds nothing.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<ObjectKey, KeySlot>> {
        // ⚠️ Poison is recovered rather than propagated, deliberately. A
        // poisoned lock here means some *other* test panicked while holding
        // it; the map is a plain `HashMap` with no invariant a panic could
        // have broken mid-update, so the recovered state is sound. Panicking
        // instead would turn one failing test into a cascade of unrelated
        // ones, hiding the original.
        self.slots
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

/// The [`PreconditionToken`] a given generation renders as.
///
/// ⚠️ **Centralized so a conditional `put`'s check and a `put`'s returned
/// `ObjectMeta` can never disagree about what a generation's token looks
/// like.** Two separate `format!("gen-{n}")` call sites is exactly the kind
/// of duplication that drifts silently the first time one of them changes.
fn token_for_generation(generation: u64) -> PreconditionToken {
    PreconditionToken::new(format!("gen-{generation}"))
}

/// Slices `payload` according to `range`, or classifies why it cannot.
fn slice_range(key: &ObjectKey, payload: &[u8], range: ByteRange) -> Result<Vec<u8>> {
    let object_size = payload.len();
    match range {
        ByteRange::Full => Ok(payload.to_vec()),
        ByteRange::Bounded(bounded) => {
            let offset = bounded.offset();
            let length = bounded.length();
            let out_of_bounds = || Error::ByteRangeOutOfBounds {
                key: key.clone(),
                offset,
                length,
                // ⚠️ `unwrap_or(u64::MAX)`, not `as`: this is only ever
                // constructed on the error path, where the exact value does
                // not change the classification — reporting the true size
                // wrongly matters, reporting it as "too large to state" does
                // not, and neither reachable on the 64-bit hosts `NFR-40`
                // targets, where `usize` and `u64` are the same width.
                object_size: u64::try_from(object_size).unwrap_or(u64::MAX),
            };
            // ⚠️ `checked_add`/`try_from` rather than `+`/`as`: both a `u64`
            // overflow and an offset or end past what `usize` can index are
            // out-of-bounds requests, not values to wrap or truncate into
            // something that looks in-bounds.
            let start = usize::try_from(offset).map_err(|_| out_of_bounds())?;
            let length = usize::try_from(length).map_err(|_| out_of_bounds())?;
            let end = start
                .checked_add(length)
                .filter(|&end| end <= object_size)
                .ok_or_else(out_of_bounds)?;
            Ok(payload[start..end].to_vec())
        }
    }
}

impl ObjectStore for FakeObjectStore {
    fn get<'a>(&'a self, key: &'a ObjectKey, range: ByteRange) -> BoxFuture<'a, Result<Vec<u8>>> {
        Box::pin(async move {
            let payload = self
                .lock()
                .get(key)
                .and_then(|slot| slot.payload.clone())
                .ok_or_else(|| Error::ObjectNotFound { key: key.clone() })?;
            slice_range(key, &payload, range)
        })
    }

    fn put<'a>(
        &'a self,
        key: &'a ObjectKey,
        payload: Vec<u8>,
        precondition: Option<Precondition>,
    ) -> BoxFuture<'a, Result<ObjectMeta>> {
        Box::pin(async move {
            let size = payload.len();
            let generation = {
                let mut slots = self.lock();
                // ⚠️ Checked and written under the one lock acquisition —
                // never a separate check followed by a separate write, which
                // is exactly the gap a second racing `put` could land in.
                let holds = match &precondition {
                    None => true,
                    Some(Precondition::IfAbsent) => {
                        slots.get(key).is_none_or(|s| s.payload.is_none())
                    }
                    Some(Precondition::IfMatches(token)) => slots.get(key).is_some_and(|s| {
                        s.payload.is_some() && token_for_generation(s.generation) == *token
                    }),
                };
                if !holds {
                    return Err(Error::PreconditionFailed { key: key.clone() });
                }
                // ⚠️ The generation survives an overwrite *and* a delete, and
                // starts from the previous one rather than from zero — see
                // `KeySlot`'s doc comment for why a fake that reset it on
                // delete would be lying about what a real backend does.
                let generation = slots.get(key).map_or(0, |s| s.generation) + 1;
                slots.insert(
                    key.clone(),
                    KeySlot {
                        payload: Some(payload),
                        generation,
                    },
                );
                generation
            };
            Ok(ObjectMeta {
                // `payload.len()` is bounded by what actually fit in memory
                // to construct this future, so a lossy cast here never loses
                // anything a real deployment could reach; see `slice_range`'s
                // matching comment for the same reasoning stated in full.
                size: u64::try_from(size).unwrap_or(u64::MAX),
                precondition_token: token_for_generation(generation),
            })
        })
    }

    fn delete<'a>(&'a self, keys: &'a [ObjectKey]) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            {
                let mut slots = self.lock();
                for key in keys {
                    // ⚠️ Clears the payload rather than removing the entry —
                    // see `KeySlot`'s doc comment. A key nothing ever put is
                    // not in the map at all, so this is still a true no-op
                    // for `deleting_an_already_absent_key_succeeds`.
                    if let Some(slot) = slots.get_mut(key) {
                        slot.payload = None;
                    }
                }
            }
            Ok(())
        })
    }
}
