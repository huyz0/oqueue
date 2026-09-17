//! The candidate sweep: what one compaction round is made of.
//!
//! ⚠️ **The cadence is only the rate at which candidates are looked for**
//! (`ADR-0036` decision 1). A sweep reads the coordinator's in-memory index
//! and costs no object-storage operation, so doc 14 §7's table — $720/day at a
//! 60 s cadence over 100k partitions — prices every partition compacting every
//! round rather than the steady state. What scales with partition count here
//! is a memory scan.
//!
//! ⚠️ **The partitions come from the caller, not from the index**, and that is
//! a deferral rather than a design. [`MaterializedIndex`] has no way to
//! enumerate what it holds, and adding one is a change to an `oqueue-core`
//! trait — an ADR, every fake and every implementation in one commit
//! (non-negotiable 6), which is not this row's scope. The coordinator holds
//! the catalog and is the caller; `M5.15`, which claims a compaction job
//! against a real coordinator, is where the enumeration gets an owner.
//!
//! ⚠️ **And so does each candidate's starting offset, for a harder reason.**
//! A sweep decides *whether* a range is worth compacting; it cannot decide
//! *where a partition has got to*, because that is state and this function has
//! none. Two attempts to derive it failed in review and both failed the same
//! way — planning every candidate from zero left any partition bigger than one
//! plan's budget unplannable for ever; fixed windows from zero moved the wall
//! to four windows; and inferring the start from the first object smaller than
//! a compacted one anchors permanently on any early window that is merely
//! *not worth compacting*, which is a different thing from compacted. Each
//! version reported the stuck partition as neither planned nor held over.
//! `M5.44` is the row that gives the cursor an owner and a home; until then
//! the caller says where to look, and a caller that always says zero gets the
//! behaviour of a caller that always says zero.

use core::time::Duration;

use std::collections::HashSet;

use oqueue_core::{MaterializedIndex, Offset, PartitionId, Result, TopicId};

use crate::{COMPACTION_PLAN_RECORDS_BUDGET, CompactionPlan, Planning, plan};

/// How often candidates are looked for.
///
/// ⚠️ **Not chosen on doc 14 §7's cost table** (`ADR-0036`): a sweep that finds
/// no candidate costs no object-storage operation, so the table's $720/day at
/// 60 s prices every partition compacting every round. What this interval buys
/// is detection latency — a partition crossing the threshold just after a
/// sweep serves amplified for up to one interval — and 30 minutes is chosen
/// against the timescales the rest of this milestone works in.
///
/// It is a constant rather than a knob, per `AGENTS.md` non-negotiable 2.
pub const COMPACTION_SWEEP_INTERVAL: Duration = Duration::from_mins(30);

/// A partition to consider, and where compaction has reached in it.
///
/// ⚠️ **`from` is the caller's**, and `M5.44` is where it gets an owner that
/// remembers it across sweeps. A caller with nothing to remember passes
/// [`Offset::ZERO`] and gets a sweep that re-examines the whole partition
/// every round, which is correct and wasteful rather than wrong.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct Candidate {
    topic: TopicId,
    partition: PartitionId,
    from: Offset,
}

impl Candidate {
    /// Names a partition and where to start looking in it.
    #[must_use]
    pub const fn new(topic: TopicId, partition: PartitionId, from: Offset) -> Self {
        Self {
            topic,
            partition,
            from,
        }
    }

    /// The topic.
    #[must_use]
    pub const fn topic(&self) -> &TopicId {
        &self.topic
    }

    /// The partition.
    #[must_use]
    pub const fn partition(&self) -> PartitionId {
        self.partition
    }

    /// Where compaction has reached.
    #[must_use]
    pub const fn from(&self) -> Offset {
        self.from
    }
}

/// One round's worth of work, and what did not fit in it.
///
/// ⚠️ **Held over carries the partition it is about**, not a count: an
/// operator reading "three held over" cannot tell a backlog from a stall, and
/// the partitions are what say which.
///
/// ⚠️ **There is no deferred list.** A window is capped at
/// [`COMPACTION_PLAN_RECORDS_BUDGET`] records and offsets are dense, so a
/// window's records never exceed the budget and `plan`'s own
/// [`Deferred`](crate::Planning::Deferred) is reachable only through
/// `Offset::add` overflowing near `i64::MAX`; that lands in `held_over` with
/// everything else that did not fit. The variant stays reachable for a caller
/// that picks its own range, which is what it is for.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Sweep {
    round: Vec<CompactionPlan>,
    held_over: Vec<(TopicId, PartitionId)>,
}

impl Sweep {
    /// The plans this round will run.
    #[must_use]
    pub fn round(&self) -> &[CompactionPlan] {
        &self.round
    }

    /// Candidates worth compacting that did not fit this round.
    #[must_use]
    pub fn held_over(&self) -> &[(TopicId, PartitionId)] {
        &self.held_over
    }
}

/// Surveys `candidates` and builds one round.
///
/// Each candidate is planned over one window of at most
/// [`COMPACTION_PLAN_RECORDS_BUDGET`] records, starting at its own
/// [`from`](Candidate::from).
///
/// ⚠️ **Duplicates are dropped**, by `(topic, partition)` and not by the whole
/// candidate: two windows of one partition in one round would overlap or abut,
/// and `merge_round` refuses an overlapping round outright — one caller's slip
/// discarding every other partition's work.
///
/// # Errors
///
/// Propagates the index's own error.
pub fn sweep<I>(index: &I, candidates: &[Candidate]) -> Result<Sweep>
where
    I: MaterializedIndex + ?Sized,
{
    let mut swept = Sweep::default();
    let mut budget = COMPACTION_PLAN_RECORDS_BUDGET;
    let mut seen: HashSet<(TopicId, PartitionId)> = HashSet::new();

    for candidate in candidates {
        let topic = candidate.topic();
        let partition = candidate.partition();
        if !seen.insert((topic.clone(), partition)) {
            continue;
        }
        // ⚠️ **A partition the index has not folded costs one lookup and no
        // walk**, which is what makes a sweep over a cold catalog cheap: the
        // window below is empty, and `plan`'s own walk returns before asking
        // for a batch.
        let end = index.end_offset(topic, partition);
        if candidate.from() >= end {
            continue;
        }
        // Offsets are dense and gap-free (FR-11), so a window of the budget's
        // width holds exactly the budget's records.
        let window_end = candidate
            .from()
            .add(COMPACTION_PLAN_RECORDS_BUDGET)
            .map_or(end, |limit| limit.min(end));

        match plan(index, topic, partition, candidate.from(), window_end)? {
            Planning::NotWorthIt => {}
            // ⚠️ **Held over rather than refused.** A window is capped at the
            // budget and offsets are dense, so the only way here is
            // `Offset::add` overflowing near `i64::MAX` and the window falling
            // back to the whole range — and one candidate's arithmetic edge
            // must not discard every other partition's work.
            Planning::Deferred(_) => swept.held_over.push((topic.clone(), partition)),
            Planning::Planned(planned) => {
                let cost = planned.cost().records_rewritten();
                if cost > budget {
                    // ⚠️ **Held over, not shrunk.** One object is what a round
                    // writes, so a plan that does not fit is next round's —
                    // and trimming it here would plan a range nobody measured.
                    swept.held_over.push((topic.clone(), partition));
                    continue;
                }
                budget -= cost;
                swept.round.push(planned);
            }
        }
    }

    Ok(swept)
}
