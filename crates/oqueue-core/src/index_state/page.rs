//! `find_batches`'s read surface: the page a cold read is bounded by
//! (`M10.20`, from `M3.44`).
//!
//! ⚠️ **The natural cut from the fold beside it.** `index_state.rs` keeps
//! `apply` and tier demotion — the write side, folding a metadata log into
//! `IndexState` — while this holds `Page` and the count it bounds, the type
//! [`super::IndexState::find_batches`] pages results through. `push`'s own
//! doc carries the contiguity rule a caller must respect, unchanged by the
//! move: it was module-private and enforced by nothing but its own comment
//! before this split, and it still is now — becoming visible to the parent
//! module rather than staying invisible to everything did not turn that
//! comment into a checked invariant, so this note exists to say so rather
//! than let the visibility change read as if it had.

use crate::IndexedBatch;

/// How many batches one [`MaterializedIndex::find_batches`] page may name.
///
/// ⚠️ **The bound on a cold read**, and the reason `max_bytes` alone is not
/// one: every history batch's length is unknown until its footer is read, so a
/// byte budget can price none of them (`ADR-0022`). Without a count the page
/// would be the whole of a partition's history; with it, NFR-30's "bounded
/// GETs, zero LIST" has a number behind it.
///
/// ⚠️ **UNDERIVED**, exactly like [`super::TAIL_WINDOW_ENTRIES`]. It is a
/// placeholder with the right shape, chosen so a page's footer resolution
/// stays in the tens of GETs rather than the thousands; `M14` measures what it
/// should be against a real workload.
///
/// It is a constant rather than a knob, per `AGENTS.md` non-negotiable 2.
///
/// [`MaterializedIndex::find_batches`]: crate::MaterializedIndex::find_batches
pub const MAX_BATCHES_PER_PAGE: usize = 64;

/// A page of batches under two bounds, and the rule that closes it.
///
/// ⚠️ Its own type rather than four locals, because the rule has interacting
/// parts — always yield one, charge only a length the index knows, cap the
/// count — and `find_batches` would otherwise carry them at a nesting depth
/// `clippy.toml`'s cognitive-complexity threshold exists to refuse.
#[derive(Debug)]
pub(super) struct Page {
    batches: Vec<IndexedBatch>,
    spent: u64,
    max_bytes: u64,
}

impl Page {
    pub(super) const fn new(max_bytes: u64) -> Self {
        Self {
            batches: Vec::new(),
            spent: 0,
            max_bytes,
        }
    }

    /// Adds a batch if both bounds allow. Returns whether it was added.
    ///
    /// ⚠️ **A `false` ends the page, and the caller must not skip past it to a
    /// batch that would have fit.** The reason is contiguity, not size: what a
    /// fetch is handed is a *run* of a partition's log, and a run with a hole
    /// in it is records vanishing from a consumer's stream with no error
    /// anywhere. A later batch being smaller is common and is not a licence to
    /// take it.
    pub(super) fn push(&mut self, batch: IndexedBatch) -> bool {
        // ⚠️ `is_empty()` guards both bounds: the very first batch is admitted
        // however large, or a partition whose next object exceeds `max_bytes`
        // would return empty forever and the consumer would never advance.
        if !self.batches.is_empty() {
            if self.batches.len() >= MAX_BATCHES_PER_PAGE {
                return false;
            }
            // ⚠️ A batch the index cannot price charges nothing. It is not
            // free — it is unpriceable, and pretending otherwise would enforce
            // a budget against an invented number (`ADR-0022`).
            if let Some(len) = batch.known_len()
                && self.spent.saturating_add(len) > self.max_bytes
            {
                return false;
            }
        }
        if let Some(len) = batch.known_len() {
            self.spent = self.spent.saturating_add(len);
        }
        self.batches.push(batch);
        true
    }

    pub(super) fn finish(self) -> Vec<IndexedBatch> {
        self.batches
    }
}
