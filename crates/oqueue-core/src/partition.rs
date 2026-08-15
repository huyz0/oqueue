//! Partition identity.

use crate::{Error, Result};

/// A partition index, guaranteed non-negative.
///
/// # Invariant
///
/// **The wrapped value is never negative.** Backed by `i32` because that is the
/// protocol's type for a partition index, so no conversion is needed at the
/// wire boundary — but the negative half of that range is unrepresentable here.
///
/// ⚠️ Kafka uses `-1` on the wire to mean "no partition" in some requests. That
/// is a *sentinel*, not a partition, and it is `M2`'s job to map it to `None`
/// rather than to a `PartitionId`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct PartitionId(i32);

impl PartitionId {
    /// Builds a partition id.
    ///
    /// # Errors
    ///
    /// [`Error::NegativePartitionId`] if `index` is negative.
    pub const fn new(index: i32) -> Result<Self> {
        if index < 0 {
            return Err(Error::NegativePartitionId { got: index });
        }
        Ok(Self(index))
    }

    /// The partition index, always `>= 0`.
    #[must_use]
    pub const fn get(self) -> i32 {
        self.0
    }
}

impl core::fmt::Display for PartitionId {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}
