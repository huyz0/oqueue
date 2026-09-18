//! `allocator.rs`'s own tests, split out at the 500-line limit.
//!
//! ⚠️ **Split rather than trimmed**, on `admission/tests.rs`'s own precedent
//! (itself on `oqueue-codec/src/fetch/tests.rs`'s) — the module boundary is
//! `stage`/`apply`/eviction's own code versus the tests that pin it, not
//! any one test being cuttable. `M11.8`'s eviction test is what pushed this
//! file over the line.

// A panic in a test harness is the test failing, which is what it is for.
#![allow(clippy::expect_used)]

use super::Allocator;
use oqueue_core::{
    ByteRange, CommitVersion, CommittedSpan, Error, ObjectKey, Offset, PartitionId, TopicId,
};

fn topic() -> TopicId {
    TopicId::new("t".to_owned()).expect("a valid topic id")
}

fn partition() -> PartitionId {
    PartitionId::new(0).expect("a valid partition")
}

fn span(records: u32) -> CommittedSpan {
    CommittedSpan::new(topic(), partition(), records, ByteRange::Full, None)
}

fn object() -> ObjectKey {
    ObjectKey::new("o".to_owned()).expect("a valid object key")
}

/// ⚠️ The allocator is reachable through `CoordinatorLoop`, which an
/// operator may format on an error or shutdown path. Its `Debug` must
/// therefore not become a listing of every tenant's topics — the rule
/// `FakeMetadataLog` and `FakeMaterializedIndex` already follow in
/// `oqueue-core`, and it binds harder here because this one is not a fake.
#[test]
fn formatting_the_allocator_summarises_rather_than_lists() {
    let mut allocator = Allocator::new();
    let span = CommittedSpan::new(
        TopicId::new("secret-tenant-topic".to_owned()).expect("a valid topic id"),
        PartitionId::new(0).expect("a valid partition"),
        1,
        ByteRange::Full,
        None,
    );
    let staged = allocator
        .stage(
            ObjectKey::new("o".to_owned()).expect("a valid object key"),
            vec![span],
            oqueue_core::Timestamp::EPOCH,
        )
        .expect("a first commit stages");
    allocator.apply(staged);

    let rendered = format!("{allocator:?}");
    assert!(
        !rendered.contains("secret-tenant-topic"),
        "a Debug line must not name a tenant's topics: {rendered}"
    );
    assert!(
        rendered.contains("next_version: CommitVersion(1)"),
        "and must still say where the line has reached: {rendered}"
    );
    assert!(
        rendered.contains("partitions: 1"),
        "and how much it is holding: {rendered}"
    );
}

/// ⚠️ **A partition's line refuses rather than wrapping.** An offset is an
/// `i64` on the wire, and a wrapped one is *smaller* than the one before
/// it — every later comparison, every watermark, every consumer's position
/// is then wrong, and nothing tells anybody. `Offset::add` is what refuses;
/// this is what reaches it, which two-billion produce requests otherwise
/// would.
#[test]
fn a_partition_at_the_end_of_its_line_refuses_rather_than_wrapping() {
    let allocator = Allocator::seeded(
        CommitVersion::ZERO,
        &[(
            topic(),
            partition(),
            Offset::new(i64::MAX - 1).expect("in range"),
        )],
    );

    let refused = allocator.stage(object(), vec![span(2)], oqueue_core::Timestamp::EPOCH);

    assert!(
        matches!(refused, Err(Error::OffsetOverflow { .. })),
        "a line one record from its end must refuse two: {refused:?}"
    );
    // ⚠️ **And the one that fits still fits**, which is what says this is a
    // ceiling rather than an off-by-one: a guard one too eager refuses the
    // last legal record of every partition that ever reaches here.
    allocator
        .stage(object(), vec![span(1)], oqueue_core::Timestamp::EPOCH)
        .expect("the last record on the line is still assignable");
}

/// ⚠️ **And the shard's version line does the same.** A wrapped
/// `CommitVersion` is a version *below* one already folded, which
/// `MaterializedIndex` guarantee 1 refuses as non-monotonic — so the shard
/// stops committing with a message about ordering rather than about the
/// counter that ran out.
#[test]
fn a_version_line_at_its_end_refuses_rather_than_wrapping() {
    let allocator = Allocator::seeded(CommitVersion::new(u64::MAX), &[]);

    let refused = allocator.stage(object(), vec![span(1)], oqueue_core::Timestamp::EPOCH);

    assert!(
        matches!(refused, Err(Error::CommitVersionOverflow { .. })),
        "the last version on the line cannot be advanced past: {refused:?}"
    );
    // ⚠️ **And the last legal version still stages**, the same companion
    // the offset test carries and for the same reason: a guard one step
    // too eager costs a shard its final commit, and a test that only ever
    // probes the value past the end cannot tell the two apart.
    Allocator::seeded(CommitVersion::new(u64::MAX - 1), &[])
        .stage(object(), vec![span(1)], oqueue_core::Timestamp::EPOCH)
        .expect("the last version on the line is still assignable");
}

/// ⚠️ **Nothing is consumed by a refusal**, which is what makes both
/// guards safe to hit: `stage` computes without taking, so a caller that
/// retries a smaller batch gets the offsets it would have got anyway.
#[test]
fn a_refused_stage_consumes_nothing() {
    let allocator = Allocator::seeded(
        CommitVersion::new(7),
        &[(
            topic(),
            partition(),
            Offset::new(i64::MAX - 1).expect("in range"),
        )],
    );

    assert!(
        allocator
            .stage(object(), vec![span(2)], oqueue_core::Timestamp::EPOCH)
            .is_err()
    );

    let staged = allocator
        .stage(object(), vec![span(1)], oqueue_core::Timestamp::EPOCH)
        .expect("a batch that fits still fits");
    assert_eq!(staged.entry.version(), CommitVersion::new(7));
}

/// ⚠️ **`M11.8`, driven against a small `cap` rather than the real
/// `expiry::MAX_TRACKED_PRODUCERS`** (100,000) — reaching that through
/// real commits would make this test a stopwatch exercise rather than a
/// property one, `sequence_wraps_from_i32_max_back_to_zero`'s own
/// precedent for the same reasoning one boundary over.
#[test]
fn evict_over_cap_removes_the_least_recently_touched_entries_first() {
    use super::{ProducerState, evict_over_cap, expiry};
    use oqueue_core::{ProducerEpoch, ProducerId};

    fn key(id: i64) -> (ProducerId, TopicId, PartitionId) {
        (
            ProducerId::new(id).expect("a valid producer id"),
            topic(),
            partition(),
        )
    }
    fn state() -> ProducerState {
        ProducerState::provisional(ProducerEpoch::ZERO, 0, 1)
    }

    let mut producer_state = std::collections::HashMap::new();
    let mut recency = expiry::Recency::default();
    for id in 1..=3 {
        producer_state.insert(key(id), state());
        recency.touch(key(id));
    }

    evict_over_cap(&mut producer_state, &mut recency, 3);
    assert_eq!(
        producer_state.len(),
        3,
        "at the cap, nothing is evicted yet"
    );

    evict_over_cap(&mut producer_state, &mut recency, 2);
    assert_eq!(producer_state.len(), 2);
    assert!(
        !producer_state.contains_key(&key(1)),
        "producer 1 was touched least recently"
    );
    assert!(producer_state.contains_key(&key(2)));
    assert!(producer_state.contains_key(&key(3)));

    evict_over_cap(&mut producer_state, &mut recency, 0);
    assert!(producer_state.is_empty());
}
