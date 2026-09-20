//! ⚠️ `code-structure.md` rule 26's allowance, taken here: a test may
//! unwrap. Every `expect` below is over a value the test itself
//! constructed in range.
#![allow(clippy::expect_used, clippy::unwrap_used)]

use super::{
    MAX_OBJECT_SEQUENCE, MAX_REGION_INDEX, MAX_WRITER_EPOCH, NONCE_BYTES, Nonce, NonceMinter,
    NonceSource, ParsedNonce, WriterEpoch,
};
use crate::{CoordinatorEpoch, Error};
use proptest::prelude::*;
use std::collections::HashSet;

/// The nonce for `(epoch, object, region)`, for the layout tests.
///
/// ⚠️ **Uses `NonceMinter::at`, the `#[cfg(test)]` fast-forward**, because
/// advancing a real minter to object `0x0607_0809_0a` would take 26 billion
/// calls — a four-minute test run, measured, when this helper looped. Nothing
/// production can reach `at`. ⚠️ **The property test does not use this helper
/// and does not fast-forward anything**: `no_call_sequence_repeats_a_nonce`
/// runs entirely on the public API, which is the review finding that put the
/// counter inside `NonceMinter` in the first place.
fn nonce(epoch: u64, object: u64, region: u32) -> Nonce {
    let mut minter = NonceMinter::at(epoch, object);
    let mut source = minter.next_object().expect("in range");
    for index in 0..region {
        source.for_region(index).expect("in range and in order");
    }
    source.for_region(region).expect("in range and in order")
}

fn epoch(value: u64) -> WriterEpoch {
    WriterEpoch::from_coordinator_epoch(CoordinatorEpoch::new(value))
}

#[test]
fn the_bit_layout_is_epoch_then_sequence_then_region() {
    let value = nonce(0x01_02_03_04_05, 0x06_07_08_09_0a, 0x0b_0c);
    assert_eq!(
        value.as_bytes(),
        &[
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0a, 0x0b, 0x0c
        ]
    );
}

#[test]
fn a_nonce_is_ninety_six_bits() {
    assert_eq!(NONCE_BYTES, 12);
    assert_eq!(nonce(0, 0, 0).as_bytes().len(), 12);
}

/// ⚠️ **Every byte `0xff`, region index included.** An earlier version asked
/// for region 0 here and asserted bytes 10-11 were zero, which would have
/// stayed green if the region field had narrowed to 8 bits — the one thing
/// this test exists to catch.
#[test]
fn the_maximum_in_range_triple_fills_every_bit() {
    let mut minter = NonceMinter::at(MAX_WRITER_EPOCH, MAX_OBJECT_SEQUENCE);
    let mut source = minter.next_object().expect("at the ceiling");
    for index in 0..MAX_REGION_INDEX {
        source.for_region(index).expect("in order");
    }
    let last = source
        .for_region(MAX_REGION_INDEX)
        .expect("the last region");
    assert_eq!(last.as_bytes(), &[0xff; NONCE_BYTES]);
}

#[test]
fn a_writer_epoch_past_the_ceiling_is_refused_not_truncated() {
    assert_eq!(
        NonceMinter::new(epoch(MAX_WRITER_EPOCH + 1)).unwrap_err(),
        Error::NonceWriterEpochOutOfRange {
            got: MAX_WRITER_EPOCH + 1
        }
    );
    assert!(NonceMinter::new(epoch(u64::MAX)).is_err());
}

#[test]
fn an_exhausted_object_sequence_is_refused_not_wrapped() {
    let mut minter = NonceMinter::at(0, MAX_OBJECT_SEQUENCE);
    // The last object this writer may ever seal.
    assert!(minter.next_object().is_ok());
    assert_eq!(
        minter.next_object().unwrap_err(),
        Error::NonceObjectSequenceOutOfRange {
            got: MAX_OBJECT_SEQUENCE + 1
        }
    );
    // ⚠️ And it stays refused rather than wrapping to an object already used.
    assert!(minter.next_object().is_err());
}

