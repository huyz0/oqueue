//! The coordinator vocabulary: commit versions, epochs, read modes, and the
//! shape of a metadata-log record.
//!
//! ⚠️ `invariants.rs` makes the case that a property test and a round trip
//! catch different things and that **both are needed** — the property says
//! what cannot be produced, the round trip says an operation returns what it
//! should. Both shapes are here: `advance` is pinned for monotonicity *and*
//! for its sum, with the boundaries pinned by example besides. What each test
//! asserts is a behaviour the rest of `M3` rests on — that a version cannot go
//! backwards, that a read mode's satisfaction rule is the one `M3.10` will
//! route on, and that a log record carries a **delta** rather than a position.

// Every `expect` below is on a value the test itself just constructed from a
// generator constrained to the valid range, so a panic means the generator is
// wrong, not the code under test. Same allowance, same reason, as
// `invariants.rs`.
#![allow(clippy::expect_used)]

use oqueue_core::{
    CommitVersion, CommittedSpan, CoordinatorEpoch, Error, MetadataRecord, ObjectKey, PartitionId,
    ReadMode, TopicId,
};
use proptest::prelude::*;

fn a_topic() -> TopicId {
    TopicId::new("t".to_owned()).expect("literal is a valid topic id")
}

fn a_partition() -> PartitionId {
    PartitionId::new(0).expect("0 is a valid partition")
}

fn an_object() -> ObjectKey {
    ObjectKey::new("o".to_owned()).expect("literal is a valid object key")
}

proptest! {
    /// ⚠️ The load-bearing one, and the reason this type is not a bare `u64`.
    /// `ADR-0020` makes `CommitVersion` the single scalar staleness compares
    /// on, so a silent wrap is the one arithmetic result that breaks ordering
    /// without failing anything: the successor comes back *smaller* than what
    /// it started from and every later comparison is wrong.
    #[test]
    fn a_commit_version_never_advances_backwards(base in any::<u64>(), delta in any::<u64>()) {
        let v = CommitVersion::new(base);
        match v.advance(delta) {
            Ok(next) => {
                prop_assert!(next >= v);
                // ⚠️ Monotonicity alone would pass an `advance` that ignored
                // `delta` entirely, so the sum is pinned too.
                prop_assert_eq!(
                    next.get(),
                    base.checked_add(delta).expect("the Ok arm means no overflow")
                );
            }
            Err(e) => {
                prop_assert!(base.checked_add(delta).is_none());
                prop_assert_eq!(e, Error::CommitVersionOverflow { base, delta });
            }
        }
    }

    /// Ordering on the newtype is the ordering on the number it wraps — what
    /// lets `AtLeast(v)` be a `u64` compare and nothing more.
    #[test]
    fn commit_version_order_follows_the_wrapped_value(a in any::<u64>(), b in any::<u64>()) {
        prop_assert_eq!(CommitVersion::new(a) < CommitVersion::new(b), a < b);
        prop_assert_eq!(CommitVersion::new(a) == CommitVersion::new(b), a == b);
    }

    /// `Stale` is satisfied by any cached version — it is the Fetch path, and
    /// it never forces a round trip.
    #[test]
    fn stale_reads_are_satisfied_by_any_cached_version(cached in any::<u64>()) {
        prop_assert!(ReadMode::Stale.satisfiable_from_cache(CommitVersion::new(cached)));
    }

    /// `AtLeast(v)` is exactly the read-your-writes rule: a cache at or past
    /// the produce response's version answers; a cache behind it must not.
    #[test]
    fn at_least_is_satisfied_only_at_or_past_its_version(
        want in any::<u64>(),
        cached in any::<u64>(),
    ) {
        let mode = ReadMode::AtLeast(CommitVersion::new(want));
        prop_assert_eq!(
            mode.satisfiable_from_cache(CommitVersion::new(cached)),
            cached >= want
        );
    }

    /// ⚠️ `Linearizable` is never satisfiable from cache, however fresh the
    /// cache is. This is hazard H1: `ListOffsets` served from a stale cache
    /// reports a log-end-offset below truth and produces negative consumer
    /// lag, so the mode exists to route to the coordinator unconditionally.
    #[test]
    fn linearizable_reads_are_never_satisfiable_from_cache(cached in any::<u64>()) {
        prop_assert!(!ReadMode::Linearizable.satisfiable_from_cache(CommitVersion::new(cached)));
    }

    /// ⚠️ The delta-shape assertion, and the point of `M3.md` task 2.
    ///
    /// Positions are **derivable** by folding a run of spans over a starting
    /// offset — the fold below is what `M3.8` will implement — and that is
    /// only true because a span carries a *count* rather than a position. The
    /// fold also shows where gap-freeness comes from: each span begins exactly
    /// where the previous one ended, with no allocator having to remember to
    /// make it so. A record carrying its own `base_offset` would make this
    /// fold redundant and the log a statement of state rather than of events.
    #[test]
    fn positions_are_derived_by_folding_span_counts(
        counts in prop::collection::vec(0u32..10_000, 1..32),
    ) {
        let spans: Vec<CommittedSpan> = counts
            .iter()
            .map(|&c| CommittedSpan::new(a_topic(), a_partition(), c))
            .collect();

        let mut bases = Vec::with_capacity(spans.len());
        let mut next = 0u64;
        for span in &spans {
            bases.push(next);
            next += u64::from(span.record_count());
        }

        // Gap-free: every span ends exactly where the next one begins.
        for (i, span) in spans.iter().enumerate() {
            let end = bases[i] + u64::from(span.record_count());
            let following = bases.get(i + 1).copied().unwrap_or(next);
            prop_assert_eq!(end, following);
        }
        prop_assert_eq!(next, counts.iter().map(|&c| u64::from(c)).sum::<u64>());
    }

    /// The count is what distinguishes one span from another, so it is
    /// load-bearing rather than decoration a derived `PartialEq` carries anyway.
    #[test]
    fn spans_differing_only_in_count_are_different_records(a in any::<u32>(), b in any::<u32>()) {
        prop_assume!(a != b);
        prop_assert_ne!(
            CommittedSpan::new(a_topic(), a_partition(), a),
            CommittedSpan::new(a_topic(), a_partition(), b)
        );
    }

    /// An epoch orders like the number it wraps — what lets a reader tell a
    /// failover-rewound log from a current one with a single compare.
    #[test]
    fn coordinator_epoch_order_follows_the_wrapped_value(a in any::<u64>(), b in any::<u64>()) {
        prop_assert_eq!(CoordinatorEpoch::new(a) < CoordinatorEpoch::new(b), a < b);
    }
}

