//! `NFR-33`: KMS calls follow DEK rotation, not produce volume.

// Sites are on values this test constructed from literals it controls.
#![allow(clippy::expect_used)]

use crate::support::{CountingKeyProvider, block_on, key_id};
use oqueue_core::{Dek, FakeClock, FakeKeyProvider, KeyId, OperationalMetrics, TopicId};
use oqueue_crypto::{DEK_MAX_AGE_MS, DEK_MAX_SEALED_BYTES, DekCache, FakeEntropy};

type TestCache = DekCache<FakeClock, CountingKeyProvider<FakeKeyProvider>, FakeEntropy>;

fn cache() -> TestCache {
    DekCache::new(
        FakeClock::new(),
        CountingKeyProvider::new(FakeKeyProvider::new()),
        FakeEntropy::new(),
    )
}

fn topic() -> TopicId {
    TopicId::new("orders").expect("non-empty")
}

/// One region's worth of produce, charged against the live DEK.
fn seal(cache: &TestCache, topic: &TopicId, key_id: &KeyId, bytes: u64) {
    block_on(
        cache.with_live_dek(topic, key_id, bytes, |dek, id, wrapped| {
            // ⚠️ Asserted inside the closure, because this is the only place the
            // three arrive as a set: a footer written from a DEK of one rotation
            // and a wrapped blob of another would be an object nothing can open.
            assert_eq!(dek.expose().len(), 32);
            assert!(!wrapped.as_redacted().expose().is_empty());
            assert_eq!(id, key_id, "the cache must not quietly change the KEK");
        }),
    )
    .expect("the fake provider wraps");
}

/// One mebibyte, as a function rather than a `const` — a numeric `const` is a
/// threshold `check-drift.sh` asks to have pinned, and this is a unit.
const fn mib() -> u64 {
    1_048_576
}

/// The requirement, in one test, in the three parts it is made of.
///
/// ⚠️ **A tenfold difference in produce volume, not a bigger absolute one.**
/// The failure this guards against is a cache that calls the KMS per flush, or
/// per region, or per megabyte; any of those makes the count scale with
/// volume, and only comparing two volumes can tell that apart from a count
/// that happens to be low.
#[test]
fn kms_calls_follow_rotation_not_produce_volume() {
    let kek = key_id();

    // ── part 1: volume does not move the count ─────────────────────────────
    let thin = cache();
    for _ in 0..10 {
        seal(&thin, &topic(), &kek, mib());
    }
    let thick = cache();
    for _ in 0..100 {
        seal(&thick, &topic(), &kek, mib());
    }

    assert_eq!(
        thin.provider().wraps(),
        thick.provider().wraps(),
        "ten times the produce volume must cost the same number of KMS wraps"
    );
    assert_eq!(thick.provider().wraps(), 1, "one wrap, for the first mint");
    assert_eq!(thick.sealed_bytes(&topic()), Some(100 * mib()));

    // ── part 2: the byte bound rotates, exactly once ───────────────────────
    // ⚠️ One charge that takes the DEK over its bound. The bound is checked
    // *before* the charge, so this call still uses the incumbent and it is the
    // next one that rotates.
    seal(&thick, &topic(), &kek, DEK_MAX_SEALED_BYTES);
    assert_eq!(
        thick.provider().wraps(),
        1,
        "crossing is not itself a rotation"
    );

    seal(&thick, &topic(), &kek, mib());
    assert_eq!(
        thick.provider().wraps(),
        2,
        "exactly one more wrap, for the DEK the byte bound retired"
    );
    assert_eq!(
        thick.sealed_bytes(&topic()),
        Some(mib()),
        "the fresh DEK starts its own count"
    );

    // ── part 3: the time bound rotates, exactly once ───────────────────────
    // A `FakeClock`, so seven days cost no wall-clock time at all.
    thick
        .clock()
        .advance(DEK_MAX_AGE_MS)
        .expect("seven days is in range");
    seal(&thick, &topic(), &kek, mib());
    assert_eq!(
        thick.provider().wraps(),
        3,
        "exactly one more wrap, for the DEK the age bound retired"
    );

    // And volume under the new DEK still costs nothing.
    for _ in 0..50 {
        seal(&thick, &topic(), &kek, mib());
    }
    assert_eq!(thick.provider().wraps(), 3);
    assert_eq!(thick.mints(&topic()), 3);
}

#[test]
fn metrics_distinguish_a_rotation_miss_from_a_reused_dek_hit() {
    let metrics = OperationalMetrics::default();
    let cache = cache().with_metrics(metrics.clone());
    let kek = key_id();
    let name = topic();

    seal(&cache, &name, &kek, mib());
    seal(&cache, &name, &kek, mib());

    let snapshot = metrics.snapshot();
    assert_eq!(snapshot.encryption_cache_misses, 1);
    assert_eq!(snapshot.encryption_cache_hits, 1);
}

