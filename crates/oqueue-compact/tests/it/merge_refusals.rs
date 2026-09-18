//! Every way the merge refuses, because each is a way acknowledged records go
//! missing with nothing failing (NFR-20).

// A panic in a test harness is the test failing, which is what it is for.
#![allow(clippy::expect_used)]

use oqueue_compact::{COMPACTION_PLAN_RECORDS_BUDGET, merge};
use oqueue_core::{BundleBuilder, CommittedSpan, Error, ObjectRef, ObjectStore, PushedRecords};
use std::sync::atomic::Ordering;

use crate::naming::namer;
use crate::support::{
    Counting, inputs_of, key, offset, other, partition, planned, planned_from, planned_records,
    topic, write_input,
};

/// ⚠️ **An object that does not hold what the index said is a refusal, not a
/// shorter output.** This is how acknowledged records go missing with nothing
/// failing: every other object tiles, the output parses, and one offset's
/// records are simply absent with every later offset shifted down.
///
/// ⚠️ **The object is hollow rather than relabelled.** A ref claiming two
/// records where the object holds one moves that ref's end offset and is
/// refused by the tiling instead — round three measured exactly that, and the
/// guard this test is for could be deleted with the suite green. Here the ref
/// tiles correctly and the *object* is what disagrees.
#[tokio::test]
async fn an_object_short_of_its_indexed_record_count_is_refused() {
    let store = Counting::new();
    let refs = inputs_of(&store, 13, 1, 32).await;

    // An object at offset 7 holding only another topic's region: it tiles, and
    // it contributes nothing to this partition.
    let mut builder = BundleBuilder::new();
    builder
        .push(
            other(),
            partition(),
            PushedRecords {
                count: 1,
                producer: None,
            },
            &[b'x'; 32],
        )
        .expect("a valid region");
    let sealed = builder.seal().expect("a sealed bundle");
    store
        .inner
        .put(&key("hollow"), sealed.into_payload(), None)
        .await
        .expect("a store that accepts");

    let mut refs = refs;
    refs[7] = ObjectRef::new(key("hollow"), offset(7), 1);
    let outcome = merge(&store, &planned(13), &refs, &mut namer()).await;
    assert!(
        matches!(outcome, Err(Error::IndexObjectMismatch)),
        "the index said one record for this partition and the object holds none: {outcome:?}"
    );
}

/// An input reaching past the plan's range would be copied whole, and the
/// overlap would then exist in two objects and be served twice.
#[tokio::test]
async fn an_input_straddling_the_plan_s_range_is_refused() {
    let store = Counting::new();
    let mut refs = inputs_of(&store, 13, 1, 32).await;
    // ⚠️ **The object really holds both records**, so the record-count check
    // cannot be what refuses this: with the reach test dropped, the merge
    // succeeds and copies a record past the plan's end.
    write_input(&store, "straddle-high", &topic(), 2, 32).await;
    let last = refs.len() - 1;
    refs[last] = ObjectRef::new(key("straddle-high"), refs[last].base_offset(), 2);
    let outcome = merge(&store, &planned(13), &refs, &mut namer()).await;
    assert!(
        matches!(outcome, Err(Error::IndexObjectMismatch)),
        "refused rather than trimmed: {outcome:?}"
    );
}

/// ⚠️ **A retry writes its own key, and the first output is untouched.**
/// `ADR-0037`: the streaming seal cannot be conditioned — `object_store`
/// carries no `PutMode` to `CompleteMultipartUpload` — so what keeps a rewrite
/// from landing on bytes an index entry already names is that no two attempts
/// share a key.
///
/// ⚠️ **Asserted against the store rather than against the index** (`M5.75`).
/// `M5.13`'s fold already makes the index safe: a second attempt at one plan
/// retires references the first retired and is refused, so the index names one
/// object whatever keys were used. What that says nothing about is the bytes —
/// and if the first attempt's swap committed, bytes overwritten under a shared
/// key are bytes a fetch resolves.
#[tokio::test]
async fn a_retry_writes_its_own_key_and_leaves_the_first_output_intact() {
    let store = Counting::new();
    let refs = inputs_of(&store, 13, 1, 64).await;
    // ⚠️ **One namer across both attempts**, which is the case that matters: a
    // retry is the same compactor trying again, and it is the sequence rather
    // than the caller's diligence that makes the second key a different one.
    let mut namer = namer();
    let before = store.inner.len();

    let first = merge(&store, &planned(13), &refs, &mut namer)
        .await
        .expect("the first merge");
    let retry = merge(&store, &planned(13), &refs, &mut namer)
        .await
        .expect("the retry, under its own key");

    let first_key = first.object().expect("a merge seals one object");
    let retry_key = retry.object().expect("and so does the retry");
    assert_ne!(first_key, retry_key, "two attempts, two keys");
    assert_eq!(
        store.inner.len(),
        before + 2,
        "and two objects in the store, not one overwritten"
    );

    let first_bytes = store
        .inner
        .get(first_key, oqueue_core::ByteRange::Full)
        .await
        .expect("the first output is still there");
    let retry_bytes = store
        .inner
        .get(retry_key, oqueue_core::ByteRange::Full)
        .await
        .expect("and so is the second");
    assert_eq!(
        first_bytes, retry_bytes,
        "the same inputs merge to the same bytes"
    );
}

