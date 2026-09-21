//! The write side: one live data encryption key per topic, rotated on bytes
//! or on time.
//!
//! `NFR-33` is the requirement — *KMS calls are a function of DEK rotation,
//! not of produce volume* — and this module is where it is met. A broker that
//! wrapped a fresh DEK per flush would make every produce a KMS call: the
//! quota would bound throughput, and the KMS bill would scale with the log.
//! Instead one DEK per topic is minted, wrapped **once**, and held with the
//! [`WrappedKey`] the provider returned, so sealing a region needs the cache
//! and nothing else.
//!
//! # ⚠️ Why a DEK is retired at all
//!
//! `ADR-0050` point 3: whichever comes first of [`DEK_MAX_SEALED_BYTES`] or
//! [`DEK_MAX_AGE_MS`]. The byte bound is the security argument — with a
//! constructed (counter) nonce under one AES-GCM key, what is bounded is how
//! much plaintext may share a key — and the time bound is the blast-radius
//! argument: a key that leaked is a key that opens everything sealed under it,
//! so its window is capped even for a topic that produces almost nothing.
//!
//! # ⚠️ What this cache is not
//!
//! It is not a *read* cache. The read side is keyed by the wrapped blob rather
//! than by topic, because a reader meets objects sealed under DEKs this topic
//! retired long ago — see [`unwrap_cache`](crate::unwrap_cache).

use crate::entropy::{Entropy, mint_dek};
use oqueue_core::{
    Clock, Dek, KeyId, KeyProvider, OperationalMetrics, Redacted, Result, Timestamp, TopicId,
    WrappedKey,
};
use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::sync::{Mutex, MutexGuard, PoisonError};
use tracing::Instrument;
use zeroize::Zeroize as _;

/// How many bytes may be sealed under one DEK before it is retired: 64 GiB.
///
/// ⚠️ **Weakening is *raising*.** `ADR-0050` point 3 derives it from the
/// security argument rather than from a KMS quota: under a constructed nonce,
/// the bound that matters is how much plaintext shares one AES-GCM key, and
/// 64 GiB sits far below the algorithm's own limit while still leaving KMS
/// calls at roughly one per topic per rotation, which is what `NFR-33` asks
/// for. Raising it puts more of a tenant's log behind one key and widens what
/// a single compromised DEK opens. Lowering it only spends more KMS calls.
///
/// ⚠️ **Counted as plaintext offered to [`DekCache::with_live_dek`]**, not as
/// ciphertext written — the two differ by the AEAD tag per region, and the
/// quantity the security argument bounds is the plaintext.
pub const DEK_MAX_SEALED_BYTES: u64 = 68_719_476_736;

/// How long a DEK may stay live before it is retired: 7 days, in
/// milliseconds.
///
/// ⚠️ **Weakening is *raising*.** It bounds the window in which a leaked DEK
/// is still the one being written under, for a topic whose traffic would never
/// reach [`DEK_MAX_SEALED_BYTES`]. Without it a near-idle topic would keep one
/// key forever, which is the case the byte bound alone cannot catch. Lowering
/// it costs one extra `wrap` per topic per window and nothing else.
///
/// Equal to `ADR-0050` point 3's 7 days, and deliberately *not* shared with
/// `oqueue-compact`'s `DEFAULT_RETENTION_MS`, which is the same number for an
/// unrelated reason.
pub const DEK_MAX_AGE_MS: i64 = 604_800_000;

/// The live DEK for one topic, and what is known about how far it has gone.
#[derive(Debug)]
struct LiveDek {
    /// The KEK it was wrapped under. ⚠️ Part of the identity: a topic whose
    /// configured KEK changes needs a new DEK, not a re-labelled one.
    key_id: KeyId,
    /// What the provider returned, so the footer's envelope can be written
    /// without calling the KMS again.
    wrapped: WrappedKey,
    /// The plaintext key. ⚠️ Zeroized when this entry is dropped, because
    /// [`Dek`] is `ZeroizeOnDrop`; retiring a DEK is therefore a `HashMap`
    /// insert that replaces it, and the wipe is not something a caller can
    /// forget.
    dek: Dek,
    /// When it was minted, for [`DEK_MAX_AGE_MS`].
    minted_at: Timestamp,
    /// Plaintext bytes sealed under it so far, for [`DEK_MAX_SEALED_BYTES`].
    /// Saturating, so an absurd charge cannot wrap it back under the bound.
    sealed_bytes: u64,
}

