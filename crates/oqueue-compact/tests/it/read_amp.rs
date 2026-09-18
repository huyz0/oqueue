//! What the trigger costs the index, measured through the public surface.
//!
//! ⚠️ **Here rather than in `read_amp.rs`** (`M5.41`): both exercise
//! `read_amp` and `plan` through the crate's public API only, and both need a
//! `MaterializedIndex` double that counts what is asked of it — not a store
//! double, because nothing on this path may reach a store at all (`M5.38`).
//! The module was 38 lines short of `code-structure.md`'s 500-line limit when
//! they moved, so headroom is what the move bought rather than what forced it.

// A panic in a test harness is the test failing, which is what it is for.
#![allow(clippy::expect_used)]

use oqueue_compact::{Planning, plan, read_amp};
use oqueue_core::{
    CommitVersion, FakeMaterializedIndex, MAX_BATCHES_PER_PAGE, MaterializedIndex, MetadataEntry,
    ObjectKey, Offset, PartitionId, TAIL_WINDOW_ENTRIES, TopicId,
};
use std::sync::atomic::{AtomicUsize, Ordering};

use crate::support::{history_index, offset, partition, topic};

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
            inner: history_index(counts),
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

    fn manifest(&self, topic: &TopicId, partition: PartitionId) -> Option<(ObjectKey, Offset)> {
        self.inner.manifest(topic, partition)
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

/// ⚠️ **One walk per plan, including the straddling one.** `plan` trims a
/// range at the tail boundary and needs the history portion's
/// amplification; a second `read_amp` call for it doubled the index walks
/// on the path a live partition always takes.
#[test]
fn planning_a_straddling_range_walks_the_index_once() {
    let mut counts = vec![1_u32; 20];
    counts.extend(std::iter::repeat_n(1_u32, TAIL_WINDOW_ENTRIES));
    let index = CountingIndex {
        inner: history_index(&counts),
        queries: AtomicUsize::new(0),
    };
    let whole = 20 + i64::try_from(TAIL_WINDOW_ENTRIES).expect("a small window");

    let planned = plan(&index, &topic(), partition(), offset(0), offset(whole))
        .expect("an index that answers");
    let Planning::Planned(planned) = planned else {
        panic!("the history portion is amplified: {planned:?}")
    };
    // ⚠️ **The history portion only.** The trimmed measurement must stop at the
    // tail boundary: counting the first tail object into it would plan a range
    // whose last object is still served from cache.
    assert_eq!(
        planned.amplification().records(),
        20,
        "twenty history records, and none of the tail's"
    );
    assert_eq!(planned.end(), offset(20), "the plan stops at the boundary");
    assert_eq!(
        index.queries.load(Ordering::Relaxed),
        3,
        "one walk, paged: {whole} objects over {MAX_BATCHES_PER_PAGE}-batch pages"
    );
}