#[test]
fn a_region_index_past_the_ceiling_is_refused_not_truncated() {
    let mut minter = NonceMinter::new(epoch(0)).expect("in range");
    let mut source = minter.next_object().expect("object zero");
    assert_eq!(
        source.for_region(MAX_REGION_INDEX + 1).unwrap_err(),
        Error::NonceRegionOutOfRange {
            got: MAX_REGION_INDEX + 1
        }
    );
    // ⚠️ The refusal did not advance the counter, so the object can still
    // be sealed correctly after a caller's bad call.
    assert_eq!(source.minted(), 0);
    assert!(source.for_region(0).is_ok());
}

#[test]
fn asking_twice_for_one_region_is_refused() {
    let mut minter = NonceMinter::new(epoch(7)).expect("in range");
    let mut source = minter.next_object().expect("object zero");
    let first = source.for_region(0).expect("region zero");
    assert_eq!(
        source.for_region(0).unwrap_err(),
        Error::NonceRegionOutOfOrder {
            expected: 1,
            got: 0
        }
    );
    // And the source is still usable for the region that really is next.
    let second = source.for_region(1).expect("region one");
    assert_ne!(first, second);
}

#[test]
fn a_skipped_region_is_refused() {
    let mut minter = NonceMinter::new(epoch(7)).expect("in range");
    let mut source = minter.next_object().expect("object zero");
    assert_eq!(
        source.for_region(1).unwrap_err(),
        Error::NonceRegionOutOfOrder {
            expected: 0,
            got: 1
        }
    );
}

#[test]
fn two_regions_of_one_object_differ() {
    let mut minter = NonceMinter::new(epoch(3)).expect("in range");
    let mut source = minter.next_object().expect("object zero");
    let a = source.for_region(0).expect("region zero");
    let b = source.for_region(1).expect("region one");
    assert_ne!(a, b);
}

#[test]
fn two_objects_of_one_writer_differ() {
    let mut minter = NonceMinter::new(epoch(3)).expect("in range");
    let mut first = minter.next_object().expect("object zero");
    let mut second = minter.next_object().expect("object one");
    assert_eq!(minter.issued(), 2);
    assert_ne!(
        first.for_region(0).expect("region zero"),
        second.for_region(0).expect("region zero")
    );
}

#[test]
fn two_writers_at_the_same_object_and_region_differ() {
    assert_ne!(nonce(3, 4, 0), nonce(4, 4, 0));
}

#[test]
fn the_maximum_in_range_values_do_not_collide() {
    let max = nonce(MAX_WRITER_EPOCH, 3, MAX_REGION_INDEX);
    let one_less_epoch = nonce(MAX_WRITER_EPOCH - 1, 3, MAX_REGION_INDEX);
    let one_less_object = nonce(MAX_WRITER_EPOCH, 2, MAX_REGION_INDEX);
    let one_less_region = nonce(MAX_WRITER_EPOCH, 3, MAX_REGION_INDEX - 1);
    let seen: HashSet<Nonce> = [max, one_less_epoch, one_less_object, one_less_region].into();
    assert_eq!(seen.len(), 4, "the top of each field must stay distinct");
}

/// ⚠️ **What the type still cannot enforce, named so nobody assumes it does.**
/// Two minters at one writer epoch — which is what a process restarted with
/// its predecessor's epoch is — hand out the same nonces from their first
/// call. No code in `oqueue-core` can tell: it names no process and reads no
/// clock. Backlog row `M8.10` owns minting an epoch that survives a restart,
/// and obligation (1) in the module docs is this sentence.
#[test]
fn a_restarted_writer_reusing_its_epoch_repeats_nonces() {
    let mut before = NonceMinter::new(epoch(1)).expect("in range");
    let mut after = NonceMinter::new(epoch(1)).expect("in range");
    let mut first = before.next_object().expect("object zero");
    let mut second = after.next_object().expect("object zero again");
    assert_eq!(
        first.for_region(0).expect("region zero"),
        second.for_region(0).expect("region zero"),
        "the caller's obligation -- see M8.10, not this type"
    );
}

