//! The accounting one walk of a range does, object by object.
//!
//! ⚠️ **Its own module because the paging and the accounting are two
//! concepts** (`code-structure.md` rule 18). [`read_amp`](super::read_amp)
//! owns the loop over `find_batches` pages; what the batches contribute to the
//! measurement is here, and holding both in one function put that function
//! past the 50-line limit once `M5.46` added the object-alignment test.
//!
//! ⚠️ **The unit is an object, not a span, and that distinction is the whole
//! of `M5.46`'s second round.** One object may hold several disjoint spans of
//! one partition — `IndexState`'s fold admits a partition appearing twice in
//! one batch, and `M3.8` calls it ordinary. A filter applied per span admits
//! half an object whenever a range edge falls between two of its spans: the
//! measurement then counts five records where the object holds ten, `merge`
//! reads the object once and takes every region it holds for the partition,
//! and the round fails `take_regions`' record-count check with every other
//! partition's work inside it. So spans are folded into their object first and
//! the range is decided over objects afterwards.

use oqueue_core::{IndexedBatch, ObjectKey, Offset, Result};
use std::collections::{HashMap, HashSet};

use super::{ReadAmp, objects_needed};

/// Everything one object holds for the partition, across all of its spans.
#[derive(Debug, Clone)]
struct Extent {
    first_base: Offset,
    last_end: Offset,
    records: i64,
    tail: bool,
    /// ⚠️ **Sticky, and only ever set false.** One span outside the range
    /// disqualifies the object it belongs to, however many of its siblings sit
    /// comfortably inside.
    whole: bool,
}

/// What one walk of a range accumulates.
#[derive(Debug)]
pub(super) struct Walk {
    start: Offset,
    end: Offset,
    /// Every object the walk has seen, with what it holds, in the order the
    /// walk first saw each — ascending by the span that introduced it.
    ///
    /// ⚠️ **One collection, not an order beside a map.** The two were separate
    /// until `M5.54`, and `finish` then had to ask the map for a key the order
    /// named — a lookup nothing could make fail, since `visit` writes both in
    /// one arm and neither is ever removed from, so the `else` branch was one
    /// no index could take. `M5.43` closed on an unreachable branch being
    /// worse than none: it reads as a case that happens.
    seen: Vec<(ObjectKey, Extent)>,
    /// Where each object sits in [`seen`](Self::seen), so folding a second
    /// span into it stays a lookup rather than a scan.
    at: HashMap<ObjectKey, usize>,
    /// Objects the walk knows are not whole without having seen why.
    ///
    /// ⚠️ **Because a walk from `start` cannot see what lies below it.**
    /// `find_batches` answers from the batch holding the cursor, so an object
    /// whose earlier span ends exactly at `start` is never returned — and the
    /// object then looks entirely inside a range it straddles. `read_amp`
    /// probes one page below the start and names those objects here.
    barred: HashSet<ObjectKey>,
}

impl Walk {
    pub(super) fn over(start: Offset, end: Offset) -> Self {
        Self {
            start,
            end,
            seen: Vec::new(),
            at: HashMap::new(),
            barred: HashSet::new(),
        }
    }

    /// Names an object the walk will not see the whole of.
    pub(super) fn bar(&mut self, key: &ObjectKey) {
        self.barred.insert(key.clone());
    }

    /// Folds one batch into its object, returning where that batch ends — or
    /// `None` when the batch is past the range and the page's remainder is too.
    ///
    /// ⚠️ **A batch past the end is folded before the walk stops.** It is the
    /// far edge of the same defect the module header describes: an object
    /// whose later span begins at or after `end` would otherwise be admitted
    /// on its earlier span alone, and a ref carrying half an object's records
    /// into a GET that returns all of them is what `take_regions` refuses.
    pub(super) fn visit(&mut self, batch: &IndexedBatch) -> Result<Option<Offset>> {
        let reference = batch.reference();
        let base = reference.base_offset();
        let object_end = reference.end_offset()?;
        let past = base >= self.end;

        let key = reference.object().clone();
        // ⚠️ A span reaching below the start disqualifies its object, and the
        // walk sees such a span because `find_batches` answers from the batch
        // holding the cursor rather than from the next one after it. That is
        // what makes the disqualification possible at all.
        let whole = !past && base >= self.start && object_end <= self.end;
        if let Some(&at) = self.at.get(&key) {
            let extent = &mut self.seen[at].1;
            extent.first_base = extent.first_base.min(base);
            extent.last_end = extent.last_end.max(object_end);
            extent.records += object_end.get() - base.get();
            extent.tail |= batch.bytes().is_some();
            extent.whole &= whole;
        } else {
            self.at.insert(key.clone(), self.seen.len());
            self.seen.push((
                key,
                Extent {
                    first_base: base,
                    last_end: object_end,
                    records: object_end.get() - base.get(),
                    // An inline byte range is what the tail tier is: the index
                    // already knows where to read, so the fetch is one GET.
                    tail: batch.bytes().is_some(),
                    whole,
                },
            ));
        }

        if past { Ok(None) } else { Ok(Some(object_end)) }
    }

    /// The measurement, over the objects that lie wholly inside the range and
    /// tile one run of it.
    ///
    /// ⚠️ **A run, not a set.** Compaction rewrites a contiguous range, and
    /// `merge`'s `tiling()` refuses anything else — so an admissible object
    /// that does not begin where the last one ended ends the run rather than
    /// joining it, and what is reported is the range actually covered.
    pub(super) fn finish(self) -> ReadAmp {
        let mut covered: Option<(Offset, Offset)> = None;
        let mut objects_touched = 0_usize;
        let mut records = 0_i64;
        let mut tail_objects = 0_usize;
        let mut first_tail_base: Option<Offset> = None;
        let mut history_objects = 0_usize;
        let mut history_records = 0_i64;

        for (key, extent) in &self.seen {
            if !extent.whole || self.barred.contains(key) {
                continue;
            }
            match covered {
                None => covered = Some((extent.first_base, extent.last_end)),
                Some((low, high)) => {
                    if extent.first_base != high {
                        break;
                    }
                    covered = Some((low, extent.last_end));
                }
            }
            objects_touched += 1;
            records += extent.records;
            if extent.tail {
                tail_objects += 1;
                // Ascending order, so the first one seen is the boundary --
                // `min` rather than a first-write-wins flag, because ordering
                // is `find_batches`' guarantee and not this walk's.
                first_tail_base =
                    Some(first_tail_base.map_or(extent.first_base, |base: Offset| {
                        base.min(extent.first_base)
                    }));
            } else if first_tail_base.is_none() {
                // Ascending order, so "no tail object yet" is "still history".
                history_objects += 1;
                history_records += extent.records;
            }
        }

        ReadAmp {
            objects_touched,
            objects_needed: objects_needed(records),
            records,
            tail_objects,
            first_tail_base,
            history_objects,
            history_records,
            covered,
        }
    }
}
