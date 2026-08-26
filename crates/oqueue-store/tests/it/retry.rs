//! `ADR-0008`'s premise, discharged: what a `RetryPolicy` becomes downstream.
//!
//! `M3.13`. The ADR chose `object_store` partly for its retry handling, and
//! since `M1.56` a `RetryPolicy` has sat in `oqueue-core` with no caller above
//! a vendor retry nobody configured — *no double-retry existed precisely
//! because nothing invoked it*. This is where the two are connected, so what is
//! pinned here is that the translation says what the policy says.

#![allow(clippy::expect_used)]

use core::num::NonZeroU32;
use core::time::Duration;
use oqueue_core::RetryPolicy;
use oqueue_store::retry_config_for;

/// Every field the policy names arrives, and the multiplier matches the
/// doubling the policy describes in prose — the one field where the two shapes
/// could silently disagree.
#[test]
fn a_policy_becomes_the_vendor_config_it_describes() {
    let policy = RetryPolicy::new(
        NonZeroU32::new(7).expect("7 is non-zero"),
        Duration::from_millis(250),
        Duration::from_secs(30),
    );

    let config = retry_config_for(policy);

    assert_eq!(
        config.max_retries, 6,
        "⚠️ seven *attempts* is six retries: `RetryPolicy::decide` gives up \
         once attempts reach the limit, while `object_store` counts retries \
         from zero — copying the number across would buy an attempt nobody \
         asked for"
    );
    assert_eq!(config.backoff.init_backoff, Duration::from_millis(250));
    assert_eq!(config.backoff.max_backoff, Duration::from_secs(30));
    assert!(
        (config.backoff.base - 2.0).abs() < f64::EPSILON,
        "the policy doubles, so the multiplier is 2"
    );
}

/// ⚠️ The default is what a backend built from the environment uses, so it is
/// worth seeing rather than trusting: three bounded retries, doubling from
/// 100 ms, capped at 5 s.
#[test]
fn the_default_policy_is_the_one_a_backend_gets() {
    let config = retry_config_for(RetryPolicy::DEFAULT);

    assert_eq!(config.max_retries, 2, "three attempts is two retries");
    assert_eq!(config.backoff.init_backoff, Duration::from_millis(100));
    assert_eq!(config.backoff.max_backoff, Duration::from_secs(5));
}

/// ⚠️ **The boundary case, and the one a straight copy inverts.** A policy of
/// one attempt means *never retry*; a vendor `max_retries` of one means retry
/// once. A caller asking for no retries must not get one.
#[test]
fn a_policy_of_one_attempt_configures_no_retries_at_all() {
    let never = RetryPolicy::new(
        NonZeroU32::new(1).expect("1 is non-zero"),
        Duration::from_millis(10),
        Duration::from_millis(10),
    );
    assert_eq!(retry_config_for(never).max_retries, 0);
}

/// ⚠️ `retry_timeout` is left at the vendor's default deliberately:
/// `RetryPolicy` names attempts and delays and no wall-clock ceiling, so
/// translating one would be a number nobody measured. Pinned so that a later
/// change to it is a decision rather than a drift.
#[test]
fn the_wall_clock_ceiling_is_the_vendors_because_the_policy_names_none() {
    let translated = retry_config_for(RetryPolicy::DEFAULT);
    let vendor = object_store::RetryConfig::default();
    assert_eq!(translated.retry_timeout, vendor.retry_timeout);
}