impl LiveDek {
    /// Whether this entry may still be used at `now` under `key_id`.
    ///
    /// ⚠️ **Checked before the bytes are charged, never after**, which is what
    /// makes [`DekCache::with_live_dek`]'s retry terminate: a freshly minted
    /// entry is always usable, whatever the caller is about to seal.
    fn usable(&self, key_id: &KeyId, now: Timestamp) -> bool {
        self.key_id == *key_id
            && self.sealed_bytes < DEK_MAX_SEALED_BYTES
            && now.as_millis().saturating_sub(self.minted_at.as_millis()) < DEK_MAX_AGE_MS
    }
}

/// One live DEK per topic, wrapped once and held until it is retired.
///
/// # Sharing it across tasks
///
/// A `std::sync::Mutex`, not an async one (`async-concurrency.md` rules 6 and
/// 8): the critical section is a `HashMap` probe and an integer add, and
/// **nothing is awaited while it is held**. The one `.await` here is
/// [`KeyProvider::wrap`], and it happens with no lock held at all —
/// [`DekCache::with_live_dek`] drops the guard, wraps, and takes the lock
/// again to install.
///
/// ⚠️ **That gap is a real race and is resolved by keeping the incumbent.**
/// Two tasks that both find no usable DEK both mint and both call `wrap`; the
/// second to finish sees an entry already installed, discards its own DEK, and
/// uses the incumbent. So the worst case is one wasted `wrap` per contended
/// rotation — never two live DEKs for a topic, and never a DEK written into an
/// envelope that the cache is not holding. Serializing the wrap instead would
/// mean holding a lock across a network round trip, which rule 6 forbids for
/// the reason it forbids it everywhere: every other topic's flush would wait
/// behind this topic's KMS call.
#[derive(Debug)]
pub struct DekCache<C, P, E> {
    clock: C,
    provider: P,
    entropy: E,
    live: Mutex<HashMap<TopicId, LiveDek>>,
    metrics: OperationalMetrics,
    /// Mints since construction, for the tests that assert rotation happened.
    mints: Mutex<HashMap<TopicId, u64>>,
}

impl<C: Clock, P: KeyProvider, E: Entropy> DekCache<C, P, E> {
    /// A cache holding nothing, reading `clock`, wrapping through `provider`,
    /// minting from `entropy`.
    #[must_use]
    pub fn new(clock: C, provider: P, entropy: E) -> Self {
        Self {
            clock,
            provider,
            entropy,
            live: Mutex::new(HashMap::new()),
            mints: Mutex::new(HashMap::new()),
            metrics: OperationalMetrics::default(),
        }
    }

    /// Shares an operational metrics set with the composition root.
    #[must_use]
    pub fn with_metrics(mut self, metrics: OperationalMetrics) -> Self {
        self.metrics = metrics;
        self
    }

    /// The metrics set receiving cache observations.
    #[must_use]
    pub const fn metrics(&self) -> &OperationalMetrics {
        &self.metrics
    }

    /// Runs `seal` against `topic`'s live DEK, minting and wrapping a new one
    /// first if the live one is missing or retired, and charging `bytes`
    /// against it afterwards.
    ///
    /// `seal` is handed the plaintext DEK, the key id, and the wrapped blob —
    /// which is exactly what a region's footer envelope needs
    /// (`oqueue_core::RegionEnvelope`), and is why all three are passed rather
    /// than the caller fetching the last two separately and risking a pair
    /// from two different rotations.
    ///
    /// ⚠️ **A closure rather than a returned `&Dek`**, and not for style:
    /// [`Dek`] is deliberately not `Clone` (every clone is another plaintext
    /// copy with its own lifetime), so the key cannot leave the cache. Under
    /// the closure the lock is held, so the DEK cannot be retired underneath a
    /// seal that is using it.
    ///
    /// ⚠️ **Do not `.await` inside `seal`.** It runs under a
    /// `std::sync::Mutex`; the signature takes `FnOnce`, not an async closure,
    /// so this is a rule the type system already holds.
    ///
    /// ⚠️ **And do not call back into this cache from `seal`.** The lock is
    /// held for the length of the call and `std::sync::Mutex` is not
    /// reentrant, so [`Self::with_live_dek`], [`Self::sealed_bytes`],
    /// [`Self::holds_live_dek`], [`Self::live_topics`] and
    /// [`Self::live_dek_is`] all deadlock that task permanently if called from
    /// inside. Nothing in the type system prevents it, unlike the `.await`
    /// above, so it is a rule rather than a guarantee.
    ///
    /// # Errors
    ///
    /// [`oqueue_core::Error::EntropyUnavailable`] if a fresh DEK cannot be
    /// minted, or whatever [`KeyProvider::wrap`] returned — including
    /// [`oqueue_core::Error::EncryptionDisabled`] from
    /// [`NoOpKeyProvider`](crate::NoOpKeyProvider). ⚠️ A failure here is a
    /// produce that is **refused**, never one that falls back to writing
    /// plaintext (`ADR-0050` point 5).
    pub async fn with_live_dek<R, F>(
        &self,
        topic: &TopicId,
        key_id: &KeyId,
        bytes: u64,
        seal: F,
    ) -> Result<R>
    where
        F: FnOnce(&Dek, &KeyId, &WrappedKey) -> R,
    {
        // ⚠️ **Two attempts at most, and no loop**, which is a property worth
        // stating rather than a shape that fell out: a `loop { use or rotate }`
        // reads naturally and spins forever the moment `usable` is wrong in the
        // one direction that matters — always false. `rotate_and_use` installs
        // and seals under one lock, so the second attempt cannot miss and there
        // is no third.
        match self.use_live(topic, key_id, bytes, seal) {
            Ok(result) => Ok(result),
            // `use_live` hands the closure back untouched when it did not run
            // it, which is what makes this total without an `unreachable!`.
            Err(seal) => self.rotate_and_use(topic, key_id, bytes, seal).await,
        }
    }

