//! What a read that *cannot* succeed costs the store.
//!
//! ⚠️ **Its own file because a failure is bounded by something else.** A
//! healthy read is bounded by bytes: it fetched something, so it is charged for
//! it, and `reads.rs` asserts that. A read that fetched nothing is charged
//! nothing — deliberately, so one fault cannot blank the healthy partitions
//! behind it — which leaves it bounded by the object cache and the failure cap
//! instead, and nothing here can be stated in bytes at all.
//!
//! ⚠️ **Both tiers, and separately.** A tail read fails in the stamp and a
//! history read in the footer: two call sites, which have to charge and count
//! the failure each on their own. A fixture that exercises one leaves the other
//! pinned by nothing, and every one of these tests names which tier it is in.

#![allow(clippy::expect_used)]
// ⚠️ `pub` here is `pub(crate)` in effect — see `roundtrip.rs`.
#![allow(unreachable_pub)]

use crate::budget::{repeated_fetch, single_partition_len};
use crate::reads::capped_fetch;
use crate::roundtrip::produce;
use crate::support::{Broker, broker};
use oqueue_broker::Dispatcher;
use oqueue_core::{FaultConfig, ObjectStore, Operation, StormKind};
use std::sync::Arc;

/// ⚠️ **A store that is failing is asked less, not more.** A failed read
/// fetched nothing and is charged nothing — deliberately, so one fault cannot
/// blank the healthy partitions behind it — and the object cache only stops
/// the *same* object being asked for twice. Distinct partitions in distinct
/// bundles escape both, so a frame naming thirty-two of them against a sick
/// store bought thirty-two GETs for a `max_bytes` of one: the read rate rising
/// with the client's subscription fan-out, precisely when the store is least
/// able to serve it, and repeating on every poll.
#[tokio::test]
async fn a_failing_store_is_not_asked_once_per_partition_named() {
    let names = ["a", "b", "c", "d", "e", "f", "g", "h"];
    let broker = broker(&names).await;
    let dispatcher = Dispatcher::new(Arc::clone(&broker.cluster));
    // One flush per topic, so every partition lives in a *different* object
    // and the cache has nothing to dedup.
    for name in &names {
        produce(&dispatcher, &broker, &[name]).await;
    }
    let before = broker.store.counts().count(Operation::Get);
    broker.store.inner().set_faults(FaultConfig {
        storm: Some((StormKind::Transient, u32::MAX)),
        ..FaultConfig::default()
    });

    let response = capped_fetch(&dispatcher, &broker, &names, 1 << 20, 1).await;

    let spent = broker.store.counts().count(Operation::Get) - before;
    assert!(
        spent <= oqueue_broker::MAX_FAILED_FETCHES_PER_REQUEST.into(),
        "eight distinct failing partitions must not buy eight reads: {spent}"
    );
    // ⚠️ **And every one of them still says so.** Refusing to ask the store is
    // not licence to answer an empty partition: a client that reads `NONE`
    // with no records concludes it is caught up, which is the silent wrongness
    // doc 12 §4.6 names.
    for topic in &response.responses {
        assert_ne!(
            topic.partitions[0].error_code, 0,
            "a partition nobody read must not be framed as caught up"
        );
    }
}

/// ⚠️ **One partition's fault does not blank the partitions behind it.** A
/// read that failed fetched nothing, so charging it a share of the budget
/// would spend the response on bytes nobody has — and the healthy partitions
/// after it would be framed `NONE` with no records, which every client reads
/// as "nothing was written". That is doc 12 §4.6's silent wrongness produced
/// by the bound written to prevent it.
#[tokio::test]
async fn one_partitions_fault_does_not_starve_the_healthy_ones_behind_it() {
    let names = ["a", "b", "c", "d"];
    let broker = broker(&names).await;
    let dispatcher = Dispatcher::new(Arc::clone(&broker.cluster));
    produce(&dispatcher, &broker, &names).await;
    // Exactly one GET fails: the first partition's.
    broker.store.inner().set_faults(FaultConfig {
        storm: Some((StormKind::Transient, 1)),
        ..FaultConfig::default()
    });

    let response = capped_fetch(&dispatcher, &broker, &names, 1 << 20, 1 << 20).await;

    let faulted = &response.responses[0].partitions[0];
    assert_eq!(
        faulted.error_code,
        kafka_protocol::error::ResponseError::OffsetNotAvailable.code(),
        "the partition that faulted says so"
    );
    for topic in response.responses.iter().skip(1) {
        let p = &topic.partitions[0];
        assert_eq!(p.error_code, 0);
        assert!(
            p.records.as_ref().is_some_and(|r| !r.is_empty()),
            "a healthy partition must not be blanked by somebody else's fault"
        );
    }
}

