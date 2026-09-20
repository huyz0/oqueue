//! The read side: unwrapped data encryption keys, kept by wrapped blob for a
//! while.
//!
//! # ⚠️ Keyed by the wrapped blob, never by topic
//!
//! A writer has *one* live DEK per topic; a reader has as many as the topic
//! has ever rotated through, because an object sealed six months ago carries
//! the envelope it was sealed with. Keying this cache by topic would make
//! every read of an older object evict the entry the newer objects are using,
//! and a scan backwards through a partition would then be one KMS `unwrap` per
//! object — which is the cost `NFR-33` exists to refuse, on the read side
//! rather than the write side.
//!
//! The wrapped blob is not secret (it is stored in the clear beside the data)
//! and it names exactly one DEK, so it is the identity to key on. ⚠️ The key id
//! is part of the identity too: the same bytes under a different KEK are a
//! different `unwrap` call with a different answer, and `ADR-0006` guarantee 2
//! says one of the two must fail.
//!
//! # ⚠️ What the TTL costs and what it buys
//!
//! It buys a bounded window in which a **revoked or rotated KEK is still
//! effectively granting reads**: a DEK unwrapped before the revocation stays
//! usable until it ages out, so revocation takes effect within
//! [`UNWRAPPED_DEK_TTL_MS`] rather than instantly. It also buys a bound on how
//! long plaintext key material for a tenant sits in this process's memory,
//! which is what a heap dump would find.
//!
//! It costs availability, and `ADR-0050`'s last consequence names it: **a KMS
//! outage degrades reads for BYOK topics once a DEK ages out of cache.** While
//! an entry is live the outage is invisible; the moment it expires, reads of
//! that key domain fail until the KMS answers again. Lengthening the TTL trades
//! revocation latency for outage tolerance, and there is no setting that gets
//! both.

use oqueue_core::{Clock, Dek, KeyId, KeyProvider, Result, Timestamp, WrappedKey};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::{Mutex, MutexGuard, PoisonError};

/// How long an unwrapped DEK is kept before the KMS must be asked again: five
/// minutes, in milliseconds.
///
/// ⚠️ **Weakening is *raising*.** A longer TTL widens the window in which a
/// revoked KEK still grants reads (`ADR-0050` point 5 defines revocation as a
/// refusal a client can see, and an entry that has not expired never reaches
/// the refusal) and lengthens how long plaintext key material sits in memory.
/// Lowering it is safe and costs availability: a KMS outage starts failing
/// BYOK reads sooner.
///
/// ⚠️ **Chosen, not measured.** Five minutes keeps a full scan of a large
/// partition's history at one `unwrap` per DEK while keeping revocation
/// latency inside the minutes an operator revoking a key expects. `M15`, which
/// is where a real KMS is finally in the loop, is where a measurement could
/// replace it.
pub const UNWRAPPED_DEK_TTL_MS: i64 = 300_000;

/// What identifies an unwrapped DEK: the KEK it came from, and the blob.
///
/// ⚠️ The blob's bytes are cloned into the key. They are ciphertext already
/// stored in the clear, so this is not another copy of anything secret — the
/// secret is the [`Dek`] in the value, and that one zeroizes on drop.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct BlobId {
    key_id: KeyId,
    wrapped: Vec<u8>,
}

impl BlobId {
    fn of(key_id: &KeyId, wrapped: &WrappedKey) -> Self {
        Self {
            key_id: key_id.clone(),
            wrapped: wrapped.as_redacted().expose().clone(),
        }
    }
}

/// A DEK that came back from the KMS, and when.
#[derive(Debug)]
struct CachedDek {
    dek: Dek,
    unwrapped_at: Timestamp,
}

impl CachedDek {
    const fn fresh_at(&self, now: Timestamp) -> bool {
        now.as_millis()
            .saturating_sub(self.unwrapped_at.as_millis())
            < UNWRAPPED_DEK_TTL_MS
    }
}

/// Unwrapped DEKs, keyed by the wrapped blob that produced them.
///
/// # Sharing it across tasks
///
/// The same discipline as [`DekCache`](crate::DekCache): a
/// `std::sync::Mutex` around the map, and **no `.await` while it is held**.
/// [`KeyProvider::unwrap`] is awaited with no lock at all, and two readers that
/// race on a cold entry both unwrap — at most one wasted KMS call, never a
/// lock held across a network round trip while every other read waits
/// (`async-concurrency.md` rule 6).
#[derive(Debug)]
pub struct UnwrappedDekCache<C, P> {
    clock: C,
    provider: P,
    entries: Mutex<HashMap<BlobId, CachedDek>>,
}

impl<C: Clock, P: KeyProvider> UnwrappedDekCache<C, P> {
    /// A cache holding nothing, reading `clock`, unwrapping through
    /// `provider`.
    #[must_use]
    pub fn new(clock: C, provider: P) -> Self {
        Self {
            clock,
            provider,
            entries: Mutex::new(HashMap::new()),
        }
    }

