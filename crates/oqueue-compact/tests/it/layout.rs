//! The round's output layout: one object, and one read per partition-fetch.

// A panic in a test harness is the test failing, which is what it is for.
#![allow(clippy::expect_used)]

use oqueue_compact::{COMPACTION_PLAN_RECORDS_BUDGET, PlannedInputs, merge_round};
use oqueue_core::{ByteRange, Error, ObjectStore, Region, TopicId, parse_footer};
use std::sync::atomic::Ordering;

use crate::naming::namer;
use crate::support::{
    Counting, fill_for, inputs_of_topic, partition, partition_n, planned_from_topic,
    planned_records_topic, planned_topic, shifted_inputs, topic,
};

/// The regions one partition contributed, in the order the object holds them.
fn runs(regions: &[Region]) -> Vec<(TopicId, i32)> {
    let mut seen: Vec<(TopicId, i32)> = Vec::new();
    for region in regions {
        let here = (region.topic().clone(), region.partition().get());
        if seen.last() != Some(&here) {
            seen.push(here);
        }
    }
    seen
}

/// ⚠️ **One run per partition, whatever order the plans arrive in.** A second
/// run for a partition already laid down is a second ranged GET for every
/// fetch over that range, for ever.
#[tokio::test]
async fn a_round_lays_each_partition_down_once() {
    let store = Counting::new();
    let mut round = Vec::new();
    // Interleaved on purpose: u/1, t/1, u/0, t/0.
    for (name, index) in [("u", 1), ("t", 1), ("u", 0), ("t", 0)] {
        let topic = TopicId::new(name).expect("a valid topic");
        let refs = inputs_of_topic(&store, (&topic, partition_n(index)), 13, 1, 32).await;
        round.push(PlannedInputs::new(
            planned_topic(&topic, partition_n(index), 13),
            refs,
        ));
    }

    let outcome = merge_round(&store, &round, &mut namer())
        .await
        .expect("a round that runs");
    assert_eq!(outcome.puts(), 1, "one object for the whole round");
    assert_eq!(outcome.gets(), 52, "one read per input across four plans");
    assert_eq!(outcome.records(), 52);
    let sealed = outcome.object().expect("a round seals one object");

    let written = store
        .inner
        .get(sealed, ByteRange::Full)
        .await
        .expect("an output object");
    let regions = parse_footer(&written, written.len() as u64).expect("a valid footer");
    let order = runs(&regions);
    assert_eq!(
        order,
        vec![
            (topic(), 0),
            (topic(), 1),
            (TopicId::new("u").expect("a valid topic"), 0),
            (TopicId::new("u").expect("a valid topic"), 1),
        ],
        "topic, then partition, each exactly once"
    );
}

/// ⚠️ **The end-to-end claim**: after compaction, a fetch over the whole
/// compacted range reads one contiguous span of one object — which is the
/// `read_amp == 1.0` `M5.1` measures, asserted against the bytes rather than
/// against the index.
#[tokio::test]
async fn a_compacted_partition_s_range_is_one_contiguous_span() {
    let store = Counting::new();
    let mut round = Vec::new();
    for (name, index) in [("t", 0), ("u", 0)] {
        let topic = TopicId::new(name).expect("a valid topic");
        let refs = inputs_of_topic(&store, (&topic, partition_n(index)), 13, 1, 64).await;
        round.push(PlannedInputs::new(
            planned_topic(&topic, partition_n(index), 13),
            refs,
        ));
    }
    store.gets.store(0, Ordering::Relaxed);

    let outcome = merge_round(&store, &round, &mut namer())
        .await
        .expect("a round that runs");
    let sealed = outcome.object().expect("a round seals one object");

    let written = store
        .inner
        .get(sealed, ByteRange::Full)
        .await
        .expect("an output object");
    let regions = parse_footer(&written, written.len() as u64).expect("a valid footer");
    let mine: Vec<&Region> = regions
        .iter()
        .filter(|region| region.topic() == &topic() && region.partition() == partition())
        .collect();
    assert_eq!(mine.len(), 13, "thirteen inputs, thirteen regions");

    let span = contiguous_span(&mine);

    // One ranged GET over that span returns every record of the range.
    store.gets.store(0, Ordering::Relaxed);
    let read = store.inner.get(sealed, span).await.expect("a ranged read");
    let ByteRange::Bounded(bounded) = span else {
        panic!("a bounded span")
    };
    assert_eq!(
        read.len() as u64,
        bounded.length(),
        "one read covers the whole compacted range"
    );
    assert!(
        read.iter().all(|byte| *byte == fill_for(&topic())),
        "and it holds this topic's records only — `u`'s fill byte differs"
    );
}

/// The one span covering `regions`, asserting they are contiguous on the way.
fn contiguous_span(regions: &[&Region]) -> ByteRange {
    let mut start = None;
    let mut next: Option<u64> = None;
    for region in regions {
        let ByteRange::Bounded(span) = region.bytes() else {
            panic!("a bundled region is always bounded")
        };
        if let Some(expected) = next {
            assert_eq!(
                span.offset(),
                expected,
                "this partition's regions must be one contiguous span"
            );
        }
        start.get_or_insert_with(|| span.offset());
        next = Some(span.offset() + span.length());
    }
    let from = start.expect("at least one region");
    ByteRange::bounded(from, next.expect("at least one region") - from).expect("a valid range")
}

