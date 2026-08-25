//! Assign → journal → ack: what the single coordinator promises about offsets.
//!
//! `M3.7`, and the FR-11 property the milestone's completion condition runs:
//! concurrent producers over one partition see exactly `0..k` with no gaps and
//! no overlaps. The other tests here are the ordering claims that make that
//! property hold rather than happen to hold — a position is journaled before
//! it is acknowledged, a refused journal consumes neither a version nor an
//! offset, and nothing on an error path reports `0`.

// Every `expect` below is on a value the test itself constructed, or on a step
// whose failure *is* the test failing. Same allowance, same reason, as
// `oqueue-core`'s own suites.
#![allow(clippy::expect_used)]

use oqueue_coordinator::{CommitAck, Coordinator, CoordinatorError, UNASSIGNED_OFFSET};
use oqueue_core::{
    BoxFuture, ByteRange, CommitVersion, CommittedSpan, CoordinatorEpoch, Error,
    FakeMaterializedIndex, FakeMetadataLog, MaterializedIndex, MetadataEntry, MetadataLog,
    ObjectKey, Offset, PartitionId, Result, TopicId,
};
use proptest::prelude::*;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::task::JoinSet;

fn topic() -> TopicId {
    TopicId::new("t".to_owned()).expect("literal is a valid topic id")
}

fn partition() -> PartitionId {
    PartitionId::new(0).expect("0 is a valid partition")
}

fn object(n: usize) -> ObjectKey {
    ObjectKey::new(format!("o-{n}")).expect("literal is a valid object key")
}

fn span(records: u32) -> CommittedSpan {
    CommittedSpan::new(topic(), partition(), records, ByteRange::Full)
}

/// A span of a *bundled* object: a real region rather than the whole of it.
///
/// ⚠️ [`ByteRange::Full`] is only correct for an object holding one span, so
/// the multi-span tests below name bounded regions — two spans both claiming
/// the whole object are not two regions.
fn region(name: &str, partition: i32, records: u32, at: u64) -> CommittedSpan {
    CommittedSpan::new(
        TopicId::new(name.to_owned()).expect("literal is a valid topic id"),
        PartitionId::new(partition).expect("a valid partition"),
        records,
        ByteRange::bounded(at, 16).expect("a non-empty range"),
    )
}

/// A [`MetadataLog`] whose `append` can be made to refuse, so a test can watch
/// what the coordinator does when the journal step fails.
///
/// ⚠️ A fake rather than a mock (`testing.md` rule 3): it delegates to
/// [`FakeMetadataLog`] and adds one switch, so what it stores when it is *not*
/// refusing is the real contract rather than a recorded expectation.
#[derive(Debug)]
struct RefusableLog {
    inner: FakeMetadataLog,
    refusing: AtomicBool,
}

impl RefusableLog {
    const fn new() -> Self {
        Self {
            inner: FakeMetadataLog::new(),
            refusing: AtomicBool::new(false),
        }
    }

    fn refuse(&self, refusing: bool) {
        self.refusing.store(refusing, Ordering::SeqCst);
    }

    fn len(&self) -> usize {
        self.inner.len()
    }
}

impl MetadataLog for RefusableLog {
    fn append<'a>(&'a self, entries: &'a [MetadataEntry]) -> BoxFuture<'a, Result<()>> {
        Box::pin(async move {
            if self.refusing.load(Ordering::SeqCst) {
                return Err(Error::Transient);
            }
            self.inner.append(entries).await
        })
    }

    fn read_from(
        &self,
        start: CommitVersion,
        max_entries: usize,
    ) -> BoxFuture<'_, Result<Vec<MetadataEntry>>> {
        self.inner.read_from(start, max_entries)
    }

    fn last_version(&self) -> BoxFuture<'_, Result<Option<CommitVersion>>> {
        self.inner.last_version()
    }
}

/// Opens a coordinator over `log` and spawns its serializing loop, returning
/// the handle and the loop's join handle.
async fn start(log: Arc<dyn MetadataLog>) -> (Coordinator, tokio::task::JoinHandle<()>) {
    let (coordinator, driver) = Coordinator::open(log, CoordinatorEpoch::ZERO)
        .await
        .expect("a fresh log opens");
    (coordinator, tokio::spawn(driver.run()))
}