    /// Runs `open` against the DEK `wrapped` names, unwrapping it through the
    /// KMS only if no fresh entry is held.
    ///
    /// ⚠️ **A closure, for the same reason the write side takes one**: [`Dek`]
    /// is not `Clone`, so the key never leaves the cache, and holding the lock
    /// across the call means the entry cannot expire underneath an open that
    /// is using it.
    ///
    /// ⚠️ **Do not `.await` inside `open`** — it runs under a
    /// `std::sync::Mutex`, and `FnOnce` is what makes that unwritable.
    ///
    /// # Errors
    ///
    /// Whatever [`KeyProvider::unwrap`] returned —
    /// [`oqueue_core::Error::SecretRejected`] for a blob that was not produced
    /// under this key id, [`oqueue_core::Error::EncryptionDisabled`] from a
    /// deployment with no KMS, or a provider's own classification of a revoked
    /// key (`ADR-0050` point 5). [`oqueue_core::Error::DekLength`] if the
    /// provider returned a plaintext
    /// that is not a 256-bit key, which is a provider that does not honour
    /// `ADR-0006`.
    pub async fn with_dek<R, F>(&self, key_id: &KeyId, wrapped: &WrappedKey, open: F) -> Result<R>
    where
        F: FnOnce(&Dek) -> R,
    {
        let id = BlobId::of(key_id, wrapped);
        // ⚠️ **Two attempts at most, and no loop** — the same property
        // [`DekCache::with_live_dek`](crate::DekCache::with_live_dek) states
        // and for the same reason: a `loop { use or fetch }` spins forever if
        // `fresh_at` is ever wrong in the direction of always-false.
        // `fetch_and_use` installs and opens under one lock, so the second
        // attempt cannot miss.
        match self.use_fresh(&id, open) {
            Ok(result) => Ok(result),
            Err(open) => self.fetch_and_use(id, key_id, wrapped, open).await,
        }
    }

    /// The clock it reads. ⚠️ Exposed so a composition root and a test can
    /// hold one clock rather than two that disagree.
    pub const fn clock(&self) -> &C {
        &self.clock
    }

    /// The key provider it unwraps through — what lets `NFR-33`'s read-side
    /// test count calls at the seam.
    pub const fn provider(&self) -> &P {
        &self.provider
    }

    /// How many unwrapped DEKs are held, expired ones included until the
    /// lookup that evicts them.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries().len()
    }

    /// Whether it holds nothing at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries().is_empty()
    }

    /// Runs `open` under the lock if a fresh entry is held; evicts an expired
    /// one — which drops its [`Dek`], zeroizing it — and hands the closure
    /// back.
    ///
    /// ⚠️ **The closure comes back in the `Err`** when it did not run, which is
    /// what makes [`Self::with_dek`] two total attempts rather than a loop.
    fn use_fresh<R, F>(&self, id: &BlobId, open: F) -> core::result::Result<R, F>
    where
        F: FnOnce(&Dek) -> R,
    {
        let now = self.clock.now();
        let mut entries = self.entries();
        let Some(entry) = entries.get(id) else {
            return Err(open);
        };
        if !entry.fresh_at(now) {
            // ⚠️ Evicted here rather than left to be overwritten, so an entry
            // that is never asked for again does not keep key material alive
            // for the life of the process.
            entries.remove(id);
            return Err(open);
        }
        let result = open(&entry.dek);
        drop(entries);
        Ok(result)
    }

    /// The one KMS call, then the open — installing unless a racing reader
    /// already did, and opening under the same lock either way.
    ///
    /// ⚠️ Installing and opening together is what makes the caller's second
    /// attempt unmissable; splitting them leaves a window in which the fresh
    /// entry expires again.
    async fn fetch_and_use<R, F>(
        &self,
        id: BlobId,
        key_id: &KeyId,
        wrapped: &WrappedKey,
        open: F,
    ) -> Result<R>
    where
        F: FnOnce(&Dek) -> R,
    {
        let plaintext = self.provider.unwrap(key_id, wrapped).await?;
        // ⚠️ `Dek::from_slice` copies, and the `Redacted` the provider returned
        // is dropped at the end of this statement *without* a wipe —
        // `Redacted` has no `Drop` of its own (see its documentation) and this
        // allocation is the provider's rather than ours to have zeroized in
        // place. What this crate can guarantee is that the copy it keeps
        // zeroizes.
        let dek = Dek::from_slice(plaintext.expose())?;
        let now = self.clock.now();
        let cached = CachedDek {
            dek,
            unwrapped_at: now,
        };

        // ⚠️ One `Entry` lookup, and the reference comes out of the branch that
        // decided — never a second `get` needing a fallback for a case that
        // cannot happen.
        let mut entries = self.entries();
        let entry = match entries.entry(id) {
            Entry::Occupied(occupied) if occupied.get().fresh_at(now) => {
                // Lost the race; the DEK just unwrapped is dropped here and
                // zeroized, and the incumbent answers.
                drop(cached);
                occupied.into_mut()
            }
            Entry::Occupied(mut occupied) => {
                drop(occupied.insert(cached));
                occupied.into_mut()
            }
            Entry::Vacant(vacant) => vacant.insert(cached),
        };
        let result = open(&entry.dek);
        drop(entries);
        Ok(result)
    }

    /// ⚠️ Poison is recovered from, not propagated: every critical section
    /// here is a single map operation and cannot be left half-applied.
    fn entries(&self) -> MutexGuard<'_, HashMap<BlobId, CachedDek>> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
