//! One partition's tiers, and what a manifest does to them.
//!
//! ⚠️ **Its own module because a partition's state and the fold over the log
//! are two concepts** (`code-structure.md` rule 18), and `index_state.rs`
//! reached the 500-line limit holding both once `M5.62` added publication.
//! That file is the fold — order checking, all-or-nothing application, the
//! staging that makes it so; this is what one partition holds and the three
//! things that can happen to it: a push, a demotion, and a manifest absorbing
//! its history.

use crate::{ObjectKey, ObjectRef, Offset, TailEntry, Tiers, TimeSpan};
use core::cmp::Ordering;
use std::collections::VecDeque;

use super::TAIL_WINDOW_ENTRIES;

/// One partition's two tiers, and the offset they fold to.
///
/// ⚠️ **Two collections rather than one of a two-variant enum.** An enum is as
/// large as its widest variant, so a history entry would pay for the tail's
/// inline [`ByteRange`](crate::ByteRange) whether or not it carried one.
/// Separate collections keep an [`ObjectRef`] at its ~40-byte inline budget.
///
/// ⚠️ This is the two-tier *shape* and not yet the coarse index doc 15 §7
/// resolves doc 10 #8 to: entries are still keyed per (object, partition), so
/// the count is doc 14 §3's ~4M/s row however lean each one is. See
/// [`ObjectRef`] for what closing that would take.
///
/// ⚠️ `Default` by hand rather than derived: `Offset` deliberately has no
/// `Default`, because "the zero offset" and "no offset" are different claims
/// and a derive would quietly pick one. A fresh partition genuinely starts at
/// [`Offset::ZERO`], and saying so here is the only place that choice is made.
#[derive(Debug, Clone)]
pub(super) struct PartitionIndex {
    /// Where the next record lands — the fold of every span's count.
    pub(super) end_offset: Offset,
    /// The hot window: byte ranges inline, so a tail read is one GET.
    pub(super) tail: VecDeque<TailEntry>,
    /// Everything older that is not yet in a manifest: refs only, ranges
    /// resolved from each object's own footer at 1–3 GETs.
    pub(super) history: Vec<ObjectRef>,
    /// Everything older *than that*, as one object (`ADR-0042`).
    ///
    /// ⚠️ **One reference where there were entries**, which is what makes this
    /// state bounded: at doc 14 §3's working set a partition's seven-day
    /// history is 225.6 TB of entries and 86 B of this. It covers `[0, upto)` and
    /// `history` (or the tail, when `history` is empty) begins exactly at
    /// `upto` — the fold refuses anything else.
    pub(super) manifest: Option<(ObjectKey, Offset)>,
    /// When this partition's commits happened (`M5.86`).
    ///
    /// ⚠️ **`None` until something is committed**, and that is a different
    /// claim from "committed at the epoch": a partition the fold knows about
    /// only because a manifest was published for it has no commit time of its
    /// own, and a retention round that read `EPOCH` there would reap it
    /// immediately. `Offset` has no `Default` for the same reason one field up.
    pub(super) when: Option<TimeSpan>,
    /// The first readable offset (`M5.19`).
    ///
    /// ⚠️ **Zero until a trim moves it**, and it only moves forward: a trim at
    /// or below it is a no-op, so a retention round re-issuing one after a
    /// crash cannot move a partition's start backwards over deleted data.
    pub(super) log_start: Offset,
}

impl Default for PartitionIndex {
    fn default() -> Self {
        Self {
            end_offset: Offset::ZERO,
            tail: VecDeque::new(),
            history: Vec::new(),
            manifest: None,
            when: None,
            log_start: Offset::ZERO,
        }
    }
}

impl PartitionIndex {
    /// Whether `upto` is a boundary between two of this partition's objects,
    /// and if not, the next one above it.
    ///
    /// ⚠️ **Boundaries, not tiers.** A partition's records are dense and every
    /// object's span is contiguous, so what a manifest must meet is some
    /// object's base offset — or the partition's end, when it covers
    /// everything. Which *tier* that object is in does not matter and must
    /// not, because a publication arriving in the same batch as the commits it
    /// covers has to see them: a replay pages the log in arbitrary chunks, and
    /// a fold that refused a page holding both could not replay one.
    pub(super) fn boundary(&self, upto: Offset) -> Option<Offset> {
        // ⚠️ **A manifest may cover history and never the tail.** `absorb`
        // retains over `history` alone, so a manifest reaching into the tail
        // would leave every tail entry below it in place — and those records
        // would then be named twice, by the manifest and by the tail, which no
        // reader can tell from a partition genuinely holding them twice. It is
        // also what compaction produces: the age guard refuses to rewrite tail
        // data at all. Found by `M5.62`'s third round.
        //
        // ⚠️ **Where the tail starts, full stop.** This used to take the
        // batch's staged entries and fold them in, because a publication
        // arriving with the commits it covers has to see the arrangement they
        // produce. It still does — `projection.rs` pushes those commits into
        // this partition's copy *before* asking, in log order — so the
        // question here is about a partition as it stands, which is the only
        // question a partition can answer. `M5.13`.
        let bases: Vec<Offset> = self
            .history
            .iter()
            .map(ObjectRef::base_offset)
            .chain(
                self.tail
                    .iter()
                    .map(|entry| entry.reference().base_offset()),
            )
            .collect();
        // ⚠️ `saturating_sub`, and the empty case falls through to
        // `end_offset`: a partition with no entries at all has one boundary,
        // its own end, and a manifest covering everything meets it.
        let first_tail = bases.len().saturating_sub(TAIL_WINDOW_ENTRIES);
        let limit = bases.get(first_tail).copied().unwrap_or(self.end_offset);
        if upto == limit {
            return None;
        }
        let mut next: Option<Offset> = None;
        for base in &bases[..first_tail] {
            match base.cmp(&upto) {
                Ordering::Equal => return None,
                Ordering::Greater => {
                    next = Some(next.map_or(*base, |held: Offset| held.min(*base)));
                }
                Ordering::Less => {}
            }
        }
        Some(next.unwrap_or(limit))
    }

