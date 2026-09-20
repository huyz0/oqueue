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
//! [`UNWRAPPED_DEK_TTL_MS`] rather than instantly.
//!
//! # ⚠️ The TTL alone bounds nothing about memory — the capacity does
//!
//! ⚠️ **This section used to claim the TTL bounded how long key material sits
//! in this process, and it did not.** There is no sweep here and no timer:
//! expiry is only ever noticed by a lookup, so an entry nobody asks for again
//! was never reached by anything. A consumer scanning a partition's history
//! under ten thousand rotated DEKs would have retained ten thousand plaintext
//! DEKs for the life of the process, every one of them long expired. That was
//! a real leak of key material, not a wording problem, and the fix is
//! [`UNWRAPPED_DEK_CACHE_ENTRIES`] rather than a softer sentence.
//!
//! What now holds, exactly: **after any insert the cache holds at most
//! [`UNWRAPPED_DEK_CACHE_ENTRIES`] entries, and every one of them was
//! unwrapped within the last [`UNWRAPPED_DEK_TTL_MS`].** Both halves come from
//! the same place — every insert first drops every expired entry, then evicts
//! the least recently used until there is room. ⚠️ Between inserts the bound
//! is the weaker *at most `UNWRAPPED_DEK_CACHE_ENTRIES` entries, each at most
//! TTL old as of the last insert*: a cache that goes quiet holds what it held,
//! and nothing wakes up to wipe it. Bounding *that* needs a sweep, which needs
//! a timer, which needs a runtime this crate does not have and will not take.
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
use zeroize::Zeroize as _;

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

/// The most unwrapped DEKs held at once, after any insert.
///
/// ⚠️ **Weakening is *raising*, and it weakens two different things.** It is
/// the bound on how much plaintext key material this process holds — a heap
/// dump finds at most this many 256-bit keys — and it is also the *only* thing
/// bounding how long an entry nobody looks up again survives, because expiry
/// is noticed by a lookup and eviction by an insert. A larger cache holds more
/// keys, and holds each abandoned one across more inserts. Lowering it is safe
/// and costs KMS calls.
///
/// ⚠️ **Chosen, and here is the arithmetic rather than a feeling.** One entry
/// is a 32-byte DEK plus its `BlobId` — a key id and a wrapped blob, the
/// latter bounded by `MAX_WRAPPED_DEK_LEN` (8192) and a few hundred bytes in
/// practice for both AWS and GCP. So 1024 entries is well under a megabyte
/// typically and ~8 MiB at the format's worst case, which is a bound an
/// operator can reason about. On the other side, 1024 DEKs is 1024 rotations
/// of one topic — 64 TiB of sealed data at `DEK_MAX_SEALED_BYTES`, or twenty
/// years at the age bound — reachable inside one `UNWRAPPED_DEK_TTL_MS` window
/// only by a reader sweeping very old history very fast, which is exactly the
/// case where paying a KMS call for the oldest blob is right.
pub const UNWRAPPED_DEK_CACHE_ENTRIES: usize = 1024;

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

/// A DEK that came back from the KMS, when, and how recently it was used.
#[derive(Debug)]
struct CachedDek {
    dek: Dek,
    unwrapped_at: Timestamp,
    /// A tick from [`Entries::next_use`], not a time.
    ///
    /// ⚠️ **Deliberately not a [`Timestamp`].** Recency has to order *every*
    /// pair of entries for "least recently used" to name one, and a clock is
    /// allowed to return the same instant twice — `ADR-0004` says so, and
    /// `FakeClock` does exactly that unless a test advances it. Two entries
    /// with equal timestamps would make the victim whichever one the map
    /// happened to iterate first, which is not an eviction policy.
    last_used: u64,
}

impl CachedDek {
    const fn fresh_at(&self, now: Timestamp) -> bool {
        now.as_millis()
            .saturating_sub(self.unwrapped_at.as_millis())
            < UNWRAPPED_DEK_TTL_MS
    }
}

/// Everything the lock protects: the map, and the recency counter that orders
/// it.
///
/// ⚠️ **One struct rather than two locks**, because a recency tick handed out
/// under a different lock than the map it orders is two places for the same
/// answer to come from.
#[derive(Debug, Default)]
struct Entries {
    by_blob: HashMap<BlobId, CachedDek>,
    /// The next recency tick to hand out. Monotonic for the life of the
    /// process; at one use per nanosecond it would take 584 years to wrap.
    next_use: u64,
}

impl Entries {
    /// The tick for a use happening now.
    const fn tick(&mut self) -> u64 {
        let tick = self.next_use;
        self.next_use = self.next_use.saturating_add(1);
        tick
    }

