//! The merge executor: reading a plan's inputs and writing one output.
//!
//! ⚠️ **One GET per input, whatever its size** (`M5.md` task 4). The whole
//! object is read once and the footer is parsed out of the bytes already in
//! hand — a separate tail read to find the footer would double the request
//! count on the one path whose cost model is request-bound, and the footer is
//! at the tail of what was just read.
//!
//! ⚠️ **Sequential, so one input's bytes are resident at a time**, which is
//! the bound this executor offers on the *input* side. ⚠️ **The output side is
//! not bounded yet**: the merged payload accumulates in a
//! [`BundleBuilder`](oqueue_core::BundleBuilder) and is PUT whole, so peak
//! memory is one input plus the whole output. `M5.6`'s multipart writer makes
//! that side streaming; `M5.3`'s records budget is what bounds it until then.
//!
//! ⚠️ **No random seeks**, which falls out of reading each input once in order
//! rather than being arranged for.
//!
//! ⚠️ **This is the one operation that can silently lose acknowledged data**
//! (NFR-20), so everything here that could be a caller's responsibility is
//! this function's instead: the inputs are sorted by base offset rather than
//! trusted to arrive in order, an input straddling the plan's range is
//! refused rather than copied whole, an object whose regions do not account
//! for what the index said it holds is refused, and the output is written
//! `IfAbsent` so a retry cannot overwrite bytes an index entry already names.

// ⚠️ `redundant_pub_crate` and `unreachable_pub` disagree about a `pub(crate)`
// item in a private module, and `gather` is exactly that: `layout.rs` needs it
// and nothing outside this crate may. `pub(crate)` is the visibility that is
// true, so the lint that disagrees is the one allowed — `oqueue-core`'s
// `bundle.rs` makes the same call for the same reason.
#![allow(clippy::redundant_pub_crate)]
mod outcome;

pub use outcome::MergeOutcome;

use oqueue_core::{
    BundleBuilder, ByteRange, Error, ObjectKey, ObjectRef, ObjectStore, Precondition,
    PushedRecords, Result, parse_footer,
};

use crate::CompactionPlan;

/// Merges the plan's inputs into `output`.
///
/// `inputs` are the index's references for the partition — each carrying the
/// base offset that says where in the partition its records sit. They may
/// arrive in any order.
///
/// # Errors
///
/// [`Error::IndexObjectMismatch`] if an input straddles the plan's range, if
/// an input holds no region for the plan's partition, or if its regions do not
/// account for the record count the index recorded. Otherwise the store's own
/// errors, and [`Error::PreconditionFailed`] if `output` already exists.
pub async fn merge<S>(
    store: &S,
    plan: &CompactionPlan,
    inputs: &[ObjectRef],
    output: &ObjectKey,
) -> Result<MergeOutcome>
where
    S: ObjectStore + ?Sized,
{
    let mut builder = BundleBuilder::new();
    let (gets, records) = gather(store, plan, inputs, &mut builder).await?;
    let sealed = builder.seal()?;
    let spans = sealed.spans().to_vec();
    // ⚠️ **`IfAbsent`, so a retry cannot overwrite an output an index entry
    // already names.** A re-run writes a fresh key; `M5.13`'s commit is what
    // decides which key the index points at.
    store
        .put(output, sealed.into_payload(), Some(Precondition::IfAbsent))
        .await?;

    Ok(MergeOutcome {
        gets,
        puts: 1,
        records,
        spans,
    })
}

/// Reads one plan's inputs into `builder`, returning the reads it took and the
/// records it moved.
///
/// ⚠️ **Separated from the write so a round can share one builder**
/// (`M5.5`): several plans' regions go into one object, and the alternative —
/// one object per plan — is a PUT and an index entry per partition per round,
/// which is the object-count cost compaction exists to reduce.
///
/// # Errors
///
/// As [`merge`].
pub(crate) async fn gather<S>(
    store: &S,
    plan: &CompactionPlan,
    inputs: &[ObjectRef],
    builder: &mut BundleBuilder,
) -> Result<(usize, i64)>
where
    S: ObjectStore + ?Sized,
{
    // ⚠️ **Sorted here rather than required of the caller.** The output's
    // regions are read back in the order they were written, so an input list
    // in the wrong order produces an object that parses perfectly and serves
    // one offset's records for another's.
    let mut ordered: Vec<&ObjectRef> = inputs.iter().collect();
    ordered.sort_by_key(|reference| reference.base_offset());
    let covering = tiling(&ordered, plan)?;

    let mut gets = 0_usize;
    let mut records = 0_i64;
    for reference in covering {
        let bytes = store.get(reference.object(), ByteRange::Full).await?;
        gets += 1;
        records += i64::from(take_regions(&bytes, plan, reference, builder)?);
    }

    // ⚠️ **No final check against `plan.cost().records_rewritten()`, and none
    // is needed.** An exact tiling of `[start, end)` whose every object was
    // verified against its own indexed record count already sums to
    // `end - start`, which is what the plan was costed on — so the comparison
    // is one no input can fail, and a branch no test can distinguish is worse
    // than no branch. The tiling is the check.
    Ok((gets, records))
}