/// ⚠️ **A request's *reads* are bounded, not only its bytes.** A partition
/// whose read fails returns nothing, so charging only the bytes returned would
/// leave it free — and a client repeating a failing partition would turn one
/// frame into as many object-storage reads as it has entries. Measured here:
/// two hundred entries for one partition must cost what one costs.
///
/// ⚠️ **A store fault is the reachable version.** Nothing deletes objects in
/// `M3`, so a brownout is how a read fails today; a reaper makes it ordinary.
#[tokio::test]
async fn a_failing_partition_cannot_be_repeated_for_free() {
    let broker = broker(&["orders"]).await;
    let dispatcher = Dispatcher::new(Arc::clone(&broker.cluster));
    produce(&dispatcher, &broker, &["orders"]).await;
    let before = broker.store.counts().count(Operation::Get);
    broker.store.inner().set_faults(FaultConfig {
        storm: Some((StormKind::Transient, u32::MAX)),
        ..FaultConfig::default()
    });

    let returned = repeated_fetch(&dispatcher, &broker, "orders", 200, 1).await;

    assert_eq!(returned, 0, "every read failed, so nothing came back");
    let spent = broker.store.counts().count(Operation::Get) - before;
    assert!(
        spent <= 2,
        "two hundred entries for one failing partition must not cost two \
         hundred reads: {spent}"
    );
}

/// ⚠️ **A read that fetched a whole bundle and then could not parse it is
/// charged what it fetched.** The store was healthy and the bytes are gone off
/// it; only the object disagreed with the index that named it. Charged zero —
/// which it was until `M3.26` — a frame naming K malformed bundles pulls K
/// whole objects for a `max_bytes` of one, bounded by neither the budget nor
/// the failure cap.
///
/// ⚠️ **The observable is the healthy partition behind it.** With the charge,
/// a budget of one bundle is spent by the partition that fetched one; without
/// it, the budget is untouched and the second partition is served.
#[tokio::test]
async fn a_read_that_fetched_and_then_failed_to_parse_still_spends_the_budget() {
    let broker = broker(&["a", "b"]).await;
    let dispatcher = Dispatcher::new(Arc::clone(&broker.cluster));
    // Past the tail window and one topic per flush, so each partition resolves
    // through its own object's footer.
    for _ in 0..(oqueue_core::TAIL_WINDOW_ENTRIES + 4) {
        produce(&dispatcher, &broker, &["a"]).await;
    }
    produce(&dispatcher, &broker, &["b"]).await;
    let bundle = corrupt_the_oldest(&broker).await;

    let response = capped_fetch(&dispatcher, &broker, &["a", "b"], bundle, 1 << 20).await;

    assert_ne!(
        response.responses[0].partitions[0].error_code, 0,
        "the partition whose object will not parse says so"
    );
    let behind = &response.responses[1].partitions[0];
    assert!(
        behind.records.as_ref().is_none_or(bytes::Bytes::is_empty),
        "the bundle it pulled was the response's whole budget, so nothing is \
         left for the partition behind it"
    );
}

/// ⚠️ **And a parse failure counts against the failure cap.** The store
/// reported nothing wrong, so `FetchedObjects` counted nothing — which left
/// this the one failure mode escaping both bounds. Eight partitions in eight
/// unparsable objects must cost what two do.
///
/// ⚠️ **Every partition reads from *history* here**, and that is the whole
/// fixture: a tail read fails in `rewrite_base_offset` and a history read in
/// `parse_footer`, two call sites that have to count the failure separately.
/// A tail-only fixture leaves the second one pinned by nothing.
#[tokio::test]
async fn objects_that_will_not_parse_are_bounded_like_any_other_failure() {
    let names = ["a", "b", "c", "d", "e", "f", "g", "h"];
    let broker = broker(&names).await;
    let dispatcher = Dispatcher::new(Arc::clone(&broker.cluster));
    for name in &names {
        for _ in 0..(oqueue_core::TAIL_WINDOW_ENTRIES + 2) {
            produce(&dispatcher, &broker, &[name]).await;
        }
    }
    for key in broker.store.inner().keys() {
        broker
            .store
            .inner()
            .put(&key, b"not a bundle".to_vec(), None)
            .await
            .expect("the fake overwrites");
    }
    let before = broker.store.counts().count(Operation::Get);

    let response = capped_fetch(&dispatcher, &broker, &names, 1 << 20, 1 << 20).await;

    let spent = broker.store.counts().count(Operation::Get) - before;
    assert!(
        spent <= 2,
        "eight unparsable objects must not buy eight reads: {spent}"
    );
    for topic in &response.responses {
        assert_ne!(
            topic.partitions[0].error_code, 0,
            "and no partition is framed as caught up"
        );
    }
}

/// The oldest stored object, overwritten with bytes that are not a bundle.
/// Returns what it used to be worth, in bytes, so a caller can size a budget
/// against it.
async fn corrupt_the_oldest(broker: &Broker) -> usize {
    let mut keys = broker.store.inner().keys();
    keys.sort();
    let key = keys.first().expect("the fixture stored something").clone();
    let was = broker
        .store
        .inner()
        .get(&key, oqueue_core::ByteRange::Full)
        .await
        .expect("it is there")
        .len();
    broker
        .store
        .inner()
        .put(&key, vec![0u8; was], None)
        .await
        .expect("the fake overwrites");
    was
}

