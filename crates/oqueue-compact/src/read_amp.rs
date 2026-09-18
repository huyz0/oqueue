//! Read amplification: the one input compaction is triggered on.
//!
//! ⚠️ **Computed from the index and nothing else** (`M5.md` task 1). The
//! trigger must cost no object-storage operation, because it is evaluated for
//! every candidate partition on every sweep — `ADR-0036` decision 1 is what
//! that buys, and it is why this module takes a
//! [`MaterializedIndex`] rather than anything that can reach a network.
//!
//! ⚠️ **What holds that is this function's signature, plus one leg of
//! `check-sans-io.sh`** (`M5.38`, widened by `M5.47`). It is *not* the
//! dependency set: `oqueue-core` exports the store seam and a fake beside it,
//! so a store is one `use` away from here and the merge executor legitimately
//! brings one into the crate. Nor is it that gate's four SDK patterns, which
//! look for vendor paths and would not see a call through the core seam. The
//! leg forbids that seam's name in **every** file under this crate's `src/`
//! except the executor's, which it names — ⚠️ **so this paragraph may not write it
//! either**, which is the cost of a rule that greps rather than parses, and
//! cheaper than a rule that reads a doc comment as an exemption. ⚠️ **It was
//! this file alone until `M5.47`**, and two commits after it was written
//! `plan.rs` and `sweep.rs` were each claiming the same property with nothing
//! holding either: a rule scoped to one file is a rule the next file does not
//! inherit.
//!
//! ⚠️ **Never triggered on object count**, which `M5.md` task 1 forbids
//! outright: object *count* is a coordinator cost and object *bytes* are a
//! cloud-bill cost, and a partition holding many objects that are never read
//! together has no amplification to fix. Compacting it spends PUTs to improve
//! a number nobody observes.

mod survey;
mod walk;

pub use survey::ReadAmp;

use walk::Walk;

use oqueue_core::{Error, MaterializedIndex, Offset, PartitionId, Result, TopicId};

/// How many records a compacted object is written to hold.
///
/// ⚠️ **UNDERIVED — synthesis, not measurement**, the same status `M5.md`'s
/// risks section gives the 8-16 amplification threshold: doc 14's one
/// published segment-merge datapoint is Redpanda's 500 MiB, and 512k records
/// is that figure at a record size of 1 KiB. ⚠️ **That record size is not this
/// project's model, and calling it one was this comment's own defect**
/// (`M5.7`): no requirement, no NFR and no research document states a modelled
/// record size, so 1 KiB is an assumption made here to turn a byte figure into
/// a record figure, and it is named as one. `M14` is the milestone that
/// replaces this with a number, and nothing may cite it as derived.
///
/// ⚠️ **In records rather than bytes, because the index does not hold bytes
/// for history.** A [`TailEntry`](oqueue_core::TailEntry) carries a byte range
/// inline and an [`ObjectRef`](oqueue_core::ObjectRef) deliberately does not —
/// resolving one is the footer read the tier split exists to defer. A
/// byte-denominated target would therefore need a GET per candidate, which is
/// exactly what this module may not do.
///
/// It is a constant rather than a knob, per `AGENTS.md` non-negotiable 2.
pub const COMPACTED_OBJECT_RECORDS: u32 = 524_288;

