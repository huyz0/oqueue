//! The fold itself: how a metadata log becomes something a fetch can query.
//!
//! ⚠️ **Its own module because the trait file reached the 500-line limit**, and
//! the split is along the seam rather than at a convenient line:
//! `materialized_index.rs` holds the contract and the fake that stands in for
//! it, and this holds the one implementation of the fold both that fake and
//! `oqueue-index`'s `MemoryIndex` share. Two copies of a fold this subtle —
//! order checking, all-or-nothing application, non-wrapping addition, tier
//! demotion — would drift, and the conformance suite would then be asserting
//! the contract against two different meanings of it.
//!
//! ⚠️ **`ADR-0022`'s fold and its read surface are two more concepts under
//! one file, and `M10.20` (from `M3.44`) is where they part ways.** `apply`
//! and tier demotion — the write side, below — hold this module doc's own
//! subject; `find_batches` and the `Page` it pages results through are
//! [`page`]'s, on the same file-per-concept precedent `oqueue-codec`'s
//! `records`/`records::count` split just took.

use crate::{
    CommitVersion, Error, IndexedBatch, MetadataEntry, MetadataRecord, ObjectKey, ObjectRef,
    Offset, PartitionId, Result, TailEntry, TopicId,
};
use std::collections::HashMap;

mod page;
mod partition;
mod projection;
pub use page::MAX_BATCHES_PER_PAGE;
use page::Page;
use partition::PartitionIndex;
use projection::{Effect, Staged, StagedSpans, precedes};

/// How many entries a partition keeps in the tail tier, byte ranges inline.
///
/// ⚠️ **UNDERIVED, and `M3.md` says so.** The tail/history split is sized
/// against an assumed workload — doc 10 #25, which `M3.md`'s Risks section
/// names as gating this milestone's arithmetic. This value is a placeholder
/// with the right *shape* and not a derived constant; nothing may cite it as
/// one, and the milestone that derives NFR-13 is what replaces it.
///
/// ⚠️ **Bounded per partition, which is not bounded.** The window caps the
/// expensive tier for one partition; the partition map itself has no cap, so
/// steady-state cost still scales with partitions active on the node. That is
/// NFR-11's concern — **`M7`'s requirement**, and one M3 does not bound: a
/// ceiling at this keying gives back range a rebuild cannot restore, so
/// `roadmap.md` carries the enforcement to `M5` beside the coarse per-object
/// keying that makes it feasible (`M3.11`).
///
/// It is a constant rather than a knob, per `AGENTS.md` non-negotiable 2.
pub const TAIL_WINDOW_ENTRIES: usize = 128;

/// What one `apply` has read out of its batch and not yet applied.
///
/// ⚠️ **One scratch rather than three locals**, since `M5.13` added the third
/// and `apply` outgrew `code-structure.md`'s 50-line limit. What it holds is
/// the batch's own effect, computed without touching `self` — which is
/// guarantee 2's shape, and grouping them names it.
#[derive(Default)]
struct Batch<'a> {
    /// The publications and swaps, **in the order the log holds them**, which
    /// is what lets one walk stand in for a check and an apply. Two of them
    /// for one partition interact, and judging each against the pre-batch
    /// index admitted pairs that one page at a time were refused — `M5.13`'s
    /// first round measured three shapes of it.
    effects: Vec<Staged<'a>>,
    /// Staged as (running end offset, entries this batch adds), so a failure
    /// part-way through commits neither.
    staged: StagedSpans<'a>,
}

/// The state every in-memory materialization keeps, and the fold over it.
///
/// ⚠️ Shared by [`FakeMaterializedIndex`](crate::FakeMaterializedIndex) and by
/// `oqueue-index`'s `MemoryIndex` — the module doc above says why, and said it
/// twice verbatim until `M3.37`; the `M3.6` minor that noticed was relocated
/// rather than resolved by `M3.8`'s module split.
///
/// ⚠️ **Nested rather than keyed by `(TopicId, PartitionId)`.** A tuple key has
/// no borrowed form, so every lookup would have to `clone()` the topic name to
/// build one — a heap allocation on the Fetch path NFR-2 and NFR-3 bound, and
/// the very per-call allocation this seam refuses to be async in order to
/// avoid. Nested, a read borrows.
#[derive(Debug, Default)]
pub struct IndexState {
    applied_upto: Option<CommitVersion>,
    partitions: HashMap<TopicId, HashMap<PartitionId, PartitionIndex>>,
    /// How many entries the tiers hold in total — maintained, never counted.
    entries: usize,
}

