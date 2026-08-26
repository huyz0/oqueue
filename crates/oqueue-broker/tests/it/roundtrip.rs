//! What `M3.14` is for: a produce and a fetch that go all the way through.
//!
//! ⚠️ **Through the public seam only** — `Dispatcher` as a `Handler`, framed
//! request bytes in, response bytes out. That is what a client does, and it is
//! the only shape that can catch a handler wired to the wrong cluster, a
//! dispatch arm that never awaits, or an offset that is right inside the
//! coordinator and wrong on the wire.
//!
//! Three claims live here rather than in a unit test, because each spans the
//! write path, the store and the read path at once: FR-11's ordering, FR-32's
//! one PUT for N topics, and FR-12's zero GETs at the high watermark.

#![allow(clippy::expect_used)]
// ⚠️ `pub` here is `pub(crate)` in effect — `main.rs` is the only root of this
// test binary. `unreachable_pub` and `redundant_pub_crate` each refuse what the
// other asks for, and neither can tell a test binary's shared module from a
// library's.
#![allow(unreachable_pub)]
#![allow(clippy::redundant_pub_crate)]

use crate::support::{Broker, broker, golden_batch};
use kafka_protocol::messages::fetch_request::{FetchPartition, FetchTopic};
use kafka_protocol::messages::produce_request::{PartitionProduceData, TopicProduceData};
use kafka_protocol::messages::{
    FetchRequest, FetchResponse, ProduceRequest, ProduceResponse, RequestHeader,
};
use kafka_protocol::protocol::{Decodable, Encodable};
use oqueue_broker::{Dispatcher, Handler, HandlerResponse};
use oqueue_codec::apikey::ApiKey;
use oqueue_core::Operation;
use std::sync::Arc;

const PRODUCE_VERSION: i16 = 13;
pub(crate) const FETCH_VERSION: i16 = 13;

/// A full request frame: header at the API's header version, then `body`.
pub(crate) fn framed(api_key: ApiKey, version: i16, body: &[u8]) -> Vec<u8> {
    let mut frame = Vec::new();
    let mut header = RequestHeader::default();
    header.request_api_key = api_key.as_i16();
    header.request_api_version = version;
    header.correlation_id = 7;
    header
        .encode(&mut frame, api_key.request_header_version(version))
        .expect("header encodes");
    frame.extend_from_slice(body);
    frame
}

/// The response body, with the header skipped.
pub(crate) fn body_of(reply: &[u8], flexible: bool) -> &[u8] {
    &reply[if flexible { 5 } else { 4 }..]
}

pub(crate) async fn ask(dispatcher: &Dispatcher, frame: Vec<u8>) -> Vec<u8> {
    match dispatcher.handle(frame).await {
        HandlerResponse::Reply(out) => out,
        other => panic!("expected a reply, got {other:?}"),
    }
}

