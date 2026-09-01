//! What one idempotent produce carries, grouped into the one thing a span
//! either has or does not.

use crate::{ProducerEpoch, ProducerId};

/// A produce's idempotent-producer identity: who sent it, at what epoch, and
/// which sequence number in that producer's own stream.
///
/// ⚠️ **One field on [`CommittedSpan`](crate::CommittedSpan), not three.**
/// `clippy.toml`'s `too-many-arguments-threshold` is 5, and `CommittedSpan`
/// was already at 4 — three more positional parameters for one optional
/// capability would trip the lint and, more to the point, would let a caller
/// set `producer_id` without `sequence` by accident, which is not a state
/// this type can be in. `ADR-0031` attaches idempotent-producer state to a
/// span as `Option<ProducerIdentity>`: `None` for an ordinary produce, `Some`
/// for one this project can deduplicate.
///
/// ⚠️ **`sequence` carries no invariant of its own.** Real Kafka's
/// `base_sequence` starts at zero and *wraps* back to zero after `i32::MAX`
/// rather than growing without bound — deliberate protocol behaviour, not an
/// error condition — so there is no "always non-negative" claim here to
/// enforce, unlike [`ProducerId`] and [`ProducerEpoch`]'s own guards. Neither
/// `record_count` nor `bytes` are validated by [`CommittedSpan::new`]
/// (`crate::CommittedSpan`) either; both are checked upstream, by the
/// decoder that produced them, and `sequence` follows the same division of
/// labour.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProducerIdentity {
    id: ProducerId,
    epoch: ProducerEpoch,
    sequence: i32,
}

impl ProducerIdentity {
    /// Groups a produce's idempotent-producer identity.
    #[must_use]
    pub const fn new(id: ProducerId, epoch: ProducerEpoch, sequence: i32) -> Self {
        Self {
            id,
            epoch,
            sequence,
        }
    }

    /// Which producer sent this.
    #[must_use]
    pub const fn id(&self) -> ProducerId {
        self.id
    }

    /// At what epoch.
    #[must_use]
    pub const fn epoch(&self) -> ProducerEpoch {
        self.epoch
    }

    /// The sequence number, within that producer's own stream.
    #[must_use]
    pub const fn sequence(&self) -> i32 {
        self.sequence
    }
}