impl IndexState {
    /// An empty state.
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Guarantee 1 and 2: validate the whole batch, then fold it.
    ///
    /// # Errors
    ///
    /// Five variants, and **the state is left untouched for every one of
    /// them** — guarantee 2 is what makes the list worth reading, because a
    /// caller that retries a rejected batch retries it against the state it
    /// had before:
    ///
    /// - [`Error::NonMonotonicCommitVersion`], a version at or below the one
    ///   already folded;
    /// - [`Error::OffsetOverflow`], a partition's next offset past
    ///   `i64::MAX` — [`Offset`] carries the protocol's `int64` range, not a
    ///   `u64` one, so the ceiling is half where a reader might place it;
    /// - [`Error::EmptyRegion`], a [`CommittedSpan`](crate::CommittedSpan) of
    ///   no records, which would become an [`ObjectRef`] covering no offsets;
    /// - [`Error::ManifestDoesNotMeetHistory`], a `ManifestPublished` whose
    ///   `upto` does not meet what is left of the partition;
    /// - [`Error::IndexObjectMismatch`], a `RangeCompacted` retiring a
    ///   reference the partition's **history** does not hold — a reference
    ///   still inside the tail window is refused too, and compaction's age
    ///   guard is what keeps a swap away from it — or installing a run that
    ///   does not cover exactly what it retires.
    ///
    /// ⚠️ **The list is exhaustive and is meant to stay so.** An applier
    /// deciding abort-versus-retry matches on these, and doc 21 §4's point is
    /// that a variant reaching a caller the docs said could not see it is
    /// indistinguishable from a bug in the caller.
    pub fn apply(&mut self, entries: &[MetadataEntry]) -> Result<()> {
        // ⚠️ Computed into a scratch map first, so a failure part-way through
        // cannot leave a partially folded batch behind — guarantee 2. The
        // scratch holds only the partitions this batch touches.
        let mut previous = self.applied_upto;
        let mut batch = Batch::default();

        for entry in entries {
            if let Some(prev) = previous
                && entry.version() <= prev
            {
                return Err(Error::NonMonotonicCommitVersion {
                    expected_above: prev.get(),
                    got: entry.version().get(),
                });
            }
            previous = Some(entry.version());
            self.read_record(entry.record(), &mut batch)?;
        }
        let Batch { effects, staged } = batch;

        // ⚠️ **Applied to copies first**, guarantee 2 — and the check *is* the
        // fold rather than a second description of it, which is what stopped
        // the two from disagreeing. `projection.rs` says what that cost.
        let projected = self.project(&effects, &staged)?;

        // ⚠️ The only mutation of `self` in this method, and it is after every
        // fallible step — guarantee 2. The clone happens here, on the write
        // path, rather than on the read path a lookup key would have put it.
        for ((topic, partition), (end, entries)) in staged {
            if projected.contains_key(&(topic, partition)) {
                // The projection already holds this partition's commits, in
                // order with the effects between them.
                continue;
            }
            let slot = self
                .partitions
                .entry(topic.clone())
                .or_default()
                .entry(partition)
                .or_default();
            slot.end_offset = end;
            for entry in entries {
                slot.push(entry);
                self.entries += 1;
            }
        }
        for ((topic, partition), slot) in projected {
            // ⚠️ **The total moves by this partition's own count**, because a
            // replaced partition has no delta to add: a swap that retires
            // thirteen and installs one, a publication that absorbs a hundred,
            // and the commits between them are all already folded into the
            // copy.
            let before = self
                .partition(topic, partition)
                .map_or(0, PartitionIndex::entries);
            self.entries = self.entries + slot.entries() - before;
            self.partitions
                .entry(topic.clone())
                .or_default()
                .insert(partition, slot);
        }
        self.applied_upto = previous;
        Ok(())
    }

