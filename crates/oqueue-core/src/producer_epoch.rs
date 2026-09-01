//! A producer's fencing token.

use crate::{Error, Result};

/// How many times a producer's identity has been re-initialized, guaranteed
/// non-negative.
///
/// # Invariant
///
/// **The wrapped value is never negative.** Backed by `i16` because that is
/// the protocol's type for `producer_epoch` (`InitProducerIdResponse`,
/// `RecordBatch`), so no conversion is needed at the wire boundary — but the
/// negative half of that range is unrepresentable here.
///
/// ⚠️ **Kafka uses `-1` on the wire to mean "no epoch"**, the same sentinel
/// shape [`ProducerId`](crate::ProducerId)'s own doc describes for `-1`; a
/// decoder maps it away before this type is ever asked to hold it.
///
/// ⚠️ **`ADR-0031` fences a producer-state commit against
/// [`CoordinatorEpoch`](crate::CoordinatorEpoch), not against this one.** The
/// two answer different questions: `CoordinatorEpoch` says whether *this
/// process* is still the shard's one writer; `ProducerEpoch` says whether
/// *this client* is still the producer that last initialized — a zombie
/// producer retrying with a stale epoch, not a zombie coordinator. `M11.7`
/// owns the bump-and-fence mechanism this type is a building block for; this
/// type carries no `advance`/`bump` method until that task needs one.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct ProducerEpoch(i16);

impl ProducerEpoch {
    /// The epoch a freshly initialized producer starts at.
    pub const ZERO: Self = Self(0);

    /// Builds a producer epoch.
    ///
    /// # Errors
    ///
    /// [`Error::NegativeProducerEpoch`] if `value` is negative.
    pub const fn new(value: i16) -> Result<Self> {
        if value < 0 {
            return Err(Error::NegativeProducerEpoch { got: value });
        }
        Ok(Self(value))
    }

    /// The epoch, always `>= 0`.
    #[must_use]
    pub const fn get(self) -> i16 {
        self.0
    }
}

impl core::fmt::Display for ProducerEpoch {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}
