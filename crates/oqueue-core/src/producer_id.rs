//! Producer identity.

use crate::{Error, Result};

/// A producer's identity, guaranteed non-negative.
///
/// # Invariant
///
/// **The wrapped value is never negative.** Backed by `i64` because that is
/// the protocol's type for `producer_id` (`InitProducerIdResponse`,
/// `RecordBatch`), so no conversion is needed at the wire boundary — but the
/// negative half of that range is unrepresentable here.
///
/// ⚠️ **Kafka uses `-1` on the wire to mean "no producer id"** — a
/// non-idempotent client's own default, and the value a client sends when it
/// wants a fresh one. That is a *sentinel*, not an identity, and mapping it to
/// `None` is `M11`'s decoder's job, the same shape [`PartitionId`](crate::PartitionId)
/// already draws against Kafka's `-1` partition sentinel.
///
/// ⚠️ **Deliberately not sequential, and this type does not allocate one.**
/// `ADR-0031` decided a `ProducerId` needs only to be unique for the
/// producer's lifetime and comparable for epoch fencing — not dense, not
/// ordered, and not drawn from a single cluster-wide counter, because a
/// producer's writes can span partitions on different metadata shards and no
/// single shard's allocator is the right place to mint one. `oqueue-core` is
/// sans-I/O (non-negotiable 5) and cannot mint an identity itself — minting
/// is the composer's job, on `oqueue_broker::WriterId`'s own precedent
/// (process id, real clock, an in-process counter), landing where
/// `InitProducerId`'s handler is built.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProducerId(i64);

impl ProducerId {
    /// Builds a producer id.
    ///
    /// # Errors
    ///
    /// [`Error::NegativeProducerId`] if `value` is negative.
    pub const fn new(value: i64) -> Result<Self> {
        if value < 0 {
            return Err(Error::NegativeProducerId { got: value });
        }
        Ok(Self(value))
    }

    /// The producer id, always `>= 0`.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }
}

impl core::fmt::Display for ProducerId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}
