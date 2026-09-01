//! Fault injection on [`FakeObjectStore`]: every knob defaults off, and each
//! one is independently reachable and controllable.

// The workspace denies `expect_used`; these sites are on values this test
// just constructed from literals it controls, so a panic means the test is
// wrong.
#![allow(clippy::expect_used)]

use oqueue_core::{
    ByteRange, Error, FakeObjectStore, FaultConfig, ObjectKey, ObjectStore, StormKind,
};
use std::future::Future;
use std::task::{Context, Poll, Waker};

/// Drives a future to completion, counting how many times it returned
/// `Poll::Pending` before resolving — the thing `latency_polls` claims to
/// control.
fn poll_count<F: Future>(future: F) -> (F::Output, u32) {
    let mut future = Box::pin(future);
    let waker = Waker::noop();
    let mut cx = Context::from_waker(waker);
    let mut pending_count = 0;
    loop {
        match future.as_mut().poll(&mut cx) {
            Poll::Ready(value) => return (value, pending_count),
            Poll::Pending => pending_count += 1,
        }
    }
}

fn block_on<F: Future>(future: F) -> F::Output {
    poll_count(future).0
}

fn key(name: &str) -> ObjectKey {
    ObjectKey::new(name).expect("a non-empty key")
}

/// A store built with every fault at its default (off) behaves exactly like
/// `FakeObjectStore::new()` — the same acceptance `M1.6`'s tests already
/// exercise against `new()`, now proven for the other constructor too.
#[test]
fn default_fault_config_behaves_like_no_faults_at_all() {
    let store = FakeObjectStore::with_faults(FaultConfig::default());
    let k = key("topics/orders/0/x.seg");

    let meta = block_on(store.put(&k, vec![1, 2, 3], None)).expect("put succeeds");
    assert_eq!(block_on(store.get(&k, ByteRange::Full)), Ok(vec![1, 2, 3]));
    assert_eq!(meta.size, 3);
}

/// `latency_polls` makes every call return `Poll::Pending` exactly that many
/// times before resolving — not zero, not one more, not one fewer.
#[test]
fn latency_polls_is_the_exact_number_of_pending_polls() {
    let store = FakeObjectStore::with_faults(FaultConfig {
        latency_polls: 5,
        ..FaultConfig::default()
    });
    let k = key("topics/orders/0/slow.seg");

    let (result, pending_count) = poll_count(store.put(&k, vec![1], None));
    result.expect("put still succeeds, just later");
    assert_eq!(pending_count, 5);

    let (result, pending_count) = poll_count(store.get(&k, ByteRange::Full));
    result.expect("get still succeeds, just later");
    assert_eq!(pending_count, 5);
}

/// With `latency_polls` at zero (the default), no extra polls happen — the
/// knob being present costs nothing when unset.
#[test]
fn zero_latency_polls_adds_no_extra_polls() {
    let store = FakeObjectStore::new();
    let k = key("topics/orders/0/fast.seg");

    let (_, pending_count) = poll_count(store.put(&k, vec![1], None));
    assert_eq!(pending_count, 0);
}

/// `put_latency_polls` delays `put` only — `get` and `delete` are unaffected
/// (`M10.31`). This is the property `latency_polls` alone cannot give a
/// caller: a way to hold one `put` in flight while an observing `get` runs
/// at its ordinary speed.
#[test]
fn put_latency_polls_delays_put_and_nothing_else() {
    let store = FakeObjectStore::with_faults(FaultConfig {
        put_latency_polls: 5,
        ..FaultConfig::default()
    });
    let k = key("topics/orders/0/put-only-slow.seg");

    let (result, pending_count) = poll_count(store.put(&k, vec![1], None));
    result.expect("put still succeeds, just later");
    assert_eq!(pending_count, 5, "put pays the put-only delay");

    let (result, pending_count) = poll_count(store.get(&k, ByteRange::Full));
    result.expect("get succeeds");
    assert_eq!(pending_count, 0, "get is not delayed by a put-only fault");
}

/// The two latency knobs are additive, not exclusive — a caller that wants
/// `put` slower than everything else sets both.
#[test]
fn latency_polls_and_put_latency_polls_add_on_a_put() {
    let store = FakeObjectStore::with_faults(FaultConfig {
        latency_polls: 3,
        put_latency_polls: 4,
        ..FaultConfig::default()
    });
    let k = key("topics/orders/0/doubly-slow.seg");

    let (result, pending_count) = poll_count(store.put(&k, vec![1], None));
    result.expect("put still succeeds, just later");
    assert_eq!(pending_count, 7, "both delays are paid, in either order");

    let (result, pending_count) = poll_count(store.get(&k, ByteRange::Full));
    result.expect("get succeeds");
    assert_eq!(
        pending_count, 3,
        "get pays only the uniform delay, not the put-only one"
    );
}

/// A storm fails exactly its configured count of calls, across any mix of
/// `get`/`put`/`delete`, then the fake goes back to succeeding.
#[test]
fn a_storm_fails_exactly_its_configured_count_then_stops() {
    let store = FakeObjectStore::with_faults(FaultConfig {
        storm: Some((StormKind::Throttled, 2)),
        ..FaultConfig::default()
    });
    let k = key("topics/orders/0/storm.seg");

    assert_eq!(
        block_on(store.put(&k, vec![1], None)),
        Err(Error::Throttled),
        "1st call: storm still active"
    );
    assert_eq!(
        block_on(store.get(&k, ByteRange::Full)),
        Err(Error::Throttled),
        "2nd call: storm still active, and a storm is not per-key"
    );
    block_on(store.put(&k, vec![1], None)).expect("3rd call: the storm has passed");
    assert_eq!(
        block_on(store.get(&k, ByteRange::Full)),
        Ok(vec![1]),
        "the object from the 3rd call's put is really there"
    );
}