#[test]
fn a_restarted_writer_uses_a_new_coordinator_epoch() {
    let before_epoch = WriterEpoch::from_coordinator_epoch(CoordinatorEpoch::new(7));
    let after_epoch = WriterEpoch::from_coordinator_epoch(CoordinatorEpoch::new(8));
    let mut before = NonceMinter::new(before_epoch).expect("the fenced epoch is in range");
    let mut after = NonceMinter::new(after_epoch).expect("the fenced epoch is in range");

    let before_nonce = before
        .next_object()
        .expect("the first object is in range")
        .for_region(0)
        .expect("the first region is in range");
    let after_nonce = after
        .next_object()
        .expect("the first object is in range")
        .for_region(0)
        .expect("the first region is in range");

    assert_ne!(
        before_nonce.as_bytes(),
        after_nonce.as_bytes(),
        "a restarted writer must not reuse the predecessor's fenced epoch"
    );
}

/// Decoded bytes are a [`ParsedNonce`], and there is no way back to a
/// [`Nonce`] — `M8.3`/`M8.4` open with the first and seal with the second.
#[test]
fn a_decoded_nonce_is_a_different_type_than_a_minted_one() {
    let minted = nonce(5, 6, 7);
    let parsed = ParsedNonce::decode(*minted.as_bytes());
    assert_eq!(parsed.as_bytes(), minted.as_bytes());
}

/// One step an arbitrary call sequence can take.
///
/// ⚠️ **There is no operation that names an object sequence**, because the API
/// has none: a minter owns that counter. The harness keeps no counter of its
/// own either — an earlier version did, and that was the review finding: a
/// harness-side counter is a test compensating for a hole in the API instead
/// of exercising it.
#[derive(Debug, Clone, Copy)]
enum Op {
    /// Take the next object from the minter for this writer epoch, opening
    /// that minter if this is the first time the epoch is used.
    ///
    /// ⚠️ **One minter per epoch, reused** — distinct epochs are module
    /// obligation (1), the one thing the type cannot enforce, and
    /// `a_restarted_writer_reusing_its_epoch_repeats_nonces` is where that
    /// boundary is asserted instead.
    TakeObject { epoch: usize },
    /// Ask source `which` (modulo how many are open) for its next region.
    NextRegion { which: usize },
    /// Ask source `which` for region `index`, whatever it is — most of
    /// these are refused, and a refusal must never yield a nonce.
    ArbitraryRegion { which: usize, index: u32 },
}

fn op() -> impl Strategy<Value = Op> {
    prop_oneof![
        // ⚠️ A small epoch range on purpose: collisions are what this test
        // hunts, and drawing from 2^40 would never revisit a writer, so the
        // property would hold vacuously.
        // Four distinct writer epochs -- see the comment above.
        (0usize..4).prop_map(|epoch| Op::TakeObject { epoch }),
        (0usize..8).prop_map(|which| Op::NextRegion { which }),
        (0usize..8, 0u32..6).prop_map(|(which, index)| Op::ArbitraryRegion { which, index }),
    ]
}

