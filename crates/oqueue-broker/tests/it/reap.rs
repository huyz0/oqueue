//! Hazard H4 on the wire: what a fetch does when the store has reaped an
//! object the index still names.
//!
//! ⚠️ **Its own file because "was it deleted" is a different question from
//! "what may it cost".** `budget.rs` bounds a request's work; everything here
//! is about an answer that must never be an empty partition — doc 12 §4.6's
//! silent wrongness, where a successful poll returning nothing reads to every
//! client as "I am caught up" while records it had not seen are deleted
//! underneath it.
//!
//! ⚠️ **Nothing in `M3` deletes anything**, so every fixture here reaches past
//! the broker and deletes through the fake directly. That is exactly the race
//! `M5`'s reaper makes ordinary.

#![allow(clippy::expect_used)]

use crate::budget::repeated_fetch;
use crate::roundtrip::{fetch_now, produce};
use crate::support::broker;
use oqueue_broker::Dispatcher;
use oqueue_core::{ObjectStore, Operation};
use std::sync::Arc;

/// ⚠️ **Hazard H4: a 404 is never "end of log".** Object ids are never reused,
/// so an object the index named and the store does not have was **reaped** —
/// those offsets are gone, not unwritten. A reader that treated the miss as
/// the end would tell a consumer it was caught up while records it had not
/// read were being deleted underneath it: a successful poll returning nothing,
/// which is the silent wrongness doc 12 §4.6 names.
///
/// ⚠️ **And it is counted.** A nonzero rate means `M5`'s deletion delay is too
/// short, and a number nobody can read is a number nobody will act on.
#[tokio::test]
async fn an_object_the_index_still_names_but_the_store_has_reaped_is_out_of_range() {
    let broker = broker(&["orders"]).await;
    let dispatcher = Dispatcher::new(Arc::clone(&broker.cluster));
    produce(&dispatcher, &broker, &["orders"]).await;
    assert_eq!(broker.cluster.reaped_reads(), 0, "nothing reaped yet");
    let before = broker.store.counts().count(Operation::Get);
    // Delete behind the index's back — a reaper racing a stale index.
    let keys = broker.store.inner().keys();
    assert!(!keys.is_empty(), "the produce wrote something to delete");
    broker.store.delete(&keys).await.expect("the fake deletes");

    let response = fetch_now(&dispatcher, &broker, "orders", 0).await;

    let p = &response.responses[0].partitions[0];
    assert_eq!(
        p.error_code,
        kafka_protocol::error::ResponseError::OffsetOutOfRange.code(),
        "reaped is out of range, never an empty partition"
    );
    assert!(p.records.as_ref().is_none_or(bytes::Bytes::is_empty));
    assert_eq!(
        p.high_watermark, 2,
        "and the client still hears where the log claims to end"
    );
    assert_eq!(broker.cluster.reaped_reads(), 1, "counted once");
    // ⚠️ **One GET, and no retry** — measured rather than asserted. In this
    // broker the index a fetch reads *is* the coordinator's, and nothing
    // removes entries from it, so a second read would consult provably
    // identical state and pay a second GET for the same answer. `M7`'s
    // follower is where a refresh becomes real work.
    assert_eq!(
        broker.store.counts().count(Operation::Get) - before,
        1,
        "a 404 here is conclusive, so it is not paid for twice"
    );
}

/// ⚠️ **A reap partway through a page keeps what was already read.** The
/// records in hand are at offsets that *are* in range; discarding them to
/// answer `OFFSET_OUT_OF_RANGE` would tell a consumer that an offset it can be
/// served is gone, and a client resetting to `latest` would skip the very
/// records this broker had just read. The missing object becomes the *first*
/// batch of the next fetch, where out-of-range is the honest answer.
#[tokio::test]
async fn a_reap_partway_through_a_page_still_returns_what_came_before_it() {
    let broker = broker(&["orders"]).await;
    let dispatcher = Dispatcher::new(Arc::clone(&broker.cluster));
    for _ in 0..3 {
        produce(&dispatcher, &broker, &["orders"]).await;
    }
    // Three produces are three objects; take the last one only. Keys are
    // zero-padded per writer, so lexicographic order is write order.
    let mut keys = broker.store.inner().keys();
    keys.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    let last = keys.pop().expect("three objects were written");
    broker
        .store
        .delete(std::slice::from_ref(&last))
        .await
        .expect("the fake deletes");

    let response = fetch_now(&dispatcher, &broker, "orders", 0).await;

    let p = &response.responses[0].partitions[0];
    assert_eq!(
        p.error_code, 0,
        "offset 0 is readable, so the answer is records — not out of range"
    );
    let records = p.records.as_ref().expect("the surviving batches");
    let header = oqueue_codec::batch::decode_batch_header(records).expect("a batch");
    assert_eq!(header.base_offset, 0, "starting where the client asked");
    assert_eq!(
        broker.cluster.reaped_reads(),
        1,
        "and the reap is counted even though the page ended quietly"
    );
}

/// ⚠️ **And the same for a partition the store has reaped**, where the counter
/// must not be driven to any number a client chooses either.
#[tokio::test]
async fn a_reaped_partition_cannot_be_repeated_for_free() {
    let broker = broker(&["orders"]).await;
    let dispatcher = Dispatcher::new(Arc::clone(&broker.cluster));
    produce(&dispatcher, &broker, &["orders"]).await;
    let keys = broker.store.inner().keys();
    broker.store.delete(&keys).await.expect("the fake deletes");
    let before = broker.store.counts().count(Operation::Get);

    let _ = repeated_fetch(&dispatcher, &broker, "orders", 200, 1).await;

    let spent = broker.store.counts().count(Operation::Get) - before;
    assert_eq!(
        spent, 1,
        "one GET for the object behind all two hundred entries, not two \
         hundred: {spent}"
    );
    assert_eq!(
        broker.cluster.reaped_reads(),
        1,
        "and the alarm counts the *object*, not the entries naming it — an \
         alarm whose value a client picks is not a signal anyone can act on"
    );
}