#[tokio::test]
async fn a_position_is_journaled_before_it_is_acknowledged() {
    let log = Arc::new(FakeMetadataLog::new());
    let (coordinator, driver) = start(log.clone()).await;

    let ack = coordinator
        .commit(object(0), vec![span(3)])
        .await
        .expect("the commit lands");

    // The ack names a version the log is already holding — not one the
    // coordinator intends to write. `ADR-0020` point 3.
    let stored = log
        .read_from(CommitVersion::ZERO, 16)
        .await
        .expect("the log reads back");
    assert_eq!(stored.len(), 1, "the acknowledged commit is in the log");
    assert_eq!(stored[0].version(), ack.version());
    assert_eq!(ack.base_offset(&topic(), partition()), 0);

    drop(coordinator);
    driver
        .await
        .expect("the loop ends when its last handle drops");
}

#[tokio::test]
async fn a_commit_version_is_stamped_in_append_order() {
    let log = Arc::new(FakeMetadataLog::new());
    let (coordinator, driver) = start(log.clone()).await;

    let mut acked = Vec::new();
    for n in 0..3 {
        acked.push(
            coordinator
                .commit(object(n), vec![span(1)])
                .await
                .expect("the commit lands")
                .version(),
        );
    }

    let stored: Vec<CommitVersion> = log
        .read_from(CommitVersion::ZERO, 16)
        .await
        .expect("the log reads back")
        .iter()
        .map(MetadataEntry::version)
        .collect();
    assert_eq!(acked, stored, "every acked version is where the log put it");
    assert_eq!(
        stored,
        vec![
            CommitVersion::new(0),
            CommitVersion::new(1),
            CommitVersion::new(2)
        ],
        "one allocator, counting up, with no holes"
    );

    drop(coordinator);
    driver
        .await
        .expect("the loop ends when its last handle drops");
}

#[tokio::test]
async fn a_refused_journal_consumes_neither_a_version_nor_an_offset() {
    let log = Arc::new(RefusableLog::new());
    let (coordinator, driver) = start(log.clone()).await;

    log.refuse(true);
    let refused = coordinator
        .commit(object(0), vec![span(5)])
        .await
        .expect_err("a journal that refuses is not an ack");
    assert_eq!(refused, CoordinatorError::Journal(Error::Transient));
    assert_eq!(log.len(), 0, "a refused append stores nothing");

    // The retry gets the position the refused attempt would have had. If the
    // failed attempt had advanced the allocator, this would start at offset 5
    // — a gap FR-11 forbids — or at version 1.
    log.refuse(false);
    let ack = coordinator
        .commit(object(1), vec![span(5)])
        .await
        .expect("the retry lands");
    assert_eq!(ack.version(), CommitVersion::ZERO);
    assert_eq!(ack.base_offset(&topic(), partition()), 0);

    drop(coordinator);
    driver
        .await
        .expect("the loop ends when its last handle drops");
}

#[tokio::test]
async fn a_partition_no_ack_covers_reports_the_unset_sentinel() {
    let log = Arc::new(FakeMetadataLog::new());
    let (coordinator, driver) = start(log).await;

    let ack = coordinator
        .commit(object(0), vec![span(1)])
        .await
        .expect("the commit lands");

    let other = PartitionId::new(7).expect("7 is a valid partition");
    assert_eq!(
        ack.base_offset(&topic(), other),
        UNASSIGNED_OFFSET,
        "an uncovered partition has no offset, and says so"
    );
    assert_eq!(
        UNASSIGNED_OFFSET, -1,
        "the sentinel is -1; `0` is a plausible-looking offset and a safety bug"
    );

    drop(coordinator);
    driver
        .await
        .expect("the loop ends when its last handle drops");
}