    /// The clock it reads.
    ///
    /// ⚠️ Exposed so a composition root and a test can hold one clock rather
    /// than two that disagree — not so anything here can be bypassed.
    pub const fn clock(&self) -> &C {
        &self.clock
    }

    /// The key provider it wraps through.
    ///
    /// ⚠️ Exposed for the same reason as [`Self::clock`], and it is what lets
    /// `NFR-33`'s test count calls at the seam.
    pub const fn provider(&self) -> &P {
        &self.provider
    }

    /// Whether `topic` currently holds a usable DEK under `key_id`.
    ///
    /// ⚠️ Answered as of *now*; by the time a caller acts on it the answer may
    /// have changed, which is why [`Self::with_live_dek`] does not use it.
    /// It is here for an operator-facing status page and for tests.
    #[must_use]
    pub fn holds_live_dek(&self, topic: &TopicId, key_id: &KeyId) -> bool {
        let now = self.clock.now();
        self.live()
            .get(topic)
            .is_some_and(|entry| entry.usable(key_id, now))
    }

    /// How many DEKs have been minted for `topic` since construction.
    ///
    /// One per rotation, and one for the first use — so *n* rotations after
    /// the first seal reads as `n + 1`.
    #[must_use]
    pub fn mints(&self, topic: &TopicId) -> u64 {
        self.mint_counts().get(topic).copied().unwrap_or(0)
    }

    /// Plaintext bytes charged against `topic`'s live DEK, or `None` if it
    /// holds none.
    #[must_use]
    pub fn sealed_bytes(&self, topic: &TopicId) -> Option<u64> {
        self.live().get(topic).map(|entry| entry.sealed_bytes)
    }

    /// How many topics hold a live DEK.
    ///
    /// ⚠️ **The assertion that a rotation *replaced* rather than accumulated.**
    /// A retired DEK is dropped — and zeroized — by the insert that replaces
    /// it, so this staying at one across a rotation is what a test can check
    /// without reading memory it no longer owns.
    #[must_use]
    pub fn live_topics(&self) -> usize {
        self.live().len()
    }

    /// Whether `topic`'s live DEK is `dek`.
    ///
    /// ⚠️ **Constant-time**, through [`Dek::ct_eq`] — a bytewise compare of key
    /// material is a timing oracle, and an inspector is no exception. It
    /// exists so a rotation test can say *the old key is gone and this new one
    /// is here* against the cache's own state.
    #[must_use]
    pub fn live_dek_is(&self, topic: &TopicId, dek: &Dek) -> bool {
        self.live()
            .get(topic)
            .is_some_and(|entry| entry.dek.ct_eq(dek))
    }

    /// Runs `seal` under the lock if `topic` holds a usable DEK, charging
    /// `bytes`.
    ///
    /// ⚠️ **Returns the closure in the `Err` when it did not run it**, rather
    /// than an `Option` and a closure the caller has to have kept somewhere.
    /// That is what makes [`Self::with_live_dek`] two total attempts instead of
    /// a loop with an `unreachable!` in it.
    fn use_live<R, F>(
        &self,
        topic: &TopicId,
        key_id: &KeyId,
        bytes: u64,
        seal: F,
    ) -> core::result::Result<R, F>
    where
        F: FnOnce(&Dek, &KeyId, &WrappedKey) -> R,
    {
        let now = self.clock.now();
        let mut live = self.live();
        let Some(entry) = live.get_mut(topic) else {
            self.metrics.record_dek_cache_miss();
            return Err(seal);
        };
        if !entry.usable(key_id, now) {
            self.metrics.record_dek_cache_miss();
            return Err(seal);
        }
        self.metrics.record_dek_cache_hit();
        let result = seal(&entry.dek, &entry.key_id, &entry.wrapped);
        // ⚠️ Charged after the seal succeeded in running, and saturating: the
        // bound must never be walked back under by an overflow.
        entry.sealed_bytes = entry.sealed_bytes.saturating_add(bytes);
        // ⚠️ Explicit, and the last statement of the critical section: the
        // guard is deliberately held across `seal` above, because a DEK that
        // could be retired mid-seal is a region sealed under a key the cache
        // is no longer holding.
        drop(live);
        Ok(result)
    }