/// ⚠️ **An empty round is a quiet cluster, not a failure.** A sweep finds no
/// candidate on most rounds, and an error there is one an operator learns to
/// ignore — which is how a real one gets ignored too.
#[tokio::test]
async fn an_empty_round_writes_nothing_and_says_so() {
    let store = Counting::new();
    let outcome = merge_round(&store, &[], &mut namer())
        .await
        .expect("nothing to do is not a failure");
    assert_eq!(
        outcome.puts(),
        0,
        "an empty object costs what a full one does"
    );
    assert_eq!(outcome.gets(), 0);
    assert_eq!(outcome.records(), 0);
    assert!(outcome.spans().is_empty());
    assert_eq!(store.puts.load(Ordering::Relaxed), 0);
}

/// ⚠️ **Two plans over the same offsets would write those records twice**,
/// contiguously and with no error — and the commit's fold would then assign
/// offsets by span, shifting every later offset in the partition. `merge`
/// refuses this between an input list's references; this is the same refusal
/// one level up.
#[tokio::test]
async fn a_round_naming_one_range_twice_is_refused() {
    let store = Counting::new();
    let refs = inputs_of_topic(&store, (&topic(), partition()), 13, 1, 32).await;
    let round = vec![
        PlannedInputs::new(planned_topic(&topic(), partition(), 13), refs.clone()),
        PlannedInputs::new(planned_topic(&topic(), partition(), 13), refs),
    ];
    let outcome = merge_round(&store, &round, &mut namer()).await;
    assert!(
        matches!(outcome, Err(Error::OverlappingCompactionPlans)),
        "the same range planned twice: {outcome:?}"
    );
    assert_eq!(
        store.puts.load(Ordering::Relaxed),
        0,
        "refused before anything is written"
    );
}

/// The budget is per plan, so without a round-level one a hundred plans at
/// budget accumulate a hundred objects' worth before the first PUT.
#[tokio::test]
async fn a_round_over_the_records_budget_is_refused() {
    let store = Counting::new();
    let per = 32_768_u32;
    let objects = 16_usize;
    let mut round = Vec::new();
    for name in ["t", "u"] {
        let topic = TopicId::new(name).expect("a valid topic");
        let refs = inputs_of_topic(&store, (&topic, partition()), objects, per, 8).await;
        round.push(PlannedInputs::new(
            planned_records_topic(&topic, partition(), per, objects),
            refs,
        ));
    }
    let outcome = merge_round(&store, &round, &mut namer()).await;
    assert!(
        matches!(outcome, Err(Error::CompactionRoundTooLarge { .. })),
        "two plans each at the budget are twice the budget: {outcome:?}"
    );
    assert_eq!(store.puts.load(Ordering::Relaxed), 0);
}

/// ⚠️ **The round's PUT count is one however many plans it holds**, which the
/// per-plan estimate cannot say on its own — a caller summing estimates over a
/// round predicts one PUT per plan and gets one for the round.
#[tokio::test]
async fn a_round_reads_every_plan_s_inputs_and_writes_once() {
    let store = Counting::new();
    let mut round = Vec::new();
    let mut estimated_gets = 0;
    for name in ["t", "u"] {
        let topic = TopicId::new(name).expect("a valid topic");
        let refs = inputs_of_topic(&store, (&topic, partition()), 13, 1, 32).await;
        let plan = planned_topic(&topic, partition(), 13);
        estimated_gets += plan.cost().gets();
        round.push(PlannedInputs::new(plan, refs));
    }
    store.gets.store(0, Ordering::Relaxed);

    let outcome = merge_round(&store, &round, &mut namer())
        .await
        .expect("a round that runs");
    assert_eq!(
        outcome.gets(),
        estimated_gets,
        "the per-plan estimates sum to the round's reads"
    );
    assert_eq!(outcome.puts(), 1, "and the round writes once regardless");
}

/// ⚠️ **Adjacent is not overlapping.** A round compacting `[0,13)` and
/// `[13,26)` of one partition is the normal shape once a partition has more
/// than one amplified range, and refusing it would stall compaction there.
#[tokio::test]
async fn a_round_with_adjacent_ranges_for_one_partition_runs() {
    let store = Counting::new();
    let low = inputs_of_topic(&store, (&topic(), partition()), 13, 1, 32).await;
    let high = shifted_inputs(&store, (&topic(), partition()), 13, 13).await;
    let round = vec![
        PlannedInputs::new(planned_topic(&topic(), partition(), 13), low),
        PlannedInputs::new(planned_from_topic(&topic(), partition(), 13, 26), high),
    ];
    let outcome = merge_round(&store, &round, &mut namer())
        .await
        .expect("adjacent ranges are not overlapping ones");
    assert_eq!(outcome.records(), 26);
}

/// ⚠️ **The budget is a ceiling, not a bar.** A round landing exactly on it
/// must run, or the largest admissible round is one record smaller than the
/// number every document states.
#[tokio::test]
async fn a_round_at_exactly_the_records_budget_runs() {
    let store = Counting::new();
    let objects = 16_usize;
    let per = u32::try_from(
        COMPACTION_PLAN_RECORDS_BUDGET / i64::try_from(objects).expect("a small count"),
    )
    .expect("a budget that divides");
    let refs = inputs_of_topic(&store, (&topic(), partition()), objects, per, 8).await;
    let round = vec![PlannedInputs::new(
        planned_records_topic(&topic(), partition(), per, objects),
        refs,
    )];
    let outcome = merge_round(&store, &round, &mut namer())
        .await
        .expect("exactly at the budget is inside it");
    assert_eq!(outcome.records(), COMPACTION_PLAN_RECORDS_BUDGET);
}
