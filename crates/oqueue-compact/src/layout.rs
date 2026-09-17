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
    BundleBuilder, CommittedSpan, Error, ObjectKey, ObjectRef, ObjectStore, Precondition, Result,
};

use crate::merge::gather;
use crate::{COMPACTION_PLAN_RECORDS_BUDGET, CompactionPlan, MergeOutcome};

/// One plan and the index references that tile its range.
#[derive(Debug, Clone)]
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
    output: &ObjectKey,
) -> Result<MergeOutcome>
where
    S: ObjectStore + ?Sized,
{
    // ⚠️ **Topic, then partition, then offset.** A fetch of one partition's
    // compacted range must land on one run of regions: interleaving two
    // partitions turns one ranged GET into as many reads as there are runs,
    // which is the amplification this whole milestone is about.
    if round.is_empty() {
        // ⚠️ **Not an error.** A sweep over a quiet cluster finds no candidate
        // every thirty minutes, and a round that reports failure for having
        // nothing to do is a failure an operator learns to ignore.
        return Ok(MergeOutcome::new(0, 0, 0, Vec::new()));
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

    let mut builder = BundleBuilder::new();
    let mut gets = 0_usize;
    let mut records = 0_i64;
    for planned in ordered {
        let (read, moved) = gather(store, planned.plan(), planned.inputs(), &mut builder).await?;
        gets += read;
        records += moved;
    }

    let sealed = builder.seal()?;
    let spans: Vec<CommittedSpan> = sealed.spans().to_vec();
    store
        .put(output, sealed.into_payload(), Some(Precondition::IfAbsent))
        .await?;

    Ok(MergeOutcome::new(gets, 1, records, spans))
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
