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
    /// a real gap, including a legitimate epoch bump whose own first
    /// sequence is not zero.
    OutOfOrder,
    /// The sequence repeats the last accepted one, but this span's own
    /// record count does not match what was recorded for it — the same
    /// sequence claiming to be a different batch.
    Duplicate,
    /// The epoch is older than the one on record — a zombie: some other
    /// incarnation of this producer already moved the line past what this
    /// sender knows about. `M11.7`, `ADR-0031` point 5. Real Kafka's own
    /// `INVALID_PRODUCER_EPOCH`, not an ordinary gap: a bumped epoch is not
    /// something a retry could ever mend.
    StaleEpoch,
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
        // ⚠️ **A zombie** (`M11.7`, `ADR-0031` point 5's producer-epoch
        // half): an epoch older than the one on record means some other
        // incarnation of this producer already moved the line past what
        // this sender knows about — real Kafka's own
        // `INVALID_PRODUCER_EPOCH`, not an ordinary gap, because a bumped
        // epoch is not something a retry could ever mend. Checked before
        // the equal-epoch arms below, and it must be: without this arm a
        // stale epoch and a stale sequence would be indistinguishable, and
        // a client fenced for one reason would be told to fix the other.
        Some(state) if epoch < state.epoch => Decision::Reject(RejectReason::StaleEpoch),
        // **A legitimate bump.** A higher epoch is a fresh `InitProducerId`
        // incarnation of the same producer id — real Kafka resets the
        // sequence line to zero on every bump, so this is exactly the
        // `None` arm's own rule, restated for a line that has history
        // instead of none. Only M11.4's non-transactional path mints an
        // epoch at all today, always zero, so a real bump needs FR-15
        // (deferred) to ever be sent by a well-behaved client — this arm
        // exists for the malformed or adversarial one, `security.md`
        // rule 3's "never trust it" applied to a header field like any
        // other.
        Some(state) if epoch > state.epoch => {
            if sequence == 0 {
                Decision::Admit
            } else {
                Decision::Reject(RejectReason::OutOfOrder)
            }
        }
        // ⚠️ Every remaining arm is reached only when `epoch == state.epoch`
        // — the two arms above already excluded both mismatch directions —
        // so it is not restated as a guard here.
        Some(state) if sequence == next_sequence(state.sequence) => Decision::Admit,
        Some(state) if !provisional && sequence == state.sequence => {
            if record_count == state.record_count {
                Decision::Replay(state)
            } else {
                Decision::Reject(RejectReason::Duplicate)
            }
        }
        // A gap, a stale sequence, or a same-bundle repeat with no real
        // offset to answer from — the safe default is to refuse rather
        // than to guess or invent one.
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
mod tests;