    /// Replaces the history a manifest covers with one reference to it.
    ///
    /// Returns how many entries it removed, and whether it superseded a
    /// manifest this partition already had.
    pub(super) fn absorb(&mut self, manifest: ObjectKey, upto: Offset) -> (usize, bool) {
        let before = self.history.len();
        self.history
            .retain(|entry| entry.end_offset().is_ok_and(|end| end > upto));
        let superseded = self.manifest.is_some();
        self.manifest = Some((manifest, upto));
        (before - self.history.len(), superseded)
    }

    /// Pushes a newly committed object onto the tail, demoting whatever falls
    /// out of the window.
    ///
    /// ⚠️ **Demotion moves an entry between tiers and never removes one.** The
    /// total therefore rises by exactly one per push, but the *tiers* do not —
    /// a push at a full window leaves the tail where it was and adds to
    /// history. `IndexState`'s accounting takes this partition's
    /// [`tiers`](Self::tiers) before and after a batch's pushes for that
    /// reason (`M5.71`), so a change here that removed an entry rather than
    /// moving it is a change to what that arithmetic means.
    /// ⚠️ `if`, not `while`: entries arrive one at a time, so at most one can
    /// fall out of the window per push. A loop here would be an unbounded one
    /// whose bound is a comparison — and a mutation flipping that comparison
    /// turns it into a hang rather than a failure, which is a worse way to
    /// find out.
    pub(super) fn push(&mut self, entry: TailEntry) {
        self.tail.push_back(entry);
        if self.tail.len() > TAIL_WINDOW_ENTRIES
            && let Some(evicted) = self.tail.pop_front()
        {
            self.history.push(evicted.demote());
        }
    }

    /// Drops every entry wholly below `start` and moves the log start there.
    ///
    /// ⚠️ **Wholly below, not overlapping.** An object whose span straddles
    /// `start` still holds live records, so its reference stays and a read
    /// from `start` resolves into it; dropping it would lose `[start, end)`.
    /// The manifest goes only when everything it covers is dead, for the same
    /// reason.
    ///
    /// ⚠️ **Removes references, never objects.** What it drops is an entry in
    /// this index; whether the object behind it can be deleted is a question
    /// about every *other* partition in it, which is liveness's (`M5.20`).
    pub(super) fn trim(&mut self, start: Offset) {
        if start <= self.log_start {
            return;
        }
        let dead = |end: Result<Offset, crate::Error>| end.is_ok_and(|end| end <= start);
        self.history.retain(|entry| !dead(entry.end_offset()));
        self.tail
            .retain(|entry| !dead(entry.reference().end_offset()));
        if self
            .manifest
            .as_ref()
            .is_some_and(|(_, upto)| *upto <= start)
        {
            self.manifest = None;
        }
        self.log_start = start;
    }

    /// Folds a batch's commit time into this partition's extent.
    pub(super) fn observe(&mut self, when: TimeSpan) {
        self.when = Some(self.when.map_or(when, |held| held.widened(when)));
    }

    /// Whether this reference is one of the history entries, by identity
    /// rather than by offset.
    ///
    /// ⚠️ **By identity, because two objects can name the same offsets.** A
    /// compaction's own output covers the range its inputs did, so a check
    /// asking "is something here at this base offset" would accept a swap
    /// retiring the object it just installed.
    pub(super) fn holds_in_history(&self, reference: &ObjectRef) -> bool {
        self.history.iter().any(|held| held == reference)
    }

    /// Swaps a run of history references for another.
    ///
    /// ⚠️ **Sorted afterwards, because the tiers are read in offset order.**
    /// `find_batches` walks history expecting ascending base offsets, and an
    /// installed reference appended at the end would be invisible to a fetch
    /// that stopped at the first entry past its start.
    pub(super) fn replace_history(&mut self, retiring: &[ObjectRef], installing: &[ObjectRef]) {
        self.history
            .retain(|held| !retiring.iter().any(|going| going == held));
        self.history.extend(installing.iter().cloned());
        self.history.sort_by_key(ObjectRef::base_offset);
    }

    /// What this partition holds, per tier.
    ///
    /// ⚠️ **Counted, and only here.** `IndexState` maintains its totals rather
    /// than walking the map, but a partition the batch *replaced* wholesale
    /// has no delta to add — so the totals move by this partition's counts
    /// before and after. ⚠️ **Not a walk**, despite reading like one: all
    /// three terms are a collection's `len()` or an `Option`'s discriminant,
    /// so this is O(1) and calling it twice per touched partition is what
    /// keeps the maintenance O(1) too.
    pub(super) fn tiers(&self) -> Tiers {
        Tiers {
            tail: self.tail.len(),
            history: self.history.len(),
            manifests: usize::from(self.manifest.is_some()),
        }
    }
}
