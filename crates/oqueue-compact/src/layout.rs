//! Merging several plans into one object, laid out so a fetch stays one read.
//!
//! ⚠️ **One output object per round, not one per plan** (`M5.md` task 5). At
//! 100k partitions a compacted object per partition is a PUT per partition per
//! round and an index entry per partition per round — the object *count* cost
//! doc 14 §7 prices, paid to fix an amplification problem. Bundling a round's
//! plans into one object is FR-32's own shape applied to compaction.
//!
//! ⚠️ **Ordered by topic, then partition, then offset, and contiguous within
//! each.** That ordering is what makes the bundling safe to do: a fetch of one
//! partition's compacted range lands on one run of regions with no other
//! partition's bytes between them, so it stays one ranged GET (FR-13). Regions
//! interleaved by arrival would turn one read into as many as there are runs.

use oqueue_core::{
    BundleStream, CommittedSpan, Error, MaterializedIndex, ObjectKey, ObjectRef, ObjectStore,
    Offset, Result,
};

use std::collections::HashMap;

use crate::merge::gather;
use crate::{COMPACTION_PLAN_RECORDS_BUDGET, CompactionNamer, CompactionPlan, MergeOutcome};

/// Folds one span into the ref for its object, adding it if new.
///
/// ⚠️ **One input per object, not per batch.** One object may hold several
/// disjoint spans of one partition — `M3.8`'s ordinary case, and what
/// `IndexState`'s fold produces — and [`merge`](crate::merge) reads an object
/// once and takes *every* region it holds for the partition. Two refs naming
/// one object therefore tile the range perfectly and then fail
/// `take_regions`' record-count check, because the count it sums is both
/// spans' and the count each ref carries is one span's. The ref that spans
/// both is what one read of that object actually delivers.
///
/// ⚠️ **No admissibility test here, and one does not survive.** A plan's range
/// is a union of whole objects by construction — `read_amp`'s walk decides it
/// over objects rather than spans, which is `M5.46`'s second round — so no
/// object this walk returns has a span outside the range. A `whole` flag was
/// here and no index could distinguish it, and `merge`'s tiling refuses a ref
/// that reaches outside the plan in any case.
fn absorb(
    inputs: &mut Vec<ObjectRef>,
    seen: &mut HashMap<ObjectKey, usize>,
    reference: &ObjectRef,
) {
    if let Some(at) = seen.get(reference.object()).copied() {
        let held = &inputs[at];
        let records = held.record_count().saturating_add(reference.record_count());
        inputs[at] = ObjectRef::new(
            reference.object().clone(),
            held.base_offset().min(reference.base_offset()),
            records,
        );
    } else {
        seen.insert(reference.object().clone(), inputs.len());
        inputs.push(reference.clone());
    }
}

/// One plan and the index references that tile its range.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PlannedInputs {
    plan: CompactionPlan,
    inputs: Vec<ObjectRef>,
}

impl PlannedInputs {
    /// Pairs a plan with the references covering it.
    #[must_use]
    pub const fn new(plan: CompactionPlan, inputs: Vec<ObjectRef>) -> Self {
        Self { plan, inputs }
    }

    /// The plan.
    #[must_use]
    pub const fn plan(&self) -> &CompactionPlan {
        &self.plan
    }

    /// The references tiling it.
    #[must_use]
    pub fn inputs(&self) -> &[ObjectRef] {
        &self.inputs
    }

    /// Reads the references tiling `plan` out of the index that produced it.
    ///
    /// ⚠️ **Derived, not supplied** (`M5.46`). Until this existed the only way
    /// to get a plan's inputs was for a caller to hand them over, and nothing
    /// in the crate did — the trigger half of the write path and the merge
    /// half had never met. The refs come from the same
    /// [`find_batches`](MaterializedIndex::find_batches) walk the measurement
    /// came from, so what tiles the plan is what measured it.
    ///
    /// ⚠️ **No straddle test here, and one does not survive.** A plan's range
    /// is a union of whole objects by construction — that is what
    /// [`ReadAmp::covered`](crate::ReadAmp::covered) reports and what `plan`
    /// takes its bounds from — so no object this walk returns hangs over
    /// either edge, and a skip for one was a branch no index could
    /// distinguish: an object ending at or before the start is dropped by
    /// [`merge`](crate::merge)'s own tiling, which is where a list that does
    /// not tile is caught in any case.
    ///
    /// # Errors
    ///
    /// Propagates the index's own error.
    pub fn derive<I>(index: &I, plan: CompactionPlan) -> Result<Self>
    where
        I: MaterializedIndex + ?Sized,
    {
        let (start, end) = (plan.start(), plan.end());
        let mut inputs: Vec<ObjectRef> = Vec::new();
        let mut seen: HashMap<ObjectKey, usize> = HashMap::new();
        let mut cursor = start;
        while cursor < end {
            // ⚠️ A trim can land between the plan and the derivation, and the
            // index then refuses the cursor (`M5.19`). The refusal says where
            // the readable log begins, so the walk resumes there rather than
            // failing the round — asked only when refused, so an untrimmed
            // derivation costs exactly what it did.
            let page = match index.find_batches(plan.topic(), plan.partition(), cursor, u64::MAX) {
                Err(Error::BelowLogStart { log_start, .. }) if log_start > cursor.get() => {
                    cursor = Offset::new(log_start)?;
                    continue;
                }
                other => other?,
            };
            if page.is_empty() {
                break;
            }
            let mut furthest = cursor;
            for batch in &page {
                let reference = batch.reference();
                let base = reference.base_offset();
                let object_end = reference.end_offset()?;
                if base >= end {
                    break;
                }
                furthest = furthest.max(object_end);
                absorb(&mut inputs, &mut seen, reference);
            }
            if furthest <= cursor {
                break;
            }
            cursor = furthest;
        }
        Ok(Self::new(plan, inputs))
    }
}

