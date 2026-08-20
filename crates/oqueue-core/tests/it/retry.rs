//! Retry decisions driven by error class — `error-handling.md` rule 7's
//! shape, and rule 9's ban on retrying a lost compare-and-swap.

// The workspace denies `expect_used`; every site here is on a value this
// test just constructed from a literal it controls, so a panic means the
// test is wrong.
#![allow(clippy::expect_used)]

use oqueue_core::{Error, ObjectKey, RetryClass, RetryDecision, RetryPolicy};
use std::num::NonZeroU32;
use std::time::Duration;

fn key(name: &str) -> ObjectKey {
    ObjectKey::new(name).expect("a non-empty key")
}

const fn policy() -> RetryPolicy {
    RetryPolicy::new(
        NonZeroU32::new(3).expect("non-zero"),
        Duration::from_millis(100),
        Duration::from_secs(10),
    )
}

/// `SlowDown` and `Throttled` are `RetryClass::Forever`.
#[test]
fn slow_down_and_throttled_are_forever() {
    assert_eq!(Error::SlowDown.retry_class(), RetryClass::Forever);
    assert_eq!(Error::Throttled.retry_class(), RetryClass::Forever);
}

/// `Transient` is `RetryClass::Bounded`.
#[test]
fn transient_is_bounded() {
    assert_eq!(Error::Transient.retry_class(), RetryClass::Bounded);
}

/// `PreconditionFailed` — the load-bearing case — is `RetryClass::Never`.
#[test]
fn precondition_failed_is_never_retried() {
    let err = Error::PreconditionFailed { key: key("k") };
    assert_eq!(err.retry_class(), RetryClass::Never);
}

/// `PreconditionFailed` reaches the caller on the very first attempt — the
/// acceptance criterion stated directly: a lost CAS is never given a second
/// try at the point it is detected.
#[test]
fn precondition_failed_gives_up_on_the_first_attempt() {
    let err = Error::PreconditionFailed { key: key("k") };
    assert_eq!(policy().decide(&err, 1), RetryDecision::GiveUp);
}

/// Every other client-error-shaped variant is also `RetryClass::Never` —
/// retrying a malformed request or an absent object does not make either
/// stop being wrong.
#[test]
fn client_errors_are_never_retried() {
    let cases = [
        Error::ObjectNotFound { key: key("k") },
        Error::EmptyObjectKey,
        Error::EmptyByteRange,
        Error::ByteRangeOutOfBounds {
            key: key("k"),
            offset: 0,
            length: 1,
            object_size: 0,
        },
        Error::ChunkLengthTooLarge {
            length: 5,
            chunk_size: 4,
        },
        Error::PartTooSmall { bytes: 1, min: 10 },
        Error::PartTooLarge { bytes: 10, max: 1 },
        Error::TooManyParts { max: 1 },
        Error::ObjectTooLarge { max: 1 },
    ];
    for err in cases {
        assert_eq!(
            err.retry_class(),
            RetryClass::Never,
            "{err:?} must be RetryClass::Never"
        );
    }
}

/// `Forever`-class errors retry no matter how many attempts have already
/// been made — that is what "forever" means.
#[test]
fn forever_class_retries_past_any_bounded_limit() {
    let result = policy().decide(&Error::Throttled, 1_000_000);
    assert!(
        matches!(result, RetryDecision::Retry(_)),
        "Throttled must still retry at attempt 1,000,000, got {result:?}"
    );
}

/// `Bounded`-class errors retry up to the configured limit, then give up.
#[test]
fn bounded_class_gives_up_once_the_limit_is_reached() {
    let policy = policy(); // max_bounded_attempts = 3
    assert!(matches!(
        policy.decide(&Error::Transient, 1),
        RetryDecision::Retry(_)
    ));
    assert!(matches!(
        policy.decide(&Error::Transient, 2),
        RetryDecision::Retry(_)
    ));
    assert_eq!(policy.decide(&Error::Transient, 3), RetryDecision::GiveUp);
    assert_eq!(policy.decide(&Error::Transient, 4), RetryDecision::GiveUp);
}

/// Backoff doubles per attempt, starting from the configured base delay.
#[test]
fn backoff_doubles_per_attempt() {
    let policy = RetryPolicy::new(
        NonZeroU32::new(10).expect("non-zero"),
        Duration::from_millis(100),
        Duration::from_hours(1),
    );
    let delay_at = |attempt| match policy.decide(&Error::Transient, attempt) {
        RetryDecision::Retry(d) => d,
        RetryDecision::GiveUp => panic!("expected a retry at attempt {attempt}"),
    };
    assert_eq!(delay_at(1), Duration::from_millis(100));
    assert_eq!(delay_at(2), Duration::from_millis(200));
    assert_eq!(delay_at(3), Duration::from_millis(400));
    assert_eq!(delay_at(4), Duration::from_millis(800));
}

/// Backoff never exceeds `max_delay`, however many attempts have passed.
#[test]
fn backoff_is_capped_at_max_delay() {
    let policy = RetryPolicy::new(
        NonZeroU32::new(1).expect("non-zero"),
        Duration::from_millis(100),
        Duration::from_secs(1),
    );
    match policy.decide(&Error::Throttled, 100) {
        RetryDecision::Retry(d) => assert!(
            d <= Duration::from_secs(1),
            "backoff must be capped at max_delay, got {d:?}"
        ),
        RetryDecision::GiveUp => panic!("Throttled must always retry"),
    }
}

/// A pathologically large attempt count does not overflow or panic — the
/// exponent computation saturates rather than wrapping.
#[test]
fn an_enormous_attempt_count_does_not_panic() {
    let policy = policy();
    let result = policy.decide(&Error::Throttled, u32::MAX);
    assert!(matches!(result, RetryDecision::Retry(d) if d <= Duration::from_secs(10)));
}