/// The estimate and the run must agree — `M5.3`'s criterion, which had no
/// executor to check against until this one.
#[tokio::test]
async fn the_cost_estimate_predicts_what_the_run_does() {
    let store = Counting::new();
    let refs = inputs_of(&store, 13, 1, 64).await;
    let planned = planned(13);
    let estimate = planned.cost();
    let outcome = merge(&store, &planned, &refs, &mut namer())
        .await
        .expect("a merge that runs");

    assert_eq!(estimate.gets(), outcome.gets(), "one read per input object");
    assert_eq!(
        estimate.puts(),
        outcome.puts(),
        "one output while the records fit one compacted object"
    );
    assert_eq!(estimate.records_rewritten(), outcome.records());
}

/// The spans travel out of the merge, so the commit needs no second read.
#[tokio::test]
async fn a_merge_reports_the_spans_of_what_it_wrote() {
    let store = Counting::new();
    let refs = inputs_of(&store, 13, 1, 64).await;
    let outcome = merge(&store, &planned(13), &refs, &mut namer())
        .await
        .expect("a merge that runs");
    let total: u32 = outcome
        .spans()
        .iter()
        .map(CommittedSpan::record_count)
        .sum();
    assert_eq!(total, 13, "every record the merge moved is in a span");
    assert!(
        outcome
            .spans()
            .iter()
            .all(|span| span.topic() == &topic() && span.partition() == partition())
    );
}

/// ⚠️ **An input wholly outside the plan costs no read at all.** A merge that
/// read it would pay a GET per candidate object the plan does not cover, and
/// then refuse on the record count — the right answer for the wrong reason.
#[tokio::test]
async fn an_input_outside_the_plan_s_range_is_skipped_without_a_read() {
    let store = Counting::new();
    let mut refs = inputs_of(&store, 13, 1, 32).await;
    write_input(&store, "far", &topic(), 1, 32).await;
    refs.push(ObjectRef::new(key("far"), offset(500), 1));

    let outcome = merge(&store, &planned(13), &refs, &mut namer())
        .await
        .expect("a merge that runs");
    assert_eq!(
        outcome.gets(),
        13,
        "the object outside the range is neither read nor refused"
    );
    assert_eq!(outcome.records(), 13);
}

/// The lower edge straddles too, and refusing only the upper one would copy
/// records below the plan's start.
#[tokio::test]
async fn an_input_reaching_below_the_plan_s_start_is_refused() {
    let store = Counting::new();
    let mut refs = Vec::new();
    for i in 0..26_i64 {
        let name = format!("in-{i}");
        write_input(&store, &name, &topic(), 1, 32).await;
        refs.push(ObjectRef::new(key(&name), offset(i), 1));
    }
    // ⚠️ **The object really holds both records**, so the record-count check
    // cannot be what refuses this: with the lower-edge test dropped, the merge
    // succeeds and copies a record below the plan's start.
    write_input(&store, "straddle-low", &topic(), 2, 32).await;
    refs[13] = ObjectRef::new(key("straddle-low"), offset(12), 2);

    let outcome = merge(&store, &planned_from(13, 26), &refs[13..], &mut namer()).await;
    assert!(
        matches!(outcome, Err(Error::IndexObjectMismatch)),
        "an input reaching below the plan's start is refused: {outcome:?}"
    );
}

/// ⚠️ **A missing input is records gone, and nothing else fails.** The output
/// parses, the record counts of every object present agree with the index, and
/// the offsets the absent object held are simply not there.
#[tokio::test]
async fn a_gap_in_the_inputs_is_refused() {
    let store = Counting::new();
    let mut refs = inputs_of(&store, 13, 1, 32).await;
    refs.remove(7);
    let outcome = merge(&store, &planned(13), &refs, &mut namer()).await;
    assert!(
        matches!(outcome, Err(Error::IndexObjectMismatch)),
        "offset 7's record is missing from the inputs: {outcome:?}"
    );
}