/// ⚠️ The boundary, pinned by example rather than left to a generator.
/// `u64::MAX` is where `checked_add` turns over, and a uniform generator
/// reaches it only by luck.
#[test]
fn advance_pins_its_boundaries() {
    assert_eq!(
        CommitVersion::ZERO
            .advance(0)
            .expect("0 + 0 does not overflow")
            .get(),
        0
    );
    assert_eq!(
        CommitVersion::new(u64::MAX)
            .advance(0)
            .expect("MAX + 0 does not overflow")
            .get(),
        u64::MAX
    );
    assert_eq!(
        CommitVersion::new(u64::MAX).advance(1),
        Err(Error::CommitVersionOverflow {
            base: u64::MAX,
            delta: 1
        })
    );
}

/// A commit record names the object and the spans it added, and nothing that
/// could be derived from applying the log — no offset, no high watermark.
#[test]
fn a_commit_record_carries_spans_and_the_object_it_came_from() {
    let record = MetadataRecord::BatchCommitted {
        object: an_object(),
        spans: vec![CommittedSpan::new(a_topic(), a_partition(), 3)],
    };
    match record {
        MetadataRecord::BatchCommitted { object, spans } => {
            assert_eq!(object, an_object());
            assert_eq!(spans.len(), 1);
            assert_eq!(spans[0].record_count(), 3);
        }
        MetadataRecord::EpochChanged { .. } => panic!("constructed a BatchCommitted"),
    }
}

/// The epoch change is its own event rather than a field on a commit, because
/// it is what tells a reader the log it was following has been rewound by a
/// failover (`M3.md` task 3).
#[test]
fn an_epoch_change_is_its_own_record() {
    let record = MetadataRecord::EpochChanged {
        epoch: CoordinatorEpoch::new(7),
    };
    match record {
        MetadataRecord::EpochChanged { epoch } => assert_eq!(epoch.get(), 7),
        MetadataRecord::BatchCommitted { .. } => panic!("constructed an EpochChanged"),
    }
}

/// ⚠️ Mutation testing found these gaps, and the convention `invariants.rs`
/// set is to kill such a mutant rather than argue it into
/// `baselines/mutants.txt` — that file is empty and the ratio to keep.
///
/// The property tests above assert ordering and satisfaction rules, which is
/// the behaviour. None of them asserts that `Display` renders anything, so
/// `fmt -> Ok(Default::default())` survived on both newtypes. That is not an
/// invariant violation; it is the ordinary correctness an operator reading a
/// log line depends on.
mod kills_surviving_mutants {
    use super::{CommitVersion, CoordinatorEpoch};

    #[test]
    fn display_renders_the_value() {
        assert_eq!(CommitVersion::new(42).to_string(), "42");
        assert_eq!(CoordinatorEpoch::new(7).to_string(), "7");
    }
}
