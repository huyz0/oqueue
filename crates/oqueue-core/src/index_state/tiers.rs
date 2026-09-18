//! Three counts where there was one, and what each tier costs.
//!
//! ⚠️ **The tiers grow by different laws, which is why one number cannot
//! govern them** (`ADR-0043` decision 1). The tail is a constant multiple of
//! a node's partitions — `TAIL_WINDOW_ENTRIES` each, and no more. A manifest
//! reference is one per partition. Un-absorbed history is neither: it is a
//! **rate**, doc 14 §3's ~4M `(object, partition)` entries a second, bounded
//! only by how often the compaction sweep runs. A ceiling over the sum
//! therefore measures mostly the tail at a short sweep interval and mostly
//! history at a long one, so it fires for the wrong reason and stays quiet for
//! the right one.
//!
//! ⚠️ **The ceiling still belongs over the total**, and this module does not
//! change that — `ADR-0043` decision 1 as its first round corrected it: the
//! two terms cross at a 40.2-second sweep, so above it history dominates and
//! below it the tail does, and bounding either alone lets the other run. What
//! the tiers need separately is *attribution*: which one moved, so an alarm
//! says what to do about it.

use crate::{ObjectKey, ObjectRef, Offset, TailEntry};
use core::mem::size_of;
use core::ops::{Add, Sub};

/// How many entries each tier of the index holds.
///
/// ⚠️ **Maintained, never counted.** [`IndexState`](crate::IndexState) moves
/// these by a partition's before-and-after, and each field is a collection's
/// `len()`, so the maintenance is O(1) per partition a batch touches rather
/// than O(entries) — the property `entries()` had and this keeps.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Tiers {
    /// The hot window: byte ranges inline, so a tail read is one GET.
    pub tail: usize,
    /// Everything older that no manifest has absorbed yet. ⚠️ **The only term
    /// that runs away**, and the one an alarm is about.
    pub history: usize,
    /// One per partition that has published a manifest (`ADR-0042`).
    pub manifests: usize,
}

impl Tiers {
    /// Every entry, in every tier — what `entries()` has always returned.
    #[must_use]
    pub const fn total(self) -> usize {
        self.tail + self.history + self.manifests
    }

    /// What these entries cost in memory, at an object key of `key_width`
    /// bytes.
    ///
    /// ⚠️ **Derived from the types, not restated from `ADR-0043`.** That ADR's
    /// table was wrong in the same direction three times (`M5.64`), each time
    /// because a correction wrote a new number down instead of computing one.
    /// The widths here are `size_of` plus the key's own heap allocation, which
    /// is larger than the struct it hangs off. `tests/it/index_cost.rs` holds
    /// this function to the 94 / 118 / 86 B the ADR quotes, in the same test
    /// that derives them — because this recomputes the widths rather than
    /// importing them, and two derivations that nothing compares can drift.
    ///
    /// ⚠️ **`key_width` is a parameter because a key's width is a deployment's
    /// property, not this crate's.** `BundleNamer` writes 54 B at its wide end
    /// today, and a bound quotes the wide end.
    #[must_use]
    pub const fn bytes(self, key_width: usize) -> usize {
        self.tail * (size_of::<TailEntry>() + key_width)
            + self.history * (size_of::<ObjectRef>() + key_width)
            + self.manifests * (size_of::<ObjectKey>() + size_of::<Offset>() + key_width)
    }
}

/// ⚠️ **Saturating, and the order at the call site is what makes it
/// unreachable.** A partition the batch replaced wholesale is folded as
/// `total + after - before`, adding before subtracting — so the term being
/// removed is always dominated by a total that already contains it, and the
/// clamp can only be reached on a count that had already drifted. Written the
/// other way round, `total - before + after`, the intermediate genuinely can
/// go negative, which is why the order is stated here rather than left to the
/// two call sites. Wrapping instead of clamping would turn a drift of one into
/// a quota of `usize::MAX` — an alarm that can never fire, which is the
/// failure mode `ADR-0043` decision 3 exists to avoid.
impl Add for Tiers {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            tail: self.tail.saturating_add(other.tail),
            history: self.history.saturating_add(other.history),
            manifests: self.manifests.saturating_add(other.manifests),
        }
    }
}

impl Sub for Tiers {
    type Output = Self;

    fn sub(self, other: Self) -> Self {
        Self {
            tail: self.tail.saturating_sub(other.tail),
            history: self.history.saturating_sub(other.history),
            manifests: self.manifests.saturating_sub(other.manifests),
        }
    }
}
