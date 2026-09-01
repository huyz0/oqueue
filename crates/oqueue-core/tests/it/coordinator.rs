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
    ByteRange, CacheState, CommitVersion, CommittedSpan, CoordinatorEpoch, Error,
    MAX_METADATA_STALENESS_MS, MetadataRecord, ObjectKey, PartitionId, ReadMode, RefreshReason,
    SessionWatermark, TopicId,
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

    /// `Stale` is admitted by any cached version — it is the Fetch path, and
    /// the version question never forces a round trip on it. ⚠️ The silence
    /// breaker still can, which is why `silent_for_ms` is `0` here and has a
    /// case of its own below.
    #[test]
    fn stale_reads_are_admitted_at_any_cached_version(cached in any::<u64>()) {
        let cache = CacheState::new(
            CoordinatorEpoch::ZERO,
            Some(CommitVersion::new(cached)),
            0,
        );
        prop_assert_eq!(cache.admits(ReadMode::Stale, CoordinatorEpoch::ZERO), Ok(()));
    }

    /// `AtLeast(v)` is exactly the read-your-writes rule: a cache at or past
    /// the produce response's version answers; a cache behind it must not.
    #[test]
    fn at_least_is_admitted_only_at_or_past_its_version(
        want in any::<u64>(),
        cached in any::<u64>(),
    ) {
        let epoch = CoordinatorEpoch::ZERO;
        let mode = ReadMode::AtLeast(SessionWatermark::new(epoch, CommitVersion::new(want)));
        let cache = CacheState::new(epoch, Some(CommitVersion::new(cached)), 0);
        prop_assert_eq!(
            cache.admits(mode, epoch),
            if cached >= want {
                Ok(())
            } else {
                Err(RefreshReason::NotYetVisible { wanted: want, applied: Some(cached) })
            }
        );
    }

    /// ⚠️ **A cache that has folded *nothing* admits nothing**, and until
    /// `M3.30` no test built one. `applied` is an `Option` precisely because a
    /// cache exists before its first delta arrives, and `is_some_and` on
    /// `None` is `false` — but so is every other reading of a value nobody
    /// constructs, so flipping it to `is_none_or` left the suite green. What
    /// that flip buys a client is the exact failure `AtLeast` exists to
    /// prevent: a producer's own consumer, reading back at the version its ack
    /// carried, served an empty partition by a cache holding nothing at all.
    #[test]
    fn a_cache_that_has_folded_nothing_admits_no_read_your_writes(
        want in any::<u64>(),
    ) {
        let epoch = CoordinatorEpoch::ZERO;
        let mode = ReadMode::AtLeast(SessionWatermark::new(epoch, CommitVersion::new(want)));
        let cache = CacheState::new(epoch, None, 0);
        prop_assert_eq!(
            cache.admits(mode, epoch),
            Err(RefreshReason::NotYetVisible { wanted: want, applied: None })
        );
    }

    /// ⚠️ **A watermark from a *newer* epoch says the cache is the stale one**,
    /// and answering it as "the cache is current and correct" is exactly
    /// backwards. It is the one piece of evidence a reader-side cache has that
    /// its own coordinator is the departed one: the client was served by an
    /// incarnation this side has not heard from, so the remediation is to go
    /// back to the coordinator rather than to tell the client its watermark is
    /// incomparable.
    ///
    /// ⚠️ **Both directions in one property**, because an inequality passes a
    /// test that only ever looks at one of them — which is how the direction
    /// went unnoticed.
    #[test]
    fn the_direction_of_a_watermarks_epoch_decides_which_side_is_stale(
        cache_epoch in 1_u64..=u64::MAX - 1,
        version in any::<u64>(),
    ) {
        let epoch = CoordinatorEpoch::new(cache_epoch);
        let cache = CacheState::new(epoch, Some(CommitVersion::new(version)), 0);

        let newer = SessionWatermark::new(
            CoordinatorEpoch::new(cache_epoch + 1),
            CommitVersion::new(version),
        );
        prop_assert_eq!(
            cache.admits(ReadMode::AtLeast(newer), epoch),
            Err(RefreshReason::CacheFromAnOlderEpoch {
                watermark: cache_epoch + 1,
                cache: cache_epoch,
            }),
            "a newer watermark means this cache's coordinator departed"
        );

        let older = SessionWatermark::new(
            CoordinatorEpoch::new(cache_epoch - 1),
            CommitVersion::new(version),
        );
        prop_assert_eq!(
            cache.admits(ReadMode::AtLeast(older), epoch),
            Err(RefreshReason::WatermarkFromAnotherEpoch {
                watermark: cache_epoch - 1,
                cache: cache_epoch,
            }),
            "an older watermark is the incomparable one"
        );
    }

    /// ⚠️ **The newer-epoch discard outranks the silence breaker** (`M10.19`,
    /// from `M3.43`). Before this fix a cache silent past the limit answered
    /// `PushStreamSilent` — "go refresh this cache" — to a caller presenting
    /// a watermark from a newer epoch, which is exactly the one piece of
    /// evidence this cache's own coordinator has departed and it should be
    /// discarded outright, not refreshed in place. Both conditions are pure
    /// functions of `CacheState`'s three fields, so the property holds
    /// without a follower to make the silence real — see `M3.43`'s own note
    /// that this ordering is unreachable today because `Cluster::cache_state`
    /// hardcodes `silent_for_ms` to zero, which this test does not: it builds
    /// `CacheState::new` directly, the only way to exercise the ordering
    /// before `M7`'s follower exists.
    #[test]
    fn a_newer_epoch_outranks_silence_even_past_the_staleness_limit(
        cache_epoch in 0_u64..u64::MAX,
        version in any::<u64>(),
        silent in (MAX_METADATA_STALENESS_MS + 1)..=u64::MAX,
    ) {
        let epoch = CoordinatorEpoch::new(cache_epoch);
        let cache = CacheState::new(epoch, Some(CommitVersion::new(version)), silent);
        let newer = SessionWatermark::new(
            CoordinatorEpoch::new(cache_epoch + 1),
            CommitVersion::new(version),
        );
        prop_assert_eq!(
            cache.admits(ReadMode::AtLeast(newer), epoch),
            Err(RefreshReason::CacheFromAnOlderEpoch {
                watermark: cache_epoch + 1,
                cache: cache_epoch,
            }),
            "a departed coordinator must be discarded, not merely refreshed"
        );
    }

    /// ⚠️ **The epoch fence is in front of `Linearizable` too**, and the
    /// distinction is what makes that right: a cache from a departed epoch
    /// says *this agent* is not the incarnation it believes it is, which is a
    /// fact about the process rather than about how fresh a cache is. The one
    /// caller reads an offset out of its own index on `Authoritative` and
    /// refuses on anything else, so a `Linearizable` short-circuit placed
    /// above this check would let a follower serve a log-end-offset off a
    /// rewound line — hazard H1, arriving through the fence built to stop it.
    ///
    /// ⚠️ **Unasserted for one round of review, and reachable by nothing**,
    /// because `Cluster::cache_state` reports the agent's own epoch today. It
    /// becomes reachable with `M7`'s follower, which is exactly when nobody
    /// will be looking at this ordering.
    #[test]
    fn a_departed_incarnation_refuses_an_authoritative_read_too(
        cache_epoch in 0_u64..=u64::MAX - 1,
        cached in any::<u64>(),
    ) {
        let cache = CacheState::new(
            CoordinatorEpoch::new(cache_epoch),
            Some(CommitVersion::new(cached)),
            0,
        );
        let current = CoordinatorEpoch::new(cache_epoch + 1);
        prop_assert_eq!(
            cache.admits(ReadMode::Linearizable, current),
            Err(RefreshReason::CacheFromAnotherEpoch {
                cache: cache_epoch,
                current: cache_epoch + 1,
            }),
            "the fence is about the agent, not about freshness"
        );
    }

    /// ⚠️ `Linearizable` is never admitted from cache, however fresh. Hazard
    /// H1: `ListOffsets` served from a stale cache reports a log-end-offset
    /// below truth and produces negative consumer lag, so the mode exists to
    /// route to the coordinator unconditionally.
    #[test]
    fn linearizable_reads_are_never_admitted_from_cache(cached in any::<u64>()) {
        let cache = CacheState::new(
            CoordinatorEpoch::ZERO,
            Some(CommitVersion::new(cached)),
            0,
        );
        prop_assert_eq!(
            cache.admits(ReadMode::Linearizable, CoordinatorEpoch::ZERO),
            Err(RefreshReason::Authoritative)
        );
    }

    /// ⚠️ Hazard H4's breaker, and it is in front of **every mode that reads
    /// the cache**. A cache that is current and one whose push stream died
    /// hold the same version; only elapsed silence tells them apart, so no
    /// version comparison can do this and `Stale` is not exempt from it.
    ///
    /// ⚠️ **`Linearizable` is not one of them, and `M3.30` corrected this
    /// property to say so.** It reported `PushStreamSilent` — a cache fault,
    /// raised at the `ListOffsets` rate, for a read no cache may answer and
    /// which therefore never consults one. An operator watching that alarm
    /// could not tell a dead push stream from ordinary traffic, while
    /// `Authoritative`, the reason that is *not* a fault and the one they need
    /// to see, read zero.
    #[test]
    fn silence_past_the_limit_refuses_every_mode_that_reads_the_cache(
        silent in (MAX_METADATA_STALENESS_MS + 1)..=u64::MAX,
        cached in any::<u64>(),
    ) {
        let epoch = CoordinatorEpoch::ZERO;
        let cache = CacheState::new(epoch, Some(CommitVersion::new(cached)), silent);
        for mode in [
            ReadMode::Stale,
            ReadMode::AtLeast(SessionWatermark::new(epoch, CommitVersion::ZERO)),
        ] {
            prop_assert_eq!(
                cache.admits(mode, epoch),
                Err(RefreshReason::PushStreamSilent {
                    silent_for_ms: silent,
                    limit_ms: MAX_METADATA_STALENESS_MS,
                })
            );
        }
        prop_assert_eq!(
            cache.admits(ReadMode::Linearizable, epoch),
            Err(RefreshReason::Authoritative),
            "a read no cache may answer must not raise a cache fault"
        );
    }

    /// ⚠️ **The breaker's own edge**, which the limit's source states exactly:
    /// doc 12 §4.6 says an agent stops serving when
    /// `now - last_delta_received > staleness_limit`, so silence *equal* to
    /// the limit still serves. One millisecond either way is the difference
    /// between a bound and an off-by-one nobody would see.
    #[test]
    fn silence_exactly_at_the_limit_still_serves(cached in any::<u64>()) {
        let epoch = CoordinatorEpoch::ZERO;
        let cache = CacheState::new(
            epoch,
            Some(CommitVersion::new(cached)),
            MAX_METADATA_STALENESS_MS,
        );
        prop_assert_eq!(cache.admits(ReadMode::Stale, epoch), Ok(()));
    }

    /// ⚠️ Hazard H5, and it comes **first**. A cache from an incarnation that
    /// is gone holds positions on a line a failover may have rewound, so it
    /// can be arbitrarily far ahead by version and must still not answer —
    /// which is why the epoch fence is not one more freshness clause.
    #[test]
    fn a_cache_from_a_departed_incarnation_never_answers(
        cached in any::<u64>(),
        silent in 0..=MAX_METADATA_STALENESS_MS,
    ) {
        let cache = CacheState::new(
            CoordinatorEpoch::new(1),
            Some(CommitVersion::new(cached)),
            silent,
        );
        prop_assert_eq!(
            cache.admits(ReadMode::Stale, CoordinatorEpoch::new(2)),
            Err(RefreshReason::CacheFromAnotherEpoch { cache: 1, current: 2 })
        );
    }

    /// ⚠️ `ADR-0023`: a watermark carried across a rebalance is **not
    /// behind** — it is incomparable, and answering it from a version compare
    /// is the half of that mistake that serves a reader data missing its own
    /// write.
    #[test]
    fn a_watermark_from_another_line_is_incomparable_not_behind(cached in any::<u64>()) {
        let cache = CacheState::new(
            CoordinatorEpoch::new(2),
            Some(CommitVersion::new(cached)),
            0,
        );
        let carried = SessionWatermark::new(CoordinatorEpoch::new(1), CommitVersion::ZERO);
        prop_assert_eq!(
            cache.admits(ReadMode::AtLeast(carried), CoordinatorEpoch::new(2)),
            Err(RefreshReason::WatermarkFromAnotherEpoch { watermark: 1, cache: 2 }),
            "the cache is current and correct; it is the watermark that cannot \
             be compared against it, and discarding a good cache per session \
             is the cost of confusing the two"
        );
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
            .map(|&c| CommittedSpan::new(a_topic(), a_partition(), c, ByteRange::Full))
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
            CommittedSpan::new(a_topic(), a_partition(), a, ByteRange::Full),
            CommittedSpan::new(a_topic(), a_partition(), b, ByteRange::Full)
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
        spans: vec![CommittedSpan::new(
            a_topic(),
            a_partition(),
            3,
            ByteRange::Full,
        )],
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
