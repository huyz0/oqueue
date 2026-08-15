//! Log offsets, and arithmetic that refuses to wrap.

use crate::{Error, Result};

/// A position in a partition's log, guaranteed non-negative.
///
/// # Invariant
///
/// **The wrapped value is never negative**, and **no operation on it ever
/// wraps**. Backed by `i64` because that is the protocol's type for an offset.
///
/// ⚠️ The second half is the one that matters. `M3`'s offset sequencing rests
/// on offsets being monotonic, and a silent wrap is the only arithmetic result
/// that breaks monotonicity without failing anything — the sum comes back
/// smaller than what it started from, and every later comparison is wrong. So
/// [`Offset::add`] returns [`Error::OffsetOverflow`] rather than a number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Offset(i64);

impl Offset {
    /// The first offset in any log.
    pub const ZERO: Self = Self(0);

    /// Builds an offset.
    ///
    /// # Errors
    ///
    /// [`Error::NegativeOffset`] if `value` is negative.
    pub const fn new(value: i64) -> Result<Self> {
        if value < 0 {
            return Err(Error::NegativeOffset { got: value });
        }
        Ok(Self(value))
    }

    /// The offset, always `>= 0`.
    #[must_use]
    pub const fn get(self) -> i64 {
        self.0
    }

    /// Advances by `delta`.
    ///
    /// # Errors
    ///
    /// [`Error::NegativeOffsetDelta`] if `delta` is negative — moving an offset
    /// backwards is not an arithmetic operation on this type, it is a seek, and
    /// callers that mean it construct the target directly.
    ///
    /// [`Error::OffsetOverflow`] if the sum would leave the `i64` range. ⚠️ The
    /// error is the point: the alternative is a value smaller than `self`.
    pub const fn add(self, delta: i64) -> Result<Self> {
        if delta < 0 {
            return Err(Error::NegativeOffsetDelta { got: delta });
        }
        match self.0.checked_add(delta) {
            Some(sum) => Ok(Self(sum)),
            None => Err(Error::OffsetOverflow {
                base: self.0,
                delta,
            }),
        }
    }
}

impl core::fmt::Display for Offset {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}
