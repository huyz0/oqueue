//! The read side: many objects under one DEK cost one `unwrap`.

// Sites are on values this test constructed from literals it controls.
#![allow(clippy::expect_used)]

use crate::support::{CountingKeyProvider, block_on, key_id};
use oqueue_core::{
    BoxFuture, DEK_BYTES, Dek, Error, FakeClock, FakeKeyProvider, KeyId, KeyProvider,
    OperationalMetrics, Redacted, Result, WrappedKey,
};
use oqueue_crypto::{UNWRAPPED_DEK_CACHE_ENTRIES, UNWRAPPED_DEK_TTL_MS, UnwrappedDekCache};

type TestCache = UnwrappedDekCache<FakeClock, CountingKeyProvider<FakeKeyProvider>>;

fn cache() -> TestCache {
    UnwrappedDekCache::new(
        FakeClock::new(),
        CountingKeyProvider::new(FakeKeyProvider::new()),
    )
}

#[derive(Debug)]
struct RevokedProvider;

impl KeyProvider for RevokedProvider {
    fn wrap<'a>(
        &'a self,
        key_id: &'a KeyId,
        _plaintext: &'a Redacted<Vec<u8>>,
    ) -> BoxFuture<'a, Result<WrappedKey>> {
        Box::pin(async move {
            Err(Error::KeyRevoked {
                key_id: key_id.clone(),
            })
        })
    }

    fn unwrap<'a>(
        &'a self,
        key_id: &'a KeyId,
        _wrapped: &'a WrappedKey,
    ) -> BoxFuture<'a, Result<Redacted<Vec<u8>>>> {
        Box::pin(async move {
            Err(Error::KeyRevoked {
                key_id: key_id.clone(),
            })
        })
    }
}

/// A wrapped blob for the DEK made of `filler`, produced through a *separate*
/// provider so building the fixture does not show up in the counts under
/// test.
fn wrapped(key_id: &KeyId, filler: u8) -> WrappedKey {
    let provider = FakeKeyProvider::new();
    let plaintext = Redacted::new(vec![filler; DEK_BYTES]);
    block_on(provider.wrap(key_id, &plaintext)).expect("the fake provider wraps")
}

/// Reads one object's worth: opens under the DEK the blob names and returns
/// the key's first byte, which is what identifies it here.
fn read(cache: &TestCache, key_id: &KeyId, blob: &WrappedKey) -> u8 {
    block_on(cache.with_dek(key_id, blob, |dek: &Dek| dek.expose()[0]))
        .expect("the fake provider unwraps")
}

/// The read-side half of `NFR-33`: volume of *objects* does not move the KMS
/// count either.
#[test]
fn many_objects_under_one_dek_cost_one_unwrap() {
    let kek = key_id();
    let cache = cache();
    let blob = wrapped(&kek, 0x11);

    for _ in 0..200 {
        assert_eq!(read(&cache, &kek, &blob), 0x11);
    }

    assert_eq!(
        cache.provider().unwraps(),
        1,
        "two hundred objects sealed under one DEK are one KMS unwrap"
    );
    assert_eq!(cache.len(), 1);
}

#[test]
fn metrics_distinguish_a_cold_unwrap_from_a_warm_hit() {
    let metrics = OperationalMetrics::default();
    let cache = cache().with_metrics(metrics.clone());
    let kek = key_id();
    let blob = wrapped(&kek, 0x12);

    assert_eq!(read(&cache, &kek, &blob), 0x12);
    assert_eq!(read(&cache, &kek, &blob), 0x12);

    let snapshot = metrics.snapshot();
    assert_eq!(snapshot.encryption_cache_misses, 1);
    assert_eq!(snapshot.encryption_cache_hits, 1);
}

#[test]
fn a_revoked_key_fails_reads_with_a_specific_non_retryable_error() {
    let kek = key_id();
    let blob = wrapped(&kek, 0x11);
    let cache = UnwrappedDekCache::new(FakeClock::new(), RevokedProvider);

    let result = block_on(cache.with_dek(&kek, &blob, |dek: &Dek| dek.expose()[0]));

    assert_eq!(result, Err(Error::KeyRevoked { key_id: kek }));
}

/// A fresh cache holds nothing, and one fetch makes that false.
///
/// ⚠️ Small, and here because `is_empty` is the accessor an operator-facing
/// status page would read: an `is_empty` that always answered "no" would make
/// a cold cache look warm, which is exactly the state worth being able to see
/// during a KMS outage.
#[test]
fn a_fresh_cache_is_empty_and_a_fetch_makes_it_not() {
    let kek = key_id();
    let cache = cache();
    assert!(cache.is_empty());
    assert_eq!(cache.len(), 0);

    read(&cache, &kek, &wrapped(&kek, 0x66));

    assert!(!cache.is_empty());
    assert_eq!(cache.len(), 1);
}

