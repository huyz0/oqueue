//! `KeyLayout`: computable keys, hash fan-out that distributes, and no
//! collision between distinct topic-partitions.

// The workspace denies `expect_used`; every site here is on a value this
// test just constructed from a literal it controls or a generator
// constrained to the valid range, so a panic means the test is wrong.
#![allow(clippy::expect_used)]

use oqueue_core::{KeyLayout, Offset, PartitionId, TopicId};
use proptest::prelude::*;
use std::num::{NonZeroU32, NonZeroU64};

fn topic(name: &str) -> TopicId {
    TopicId::new(name).expect("a non-empty topic name")
}

fn partition(index: i32) -> PartitionId {
    PartitionId::new(index).expect("a non-negative partition index")
}

fn offset(value: i64) -> Offset {
    Offset::new(value).expect("a non-negative offset")
}

const fn layout(alignment: u64, bucket_count: u32) -> KeyLayout {
    KeyLayout::new(
        NonZeroU64::new(alignment).expect("a non-zero alignment"),
        NonZeroU32::new(bucket_count).expect("a non-zero bucket count"),
    )
}

proptest! {
    /// Every offset in the same alignment quantum computes the same key —
    /// the object that would hold it, once written.
    #[test]
    fn offsets_in_the_same_quantum_compute_the_same_key(
        base in 0i64..1_000_000,
        within in 0i64..64,
    ) {
        let layout = layout(64, 16);
        let t = topic("orders");
        let p = partition(0);
        let quantum_start = (base / 64) * 64;
        let a = layout.object_key(&t, p, offset(quantum_start)).expect("valid key");
        let b = layout.object_key(&t, p, offset(quantum_start + within)).expect("valid key");
        prop_assert_eq!(a, b);
    }

    /// Two distinct `(topic, partition)` identities never compute the same
    /// key, regardless of hash bucket — the identity is embedded in the key
    /// itself, not only encoded through the hash.
    #[test]
    fn distinct_topic_partitions_never_collide(
        topic_a in "[a-z]{1,8}",
        partition_a in 0i32..64,
        topic_b in "[a-z]{1,8}",
        partition_b in 0i32..64,
        off in 0i64..1_000_000,
    ) {
        prop_assume!((topic_a.clone(), partition_a) != (topic_b.clone(), partition_b));
        let layout = layout(64, 4); // few buckets, so hash collisions are common and must not matter
        let key_a = layout
            .object_key(&topic(&topic_a), partition(partition_a), offset(off))
            .expect("valid key");
        let key_b = layout
            .object_key(&topic(&topic_b), partition(partition_b), offset(off))
            .expect("valid key");
        prop_assert_ne!(key_a, key_b);
    }

    /// The same `(topic, partition)` at different quanta never collides
    /// either, even though the bucket segment is identical for both.
    #[test]
    fn distinct_quanta_of_the_same_topic_partition_never_collide(
        quantum_a in 0i64..1000,
        quantum_b in 0i64..1000,
    ) {
        prop_assume!(quantum_a != quantum_b);
        let layout = layout(64, 4);
        let t = topic("orders");
        let p = partition(0);
        let key_a = layout.object_key(&t, p, offset(quantum_a * 64)).expect("valid key");
        let key_b = layout.object_key(&t, p, offset(quantum_b * 64)).expect("valid key");
        prop_assert_ne!(key_a, key_b);
    }
}

/// Fan-out actually distributes: many distinct topics against a small bucket
/// count exercise more than one bucket, not the same one every time.
///
/// ⚠️ Not a property test — "distributes" is a statistical claim about many
/// inputs together, not a per-input invariant a single proptest case can
/// state. Stated as one example with enough inputs that a hash collapsing
/// everything into one bucket would be immediately visible.
#[test]
fn many_distinct_topics_spread_across_more_than_one_bucket() {
    let layout = layout(64, 16);
    let p = partition(0);
    let keys: Vec<_> = (0..200)
        .map(|i| {
            layout
                .object_key(&topic(&format!("topic-{i}")), p, offset(0))
                .expect("valid key")
        })
        .collect();

    // The bucket is the key's first path segment.
    let buckets: std::collections::HashSet<&str> = keys
        .iter()
        .map(|k| {
            k.as_str()
                .split('/')
                .next()
                .expect("a key has a first segment")
        })
        .collect();

    assert!(
        buckets.len() > 1,
        "200 distinct topics against 16 buckets produced only {} distinct bucket(s)",
        buckets.len()
    );
}

/// Two logically identical calls (same topic, partition, and quantum)
/// compute identical keys — the function is pure.
#[test]
fn identical_inputs_always_compute_the_identical_key() {
    let layout = layout(4 * 1024 * 1024, 64);
    let t = topic("orders");
    let p = partition(3);
    let o = offset(123_456_789);
    assert_eq!(
        layout.object_key(&t, p, o).expect("valid key"),
        layout.object_key(&t, p, o).expect("valid key")
    );
}

/// The key contains no dependence on lexicographic adjacency: two offsets
/// one alignment quantum apart do not necessarily sort adjacently as
/// strings, and nothing in this module claims otherwise. Demonstrated with a
/// concrete pair whose bucket segments differ.
#[test]
fn keys_are_not_assumed_lexicographically_ordered() {
    let layout = layout(64, 1000);
    let a = layout
        .object_key(&topic("a"), partition(0), offset(0))
        .expect("valid key");
    let b = layout
        .object_key(&topic("b"), partition(0), offset(64))
        .expect("valid key");
    // Nothing is asserted about `a < b` or `a > b` — the point is that no
    // code anywhere, including this test, may rely on either.
    assert_ne!(a, b);
}