/// Computes read amplification for `[start, end)` from the index alone.
///
/// Counts the distinct objects a fetch over the range would touch, against the
/// number the same records would occupy at [`COMPACTED_OBJECT_RECORDS`].
///
/// ⚠️ **Pages, because [`find_batches`](MaterializedIndex::find_batches) does.**
/// A page is bounded by `MAX_BATCHES_PER_PAGE` so that a cold read cannot name
/// the whole of a partition's history; a range wider than one page is
/// therefore walked, resuming from the last object's end offset. An empty page
/// or a page that does not advance ends the walk, so a malformed index costs a
/// bounded loop rather than a hang.
///
/// ⚠️ **Distinct objects, not batches.** Resuming mid-object would otherwise
/// count one object twice and report amplification that compaction cannot
/// remove.
///
/// # Errors
///
/// Propagates [`find_batches`](MaterializedIndex::find_batches)' own error —
/// [`Error::OffsetOverflow`](oqueue_core::Error::OffsetOverflow), which means
/// the fold that produced the entry was already wrong.
pub fn read_amp<I>(
    index: &I,
    topic: &TopicId,
    partition: PartitionId,
    start: Offset,
    end: Offset,
) -> Result<ReadAmp>
where
    I: MaterializedIndex + ?Sized,
{
    // ⚠️ **From the log start, if a trim moved past `start`** (`M5.19`). A
    // sweep's default cursor is zero, and on a trimmed partition the index
    // refuses a read below its start — so without this every sweep over a
    // trimmed partition failed outright, and took every other candidate in
    // its round with it.
    //
    // ⚠️ **Retried on the refusal, not probed for up front.** A first draft
    // asked the index where the log starts before every walk, and the cost
    // tests caught it: one query more on every partition, trimmed or not,
    // where a range one page covers is supposed to cost one query and a cursor
    // at the end none. The refusal carries the start, so asking only when told
    // costs an untrimmed partition nothing.
    match survey(index, topic, partition, start, end) {
        Err(Error::BelowLogStart { log_start, .. }) if log_start > start.get() => {
            survey(index, topic, partition, Offset::new(log_start)?, end)
        }
        other => other,
    }
}

/// [`read_amp`] over a `start` the index can read from.
fn survey<I>(
    index: &I,
    topic: &TopicId,
    partition: PartitionId,
    start: Offset,
    end: Offset,
) -> Result<ReadAmp>
where
    I: MaterializedIndex + ?Sized,
{
    let mut walk = Walk::over(start, end);

    // ⚠️ **One lookup below the start, and it is what makes the alignment
    // sound** (`M5.46`). `find_batches` answers from the batch holding the
    // cursor, so an object whose earlier span for this partition ends exactly
    // at `start` is never returned by the walk below — and that object then
    // measures as though it lay wholly inside a range it straddles. This probe
    // names it. It costs one index lookup per candidate whose cursor is not
    // zero, and no object-storage operation, which is the property `ADR-0036`
    // rests on.
    // ⚠️ **`start < end` guards it**, so a cursor at or past the partition's
    // end costs the probe nothing: a sweep over a cold catalog stays one
    // `end_offset` lookup per partition and no index walk, which is what
    // `sweep`'s own tests count.
    // ⚠️ **No `start > ZERO` test, and one does not survive**: `Offset::new`
    // refuses a negative offset, so a range starting at zero skips the probe
    // through the `let Ok` below and a test for it is a branch no input can
    // distinguish.
    if start < end
        && let Ok(before) = Offset::new(start.get() - 1)
    {
        // ⚠️ **At the log start the probe below it is refused, and it is
        // not needed**: nothing wholly below the start survives a trim, so
        // the only objects left to bar are those straddling it — which a
        // read *from* the start returns, and the base test below bars just
        // the same.
        let probe = match index.find_batches(topic, partition, before, u64::MAX) {
            Err(Error::BelowLogStart { .. }) => {
                index.find_batches(topic, partition, start, u64::MAX)?
            }
            other => other?,
        };
        for batch in probe {
            let reference = batch.reference();
            if reference.base_offset() < start {
                walk.bar(reference.object());
            }
        }
    }

    let mut cursor = start;

    while cursor < end {
        let page = index.find_batches(topic, partition, cursor, u64::MAX)?;
        if page.is_empty() {
            break;
        }
        let mut furthest = cursor;
        for batch in &page {
            match walk.visit(batch)? {
                None => break,
                Some(object_end) => furthest = furthest.max(object_end),
            }
        }
        if furthest <= cursor {
            break;
        }
        cursor = furthest;
    }

    Ok(walk.finish())
}

/// How many compacted objects `records` would occupy.
///
/// ⚠️ Saturating on a 32-bit target rather than wrapping: a count that cannot
/// be represented is not a small one. It cannot exceed the entries the index
/// holds, so reaching the saturation means the fold that produced them was
/// already wrong.
fn objects_needed(records: i64) -> usize {
    let target = i64::from(COMPACTED_OBJECT_RECORDS);
    let needed = records
        .div_euclid(target)
        .saturating_add(i64::from(records.rem_euclid(target) != 0));
    usize::try_from(needed).unwrap_or(usize::MAX)
}

#[cfg(test)]
mod tests;