/// The mirror: one object named twice writes its records twice.
#[tokio::test]
async fn a_duplicated_input_is_refused() {
    let store = Counting::new();
    let mut refs = inputs_of(&store, 13, 1, 32).await;
    refs.push(refs[3].clone());
    let outcome = merge(&store, &planned(13), &refs, &mut namer()).await;
    assert!(
        matches!(outcome, Err(Error::IndexObjectMismatch)),
        "offset 3's record would be written twice: {outcome:?}"
    );
}

/// Inputs that stop short of the plan's end leave the tail of the range
/// uncovered, which is the same loss as a gap and needs the same refusal.
#[tokio::test]
async fn inputs_that_do_not_reach_the_plan_s_end_are_refused() {
    let store = Counting::new();
    let mut refs = inputs_of(&store, 13, 1, 32).await;
    refs.truncate(11);
    let outcome = merge(&store, &planned(13), &refs, &mut namer()).await;
    assert!(
        matches!(outcome, Err(Error::IndexObjectMismatch)),
        "the last two offsets are uncovered: {outcome:?}"
    );
}

/// ⚠️ **The estimate must predict the run at the largest plan the budget
/// admits**, not only at a size where a hardcoded one-output count happens to
/// be right — which is what round two measured against the eight-object budget
/// this task lowered.
#[tokio::test]
async fn the_estimate_predicts_the_run_at_the_budget_s_largest_plan() {
    let store = Counting::new();
    // ⚠️ **Derived from the budget, not written beside it.** A fixture fixed
    // at today's 524_288 would keep passing when `M5.6` raises the budget, and
    // the estimate-versus-run agreement this test exists for is exactly what a
    // raise breaks while the merge still writes one object.
    let objects = 16_usize;
    let per = u32::try_from(
        COMPACTION_PLAN_RECORDS_BUDGET / i64::try_from(objects).expect("a small count"),
    )
    .expect("a budget that divides");
    let mut refs = Vec::new();
    for i in 0..objects {
        let name = format!("big-{i}");
        write_input(&store, &name, &topic(), per, 8).await;
        refs.push(ObjectRef::new(
            key(&name),
            offset(i64::try_from(i).expect("a small count") * i64::from(per)),
            per,
        ));
    }
    store.gets.store(0, Ordering::Relaxed);

    let end = i64::from(per) * i64::try_from(objects).expect("a small count");
    let planned = planned_records(per, objects);
    let estimate = planned.cost();
    let outcome = merge(&store, &planned, &refs, &mut namer())
        .await
        .expect("a merge that runs");

    assert_eq!(end, estimate.records_rewritten(), "the whole budget");
    assert_eq!(estimate.gets(), outcome.gets());
    assert_eq!(
        estimate.puts(),
        outcome.puts(),
        "one output, and the estimate says so"
    );
    assert_eq!(estimate.records_rewritten(), outcome.records());
}

/// ⚠️ **Every input outside the range is a covering of nothing, and a plan
/// with a range to rewrite must refuse it.** The filter drops each one without
/// a read — which the case above asserts — and what is left is an empty
/// covering that reaches `plan.start()` rather than `plan.end()`.
///
/// ⚠️ **Measured, and it was unguarded before `M5.12`.** Having the empty case
/// fall back to `(plan.start(), plan.end())` instead of `(start, start)`
/// leaves the whole suite green while `merge` writes an object holding nothing
/// for a plan that was costed on thirteen records — a compaction that retires
/// thirteen objects and installs one empty one. Its first round found that.
#[tokio::test]
async fn a_plan_whose_inputs_all_lie_outside_its_range_is_refused() {
    let store = Counting::new();
    let mut refs = Vec::new();
    for i in 0..3_i64 {
        let name = format!("far-{i}");
        write_input(&store, &name, &topic(), 1, 32).await;
        refs.push(ObjectRef::new(key(&name), offset(500 + i), 1));
    }

    // ⚠️ **Which refusal, not that one happened.** With the empty case
    // fabricating `(start, end)` the tiling passes and the merge fails anyway,
    // one step later, because a bundle holding nothing cannot be sealed — so
    // `expect_err` alone is satisfied by the defect. `IndexObjectMismatch` is
    // the tiling's answer; `EmptyBundle` is the writer's.
    let refused = merge(&store, &planned(13), &refs, &mut namer())
        .await
        .expect_err("a covering of nothing does not tile a range of thirteen");
    assert!(
        matches!(refused, Error::IndexObjectMismatch),
        "refused by the tiling, before anything was written: {refused:?}"
    );
}
