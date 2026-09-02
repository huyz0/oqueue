//! Bounding `producer_state`'s growth — `ADR-0031` point 6, `M3.11`'s quota
//! problem one structure over.
//!
//! ⚠️ **A `HashMap` keyed by every producer that has ever written has no
//! natural ceiling**, the same shape `M3.11` found for the offset index —
//! but not the same *outcome*. `M3.11`'s enforced quota proved unachievable
//! there because eviction gave back range a **rebuild could not restore**:
//! replaying the log reproduced the same entry count and re-evicted the
//! same range forever. `Allocator` has no rebuild-from-log path at all yet
//! (`M6`'s cold start is what adds one) — a coordinator lives its whole
//! life from an empty log today — so an evicted producer's entry is simply
//! gone until that producer sends again, not a loop that cannot converge.
//! The two quotas share a name and not a fate.
//!
//! ⚠️ **Bounded by count, not wall-clock time.** This crate is sans-I/O
//! (non-negotiable 5) and reads no clock; a wall-clock TTL would need one
//! threaded through `Allocator`, which nothing else here does. A generous
//! count serves `ADR-0031` point 6's actual requirement — never evict a
//! producer a retry still needs within its own `request.timeout.ms`-scale
//! window — because a just-active producer's entry is, by construction,
//! the most recently touched one, and is evicted only after
//! [`MAX_TRACKED_PRODUCERS`] *other* distinct producers have been touched
//! since. [`MAX_TRACKED_PRODUCERS`] is chosen generously rather than
//! modeled against a cited throughput number, since no NFR states a
//! concurrent-idempotent-producer count to derive one from — revisit if a
//! real workload's numbers say otherwise.

use std::collections::BTreeMap;
use std::hash::Hash;

/// How many distinct `(producer, topic, partition)` lines one shard's
/// allocator tracks before evicting the least recently touched.
///
/// ⚠️ **Lowering this is the weakening direction** (non-negotiable 2) — it
/// shortens the window a retry has before its producer's state might be
/// gone, the opposite of most thresholds in `check-drift.sh`'s table.
/// Raising it only costs memory: an entry is a handful of small fields, so
/// even this count's own upper reaches are a few megabytes.
pub(crate) const MAX_TRACKED_PRODUCERS: usize = 100_000;

/// Which of a bounded set of keys was touched least recently — the piece
/// [`Allocator::apply`](super::Allocator::apply) needs to evict in
/// `O(log n)` rather than scanning the whole map on every commit once at
/// capacity, which the commit path (`performance.md`'s hot-path discipline)
/// cannot afford to do unboundedly.
///
/// ⚠️ **Two maps, not one, and both are load-bearing.** `order` answers
/// "which key is oldest" in `O(log n)`; `last_touch` is what lets a
/// *re*-touch find and remove its own stale entry from `order` before
/// re-inserting — without it, touching the same key twice would leave two
/// entries in `order`, one of which never gets removed and would name a key
/// as evictable long after it was actually touched again.
#[derive(Debug)]
pub(crate) struct Recency<K> {
    next: u64,
    order: BTreeMap<u64, K>,
    last_touch: std::collections::HashMap<K, u64>,
}

// ⚠️ Hand-written rather than `#[derive(Default)]`: the derive adds a
// `K: Default` bound to the impl, even though every field here defaults
// fine regardless of what `K` is — `BTreeMap`/`HashMap` are empty and `u64`
// is `0` either way. `ProducerId`/`TopicId`/`PartitionId` (this type's only
// real key) have no `Default` of their own, by design (`producer_id.rs`'s
// own doc: no bypass around the invariant a bare `Default::default()`
// would need to skip), so the derived bound would make this type
// unusable for the one key it exists for.
impl<K> Default for Recency<K> {
    fn default() -> Self {
        Self {
            next: 0,
            order: BTreeMap::new(),
            last_touch: std::collections::HashMap::new(),
        }
    }
}

impl<K: Clone + Hash + Eq> Recency<K> {
    /// Records `key` as touched now — the most recent thing this tracker
    /// knows about.
    pub(crate) fn touch(&mut self, key: K) {
        if let Some(old) = self.last_touch.remove(&key) {
            self.order.remove(&old);
        }
        let stamp = self.next;
        self.next += 1;
        self.order.insert(stamp, key.clone());
        self.last_touch.insert(key, stamp);
    }

    /// The least recently touched key, forgotten by this tracker as a side
    /// effect.
    ///
    /// ⚠️ **The caller still owns removing it from whatever map it is a key
    /// into** — this type tracks order only, never the state itself, so
    /// `Allocator::apply` is what actually drops the `ProducerState` entry.
    pub(crate) fn evict_oldest(&mut self) -> Option<K> {
        let (&stamp, key) = self.order.iter().next()?;
        let key = key.clone();
        self.order.remove(&stamp);
        self.last_touch.remove(&key);
        Some(key)
    }
}

#[cfg(test)]
mod tests {
    use super::Recency;

    #[test]
    fn an_empty_tracker_evicts_nothing() {
        let mut recency: Recency<u32> = Recency::default();
        assert_eq!(recency.evict_oldest(), None);
    }

    #[test]
    fn the_least_recently_touched_key_is_evicted_first() {
        let mut recency = Recency::default();
        recency.touch("a");
        recency.touch("b");
        recency.touch("c");
        assert_eq!(recency.evict_oldest(), Some("a"));
        assert_eq!(recency.evict_oldest(), Some("b"));
        assert_eq!(recency.evict_oldest(), Some("c"));
        assert_eq!(recency.evict_oldest(), None);
    }

    /// ⚠️ **The property the whole `last_touch` map exists for.** Without
    /// removing the stale `order` entry on a re-touch, `"a"` would still be
    /// evicted first here, having been touched most recently of the three.
    #[test]
    fn retouching_a_key_moves_it_to_the_back_of_the_line() {
        let mut recency = Recency::default();
        recency.touch("a");
        recency.touch("b");
        recency.touch("a"); // "a" is now the most recently touched.
        assert_eq!(recency.evict_oldest(), Some("b"));
        assert_eq!(recency.evict_oldest(), Some("a"));
        assert_eq!(recency.evict_oldest(), None);
    }

    /// A key evicted once and then touched again is tracked fresh — evicting
    /// forgets it entirely rather than leaving a residual entry a later
    /// touch might collide with.
    #[test]
    fn a_key_evicted_and_then_retouched_is_tracked_again() {
        let mut recency = Recency::default();
        recency.touch("a");
        assert_eq!(recency.evict_oldest(), Some("a"));
        recency.touch("a");
        assert_eq!(recency.evict_oldest(), Some("a"));
    }
}
