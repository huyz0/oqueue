//! What a fetch through a manifest costs the *store*, and nothing about what
//! it returns.
//!
//! ⚠️ **Split from `manifest_reads.rs` along the concept** (`code-structure.md`
//! rule 18) once the pair reached the 500-line limit: that file asks whether a
//! manifest changes the records a fetch finds, and this asks how many GETs it
//! took to find them. Every defect `M5.63`'s first round found lived here —
//! a manifest read charged to no budget, and a tier that read one for a
//! partition with nothing left to spend.
//!
//! ⚠️ **These tests count `Operation::Get`, not records**, for the reason
//! `reads.rs` gives: an assertion about what came back is satisfied by a
//! broker that fetched a hundred times more than it returned.

#![allow(clippy::expect_used)]
// ⚠️ `pub` here is `pub(crate)` in effect — see `roundtrip.rs`.
#![allow(unreachable_pub)]

use crate::manifest_reads::{
    Covering, OBJECTS, RECORDS_PER_OBJECT, bounded, build_manifest, fold_publication, gets, loaded,
    loaded_on, partition, publish, publish_range, topic,
};
use crate::roundtrip::{FETCH_VERSION, ask, body_of, framed};
use crate::support::Broker;
use oqueue_broker::Dispatcher;
use oqueue_core::{ByteRange, MaterializedIndex, ObjectStore, Offset, TopicId};