#[tokio::test]
async fn a_coordinator_that_has_stopped_refuses_rather_than_answering() {
    let log = Arc::new(FakeMetadataLog::new());
    let (coordinator, driver) = Coordinator::open(log, CoordinatorEpoch::ZERO)
        .await
        .expect("a fresh log opens");
    drop(driver);

    assert_eq!(
        coordinator.commit(object(0), vec![span(1)]).await,
        Err(CoordinatorError::Unavailable),
        "no loop means no position — never an offset invented to fill the hole"
    );
}

#[tokio::test]
async fn opening_over_a_log_that_already_holds_entries_refuses() {
    let log = Arc::new(FakeMetadataLog::new());
    let (coordinator, driver) = start(log.clone()).await;
    coordinator
        .commit(object(0), vec![span(2)])
        .await
        .expect("the commit lands");
    drop(coordinator);
    driver.await.expect("the loop ends");

    // A second coordinator over the same log would restart its offset line at
    // zero, which is silent duplication rather than a failure. `M3.8` replays;
    // until it does, this refuses.
    assert_eq!(
        Coordinator::open(log, CoordinatorEpoch::ZERO).await.err(),
        Some(CoordinatorError::ReplayRequired { last_version: 0 })
    );
}

#[tokio::test]
async fn the_coordinator_and_the_index_fold_to_the_same_offsets() {
    let log = Arc::new(FakeMetadataLog::new());
    let (coordinator, driver) = start(log.clone()).await;

    let mut end = 0_i64;
    for n in 0..4_u32 {
        let ack = coordinator
            .commit(object(n as usize), vec![span(n + 1)])
            .await
            .expect("the commit lands");
        assert_eq!(ack.base_offset(&topic(), partition()), end);
        end += i64::from(n + 1);
    }

    let index = FakeMaterializedIndex::new();
    index
        .apply(
            &log.read_from(CommitVersion::ZERO, 64)
                .await
                .expect("the log reads back"),
        )
        .expect("the log folds");
    assert_eq!(
        index.end_offset(&topic(), partition()),
        Offset::new(end).expect("a non-negative offset"),
        "the allocator and the index derive the same line from the same log"
    );

    drop(coordinator);
    driver
        .await
        .expect("the loop ends when its last handle drops");
}

/// One object bundling four spans across three `(topic, partition)` lines.
///
/// ⚠️ `u/0` comes first on purpose. It shares a partition id with `t/0` and
/// nothing else, so a lookup matching on either half rather than both would
/// find it — and would find it *first*, which is the interleaving that makes
/// the difference observable rather than merely present.
async fn commit_a_bundle(coordinator: &Coordinator) -> CommitAck {
    coordinator
        .commit(
            object(0),
            vec![
                region("u", 0, 1, 0),
                region("t", 0, 2, 16),
                region("t", 0, 4, 32),
                region("t", 1, 3, 48),
            ],
        )
        .await
        .expect("the commit lands")
}

#[tokio::test]
async fn spans_of_one_bundled_object_accumulate_per_topic_and_partition() {
    let log = Arc::new(FakeMetadataLog::new());
    let (coordinator, driver) = start(log).await;

    let ack = commit_a_bundle(&coordinator).await;

    let placed: Vec<(&str, i32, i64)> = ack
        .assignments()
        .iter()
        .map(|a| {
            (
                a.topic().as_str(),
                a.partition().get(),
                a.base_offset().get(),
            )
        })
        .collect();
    assert_eq!(
        placed,
        vec![("u", 0, 0), ("t", 0, 0), ("t", 0, 2), ("t", 1, 0)],
        "the second `t/0` span starts where the first one ended, and no other \
         line moved with it"
    );

    drop(coordinator);
    driver
        .await
        .expect("the loop ends when its last handle drops");
}

