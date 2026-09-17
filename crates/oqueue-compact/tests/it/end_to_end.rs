//! The write path with both halves joined: a sweep's round, merged.
//!
//! ⚠️ **The test whose absence let `M5.46` through.** The trigger half
//! (`read_amp`, `plan`, `sweep`) and the merge half (`merge`, `merge_round`)
//! were each covered and had never met: the sweep produced plans nothing
//! derived inputs for, over ranges `merge`'s `tiling()` refuses whenever an
//! object hangs over an edge of the window. Every test here starts at a
//! candidate and ends at bytes in the store.

// A panic in a test harness is the test failing, which is what it is for.
#![allow(clippy::expect_used)]

use oqueue_compact::{Candidate, PlannedInputs, Planning, merge_round, plan, sweep};
use oqueue_core::{
    BundleBuilder, ByteRange, CommitVersion, CommittedSpan, FakeMaterializedIndex,
    MaterializedIndex, MetadataEntry, MetadataRecord, ObjectStore, PushedRecords,
    TAIL_WINDOW_ENTRIES, parse_footer,
};

use crate::support::{Counting, key, offset, partition, topic};

/// Writes `objects + TAIL_WINDOW_ENTRIES` objects of `per` records each into
/// the store, and folds the matching entries into an index.
///
/// ⚠️ **One set of objects, named once.** The store and the index describing
/// it are the same thing seen twice, which is what makes a plan the index
/// produced runnable against the store — a fixture that builds them
/// separately proves the halves agree only about counts.
async fn partition_of(store: &Counting, per: u32, objects: usize) -> FakeMaterializedIndex {
    partition_of_spans(store, per, objects, 1).await
}

/// As [`partition_of`], with `spans` disjoint spans of the partition per
/// object.
///
/// ⚠️ **Two spans in one object is `M3.8`'s ordinary case**, not a corner:
/// `IndexState`'s fold admits a partition appearing twice in one batch, and a
/// round that named such an object twice tiled its range perfectly and then
/// failed the record-count check with every other partition's work in it.
async fn partition_of_spans(
    store: &Counting,
    per: u32,
    objects: usize,
    spans: usize,
) -> FakeMaterializedIndex {
    let index = FakeMaterializedIndex::new();
    let mut entries = Vec::new();
    // ⚠️ **Divided by the spans**, because the window is entries deep and an
    // object of two spans fills two of them: a fixture that always wrote
    // `TAIL_WINDOW_ENTRIES` objects of tail would leave half of what it meant
    // as history inside the window, and the plan would cover objects the test
    // thought it had excluded.
    for which in 0..(objects + TAIL_WINDOW_ENTRIES / spans) {
        let name = format!("obj-{which}");
        let mut builder = BundleBuilder::new();
        for _ in 0..spans {
            builder
                .push(
                    topic(),
                    partition(),
                    PushedRecords {
                        count: per,
                        producer: None,
                    },
                    &[b'r'; 32],
                )
                .expect("a valid region");
        }
        let sealed = builder.seal().expect("a sealed bundle");
        store
            .inner
            .put(&key(&name), sealed.into_payload(), None)
            .await
            .expect("a store that accepts");
        entries.push(MetadataEntry::new(
            CommitVersion::new(which as u64 + 1),
            MetadataRecord::BatchCommitted {
                object: key(&name),
                spans: (0..spans)
                    .map(|which| {
                        CommittedSpan::new(
                            topic(),
                            partition(),
                            per,
                            ByteRange::bounded(which as u64 * 32, 32).expect("a valid range"),
                            None,
                        )
                    })
                    .collect(),
            },
        ));
    }
    index.apply(&entries).expect("a valid fold");
    index
}

