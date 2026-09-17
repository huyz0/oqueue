//! What a plan will cost to run, and the budget that refuses one.

use crate::ReadAmp;

/// The most records one plan may rewrite.
///
/// ⚠️ **A plan over this is deferred, never truncated.** A merge that runs
/// half a plan is the one compaction operation that can silently lose data —
/// the range-coverage check `M5.12` adds is the other half of that argument —
/// so the answer to "too big" is a smaller range next round, not a partial
/// rewrite of this one.
///
/// ⚠️ **In records rather than bytes**, for the reason
/// [`COMPACTED_OBJECT_RECORDS`](crate::COMPACTED_OBJECT_RECORDS) is: the
/// history tier carries no byte range, so a byte-denominated budget would
/// need a GET per candidate and the estimate would cost what it is trying to
/// bound.
///
/// ⚠️ **It is the round's ceiling as well as one plan's** (`M5.5`), so a raise
/// raises the in-memory accumulation a round builds before its first PUT, not
/// only the size of one plan. Whoever raises it owns both.
///
/// ⚠️ **One compacted object's worth, because the merge writes one object**
/// (`M5.4`). It was eight when `M5.3` set it, and that let a plan be costed at
/// two outputs and run as one — the estimate and the run disagreeing about the
/// thing the estimate exists to predict. `M5.6`'s multipart writer is what
/// earns the raise, and non-negotiable 2 names raising as the weakening
/// direction here, so the raise arrives with the writer that justifies it and
/// not before. UNDERIVED either way, `M14`'s to replace.
///
/// It is a constant rather than a knob, per `AGENTS.md` non-negotiable 2.
pub const COMPACTION_PLAN_RECORDS_BUDGET: i64 = 524_288;

/// What running a plan will cost, estimated before it runs.
///
/// ⚠️ **Estimated from the index, so the estimate costs nothing** — it is
/// arithmetic over the measurement that selected the range, and reaches no
/// store. ⚠️ **`puts` is one per output object and not one per multipart
/// part**: `M5.6`'s writer decides the part size, so the number this reports
/// is a floor until that exists, and `M5.4` is where the estimate is checked
/// against an executor's actual counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CostEstimate {
    gets: usize,
    puts: usize,
    records_rewritten: i64,
}

impl CostEstimate {
    /// Object reads: one per input object.
    #[must_use]
    pub const fn gets(&self) -> usize {
        self.gets
    }

    /// Object writes: one per output object **this plan alone** would need.
    ///
    /// ⚠️ **Not a round's PUT count.** `merge_round` writes one object for a
    /// whole round, so a caller summing estimates over an `n`-plan round
    /// predicts at least `n` PUTs against an actual one — an overstatement of
    /// `n - 1` at best, and the direction that defers a round it could
    /// afford.
    #[must_use]
    pub const fn puts(&self) -> usize {
        self.puts
    }

    /// Records the merge will move.
    #[must_use]
    pub const fn records_rewritten(&self) -> i64 {
        self.records_rewritten
    }

    /// Whether this fits [`COMPACTION_PLAN_RECORDS_BUDGET`].
    #[must_use]
    pub const fn within_budget(&self) -> bool {
        self.records_rewritten <= COMPACTION_PLAN_RECORDS_BUDGET
    }

    /// Estimates from the measurement that selected the range.
    #[must_use]
    pub const fn of(amplification: &ReadAmp) -> Self {
        Self {
            gets: amplification.objects_touched(),
            puts: amplification.objects_needed(),
            records_rewritten: amplification.records(),
        }
    }
}

#[cfg(test)]
mod tests {
    // A panic in a test harness is the test failing, which is what it is for.
    #![allow(clippy::expect_used)]

    use super::{COMPACTION_PLAN_RECORDS_BUDGET, CostEstimate};
    use crate::{COMPACTED_OBJECT_RECORDS, Planning, plan, read_amp};
    use oqueue_core::{
        ByteRange, CommitVersion, CommittedSpan, FakeMaterializedIndex, MaterializedIndex,
        MetadataEntry, MetadataRecord, ObjectKey, Offset, PartitionId, TAIL_WINDOW_ENTRIES,
        TopicId,
    };

    fn topic() -> TopicId {
        TopicId::new("t").expect("a valid topic")
    }

    fn partition() -> PartitionId {
        PartitionId::new(0).expect("a valid partition")
    }

    fn offset(value: i64) -> Offset {
        Offset::new(value).expect("a valid offset")
    }

    /// `counts`, aged out of the tail window by filler behind them.
    fn aged_index(counts: &[u32]) -> FakeMaterializedIndex {
        let index = FakeMaterializedIndex::new();
        let mut all = counts.to_vec();
        all.extend(std::iter::repeat_n(1_u32, TAIL_WINDOW_ENTRIES));
        let entries: Vec<MetadataEntry> = all
            .iter()
            .enumerate()
            .map(|(i, count)| {
                MetadataEntry::new(
                    CommitVersion::new(i as u64 + 1),
                    MetadataRecord::BatchCommitted {
                        object: ObjectKey::new(format!("obj-{i}")).expect("a valid key"),
                        spans: vec![CommittedSpan::new(
                            topic(),
                            partition(),
                            *count,
                            ByteRange::bounded(0, u64::from(*count)).expect("a valid range"),
                            None,
                        )],
                    },
                )
            })
            .collect();
        index.apply(&entries).expect("a valid fold");
        index
    }