    /// Makes room for one more entry: drops everything expired, then the least
    /// recently used until the map is under capacity.
    ///
    /// ⚠️ **Called on the insert path only**, because this crate has no
    /// runtime and therefore no sweep — see the module documentation for
    /// exactly what that does and does not bound. Every removal here drops a
    /// [`CachedDek`], and dropping one zeroizes its [`Dek`]; that is the whole
    /// mechanism by which key material stops being resident.
    fn make_room(&mut self, now: Timestamp) {
        self.by_blob.retain(|_, entry| entry.fresh_at(now));
        while self.by_blob.len() >= UNWRAPPED_DEK_CACHE_ENTRIES {
            // ⚠️ `min_by_key` over the map rather than a second ordered
            // structure: capacity is a small constant, this runs only on a
            // miss that is already paying a KMS round trip, and a heap kept in
            // step with a `HashMap` is two places for the recency of a key to
            // disagree.
            let Some(victim) = self
                .by_blob
                .iter()
                .min_by_key(|(_, entry)| entry.last_used)
                .map(|(id, _)| id.clone())
            else {
                // ⚠️ Empty and still "at capacity" is unreachable while
                // `UNWRAPPED_DEK_CACHE_ENTRIES` is positive, and breaking
                // rather than trusting that is what makes this loop
                // unconditionally terminate.
                break;
            };
            self.by_blob.remove(&victim);
        }
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
    entries: Mutex<Entries>,
}

impl<C: Clock, P: KeyProvider> UnwrappedDekCache<C, P> {
    /// A cache holding nothing, reading `clock`, unwrapping through
    /// `provider`.
    #[must_use]
    pub fn new(clock: C, provider: P) -> Self {
        Self {
            clock,
            provider,
            entries: Mutex::new(Entries::default()),
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
    /// ⚠️ **And do not call back into this cache from `open`.** The lock is
    /// held for the length of the call and `std::sync::Mutex` is not
    /// reentrant, so [`Self::with_dek`], [`Self::len`] and [`Self::is_empty`]
    /// all deadlock that task permanently if called from inside. Nothing in
    /// the type system prevents it, unlike the `.await` above, so it is a
    /// rule rather than a guarantee.
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

    /// How many unwrapped DEKs are held — never more than
    /// [`UNWRAPPED_DEK_CACHE_ENTRIES`], and expired ones included until the
    /// lookup or the insert that evicts them.
    #[must_use]
    pub fn len(&self) -> usize {
        self.entries().by_blob.len()
    }

    /// Whether it holds nothing at all.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.entries().by_blob.is_empty()
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
        let Some(entry) = entries.by_blob.get(id) else {
            return Err(open);
        };
        if !entry.fresh_at(now) {
            // ⚠️ Evicted here rather than left to be overwritten — but this
            // path only ever reaches the key being looked up, which is why it
            // is **not** what bounds the cache. An entry nobody asks for again
            // is reached by `Entries::make_room` on some later insert, and by
            // nothing else.
            entries.by_blob.remove(id);
            return Err(open);
        }
        let tick = entries.tick();
        let Some(entry) = entries.by_blob.get_mut(id) else {
            return Err(open);
        };
        entry.last_used = tick;
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
        // ⚠️ **`mut`, and wiped below, because this value is *ours*.** The
        // provider moved it into this function and no other owner exists, so
        // if it is dropped unwiped then 32 bytes of plaintext AES-256 key go
        // back to the allocator on every read-side miss — `security.md` rule 8
        // is discharged by the *key* type, and `Redacted` explicitly has no
        // `Drop` of its own (see `redacted.rs`: a blanket `Drop` cannot be
        // conditional on `T: Zeroize`). An earlier comment here called this
        // "the provider's allocation"; that was wrong about ownership, and it
        // is the argument that let the leak through.
        let mut plaintext = self.provider.unwrap(key_id, wrapped).await?;
        let dek = Dek::from_slice(plaintext.expose());
        plaintext.zeroize();
        let now = self.clock.now();
        let cached = CachedDek {
            dek: dek?,
            unwrapped_at: now,
            last_used: 0,
        };

        let mut entries = self.entries();
        // ⚠️ Before the insert, and unconditionally: this is the only place
        // anything expired is dropped that nobody looked up. See the module
        // documentation for exactly what the resulting bound is.
        entries.make_room(now);
        let tick = entries.tick();

        // ⚠️ One `Entry` lookup, and the reference comes out of the branch that
        // decided — never a second `get` needing a fallback for a case that
        // cannot happen.
        let entry = match entries.by_blob.entry(id) {
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
        entry.last_used = tick;
        let result = open(&entry.dek);
        drop(entries);
        Ok(result)
    }

    /// ⚠️ Poison is recovered from, not propagated: every critical section
    /// here is a single map operation and cannot be left half-applied.
    fn entries(&self) -> MutexGuard<'_, Entries> {
        self.entries.lock().unwrap_or_else(PoisonError::into_inner)
    }
}
