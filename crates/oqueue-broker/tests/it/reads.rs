//! What one `Fetch` costs the *store*, whatever the client asks for.
//!
//! ⚠️ **Its own file because bytes and reads are different bounds, and only
//! one of them is arithmetic the client can see.** `budget.rs` asks how many
//! bytes come back; everything here asks how many GETs were issued to produce
//! them — and every defect this row shipped and had caught lived in the gap.
//! A read charged in bytes returned cannot bound the objects it downloaded to
//! return them; and a read the store refuses outright is charged nothing at
//! all, so no byte number bounds *that* GET either — `faults.rs` is where a
//! failure that did pull real bytes gets its own charge asserted.
//!
//! ⚠️ **These tests count `Operation::Get`, not records.** An assertion about
//! what came back is satisfied by a broker that fetched a hundred times more
//! than it returned, which is exactly the state three rounds of review found
//! this row in.

#![allow(clippy::expect_used)]
// ⚠️ `pub` here is `pub(crate)` in effect — see `roundtrip.rs`.
#![allow(unreachable_pub)]

use crate::budget::{repeated_fetch, single_partition_len};
use crate::roundtrip::{FETCH_VERSION, ask, body_of, fetch_now, framed, produce};
use crate::support::{Broker, broker};
use kafka_protocol::messages::fetch_request::{FetchPartition, FetchTopic};
use kafka_protocol::messages::{FetchRequest, FetchResponse};
use kafka_protocol::protocol::{Decodable, Encodable};
use oqueue_broker::Dispatcher;
use oqueue_codec::apikey::ApiKey;
use oqueue_core::Operation;
use std::sync::Arc;

/// ⚠️ **The budget stops the *reader*, not just the index lookup** —
/// `ADR-0022`, and the half `find_batches` cannot do. A history entry has no
/// byte range to price, so a page can name up to `MAX_BATCHES_PER_PAGE`
/// batches of unknown size; without a stop that learns each object's size as
/// it goes, a small `max_bytes` still costs sixty-four whole-object GETs and
/// returns all of them.
///
/// ⚠️ **Eight topics per bundle, and that is the whole test.** A single-topic
/// fixture is the one layout where a slice *is* its object, so bytes returned
/// and bytes fetched are the same number and a stop that measures the wrong
/// one still passes. With eight, a page of sixty-four entries can pull
/// sixty-four whole objects while returning an eighth of each — measured at
/// fifty-one GETs for a four-kilobyte `max_bytes` before the stop learned to
/// count what it had fetched.
#[tokio::test]
async fn a_small_budget_stops_the_reader_partway_through_a_history_page() {
    let names = ["a", "b", "c", "d", "e", "f", "g", "h"];
    let broker = broker(&names).await;
    let dispatcher = Dispatcher::new(Arc::clone(&broker.cluster));
    // Past the tail window, so the page is unpriceable history and the index
    // lookup cannot bound it.
    let batches = oqueue_core::TAIL_WINDOW_ENTRIES + 8;
    for _ in 0..batches {
        produce(&dispatcher, &broker, &names).await;
    }
    let unbounded = fetch_now(&dispatcher, &broker, "a", 0).await.responses[0].partitions[0]
        .records
        .as_ref()
        .map_or(0, bytes::Bytes::len);
    assert!(
        unbounded > 0,
        "the fixture returns something when unbounded"
    );
    let before = broker.store.counts().count(Operation::Get);

    // ⚠️ **Room for one *object*, expressed in the bytes the client can
    // see** — eight regions, because eight topics share every bundle. This is
    // the number that separates the two stops: measuring what was fetched
    // stops after the first whole object, measuring what was returned reads
    // eight of them to hand back the same eight regions. A budget of one
    // region cannot tell them apart, which is why the earlier version of this
    // test passed against the wrong one.
    let region = unbounded / batches;
    let one = i32::try_from(region * names.len() + 1).expect("a small page");
    let returned = repeated_fetch(&dispatcher, &broker, "a", 1, one).await;

    assert!(returned > 0, "the first batch comes back regardless");
    let spent = broker.store.counts().count(Operation::Get) - before;
    assert!(
        spent <= 2,
        "and the GETs stopped with it: {spent} for a page of sixty-four"
    );
}