    /// Mints a DEK, wraps it — the one KMS call — installs it unless another
    /// task installed a usable one meanwhile, and seals with whichever is
    /// live, all under one lock after the await.
    ///
    /// ⚠️ **Installing and sealing together is what makes the caller's second
    /// attempt unmissable.** Splitting them — install, release, try again —
    /// leaves a window in which the fresh DEK is retired again, and the only
    /// honest shapes then are an unbounded loop or an error for a case that
    /// cannot happen.
    async fn rotate_and_use<R, F>(
        &self,
        topic: &TopicId,
        key_id: &KeyId,
        bytes: u64,
        seal: F,
    ) -> Result<R>
    where
        F: FnOnce(&Dek, &KeyId, &WrappedKey) -> R,
    {
        let dek = mint_dek(&self.entropy)?;
        // ⚠️ A second plaintext copy, because the seam takes `Redacted<Vec<u8>>`
        // and a provider may need it for the length of its call. Wiped below
        // whichever way the call went: `Redacted` has no `Drop` of its own
        // (see its documentation), so this is the owner's obligation.
        let mut plaintext = Redacted::new(dek.expose().to_vec());
        let wrapped = traced_wrap(&self.provider, &self.metrics, key_id, &plaintext).await;
        plaintext.zeroize();
        let wrapped = wrapped?;

        let now = self.clock.now();
        let fresh = LiveDek {
            key_id: key_id.clone(),
            wrapped,
            dek,
            minted_at: now,
            sealed_bytes: 0,
        };

        // ⚠️ One `Entry` lookup, and the reference comes out of the branch
        // that decided — never a second `get` that would need a fallback for a
        // case that cannot happen. A fallback here would have to be *some*
        // `LiveDek`, and the only values available are wrong ones: sealing
        // under an all-zero DEK is precisely the silent compromise this whole
        // module exists to prevent.
        let mut live = self.live();
        let (installed, entry) = match live.entry(topic.clone()) {
            Entry::Occupied(occupied) if occupied.get().usable(key_id, now) => {
                // Lost the race. ⚠️ `fresh` is dropped here, zeroizing the DEK
                // nobody will ever see — see the type's own documentation for
                // why serializing the wrap instead is worse.
                drop(fresh);
                (false, occupied.into_mut())
            }
            Entry::Occupied(mut occupied) => {
                // ⚠️ The retirement: the returned previous `LiveDek` is dropped
                // at the end of this statement, and dropping it zeroizes its
                // `Dek`. There is no second place a retired key is reachable
                // from.
                drop(occupied.insert(fresh));
                (true, occupied.into_mut())
            }
            Entry::Vacant(vacant) => (true, vacant.insert(fresh)),
        };

        let result = seal(&entry.dek, &entry.key_id, &entry.wrapped);
        entry.sealed_bytes = entry.sealed_bytes.saturating_add(bytes);
        drop(live);

        if installed {
            *self.mint_counts().entry(topic.clone()).or_insert(0) += 1;
        }
        Ok(result)
    }

    /// ⚠️ **Poison is recovered from, not propagated** — the same call this
    /// workspace's other shared caches make. A panic in a critical section
    /// this short cannot leave a half-updated map: every mutation here is a
    /// single `insert` or a single integer add.
    fn live(&self) -> MutexGuard<'_, HashMap<TopicId, LiveDek>> {
        self.live.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// As [`Self::live`], for the mint counter.
    fn mint_counts(&self) -> MutexGuard<'_, HashMap<TopicId, u64>> {
        self.mints.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

async fn traced_wrap<P: KeyProvider + ?Sized>(
    provider: &P,
    metrics: &OperationalMetrics,
    key_id: &KeyId,
    plaintext: &Redacted<Vec<u8>>,
) -> Result<WrappedKey> {
    let span = tracing::info_span!(
        target: "oqueue",
        "kms",
        dependency = "kms",
        operation = "wrap",
        outcome = tracing::field::Empty,
        scope = "dek",
    );
    let result = provider
        .wrap(key_id, plaintext)
        .instrument(span.clone())
        .await;
    if result.is_err() {
        metrics.record_key_domain_failure();
    }
    span.record(
        "outcome",
        if result.is_ok() { "success" } else { "failure" },
    );
    result
}