/// ⚠️ **One GET once the manifest is held**, which is the third of the row's
/// three counts and the one a per-partition cache is worth having for. The
/// request's own `FetchedObjects` is that cache: a frame naming the same
/// partition at two offsets under one manifest reads the manifest once and
/// then pays for objects alone.
#[tokio::test]
async fn a_second_offset_under_a_held_manifest_is_one_get() {
    let (broker, dispatcher) = loaded().await;
    publish(&broker, RECORDS_PER_OBJECT * 2, None).await;

    let before = gets(&broker);
    let records = twice(&dispatcher, &broker, [0, RECORDS_PER_OBJECT]).await;
    assert_eq!(
        gets(&broker) - before,
        3,
        "the manifest once, then one object each"
    );
    assert_eq!(records.len(), 2, "both were answered");
    assert!(
        records.iter().all(|bytes| !bytes.is_empty()),
        "and answered with records"
    );
}
/// ⚠️ **A manifest claiming more than it holds costs no chain walk.** The
/// chain runs backwards in time, so a search that misses *above* a manifest's
/// entries has nothing older to find — following the link would spend a GET
/// per link to learn that. It is reachable because `upto` is what the *fold*
/// admitted and the entries are what the *object* holds, and nothing makes one
/// prove the other.
#[tokio::test]
async fn a_search_missing_above_a_manifest_does_not_walk_the_chain() {
    let (broker, dispatcher) = loaded().await;
    // Both objects are written before anything is folded, because a
    // publication removes from the index the very entries the next manifest
    // would be built out of.
    let older = build_manifest(
        &broker,
        Covering {
            on: &topic(),
            from: 0,
            upto: RECORDS_PER_OBJECT,
            previous: None,
            name: "older",
        },
    )
    .await;
    let newer = build_manifest(
        &broker,
        Covering {
            on: &topic(),
            from: 0,
            upto: RECORDS_PER_OBJECT,
            previous: Some(older),
            name: "claims-more-than-it-holds",
        },
    )
    .await;
    // ⚠️ **The claim is two objects up and the object names one.** Nothing
    // makes a writer prove otherwise: `upto` is what the fold admits and the
    // entries are what the object holds.
    fold_publication(&broker, &topic(), &newer, RECORDS_PER_OBJECT * 2);
    assert_eq!(
        broker
            .index
            .manifest(&topic(), partition())
            .map(|held| held.0),
        Some(newer),
        "the index points at the newest link"
    );

    let before = gets(&broker);
    bounded(&dispatcher, &broker, RECORDS_PER_OBJECT, 1).await;
    // The manifest, and the object the index names above it — and *not* the
    // older link, which is the third GET this guard exists to not spend.
    assert_eq!(
        gets(&broker) - before,
        2,
        "the manifest, and no walk back through a chain that cannot hold it"
    );
}
/// ⚠️ **A manifest is charged to the request, like every other GET it
/// makes.** A manifest is up to `PARTITION_MANIFEST_BYTES`, one per partition
/// a frame names and one more per chain link followed — so a tier reporting no
/// cost would let a client with a `max_bytes` of one pull megabytes off the
/// store, and nothing would cut the partitions behind it off. This is the
/// defect `M3.26` fixed on the history tier and `M3.37` on the tail's, in the
/// tier `M5.63` added.
///
/// ⚠️ **Two topics, because the bound this asserts is the *request*'s.** With
/// one partition a small budget still buys one object by the overshoot rule,
/// and the number is the same whether the manifest was charged or not.
#[tokio::test]
async fn a_manifest_is_charged_against_the_requests_budget() {
    let (broker, dispatcher) = loaded_on(&["t", "u"]).await;
    let mut manifest_len = 0;
    for name in ["t", "u"] {
        let on = TopicId::new(name).expect("a valid topic");
        let key = build_manifest(
            &broker,
            Covering {
                on: &on,
                from: 0,
                upto: RECORDS_PER_OBJECT * 4,
                previous: None,
                name: "manifest",
            },
        )
        .await;
        fold_publication(&broker, &on, &key, RECORDS_PER_OBJECT * 4);
        manifest_len = broker
            .store
            .get(&key, ByteRange::Full)
            .await
            .expect("the manifest")
            .len();
    }

    // ⚠️ **A budget of exactly one manifest**, which is larger than one object
    // here. Charged, the first partition's manifest spends the request on its
    // own: it takes one object by the overshoot rule and stops, and the second
    // partition reaches the store not at all. Uncharged, that budget buys the
    // first partition three objects before it notices — and the count says so.
    let before = gets(&broker);
    both(
        &dispatcher,
        &broker,
        i32::try_from(manifest_len).expect("a small manifest"),
    )
    .await;
    assert_eq!(
        gets(&broker) - before,
        2,
        "one partition's manifest and object, and nothing for the other"
    );
}
/// ⚠️ **The row's read counts, and what makes this a manifest read rather
/// than a per-object indirection.** Cold, a fetch stopping after one batch is
/// two GETs: the manifest and the object it names. A fetch crossing two
/// objects under the same manifest is three, not four — the manifest is read
/// once for the run.
#[tokio::test]
async fn a_cold_history_fetch_reads_the_manifest_once_for_the_run() {
    let (broker, dispatcher) = loaded().await;
    let key = publish(&broker, RECORDS_PER_OBJECT * 2, None).await;

    let before = gets(&broker);
    let one = bounded(&dispatcher, &broker, 0, 1).await;
    assert_eq!(gets(&broker) - before, 2, "the manifest and one object");

    // ⚠️ **The manifest's own bytes are in the budget**, so a budget that
    // crosses two objects has to carry them: the read stops on what it has
    // fetched, and the manifest is part of that. A test naming a constant
    // instead would be asserting that the manifest cost nothing.
    let manifest_len = broker
        .store
        .get(&key, ByteRange::Full)
        .await
        .expect("the manifest")
        .len();
    let before = gets(&broker);
    let two = bounded(
        &dispatcher,
        &broker,
        0,
        i32::try_from(manifest_len + one.len() + 1).expect("a small budget"),
    )
    .await;
    assert_eq!(gets(&broker) - before, 3, "the manifest and two objects");
    assert!(
        two.len() > one.len(),
        "and the second read really did cross into the second object"
    );
}
/// ⚠️ **Three across a chain hop.** A manifest that spilled names its
/// predecessor and the chain runs backwards in time, so an offset older than
/// the manifest the index points at costs one GET per link followed plus the
/// object — and the reader follows a link only when the search misses *below*,
/// never above, where the index itself has the answer.
#[tokio::test]
async fn a_fetch_across_a_chain_hop_is_one_get_more() {
    let (broker, dispatcher) = loaded().await;
    // The older link covers the first object; the one the index points at
    // covers the second and spills from it.
    let older = publish(&broker, RECORDS_PER_OBJECT, None).await;
    let newer = publish_range(
        &broker,
        RECORDS_PER_OBJECT,
        RECORDS_PER_OBJECT * 2,
        Some(older),
    )
    .await;
    assert_eq!(
        broker.index.manifest(&topic(), partition()),
        Some((
            newer,
            Offset::new(RECORDS_PER_OBJECT * 2).expect("a valid offset")
        )),
        "the index points at the newest link"
    );

    let before = gets(&broker);
    bounded(&dispatcher, &broker, 0, 1).await;
    assert_eq!(
        gets(&broker) - before,
        3,
        "the manifest, its predecessor, and the object"
    );
}
/// ⚠️ **A tail fetch is still one GET** — FR-13, and the half a manifest must
/// not cost. The index names the tail with its byte ranges, so a read there
/// never consults a manifest at all.
#[tokio::test]
async fn a_tail_fetch_is_still_one_get() {
    let (broker, dispatcher) = loaded().await;
    publish(&broker, RECORDS_PER_OBJECT * 2, None).await;
    let tail =
        i64::try_from(OBJECTS).expect("a small count") * RECORDS_PER_OBJECT - RECORDS_PER_OBJECT;

    let before = gets(&broker);
    let records = bounded(&dispatcher, &broker, tail, 1).await;
    assert_eq!(gets(&broker) - before, 1, "one ranged GET, no manifest");
    assert!(!records.is_empty(), "and it served the records");
}
/// ⚠️ **The gap between a manifest's end and the tail's start is answered by
/// whatever holds it, not by an error.** A manifest covers a prefix; what is
/// left in history above it is still the index's, and a fetch landing there
/// must not fall between the two mechanisms.
#[tokio::test]
async fn a_fetch_between_the_manifest_and_the_tail_is_answered() {
    let (broker, dispatcher) = loaded().await;
    // Covering the first object leaves the second in history, above the
    // manifest and below the tail.
    publish(&broker, RECORDS_PER_OBJECT, None).await;

    let before = gets(&broker);
    let records = bounded(&dispatcher, &broker, RECORDS_PER_OBJECT, 1).await;
    assert_eq!(gets(&broker) - before, 1, "the index named it on its own");
    assert!(!records.is_empty(), "and it is served rather than refused");
}
/// ⚠️ **A budget of zero still serves one batch, through the manifest too.** A
/// partition whose first batch is larger than the budget must still be
/// readable or the consumer parks at that offset forever re-fetching nothing —
/// Kafka's own broker makes the same exception, and a manifest tier that
/// skipped the read for a zero allowance would reinstate exactly that park for
/// every compacted partition.
#[tokio::test]
async fn a_zero_budget_still_reads_one_batch_through_the_manifest() {
    let (broker, dispatcher) = loaded().await;
    publish(&broker, RECORDS_PER_OBJECT * 2, None).await;

    let before = gets(&broker);
    let records = bounded(&dispatcher, &broker, 0, 0).await;
    assert_eq!(gets(&broker) - before, 2, "the manifest and one object");
    assert!(!records.is_empty(), "and the one batch came back");
}
/// A `Fetch` naming the same partition at two offsets in one frame, so a test
/// can see what the request's own cache saved.
async fn twice(dispatcher: &Dispatcher, broker: &Broker, offsets: [i64; 2]) -> Vec<Vec<u8>> {
    use kafka_protocol::messages::fetch_request::{FetchPartition, FetchTopic};
    use kafka_protocol::messages::{FetchRequest, FetchResponse};
    use kafka_protocol::protocol::{Decodable, Encodable};
    use oqueue_codec::apikey::ApiKey;

    let mut request = FetchRequest::default();
    request.max_wait_ms = 0;
    request.min_bytes = 1;
    let mut t = FetchTopic::default();
    t.topic_id = broker.cluster.topic_id("t").await.expect("a hosted topic");
    for offset in offsets {
        let mut p = FetchPartition::default();
        p.partition = 0;
        p.fetch_offset = offset;
        p.partition_max_bytes = 1;
        t.partitions.push(p);
    }
    request.topics.push(t);
    let mut body = Vec::new();
    request.encode(&mut body, FETCH_VERSION).expect("encodes");
    let reply = ask(dispatcher, framed(ApiKey::Fetch, FETCH_VERSION, &body)).await;
    let mut rest = body_of(&reply, true);
    let response = FetchResponse::decode(&mut rest, FETCH_VERSION).expect("decodes");
    response.responses[0]
        .partitions
        .iter()
        .map(|part| {
            part.records
                .as_ref()
                .map_or_else(Vec::new, |bytes| bytes.to_vec())
        })
        .collect()
}
/// A `Fetch` naming two topics' partition zero, under one request budget.
async fn both(dispatcher: &Dispatcher, broker: &Broker, max_bytes: i32) {
    use kafka_protocol::messages::FetchRequest;
    use kafka_protocol::messages::fetch_request::{FetchPartition, FetchTopic};
    use kafka_protocol::protocol::Encodable;
    use oqueue_codec::apikey::ApiKey;

    let mut request = FetchRequest::default();
    request.max_wait_ms = 0;
    request.min_bytes = 1;
    request.max_bytes = max_bytes;
    for name in ["t", "u"] {
        let mut t = FetchTopic::default();
        t.topic_id = broker.cluster.topic_id(name).await.expect("a hosted topic");
        let mut p = FetchPartition::default();
        p.partition = 0;
        p.fetch_offset = 0;
        p.partition_max_bytes = 1 << 20;
        t.partitions.push(p);
        request.topics.push(t);
    }
    let mut body = Vec::new();
    request.encode(&mut body, FETCH_VERSION).expect("encodes");
    ask(dispatcher, framed(ApiKey::Fetch, FETCH_VERSION, &body)).await;
}
