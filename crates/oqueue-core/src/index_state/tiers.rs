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

use super::IndexState;
use crate::{ObjectKey, ObjectRef, Offset, TailEntry};
use crate::{PartitionId, TimeSpan, TopicId};
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

impl IndexState {
    /// How many entries this index holds, across every partition and all
    /// three tiers — the tail, un-absorbed history, and one per published
    /// manifest.
    ///
    /// ⚠️ **The number NFR-11 is about, and M3 only measures it** (`M3.11`).
    /// A node's metadata cost has to be proportional to the partitions *active
    /// on it*, and this index is keyed per `(object, partition)` — doc 14 §3's
    /// ~4M entries/s row — so it grows with everything the node has ever seen.
    /// Enforcing a ceiling on *this* keying cannot be made to work: eviction
    /// gives back range a rebuild cannot restore, because replaying the log
    /// reproduces the same count and sheds the same entries again. The coarse
    /// per-object keying is what makes a bound feasible, and `roadmap.md`
    /// defers it to `M5`, which receives the enforcement with it.
    ///
    /// ⚠️ **A running count, not a traversal.** It is read wherever growth is
    /// watched, and a walk of the partition map is O(partitions) — on the
    /// coordinator's ack path, that is the one task every producer on the
    /// shard queues behind.
    #[must_use]
    pub const fn entries(&self) -> usize {
        self.tiers.total()
    }

    /// The same entries, attributed to the tier holding them.
    ///
    /// ⚠️ **What [`entries`](Self::entries) cannot say** (`ADR-0043`
    /// decision 1). The three tiers grow by different laws — the tail is
    /// bounded by [`TAIL_WINDOW_ENTRIES`](crate::TAIL_WINDOW_ENTRIES) per partition, a manifest reference
    /// is one per partition, and un-absorbed history is a rate bounded only by
    /// the compaction sweep interval — so a total that has risen says nothing
    /// about what to do. This says which term moved, and
    /// [`Tiers::bytes`] says what it costs.
    ///
    /// ⚠️ **Still O(1), and still not a walk of the map**: these are
    /// maintained by the fold, for the reason `entries` is.
    #[must_use]
    pub const fn tiers(&self) -> Tiers {
        self.tiers
    }
}

impl IndexState {
    /// When this partition's commits happened, if any have.
    ///
    /// ⚠️ **What FR-33's decision reads, and it costs no object-storage
    /// operation** (`M5.86`). A retention round asks this of every partition a
    /// node holds, so a decision that reached for an object would be one GET
    /// per partition per round — `ADR-0036` decision 1 is the same property
    /// one requirement over, and compaction's trigger already obeys it.
    ///
    /// ⚠️ **`None` is "nothing has been committed here", not "committed at the
    /// epoch".** A partition the fold knows about only because a manifest was
    /// published for it has no commit time of its own, and a round reading
    /// `EPOCH` there would reap it on its first sweep.
    #[must_use]
    pub fn time_span(&self, topic: &TopicId, partition: PartitionId) -> Option<TimeSpan> {
        self.partition(topic, partition).and_then(|slot| slot.when)
    }
}