    /// Reads one record into the scratch this batch is accumulating.
    ///
    /// ⚠️ **Nothing here touches `self`** — guarantee 2. The three kinds that
    /// affect a partition are staged so that the checks below them see the
    /// whole batch before anything is applied.
    fn read_record<'a>(&self, record: &'a MetadataRecord, batch: &mut Batch<'a>) -> Result<()> {
        match record {
            MetadataRecord::BatchCommitted { object, spans } => {
                for span in spans {
                    self.stage_span(&mut batch.staged, object, span)?;
                }
            }
            MetadataRecord::ManifestPublished {
                topic,
                partition,
                manifest,
                upto,
            } => {
                let precedes = precedes(&batch.staged, topic, *partition);
                batch.effects.push(Staged {
                    topic,
                    partition: *partition,
                    effect: Effect::Published {
                        manifest,
                        upto: *upto,
                    },
                    precedes,
                });
            }
            MetadataRecord::RangeCompacted {
                topic,
                partition,
                retiring,
                installing,
            } => {
                let precedes = precedes(&batch.staged, topic, *partition);
                batch.effects.push(Staged {
                    topic,
                    partition: *partition,
                    effect: Effect::Swapped {
                        retiring,
                        installing,
                    },
                    precedes,
                });
            }
            // An event about the log, not about any partition.
            MetadataRecord::EpochChanged { .. } => {}
        }
        Ok(())
    }

    /// The highest version folded in.
    #[must_use]
    pub const fn applied_upto(&self) -> Option<CommitVersion> {
        self.applied_upto
    }

    /// The offset the next record for this partition will occupy.
    #[must_use]
    pub fn end_offset(&self, topic: &TopicId, partition: PartitionId) -> Offset {
        self.partition(topic, partition)
            .map_or(Offset::ZERO, |p| p.end_offset)
    }

    /// The partition's tail window: entries carrying inline byte ranges, so
    /// reading one is a single GET.
    ///
    /// ⚠️ **No production caller, and deliberately so.** A fetch goes through
    /// [`find_batches`](Self::find_batches), which pages and prices; this hands
    /// back the whole window so the suite can assert what demotion did to it.
    /// It clones — up to [`TAIL_WINDOW_ENTRIES`] entries with an
    /// [`ObjectKey`](crate::ObjectKey) each — which is fine off the read path
    /// and worth saying in a type that refuses a tuple key to avoid one clone.
    #[must_use]
    pub fn tail(&self, topic: &TopicId, partition: PartitionId) -> Vec<TailEntry> {
        self.partition(topic, partition)
            .map(|p| p.tail.iter().cloned().collect())
            .unwrap_or_default()
    }

    /// How many entries the partition has demoted to the history tier.
    ///
    /// ⚠️ A count rather than the entries themselves: resolving one needs the
    /// object's own footer, which is the read path `M3.8` builds and not this
    /// type's to perform.
    #[must_use]
    pub fn history_len(&self, topic: &TopicId, partition: PartitionId) -> usize {
        self.partition(topic, partition)
            .map_or(0, |p| p.history.len())
    }

    /// The objects a fetch from `start` must read — the fold's own answer to
    /// [`MaterializedIndex::find_batches`](crate::MaterializedIndex::find_batches),
    /// which every implementation here
    /// delegates to.
    ///
    /// ⚠️ History before tail, because history is older: the two collections
    /// are each in ascending offset order and the concatenation is too, which
    /// is what makes "ascending offset order" true without a sort.
    ///
    /// # Errors
    ///
    /// [`Error::OffsetOverflow`] if a stored entry's end offset is
    /// unrepresentable.
    pub fn find_batches(
        &self,
        topic: &TopicId,
        partition: PartitionId,
        start: Offset,
        max_bytes: u64,
    ) -> Result<Vec<IndexedBatch>> {
        let Some(slot) = self.partition(topic, partition) else {
            return Ok(Vec::new());
        };
        let mut page = Page::new(max_bytes);
        // ⚠️ **Not a scan from element zero**, and the difference is the FR-12
        // case. `history` is unbounded — `M3.md` sizes it in the millions per
        // busy partition — while the tail is capped at
        // [`TAIL_WINDOW_ENTRIES`]. A consumer sitting at the high watermark
        // asks this question on every fetch and the honest answer is "nothing",
        // so walking millions of entries to produce it would put the read
        // NFR-2 bounds at 10 ms against pure memory traffic. Base offsets are
        // ascending and gap-free, so the first entry that can still hold
        // `start` is one binary search away.
        for reference in &slot.history[Self::first_candidate(&slot.history, start)..] {
            // ⚠️ `end_offset() > start`, not `contains(start)`: everything from
            // `start` onward is wanted, so an object entirely past it is in the
            // page too — `contains` would return only the one holding `start`.
            if reference.end_offset()? > start
                && !page.push(IndexedBatch::Footer(reference.clone()))
            {
                return Ok(page.finish());
            }
        }
        for entry in &slot.tail {
            if entry.reference().end_offset()? > start
                && !page.push(IndexedBatch::Inline(entry.clone()))
            {
                return Ok(page.finish());
            }
        }
        Ok(page.finish())
    }

    /// The index of the earliest history entry that can still hold `start`.
    ///
    /// ⚠️ **One back from the partition point, not the partition point
    /// itself.** The entry *containing* `start` is the last one whose base is
    /// at or below it, and that is the one a fetch has to begin at — starting
    /// at the partition point would skip the object holding the very record
    /// asked for. The caller's `end_offset() > start` filter then drops that
    /// candidate on the one occasion it is spent (`start` exactly at its end),
    /// which is why this may be approximate by one and never by more.
    fn first_candidate(history: &[ObjectRef], start: Offset) -> usize {
        history
            .partition_point(|reference| reference.base_offset() <= start)
            .saturating_sub(1)
    }

    fn partition(&self, topic: &TopicId, partition: PartitionId) -> Option<&PartitionIndex> {
        self.partitions
            .get(topic)
            .and_then(|parts| parts.get(&partition))
    }

    /// How many entries this index holds, across every partition and both
    /// tiers.
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
        self.entries
    }

    /// The manifest naming this partition's older history, and how far it
    /// covers.
    ///
    /// ⚠️ **The one thing about a manifest this index knows.** It holds the
    /// key and the offset, never the entries — that is `ADR-0042`'s point: a
    /// partition's seven-day history at doc 14 §3's working set is 225.6 TB of
    /// entries and 86 B of this. What the entries say is in the object, and
    /// reading it is the reader's, not the fold's.
    #[must_use]
    pub fn manifest(&self, topic: &TopicId, partition: PartitionId) -> Option<(ObjectKey, Offset)> {
        self.partition(topic, partition)
            .and_then(|slot| slot.manifest.clone())
    }

    /// Returns it to its fresh state.
    pub fn clear(&mut self) {
        self.applied_upto = None;
        self.partitions.clear();
        self.entries = 0;
    }
}

