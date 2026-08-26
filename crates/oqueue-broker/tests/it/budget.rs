//! How many bytes one `Fetch` may return, whatever the client asks for.
//!
//! ⚠️ **Its own file because a budget is a different claim from a round
//! trip.** `roundtrip.rs` asks whether what went in comes back out;
//! everything here asks how much of it one response may carry — and the two
//! answers move independently, which is how `M3.14` shipped a correct round
//! trip on top of a bound that was not one.
//!
//! ⚠️ **What a fetch costs the *store* is `reads.rs`**, not here. Bytes
//! returned and GETs issued are separate bounds — a history batch is one
//! partition's region of a bundle covering every partition that flush wrote —
//! and every assertion in this file is satisfied by a broker that downloaded
//! far more than it handed back.
//!
//! ⚠️ **The sharp test is the one that names a partition twice.** Nothing in
//! the protocol dedups a request's partition list, so "the budget is the
//! request's" and "the request's, plus one free batch per partition" agree on
//! every request that names each partition once — and disagree by a factor of
//! two hundred on one that does not.

#![allow(clippy::expect_used)]
// ⚠️ `pub` here is `pub(crate)` in effect — see `roundtrip.rs`.
#![allow(unreachable_pub)]

use crate::roundtrip::{FETCH_VERSION, ask, body_of, fetch_now, framed, produce};
use crate::support::{Broker, broker};
use kafka_protocol::messages::fetch_request::{FetchPartition, FetchTopic};
use kafka_protocol::messages::{FetchRequest, FetchResponse};
use kafka_protocol::protocol::{Decodable, Encodable};
use oqueue_broker::Dispatcher;
use oqueue_codec::apikey::ApiKey;
use std::sync::Arc;

/// ⚠️ **The read budget is a megabyte, and a fetch spends it.** `find_batches`
/// prices what it names against `READ_BUDGET_BYTES`, so a budget that was
/// accidentally kilobytes would silently cut a fetch short — the client would
/// see fewer records and no error, and conclude nothing more had been written
/// until it polled again. Thirty batches is comfortably more than a couple of
/// kilobytes and comfortably less than a megabyte, so this fails on a wrong
/// budget in either direction.
#[tokio::test]
async fn one_fetch_returns_every_tail_batch_within_the_read_budget() {
    let broker = broker(&["orders"]).await;
    let dispatcher = Dispatcher::new(Arc::clone(&broker.cluster));
    let batches = 60;
    for _ in 0..batches {
        produce(&dispatcher, &broker, &["orders"]).await;
    }

    let response = fetch_now(&dispatcher, &broker, "orders", 0).await;

    let p = &response.responses[0].partitions[0];
    assert_eq!(p.error_code, 0);
    let records = p.records.as_ref().expect("records came back");
    assert!(
        records.len() > 4 * 1024,
        "the fixture must exceed a small-kilobyte budget to be evidence: {}",
        records.len()
    );
    let mut at = 0;
    let mut seen = 0;
    while at < records.len() {
        let header = oqueue_codec::batch::decode_batch_header(&records[at..]).expect("a batch");
        assert_eq!(header.base_offset, i64::from(seen) * 2);
        at += usize::try_from(header.batch_length).expect("small") + 12;
        seen += 1;
    }
    assert_eq!(seen, batches, "one fetch, every batch");
}

/// ⚠️ **The byte budget is the request's, spent across every partition it
/// names.** This is the bound `M3.14`'s review found missing: a per-partition
/// ceiling multiplied by a client-chosen partition count is not a ceiling, and
/// one frame could name enough partitions to turn a 1 MiB budget into
/// megabytes of object-storage reads concatenated in memory.
///
/// ⚠️ **The first partition still gets its records whatever the budget**, and
/// that exception is deliberate: a partition whose first batch exceeds the
/// allowance must stay readable, or a consumer parks at that offset forever
/// re-fetching nothing. What the budget bounds is everything *after* it.
#[tokio::test]
async fn one_requests_byte_budget_is_spent_across_all_of_its_partitions() {
    let names = ["a", "b", "c", "d"];
    let broker = broker(&names).await;
    let dispatcher = Dispatcher::new(Arc::clone(&broker.cluster));
    for _ in 0..3 {
        produce(&dispatcher, &broker, &names).await;
    }

    // A budget big enough for one partition's records and no more.
    let one = single_partition_len(&dispatcher, &broker, "a").await;
    let response = fetch_all(&dispatcher, &broker, &names, one).await;

    let returned = returned_bytes(&response);
    assert!(
        returned <= one * 2,
        "the whole response is bounded by the request's number, not by four \
         times it: {returned} bytes against a {one}-byte budget"
    );
    assert!(
        returned >= one,
        "and the first partition is served in full: {returned}"
    );
    for topic in &response.responses {
        assert_eq!(topic.partitions[0].error_code, 0);
    }
}

