//! Read amplification: the one input compaction is triggered on.
//!
//! ⚠️ **Computed from the index and nothing else** (`M5.md` task 1). The
//! trigger must cost no object-storage operation, because it is evaluated for
//! every candidate partition on every sweep — `ADR-0036` decision 1 is what
//! that buys, and it is why this module takes a
//! [`MaterializedIndex`] rather than anything that can reach a network.
//!
//! ⚠️ **What holds that is this function's signature, plus one leg of
//! `check-sans-io.sh` written for this file** (`M5.38`). It is *not* the
//! dependency set: `oqueue-core` exports the store seam and a fake beside it,
//! so a store is one `use` away from here and `M5.4`'s merge executor will
//! legitimately bring one into the crate. Nor is it that gate's four SDK
//! patterns, which look for vendor paths and would not see a call through the
//! core seam. The leg forbids that seam's name in this file outright — ⚠️ **so
//! this paragraph may not write it either**, which is the cost of a rule that
//! greps rather than parses, and cheaper than a rule that reads a doc comment
//! as an exemption. It fails if the file is unreadable or has moved.
//!
//! ⚠️ **Never triggered on object count**, which `M5.md` task 1 forbids
//! outright: object *count* is a coordinator cost and object *bytes* are a
//! cloud-bill cost, and a partition holding many objects that are never read
//! together has no amplification to fix. Compacting it spends PUTs to improve
//! a number nobody observes.

mod survey;

pub use survey::ReadAmp;

use oqueue_core::{MaterializedIndex, ObjectKey, Offset, PartitionId, Result, TopicId};
use std::collections::HashSet;

/// How many records a compacted object is written to hold.
///
/// ⚠️ **UNDERIVED — synthesis, not measurement**, the same status `M5.md`'s
/// risks section gives the 8-16 amplification threshold: doc 14 §7's one
/// published segment-merge datapoint is Redpanda's 500 MiB, which at this
/// project's modelled ~1 KiB record is ~512k records. `M14` is the milestone
/// that replaces this with a number, and nothing may cite it as derived.
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
    let mut seen: HashSet<ObjectKey> = HashSet::new();
    let mut tail: HashSet<ObjectKey> = HashSet::new();
    let mut first_tail_base: Option<Offset> = None;
    let mut records: i64 = 0;
    let mut cursor = start;

    while cursor < end {
        let page = index.find_batches(topic, partition, cursor, u64::MAX)?;
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
            // ⚠️ **Records accrue per span, objects per key.** One object may
            // hold two disjoint spans for one partition — `IndexState`'s fold
            // admits a partition appearing twice in one batch — so counting
            // records only on first sight of a key would under-count the
            // range and report amplification lower than the truth, which is
            // the direction that silently skips compaction.
            records += overlap(base, object_end, start, end);
            seen.insert(reference.object().clone());
            if batch.bytes().is_some() {
                // An inline byte range is what the tail tier is: the index
                // already knows where to read, so the fetch is one GET.
                tail.insert(reference.object().clone());
                // Ascending order, so the first one seen is the boundary --
                // `min` rather than a first-write-wins flag, because ordering
                // is `find_batches`' guarantee and not this walk's.
                first_tail_base = Some(first_tail_base.map_or(base, |b: Offset| b.min(base)));
            }
        }
        if furthest <= cursor {
            break;
        }
        cursor = furthest;
    }

    let needed = records
        .div_euclid(i64::from(COMPACTED_OBJECT_RECORDS))
        .saturating_add(i64::from(
            records.rem_euclid(i64::from(COMPACTED_OBJECT_RECORDS)) != 0,
        ));

    Ok(ReadAmp {
        objects_touched: seen.len(),
        // `needed` is a count of objects over one partition's range and cannot
        // exceed the entries the index holds, so a failure here means the fold
        // that produced them was already wrong. Saturating is the honest
        // reading on a 32-bit target: an unrepresentable count is not a small
        // one.
        objects_needed: usize::try_from(needed).unwrap_or(usize::MAX),
        records,
        tail_objects: tail.len(),
        first_tail_base,
    })
}

/// How many records of `[base, object_end)` fall inside `[start, end)`.
fn overlap(base: Offset, object_end: Offset, start: Offset, end: Offset) -> i64 {
    let lower = base.max(start).get();
    let upper = object_end.min(end).get();
    (upper - lower).max(0)
}

#[cfg(test)]
mod tests {
    // A panic in a test harness is the test failing, which is what it is for.
    #![allow(clippy::expect_used)]

    use super::{COMPACTED_OBJECT_RECORDS, read_amp};
    use oqueue_core::{
        ByteRange, CommitVersion, CommittedSpan, FakeMaterializedIndex, MAX_BATCHES_PER_PAGE,
        MaterializedIndex, MetadataEntry, MetadataRecord, ObjectKey, Offset, PartitionId,
        TAIL_WINDOW_ENTRIES, TopicId,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};