    #[test]
    fn an_estimate_is_one_get_per_input_and_one_put_per_output() {
        let inputs = 20_usize;
        let index = aged_index(&vec![1_u32; inputs]);
        let amp = read_amp(
            &index,
            &topic(),
            partition(),
            offset(0),
            offset(i64::try_from(inputs).expect("a small count")),
        )
        .expect("an index that answers");
        let cost = CostEstimate::of(&amp);
        assert_eq!(cost.gets(), inputs, "one read per input object");
        assert_eq!(cost.puts(), 1, "20 records fit one compacted object");

        // ⚠️ **And more than one when the records do not fit one**, which the
        // case above cannot say: a constant `1` satisfies it exactly.
        let big = aged_index(&[COMPACTED_OBJECT_RECORDS; 3]);
        let amp = read_amp(
            &big,
            &topic(),
            partition(),
            offset(0),
            offset(i64::from(COMPACTED_OBJECT_RECORDS) * 3),
        )
        .expect("an index that answers");
        assert_eq!(
            CostEstimate::of(&amp).puts(),
            3,
            "three compacted objects' worth of records is three outputs"
        );
        assert_eq!(
            cost.records_rewritten(),
            i64::try_from(inputs).expect("a small count")
        );
        assert!(cost.within_budget());
    }

    /// ⚠️ **Deferred, not truncated.** A merge that runs part of a plan is the
    /// one compaction operation that can silently lose data, so an over-budget
    /// plan comes back with its estimate and no range at all.
    #[test]
    fn a_plan_over_the_budget_is_deferred_with_its_estimate() {
        // ⚠️ Small enough objects that the range is amplified as well as
        // over budget, and the two pull against each other: past the budget
        // the records need two compacted objects, so `objects_needed` is 2 and
        // the ratio is `objects / 2` — which must still clear 12. A
        // thirty-fourth of the compacted target gives 35 objects and a ratio
        // of 17.5. ⚠️ The ratio is **not** `COMPACTED_OBJECT_RECORDS / per`,
        // which an earlier version of this comment said: that is the ratio
        // only while the ceiling in `objects_needed` does not bite.
        let per = COMPACTED_OBJECT_RECORDS / 34;
        let objects = usize::try_from(COMPACTION_PLAN_RECORDS_BUDGET / i64::from(per) + 1)
            .expect("a small count");
        let index = aged_index(&vec![per; objects]);
        let end = offset(i64::from(per) * i64::try_from(objects).expect("a small count"));

        let outcome =
            plan(&index, &topic(), partition(), offset(0), end).expect("an index that answers");
        let Planning::Deferred(estimate) = outcome else {
            panic!("over the budget, so deferred: {outcome:?}")
        };
        assert!(
            estimate.records_rewritten() > COMPACTION_PLAN_RECORDS_BUDGET,
            "the estimate that refused it travels with the refusal"
        );
        assert_eq!(estimate.gets(), objects);
    }

    #[test]
    fn a_plan_at_exactly_the_budget_still_runs() {
        let at = COMPACTION_PLAN_RECORDS_BUDGET;
        assert!(
            CostEstimate {
                gets: 1,
                puts: 1,
                records_rewritten: at,
            }
            .within_budget(),
            "the budget is a ceiling, not a bar"
        );
        assert!(
            !CostEstimate {
                gets: 1,
                puts: 1,
                records_rewritten: at + 1,
            }
            .within_budget()
        );

        // ⚠️ **And through `plan()`, not only the predicate.** Asserting the
        // predicate alone leaves `>=` in the gate indistinguishable from `>`:
        // a plan at exactly the budget would defer for ever with the suite
        // green, because the over-budget fixture sits well past the boundary.
        // Objects small enough to clear the amplification threshold, in a
        // count whose records land exactly on it.
        // A sixteenth of the compacted target divides the budget exactly and
        // puts the ratio at 16, clear of the threshold.
        let per = i64::from(COMPACTED_OBJECT_RECORDS / 16);
        let objects = usize::try_from(COMPACTION_PLAN_RECORDS_BUDGET / per).expect("a small count");
        let per = u32::try_from(per).expect("a small count");
        let index = aged_index(&vec![per; objects]);
        let end = offset(i64::from(per) * i64::try_from(objects).expect("a small count"));
        assert_eq!(
            end.get(),
            COMPACTION_PLAN_RECORDS_BUDGET,
            "the fixture must land on the boundary, not near it"
        );

        let outcome =
            plan(&index, &topic(), partition(), offset(0), end).expect("an index that answers");
        assert!(
            matches!(outcome, Planning::Planned(_)),
            "exactly at the budget is inside it: {outcome:?}"
        );
    }
}