/// Merges every plan into one object, laid out by topic, partition and offset.
///
/// An empty round writes nothing and reports a run of nothing.
///
/// # Errors
///
/// Whatever [`merge`](crate::merge) returns for any one plan, plus
/// [`Error::OverlappingCompactionPlans`] and
/// [`Error::CompactionRoundTooLarge`] — see [`admissible`].
pub async fn merge_round<S>(
    store: &S,
    round: &[PlannedInputs],
    namer: &mut CompactionNamer,
) -> Result<MergeOutcome>
where
    S: ObjectStore + ?Sized,
{
    if round.is_empty() {
        // ⚠️ **Not an error.** A sweep over a quiet cluster finds no candidate
        // every thirty minutes, and a round that reports failure for having
        // nothing to do is a failure an operator learns to ignore.
        // ⚠️ **And no key is minted**, which is why the namer is untouched
        // above this line: a sequence number spent on an object that was never
        // written is a gap in a sequence whose only job is to be unrepeatable,
        // and a reader of the store would have no way to tell it from a write
        // that was lost.
        return Ok(MergeOutcome::empty());
    }

    // ⚠️ **Topic, then partition, then offset.** A fetch of one partition's
    // compacted range must land on one run of regions: interleaving two
    // partitions turns one ranged GET into as many reads as there are runs,
    // which is the amplification this whole milestone is about.
    let mut ordered: Vec<&PlannedInputs> = round.iter().collect();
    ordered.sort_by(|left, right| {
        let plan = (left.plan(), right.plan());
        plan.0
            .topic()
            .cmp(plan.1.topic())
            .then(plan.0.partition().cmp(&plan.1.partition()))
            .then(plan.0.start().cmp(&plan.1.start()))
    });
    admissible(&ordered)?;

    let output = namer.next_key()?;
    let mut stream = BundleStream::open(store, &output).await?;
    let mut gets = 0_usize;
    let mut records = 0_i64;
    for planned in ordered {
        let (read, moved) = gather(store, planned.plan(), planned.inputs(), &mut stream).await?;
        gets += read;
        records += moved;
    }
    let (spans, written): (Vec<CommittedSpan>, _) = stream.finish().await?;

    Ok(MergeOutcome {
        gets,
        puts: 1,
        records,
        spans,
        written,
        object: Some(output),
    })
}

/// Refuses a round whose plans overlap, or whose total is over the budget.
///
/// ⚠️ **Both checks are about what one object may hold, and both are the
/// round's rather than a plan's.** `merge` refuses an input list that names one
/// object twice; nothing until here refuses a *round* that names one partition
/// range twice, and the result would be an object holding those records twice,
/// contiguously, with no error — after which the commit's fold assigns offsets
/// by span and every later offset in the partition shifts. And
/// [`COMPACTION_PLAN_RECORDS_BUDGET`](crate::COMPACTION_PLAN_RECORDS_BUDGET) is
/// per plan, so a hundred plans at budget would accumulate a hundred objects'
/// worth in one builder before the first PUT.
///
/// # Errors
///
/// [`Error::OverlappingCompactionPlans`] if two plans for one partition
/// overlap, and [`Error::CompactionRoundTooLarge`] if the round's records
/// exceed [`COMPACTION_PLAN_RECORDS_BUDGET`](crate::COMPACTION_PLAN_RECORDS_BUDGET).
fn admissible(ordered: &[&PlannedInputs]) -> Result<()> {
    let mut records = 0_i64;
    let mut previous: Option<&CompactionPlan> = None;
    for planned in ordered {
        let plan = planned.plan();
        if let Some(last) = previous
            && last.topic() == plan.topic()
            && last.partition() == plan.partition()
            && plan.start() < last.end()
        {
            return Err(Error::OverlappingCompactionPlans);
        }
        records = records.saturating_add(plan.cost().records_rewritten());
        previous = Some(plan);
    }
    if records > COMPACTION_PLAN_RECORDS_BUDGET {
        return Err(Error::CompactionRoundTooLarge {
            records,
            budget: COMPACTION_PLAN_RECORDS_BUDGET,
        });
    }
    Ok(())
}
