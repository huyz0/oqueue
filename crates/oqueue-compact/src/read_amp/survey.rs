//! The measurement [`read_amp`](super::read_amp) produces.
//!
//! ⚠️ **Its own module because the walk and what the walk reports are two
//! concepts** (`code-structure.md` rule 18, and `oqueue-core`'s own
//! `index_state`/`page` split): this file is the reading, `read_amp.rs` is the
//! taking of it, and the one file reached the 500-line limit holding both.

use oqueue_core::Offset;

/// What a range costs to read, against what it could cost.
///
/// ⚠️ **A measurement, not a verdict.** Whether a range is worth compacting is
/// the planner's question (`M5.2`), and it needs the range's *age* as well as
/// this — tail data still served from cache is never rewritten however
/// amplified it looks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReadAmp {
    pub(super) objects_touched: usize,
    pub(super) objects_needed: usize,
    pub(super) records: i64,
    pub(super) tail_objects: usize,
    pub(super) first_tail_base: Option<Offset>,
    pub(super) history_objects: usize,
    pub(super) history_records: i64,
}

impl ReadAmp {
    /// Objects a fetch over the range must read today.
    #[must_use]
    pub const fn objects_touched(&self) -> usize {
        self.objects_touched
    }

    /// Objects the same records would occupy at the compacted layout —
    /// `ceil(records / COMPACTED_OBJECT_RECORDS)`, and never zero while any
    /// record is in range.
    #[must_use]
    pub const fn objects_needed(&self) -> usize {
        self.objects_needed
    }

    /// Records the range holds, as the index accounts for them.
    #[must_use]
    pub const fn records(&self) -> i64 {
        self.records
    }

    /// How many of the touched objects are still in the index's tail tier.
    ///
    /// ⚠️ **The witness the age guard's tests assert on, not its input**
    /// (`M5.41` corrects `M5.2`, which said the opposite). The planner matches
    /// on [`first_tail_base`](Self::first_tail_base) — that is what trims a
    /// straddling range — and a maintainer who believed this sentence would
    /// delete the boundary as redundant and restore the defect that made a
    /// live partition permanently uncompactable. This count is a proxy for the
    /// range's age rather than the thing itself: the tail tier is the hot window by construction — byte
    /// ranges inline, one GET — so "still in the tail" is what this index can
    /// say in place of "still inside the latency SLO". `M5.16` gives the index
    /// `ts_min`/`ts_max` and the guard becomes time-based there.
    ///
    /// ⚠️ **It reads a tier off a property the contract does not promise is
    /// one.** [`IndexedBatch::bytes`](oqueue_core::IndexedBatch::bytes) is
    /// documented as where inside the object to read *if the index knows* — a
    /// known-length property every implementation answers from the tail window
    /// today, because all of them are the one fold in `oqueue-core`. An index
    /// that answered `None` for everything, a snapshot-backed one say, would
    /// report no tail objects at all, and the age guard would vanish silently
    /// rather than fail. That is the second reason `M5.16`'s timestamps are
    /// where this ends up.
    #[must_use]
    pub const fn tail_objects(&self) -> usize {
        self.tail_objects
    }

    /// The same measurement over the range's history portion alone.
    ///
    /// ⚠️ **Accumulated during the one walk, not by walking again**
    /// (`M5.41`). The planner trims a straddling range at
    /// [`first_tail_base`](Self::first_tail_base) and needs the amplification
    /// of what is left; measuring that with a second `read_amp` call meant two
    /// index walks per candidate, on the path that by the planner's own doc a
    /// live partition always takes — ~200k page walks per sweep at 100k
    /// partitions where 100k do.
    #[must_use]
    pub fn before_tail(&self) -> Self {
        Self {
            objects_touched: self.history_objects,
            objects_needed: super::objects_needed(self.history_records),
            records: self.history_records,
            tail_objects: 0,
            first_tail_base: None,
            history_objects: self.history_objects,
            history_records: self.history_records,
        }
    }

    /// Where the tail tier starts inside the measured range, if it does.
    ///
    /// ⚠️ **What lets a straddling range be trimmed rather than declined**
    /// (`M5.2`). A live partition's range naturally runs from cold history
    /// into the hot window, and a planner that declined the whole of it would
    /// leave the most amplified partition in the system permanently
    /// uncompacted — which is the case FR-34 exists for.
    #[must_use]
    pub const fn first_tail_base(&self) -> Option<Offset> {
        self.first_tail_base
    }

    /// The ratio compaction is triggered on.
    ///
    /// ⚠️ **`0.0` for a range holding nothing**, which is not a low ratio but
    /// the absence of one: there is no work, and a planner comparing against a
    /// threshold must not read it as "already compact enough" in a context
    /// where that would mean something different. `1.0` is the compacted
    /// layout.
    #[must_use]
    pub fn ratio(&self) -> f64 {
        if self.objects_needed == 0 {
            return 0.0;
        }
        #[expect(
            clippy::cast_precision_loss,
            reason = "both counts are object counts over one partition's range; \
                      f64 is exact to 2^53 and the index cannot hold that many"
        )]
        {
            self.objects_touched as f64 / self.objects_needed as f64
        }
    }
}
