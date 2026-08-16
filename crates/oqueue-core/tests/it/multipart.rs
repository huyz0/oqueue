//! Multipart bounds: each of the four is enforced at its own boundary, one
//! under, one at, one over.

// The workspace denies `expect_used`; these sites are on values this test
// just constructed from literals it controls, so a panic means the test is
// wrong.
#![allow(clippy::expect_used)]

use oqueue_core::{Error, MultipartLimits, MultipartSession};

/// S3's own documented multipart bounds (doc 04 §5) — used here as
/// realistic test data, not as a constant this crate exports. `oqueue-store`
/// (`M1.16`) constructs its own when it actually talks to S3.
const S3_SHAPED_LIMITS: MultipartLimits = MultipartLimits {
    min_part_size: 5 * 1024 * 1024,        // 5 MiB
    max_part_size: 5 * 1024 * 1024 * 1024, // 5 GiB
    max_parts: 10_000,
    max_object_size: 5 * 1024 * 1024 * 1024 * 1024, // 5 TiB
};

/// Small bounds, so a test can actually construct "one over" without
/// allocating gigabytes.
const TINY_LIMITS: MultipartLimits = MultipartLimits {
    min_part_size: 10,
    max_part_size: 100,
    max_parts: 3,
    max_object_size: 250,
};

/// A part at exactly the minimum, as the non-last part, is accepted.
#[test]
fn a_part_at_the_minimum_size_is_accepted() {
    let mut session = MultipartSession::new(TINY_LIMITS);
    session.add_part(10).expect("at the minimum");
    session.add_part(1).expect("a smaller last part");
    let parts = session.finish().expect("finish succeeds");
    assert_eq!(parts, vec![10, 1]);
}

/// A non-last part **under** the minimum is rejected — only at `finish`,
/// since nothing knows it was not the last part until then.
#[test]
fn a_non_last_part_under_the_minimum_is_rejected_at_finish() {
    let mut session = MultipartSession::new(TINY_LIMITS);
    session
        .add_part(9)
        .expect("add_part alone does not know this is too small yet");
    session
        .add_part(5)
        .expect("a second part, so the first is provably not last");

    assert_eq!(
        session.finish(),
        Err(Error::PartTooSmall { bytes: 9, min: 10 })
    );
}

/// A single part under the minimum, as the upload's only (and therefore
/// last) part, is accepted — the minimum never applies to it.
#[test]
fn a_single_undersized_part_is_accepted_as_the_only_part() {
    let mut session = MultipartSession::new(TINY_LIMITS);
    session
        .add_part(1)
        .expect("the only part, exempt from the minimum");
    assert_eq!(session.finish(), Ok(vec![1]));
}

/// A part at exactly the maximum is accepted.
#[test]
fn a_part_at_the_maximum_size_is_accepted() {
    let mut session = MultipartSession::new(TINY_LIMITS);
    session.add_part(100).expect("at the maximum");
}

/// A part **over** the maximum is rejected immediately, at `add_part`.
#[test]
fn a_part_over_the_maximum_size_is_rejected_immediately() {
    let mut session = MultipartSession::new(TINY_LIMITS);
    assert_eq!(
        session.add_part(101),
        Err(Error::PartTooLarge {
            bytes: 101,
            max: 100
        })
    );
}

/// Exactly `max_parts` parts are accepted.
#[test]
fn exactly_the_maximum_part_count_is_accepted() {
    let mut session = MultipartSession::new(TINY_LIMITS);
    for _ in 0..TINY_LIMITS.max_parts {
        session.add_part(10).expect("within the part-count limit");
    }
}

/// One more than `max_parts` is rejected immediately.
#[test]
fn one_more_than_the_maximum_part_count_is_rejected_immediately() {
    let mut session = MultipartSession::new(TINY_LIMITS);
    for _ in 0..TINY_LIMITS.max_parts {
        session.add_part(10).expect("within the part-count limit");
    }
    assert_eq!(
        session.add_part(10),
        Err(Error::TooManyParts {
            max: TINY_LIMITS.max_parts
        })
    );
}

/// A running total at exactly `max_object_size` is accepted.
#[test]
fn a_total_at_the_maximum_object_size_is_accepted() {
    let mut session = MultipartSession::new(TINY_LIMITS);
    session.add_part(100).expect("part 1");
    session.add_part(100).expect("part 2");
    session
        .add_part(50)
        .expect("part 3, total exactly at the maximum");
}

/// A running total **over** `max_object_size` is rejected immediately, on
/// the part that would push it over — not retroactively at `finish`.
#[test]
fn a_total_over_the_maximum_object_size_is_rejected_immediately() {
    let mut session = MultipartSession::new(TINY_LIMITS);
    session.add_part(100).expect("part 1");
    session.add_part(100).expect("part 2");
    assert_eq!(
        session.add_part(51),
        Err(Error::ObjectTooLarge { max: 250 })
    );
}

/// Part numbers are 1-based and assigned in the order parts were added.
#[test]
fn part_numbers_are_one_based_and_sequential() {
    let mut session = MultipartSession::new(TINY_LIMITS);
    assert_eq!(session.add_part(10), Ok(1));
    assert_eq!(session.add_part(10), Ok(2));
    assert_eq!(session.add_part(10), Ok(3));
}

/// An empty session — no parts ever added — finishes with nothing to
/// violate, rather than erroring.
#[test]
fn an_empty_session_finishes_successfully() {
    let session = MultipartSession::new(TINY_LIMITS);
    assert_eq!(session.finish(), Ok(Vec::new()));
}

/// Realistic exercise against S3's own documented shape: a streaming writer
/// of unknown final size accumulating several full-size parts and one small
/// trailing part, exactly the pattern doc 04 §5 names multipart for.
#[test]
fn s3_shaped_limits_accept_a_realistic_streaming_upload() {
    let mut session = MultipartSession::new(S3_SHAPED_LIMITS);
    session
        .add_part(S3_SHAPED_LIMITS.min_part_size)
        .expect("a full-size part");
    session
        .add_part(S3_SHAPED_LIMITS.min_part_size)
        .expect("another full-size part");
    session.add_part(1234).expect("a small trailing part");

    assert_eq!(
        session.finish(),
        Ok(vec![
            S3_SHAPED_LIMITS.min_part_size,
            S3_SHAPED_LIMITS.min_part_size,
            1234,
        ])
    );
}
