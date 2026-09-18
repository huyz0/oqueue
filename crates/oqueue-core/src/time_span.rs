//! When a partition's data was committed, as the index knows it.

use crate::Timestamp;

/// The oldest and newest commit that put records into a partition.
///
/// ⚠️ **Folded from the log, never read from an object** (`M5.86`). FR-33's
/// decision — is this partition's data older than its retention — has to cost
/// no object-storage operation, because a retention round evaluates it for
/// every partition a node holds. `ADR-0036` decision 1 is the same property
/// for compaction's trigger, and this is what makes retention's trigger
/// obey it too.
///
/// ⚠️ **Commit time, not record time.** A record's own timestamp is
/// client-supplied and unordered — the Kafka protocol lets a producer send
/// anything — so a retention clock driven by it is one a client can defeat by
/// backdating. This is the moment the coordinator's log took the object.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TimeSpan {
    min: Timestamp,
    max: Timestamp,
}

impl TimeSpan {
    /// The extent of a single commit.
    #[must_use]
    pub const fn at(when: Timestamp) -> Self {
        Self {
            min: when,
            max: when,
        }
    }

    /// The oldest commit that touched the partition.
    #[must_use]
    pub const fn min(self) -> Timestamp {
        self.min
    }

    /// The newest.
    ///
    /// ⚠️ **What a retention round compares against**, because a partition is
    /// dead when its *last* write is older than the retention — not its first.
    #[must_use]
    pub const fn max(self) -> Timestamp {
        self.max
    }

    /// Widens this span to include `other`.
    ///
    /// ⚠️ **Not `max` alone.** A replay folds the log from the beginning and a
    /// live coordinator folds it forward, and the two must reach the same
    /// state — so the fold takes the extent of everything seen rather than the
    /// last thing seen, which would depend on arrival order.
    #[must_use]
    pub fn widened(self, other: Self) -> Self {
        Self {
            min: self.min.min(other.min),
            max: self.max.max(other.max),
        }
    }
}
