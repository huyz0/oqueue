//! What the swap installs must cover what it retires.
//!
//! ⚠️ **The one operation that can lose acknowledged data without anything
//! failing** (NFR-20, `M5.12`). A compaction commit replaces a run of input
//! references with a run of output ones, and every offset the inputs named
//! must be named by the outputs afterwards. An output set missing one range
//! produces an index that is internally consistent, an object store that is
//! healthy, and a consumer whose poll returns nothing where records were
//! acknowledged.
//!
//! ⚠️ **Cheap insurance, and that is the whole argument for it.** The check is
//! a sort and a walk over two small lists of references — no store access, no
//! record parsing — against a failure that is silent and permanent. `M5.md`
//! task 9 calls it exactly that.
//!
//! ⚠️ **In `oqueue-core`, not in `oqueue-compact`, because the fold reads
//! it.** `M5.12` put it beside the merge that produces the outputs; `M5.13`
//! made the *coordinator* the caller — `IndexState` refuses a swap whose
//! outputs do not cover its inputs — and `oqueue-compact` depends on this
//! crate rather than the other way round. `oqueue-compact` re-exports it, so
//! the merge side reads the same function it always did.
//!
//! ⚠️ **It is not the same check `merge`'s `tiling()` makes.** That one
//! verifies the *inputs* tile the plan's range before anything is read; this
//! one verifies the *outputs* cover the inputs before anything is swapped. The
//! contiguity rule underneath them is one function, [`contiguous_span`], so
//! the two cannot drift into different notions of what a gap is.

use crate::{Error, ObjectRef, Offset, Result};

/// The exact offset range a set of references covers, or an error if they do
/// not cover one.
///
/// ⚠️ **Contiguous, ascending, no gap, no overlap, and nothing empty.** A gap is records the
/// set does not hold; an overlap is records it holds twice, which a reader
/// cannot tell from a partition that genuinely has them twice. `None` for an
/// empty set, which is a set covering nothing rather than a set covering
/// everything — the distinction the caller has to make, not this function.
///
/// ⚠️ **It sorts a copy.** Asking the caller for ascending order is asking for
/// a precondition nothing checks, and the cost here is a sort of a list whose
/// length is a compaction round's input count.
///
/// # Errors
///
/// [`Error::IndexObjectMismatch`] if the references leave a gap, overlap, or
/// include one covering no offsets, and [`Error::OffsetOverflow`] if one of
/// their own end offsets is unrepresentable.
pub fn contiguous_span(refs: &[&ObjectRef]) -> Result<Option<(Offset, Offset)>> {
    let mut ordered: Vec<&&ObjectRef> = refs.iter().collect();
    ordered.sort_by_key(|reference| reference.base_offset());
    let mut span: Option<(Offset, Offset)> = None;
    for reference in ordered {
        let base = reference.base_offset();
        let end = reference.end_offset()?;
        // ⚠️ **A reference covering no offsets is refused, and it is neither a
        // gap nor an overlap** — which is why the contiguity test alone let it
        // through. Measured by `M5.13`'s third round: an `installing` list of
        // `merged@0+1` and `zero@1+0` spans exactly what it retired, so the
        // swap was accepted, and the zero-length reference then sorted into
        // history *ahead of* the real object at the same base offset. A fetch
        // resuming at that offset partition-points onto the empty one, finds
        // its end is not past the start, and resumes at the next object — the
        // acknowledged record at that offset is never returned. Silent, and
        // only from the offset a consumer actually resumes at.
        if end == base {
            return Err(Error::IndexObjectMismatch);
        }
        span = Some(match span {
            None => (base, end),
            Some((low, high)) => {
                if base != high {
                    return Err(Error::IndexObjectMismatch);
                }
                (low, end)
            }
        });
    }
    Ok(span)
}

/// Refuses a swap whose outputs do not cover its inputs.
///
/// ⚠️ **A refusal, not a warning** (`M5.12`'s row, in those words). A
/// compaction that cannot prove it kept every offset is a compaction that does
/// not happen; the inputs stay live and the next sweep tries again, which
/// costs a round and loses nothing.
///
/// ⚠️ **It knows nothing about partitions, and the caller must.** An
/// [`ObjectRef`] carries an object key, a base offset and a record count —
/// no topic and no partition — so two references from different partitions
/// with the same offsets compare equal here and a swap between them is
/// admitted. Offsets are per partition, so a set mixing two of them describes
/// no range at all. Compaction plans one partition at a time
/// (`CompactionPlan` carries the pair), which is why this takes the cheaper
/// type; a caller that ever hands over a mixed set is asking a question this
/// cannot answer rather than getting a wrong answer to the right one.
///
/// ⚠️ **Equality, not containment.** Outputs covering *more* than the inputs
/// is refused too: those extra offsets are either records another reference
/// still names — served twice once this lands — or records nothing else
/// names, which the inputs were not authorised to produce. Compaction rewrites
/// a range; it does not extend one.
///
/// # Errors
///
/// [`Error::IndexObjectMismatch`] if either side is not contiguous, if the
/// covered ranges differ, or if the inputs cover a range and the outputs cover
/// nothing. [`Error::OffsetOverflow`] if a reference's own fold was wrong.
pub fn covers(retiring: &[&ObjectRef], installing: &[&ObjectRef]) -> Result<()> {
    let retired = contiguous_span(retiring)?;
    let installed = contiguous_span(installing)?;
    // ⚠️ **Retiring nothing is not a swap and is refused**, along with every
    // other mismatch. A commit that installs objects while retiring none adds
    // records to a partition through the compaction path, which is the write
    // path's job and is checked nowhere here; a commit that retires and
    // installs nothing is a commit with no subject. Both are refused rather
    // than admitted as no-ops, and the one arm that succeeds is the one where
    // both sides cover the same range.
    if matches!((retired, installed), (Some(from), Some(to)) if from == to) {
        return Ok(());
    }
    Err(Error::IndexObjectMismatch)
}
