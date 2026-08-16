//! `RateGovernor`: admits up to the configured ceiling, delays above it, and
//! never sleeps — every timing decision comes from a [`FakeClock`].

// The workspace denies `expect_used`; every site here is on a value this
// test just constructed from a literal it controls, so a panic means the
// test is wrong.
#![allow(clippy::expect_used)]

use oqueue_core::{FakeClock, OpClass, RateDecision, RateGovernor, RateLimitPolicy};
use std::num::NonZeroU32;
use std::time::Duration;

const fn fixed(writes_per_sec: u32, reads_per_sec: u32) -> RateLimitPolicy {
    RateLimitPolicy::FixedCeiling {
        writes_per_sec: NonZeroU32::new(writes_per_sec).expect("non-zero"),
        reads_per_sec: NonZeroU32::new(reads_per_sec).expect("non-zero"),
    }
}

/// A fixed ceiling admits exactly that many requests, then delays.
#[test]
fn fixed_ceiling_admits_up_to_the_configured_rate() {
    let clock = FakeClock::new();
    let mut governor = RateGovernor::new(fixed(5, 5), &clock);

    for i in 0..5 {
        assert_eq!(
            governor.admit(OpClass::Write, &clock),
            RateDecision::Admit,
            "request {i} should be admitted, within the ceiling"
        );
    }
    assert!(
        matches!(
            governor.admit(OpClass::Write, &clock),
            RateDecision::Delay(_)
        ),
        "the 6th write in the same second must be delayed, not admitted"
    );
}

/// Writes and reads are governed independently, at S3's own different
/// ceilings.
#[test]
fn writes_and_reads_are_governed_independently() {
    let clock = FakeClock::new();
    let mut governor = RateGovernor::new(fixed(2, 4), &clock);

    for _ in 0..2 {
        assert_eq!(governor.admit(OpClass::Write, &clock), RateDecision::Admit);
    }
    assert!(matches!(
        governor.admit(OpClass::Write, &clock),
        RateDecision::Delay(_)
    ));

    // Reads have their own, larger budget and are unaffected by writes
    // having exhausted theirs.
    for _ in 0..4 {
        assert_eq!(governor.admit(OpClass::Read, &clock), RateDecision::Admit);
    }
    assert!(matches!(
        governor.admit(OpClass::Read, &clock),
        RateDecision::Delay(_)
    ));
}

/// After exhausting the ceiling, advancing the fake clock by a full second
/// refills the bucket — proven with no `sleep`, only `FakeClock::advance`.
#[test]
fn the_bucket_refills_after_the_clock_advances() {
    let clock = FakeClock::new();
    let mut governor = RateGovernor::new(fixed(3, 3), &clock);

    for _ in 0..3 {
        assert_eq!(governor.admit(OpClass::Write, &clock), RateDecision::Admit);
    }
    assert!(matches!(
        governor.admit(OpClass::Write, &clock),
        RateDecision::Delay(_)
    ));

    clock
        .advance(1000)
        .expect("advancing a fake clock forward succeeds");

    for i in 0..3 {
        assert_eq!(
            governor.admit(OpClass::Write, &clock),
            RateDecision::Admit,
            "request {i} after refill should be admitted"
        );
    }
}

/// ⚠️ **Regression for a real starvation bug review found**: refilling used
/// to reset the bucket's reference point to `now` on every call, even when
/// the elapsed slice was too small to earn a whole token — which discarded
/// that fractional progress every time and meant no call ever accrued a
/// token if calls arrived more often than `1000 / ceiling` ms apart, no
/// matter how much real time passed. Over exactly 2 real seconds taken in
/// 1ms steps, exactly `7 * 2 = 14` tokens must accrue — not "roughly", since
/// `2000 * 7 = 14000` divides `1000` evenly and the exact-arithmetic design
/// this replaced the bug with owes no rounding slack here.
#[test]
fn many_small_clock_advances_still_accrue_tokens_over_time() {
    let clock = FakeClock::new();
    let mut governor = RateGovernor::new(fixed(7, 7), &clock);

    // Drain the initial burst.
    for _ in 0..7 {
        assert_eq!(governor.admit(OpClass::Write, &clock), RateDecision::Admit);
    }

    // 2 real seconds, taken in 1ms steps -- far more frequent than the
    // ~143ms one token at 7/sec is worth, which is exactly the regime the
    // bug starved in.
    let mut admitted = 0;
    for _ in 0..2000 {
        clock
            .advance(1)
            .expect("advancing a fake clock forward succeeds");
        if governor.admit(OpClass::Write, &clock) == RateDecision::Admit {
            admitted += 1;
        }
    }

    assert_eq!(
        admitted, 14,
        "2 real seconds at 7/sec must accrue exactly 14 tokens"
    );
}

