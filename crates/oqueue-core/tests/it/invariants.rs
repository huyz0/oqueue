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

use oqueue_core::{Error, ObjectKey, Offset, PartitionId, TopicId};
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
