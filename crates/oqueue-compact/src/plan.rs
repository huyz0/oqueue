//! Compaction planning: which `(partition, range)` is worth rewriting.

use crate::{ReadAmp, read_amp};
use oqueue_core::{MaterializedIndex, Offset, PartitionId, Result, TopicId};

/// The amplification a range must exceed before it is worth rewriting.
///
/// ⚠️ **Synthesis, not measurement**, and `M5.md`'s risks section says so: the
/// 8-16 band it sits inside is a break-even model rather than a number anyone
/// measured, and `M14` replaces it. 12 is the middle of that band.
///
/// ⚠️ **Strictly exceeded, not met.** A range at exactly the threshold is not
/// planned, because the compacted layout itself measures 1.0 and every value
/// in between is a judgement this constant exists to make once.
///
/// It is a constant rather than a knob, per `AGENTS.md` non-negotiable 2.
pub const COMPACTION_READ_AMP_THRESHOLD: u32 = 12;

/// One range selected for rewriting, and the measurement that selected it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CompactionPlan {
    topic: TopicId,
    partition: PartitionId,
    start: Offset,
    end: Offset,
    amplification: ReadAmp,
}

impl CompactionPlan {
    /// The topic whose range this rewrites.
    #[must_use]
    pub const fn topic(&self) -> &TopicId {
        &self.topic
    }

    /// The partition whose range this rewrites.
    #[must_use]
    pub const fn partition(&self) -> PartitionId {
        self.partition
    }

    /// The first offset covered.
    #[must_use]
    pub const fn start(&self) -> Offset {
        self.start
    }

    /// The offset just past the last one covered.
    #[must_use]
    pub const fn end(&self) -> Offset {
        self.end
    }

    /// What selected it.
    #[must_use]
    pub const fn amplification(&self) -> ReadAmp {
        self.amplification
    }
}

/// Plans a rewrite of `[start, end)`, or declines to.
///
/// Two conditions, **both** required (`M5.md` task 2):
///
/// 1. Read amplification strictly exceeds [`COMPACTION_READ_AMP_THRESHOLD`].
/// 2. No object in the *planned* range is still in the index's tail tier —
///    and a range that straddles the boundary is **trimmed** to its history
///    portion rather than declined, because a live partition's range always
///    straddles it.
///
/// ⚠️ **The second is the age guard, and it is what makes the cases `M5.md`'s
/// risks section names cost nothing** — pure tail consumption served from
/// cache is never rewritten, and neither is a range already contiguous, which
/// fails the first condition instead. ⚠️ **It is a tier test rather than a
/// clock read**, because this index holds no timestamp and an `ObjectRef` that
/// carried one would cost 16 bytes against the ~40-byte budget doc 14 §3's
/// arithmetic rests on. The known gap: a partition that stops being written
/// keeps its last `TAIL_WINDOW_ENTRIES` objects in the tail for ever, so an
/// idle partition's tail is never compacted until `M5.16` gives the index
/// `ts_min`/`ts_max` and the guard becomes time-based.
///
/// ⚠️ **Planning costs no object-storage operation**, because everything it
/// reads is [`read_amp`]'s, which reads the index alone (`ADR-0036`).
///
/// # Errors
///
/// Propagates the index's own error.
pub fn plan<I>(
    index: &I,
    topic: &TopicId,
    partition: PartitionId,
    start: Offset,
    end: Offset,
) -> Result<Option<CompactionPlan>>
where
    I: MaterializedIndex + ?Sized,
{
    let surveyed = read_amp(index, topic, partition, start, end)?;

    // ⚠️ **Trimmed, not declined.** A live partition's range runs from cold
    // history into the hot window, so declining anything that touches the tail
    // would leave the most amplified partition in the system permanently
    // uncompacted -- the case FR-34 exists for. What the guard forbids is
    // *rewriting* tail data, and dropping the tail portion of the range obeys
    // that while still planning the history portion.
    // ⚠️ **No separate "starts inside the tail" arm, and none is needed**: a
    // boundary at or before `start` trims the range to an empty one, whose
    // amplification is 0.0 and which therefore fails the threshold below. An
    // explicit guard for it was a branch no input could distinguish, which
    // cargo-mutants said by surviving its removal.
    let (end, amplification) = match surveyed.first_tail_base() {
        None => (end, surveyed),
        Some(boundary) => (
            boundary,
            read_amp(index, topic, partition, start, boundary)?,
        ),
    };

    // The trim above is what makes the guard hold, so there is no second test
    // for it here: after trimming, a planned range has no tail object in it by
    // construction, and `a_range_straddling_the_tail_boundary_is_trimmed...`
    // asserts exactly that on the result.
    if amplification.ratio() <= f64::from(COMPACTION_READ_AMP_THRESHOLD) {
        return Ok(None);
    }
    Ok(Some(CompactionPlan {
        topic: topic.clone(),
        partition,
        start,
        end,
        amplification,
    }))
}

#[cfg(test)]
mod tests {
    // A panic in a test harness is the test failing, which is what it is for.
    #![allow(clippy::expect_used)]

    use super::{COMPACTION_READ_AMP_THRESHOLD, plan};
    use crate::COMPACTED_OBJECT_RECORDS;
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

