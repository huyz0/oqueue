//! M10's pause, with the coordinator lease as its subject (`M6.14`, `M6.md`
//! task 13a): a holder whose renewal is withheld past its TTL has lost its
//! lease and does not know — and must fence itself anyway.
//!
//! ⚠️ **Through the real `S3Store` over the model**, so the pause is the one
//! `M10.8` built — a response withheld after the write landed — and not a
//! fake clock stepped by the test. The lease's clock is tokio's, which the
//! paused runtime advances exactly as it advances the withheld response.

#![allow(clippy::expect_used)]

use super::faults::Fault;
use super::model::ModelS3;
use super::store_over;
use core::time::Duration;
use oqueue_core::{Clock, LEASE_SKEW_MS, LEASE_TTL_MS, ObjectStore, ObjectStoreLease, Timestamp};
use std::sync::Arc;

/// Wall-clock milliseconds as the paused runtime sees them.
#[derive(Debug)]
struct RuntimeClock(tokio::time::Instant);

impl Clock for RuntimeClock {
    fn now(&self) -> Timestamp {
        let elapsed = i64::try_from(self.0.elapsed().as_millis()).expect("a short test");
        Timestamp::from_millis(1_700_000_000_000 + elapsed).expect("a valid time")
    }
}

const fn millis(ms: i64) -> Duration {
    Duration::from_millis(ms.unsigned_abs())
}

fn lease(
    store: &Arc<dyn ObjectStore>,
    name: &str,
    clock: &Arc<RuntimeClock>,
) -> Arc<ObjectStoreLease> {
    Arc::new(ObjectStoreLease::new(
        Arc::clone(store),
        "meta/0",
        name,
        Arc::clone(clock) as Arc<dyn Clock>,
    ))
}

/// ⚠️ **The metastable case, reached by a pause**: halfway through its
/// lease the holder renews, and the renewal's response is withheld until
/// after its deadline. The holder stops acting at that deadline with the
/// renewal still in flight; when the renewal lands — past the deadline minus
/// the skew, where a successor may already have read the old expiry — it
/// does not move the deadline, so the holder stays fenced; and once the
/// renewal's own recorded expiry has passed, a successor takes over.
#[tokio::test(start_paused = true)]
async fn a_lease_paused_past_its_ttl_fences_its_holder() {
    let model = ModelS3::default();
    let store: Arc<dyn ObjectStore> = Arc::new(store_over(&model));
    let clock = Arc::new(RuntimeClock(tokio::time::Instant::now()));
    let holder = lease(&store, "holder", &clock);
    assert!(holder.acquire().await.expect("acquires"));

    // Renew at TTL/2; the response is withheld until TTL/2 + 5.5 s = TTL + 0.5 s.
    tokio::time::sleep(millis(LEASE_TTL_MS / 2)).await;
    model.inject(Fault::Pause {
        method: "PUT",
        holding: millis(LEASE_TTL_MS / 2 + LEASE_SKEW_MS / 2),
        remaining: 1,
    });
    let renewing = tokio::spawn({
        let holder = Arc::clone(&holder);
        async move { holder.renew().await }
    });

    tokio::time::sleep(millis(LEASE_TTL_MS / 2)).await;
    assert_eq!(model.faults_injected(), 1, "the renewal met the pause");
    assert!(!renewing.is_finished(), "and is still in flight");
    assert!(
        !holder.is_held(),
        "the holder fenced itself at its own deadline"
    );

    let _ = renewing.await.expect("the withheld renewal completes");
    assert!(
        !holder.is_held(),
        "a renewal that landed past the deadline minus the skew does not revive it"
    );

    // The late renewal did write its expiry (TTL/2 + TTL); past it plus the
    // skew, a successor takes the next term and the old holder learns it.
    tokio::time::sleep(millis(LEASE_TTL_MS / 2 + LEASE_SKEW_MS)).await;
    let successor = lease(&store, "successor", &clock);
    assert!(
        successor.acquire().await.expect("asks"),
        "a successor takes over"
    );
    assert!(successor.is_held());
    assert!(
        !holder.renew().await.expect("renews"),
        "the old holder stays out"
    );
}

/// The converse, so the pause is not what fences: a renewal halfway through
/// the lease, withheld for less than the skew, extends it past the original
/// deadline.
#[tokio::test(start_paused = true)]
async fn a_short_pause_does_not_cost_the_lease() {
    let model = ModelS3::default();
    let store: Arc<dyn ObjectStore> = Arc::new(store_over(&model));
    let clock = Arc::new(RuntimeClock(tokio::time::Instant::now()));
    let holder = lease(&store, "holder", &clock);
    assert!(holder.acquire().await.expect("acquires"));
    tokio::time::sleep(millis(LEASE_TTL_MS / 2)).await;
    model.inject(Fault::Pause {
        method: "PUT",
        holding: millis(LEASE_SKEW_MS / 2),
        remaining: 1,
    });
    assert!(holder.renew().await.expect("renews"));
    tokio::time::sleep(millis(LEASE_TTL_MS / 2 + LEASE_SKEW_MS)).await;
    assert!(
        holder.is_held(),
        "past the original deadline, still held: the renewal moved it"
    );
}
