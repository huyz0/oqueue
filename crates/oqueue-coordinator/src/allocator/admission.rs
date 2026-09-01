//! Producer-sequence admission — split from `allocator.rs` at the 500-line
//! limit (`M11.3`), and a real seam rather than a trim: offset staging and
//! sequence admission are two different concepts that happen to share one
//! allocator, exactly the way `crash_points.rs`/`crash_points/broker_died.rs`
//! split for the same reason (`M10.30`).

use super::{Allocator, ProducerState};
use crate::commit::Assignment;
use oqueue_core::{CommittedSpan, PartitionId, ProducerEpoch, ProducerId, TopicId};
use std::collections::HashMap;

/// The sequence a producer's *next* batch is expected to carry, after `last`.
///
/// ⚠️ **Not `last + 1`.** Real Kafka's `base_sequence` wraps from
/// `i32::MAX` back to `0` rather than overflowing — deliberate protocol
/// behaviour a producer's own client library performs, not an error
/// condition — so `i32::MAX.wrapping_add(1)` (`i32::MIN`) would reject every
/// legitimate producer that has sent more than two billion batches.
const fn next_sequence(last: i32) -> i32 {
    if last == i32::MAX { 0 } else { last + 1 }
}

/// Why [`Allocator::admit`] excluded a span from the commit it was offered
/// in.
///
/// ⚠️ **`pub`, not `pub(crate)`, from `M11.6`.** Its own doc used to say
/// "coordinator-internal, not a wire code" — true of the *codes* (naming
/// Kafka's own error numbers here would put a protocol concern in the one
/// crate this workspace keeps sans wire-format), but a caller across the
/// crate boundary still needs to *name* this type to map it, which a
/// `pub(crate)` variant inside a public [`SpanOutcome`](crate::SpanOutcome)
/// cannot let it do. `oqueue-broker`'s `produce` handler is that caller —
/// see [`error_codes::OUT_OF_ORDER_SEQUENCE_NUMBER`] and
/// [`error_codes::DUPLICATE_SEQUENCE_NUMBER`] in `oqueue-codec`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    /// The sequence skips ahead of what this producer's line expects next —
    /// a real gap, or (for now) an epoch that does not match the one on
    /// record. `M11.7` owns turning the epoch half of this into fencing.
    OutOfOrder,
    /// The sequence repeats the last accepted one, but this span's own
    /// record count does not match what was recorded for it — the same
    /// sequence claiming to be a different batch.
    Duplicate,
}

/// What [`Allocator::admit`] decided about every span it was handed, each
/// tagged with its position in the slice `admit` was given.
///
/// ⚠️ **The position, not the `(topic, partition)`, is the correlation key**
/// (`M11.6`). `CommitAck`'s own doc already warns that a bundled commit may
/// name one `(topic, partition)` in more than one span — `FR-32` bundling
/// several producers into one object is exactly that case — so a caller
/// reconstructing "what happened to the *n*th span I gave `admit`" cannot key
/// on the pair without risking two spans answering as one. `serve` is the
/// one place that reconstruction happens, from these indices.
#[derive(Debug, Default)]
pub(crate) struct Admission {
    /// Spans to forward to [`Allocator::stage`](super::Allocator::stage) —
    /// no producer identity, or genuinely the next sequence in their
    /// producer's line.
    pub(crate) admitted: Vec<(usize, CommittedSpan)>,
    /// Spans whose sequence exactly repeats what already committed. No new
    /// offset is assigned; this *is* the answer — the transparent-success
    /// case a real duplicate must produce (`ADR-0031` point 2).
    pub(crate) replayed: Vec<(usize, TopicId, PartitionId, Assignment)>,
    /// Spans excluded entirely: not forwarded to `stage`, and not answered
    /// with an offset.
    pub(crate) rejected: Vec<(usize, TopicId, PartitionId, RejectReason)>,
}

/// What one span's sequence, checked against `current`, decides.
///
/// ⚠️ **Split out of `admit`'s own loop** (`too-many-lines`, and the reason
/// is more than the lint: three different *reasons* to reject all took the
/// same action, which read to `clippy::match_same_arms` as one arm three
/// times — a real report, not a false positive, once the *why* and the
/// *what to do* are two separate steps instead of one match doing both.
#[derive(Clone, Copy)]
enum Decision {
    /// Genuinely the next sequence — or the first one a fresh line ever
    /// sees.
    Admit,
    /// The sequence repeats the last accepted one and the record count
    /// matches: the transparent-success case a real duplicate must
    /// produce. Carries the state it replays, so the caller does not have
    /// to re-derive what `classify` already matched against.
    Replay(ProducerState),
    /// Excluded, for `reason`.
    Reject(RejectReason),
}