proptest! {
    /// `ADR-0050` point 2's property, and the `m8-complete.sh` leg that
    /// runs it by name: **no sequence of calls against this API can make
    /// one nonce come out twice.**
    ///
    /// The operations are over minters and sources only — take the next
    /// object from one of four writers, ask a source for its next region,
    /// ask a source for an arbitrary region index — interleaved in any
    /// order, with many sources live at once and sources of one writer
    /// interleaved with each other's. Nothing in the harness supplies a
    /// value the API would otherwise let a caller choose. Every nonce that
    /// is actually produced goes into a set; a repeat fails. A refused call
    /// must yield nothing.
    #[test]
    fn no_call_sequence_repeats_a_nonce(ops in prop::collection::vec(op(), 1..200)) {
        let mut minters: Vec<Option<NonceMinter>> = (0..4).map(|_| None).collect();
        let mut sources: Vec<NonceSource> = Vec::new();
        let mut seen: HashSet<[u8; NONCE_BYTES]> = HashSet::new();

        for step in ops {
            match step {
                Op::TakeObject { epoch } => {
                    let slot = &mut minters[epoch];
                    if slot.is_none() {
                        let epoch = u64::try_from(epoch).expect("a small index");
                        *slot = Some(NonceMinter::new(self::epoch(epoch)).expect("in range"));
                    }
                    let minter = slot.as_mut().expect("just opened");
                    sources.push(minter.next_object().expect("far below the ceiling"));
                }
                Op::NextRegion { which } | Op::ArbitraryRegion { which, .. } => {
                    if sources.is_empty() {
                        continue;
                    }
                    let slot = which % sources.len();
                    let source = &mut sources[slot];
                    let index = match step {
                        Op::ArbitraryRegion { index, .. } => index,
                        Op::NextRegion { .. } | Op::TakeObject { .. } => source.minted(),
                    };
                    let Ok(value) = source.for_region(index) else {
                        continue;
                    };
                    let bytes = *value.as_bytes();
                    prop_assert!(seen.insert(bytes), "a nonce repeated: {bytes:?}");
                }
            }
        }
    }

    /// The layout is injective: distinct in-range triples never collide.
    #[test]
    fn distinct_triples_never_collide(
        // ⚠️ **The whole range of each field** (`M8.2`'s second review
        // round): drawing the object sequence from `0..8` left 37 of its 40
        // bits never varied, so a layout that duplicated a byte of that
        // field survived this property. `nonce()` fast-forwards through
        // `NonceMinter::at`, so the wide range costs nothing.
        a in (0u64..=MAX_WRITER_EPOCH, 0u64..=MAX_OBJECT_SEQUENCE, 0u32..=MAX_REGION_INDEX),
        b in (0u64..=MAX_WRITER_EPOCH, 0u64..=MAX_OBJECT_SEQUENCE, 0u32..=MAX_REGION_INDEX),
    ) {
        let first = nonce(a.0, a.1, a.2);
        let second = nonce(b.0, b.1, b.2);
        prop_assert_eq!(a == b, first == second);
    }
}

/// ⚠️ **Literal, not derived from the constants themselves** (`M8.2`'s
/// mutants): every other range test says `MAX_REGION_INDEX + 1`, which moves
/// with the constant, so a ceiling that grew by one would keep them all
/// green. These are the widths the layout's injectivity argument rests on.
#[test]
fn the_ceilings_are_the_widths_the_layout_claims() {
    assert_eq!(MAX_WRITER_EPOCH, 1_099_511_627_775, "40 bits");
    assert_eq!(MAX_OBJECT_SEQUENCE, 1_099_511_627_775, "40 bits");
    assert_eq!(MAX_REGION_INDEX, 65_535, "16 bits");
    assert!(
        NonceMinter::new(epoch(1_099_511_627_776)).is_err(),
        "past 40 bits"
    );
    let mut minter = NonceMinter::at(0, 1_099_511_627_776);
    assert!(minter.next_object().is_err(), "past 40 bits");
    let mut minter = NonceMinter::new(epoch(0)).expect("in range");
    let mut source = minter.next_object().expect("object zero");
    assert!(source.for_region(65_536).is_err(), "past 16 bits");
    assert_eq!(source.minted(), 0, "a refusal mints nothing");
    source.for_region(0).expect("the first region");
    source.for_region(1).expect("the second");
    assert_eq!(source.minted(), 2, "and minting advances it");
}