/// Produces one batch into each named topic's partition 0, in one request.
///
/// ⚠️ **Each topic's records say which topic they are.** A bundle holds every
/// topic's bytes in one payload, so records that were identical across topics
/// would let a read that resolved the *wrong* region still look right. The
/// name rides in the payload so that mistake is visible.
pub(crate) async fn produce(
    dispatcher: &Dispatcher,
    broker: &Broker,
    topics: &[&'static str],
) -> ProduceResponse {
    let mut request = ProduceRequest::default();
    request.acks = -1;
    for name in topics {
        let mut t = TopicProduceData::default();
        t.topic_id = broker.cluster.topic_id(name).expect("a hosted topic");
        let mut p = PartitionProduceData::default();
        p.index = 0;
        p.records = Some(bytes::Bytes::from(golden_batch(&[
            name.as_bytes(),
            b"world",
        ])));
        t.partition_data.push(p);
        request.topic_data.push(t);
    }
    let mut body = Vec::new();
    request.encode(&mut body, PRODUCE_VERSION).expect("encodes");
    let reply = ask(dispatcher, framed(ApiKey::Produce, PRODUCE_VERSION, &body)).await;
    let mut rest = body_of(&reply, true);
    let response = ProduceResponse::decode(&mut rest, PRODUCE_VERSION).expect("decodes");
    assert!(rest.is_empty());
    response
}

async fn fetch(
    dispatcher: &Dispatcher,
    broker: &Broker,
    topic: &str,
    offset: i64,
) -> FetchResponse {
    fetch_now(dispatcher, broker, topic, offset).await
}

/// A `Fetch` that will not wait: one look, one answer.
pub(crate) async fn fetch_now(
    dispatcher: &Dispatcher,
    broker: &Broker,
    topic: &str,
    offset: i64,
) -> FetchResponse {
    let mut request = FetchRequest::default();
    request.max_wait_ms = 0;
    request.min_bytes = 1;
    let mut t = FetchTopic::default();
    t.topic_id = broker.cluster.topic_id(topic).expect("a hosted topic");
    let mut p = FetchPartition::default();
    p.partition = 0;
    p.fetch_offset = offset;
    p.partition_max_bytes = 1 << 20;
    t.partitions.push(p);
    request.topics.push(t);
    let mut body = Vec::new();
    request.encode(&mut body, FETCH_VERSION).expect("encodes");
    let reply = ask(dispatcher, framed(ApiKey::Fetch, FETCH_VERSION, &body)).await;
    let mut rest = body_of(&reply, true);
    let response = FetchResponse::decode(&mut rest, FETCH_VERSION).expect("decodes");
    assert!(rest.is_empty());
    response
}

/// ⚠️ **The milestone's own sentence, on the wire**: a record is acknowledged
/// only after it is in object storage and its position is committed, and what
/// comes back out is what went in, in order, at the offsets the ack named.
#[tokio::test]
async fn a_produce_then_fetch_round_trip_is_ordered_end_to_end() {
    let broker = broker(&["orders"]).await;
    let dispatcher = Dispatcher::new(Arc::clone(&broker.cluster));

    let mut bases = Vec::new();
    for _ in 0..3 {
        let response = produce(&dispatcher, &broker, &["orders"]).await;
        let p = &response.responses[0].partition_responses[0];
        assert_eq!(p.error_code, 0, "the produce was acknowledged");
        bases.push(p.base_offset);
    }
    assert_eq!(bases, [0, 2, 4], "gap-free, in order, two records each");

    let response = fetch(&dispatcher, &broker, "orders", 0).await;
    let p = &response.responses[0].partitions[0];
    assert_eq!(p.error_code, 0);
    assert_eq!(p.high_watermark, 6);
    let records = p.records.as_ref().expect("three batches came back");

    // Every batch, in offset order, each stamped with what the commit gave it.
    let mut at = 0;
    for expected in bases {
        let batch = &records[at..];
        let header = oqueue_codec::batch::decode_batch_header(batch).expect("a batch");
        assert_eq!(header.base_offset, expected);
        let coverage = oqueue_codec::batch::crc_coverage(batch).expect("spans");
        assert_eq!(
            oqueue_checksum::crc32c(coverage),
            oqueue_codec::batch::stored_crc(batch).expect("crc"),
            "the offset stamp recomputes no checksum"
        );
        at += usize::try_from(header.batch_length).expect("small") + 12;
    }
    assert_eq!(at, records.len(), "nothing else rode along");
}

/// ⚠️ **FR-32, end to end.** One request spanning four topics is one PUT.
/// `M3.13` made this a property of the bundled format; here it is a property
/// of the broker, counted at the store the composition root would hand it.
#[tokio::test]
async fn one_produce_spanning_four_topics_issues_exactly_one_put() {
    let names = ["orders", "payments", "shipments", "returns"];
    let broker = broker(&names).await;
    let dispatcher = Dispatcher::new(Arc::clone(&broker.cluster));

    let response = produce(&dispatcher, &broker, &names).await;

    assert_eq!(response.responses.len(), 4);
    for topic in &response.responses {
        assert_eq!(topic.partition_responses[0].error_code, 0);
        assert_eq!(
            topic.partition_responses[0].base_offset, 0,
            "each partition's offset line is its own"
        );
    }
    assert_eq!(
        broker.store.counts().count(Operation::Put),
        1,
        "four topics, one object — this ratio is the cost model"
    );

    // And every topic's records are readable out of that one object.
    for name in names {
        let response = fetch(&dispatcher, &broker, name, 0).await;
        let p = &response.responses[0].partitions[0];
        assert_eq!(p.error_code, 0, "{name}");
        assert_eq!(p.high_watermark, 2, "{name}");
        assert!(
            p.records.as_ref().is_some_and(|r| !r.is_empty()),
            "{name}: one PUT that lost three topics would still count as one"
        );
    }
}

/// ⚠️ **Hazard H2, on the wire: produce then immediately consume, in one
/// session, never sees an empty partition.** Doc 12 §4.6 calls this the second
/// staleness hazard and the reason it matters is that the failure is silent —
/// a successful poll returning no records, which every client reads as
/// "nothing was written". The produce's ack carries a watermark, the session
/// remembers it, and the next fetch on that connection will not answer until
/// the index has folded that far.
///
/// ⚠️ **What this does *not* prove is that the session mechanism works**, and
/// saying so is the point: in a single-node broker the coordinator folds
/// before it acks, so this passes on that ordering alone — deleting the
/// session plumbing entirely leaves it green. It is here because it is the
/// claim a client makes, and because the day the ordering stops holding this
/// is what notices. The mechanism itself is constrained by
/// `session.rs`'s own tests, which set a promise the index has *not* reached.
#[tokio::test]
async fn a_produce_followed_immediately_by_a_fetch_sees_its_own_records() {
    let broker = broker(&["orders"]).await;
    let dispatcher = Dispatcher::new(Arc::clone(&broker.cluster));

    for round in 0..5_i64 {
        let response = produce(&dispatcher, &broker, &["orders"]).await;
        let p = &response.responses[0].partition_responses[0];
        assert_eq!(p.error_code, 0);
        assert_eq!(p.base_offset, round * 2);

        let response = fetch_now(&dispatcher, &broker, "orders", round * 2).await;
        let p = &response.responses[0].partitions[0];
        assert_eq!(p.error_code, 0, "round {round}");
        assert_eq!(p.high_watermark, (round + 1) * 2, "round {round}");
        assert!(
            p.records.as_ref().is_some_and(|r| !r.is_empty()),
            "round {round}: a producer's own consumer must never see an empty \
             partition — that is the silent wrongness H2 names"
        );
    }
}

/// ⚠️ **FR-12: a fetch at the high watermark issues zero GETs.** The idle poll
/// is what every consumer runs most of the time, and it must cost an index
/// lookup and nothing else — object storage charges per request.
#[tokio::test]
async fn a_fetch_at_the_high_watermark_issues_no_gets() {
    let broker = broker(&["orders"]).await;
    let dispatcher = Dispatcher::new(Arc::clone(&broker.cluster));
    produce(&dispatcher, &broker, &["orders"]).await;
    let before = broker.store.counts().count(Operation::Get);

    let response = fetch(&dispatcher, &broker, "orders", 2).await;

    let p = &response.responses[0].partitions[0];
    assert_eq!(p.error_code, 0);
    assert_eq!(p.high_watermark, 2);
    assert!(p.records.as_ref().is_none_or(bytes::Bytes::is_empty));
    assert_eq!(
        broker.store.counts().count(Operation::Get),
        before,
        "the idle poll must not touch object storage"
    );
}

/// ⚠️ **Past the tail window, a read still works** — and costs the footer
/// resolution the index no longer holds a byte range for. `TAIL_WINDOW_ENTRIES`
/// is 128, so 130 produces put the oldest batches in the history tier, where
/// the object's own footer is what says where the records are.
///
/// ⚠️ **Two topics per object, deliberately.** A bundle holding one region
/// would be matched by any predicate that got the region lookup wrong; two
/// means the read has to pick the right one, which is the whole of what the
/// footer is for.
#[tokio::test]
async fn a_read_of_history_resolves_through_the_object_s_own_footer() {
    let names = ["orders", "payments"];
    let broker = broker(&names).await;
    let dispatcher = Dispatcher::new(Arc::clone(&broker.cluster));
    let over = oqueue_core::TAIL_WINDOW_ENTRIES + 2;
    for _ in 0..over {
        produce(&dispatcher, &broker, &names).await;
    }

    for name in names {
        let response = fetch(&dispatcher, &broker, name, 0).await;
        let p = &response.responses[0].partitions[0];
        assert_eq!(p.error_code, 0, "{name}: a demoted entry is still readable");
        assert_eq!(
            p.high_watermark,
            i64::try_from(over).expect("small") * 2,
            "{name}: the watermark counts every record, tier or no tier"
        );
        let records = p.records.as_ref().expect("the oldest batch came back");
        let header = oqueue_codec::batch::decode_batch_header(records).expect("a batch");
        assert_eq!(header.base_offset, 0, "{name}: the first record produced");
        // ⚠️ **This topic's own bytes**, not its neighbour's. Both topics are
        // in one object, and only the footer says which range is whose — a
        // resolution that picked the other region would satisfy every
        // assertion above and hand a consumer another topic's records.
        assert!(
            records
                .windows(name.len())
                .any(|window| window == name.as_bytes()),
            "{name}: the records came from the wrong region"
        );
        let other = names.iter().find(|n| **n != name).expect("two topics");
        assert!(
            !records
                .windows(other.len())
                .any(|window| window == other.as_bytes()),
            "{name}: {other}'s records rode along"
        );
    }
}