/// ⚠️ **Candidate to bytes, with nothing supplied in between.** The sweep
/// picks the range, derives the inputs and the round runs — the sequence
/// `FR-34` describes and the one nothing exercised before `M5.46`.
#[tokio::test]
async fn a_sweep_s_round_merges() {
    let store = Counting::new();
    // ⚠️ **More objects than `MAX_BATCHES_PER_PAGE`**, so both walks page:
    // the measurement's and the derivation's. At one page each, a derivation
    // that stopped after the first page would still tile a shorter plan and
    // nothing would say so.
    let objects = 70_usize;
    let per = 7_u32;
    let index = partition_of(&store, per, objects).await;

    let swept = sweep(&index, &[Candidate::new(topic(), partition(), offset(0))])
        .expect("an index that answers");
    assert_eq!(swept.round().len(), 1, "one amplified partition");
    let planned = &swept.round()[0];
    let records = i64::from(per) * i64::try_from(objects).expect("a small count");
    assert_eq!(planned.plan().start(), offset(0));
    assert_eq!(
        planned.plan().end(),
        offset(records),
        "the history portion, and it ends on an object boundary"
    );
    assert_eq!(
        planned.inputs().len(),
        objects,
        "one input per history object"
    );

    let outcome = merge_round(&store, swept.round(), &key("out"))
        .await
        .expect("a round the sweep produced is a round that runs");
    assert_eq!(outcome.puts(), 1, "one object for the round");
    assert_eq!(outcome.gets(), objects, "one read per input");
    assert_eq!(outcome.records(), records, "every record, once");

    let written = store
        .inner
        .get(&key("out"), ByteRange::Full)
        .await
        .expect("an output object");
    let regions = parse_footer(&written, written.len() as u64).expect("a valid footer");
    let held: u32 = regions.iter().map(oqueue_core::Region::record_count).sum();
    assert_eq!(
        i64::from(held),
        records,
        "and the object holds them, not just the outcome"
    );
}

/// ⚠️ **Whatever the objects measure.** A sweep's window is offset arithmetic
/// and an object's extent is not, so a plan's range must land on object
/// boundaries by construction rather than because the fixture's objects hold
/// one record each — which is what every earlier test's objects did.
#[tokio::test]
async fn a_sweep_over_objects_of_any_size_produces_a_round_that_merges() {
    for per in [1_u32, 2, 3, 7, 10, 64] {
        let store = Counting::new();
        let objects = 20_usize;
        let index = partition_of(&store, per, objects).await;

        let swept = sweep(&index, &[Candidate::new(topic(), partition(), offset(0))])
            .expect("an index that answers");
        assert_eq!(swept.round().len(), 1, "amplified at every object size");
        let outcome = merge_round(&store, swept.round(), &key("out"))
            .await
            .unwrap_or_else(|error| {
                panic!("a round that runs at {per} records per object: {error}")
            });
        assert_eq!(
            outcome.records(),
            i64::from(per) * i64::try_from(objects).expect("a small count"),
        );
    }
}

/// ⚠️ **A cursor that stopped inside an object**, which is the case the
/// alignment is for: `M5.44` gives the cursor a home and nothing promises it
/// lands on a boundary. The plan must start at the next boundary rather than
/// at the cursor, because an object reaching below the plan's start is one
/// `merge` refuses — and refusing is the good outcome; copying it whole would
/// serve records twice once the inputs retire.
#[tokio::test]
async fn a_cursor_inside_an_object_plans_from_the_next_boundary() {
    let store = Counting::new();
    let (per, objects) = (7_u32, 20_usize);
    let index = partition_of(&store, per, objects).await;

    let swept = sweep(&index, &[Candidate::new(topic(), partition(), offset(3))])
        .expect("an index that answers");
    let planned = &swept.round()[0];
    assert_eq!(
        planned.plan().start(),
        offset(i64::from(per)),
        "the object holding offset 3 is not a whole object of this plan's"
    );
    assert_eq!(planned.inputs().len(), objects - 1);

    let outcome = merge_round(&store, swept.round(), &key("out"))
        .await
        .expect("a round that runs");
    assert_eq!(
        outcome.records(),
        i64::from(per) * i64::try_from(objects - 1).expect("a small count"),
    );
}