/// The TTL is real: once an entry ages out, the KMS is asked again.
///
/// ⚠️ This is the cost `ADR-0050`'s last consequence names — **a KMS outage
/// degrades reads for BYOK topics once a DEK ages out of cache** — made
/// visible. Up to the TTL the outage is invisible; past it, the read is a KMS
/// call and fails if the KMS does.
#[test]
fn an_aged_out_dek_is_unwrapped_again() {
    let kek = key_id();
    let cache = cache();
    let blob = wrapped(&kek, 0x22);

    read(&cache, &kek, &blob);
    assert_eq!(cache.provider().unwraps(), 1);

    // One millisecond short of the TTL is still a hit.
    cache
        .clock()
        .advance(UNWRAPPED_DEK_TTL_MS - 1)
        .expect("in range");
    read(&cache, &kek, &blob);
    assert_eq!(cache.provider().unwraps(), 1, "the TTL has not elapsed");

    cache.clock().advance(1).expect("in range");
    read(&cache, &kek, &blob);
    assert_eq!(
        cache.provider().unwraps(),
        2,
        "exactly one more unwrap once the entry expired"
    );
    assert_eq!(cache.len(), 1, "the expired entry was replaced, not kept");
}

/// A different wrapped blob is a different entry, not a hit on the first.
///
/// ⚠️ **The mistake this rules out is keying by topic.** A reader meets
/// objects sealed under every DEK the topic ever rotated through; a per-topic
/// key would return the wrong DEK for all but the newest, which is a region
/// that fails to open — or, if the two happened to agree, a cache that hides a
/// rotation.
#[test]
fn a_different_wrapped_blob_is_a_different_entry() {
    let kek = key_id();
    let cache = cache();
    let first = wrapped(&kek, 0x33);
    let second = wrapped(&kek, 0x44);

    assert_eq!(read(&cache, &kek, &first), 0x33);
    assert_eq!(read(&cache, &kek, &second), 0x44);
    assert_eq!(cache.provider().unwraps(), 2);
    assert_eq!(cache.len(), 2, "two blobs, two entries");

    // And both are now hits.
    assert_eq!(read(&cache, &kek, &first), 0x33);
    assert_eq!(read(&cache, &kek, &second), 0x44);
    assert_eq!(cache.provider().unwraps(), 2);
}

/// The same bytes under a different KEK are a different entry, and the
/// provider refuses them — a cached DEK is never served across key domains.
#[test]
fn the_key_id_is_part_of_the_entry_identity() {
    let kek = key_id();
    let cache = cache();
    let blob = wrapped(&kek, 0x55);
    read(&cache, &kek, &blob);

    let other = KeyId::new("projects/p/locations/l/keyRings/r/cryptoKeys/k").expect("non-empty");
    let refused = block_on(cache.with_dek(&other, &blob, |dek: &Dek| dek.expose()[0]));

    assert!(
        refused.is_err(),
        "a blob wrapped under one KEK must not be served from another's cache entry"
    );
    assert_eq!(
        cache.provider().unwraps(),
        2,
        "the second was a real attempt"
    );
    assert_eq!(cache.len(), 1, "a refused unwrap caches nothing");
    assert!(!cache.is_empty());
}

/// A wrapped blob for the DEK made of a 32-byte pattern derived from `n`, so a
/// test can make as many distinct blobs as it likes.
fn nth_blob(key_id: &KeyId, n: usize) -> WrappedKey {
    let provider = FakeKeyProvider::new();
    let mut material = [0xC3u8; DEK_BYTES];
    material[..8].copy_from_slice(&(n as u64).to_be_bytes());
    let plaintext = Redacted::new(material.to_vec());
    block_on(provider.wrap(key_id, &plaintext)).expect("the fake provider wraps")
}

/// An entry nobody ever looks up again is gone after a later insert past its
/// TTL.
///
/// ⚠️ **This is the leak the review found, as a test.** Expiry is noticed by a
/// lookup and eviction by an insert; an abandoned entry is reached by neither
/// unless an insert sweeps it, which is what `Entries::make_room` now does.
/// Without that sweep this test fails with `len() == 2` — the abandoned DEK
/// resident forever with nothing that would ever wipe it.
#[test]
fn an_abandoned_entry_is_swept_by_a_later_insert() {
    let kek = key_id();
    let cache = cache();

    read(&cache, &kek, &wrapped(&kek, 0x77));
    assert_eq!(cache.len(), 1);

    // Nobody ever asks for that blob again.
    cache
        .clock()
        .advance(UNWRAPPED_DEK_TTL_MS)
        .expect("in range");

    // An unrelated read. Its insert is what has to do the sweeping.
    read(&cache, &kek, &wrapped(&kek, 0x88));

    assert_eq!(
        cache.len(),
        1,
        "the expired entry nobody looked up must not still be resident"
    );
}

