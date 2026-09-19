//! The renewal task keeps a lease across many TTLs, and stops once it is lost.

#![allow(clippy::expect_used)]

use super::renew_lease;
use core::time::Duration;
use oqueue_core::{
    FakeClock, FakeObjectStore, LEASE_RENEW_MS, LEASE_TTL_MS, ObjectStore, ObjectStoreLease,
    Timestamp,
};
use std::sync::Arc;

#[tokio::test(start_paused = true)]
async fn a_renewed_lease_is_held_across_many_ttls_and_the_task_ends_when_it_is_lost() {
    let store: Arc<dyn ObjectStore> = Arc::new(FakeObjectStore::new());
    let clock = Arc::new(FakeClock::starting_at(
        Timestamp::from_millis(1_700_000_000_000).expect("a valid time"),
    ));
    let lease = Arc::new(ObjectStoreLease::new(
        Arc::clone(&store),
        "meta/0",
        "a",
        Arc::clone(&clock) as _,
    ));
    assert!(lease.acquire().await.expect("acquires"));
    let task = tokio::spawn(renew_lease(Arc::clone(&lease)));

    let period = Duration::from_millis(LEASE_RENEW_MS.unsigned_abs());
    for _ in 0..(3 * LEASE_TTL_MS / LEASE_RENEW_MS) {
        tokio::time::sleep(period).await;
        clock.advance(LEASE_RENEW_MS).expect("the clock moves");
        tokio::task::yield_now().await;
        assert!(lease.is_held(), "renewed before it could lapse");
    }

    // A successor's term appears: the next renewal finds it and the task ends.
    let term = lease.term().expect("a term") + 1;
    let key = oqueue_core::ObjectKey::new(format!("meta/0/lease/{term:020}")).expect("a key");
    let mut body = b"OQLS".to_vec();
    body.extend_from_slice(&i64::MAX.to_be_bytes());
    store.put(&key, body, None).await.expect("a successor");
    tokio::time::sleep(period * 2).await;
    assert!(task.is_finished(), "the task ends once the lease is lost");
    assert!(!lease.is_held());
}
