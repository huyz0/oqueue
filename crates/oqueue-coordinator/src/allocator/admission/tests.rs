//! `admission.rs`'s tests, split out at the 500-line limit.
//!
//! ⚠️ **Split rather than trimmed**, on `oqueue-codec/src/fetch/tests.rs`'s
//! own precedent: the module boundary the line limit is pointing at is
//! `classify`/`admit`'s own code versus the tests that pin it, not any one
//! test being cuttable — `M11.7` is what pushed this file over the line,
//! adding the epoch-fencing cases beside the sequence ones already here.

// A panic in a test harness is the test failing, which is what it is for.
#![allow(clippy::expect_used)]

use super::RejectReason;
use crate::allocator::Allocator;
use oqueue_core::{
    ByteRange, CommittedSpan, ObjectKey, PartitionId, ProducerEpoch, ProducerId, ProducerIdentity,
    TopicId,
};

fn topic() -> TopicId {
    TopicId::new("t".to_owned()).expect("a valid topic id")
}

fn partition() -> PartitionId {
    PartitionId::new(0).expect("a valid partition")
}

fn object() -> ObjectKey {
    ObjectKey::new("o".to_owned()).expect("a valid object key")
}

fn producer(id: i64) -> ProducerId {
    ProducerId::new(id).expect("a valid producer id")
}

fn identity(id: i64, sequence: i32) -> ProducerIdentity {
    ProducerIdentity::new(producer(id), ProducerEpoch::ZERO, sequence)
}

fn span_from(id: i64, sequence: i32, records: u32) -> CommittedSpan {
    CommittedSpan::new(
        topic(),
        partition(),
        records,
        ByteRange::Full,
        Some(identity(id, sequence)),
    )
}

/// Drives one span all the way through `admit` → `stage` → `apply`, so a
/// later `admit` call in the same test sees genuinely committed state —
/// not a hand-built fixture standing in for it.
fn commit(allocator: &mut Allocator, span: CommittedSpan) {
    let admission = allocator.admit(vec![span]);
    assert!(
        admission.rejected.is_empty(),
        "the fixture's own commit must not be rejected: {:?}",
        admission.rejected
    );
    assert!(
        admission.replayed.is_empty(),
        "the fixture's own commit must not be a replay"
    );
    let admitted: Vec<CommittedSpan> = admission.admitted.into_iter().map(|(_, s)| s).collect();
    let staged = allocator
        .stage(object(), admitted)
        .expect("a fresh allocator always has room");
    allocator.apply(staged);
}

#[test]
fn a_span_with_no_producer_identity_is_always_admitted() {
    let allocator = Allocator::new();
    let span = CommittedSpan::new(topic(), partition(), 1, ByteRange::Full, None);
    let admission = allocator.admit(vec![span]);
    assert_eq!(admission.admitted.len(), 1);
    assert!(admission.replayed.is_empty());
    assert!(admission.rejected.is_empty());
}

#[test]
fn a_producers_first_ever_span_is_admitted_at_sequence_zero() {
    let allocator = Allocator::new();
    let admission = allocator.admit(vec![span_from(1, 0, 1)]);
    assert_eq!(admission.admitted.len(), 1);
    assert!(admission.rejected.is_empty());
}

#[test]
fn a_producers_first_span_at_a_nonzero_sequence_is_rejected() {
    let allocator = Allocator::new();
    let admission = allocator.admit(vec![span_from(1, 5, 1)]);
    assert!(admission.admitted.is_empty());
    assert_eq!(
        admission.rejected,
        vec![(0, topic(), partition(), RejectReason::OutOfOrder)]
    );
}

#[test]
fn the_next_sequence_after_a_committed_one_is_admitted() {
    let mut allocator = Allocator::new();
    commit(&mut allocator, span_from(1, 0, 2));
    let admission = allocator.admit(vec![span_from(1, 1, 3)]);
    assert_eq!(admission.admitted.len(), 1);
    assert!(admission.rejected.is_empty());
}

#[test]
fn an_exact_replay_answers_the_recorded_offset_without_a_new_one() {
    let mut allocator = Allocator::new();
    commit(&mut allocator, span_from(1, 0, 2));
    let admission = allocator.admit(vec![span_from(1, 0, 2)]);
    assert!(admission.admitted.is_empty(), "no new offset is assigned");
    assert_eq!(admission.replayed.len(), 1);
    let (index, t, p, assignment) = &admission.replayed[0];
    assert_eq!(*index, 0);
    assert_eq!((t, p), (&topic(), &partition()));
    assert_eq!(assignment.base_offset(), oqueue_core::Offset::ZERO);
}