#[cfg(test)]
mod tests {
    // A panic in a test harness is the test failing, which is what it is for.
    #![allow(clippy::expect_used)]

    use super::IndexState;
    use crate::{ObjectKey, ObjectRef, Offset};

    fn refs(bases_and_counts: &[(i64, u32)]) -> Vec<ObjectRef> {
        bases_and_counts
            .iter()
            .map(|(base, count)| {
                ObjectRef::new(
                    ObjectKey::new(format!("obj-{base}")).expect("a valid key"),
                    Offset::new(*base).expect("a valid offset"),
                    *count,
                )
            })
            .collect()
    }

    fn at(history: &[ObjectRef], start: i64) -> usize {
        IndexState::first_candidate(history, Offset::new(start).expect("a valid offset"))
    }

    /// ⚠️ **The search, not the filter.** Returning `0` here is correct output
    /// with the wrong cost — the caller's `end_offset > start` filter drops
    /// what the search should have skipped — so a test that only looked at
    /// `find_batches`'s result would pass against a full scan of a vector
    /// `M3.md` sizes in the millions. This looks at the index itself.
    #[test]
    fn the_history_search_lands_on_the_entry_that_still_holds_start() {
        let history = refs(&[(0, 1), (1, 1), (2, 1)]);
        assert_eq!(at(&history, 0), 0);
        assert_eq!(at(&history, 1), 1);
        assert_eq!(at(&history, 2), 2, "not a scan from zero");
    }

    /// ⚠️ **One back from the partition point.** The entry containing `start`
    /// is the last one whose base is at or below it, so landing on the
    /// partition point itself would skip the object holding the very record
    /// asked for.
    #[test]
    fn the_history_search_lands_inside_a_multi_record_entry() {
        let history = refs(&[(0, 2), (2, 2), (4, 2)]);
        assert_eq!(at(&history, 3), 1, "[2, 4) holds offset 3");
        assert_eq!(at(&history, 4), 2);
        assert_eq!(at(&history, 5), 2, "[4, 6) holds offset 5");
    }

    /// Past the end, and on an empty tier: approximate by one at most, never
    /// out of bounds.
    #[test]
    fn the_history_search_stays_in_bounds_at_both_edges() {
        assert_eq!(at(&[], 0), 0, "an empty tier has nothing to point at");
        let history = refs(&[(0, 1), (1, 1)]);
        assert_eq!(
            at(&history, 99),
            1,
            "past everything: the last entry, which the caller's filter drops"
        );
    }
}