    /// Folds one object per count, in order.
    fn index_of(counts: &[u32]) -> FakeMaterializedIndex {
        let index = FakeMaterializedIndex::new();
        let entries: Vec<MetadataEntry> = counts
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

    /// `counts`, then enough further objects to push every one of them out of
    /// the tail window and into the history tier.
    fn aged_index(counts: &[u32]) -> FakeMaterializedIndex {
        let mut all = counts.to_vec();
        all.extend(std::iter::repeat_n(1_u32, TAIL_WINDOW_ENTRIES));
        index_of(&all)
    }

    /// The four combinations the task asks for: over/under the threshold,
    /// crossed with aged out of the tail or still in it. ⚠️ **Exactly one
    /// plans**, and a table rather than four tests so that the crossing is the
    /// assertion rather than a fact spread over four names.
    #[test]
    fn only_an_amplified_range_that_has_left_the_tail_is_planned() {
        // (objects in range, aged out of the tail, plans)
        let cases = [
            (40_i64, true, true),
            (2_i64, true, false),
            (40_i64, false, false),
            (2_i64, false, false),
        ];
        for (objects, aged, expected) in cases {
            let counts = vec![1_u32; usize::try_from(objects).expect("a small count")];
            let index = if aged {
                aged_index(&counts)
            } else {
                index_of(&counts)
            };
            let planned = plan(&index, &topic(), partition(), offset(0), offset(objects))
                .expect("an index that answers");
            assert_eq!(
                planned.is_some(),
                expected,
                "objects={objects} aged={aged}: amplified *and* out of the tail is the one \
                 case that plans"
            );
        }
    }

    #[test]
    fn a_range_already_contiguous_is_never_planned_however_old() {
        let index = aged_index(&[COMPACTED_OBJECT_RECORDS]);
        assert!(
            plan(
                &index,
                &topic(),
                partition(),
                offset(0),
                offset(i64::from(COMPACTED_OBJECT_RECORDS))
            )
            .expect("an index that answers")
            .is_none(),
            "one object at the compacted size is what compaction produces"
        );
    }

    #[test]
    fn a_plan_carries_the_measurement_that_selected_it() {
        let objects = 40_usize;
        let index = aged_index(&vec![1_u32; objects]);
        let planned = plan(
            &index,
            &topic(),
            partition(),
            offset(0),
            offset(i64::try_from(objects).expect("a small count")),
        )
        .expect("an index that answers")
        .expect("a plan");
        assert_eq!(planned.topic(), &topic());
        assert_eq!(planned.partition(), partition());
        assert_eq!(planned.start(), offset(0));
        assert_eq!(
            planned.end(),
            offset(i64::try_from(objects).expect("a small count"))
        );
        assert_eq!(planned.amplification().objects_touched(), objects);
    }

    /// ⚠️ **The case FR-34 exists for**: a live partition whose range runs
    /// from amplified history into the hot window. Declining it would leave
    /// the worst partition in the system permanently uncompacted.
    #[test]
    fn a_range_straddling_the_tail_boundary_is_trimmed_rather_than_declined() {
        let history = 40_i64;
        let index = aged_index(&vec![
            1_u32;
            usize::try_from(history).expect("a small count")
        ]);
        let whole = history + i64::try_from(TAIL_WINDOW_ENTRIES).expect("a small window");

        let planned = plan(&index, &topic(), partition(), offset(0), offset(whole))
            .expect("an index that answers")
            .expect("a plan over the history portion");
        assert_eq!(
            planned.end(),
            offset(history),
            "the plan stops where the tail begins"
        );
        assert_eq!(
            planned.amplification().tail_objects(),
            0,
            "nothing planned is still in the tail"
        );
    }

    /// A range wholly inside the tail has no history portion to trim to.
    #[test]
    fn a_range_starting_inside_the_tail_is_declined() {
        let history = 40_i64;
        let index = aged_index(&vec![
            1_u32;
            usize::try_from(history).expect("a small count")
        ]);
        let whole = history + i64::try_from(TAIL_WINDOW_ENTRIES).expect("a small window");
        assert!(
            plan(
                &index,
                &topic(),
                partition(),
                offset(history),
                offset(whole)
            )
            .expect("an index that answers")
            .is_none(),
            "tail data still served from cache is never rewritten"
        );
    }

    /// ⚠️ **The threshold is strictly exceeded, and this is the only test that
    /// says so.** Without it any comparison and any value between the other
    /// tests' 2 and 40 passes the suite.
    #[test]
    fn a_range_at_exactly_the_threshold_is_not_planned() {
        let at = i64::from(COMPACTION_READ_AMP_THRESHOLD);
        let index = aged_index(&vec![1_u32; usize::try_from(at).expect("a small count")]);
        let planned = plan(&index, &topic(), partition(), offset(0), offset(at))
            .expect("an index that answers");
        assert!(
            planned.is_none(),
            "amplification of exactly {at} is the threshold, not past it"
        );

        let over = at + 1;
        let index = aged_index(&vec![1_u32; usize::try_from(over).expect("a small count")]);
        assert!(
            plan(&index, &topic(), partition(), offset(0), offset(over))
                .expect("an index that answers")
                .is_some(),
            "one object more is past it"
        );
    }
}
