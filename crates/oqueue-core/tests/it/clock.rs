//! The `Clock` contract, and the fake's fidelity to it.

// ⚠️ The workspace denies `expect_used`; the root `Cargo.toml` says the
// allowance belongs where a reader can see which tests took it. Every `expect`
// below is on a value this test just built from a generator constrained to the
// valid range, so a panic means the generator is wrong, not the code under test.
#![allow(clippy::expect_used)]

use oqueue_core::{Clock, Error, FakeClock, Timestamp};
use proptest::prelude::*;

/// The acceptance criterion, stated directly: time does not move on its own.
#[test]
fn two_reads_with_no_advance_return_the_same_instant() {
    let clock = FakeClock::new();
    let first = clock.now();
    let second = clock.now();
    assert_eq!(first, second);
    assert_eq!(first, Timestamp::EPOCH);
}

/// And it does move when told to.
#[test]
fn advancing_moves_time_forward_by_exactly_that_much() {
    let clock = FakeClock::new();
    let before = clock.now();
    let returned = clock.advance(1_500).expect("1500 is a valid advance");
    let after = clock.now();
    assert_eq!(after.as_millis(), before.as_millis() + 1_500);
    assert_eq!(after, returned);
}

proptest! {
    /// ⚠️ The guarantee the trait's documentation binds implementors to: two
    /// reads in program order never yield a smaller second value. Asserted
    /// across arbitrary advance sequences, including zero-length ones.
    #[test]
    fn the_fake_is_never_observed_to_go_backwards(steps in prop::collection::vec(0i64..1_000_000, 0..64)) {
        let clock = FakeClock::new();
        let mut previous = clock.now();
        for step in steps {
            clock.advance(step).expect("step is in range");
            let current = clock.now();
            prop_assert!(current >= previous, "{current} < {previous}");
            previous = current;
        }
    }

    /// A fake cannot be driven backwards, so a test cannot construct a state
    /// no real implementor could produce.
    #[test]
    fn advancing_by_a_negative_amount_is_refused(delta in i64::MIN..0) {
        let clock = FakeClock::new();
        prop_assert_eq!(clock.advance(delta), Err(Error::NegativeClockAdvance { got: delta }));
        prop_assert_eq!(clock.now(), Timestamp::EPOCH);
    }

    /// No `i64` produces a negative `Timestamp`.
    #[test]
    fn timestamp_is_never_negative(millis in any::<i64>()) {
        match Timestamp::from_millis(millis) {
            Ok(t) => prop_assert!(t.as_millis() >= 0),
            Err(e) => {
                prop_assert!(millis < 0);
                prop_assert_eq!(e, Error::NegativeTimestamp { got: millis });
            }
        }
    }
}

/// Overflow is refused rather than wrapped, for the same reason `Offset::add`
/// refuses it: a wrapped clock reads as time travelling backwards.
#[test]
fn advancing_past_the_maximum_errors_rather_than_wrapping() {
    let clock = FakeClock::starting_at(Timestamp::from_millis(i64::MAX).expect("valid"));
    assert_eq!(
        clock.advance(1),
        Err(Error::TimestampOverflow {
            base: i64::MAX,
            delta: 1
        })
    );
    assert_eq!(clock.now().as_millis(), i64::MAX);
}

/// ⚠️ The fake is `Send + Sync` and takes `&self`, so it will be shared through
/// an `Arc`. Review found that an earlier load-check-store `advance` lost
/// updates and let an observer see time run backwards — the one guarantee the
/// contract has. This is the test that would have caught it.
#[test]
fn concurrent_advances_neither_lose_updates_nor_go_backwards() {
    use std::sync::Arc;

    const THREADS: i64 = 4;
    const STEPS: i64 = 2_000;

    let clock = Arc::new(FakeClock::new());
    let observed_backwards = Arc::new(std::sync::atomic::AtomicBool::new(false));

    std::thread::scope(|scope| {
        for _ in 0..THREADS {
            let clock = Arc::clone(&clock);
            let flag = Arc::clone(&observed_backwards);
            scope.spawn(move || {
                let mut previous = clock.now();
                for _ in 0..STEPS {
                    clock.advance(1).expect("1 is a valid advance");
                    let current = clock.now();
                    if current < previous {
                        flag.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                    previous = current;
                }
            });
        }
    });

    assert_eq!(
        clock.now().as_millis(),
        THREADS * STEPS,
        "advances were lost"
    );
    assert!(
        !observed_backwards.load(std::sync::atomic::Ordering::SeqCst),
        "an observer saw the clock run backwards"
    );
}

/// ⚠️ Kills the `< ` -> `<=` mutant in `FakeClock::advance`: advancing by zero
/// is valid and must not be rejected as negative.
#[test]
fn advancing_by_zero_is_valid() {
    let clock = FakeClock::new();
    clock.advance(10).expect("valid");
    assert_eq!(clock.advance(0).map(Timestamp::as_millis), Ok(10));
    assert_eq!(clock.now().as_millis(), 10);
}
