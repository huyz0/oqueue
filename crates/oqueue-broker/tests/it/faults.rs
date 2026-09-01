//! What a read that *cannot* succeed costs the store.
//!
//! ⚠️ **Its own file because a failure is bounded by something else.** A
//! healthy read is bounded by bytes: it fetched something, so it is charged for
//! it, and `reads.rs` asserts that. A read the store refuses outright fetched
//! nothing and is charged nothing — deliberately, so one fault cannot blank
//! the healthy partitions behind it — which leaves it bounded by the object
//! cache and the failure cap instead. ⚠️ **Not every failure here is that
//! cheap**: `a_failed_parse_leaves_the_exemption_for_the_partition_behind_it`
//! below pulls a whole bundle from a store that answered honestly, and only
//! the parse after it fails — the budget is spent exactly as a successful
//! read would spend it, and what this file asserts for that case is the
//! *exemption* surviving, not a byte count of zero.
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

/// ⚠️ **A store that is failing is asked less, not more.** A read the store
/// refuses outright fetched nothing and is charged nothing — deliberately,
/// so one fault cannot blank the healthy partitions behind it — and the
/// object cache only stops the *same* object being asked for twice. Distinct
/// partitions in distinct bundles escape both, so a frame naming thirty-two of them against a sick
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
    // ⚠️ **A literal, not the constant under test** (`M3.36`). Asserting
    // `spent <= MAX_FAILED_FETCHES_PER_REQUEST` compares the measurement to
    // the very number it exists to pin: raise the constant to sixty-four and
    // the eight-distinct-failing-partitions fan-out this was written against
    // is restored, green. The value is held by `check-drift.sh`'s pin map, so
    // moving it is a diff someone reviews; the number here is what the *test*
    // requires, and the two disagreeing is the point of writing both.
    assert!(
        spent <= 2,
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

/// ⚠️ **One partition's fault does not blank the partitions behind it.** The
/// store refuses this partition's GET outright, so it fetched nothing and
/// charging it a share of the budget would spend the response on bytes
/// nobody has — and the healthy partitions after it would be framed `NONE`
/// with no records, which every client reads as "nothing was written". That
/// is doc 12 §4.6's silent wrongness produced by the bound written to
/// prevent it.
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
/// ⚠️ **The observable here is that the healthy partition behind it is served
/// at all**, which is `M3.39`'s half; the *charge* is pinned separately, by
/// `a_failed_read_spends_the_budget_but_not_the_exemption`, whose third
/// partition is what tells a spent budget from an unspent one. A two-partition
/// fixture cannot: once the exemption survives a failure, the second partition
/// reads either way.
#[tokio::test]
async fn a_failed_parse_leaves_the_exemption_for_the_partition_behind_it() {
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
    // ⚠️ **The bytes are gone and the *exemption* is not** (`M3.39`). The
    // budget is spent — the store handed over a whole bundle whatever the
    // reader could do with it — but a partition that returned no records has
    // not taken the one over-the-line read a response is allowed, so the
    // healthy partition behind it is still served. ⚠️ **This asserted the
    // opposite until `M3.39`**, which is what `M3.26`'s review recorded: a
    // corrupt first partition left a healthy second one framed `NONE` with no
    // records, and a response in which no partition makes progress is doc 12
    // §4.6's silent wrongness arriving through the mechanism built to prevent
    // a consumer parking at an offset forever.
    let behind = &response.responses[1].partitions[0];
    assert_eq!(behind.error_code, 0);
    assert!(
        behind.records.as_ref().is_some_and(|r| !r.is_empty()),
        "a spent budget must not cost the partition behind it its one batch"
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
/// partition that pulled one, whether or not it could use it — and the
/// partition behind it is still served, because a read that returned nothing
/// does not take the response's one over-the-line exemption (`M3.39`).
///
/// ⚠️ **The GET count discriminates the *charge*, and only that.** Two, not
/// three: the third partition is reachable only if the failed read's bytes
/// were never charged, which is `M3.26`'s half. ⚠️ **It does not discriminate
/// the exemption**, and this said it did until `M3.37` — with a two-batch
/// budget the second partition reads whether or not the exemption survived, so
/// `M3.39`'s half is pinned by the two assertions in
/// [`a_failed_parse_leaves_the_exemption_for_the_partition_behind_it`] and by
/// `target.rs`'s unit case.
#[tokio::test]
async fn a_failed_read_spends_the_budget_but_not_the_exemption() {
    let broker = broker(&["a", "b", "c"]).await;
    let dispatcher = Dispatcher::new(Arc::clone(&broker.cluster));
    produce(&dispatcher, &broker, &["a"]).await;
    let batch = single_partition_len(&dispatcher, &broker, "a").await;
    corrupt_every_object(&broker).await;
    produce(&dispatcher, &broker, &["b"]).await;
    produce(&dispatcher, &broker, &["c"]).await;
    let before = broker.store.counts().count(Operation::Get);

    // ⚠️ **Three partitions and a two-batch budget, and both numbers matter.**
    // With two partitions this could not tell the charge from its absence:
    // the exemption serves the second partition either way, so the count was
    // two in both worlds. The third partition is the discriminator — it is
    // reachable only if the failed read left its bytes unspent.
    let _ = capped_fetch(&dispatcher, &broker, &["a", "b", "c"], batch * 2, 1 << 20).await;

    let spent = broker.store.counts().count(Operation::Get) - before;
    assert_eq!(
        spent, 2,
        "the failed read spends the budget (so `c` is not reachable) and not \
         the exemption (so `b` still is): {spent}"
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

/// ⚠️ **Two failures, then a healthy partition — the boundary the cap actually
/// governs**, and the one nothing asserted. One test arms a single fault and
/// never reaches the cap; another fails every GET and leaves no healthy
/// partition to observe. What the cap *does* is refuse a partition whose object
/// is present and readable, without a GET, because two unrelated ones failed
/// earlier in the same frame.
///
/// ⚠️ **It is a real cost, and it is the trade the cap makes.** A consumer on
/// ten partitions where two hold reaped objects is told `OFFSET_NOT_AVAILABLE`
/// for the eight healthy ones, every poll. Refusing to *ask* is what stops a
/// frame's read rate against a sick store rising with the client's own
/// subscription fan-out — but a client author cannot predict it from the
/// protocol, and neither exempting a healthy partition nor removing the refusal
/// would have been detected before this.
#[tokio::test]
async fn two_failures_refuse_the_healthy_partition_behind_them_without_a_read() {
    let names = ["a", "b", "c"];
    let broker = broker(&names).await;
    let dispatcher = Dispatcher::new(Arc::clone(&broker.cluster));
    // Each in its own object, so the cache dedups nothing.
    for name in &names {
        produce(&dispatcher, &broker, &[name]).await;
    }
    let keys = broker.store.inner().keys();
    // Reap exactly the first two partitions' objects. `c`'s is untouched.
    let mut sorted = keys;
    sorted.sort();
    broker
        .store
        .delete(&sorted[..2])
        .await
        .expect("the fake deletes");
    let before = broker.store.counts().count(Operation::Get);

    let response = capped_fetch(&dispatcher, &broker, &names, 1 << 20, 1 << 20).await;

    let spent = broker.store.counts().count(Operation::Get) - before;
    assert_eq!(
        spent, 2,
        "the third partition must be refused without reaching the store: {spent}"
    );
    for topic in &response.responses {
        assert_ne!(
            topic.partitions[0].error_code, 0,
            "and every partition, healthy or not, is told so rather than \
             framed as caught up"
        );
    }
}