    fn topic() -> TopicId {
        TopicId::new("t").expect("a valid topic")
    }

    fn partition() -> PartitionId {
        PartitionId::new(0).expect("a valid partition")
    }

    fn offset(value: i64) -> Offset {
        Offset::new(value).expect("a valid offset")
    }

    /// Folds `counts` into an index, one object per entry, in order.
    fn index_of(counts: &[u32]) -> FakeMaterializedIndex {
        let index = FakeMaterializedIndex::new();
        let entries: Vec<MetadataEntry> = counts
            .iter()
            .enumerate()
            .map(|(i, count)| {
                let version = CommitVersion::new(i as u64 + 1);
                let span = CommittedSpan::new(
                    topic(),
                    partition(),
                    *count,
                    ByteRange::bounded(0, u64::from(*count)).expect("a valid range"),
                    None,
                );
                MetadataEntry::new(
                    version,
                    MetadataRecord::BatchCommitted {
                        object: ObjectKey::new(format!("obj-{i}")).expect("a valid key"),
                        spans: vec![span],
                    },
                )
            })
            .collect();
        index.apply(&entries).expect("a valid fold");
        index
    }

    #[test]
    fn one_object_covering_the_range_is_unamplified() {
        let index = index_of(&[COMPACTED_OBJECT_RECORDS]);
        let amp = read_amp(
            &index,
            &topic(),
            partition(),
            offset(0),
            offset(i64::from(COMPACTED_OBJECT_RECORDS)),
        )
        .expect("an index that answers");
        assert_eq!(amp.objects_touched(), 1);
        assert_eq!(amp.objects_needed(), 1);
        assert!((amp.ratio() - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn the_same_range_spread_over_twelve_objects_amplifies_twelvefold() {
        let per = COMPACTED_OBJECT_RECORDS / 12;
        assert!(per > 0, "the compacted target must exceed twelve records");
        let index = index_of(&[per; 12]);
        let amp = read_amp(
            &index,
            &topic(),
            partition(),
            offset(0),
            offset(i64::from(per) * 12),
        )
        .expect("an index that answers");
        assert_eq!(amp.objects_touched(), 12);
        assert_eq!(amp.objects_needed(), 1);
        assert!((amp.ratio() - 12.0).abs() < f64::EPSILON);
    }

    #[test]
    fn a_range_wider_than_one_page_counts_every_object() {
        let objects = MAX_BATCHES_PER_PAGE * 3 + 7;
        let index = index_of(&vec![1_u32; objects]);
        let amp = read_amp(
            &index,
            &topic(),
            partition(),
            offset(0),
            offset(i64::try_from(objects).expect("a representable count")),
        )
        .expect("an index that answers");
        assert_eq!(
            amp.objects_touched(),
            objects,
            "the walk must resume past a page bound, not stop at it"
        );
        assert_eq!(
            amp.records(),
            i64::try_from(objects).expect("a representable count")
        );
    }

    #[test]
    fn records_outside_the_range_are_not_counted() {
        let index = index_of(&[10; 4]);
        let amp = read_amp(&index, &topic(), partition(), offset(15), offset(25))
            .expect("an index that answers");
        assert_eq!(
            amp.objects_touched(),
            2,
            "offsets 15..25 straddle the second and third objects only"
        );
        assert_eq!(amp.records(), 10, "the overlap, not the objects' own spans");
    }

    #[test]
    fn an_unfolded_partition_has_no_amplification() {
        let index = FakeMaterializedIndex::new();
        let amp = read_amp(&index, &topic(), partition(), offset(0), offset(100))
            .expect("an index that answers");
        assert_eq!(amp.objects_touched(), 0);
        assert_eq!(amp.objects_needed(), 0);
        assert!(
            (amp.ratio() - 0.0).abs() < f64::EPSILON,
            "no work is not a low ratio"
        );
    }

    #[test]
    fn objects_needed_is_the_compacted_layout_rather_than_always_one() {
        let per = COMPACTED_OBJECT_RECORDS;
        let index = index_of(&[per, per, 1]);
        let amp = read_amp(
            &index,
            &topic(),
            partition(),
            offset(0),
            offset(i64::from(per) * 2 + 1),
        )
        .expect("an index that answers");
        assert_eq!(amp.objects_touched(), 3);
        assert_eq!(
            amp.objects_needed(),
            3,
            "two full compacted objects and a remainder"
        );
        assert!((amp.ratio() - 1.0).abs() < f64::EPSILON);
    }

    /// Counts what the walk asks of the index, because the cost of the trigger
    /// is the point of it.
    #[derive(Debug)]
    struct CountingIndex {
        inner: FakeMaterializedIndex,
        queries: AtomicUsize,
    }

    impl CountingIndex {
        fn over(counts: &[u32]) -> Self {
            Self {
                inner: index_of(counts),
                queries: AtomicUsize::new(0),
            }
        }
    }

    impl MaterializedIndex for CountingIndex {
        fn apply(&self, entries: &[MetadataEntry]) -> oqueue_core::Result<()> {
            self.inner.apply(entries)
        }

        fn applied_upto(&self) -> Option<CommitVersion> {
            self.inner.applied_upto()
        }

        fn entries(&self) -> usize {
            self.inner.entries()
        }

        fn end_offset(&self, topic: &TopicId, partition: PartitionId) -> Offset {
            self.inner.end_offset(topic, partition)
        }

        fn find_batches(
            &self,
            topic: &TopicId,
            partition: PartitionId,
            start: Offset,
            max_bytes: u64,
        ) -> oqueue_core::Result<Vec<oqueue_core::IndexedBatch>> {
            self.queries.fetch_add(1, Ordering::Relaxed);
            self.inner.find_batches(topic, partition, start, max_bytes)
        }

        fn clear(&self) {
            self.inner.clear();
        }
    }

    /// ⚠️ **A range ending exactly on an object boundary must not ask again.**
    /// The walk's own bound is `cursor < end`; at `<=` the cursor that has
    /// reached the end queries one more page, finds every batch at or past the
    /// end and counts none — the same answer for one extra index query per
    /// candidate partition per sweep, which at `ADR-0036`'s cadence is the
    /// cost this whole module is shaped around.
    #[test]
    fn a_walk_that_reaches_the_end_of_its_range_asks_no_further() {
        let index = CountingIndex::over(&[10; 3]);
        let amp = read_amp(&index, &topic(), partition(), offset(0), offset(20))
            .expect("an index that answers");
        assert_eq!(amp.objects_touched(), 2);
        assert_eq!(
            index.queries.load(Ordering::Relaxed),
            1,
            "one page covered the range, so one query is the whole cost"
        );
    }

    /// ⚠️ **One object, two spans for one partition** — which `IndexState`'s
    /// fold admits. Counting records per key rather than per span would report
    /// half the records and so half the amplification, and a planner reading
    /// that skips a range that needs compacting.
    #[test]
    fn two_spans_of_one_object_count_their_records_separately() {
        let index = FakeMaterializedIndex::new();
        let span = |count: u32| {
            CommittedSpan::new(
                topic(),
                partition(),
                count,
                ByteRange::bounded(0, u64::from(count)).expect("a valid range"),
                None,
            )
        };
        let entry = MetadataEntry::new(
            CommitVersion::new(1),
            MetadataRecord::BatchCommitted {
                object: ObjectKey::new("obj-0").expect("a valid key"),
                spans: vec![
                    span(COMPACTED_OBJECT_RECORDS),
                    span(COMPACTED_OBJECT_RECORDS),
                ],
            },
        );
        index.apply(&[entry]).expect("a valid fold");

        let amp = read_amp(
            &index,
            &topic(),
            partition(),
            offset(0),
            offset(i64::from(COMPACTED_OBJECT_RECORDS) * 2),
        )
        .expect("an index that answers");
        assert_eq!(amp.objects_touched(), 1, "one key, however many spans");
        assert_eq!(
            amp.records(),
            i64::from(COMPACTED_OBJECT_RECORDS) * 2,
            "both spans' records are in range"
        );
        assert_eq!(amp.objects_needed(), 2);
    }

    /// ⚠️ **The tail tier is counted, not merely detected.** The planner trims
    /// a range at the first tail object, so a count that always answered zero
    /// would let tail data be rewritten — the one thing the age guard exists
    /// to prevent.
    #[test]
    fn objects_still_in_the_tail_are_counted_and_located() {
        let hot = 12;
        let index = index_of(&vec![1_u32; hot]);
        let amp = read_amp(
            &index,
            &topic(),
            partition(),
            offset(0),
            offset(i64::try_from(hot).expect("a small count")),
        )
        .expect("an index that answers");
        assert_eq!(
            amp.tail_objects(),
            hot,
            "a fresh partition's objects are all inside the tail window"
        );
        assert_eq!(
            amp.first_tail_base(),
            Some(offset(0)),
            "the boundary is the first of them, in ascending order"
        );
    }

    #[test]
    fn a_range_wholly_in_history_reports_no_tail() {
        let mut counts = vec![1_u32; 4];
        counts.extend(std::iter::repeat_n(1_u32, TAIL_WINDOW_ENTRIES));
        let index = index_of(&counts);
        let amp = read_amp(&index, &topic(), partition(), offset(0), offset(4))
            .expect("an index that answers");
        assert_eq!(amp.tail_objects(), 0);
        assert_eq!(amp.first_tail_base(), None);
    }
}