/// The cache never exceeds its capacity, however many distinct blobs are read.
///
/// ⚠️ Every blob here is *fresh* — the clock never moves — so nothing is
/// expired and the capacity is the only thing that can hold the size down.
/// That separates this assertion from the sweep above: it is the least-
/// recently-used eviction being tested, not the TTL.
#[test]
fn the_cache_never_exceeds_its_capacity() {
    let kek = key_id();
    let cache = cache();

    for n in 0..(UNWRAPPED_DEK_CACHE_ENTRIES + 50) {
        read(&cache, &kek, &nth_blob(&kek, n));
        assert!(
            cache.len() <= UNWRAPPED_DEK_CACHE_ENTRIES,
            "cache grew to {} past the capacity of {UNWRAPPED_DEK_CACHE_ENTRIES}",
            cache.len()
        );
    }

    assert_eq!(cache.len(), UNWRAPPED_DEK_CACHE_ENTRIES);
}

/// Eviction takes an expired entry before it takes a live one.
///
/// ⚠️ The ordering matters and is not an optimisation: evicting a live entry
/// while an expired one sits beside it costs a KMS call for a DEK that was
/// still valid *and* leaves key material resident past its TTL. Both halves of
/// `make_room` are needed, in that order.
#[test]
fn eviction_prefers_expired_entries_over_live_ones() {
    let kek = key_id();
    let cache = cache();

    // One blob read early, then left alone — it will be the oldest by
    // recency as well as expired, so this alone would not separate the two
    // policies.
    let stale = wrapped(&kek, 0x99);
    read(&cache, &kek, &stale);

    cache
        .clock()
        .advance(UNWRAPPED_DEK_TTL_MS)
        .expect("in range");

    // Fill to exactly capacity with live blobs. The very first of these is now
    // the least recently used *live* entry, so an eviction that ignored expiry
    // would take it.
    let first_live = nth_blob(&kek, 0);
    read(&cache, &kek, &first_live);
    for n in 1..UNWRAPPED_DEK_CACHE_ENTRIES {
        read(&cache, &kek, &nth_blob(&kek, n));
    }
    assert_eq!(cache.len(), UNWRAPPED_DEK_CACHE_ENTRIES);

    // The stale entry is gone; the live one that would have been the LRU
    // victim is still a hit.
    let before = cache.provider().unwraps();
    read(&cache, &kek, &first_live);
    assert_eq!(
        cache.provider().unwraps(),
        before,
        "the oldest live entry must have survived, because an expired one was \
         there to take instead"
    );
}

/// Eviction takes the *least recently used* live entry, not an arbitrary one.
///
/// # ⚠️ What this rests on
///
/// For the real code the assertion is **deterministic**: with the clock never
/// moving nothing expires, so `make_room` falls through to the recency
/// comparison, and the victims are exactly the entries not touched in the
/// second pass.
///
/// Against the failure it guards — a recency counter that does not advance, so
/// every entry ties and the victim is whichever the map iterates first — it is
/// not a proof but is not close: the tie-broken order is bucket order, which a
/// randomly seeded `HashMap` makes uncorrelated with insertion order, so the
/// chance that the evicted set avoids all `TOUCHED` touched entries is about
/// `0.75^256`. That is not a flake rate to reason about.
///
/// ⚠️ It deliberately says nothing about *which* live entry is best to evict.
/// LRU is a policy; what makes it worth a test is that the alternative here is
/// not a different policy but no policy at all.
#[test]
fn eviction_takes_the_least_recently_used_live_entry() {
    /// How many entries are touched, and how many new blobs are then read.
    const TOUCHED: usize = 256;

    let kek = key_id();
    let cache = cache();

    // Fill to capacity. Nothing expires: the clock never moves.
    for n in 0..UNWRAPPED_DEK_CACHE_ENTRIES {
        read(&cache, &kek, &nth_blob(&kek, n));
    }
    assert_eq!(cache.len(), UNWRAPPED_DEK_CACHE_ENTRIES);

    // Touch the oldest `TOUCHED`, making them the most recently used.
    for n in 0..TOUCHED {
        read(&cache, &kek, &nth_blob(&kek, n));
    }

    // `TOUCHED` new blobs, so `TOUCHED` evictions.
    for n in 0..TOUCHED {
        read(
            &cache,
            &kek,
            &nth_blob(&kek, UNWRAPPED_DEK_CACHE_ENTRIES + n),
        );
    }
    assert_eq!(cache.len(), UNWRAPPED_DEK_CACHE_ENTRIES);

    // Every touched entry survived.
    let before = cache.provider().unwraps();
    for n in 0..TOUCHED {
        read(&cache, &kek, &nth_blob(&kek, n));
    }
    assert_eq!(
        cache.provider().unwraps(),
        before,
        "an entry used since the fill must not have been the victim"
    );

    // And the least recently used one did not.
    read(&cache, &kek, &nth_blob(&kek, TOUCHED));
    assert_eq!(
        cache.provider().unwraps(),
        before + 1,
        "the least recently used entry must have been evicted"
    );
}
