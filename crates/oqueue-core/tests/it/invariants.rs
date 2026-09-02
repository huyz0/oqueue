//! Each identifier's documented invariant, asserted to be **unproducible**.
//!
//! ⚠️ These are deliberately not constructor/accessor round trips. A round trip
//! asserts that what went in comes out, which every newtype satisfies whether or
//! not it validates anything — it would pass against a plain `struct
//! TopicId(String)` with no checking at all, and so constrains nothing. What
//! each test below asserts instead is that **no input produces a value
//! violating the invariant**: the constructor is driven with arbitrary input and
//! every `Ok` is inspected.

// ⚠️ The workspace denies `expect_used` and `unwrap_used`, and the root
// `Cargo.toml` says the allowance belongs where a reader can see which tests
// took it. This is that place. Every `expect` below is on a value the test
// itself just constructed from a generator constrained to the valid range, so a
// panic here means the generator is wrong, not the code under test.
#![allow(clippy::expect_used)]

use oqueue_core::{
    Error, ObjectKey, Offset, PartitionId, Principal, ProducerEpoch, ProducerId, Timestamp, TopicId,
};
use proptest::prelude::*;

proptest! {
    /// No string produces a `TopicId` wrapping an empty string.
    #[test]
    fn topic_id_is_never_empty(name in ".*") {
        match TopicId::new(name.clone()) {
            Ok(id) => prop_assert!(!id.as_str().is_empty()),
            Err(e) => {
                prop_assert!(name.is_empty());
                prop_assert_eq!(e, Error::EmptyTopicId);
            }
        }
    }

    /// No `i32` produces a negative `PartitionId`.
    #[test]
    fn partition_id_is_never_negative(index in any::<i32>()) {
        match PartitionId::new(index) {
            Ok(id) => prop_assert!(id.get() >= 0),
            Err(e) => {
                prop_assert!(index < 0);
                prop_assert_eq!(e, Error::NegativePartitionId { got: index });
            }
        }
    }

    /// No `i64` produces a negative `Offset`.
    #[test]
    fn offset_is_never_negative(value in any::<i64>()) {
        match Offset::new(value) {
            Ok(o) => prop_assert!(o.get() >= 0),
            Err(e) => {
                prop_assert!(value < 0);
                prop_assert_eq!(e, Error::NegativeOffset { got: value });
            }
        }
    }

    /// No `i64` produces a negative `ProducerId`.
    #[test]
    fn producer_id_is_never_negative(value in any::<i64>()) {
        match ProducerId::new(value) {
            Ok(id) => prop_assert!(id.get() >= 0),
            Err(e) => {
                prop_assert!(value < 0);
                prop_assert_eq!(e, Error::NegativeProducerId { got: value });
            }
        }
    }

    /// No `i16` produces a negative `ProducerEpoch`.
    #[test]
    fn producer_epoch_is_never_negative(value in any::<i16>()) {
        match ProducerEpoch::new(value) {
            Ok(epoch) => prop_assert!(epoch.get() >= 0),
            Err(e) => {
                prop_assert!(value < 0);
                prop_assert_eq!(e, Error::NegativeProducerEpoch { got: value });
            }
        }
    }

    /// ⚠️ The load-bearing one. No addition ever yields an offset **smaller
    /// than the one it started from** — which is exactly what a silent wrap
    /// would produce, and what M3's sequencing cannot survive.
    #[test]
    fn offset_addition_never_goes_backwards(base in 0i64.., delta in 0i64..) {
        let start = Offset::new(base).expect("base is non-negative");
        match start.add(delta) {
            Ok(sum) => {
                prop_assert!(sum >= start);
                prop_assert!(sum.get() >= 0);
                prop_assert_eq!(sum.get(), base + delta);
            }
            Err(e) => {
                prop_assert!(base.checked_add(delta).is_none());
                prop_assert_eq!(e, Error::OffsetOverflow { base, delta });
            }
        }
    }

    /// Advancing by a negative amount is refused rather than silently
    /// subtracting, so `add` cannot be used to walk an offset backwards.
    #[test]
    fn offset_addition_refuses_negative_delta(base in 0i64.., delta in i64::MIN..0) {
        let start = Offset::new(base).expect("base is non-negative");
        prop_assert_eq!(start.add(delta), Err(Error::NegativeOffsetDelta { got: delta }));
    }

    /// No string produces an `ObjectKey` wrapping an empty string.
    #[test]
    fn object_key_is_never_empty(key in ".*") {
        match ObjectKey::new(key.clone()) {
            Ok(k) => prop_assert!(!k.as_str().is_empty()),
            Err(e) => {
                prop_assert!(key.is_empty());
                prop_assert_eq!(e, Error::EmptyObjectKey);
            }
        }
    }

    /// No string produces a `Principal` wrapping an empty string.
    #[test]
    fn principal_is_never_empty(name in ".*") {
        match Principal::new(name.clone()) {
            Ok(p) => prop_assert!(!p.as_str().is_empty()),
            Err(e) => {
                prop_assert!(name.is_empty());
                prop_assert_eq!(e, Error::EmptyPrincipal);
            }
        }
    }
}

/// `Offset::ZERO` is the documented floor and satisfies the invariant.
#[test]
fn offset_zero_is_the_floor() {
    assert_eq!(Offset::ZERO.get(), 0);
    assert_eq!(Offset::new(0).expect("zero is valid"), Offset::ZERO);
}

/// Overflow at the boundary, stated as an example because the exact edge is
/// worth naming rather than leaving to a generator to find.
#[test]
fn offset_add_at_the_boundary_errors_rather_than_wrapping() {
    let max = Offset::new(i64::MAX).expect("i64::MAX is non-negative");
    assert_eq!(
        max.add(1),
        Err(Error::OffsetOverflow {
            base: i64::MAX,
            delta: 1
        })
    );
    assert_eq!(max.add(0).map(Offset::get), Ok(i64::MAX));
}

