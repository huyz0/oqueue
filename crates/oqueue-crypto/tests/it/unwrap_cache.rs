//! The read side: many objects under one DEK cost one `unwrap`.

// Sites are on values this test constructed from literals it controls.
#![allow(clippy::expect_used)]

use crate::support::{CountingKeyProvider, block_on, key_id};
use oqueue_core::{
    DEK_BYTES, Dek, FakeClock, FakeKeyProvider, KeyId, KeyProvider as _, Redacted, WrappedKey,
};
use oqueue_crypto::{UNWRAPPED_DEK_TTL_MS, UnwrappedDekCache};

type TestCache = UnwrappedDekCache<FakeClock, CountingKeyProvider<FakeKeyProvider>>;

fn cache() -> TestCache {
    UnwrappedDekCache::new(
        FakeClock::new(),
        CountingKeyProvider::new(FakeKeyProvider::new()),
    )
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
