//! What offsets a commit takes: within one bundled object, and under
//! concurrent producers.
//!
//! ⚠️ **Split from `sequencing.rs` at the 500-line limit, along the concept
//! rather than at a convenient line.** That file asserts the *ordering* claims
//! — journal before ack, a refusal consuming nothing, `-1` on every path with
//! no offset. This one asserts what the numbers are once the ordering holds.

#![allow(clippy::expect_used)]

use crate::support::{object, partition, region, span, start, topic};
use oqueue_coordinator::{CommitAck, Coordinator};
use oqueue_core::{
    CommitVersion, FakeMaterializedIndex, FakeMetadataLog, MaterializedIndex, MetadataLog, Offset,
    PartitionId, TopicId,
};
use proptest::prelude::*;
use std::sync::Arc;
use tokio::task::JoinSet;

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