/// ⚠️ Mutation testing found these gaps; each assertion below kills a specific
/// surviving mutant that the property tests above let through.
///
/// The property tests assert what *cannot* be produced, which is the invariant.
/// They do not assert that an accessor returns what was put in, or that
/// `Display` renders anything — so `as_str -> "xyzzy"` and
/// `fmt -> Ok(Default::default())` both survived. Those are not invariant
/// violations, they are the ordinary correctness a round trip catches, and
/// `M0.5` was right that a round trip alone would have constrained nothing.
/// Both are needed.
mod kills_surviving_mutants {
    use super::{
        Error, ObjectKey, Offset, PartitionId, Principal, ProducerEpoch, ProducerId, Timestamp,
        TopicId,
    };
    use oqueue_core::{ByteRange, CommittedSpan, ProducerIdentity};

    #[test]
    fn accessors_return_what_was_constructed() {
        assert_eq!(TopicId::new("orders").expect("valid").as_str(), "orders");
        assert_eq!(
            ObjectKey::new("a/b.seg").expect("valid").as_str(),
            "a/b.seg"
        );
        assert_eq!(PartitionId::new(7).expect("valid").get(), 7);
        assert_eq!(Offset::new(42).expect("valid").get(), 42);
        assert_eq!(Timestamp::from_millis(9).expect("valid").as_millis(), 9);
        assert_eq!(ProducerId::new(11).expect("valid").get(), 11);
        assert_eq!(ProducerEpoch::new(3).expect("valid").get(), 3);
        assert_eq!(Principal::new("alice").expect("valid").as_str(), "alice");
    }

    /// ⚠️ **`ProducerIdentity`'s own three accessors, and `CommittedSpan`'s
    /// `producer()` — mutation testing found each unguarded**: an accessor
    /// mutated to a constant (`sequence -> 0`) or `producer()` mutated to
    /// always answer `None` both survived every other test in the tree,
    /// because `CommittedSpan`/`MetadataEntry`'s derived `PartialEq` compares
    /// the raw field directly and never calls the accessor at all — a
    /// round-trip test proves the *field* survives a log, not that the
    /// *accessor* reads it back.
    #[test]
    fn producer_identity_accessors_return_what_was_constructed() {
        let id = ProducerId::new(11).expect("valid");
        let epoch = ProducerEpoch::new(3).expect("valid");
        let identity = ProducerIdentity::new(id, epoch, 42);
        assert_eq!(identity.id(), id);
        assert_eq!(identity.epoch(), epoch);
        assert_eq!(identity.sequence(), 42);

        let span = CommittedSpan::new(
            TopicId::new("orders").expect("valid"),
            PartitionId::new(0).expect("valid"),
            2,
            ByteRange::Full,
            Some(identity),
        );
        assert_eq!(span.producer(), Some(identity));

        let bare = CommittedSpan::new(
            TopicId::new("orders").expect("valid"),
            PartitionId::new(0).expect("valid"),
            2,
            ByteRange::Full,
            None,
        );
        assert_eq!(bare.producer(), None);
    }

    #[test]
    fn display_renders_the_value() {
        assert_eq!(TopicId::new("orders").expect("valid").to_string(), "orders");
        assert_eq!(
            ObjectKey::new("a/b.seg").expect("valid").to_string(),
            "a/b.seg"
        );
        assert_eq!(PartitionId::new(7).expect("valid").to_string(), "7");
        assert_eq!(Offset::new(42).expect("valid").to_string(), "42");
        assert_eq!(Timestamp::from_millis(9).expect("valid").to_string(), "9ms");
        assert_eq!(ProducerId::new(11).expect("valid").to_string(), "11");
        assert_eq!(ProducerEpoch::new(3).expect("valid").to_string(), "3");
        assert_eq!(Principal::new("alice").expect("valid").to_string(), "alice");
    }

    /// ⚠️ Zero is the boundary every `< 0` guard turns on, and `< ` mutated to
    /// `<=` rejects it. A generator reaches 0 only by luck; this does not.
    #[test]
    fn zero_is_valid_everywhere_it_should_be() {
        assert_eq!(PartitionId::new(0).expect("zero is a partition").get(), 0);
        assert_eq!(Offset::new(0).expect("zero is an offset").get(), 0);
        assert_eq!(Timestamp::from_millis(0).expect("epoch").as_millis(), 0);
        assert_eq!(ProducerId::new(0).expect("zero is a producer id").get(), 0);
        assert_eq!(ProducerEpoch::new(0).expect("zero is an epoch").get(), 0);
        assert_eq!(ProducerEpoch::ZERO.get(), 0);
        let start = Offset::new(5).expect("valid");
        assert_eq!(
            start.add(0).map(Offset::get),
            Ok(5),
            "advancing by zero is valid"
        );
    }

    /// And that the guards still reject one below the boundary.
    #[test]
    fn minus_one_is_rejected_everywhere() {
        assert_eq!(
            PartitionId::new(-1),
            Err(Error::NegativePartitionId { got: -1 })
        );
        assert_eq!(Offset::new(-1), Err(Error::NegativeOffset { got: -1 }));
        assert_eq!(
            Timestamp::from_millis(-1),
            Err(Error::NegativeTimestamp { got: -1 })
        );
        assert_eq!(
            ProducerId::new(-1),
            Err(Error::NegativeProducerId { got: -1 })
        );
        assert_eq!(
            ProducerEpoch::new(-1),
            Err(Error::NegativeProducerEpoch { got: -1 })
        );
    }
}