/// The inputs that tile the plan's range exactly, in ascending order.
///
/// ⚠️ **Tiling rather than overlapping, and this is the NFR-20 check.**
/// Sorting the inputs fixes their *order*; it says nothing about whether they
/// cover the range. A list missing the object at one offset produces a
/// perfectly valid output with those records absent, and a list naming one
/// object twice produces one with them present twice — both silent, both
/// acknowledged data. So the refs must start at the plan's start, end at its
/// end, and each one must begin exactly where the last ended.
///
/// Inputs wholly outside the range are dropped before the check, and cost no
/// read: a sweep that hands over a partition's whole ref list should not pay a
/// GET per object the plan does not cover.
///
/// # Errors
///
/// [`Error::IndexObjectMismatch`] if an input straddles either edge of the
/// range, if the refs leave a gap, if they overlap, or if they do not reach
/// the range's ends — the first of those through the contiguity and reach
/// tests rather than a test of its own, for the reason the body gives. ⚠️ **Refused, not trimmed**: trimming inside an object
/// means parsing records to find the offset boundary, and copying a straddler
/// whole would write records the plan does not cover — which, once the inputs
/// retire, leaves the overlap in two objects and serves it twice.
fn tiling<'a>(ordered: &[&'a ObjectRef], plan: &CompactionPlan) -> Result<Vec<&'a ObjectRef>> {
    let mut covering: Vec<&ObjectRef> = Vec::new();
    for reference in ordered {
        let base = reference.base_offset();
        let object_end = reference.end_offset()?;
        if object_end <= plan.start() || base >= plan.end() {
            continue;
        }
        // ⚠️ **No separate straddle test, and none survives one.** A dedicated
        // `base < start || object_end > end` refusal was here and cargo-mutants
        // showed both halves redundant: an input reaching below the start fails
        // the contiguity test below, because the first tile must begin exactly
        // at `plan.start()`, and one reaching past the end fails the reach test
        // after the loop. Both still refuse, with the same error and the same
        // tests observing it; a branch no input can distinguish is worse than
        // no branch.
        let expected = match covering.last() {
            None => plan.start(),
            Some(previous) => previous.end_offset()?,
        };
        if base != expected {
            return Err(Error::IndexObjectMismatch);
        }
        covering.push(reference);
    }
    let reached = match covering.last() {
        None => plan.start(),
        Some(last) => last.end_offset()?,
    };
    if reached != plan.end() {
        return Err(Error::IndexObjectMismatch);
    }
    Ok(covering)
}

/// Appends this object's regions for the plan's partition, returning how many
/// records they held.
///
/// # Errors
///
/// [`Error::IndexObjectMismatch`] if the regions do not account for the record
/// count the index recorded — ⚠️ **a short object with a healthy footer is how
/// acknowledged records go missing without anything failing**, and this is the
/// one place both numbers are in hand. [`Error::UnboundedRegion`] or
/// [`Error::MalformedBundleFooter`] for bytes no `BundleBuilder` wrote.
fn take_regions(
    bytes: &[u8],
    plan: &CompactionPlan,
    reference: &ObjectRef,
    builder: &mut BundleBuilder,
) -> Result<u32> {
    let mut taken = 0_u32;
    for region in parse_footer(bytes, bytes.len() as u64)? {
        if region.topic() != plan.topic() || region.partition() != plan.partition() {
            continue;
        }
        let ByteRange::Bounded(span) = region.bytes() else {
            // `BundleBuilder::push` only ever produces a bounded range, so a
            // `Full` one means this footer was written by something else.
            return Err(Error::UnboundedRegion);
        };
        let from = usize::try_from(span.offset()).unwrap_or(usize::MAX);
        let to = from.saturating_add(usize::try_from(span.length()).unwrap_or(usize::MAX));
        let slice = bytes
            .get(from..to)
            .ok_or(Error::MalformedBundleFooter { at: from })?;
        builder.push(
            plan.topic().clone(),
            plan.partition(),
            PushedRecords {
                count: region.record_count(),
                producer: None,
            },
            slice,
        )?;
        taken = taken.saturating_add(region.record_count());
    }
    if taken != reference.record_count() {
        return Err(Error::IndexObjectMismatch);
    }
    Ok(taken)
}
