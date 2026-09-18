//! The check before the swap: what is installed covers what is retired.
//!
//! ⚠️ **`M5.12`, and `M5.md` task 9 calls it cheap insurance on the one
//! operation that can silently lose data.** Every case here is a set of
//! references that is *almost* right — one range missing, one range doubled,
//! one object reaching past the end — because a set that is obviously wrong is
//! not the one that ships.

// A panic in a test harness is the test failing, which is what it is for.
#![allow(clippy::expect_used)]

use oqueue_core::{Error, ObjectKey, ObjectRef, Offset, contiguous_span, covers};

fn reference(name: &str, base: i64, records: u32) -> ObjectRef {
    ObjectRef::new(
        ObjectKey::new(name).expect("a valid key"),
        Offset::new(base).expect("a valid offset"),
        records,
    )
}

fn offset(value: i64) -> Offset {
    Offset::new(value).expect("a valid offset")
}

/// The ordinary swap: four inputs become one output over the same range.
#[test]
fn one_output_covering_every_input_is_admitted() {
    let inputs = [
        reference("in-0", 0, 10),
        reference("in-1", 10, 10),
        reference("in-2", 20, 10),
        reference("in-3", 30, 10),
    ];
    let outputs = [reference("out", 0, 40)];
    covers(
        &inputs.iter().collect::<Vec<_>>(),
        &outputs.iter().collect::<Vec<_>>(),
    )
    .expect("the outputs cover the inputs exactly");
}

/// ⚠️ **The row's acceptance, in its own words**: an output set missing one
/// offset range is refused, not warned about. The outputs here are valid
/// objects, contiguous among themselves, and simply stop ten records early —
/// which is the shape that produces an index nothing can tell is wrong.
#[test]
fn an_output_set_missing_one_range_is_refused() {
    let inputs = [
        reference("in-0", 0, 10),
        reference("in-1", 10, 10),
        reference("in-2", 20, 10),
    ];
    let outputs = [reference("out", 0, 20)];
    assert!(matches!(
        covers(
            &inputs.iter().collect::<Vec<_>>(),
            &outputs.iter().collect::<Vec<_>>()
        ),
        Err(Error::IndexObjectMismatch)
    ));
}

/// ⚠️ **And missing a range at the *start*, which is the endpoint a check on
/// the far end alone leaves unconstrained.** Measured: with the rule weakened
/// to compare only the high offsets, every other case here stays green and
/// this one reds — `M5.12`'s first round found the suite in exactly that
/// state, and cargo-mutants cannot reach it because its operators replace the
/// whole comparison rather than one half of it.
#[test]
fn an_output_set_missing_the_first_range_is_refused() {
    let inputs = [reference("in-0", 0, 30)];
    let outputs = [reference("out", 10, 20)];
    assert!(matches!(
        covers(
            &inputs.iter().collect::<Vec<_>>(),
            &outputs.iter().collect::<Vec<_>>()
        ),
        Err(Error::IndexObjectMismatch)
    ));
}

/// ⚠️ **And missing a range in the *middle*, which the end-to-end reach test
/// cannot see.** Outputs starting and ending exactly where the inputs do, with
/// a hole between them, is the case a check on the endpoints alone admits.
#[test]
fn an_output_set_with_a_hole_in_the_middle_is_refused() {
    let inputs = [reference("in-0", 0, 30)];
    let outputs = [reference("out-0", 0, 10), reference("out-1", 20, 10)];
    assert!(matches!(
        covers(
            &inputs.iter().collect::<Vec<_>>(),
            &outputs.iter().collect::<Vec<_>>()
        ),
        Err(Error::IndexObjectMismatch)
    ));
}

/// ⚠️ **Covering *more* is refused too.** Those offsets are either records
/// another reference still names — served twice once this lands — or records
/// nothing else names, which compaction was not authorised to produce.
#[test]
fn outputs_reaching_past_the_inputs_are_refused() {
    let inputs = [reference("in-0", 0, 20)];
    let outputs = [reference("out", 0, 30)];
    assert!(matches!(
        covers(
            &inputs.iter().collect::<Vec<_>>(),
            &outputs.iter().collect::<Vec<_>>()
        ),
        Err(Error::IndexObjectMismatch)
    ));
}