/// `M11.16`: every replay case above replays at offset zero, which cannot
/// distinguish "answers the recorded offset" from "always answers zero" —
/// a mutant hardcoding `Offset::ZERO` in place of the real recorded value
/// would pass every one of them. This one commits a second span first, so
/// the sequence being replayed is recorded at a genuinely nonzero offset,
/// and the replay must answer *that* value.
#[test]
fn a_replay_of_a_nonzero_recorded_offset_answers_that_offset_not_zero() {
    let mut allocator = Allocator::new();
    commit(&mut allocator, span_from(1, 0, 2)); // occupies offsets 0..2
    commit(&mut allocator, span_from(1, 1, 3)); // occupies offsets 2..5
    let admission = allocator.admit(vec![span_from(1, 1, 3)]);
    assert!(admission.admitted.is_empty(), "no new offset is assigned");
    assert_eq!(admission.replayed.len(), 1);
    let (index, t, p, assignment) = &admission.replayed[0];
    assert_eq!(*index, 0);
    assert_eq!((t, p), (&topic(), &partition()));
    assert_eq!(
        assignment.base_offset(),
        oqueue_core::Offset::new(2).expect("a valid offset")
    );
}

#[test]
fn a_replay_whose_record_count_does_not_match_is_a_duplicate_not_a_replay() {
    let mut allocator = Allocator::new();
    commit(&mut allocator, span_from(1, 0, 2));
    // Same sequence, different record count: not the same batch.
    let admission = allocator.admit(vec![span_from(1, 0, 99)]);
    assert!(admission.admitted.is_empty());
    assert!(admission.replayed.is_empty());
    assert_eq!(
        admission.rejected,
        vec![(0, topic(), partition(), RejectReason::Duplicate)]
    );
}

#[test]
fn a_sequence_gap_is_rejected_as_out_of_order() {
    let mut allocator = Allocator::new();
    commit(&mut allocator, span_from(1, 0, 2));
    let admission = allocator.admit(vec![span_from(1, 5, 1)]);
    assert!(admission.admitted.is_empty());
    assert_eq!(
        admission.rejected,
        vec![(0, topic(), partition(), RejectReason::OutOfOrder)]
    );
}

/// A bumped epoch is a legitimate `InitProducerId` incarnation — but
/// only its own genuine first send (sequence zero) is. Sequence one, at
/// a higher epoch, is not stale (it is not a zombie); it is a gap in
/// the new line — `M11.7` distinguishes it from
/// [`a_stale_epoch_is_rejected_as_a_zombie`] on exactly this point.
#[test]
fn a_bumped_epoch_with_the_wrong_first_sequence_is_an_ordinary_gap() {
    let mut allocator = Allocator::new();
    commit(&mut allocator, span_from(1, 0, 2));
    let span = CommittedSpan::new(
        topic(),
        partition(),
        1,
        ByteRange::Full,
        Some(ProducerIdentity::new(
            producer(1),
            ProducerEpoch::new(1).expect("valid"),
            1,
        )),
    );
    let admission = allocator.admit(vec![span]);
    assert!(admission.admitted.is_empty());
    assert_eq!(
        admission.rejected,
        vec![(0, topic(), partition(), RejectReason::OutOfOrder)]
    );
}

/// ⚠️ **`M11.7`, `ADR-0031` point 5.** A bumped epoch's own genuine
/// first send — sequence zero — is admitted, resetting the line exactly
/// as the `None` case's own rule does for a producer with no history at
/// all.
#[test]
fn a_bumped_epoch_starting_at_sequence_zero_is_admitted() {
    let mut allocator = Allocator::new();
    commit(&mut allocator, span_from(1, 0, 2));
    let admission = allocator.admit(vec![CommittedSpan::new(
        topic(),
        partition(),
        1,
        ByteRange::Full,
        Some(ProducerIdentity::new(
            producer(1),
            ProducerEpoch::new(1).expect("valid"),
            0,
        )),
    )]);
    assert_eq!(
        admission.admitted.len(),
        1,
        "the new line's genuine first send"
    );
    assert!(admission.rejected.is_empty());
}