/// ⚠️ **One bad object is one failure, however many partitions read it.** The
/// cap counts *distinct objects the store could not usefully answer for* —
/// `FetchedObjects::get` increments only on a miss for exactly that reason —
/// and four topics sharing one corrupt bundle are one miss and three cache
/// hits. Counted per read instead, a single corrupt object trips a cap of two
/// by itself and refuses every healthy partition behind it, on every poll, for
/// as long as the object stays corrupt.
#[tokio::test]
async fn one_corrupt_object_read_by_many_partitions_is_one_failure() {
    let shared = ["a", "b", "c", "d"];
    let broker = broker(&["a", "b", "c", "d", "e"]).await;
    let dispatcher = Dispatcher::new(Arc::clone(&broker.cluster));
    // One bundle per flush covering all four, pushed past the tail window so
    // every one of them resolves through that bundle's footer — the same
    // object, at the same range, so the second reader is a cache hit.
    for _ in 0..(oqueue_core::TAIL_WINDOW_ENTRIES + 4) {
        produce(&dispatcher, &broker, &shared).await;
    }
    corrupt_the_oldest(&broker).await;
    // And one healthy partition, in an object of its own, behind them.
    produce(&dispatcher, &broker, &["e"]).await;

    let response = capped_fetch(
        &dispatcher,
        &broker,
        &["a", "b", "c", "d", "e"],
        1 << 20,
        1 << 20,
    )
    .await;

    for topic in response.responses.iter().take(4) {
        assert_ne!(
            topic.partitions[0].error_code, 0,
            "the partitions whose bundle will not parse say so"
        );
    }
    let healthy = &response.responses[4].partitions[0];
    assert_eq!(
        healthy.error_code, 0,
        "a partition in a different, healthy object must not be refused \
         because three others read the same bad one"
    );
    assert!(healthy.records.as_ref().is_some_and(|r| !r.is_empty()));
}

/// ⚠️ **And the tail tier charges and counts too.** A tail read's range *is*
/// its batch, so on the success path what it fetched and what it returns are
/// the same number — and on the failure path they are not, which is the whole
/// point: the bytes came off the store and the records did not. Reported as
/// zero, a read that pulled a whole batch and could not stamp it costs the
/// request nothing, and a frame naming sixty-four such partitions issues
/// sixty-four GETs against a `max_bytes` of one.
///
/// ⚠️ **Two separate claims, two separate observables**, and both are GET
/// counts: the failure *cap* stops the third partition being read at all, and
/// the *charge* stops the second one being read on a spent budget.
#[tokio::test]
async fn a_tail_read_that_could_not_be_stamped_is_charged_and_counted() {
    let broker = broker(&["a", "b", "c"]).await;
    let dispatcher = Dispatcher::new(Arc::clone(&broker.cluster));
    // Two objects, each one topic's, both still inside the tail window — so
    // both are read as ranges rather than resolved through a footer.
    produce(&dispatcher, &broker, &["a"]).await;
    produce(&dispatcher, &broker, &["b"]).await;
    corrupt_every_object(&broker).await;
    // And a healthy partition behind them, in an object written afterwards.
    produce(&dispatcher, &broker, &["c"]).await;
    let before = broker.store.counts().count(Operation::Get);

    let response = capped_fetch(&dispatcher, &broker, &["a", "b", "c"], 1 << 20, 1 << 20).await;

    let spent = broker.store.counts().count(Operation::Get) - before;
    assert!(
        spent <= 2,
        "two unstampable tail objects are two failures, and the third \
         partition is refused without a read: {spent}"
    );
    for topic in &response.responses {
        assert_ne!(
            topic.partitions[0].error_code, 0,
            "and none of them is framed as caught up"
        );
    }
}

/// ⚠️ **The charge, on its own**, with one failure rather than two so the cap
/// is not what is being measured. A budget of one batch is spent by the
/// partition that pulled one, whether or not it could use it.
#[tokio::test]
async fn a_spent_budget_stops_the_partition_behind_an_unstampable_tail_read() {
    let broker = broker(&["a", "b"]).await;
    let dispatcher = Dispatcher::new(Arc::clone(&broker.cluster));
    produce(&dispatcher, &broker, &["a"]).await;
    let batch = single_partition_len(&dispatcher, &broker, "a").await;
    corrupt_every_object(&broker).await;
    produce(&dispatcher, &broker, &["b"]).await;
    let before = broker.store.counts().count(Operation::Get);

    let _ = capped_fetch(&dispatcher, &broker, &["a", "b"], batch, 1 << 20).await;

    let spent = broker.store.counts().count(Operation::Get) - before;
    assert_eq!(
        spent, 1,
        "the failed read pulled the whole budget off the store, so nothing is \
         left for the partition behind it: {spent}"
    );
}

/// Every stored object, overwritten with zeros of the same length — bytes a
/// store returns happily and neither tier can parse.
async fn corrupt_every_object(broker: &Broker) {
    for key in broker.store.inner().keys() {
        let was = broker
            .store
            .inner()
            .get(&key, oqueue_core::ByteRange::Full)
            .await
            .expect("it is there")
            .len();
        broker
            .store
            .inner()
            .put(&key, vec![0u8; was], None)
            .await
            .expect("the fake overwrites");
    }
}