#[tokio::test]
async fn every_line_a_bundle_advanced_continues_from_where_it_left_off() {
    let log = Arc::new(FakeMetadataLog::new());
    let (coordinator, driver) = start(log.clone()).await;
    commit_a_bundle(&coordinator).await;

    let next = coordinator
        .commit(object(1), vec![region("t", 0, 1, 0), region("t", 1, 2, 16)])
        .await
        .expect("the commit lands");
    assert_eq!(
        next.base_offset(&topic(), partition()),
        6,
        "`t/0` took two spans of the bundle, so it continues from 6, not from 2"
    );
    assert_eq!(
        next.base_offset(&topic(), PartitionId::new(1).expect("a valid partition")),
        3,
        "a line the same bundle advanced is remembered separately from its \
         neighbours"
    );

    let index = FakeMaterializedIndex::new();
    index
        .apply(
            &log.read_from(CommitVersion::ZERO, 64)
                .await
                .expect("the log reads back"),
        )
        .expect("the log folds");
    for (name, part, end) in [("t", 0, 7_i64), ("t", 1, 5), ("u", 0, 1)] {
        assert_eq!(
            index.end_offset(
                &TopicId::new(name.to_owned()).expect("a valid topic id"),
                PartitionId::new(part).expect("a valid partition"),
            ),
            Offset::new(end).expect("a non-negative offset"),
            "{name}/{part}: the allocator and the index fold the same log alike"
        );
    }

    drop(coordinator);
    driver
        .await
        .expect("the loop ends when its last handle drops");
}

#[tokio::test]
async fn a_partition_named_twice_in_one_object_is_attributed_by_position() {
    let log = Arc::new(FakeMetadataLog::new());
    let (coordinator, driver) = start(log).await;

    // Two producers' batches for `t/0`, bundled into one object.
    let ack = coordinator
        .commit(object(0), vec![region("t", 0, 2, 0), region("t", 0, 4, 16)])
        .await
        .expect("the commit lands");

    let bases: Vec<i64> = ack
        .assignments()
        .iter()
        .map(|a| a.base_offset().get())
        .collect();
    assert_eq!(
        bases,
        vec![0, 2],
        "the nth assignment describes the nth span — the only attribution a \
         bundler may use, since the pairs are not distinct"
    );
    assert_eq!(
        ack.base_offset(&topic(), partition()),
        0,
        "and `base_offset` answers the coarser question: where this commit's \
         contribution to the partition begins"
    );
    // The contribution is contiguous from there, which is what makes the
    // coarser answer usable at all: 2 + 4 records, no hole between them.
    assert_eq!(
        ack.assignments()[1].base_offset().get(),
        bases[0] + i64::from(ack.assignments()[0].record_count())
    );

    drop(coordinator);
    driver
        .await
        .expect("the loop ends when its last handle drops");
}

/// The FR-11 case `scripts/gates/m3-complete.sh` runs.
///
/// ⚠️ The assertion is set-shaped on purpose: it says the union of every
/// producer's assigned range is exactly `0..k`. That admits any interleaving —
/// which is the point, since the scheduler picks one — while rejecting a gap,
/// an overlap, and a duplicate alike.
#[test]
fn concurrent_producers_over_one_partition_yield_exactly_zero_to_k() {
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .build()
        .expect("a test runtime");

    proptest!(|(counts in prop::collection::vec(1_u32..=8, 1..=16))| {
        let total: i64 = counts.iter().map(|c| i64::from(*c)).sum();
        let assigned = runtime.block_on(async {
            let log = Arc::new(FakeMetadataLog::new());
            let (coordinator, driver) = start(log).await;

            let mut producers = JoinSet::new();
            for (n, count) in counts.iter().copied().enumerate() {
                let handle = coordinator.clone();
                producers.spawn(async move {
                    let ack = handle
                        .commit(object(n), vec![span(count)])
                        .await
                        .expect("every commit lands");
                    let assignment = &ack.assignments()[0];
                    (assignment.base_offset().get(), assignment.record_count())
                });
            }

            let mut ranges = Vec::new();
            while let Some(joined) = producers.join_next().await {
                ranges.push(joined.expect("no producer panics"));
            }
            drop(coordinator);
            driver.await.expect("the loop ends");
            ranges
        });

        let mut covered: Vec<i64> = assigned
            .iter()
            .flat_map(|(base, count)| *base..*base + i64::from(*count))
            .collect();
        covered.sort_unstable();
        prop_assert_eq!(covered, (0..total).collect::<Vec<i64>>());
    });
}