/// ⚠️ **One input per object, however many spans of the partition it holds.**
/// `merge` reads an object once and takes every region it holds for the
/// partition, so a round naming that object twice tiles the range perfectly
/// and then fails the record-count check — taking every other partition's
/// work in the round down with it.
#[tokio::test]
async fn an_object_holding_two_spans_is_one_input() {
    let store = Counting::new();
    let (per, objects) = (5_u32, 40_usize);
    let index = partition_of_spans(&store, per, objects, 2).await;

    let swept = sweep(&index, &[Candidate::new(topic(), partition(), offset(0))])
        .expect("an index that answers");
    let planned = &swept.round()[0];
    assert_eq!(
        planned.inputs().len(),
        objects,
        "one per object, not one per span"
    );

    let outcome = merge_round(&store, swept.round(), &key("out"))
        .await
        .expect("a round that runs");
    assert_eq!(outcome.gets(), objects, "and one read per object");
    assert_eq!(
        outcome.records(),
        2 * i64::from(per) * i64::try_from(objects).expect("a small count"),
        "both spans of every object"
    );
}

/// ⚠️ **The product of the other two cases, which is where `M5.46` round two
/// found the defect.** A cursor inside an object *and* an object holding two
/// spans: the range edge falls between the spans, and a rule applied per span
/// keeps the second and drops the first — a ref carrying five records into a
/// GET that returns ten, which `take_regions` refuses with every other
/// partition's work in the round.
#[tokio::test]
async fn a_cursor_between_two_spans_of_one_object_drops_that_object_whole() {
    let store = Counting::new();
    let (per, objects) = (5_u32, 40_usize);
    let index = partition_of_spans(&store, per, objects, 2).await;

    // obj-0 holds 0..5 and 5..10, so offset 5 is between its two spans.
    let swept = sweep(
        &index,
        &[Candidate::new(topic(), partition(), offset(i64::from(per)))],
    )
    .expect("an index that answers");
    let planned = &swept.round()[0];
    assert_eq!(
        planned.plan().start(),
        offset(2 * i64::from(per)),
        "the plan starts at obj-1, not at obj-0's second span"
    );
    assert_eq!(planned.inputs().len(), objects - 1, "obj-0 is not an input");

    let outcome = merge_round(&store, swept.round(), &key("out"))
        .await
        .expect("a round that runs");
    assert_eq!(
        outcome.records(),
        2 * i64::from(per) * i64::try_from(objects - 1).expect("a small count"),
        "both spans of every object that is in, and none of the one that is out"
    );
}

/// The same defect at the far edge: a window end between two spans.
///
/// ⚠️ **Reached through `plan` rather than `sweep`**, because a sweep's window
/// ends at the partition's end and never between two spans. `M5.44`'s cursor
/// and any caller that picks its own range both can.
#[tokio::test]
async fn a_range_ending_between_two_spans_drops_that_object_whole() {
    let store = Counting::new();
    let (per, objects) = (5_u32, 40_usize);
    let index = partition_of_spans(&store, per, objects, 2).await;

    // obj-19 holds 190..195 and 195..200, so 195 is between its two spans.
    let Planning::Planned(planned) =
        plan(&index, &topic(), partition(), offset(0), offset(195)).expect("an index that answers")
    else {
        panic!("a plan over the amplified history")
    };
    assert_eq!(
        planned.end(),
        offset(190),
        "the plan ends where obj-19 begins, not inside it"
    );

    let derived = PlannedInputs::derive(&index, planned).expect("an index that answers");
    assert_eq!(derived.inputs().len(), 19);
    let outcome = merge_round(&store, &[derived], &key("out"))
        .await
        .expect("a round that runs");
    assert_eq!(
        outcome.records(),
        190,
        "every record below the object the range would have split"
    );
}
