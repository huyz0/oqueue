//! `ObjectRef`'s size budget, and the arithmetic a fetch resolves through it.

#![allow(clippy::expect_used)]

use oqueue_core::{ByteRange, ObjectKey, ObjectRef, Offset, TailEntry};

fn a_ref(base: i64, count: u32) -> ObjectRef {
    ObjectRef::new(
        ObjectKey::new("seg".to_owned()).expect("a valid key"),
        Offset::new(base).expect("a valid offset"),
        count,
    )
}

/// ⚠️ **The inline size, pinned rather than hoped for.** `M3.md` task 7 gives
/// this ~40 bytes, and this test fails if someone adds a field.
///
/// ⚠️ **It pins the inline half only.** Each entry also owns an [`ObjectKey`],
/// whose string data is on the heap and is cloned once per (object, partition)
/// span — so a bundle spanning 100 partitions holds 100 copies of one key, and
/// the real per-entry cost is well above the 40 bytes asserted here. The
/// growth arithmetic in `M3.md` (~14 GB/day, ~97 GB over a 7-day retention) is
/// computed against a per-entry figure this assertion therefore does *not*
/// fully guard. Interning the key, or keying entries per object rather than
/// per span, is what would make it whole; both are changes to how the index is
/// keyed rather than to this struct.
#[test]
fn object_ref_fits_its_forty_byte_budget() {
    assert_eq!(
        size_of::<ObjectRef>(),
        40,
        "ObjectRef left its ~40-byte budget — see M3.md task 7 and doc 14 §3"
    );
}

/// ⚠️ The tail entry is deliberately *larger*, and that is the tier's cost.
/// It is bounded by the window for exactly this reason.
#[test]
fn a_tail_entry_costs_its_inline_range() {
    assert!(
        size_of::<TailEntry>() > size_of::<ObjectRef>(),
        "a tail entry should cost more than the bare ref it demotes to"
    );
    assert_eq!(size_of::<TailEntry>(), size_of::<ObjectRef>() + 24);
}

/// The end offset is the base plus the count, and it does not wrap.
#[test]
fn end_offset_is_base_plus_count() {
    assert_eq!(
        a_ref(10, 5).end_offset().expect("in range"),
        Offset::new(15).expect("a valid offset")
    );
    assert_eq!(
        a_ref(0, 0).end_offset().expect("in range"),
        Offset::ZERO,
        "an empty contribution ends where it began"
    );
    a_ref(i64::MAX, 1)
        .end_offset()
        .expect_err("past i64 errors rather than wrapping");
}

/// ⚠️ `contains` is half-open — `[base, base + count)`. A closed upper bound
/// would make two adjacent objects both claim the boundary offset, and a
/// fetch resolving it would pick whichever the index happened to scan first.
#[test]
fn contains_is_half_open() {
    let entry = a_ref(10, 3);
    let at = |v: i64| {
        entry
            .contains(Offset::new(v).expect("a valid offset"))
            .expect("in range")
    };

    assert!(!at(9), "below the base");
    assert!(at(10), "the base itself");
    assert!(at(12), "the last record");
    assert!(!at(13), "the end offset belongs to the next object");
}

/// An empty contribution contains nothing, including its own base.
#[test]
fn an_empty_ref_contains_nothing() {
    let entry = a_ref(10, 0);
    assert!(
        !entry
            .contains(Offset::new(10).expect("a valid offset"))
            .expect("in range")
    );
}

/// Demoting drops the inline range and keeps everything a reader still needs
/// to find the object — the range then comes from the object's own footer.
#[test]
fn demoting_keeps_the_reference_and_drops_the_range() {
    let reference = a_ref(10, 3);
    let entry = TailEntry::new(reference.clone(), ByteRange::Full);

    assert_eq!(entry.bytes(), ByteRange::Full);
    assert_eq!(entry.reference(), &reference);
    assert_eq!(entry.demote(), reference);
}