/// ⚠️ **Regression for a second bug review found**, in the fix for the
/// first one: banking the unspent remainder by converting earned tokens
/// back into milliseconds (`earned * 1000 / ceiling`) is itself a second,
/// lossy floor division whenever `1000 % ceiling != 0` — it systematically
/// *under*-charges elapsed time every call, and the discount compounds
/// under sustained load until the achieved rate settles well above the
/// ceiling. `512` is deliberately not a divisor of `1000`, which is exactly
/// the regime that bug needed. Over exactly 1 real second in 1ms steps, no
/// more than `512` may ever be admitted — the ceiling is a ceiling.
#[test]
fn a_ceiling_that_does_not_divide_evenly_is_never_exceeded() {
    let clock = FakeClock::new();
    let mut governor = RateGovernor::new(fixed(512, 512), &clock);

    for _ in 0..512 {
        assert_eq!(governor.admit(OpClass::Write, &clock), RateDecision::Admit);
    }

    let mut admitted = 0;
    for _ in 0..1000 {
        clock
            .advance(1)
            .expect("advancing a fake clock forward succeeds");
        if governor.admit(OpClass::Write, &clock) == RateDecision::Admit {
            admitted += 1;
        }
    }

    assert_eq!(
        admitted, 512,
        "1 real second at 512/sec must admit exactly 512, not more"
    );
}

/// A ramping policy's ceiling doubles after one full doubling period.
#[test]
fn ramping_policy_doubles_after_one_period() {
    let clock = FakeClock::new();
    let policy = RateLimitPolicy::Ramping {
        initial_writes_per_sec: NonZeroU32::new(2).expect("non-zero"),
        initial_reads_per_sec: NonZeroU32::new(2).expect("non-zero"),
        doubling_period: Duration::from_mins(20), // 20 minutes
    };
    let mut governor = RateGovernor::new(policy, &clock);

    // At t=0: ceiling is 2.
    assert_eq!(governor.admit(OpClass::Write, &clock), RateDecision::Admit);
    assert_eq!(governor.admit(OpClass::Write, &clock), RateDecision::Admit);
    assert!(matches!(
        governor.admit(OpClass::Write, &clock),
        RateDecision::Delay(_)
    ));

    // One full doubling period later, and a fresh second's worth of tokens:
    // the ceiling is now 4.
    clock
        .advance(1200 * 1000)
        .expect("advancing a fake clock forward succeeds");
    let mut admitted = 0;
    for _ in 0..4 {
        if governor.admit(OpClass::Write, &clock) == RateDecision::Admit {
            admitted += 1;
        }
    }
    assert_eq!(
        admitted, 4,
        "after one doubling period the ceiling should be 4, not 2"
    );
}

/// A ramping policy at `t=0` behaves exactly like its initial rate — no
/// doubling has happened yet.
#[test]
fn ramping_policy_starts_at_its_initial_rate() {
    let clock = FakeClock::new();
    let policy = RateLimitPolicy::Ramping {
        initial_writes_per_sec: NonZeroU32::new(3).expect("non-zero"),
        initial_reads_per_sec: NonZeroU32::new(3).expect("non-zero"),
        doubling_period: Duration::from_mins(20),
    };
    let mut governor = RateGovernor::new(policy, &clock);

    for _ in 0..3 {
        assert_eq!(governor.admit(OpClass::Write, &clock), RateDecision::Admit);
    }
    assert!(matches!(
        governor.admit(OpClass::Write, &clock),
        RateDecision::Delay(_)
    ));
}

/// ⚠️ **Regression for M1.33**, found by M1's checkpoint review: a
/// `Ramping` policy's write and read ceilings are independent, matching
/// GCS's own documented split (doc 04 §3: roughly 1,000 write/sec, 5,000
/// read/sec of "free" capacity) — not one shared rate for both, which an
/// earlier version of this variant collapsed them into.
#[test]
fn ramping_policy_writes_and_reads_start_at_different_rates() {
    let clock = FakeClock::new();
    let policy = RateLimitPolicy::Ramping {
        initial_writes_per_sec: NonZeroU32::new(2).expect("non-zero"),
        initial_reads_per_sec: NonZeroU32::new(5).expect("non-zero"),
        doubling_period: Duration::from_mins(20),
    };
    let mut governor = RateGovernor::new(policy, &clock);

    for _ in 0..2 {
        assert_eq!(governor.admit(OpClass::Write, &clock), RateDecision::Admit);
    }
    assert!(
        matches!(
            governor.admit(OpClass::Write, &clock),
            RateDecision::Delay(_)
        ),
        "the write ceiling is 2, not 5"
    );

    for _ in 0..5 {
        assert_eq!(governor.admit(OpClass::Read, &clock), RateDecision::Admit);
    }
    assert!(
        matches!(
            governor.admit(OpClass::Read, &clock),
            RateDecision::Delay(_)
        ),
        "the read ceiling is 5, not 2, and is unaffected by the write bucket"
    );
}