/// An overlap among the outputs is records held twice, which no reader can
/// tell from a partition that genuinely holds them twice.
#[test]
fn outputs_that_overlap_each_other_are_refused() {
    let inputs = [reference("in-0", 0, 30)];
    let outputs = [reference("out-0", 0, 20), reference("out-1", 10, 20)];
    assert!(matches!(
        covers(
            &inputs.iter().collect::<Vec<_>>(),
            &outputs.iter().collect::<Vec<_>>()
        ),
        Err(Error::IndexObjectMismatch)
    ));
}

/// ⚠️ **Installing while retiring nothing is refused**, because that is the
/// write path adding records through the compaction seam, where none of the
/// write path's checks are.
#[test]
fn installing_without_retiring_is_refused() {
    let outputs = [reference("out", 0, 10)];
    assert!(matches!(
        covers(&[], &outputs.iter().collect::<Vec<_>>()),
        Err(Error::IndexObjectMismatch)
    ));
    assert!(matches!(covers(&[], &[]), Err(Error::IndexObjectMismatch)));
}

/// ⚠️ **Order is the caller's convenience, not its obligation.** A sweep hands
/// over whatever order its index walk produced, and a check that quietly
/// required ascending order would pass or fail on that rather than on the
/// ranges.
#[test]
fn the_order_the_references_arrive_in_does_not_decide_it() {
    let inputs = [reference("in-1", 10, 10), reference("in-0", 0, 10)];
    let outputs = [reference("out-1", 10, 10), reference("out-0", 0, 10)];
    covers(
        &inputs.iter().collect::<Vec<_>>(),
        &outputs.iter().collect::<Vec<_>>(),
    )
    .expect("the same ranges, whatever order they arrive in");
}

/// The span itself, which `merge`'s own tiling check reads too — one rule, so
/// the input side and the output side cannot drift into different notions of
/// what a gap is.
#[test]
fn a_span_is_none_for_nothing_and_the_whole_run_otherwise() {
    assert_eq!(contiguous_span(&[]).expect("no references"), None);
    let refs = [reference("a", 5, 10), reference("b", 15, 5)];
    assert_eq!(
        contiguous_span(&refs.iter().collect::<Vec<_>>()).expect("contiguous"),
        Some((offset(5), offset(20)))
    );
    let gapped = [reference("a", 5, 10), reference("b", 16, 5)];
    assert!(matches!(
        contiguous_span(&gapped.iter().collect::<Vec<_>>()),
        Err(Error::IndexObjectMismatch)
    ));
}

/// ⚠️ **A reference covering no offsets is refused, and it is neither a gap
/// nor an overlap** — which is why a contiguity test alone admits it. The set
/// below spans exactly what it claims to; the empty reference simply rides
/// along.
///
/// ⚠️ **What it costs is an acknowledged record**, measured by `M5.13`'s third
/// round: installed into a partition's history, the empty reference sorts in
/// ahead of the real object at the same base offset, and a fetch resuming
/// there lands on the empty one, finds its end is not past the start, and
/// resumes at the *next* object. Silent, and only from the offset a consumer
/// actually resumes at.
#[test]
fn a_reference_covering_no_offsets_is_refused() {
    let inputs = [reference("in-0", 0, 2)];
    let outputs = [reference("merged", 0, 2), reference("empty", 2, 0)];
    assert!(matches!(
        covers(
            &inputs.iter().collect::<Vec<_>>(),
            &outputs.iter().collect::<Vec<_>>()
        ),
        Err(Error::IndexObjectMismatch)
    ));

    // ⚠️ **In the middle of a run too**, where it is not even at an end of the
    // span and no endpoint comparison can see it.
    let middle = [
        reference("a", 0, 1),
        reference("empty", 1, 0),
        reference("b", 1, 1),
    ];
    assert!(matches!(
        contiguous_span(&middle.iter().collect::<Vec<_>>()),
        Err(Error::IndexObjectMismatch)
    ));

    // And on the retiring side, which reaches the same rule.
    let empty_input = [reference("empty", 0, 0)];
    let empty_output = [reference("out", 0, 0)];
    assert!(matches!(
        covers(
            &empty_input.iter().collect::<Vec<_>>(),
            &empty_output.iter().collect::<Vec<_>>()
        ),
        Err(Error::IndexObjectMismatch)
    ));
}