/// Classifies one span's sequence against `current` — pure, so `admit`'s
/// loop is only bookkeeping around it.
///
/// ⚠️ **`provisional` distinguishes where `current` came from, and a replay
/// can only answer from the *other* one.** `admit`'s `running` map records
/// an admitted span's state before `stage` has assigned it a real offset —
/// `ProducerState::provisional`'s own `base_offset: Offset::ZERO` is a
/// placeholder, never a position anything may be told it occupies. The same
/// producer sending the same sequence *twice within one bundle* would
/// otherwise have its second occurrence "replay" against that placeholder
/// and report offset zero regardless of where the first one actually lands
/// — reproduced directly, and the reason this parameter exists. A sequence
/// *comparison* against a provisional entry is fine (next-in-sequence needs
/// no real offset); answering a *replay* from one is not, so that one case
/// is refused instead — a within-bundle exact repeat is not the ordinary
/// retry-across-requests case this mechanism exists for.
fn classify(
    current: Option<ProducerState>,
    provisional: bool,
    sequence: i32,
    epoch: ProducerEpoch,
    record_count: u32,
) -> Decision {
    match current {
        // No prior state: a producer's first span to this partition starts
        // its line at sequence zero, the shape a client's first send after
        // `InitProducerId` always has.
        None if sequence == 0 => Decision::Admit,
        // ⚠️ Every remaining arm requires the recorded epoch to match —
        // `M11.7` owns the real fencing rules (a bumped epoch resets the
        // line to zero; a stale one is a zombie); until then a mismatch of
        // either direction falls through to the catch-all refusal below,
        // same as a genuine gap.
        Some(state) if state.epoch == epoch && sequence == next_sequence(state.sequence) => {
            Decision::Admit
        }
        Some(state) if !provisional && state.epoch == epoch && sequence == state.sequence => {
            if record_count == state.record_count {
                Decision::Replay(state)
            } else {
                Decision::Reject(RejectReason::Duplicate)
            }
        }
        // A gap, a stale sequence, an epoch mismatch, or a same-bundle
        // repeat with no real offset to answer from — the safe default is
        // to refuse rather than to guess or invent one.
        None | Some(_) => Decision::Reject(RejectReason::OutOfOrder),
    }
}

impl Allocator {
    /// Partitions `spans` by what their producer sequence, if any, decides —
    /// **before** any of them reaches
    /// [`stage`](super::Allocator::stage).
    ///
    /// ⚠️ **Why this is not inside `stage`'s own loop** (`ADR-0031` point 2,
    /// `M11.md` task 9's flush-batcher question): `stage` returns one
    /// `Result` for the whole commit, and `FR-32` bundles more than one
    /// producer's spans into one object — a check inside its loop would fail
    /// an unrelated producer's offset assignment over one producer's
    /// ordinary duplicate retry. `admit` runs first and excludes anything
    /// that should not reach the offset math at all.
    ///
    /// ⚠️ **A producer's own spans are checked against each other in bundle
    /// order**, via a `running` map exactly like `stage`'s own — one
    /// producer sending two spans to the same partition in one object sees
    /// the second checked against the first's newly-admitted sequence, not
    /// only against what was already committed.
    pub(crate) fn admit(&self, spans: Vec<CommittedSpan>) -> Admission {
        let mut admission = Admission::default();
        // ⚠️ Owned, not borrowed, unlike `stage`'s own `running`: a span here
        // is moved into `admission.admitted` in the same arm that reads its
        // topic, so a key borrowing it would still be live across the move.
        let mut running: HashMap<(ProducerId, TopicId, PartitionId), ProducerState> =
            HashMap::new();
        for (index, span) in spans.into_iter().enumerate() {
            let Some(identity) = span.producer() else {
                admission.admitted.push((index, span));
                continue;
            };
            let key = (identity.id(), span.topic().clone(), span.partition());
            // ⚠️ Which map answered decides whether a replay may trust the
            // offset it finds there — `running`'s own entries are
            // provisional until `stage` runs, `self.producer_state`'s are
            // real and already committed.
            let (current, provisional) = running.get(&key).map_or_else(
                || (self.producer_state.get(&key).copied(), false),
                |state| (Some(*state), true),
            );
            let decision = classify(
                current,
                provisional,
                identity.sequence(),
                identity.epoch(),
                span.record_count(),
            );
            record_decision(&mut admission, &mut running, key, (index, span), &decision);
        }
        admission
    }
}

/// Records what `classify` decided about one span — split out of `admit`'s
/// own loop (`too-many-lines`), on `classify`'s own precedent one function
/// over: the *what to do* is worth separating from the *why*, once both
/// stopped being one match doing both.
fn record_decision(
    admission: &mut Admission,
    running: &mut HashMap<(ProducerId, TopicId, PartitionId), ProducerState>,
    key: (ProducerId, TopicId, PartitionId),
    (index, span): (usize, CommittedSpan),
    decision: &Decision,
) {
    match *decision {
        Decision::Admit => {
            // ⚠️ `identity` is re-read from `span` rather than threaded in
            // as a separate argument: it would be the sixth, past
            // `clippy.toml`'s five-argument threshold, and `span.producer()`
            // is `Some` on every path that reaches here — `admit`'s own
            // early `continue` for `None` never calls this at all.
            let identity = span.producer().map(|identity| {
                ProducerState::provisional(
                    identity.epoch(),
                    identity.sequence(),
                    span.record_count(),
                )
            });
            if let Some(state) = identity {
                running.insert(key, state);
            }
            admission.admitted.push((index, span));
        }
        Decision::Replay(state) => {
            admission.replayed.push((
                index,
                span.topic().clone(),
                span.partition(),
                Assignment::new(
                    span.topic().clone(),
                    span.partition(),
                    state.base_offset,
                    state.record_count,
                ),
            ));
        }
        Decision::Reject(reason) => {
            admission
                .rejected
                .push((index, span.topic().clone(), span.partition(), reason));
        }
    }
}

#[cfg(test)]
mod tests {
    // A panic in a test harness is the test failing, which is what it is for.
    #![allow(clippy::expect_used)]

    use super::RejectReason;
    use crate::allocator::Allocator;
    use oqueue_core::{
        ByteRange, CommittedSpan, ObjectKey, PartitionId, ProducerEpoch, ProducerId,
        ProducerIdentity, TopicId,
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

    #[test]
    fn a_mismatched_epoch_is_rejected() {
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
}
