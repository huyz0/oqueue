//! The merge executor: reading a plan's inputs and writing one output.
//!
//! ⚠️ **One GET per input, whatever its size** (`M5.md` task 4). The whole
//! object is read once and the footer is parsed out of the bytes already in
//! hand — a separate tail read to find the footer would double the request
//! count on the one path whose cost model is request-bound, and the footer is
//! at the tail of what was just read.
//!
//! ⚠️ **Sequential on both sides, so peak memory is one input plus one part.**
//! One input's bytes are resident at a time, and the output goes out through a
//! [`BundleStream`](oqueue_core::BundleStream), which holds at most one
//! `BUNDLE_PART_BYTES` part plus the region metadata for the footer (`M5.6`).
//! `M5.3`'s records budget bounds the *work* rather than the memory.
//!
//! ⚠️ **No random seeks**, which falls out of reading each input once in order
//! rather than being arranged for.
//!
//! ⚠️ **This is the one operation that can silently lose acknowledged data**
//! (NFR-20), so everything here that could be a caller's responsibility is
//! this function's instead: the inputs are sorted by base offset rather than
//! trusted to arrive in order, an input that does not tile the plan's range
//! exactly is refused rather than copied, an object whose regions do not
//! account for what the index said it holds is refused, and ⚠️ **a unique
//! output key per attempt is what keeps a retry off an object the index
//! already names** — the seal itself is unconditional (`ADR-0037`), which the
//! `# Errors` section below says in the same words.

// ⚠️ `redundant_pub_crate` and `unreachable_pub` disagree about a `pub(crate)`
// item in a private module, and `gather` is exactly that: `layout.rs` needs it
// and nothing outside this crate may. `pub(crate)` is the visibility that is
// true, so the lint that disagrees is the one allowed — `oqueue-core`'s
// `bundle.rs` makes the same call for the same reason.
#![allow(clippy::redundant_pub_crate)]
mod outcome;

pub use outcome::MergeOutcome;

use oqueue_core::{
    BundleStream, ByteRange, Error, KeyDomain, ObjectRef, ObjectStore, PushedRecords, Result,
    parse_footer,
};

use crate::{CompactionNamer, CompactionPlan};
use oqueue_core::contiguous_span;

/// Merges the plan's inputs into an object this mints the name of.
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
/// errors. ⚠️ **Not `PreconditionFailed`**: the seal is unconditional
/// (`ADR-0037`), and a unique output key is what keeps a retry off an object
/// the index already names — plus [`Error::BundleSequenceExhausted`] if the
/// namer has no key left.
///
/// ⚠️ **The key is minted here rather than received** (`M5.75`). A caller that
/// passes a key can pass the same one twice, which is what the unconditional
/// seal has no defence against: the second attempt overwrites the first's
/// bytes, and after the first attempt's swap committed those are bytes an
/// index entry names.
pub async fn merge<S>(
    store: &S,
    plan: &CompactionPlan,
    inputs: &[ObjectRef],
    namer: &mut CompactionNamer,
) -> Result<MergeOutcome>
where
    S: ObjectStore + ?Sized,
{
    let output = namer.next_key()?;
    let mut stream = BundleStream::open(store, &output).await?;
    let (gets, records) = gather(store, plan, inputs, &mut stream).await?;
    let (spans, written) = stream.finish().await?;

    Ok(MergeOutcome {
        gets,
        puts: 1,
        records,
        spans,
        written,
        object: Some(output),
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
pub(crate) async fn gather<'store, S>(
    store: &'store S,
    plan: &CompactionPlan,
    inputs: &[ObjectRef],
    stream: &mut BundleStream<'store>,
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
    // M8.6 will pass the plan's topic key domain here. Until BYOK routing
    // exists, compaction can only safely consume the default path; a sealed
    // region is refused rather than copied as if it were plaintext.
    let domain = KeyDomain::default_domain();
    for reference in covering {
        let bytes = store.get(reference.object(), ByteRange::Full).await?;
        gets += 1;
        records += i64::from(take_regions(&bytes, plan, reference, stream, &domain).await?);
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
/// the range's ends. ⚠️ **A straddle is refused by the span comparison after
/// the loop, both edges of it**, and this paragraph said "through the
/// contiguity and reach tests" until `M5.12`'s third round — which was true
/// before that task shared the contiguity rule and false after, in the same
/// diff that corrected the body comment thirty lines below and not this one. ⚠️ **Refused, not trimmed**: trimming inside an object
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
        // showed both halves redundant: **both edges are refused by the single
        // comparison after this loop**, which requires the covering to span
        // exactly `[plan.start(), plan.end())`. A branch no input can
        // distinguish is worse than no branch.
        //
        // ⚠️ **Both halves of that comparison are load-bearing, and this
        // comment said otherwise until `M5.12`'s second round.** It claimed an
        // input reaching below the start was caught by contiguity — it is not:
        // contiguity now lives in `contiguous_span`, which never sees
        // `plan.start()`, so a single ref based at 0 against a plan starting
        // at 2 spans `(0, 5)` with no gap and meets the end. Only the low
        // endpoint refuses it. Trimming this to `covered.1 != plan.end()`
        // rewrites records outside the plan.
        covering.push(reference);
    }
    // ⚠️ **The contiguity rule is `coverage::contiguous_span`'s**, not a
    // second copy of it (`M5.12`). This function's own work is the *filter* —
    // which inputs the plan's range touches at all — and the range it must
    // then meet exactly. Two notions of what a gap is, one for the inputs and
    // one for the outputs, is the drift that check has to be free of.
    let span = contiguous_span(&covering)?;
    let covered = span.unwrap_or_else(|| (plan.start(), plan.start()));
    if covered != (plan.start(), plan.end()) {
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
async fn take_regions(
    bytes: &[u8],
    plan: &CompactionPlan,
    reference: &ObjectRef,
    stream: &mut BundleStream<'_>,
    domain: &KeyDomain,
) -> Result<u32> {
    let mut taken = 0_u32;
    for region in parse_footer(bytes, bytes.len() as u64)? {
        if region.topic() != plan.topic() || region.partition() != plan.partition() {
            continue;
        }
        domain.validate_region(&region)?;
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
        stream
            .push(
                plan.topic().clone(),
                plan.partition(),
                PushedRecords {
                    // ⚠️ **`None`, and it costs FR-14 nothing** (`ADR-0038`).
                    // The footer carries no producer identity by design
                    // (`M11.5`), so there is none here to carry; identity
                    // survives a rewrite in the **metadata log**, which is
                    // append-only and which `M5.13`'s swap adds to rather than
                    // rewrites. ⚠️ A merged span holds records from several
                    // input spans with several identities, and a
                    // `CommittedSpan` carries one — so carrying it here is not
                    // merely expensive, it is not well defined.
                    count: region.record_count(),
                    producer: None,
                },
                slice,
            )
            .await?;
        taken = taken.saturating_add(region.record_count());
    }
    if taken != reference.record_count() {
        return Err(Error::IndexObjectMismatch);
    }
    Ok(taken)
}