/// Both bounds are pinned at the exact byte and the exact millisecond.
///
/// ⚠️ **The edge is the whole of the decision.** `ADR-0050` point 3 chose
/// *64 GiB* and *7 days*, and a comparison that is off by one is a cache
/// rotating a byte early or a day late while every count-based assertion above
/// still passes. `usable` reads `sealed_bytes < DEK_MAX_SEALED_BYTES`, so the
/// bound is the first value that is **not** allowed — exactly the bound may be
/// reached, one past it may not.
#[test]
fn the_rotation_bounds_are_pinned_at_their_exact_edges() {
    let kek = key_id();

    // ── the byte bound ─────────────────────────────────────────────────────
    // Charging exactly the bound leaves `sealed_bytes == DEK_MAX_SEALED_BYTES`,
    // which is not `<`, so the *next* call rotates. Charging one byte less
    // leaves it under, and the next call does not.
    let at = cache();
    seal(&at, &topic(), &kek, DEK_MAX_SEALED_BYTES - 1);
    seal(&at, &topic(), &kek, 0);
    assert_eq!(
        at.provider().wraps(),
        1,
        "one byte short of the bound must not rotate"
    );
    assert_eq!(at.sealed_bytes(&topic()), Some(DEK_MAX_SEALED_BYTES - 1));

    let over = cache();
    seal(&over, &topic(), &kek, DEK_MAX_SEALED_BYTES);
    seal(&over, &topic(), &kek, 0);
    assert_eq!(
        over.provider().wraps(),
        2,
        "exactly the bound is one byte too many for the next seal"
    );

    // ── the age bound ──────────────────────────────────────────────────────
    let young = cache();
    seal(&young, &topic(), &kek, 0);
    young.clock().advance(DEK_MAX_AGE_MS - 1).expect("in range");
    seal(&young, &topic(), &kek, 0);
    assert_eq!(
        young.provider().wraps(),
        1,
        "one millisecond short of seven days must not rotate"
    );

    let old = cache();
    seal(&old, &topic(), &kek, 0);
    old.clock().advance(DEK_MAX_AGE_MS).expect("in range");
    seal(&old, &topic(), &kek, 0);
    assert_eq!(
        old.provider().wraps(),
        2,
        "exactly seven days old is too old to seal under"
    );
}

/// Rotation **replaces** the live DEK; the old one is not reachable from the
/// cache afterwards.
///
/// ⚠️ **Asserted against the cache's own state**, never by looking at the
/// memory the old [`Dek`] occupied: that memory is freed, reading it is
/// undefined behaviour, and a test that appeared to find zeroes there would be
/// asserting about the allocator. What can be checked is that the cache holds
/// one entry, and that the entry is the *new* key and not the old one.
#[test]
fn rotation_replaces_the_live_dek() {
    let kek = key_id();
    let cache = cache();

    // ⚠️ Predictable only because `FakeEntropy` is a counter and says loudly
    // that it is not entropy. This is the whole reason the seam exists.
    let first = Dek::new(FakeEntropy::block(0));
    let second = Dek::new(FakeEntropy::block(1));

    seal(&cache, &topic(), &kek, mib());
    assert!(cache.live_dek_is(&topic(), &first));
    assert!(!cache.live_dek_is(&topic(), &second));
    assert_eq!(cache.mints(&topic()), 1);

    seal(&cache, &topic(), &kek, DEK_MAX_SEALED_BYTES);
    seal(&cache, &topic(), &kek, mib());

    assert!(
        !cache.live_dek_is(&topic(), &first),
        "the retired DEK must not still be the live one"
    );
    assert!(cache.live_dek_is(&topic(), &second));
    assert_eq!(
        cache.live_topics(),
        1,
        "a rotation replaces the entry rather than accumulating a second live \
         key for the topic -- which is what makes the old one's `Drop`, and so \
         its zeroization, unavoidable"
    );
    assert_eq!(cache.mints(&topic()), 2);
}

/// Two topics do not share a DEK, and neither one's rotation touches the
/// other.
#[test]
fn each_topic_has_its_own_dek() {
    let kek = key_id();
    let cache = cache();
    let orders = topic();
    let payments = TopicId::new("payments").expect("non-empty");

    seal(&cache, &orders, &kek, mib());
    seal(&cache, &payments, &kek, mib());

    assert_eq!(
        cache.provider().wraps(),
        2,
        "one wrap per topic, not per region"
    );
    assert_eq!(cache.live_topics(), 2);
    assert!(cache.live_dek_is(&orders, &Dek::new(FakeEntropy::block(0))));
    assert!(cache.live_dek_is(&payments, &Dek::new(FakeEntropy::block(1))));

    seal(&cache, &orders, &kek, DEK_MAX_SEALED_BYTES);
    seal(&cache, &orders, &kek, mib());
    assert_eq!(cache.mints(&orders), 2);
    assert_eq!(
        cache.mints(&payments),
        1,
        "one topic's rotation is not another topic's"
    );
}

/// A topic whose KEK changes gets a new DEK rather than a relabelled one.
///
/// ⚠️ The dangerous alternative is keeping the incumbent: the footer would
/// then name the new KEK while carrying a blob wrapped under the old one, and
/// the region would be unopenable — or, worse under BYOK, would name another
/// tenant's key domain.
#[test]
fn a_changed_kek_is_a_new_dek() {
    let cache = cache();
    seal(&cache, &topic(), &key_id(), mib());

    let other = KeyId::new("projects/p/locations/l/keyRings/r/cryptoKeys/k").expect("non-empty");
    seal(&cache, &topic(), &other, mib());

    assert_eq!(cache.provider().wraps(), 2);
    assert!(cache.holds_live_dek(&topic(), &other));
    assert!(!cache.holds_live_dek(&topic(), &key_id()));
}