/// ⚠️ **The zombie `M11.md` task 10 names.** An epoch *older* than the
/// one on record means some other incarnation of this producer already
/// moved the line past what this sender knows about — refused as
/// `StaleEpoch`, real Kafka's own `INVALID_PRODUCER_EPOCH`, not the
/// ordinary `OutOfOrder` a ordinary gap gets.
#[test]
fn a_stale_epoch_is_rejected_as_a_zombie() {
    let mut allocator = Allocator::new();
    commit(
        &mut allocator,
        CommittedSpan::new(
            topic(),
            partition(),
            2,
            ByteRange::Full,
            Some(ProducerIdentity::new(
                producer(1),
                ProducerEpoch::new(1).expect("valid"),
                0,
            )),
        ),
    );
    // This sender still believes it is at epoch 0 — a zombie that has
    // not seen the bump some other incarnation already made.
    let admission = allocator.admit(vec![span_from(1, 1, 1)]);
    assert!(admission.admitted.is_empty());
    assert_eq!(
        admission.rejected,
        vec![(0, topic(), partition(), RejectReason::StaleEpoch)]
    );
}

/// ⚠️ **The load-bearing case, tested directly rather than through two
/// billion real commits** (`Allocator::seeded`'s own precedent for
/// "reaching a boundary is not a fixture, it is a geological era"): real
/// Kafka wraps a producer's sequence from `i32::MAX` back to `0` rather
/// than overflowing, and `wrapping_add(1)` would give `i32::MIN`
/// instead — rejecting every legitimate producer that has sent more
/// than two billion batches to one partition.
#[test]
fn sequence_wraps_from_i32_max_back_to_zero() {
    assert_eq!(super::next_sequence(i32::MAX), 0);
    assert_eq!(super::next_sequence(0), 1);
    assert_eq!(super::next_sequence(41), 42);
}

/// ⚠️ **The round-1 review defect this row exists to fix**: one
/// producer's rejection must not block a different producer's span in
/// the same bundled commit from being admitted.
#[test]
fn one_producers_rejection_does_not_block_anothers_admission() {
    let allocator = Allocator::new();
    // Producer 1's first span is a genuine gap (no prior state, sequence
    // 5): rejected. Producer 2's is a legitimate first span in the same
    // bundle: admitted, untouched by producer 1's own outcome.
    let admission = allocator.admit(vec![span_from(1, 5, 1), span_from(2, 0, 1)]);
    assert_eq!(
        admission.rejected,
        vec![(0, topic(), partition(), RejectReason::OutOfOrder)],
        "producer 1's gap"
    );
    assert_eq!(admission.admitted.len(), 1, "producer 2's fresh span");
    assert!(admission.replayed.is_empty());
}

/// ⚠️ **The blocking defect round 1 review found and reproduced**: the
/// same producer sending the same sequence *twice within one bundle*
/// must not report the second occurrence's offset from the first's
/// still-provisional (`Offset::ZERO`-placeholder) state — it must be
/// refused instead, never answered with a wrong position.
#[test]
fn a_same_bundle_exact_repeat_is_refused_not_answered_with_a_placeholder() {
    let mut allocator = Allocator::new();
    // Ten records already committed on this partition by another
    // producer, so offset zero is provably the wrong answer for
    // anything replayed here.
    commit(
        &mut allocator,
        CommittedSpan::new(topic(), partition(), 10, ByteRange::Full, None),
    );
    let admission = allocator.admit(vec![span_from(5, 0, 3), span_from(5, 0, 3)]);
    assert_eq!(admission.admitted.len(), 1, "the first occurrence");
    assert!(
        admission.replayed.is_empty(),
        "the second must not fabricate an offset from a provisional entry"
    );
    assert_eq!(admission.rejected.len(), 1, "the second is refused");
}

/// A producer's own two spans in one bundle are checked against each
/// other, not only against what was already committed — the second must
/// see the first's newly-admitted sequence.
#[test]
fn a_producers_two_spans_in_one_bundle_are_sequential() {
    let allocator = Allocator::new();
    let admission = allocator.admit(vec![span_from(1, 0, 1), span_from(1, 1, 1)]);
    assert_eq!(admission.admitted.len(), 2, "both are genuinely in order");
    assert!(admission.rejected.is_empty());
}

/// The reverse of the above: a bundle presenting a producer's own two
/// spans **out of order** rejects the first (a gap — the producer has no
/// prior state, so sequence 1 is not its first) and admits the second on
/// its own merits (sequence 0 *is* a legitimate first span) — proving
/// the first's rejection was never recorded into `running` for the
/// second to inherit.
#[test]
fn a_producers_two_spans_in_one_bundle_out_of_order_rejects_only_the_gap() {
    let allocator = Allocator::new();
    let admission = allocator.admit(vec![span_from(1, 1, 1), span_from(1, 0, 1)]);
    assert_eq!(
        admission.rejected,
        vec![(0, topic(), partition(), RejectReason::OutOfOrder)]
    );
    assert_eq!(
        admission.admitted.len(),
        1,
        "sequence 0 is a legitimate first span"
    );
}
