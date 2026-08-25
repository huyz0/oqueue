//! Which coordinator incarnation a response came from.

/// The generation of the coordinator that answered.
///
/// `M3.md` task 3 puts one in every response so a reader can revalidate after
/// a failover rewinds the log: a coordinator that lost its leadership lease
/// and came back has a higher epoch, and a reader holding index state from the
/// older one must discard it rather than merge it. `ADR-0020` point 4 reserves
/// object-storage conditional writes for two low-frequency control-plane uses
/// and never the offset stream — this leadership lease is one of them, and
/// compaction/checkpoint claims (`M5`/`M6`) the other.
///
/// ⚠️ **Failover itself is `M6`'s** (doc 10 #14/#15). This type is the fence
/// M3 puts in the wire vocabulary so the reader side is already written
/// against it; M3 builds a single active coordinator and does not yet elect a
/// replacement for it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct CoordinatorEpoch(u64);

impl CoordinatorEpoch {
    /// The epoch of the first coordinator to hold a log.
    pub const ZERO: Self = Self(0);

    /// Builds an epoch.
    #[must_use]
    pub const fn new(value: u64) -> Self {
        Self(value)
    }

    /// The epoch, as the `u64` a staleness fence compares on.
    #[must_use]
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl core::fmt::Display for CoordinatorEpoch {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}", self.0)
    }
}