/// Each `StormKind` produces its own distinct, classified error.
#[test]
fn each_storm_kind_produces_its_own_error() {
    for (kind, expected) in [
        (StormKind::SlowDown, Error::SlowDown),
        (StormKind::Throttled, Error::Throttled),
        (StormKind::Transient, Error::Transient),
    ] {
        let store = FakeObjectStore::with_faults(FaultConfig {
            storm: Some((kind, 1)),
            ..FaultConfig::default()
        });
        assert_eq!(
            block_on(store.put(&key("topics/orders/0/k.seg"), vec![1], None)),
            Err(expected)
        );
    }
}

/// `crash_after_put_before_ack`: the object is durably written, but `put`
/// still resolves `Err` — exactly ADR-0005's documented ambiguity. A caller
/// must not treat this `Err` as proof the object is absent.
#[test]
fn crash_after_put_writes_the_object_but_reports_failure() {
    let store = FakeObjectStore::with_faults(FaultConfig {
        crash_after_put_before_ack: 1,
        ..FaultConfig::default()
    });
    let k = key("topics/orders/0/crash.seg");

    let result = block_on(store.put(&k, vec![1, 2, 3], None));

    assert_eq!(result, Err(Error::Transient));
    assert_eq!(
        block_on(store.get(&k, ByteRange::Full)),
        Ok(vec![1, 2, 3]),
        "the object landed even though put reported failure"
    );
}

/// The crash count is consumed like the storm's: after it runs out, `put`
/// goes back to reporting success honestly.
#[test]
fn crash_after_put_stops_once_its_count_is_exhausted() {
    let store = FakeObjectStore::with_faults(FaultConfig {
        crash_after_put_before_ack: 1,
        ..FaultConfig::default()
    });
    let k = key("topics/orders/0/crash-once.seg");

    assert_eq!(
        block_on(store.put(&k, vec![1], None)),
        Err(Error::Transient)
    );
    block_on(store.put(&k, vec![2], None)).expect("the crash fault has been consumed");
    assert_eq!(block_on(store.get(&k, ByteRange::Full)), Ok(vec![2]));
}

/// Conditional-write race interleaving: two conditioned `put`s targeting the
/// same key from separate threads, each presenting the token of the object
/// that was there when the test set them up. Exactly one can hold — the
/// atomic check-and-write `M1.6` built means the race is decided cleanly,
/// never with both winning or the object left in a mixed state.
#[test]
fn racing_conditional_puts_leave_exactly_one_winner() {
    use std::sync::Arc;

    let store = Arc::new(FakeObjectStore::new());
    let k = key("topics/orders/0/cas-race.seg");
    let initial = block_on(store.put(&k, vec![0], None)).expect("seed the key");

    let store_a = Arc::clone(&store);
    let key_a = k.clone();
    let token_a = initial.precondition_token.clone();
    let handle_a = std::thread::spawn(move || {
        block_on(store_a.put(
            &key_a,
            vec![0xAA; 4],
            Some(oqueue_core::Precondition::IfMatches(token_a)),
        ))
    });

    let store_b = Arc::clone(&store);
    let key_b = k.clone();
    let token_b = initial.precondition_token;
    let handle_b = std::thread::spawn(move || {
        block_on(store_b.put(
            &key_b,
            vec![0xBB; 4],
            Some(oqueue_core::Precondition::IfMatches(token_b)),
        ))
    });

    let result_a = handle_a.join().expect("writer a does not panic");
    let result_b = handle_b.join().expect("writer b does not panic");

    let outcomes = [result_a.is_ok(), result_b.is_ok()];
    assert_eq!(
        outcomes.iter().filter(|ok| **ok).count(),
        1,
        "presenting the same token, exactly one racer's conditional put must win: {outcomes:?}"
    );

    let winner = block_on(store.get(&k, ByteRange::Full)).expect("a winner is stored");
    assert!(
        winner == vec![0xAA; 4] || winner == vec![0xBB; 4],
        "the stored payload must be exactly the winning racer's whole payload, got {winner:?}"
    );
}

/// ⚠️ **A store already in use can be broken**, which is not the same knob as
/// [`FakeObjectStore::with_faults`]. Testing what a *read* path does when an
/// object read fails needs an object first, so the store has to work and then
/// stop working — a store broken from construction has nothing to read.
#[test]
fn faults_can_be_set_on_a_store_that_has_already_succeeded() {
    let store = FakeObjectStore::new();
    let key = ObjectKey::new("k".to_owned()).expect("a valid key");
    block_on(store.put(&key, vec![1, 2, 3], None)).expect("a healthy store writes");

    store.set_faults(FaultConfig {
        storm: Some((StormKind::Transient, 1)),
        ..FaultConfig::default()
    });

    assert_eq!(
        block_on(store.get(&key, ByteRange::Full)),
        Err(Error::Transient),
        "the storm applies to the very next call"
    );
    assert_eq!(
        block_on(store.get(&key, ByteRange::Full)),
        Ok(vec![1, 2, 3]),
        "and only for as many calls as it was given"
    );
}
