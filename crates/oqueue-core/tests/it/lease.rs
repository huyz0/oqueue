//! The coordinator lease (`M6.7`): taken while free, held on the holder's own
//! clock, taken over only after it lapsed, and lost the moment a successor
//! exists.

#![allow(clippy::expect_used)]

use crate::metadata_log::block_on;
use oqueue_core::{
    FakeClock, FakeObjectStore, LEASE_SKEW_MS, LEASE_TTL_MS, ObjectStore, ObjectStoreLease,
    Timestamp,
};
use std::sync::Arc;

fn node(store: &Arc<dyn ObjectStore>, name: &str, clock: &Arc<FakeClock>) -> ObjectStoreLease {
    ObjectStoreLease::new(Arc::clone(store), "meta/0", name, Arc::clone(clock) as _)
}

fn setup() -> (Arc<dyn ObjectStore>, Arc<FakeClock>) {
    let clock = Arc::new(FakeClock::starting_at(
        Timestamp::from_millis(1_700_000_000_000).expect("a valid time"),
    ));
    (Arc::new(FakeObjectStore::new()), clock)
}

#[test]
fn a_free_lease_is_taken_and_a_held_one_is_refused() {
    let (store, clock) = setup();
    let a = node(&store, "a", &clock);
    let b = node(&store, "b", &clock);
    assert!(!a.is_held(), "nothing is held before acquiring");
    assert!(block_on(a.acquire()).expect("acquires"));
    assert!(a.is_held());
    assert_eq!(a.term(), Some(0));
    assert!(
        !block_on(b.acquire()).expect("asks"),
        "a held lease is refused"
    );
    assert!(!b.is_held());
}

/// ⚠️ **The holder fences itself on its own clock**: no renewal, and the lease
/// lapses locally the moment its TTL has passed.
#[test]
fn an_unrenewed_lease_lapses_on_the_holders_clock() {
    let (store, clock) = setup();
    let a = node(&store, "a", &clock);
    assert!(block_on(a.acquire()).expect("acquires"));
    clock.advance(LEASE_TTL_MS - 1).expect("the clock moves");
    assert!(a.is_held(), "one millisecond short");
    clock.advance(1).expect("the clock moves");
    assert!(!a.is_held(), "lapsed");
}

#[test]
fn a_renewal_extends_the_lease() {
    let (store, clock) = setup();
    let a = node(&store, "a", &clock);
    assert!(block_on(a.acquire()).expect("acquires"));
    clock.advance(LEASE_TTL_MS / 2).expect("the clock moves");
    assert!(block_on(a.renew()).expect("renews"));
    clock.advance(LEASE_TTL_MS - 1).expect("the clock moves");
    assert!(a.is_held(), "the renewal restarted the TTL");
    let b = node(&store, "b", &clock);
    assert!(
        !block_on(b.acquire()).expect("asks"),
        "and the record says so too"
    );
}

/// ⚠️ **A challenger waits out the recorded expiry plus the skew**, then takes
/// the next term — and the old holder's next renewal finds it and stops.
#[test]
fn a_lapsed_lease_is_taken_over_and_its_holder_learns_it() {
    let (store, clock) = setup();
    let a = node(&store, "a", &clock);
    let b = node(&store, "b", &clock);
    assert!(block_on(a.acquire()).expect("acquires"));
    clock
        .advance(LEASE_TTL_MS + LEASE_SKEW_MS - 1)
        .expect("the clock moves");
    assert!(
        !block_on(b.acquire()).expect("asks"),
        "within the skew margin"
    );
    clock.advance(1).expect("the clock moves");
    assert!(block_on(b.acquire()).expect("takes over"));
    assert_eq!(b.term(), Some(1));
    assert!(
        !block_on(a.renew()).expect("renews"),
        "the old holder is told"
    );
    assert!(!a.is_held());
}

/// The newest of many terms is found, so a long-lived shard still takes over.
#[test]
fn the_newest_of_many_terms_is_found() {
    let (store, clock) = setup();
    for term in 0..9_u64 {
        let holder = node(&store, &format!("n{term}"), &clock);
        assert!(block_on(holder.acquire()).expect("acquires"));
        assert_eq!(holder.term(), Some(term));
        clock
            .advance(LEASE_TTL_MS + LEASE_SKEW_MS)
            .expect("the clock moves");
    }
}

/// `Debug` names the term, never the holder.
#[test]
fn debug_names_the_term_and_not_the_holder() {
    let (store, clock) = setup();
    let a = node(&store, "secret-holder", &clock);
    assert!(block_on(a.acquire()).expect("acquires"));
    let shown = format!("{a:?}");
    assert!(shown.contains("term: Some(0)"), "{shown}");
    assert!(!shown.contains("secret-holder"), "{shown}");
}

/// ⚠️ **A successor fences the holder at once, not at its deadline**: the
/// renewal's poll finds the next term while the holder's own clock still says
/// it has time left, and it stops acting then.
#[test]
fn a_successor_fences_the_holder_before_its_deadline() {
    let (store, clock) = setup();
    let a = node(&store, "a", &clock);
    assert!(block_on(a.acquire()).expect("acquires"));
    let key = oqueue_core::ObjectKey::new("meta/0/lease/00000000000000000001").expect("a key");
    let mut body = b"OQLS".to_vec();
    body.extend_from_slice(&i64::MAX.to_be_bytes());
    block_on(store.put(&key, body, None)).expect("a successor's term");
    assert!(a.is_held(), "the holder's own clock says it has time left");
    assert!(
        !block_on(a.renew()).expect("renews"),
        "the poll finds the successor"
    );
    assert!(!a.is_held(), "and it stops acting at once");
}

/// ⚠️ **A holder paused past its deadline cannot renew its way back**
/// (`M6.7`'s review): a successor may already have read the old expiry.
#[test]
fn a_lapsed_lease_is_not_renewed() {
    let (store, clock) = setup();
    let a = node(&store, "a", &clock);
    assert!(block_on(a.acquire()).expect("acquires"));
    clock.advance(LEASE_TTL_MS).expect("the clock moves");
    assert!(!block_on(a.renew()).expect("asks"), "lapsed, so refused");
    assert!(!a.is_held());
    assert_eq!(a.term(), None, "and a lost lease names no term");
}

/// ⚠️ **A renewal that lands too late does not extend the deadline**: it
/// finished inside the skew margin, where a successor may have read the old
/// expiry, so the old deadline stands.
#[test]
fn a_late_renewal_does_not_extend_the_deadline() {
    let (store, clock) = setup();
    let a = node(&store, "a", &clock);
    assert!(block_on(a.acquire()).expect("acquires"));
    clock
        .advance(LEASE_TTL_MS - LEASE_SKEW_MS)
        .expect("the clock moves");
    assert!(
        block_on(a.renew()).expect("renews"),
        "the write itself lands"
    );
    clock.advance(LEASE_SKEW_MS).expect("the clock moves");
    assert!(!a.is_held(), "but the deadline was not moved");
}