/// ⚠️ **The bound does not depend on which of the client's two numbers is
/// smaller.** `allowance.bytes` is `min(partition_max_bytes, left)`, so a test
/// that only ever set `max_bytes` small would pin one arrangement and miss the
/// other — a client naming `partition_max_bytes = 1` against a full request
/// budget buys a GET for one byte. What bounds the reads is not the arithmetic
/// at all: it is that an object is fetched once per request.
#[tokio::test]
async fn a_tiny_per_partition_cap_does_not_buy_a_read_per_entry() {
    let broker = broker(&["orders"]).await;
    let dispatcher = Dispatcher::new(Arc::clone(&broker.cluster));
    produce(&dispatcher, &broker, &["orders"]).await;
    let before = broker.store.counts().count(Operation::Get);

    // The other arrangement: a generous request budget, a one-byte partition
    // cap, and two hundred entries naming the same partition.
    let response = capped_fetch(&dispatcher, &broker, &["orders"], 1 << 20, 1).await;

    let spent = broker.store.counts().count(Operation::Get) - before;
    assert_eq!(
        spent, 1,
        "one object behind every entry, so one GET: {spent}"
    );
    assert_eq!(response.responses.len(), 1);
}

/// ⚠️ **The budget is charged the bytes *fetched*, not the bytes returned.** A
/// history batch lives inside a bundle covering every partition one flush
/// wrote, so the slice handed back is a fraction of the download — and a
/// budget counting only the slice would let a read pull whole bundles off the
/// store to answer with far less.
///
/// ⚠️ **The observable is how many partitions get served.** Each partition is
/// capped at one region, so each returns one batch and *fetches* the whole
/// bundle behind it — four regions. Against a three-region request budget,
/// charging what was fetched serves one partition; charging what was returned
/// serves three.
#[tokio::test]
async fn the_budget_is_charged_what_a_read_fetched_not_what_it_returned() {
    let names = ["a", "b", "c", "d"];
    let broker = broker(&names).await;
    let dispatcher = Dispatcher::new(Arc::clone(&broker.cluster));
    // Past the tail window, so a read resolves through the object's footer and
    // pulls the whole bundle — all four topics' regions — for one topic's
    // slice.
    let flushes = oqueue_core::TAIL_WINDOW_ENTRIES + 4;
    for _ in 0..flushes {
        produce(&dispatcher, &broker, &names).await;
    }
    let one = single_partition_len(&dispatcher, &broker, "a").await / flushes;
    assert!(one > 0, "a region is some bytes");

    let response = capped_fetch(&dispatcher, &broker, &names, one * 3, one).await;

    let served = response
        .responses
        .iter()
        .flat_map(|topic| &topic.partitions)
        .filter(|p| p.records.as_ref().is_some_and(|r| !r.is_empty()))
        .count();
    assert_eq!(
        served, 1,
        "one partition's read fetched the whole four-region bundle, so a \
         three-region budget is gone — {served} served means only the slice \
         was charged"
    );
}

/// A `Fetch` over every named topic's partition 0, with the request budget and
/// the per-partition cap both chosen.
pub async fn capped_fetch(
    dispatcher: &Dispatcher,
    broker: &Broker,
    names: &[&str],
    max_bytes: usize,
    partition_max_bytes: usize,
) -> FetchResponse {
    let mut request = FetchRequest::default();
    request.max_wait_ms = 0;
    request.min_bytes = 1;
    request.max_bytes = i32::try_from(max_bytes).expect("a small budget");
    for name in names {
        let mut t = FetchTopic::default();
        t.topic_id = broker.cluster.topic_id(name).expect("a hosted topic");
        let mut p = FetchPartition::default();
        p.partition = 0;
        p.fetch_offset = 0;
        p.partition_max_bytes = i32::try_from(partition_max_bytes).expect("a small cap");
        t.partitions.push(p);
        request.topics.push(t);
    }
    let mut body = Vec::new();
    request.encode(&mut body, FETCH_VERSION).expect("encodes");
    let reply = ask(dispatcher, framed(ApiKey::Fetch, FETCH_VERSION, &body)).await;
    let mut rest = body_of(&reply, true);
    let response = FetchResponse::decode(&mut rest, FETCH_VERSION).expect("decodes");
    assert!(rest.is_empty());
    response
}