/// ⚠️ **And the bound does not move when the client names more partitions**,
/// which is the assertion the ratio above cannot make. A test calibrated to
/// four topics passes at exactly the value a per-partition exemption produces;
/// only comparing two partition counts separates "the budget is the request's"
/// from "the request's, plus one free batch each".
///
/// ⚠️ **Naming one partition repeatedly is the sharp version**, because nothing
/// dedups a request's partition list: two hundred entries for one partition is
/// a legal `Fetch` that a per-partition exemption answers with two hundred
/// whole batches for a `max_bytes` of one.
#[tokio::test]
async fn a_requests_byte_budget_does_not_grow_with_the_partitions_it_names() {
    let broker = broker(&["orders"]).await;
    let dispatcher = Dispatcher::new(Arc::clone(&broker.cluster));
    for _ in 0..3 {
        produce(&dispatcher, &broker, &["orders"]).await;
    }

    let once = repeated_fetch(&dispatcher, &broker, "orders", 1, 1).await;
    let many = repeated_fetch(&dispatcher, &broker, "orders", 200, 1).await;

    assert!(once > 0, "the one-entry request is served: {once}");
    assert_eq!(
        many, once,
        "two hundred entries for one partition must cost what one costs — a \
         per-partition exemption answers {many} bytes here for a one-byte budget"
    );
}

/// Total record bytes in a response.
fn returned_bytes(response: &FetchResponse) -> usize {
    response
        .responses
        .iter()
        .flat_map(|topic| &topic.partitions)
        .filter_map(|p| p.records.as_ref())
        .map(bytes::Bytes::len)
        .sum()
}

/// A `Fetch` naming one topic's partition 0 `entries` times, with the
/// request-level `max_bytes` chosen — a shape nothing in the protocol forbids.
pub async fn repeated_fetch(
    dispatcher: &Dispatcher,
    broker: &Broker,
    topic: &str,
    entries: usize,
    max_bytes: i32,
) -> usize {
    let mut request = FetchRequest::default();
    request.max_wait_ms = 0;
    request.min_bytes = 1;
    request.max_bytes = max_bytes;
    let mut t = FetchTopic::default();
    t.topic_id = broker.cluster.topic_id(topic).expect("a hosted topic");
    for _ in 0..entries {
        let mut p = FetchPartition::default();
        p.partition = 0;
        p.fetch_offset = 0;
        p.partition_max_bytes = 1 << 20;
        t.partitions.push(p);
    }
    request.topics.push(t);
    let mut body = Vec::new();
    request.encode(&mut body, FETCH_VERSION).expect("encodes");
    let reply = ask(dispatcher, framed(ApiKey::Fetch, FETCH_VERSION, &body)).await;
    let mut rest = body_of(&reply, true);
    let response = FetchResponse::decode(&mut rest, FETCH_VERSION).expect("decodes");
    assert!(rest.is_empty());
    returned_bytes(&response)
}

/// How many bytes one topic's partition returns when nothing bounds it.
pub async fn single_partition_len(dispatcher: &Dispatcher, broker: &Broker, name: &str) -> usize {
    fetch_now(dispatcher, broker, name, 0).await.responses[0].partitions[0]
        .records
        .as_ref()
        .expect("records")
        .len()
}

/// A `Fetch` naming every topic's partition 0, with the request-level
/// `max_bytes` chosen.
async fn fetch_all(
    dispatcher: &Dispatcher,
    broker: &Broker,
    names: &[&str],
    max_bytes: usize,
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
        p.partition_max_bytes = 1 << 20;
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

/// ⚠️ **A budget of zero still returns one batch.** This is the exemption's
/// whole reason: a client that asks for nothing — or whose allowance is spent
/// before the partition it cares about — must not be parked at that offset
/// forever, re-fetching and being told nothing is there. Kafka's own broker
/// makes the same exception.
#[tokio::test]
async fn a_budget_of_zero_still_returns_the_first_batch() {
    let broker = broker(&["orders"]).await;
    let dispatcher = Dispatcher::new(Arc::clone(&broker.cluster));
    produce(&dispatcher, &broker, &["orders"]).await;

    let returned = repeated_fetch(&dispatcher, &broker, "orders", 1, 0).await;

    assert!(
        returned > 0,
        "a zero allowance must not mean a consumer can never make progress"
    );
}

/// ⚠️ **And an *empty* partition does not consume the exemption.** A fetch
/// whose first partition is at its watermark returns no bytes from it; if that
/// counted as "the response has returned something", the partition that does
/// have records would be starved — the same livelock the exemption exists to
/// prevent, arriving through the partition that had nothing to give.
#[tokio::test]
async fn an_empty_partition_does_not_spend_the_one_batch_exemption() {
    let names = ["quiet", "busy"];
    let broker = broker(&names).await;
    let dispatcher = Dispatcher::new(Arc::clone(&broker.cluster));
    // Only the second topic has anything, and the first is walked first.
    produce(&dispatcher, &broker, &["busy"]).await;

    let mut request = FetchRequest::default();
    request.max_wait_ms = 0;
    request.min_bytes = 1;
    request.max_bytes = 0;
    for name in names {
        let mut t = FetchTopic::default();
        t.topic_id = broker.cluster.topic_id(name).expect("a hosted topic");
        let mut p = FetchPartition::default();
        p.partition = 0;
        p.fetch_offset = 0;
        p.partition_max_bytes = 1 << 20;
        t.partitions.push(p);
        request.topics.push(t);
    }
    let mut body = Vec::new();
    request.encode(&mut body, FETCH_VERSION).expect("encodes");
    let reply = ask(&dispatcher, framed(ApiKey::Fetch, FETCH_VERSION, &body)).await;
    let mut rest = body_of(&reply, true);
    let response = FetchResponse::decode(&mut rest, FETCH_VERSION).expect("decodes");

    assert!(
        returned_bytes(&response) > 0,
        "the busy partition must still be served: an empty first partition \
         returned no bytes, so it spent nothing"
    );
}
