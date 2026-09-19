//! A history fetch through a published manifest: `ADR-0042`'s read column.
//!
//! ⚠️ **The read count is the claim, not the correctness** (`M5.63`'s row).
//! That a fetch returns the right records is what every other test here
//! already asserts; what this file measures is what the store was asked for to
//! produce them. Cold, a history fetch is the manifest and the object. Once
//! the manifest is in the request's cache it is the object alone, however many
//! objects the run holds. Across a chain hop it is one GET more.
//!
//! ⚠️ **The manifest is folded in by the test, because nothing writes one
//! yet.** `M5.62` recorded that no production code emits `ManifestPublished`
//! and that the compaction which will is `M5.13`'s. So the fixture publishes
//! the manifest itself — the object into the store, the record into the index
//! — which is exactly the pair a compaction commit will land.

#![allow(clippy::expect_used)]
// ⚠️ `pub` here is `pub(crate)` in effect — see `roundtrip.rs`.
#![allow(unreachable_pub)]

use crate::roundtrip::{FETCH_VERSION, ask, body_of, framed, produce};
use crate::support::{Broker, broker};
use oqueue_broker::Dispatcher;
use oqueue_core::{
    ByteRange, CommitVersion, ManifestEntry, MaterializedIndex, MetadataEntry, MetadataRecord,
    ObjectKey, ObjectRef, ObjectStore, Offset, Operation, PartitionId, PartitionManifestBuilder,
    TAIL_WINDOW_ENTRIES, TopicId, parse_footer,
};
use std::sync::Arc;

/// How many objects the fixture writes.
///
/// ⚠️ **Five past the tail window, not one.** A manifest may cover history
/// and never the tail (`M5.62`), so the objects past the window are the only
/// ones a publication may name — and one test here needs a manifest larger
/// than an object, which takes more than a couple of entries.
///
/// ⚠️ **Past the tail window on purpose.** A manifest may cover history and
/// never the tail (`M5.62`), so with everything still in the window the only
/// publication the fold accepts is one covering nothing — and a test of the
/// read path through an empty manifest would be a test of nothing.
pub const OBJECTS: usize = TAIL_WINDOW_ENTRIES + 5;

/// How many records `produce` puts in one object, and so the stride between
/// the base offsets a manifest may end at.
///
/// ⚠️ **A manifest must meet a *boundary*** (`M5.62`) — some object's base
/// offset — so every `upto` here is a multiple of this rather than a number
/// chosen to read well.
pub const RECORDS_PER_OBJECT: i64 = 2;

pub fn topic() -> TopicId {
    TopicId::new("t").expect("a valid topic")
}

pub fn partition() -> PartitionId {
    PartitionId::new(0).expect("a valid partition")
}

pub fn gets(broker: &Broker) -> u64 {
    broker.store.counts().count(Operation::Get)
}

/// The fixture: `OBJECTS` one-batch objects in `t/0`, so the fold demotes all
/// but the last `TAIL_WINDOW_ENTRIES` into history.
pub async fn loaded() -> (Broker, Dispatcher) {
    loaded_on(&["t"]).await
}

/// The same, hosting several topics and writing every one of them into every
/// object — which is the layout a flush actually produces.
pub async fn loaded_on(topics: &[&'static str]) -> (Broker, Dispatcher) {
    let broker = broker(topics).await;
    let dispatcher = Dispatcher::new(Arc::clone(&broker.cluster));
    for _ in 0..OBJECTS {
        produce(&dispatcher, &broker, topics).await;
    }
    (broker, dispatcher)
}

/// Writes a manifest naming the objects holding `[0, upto)` and folds the
/// publication in, returning the manifest's key.
///
/// ⚠️ **Built from what the index itself holds**, so the byte ranges in the
/// manifest are the ranges a fetch would have used without it. A fixture that
/// invented them would be asserting that a fetch reads *some* bytes rather
/// than the ones the records are in.
pub async fn publish(broker: &Broker, upto: i64, previous: Option<ObjectKey>) -> ObjectKey {
    publish_range(broker, 0, upto, previous).await
}

/// The same, for a link of a chain that starts above zero.
pub async fn publish_range(
    broker: &Broker,
    from: i64,
    upto: i64,
    previous: Option<ObjectKey>,
) -> ObjectKey {
    let key = build_manifest(
        broker,
        Covering {
            on: &topic(),
            from,
            upto,
            previous,
            name: "manifest",
        },
    )
    .await;
    fold_publication(broker, &topic(), &key, upto);
    key
}

/// Writes the manifest object naming the objects holding `[from, upto)`.
///
/// ⚠️ **Separate from folding the publication in**, because the two are
/// separate facts: `upto` is what the *fold* admits and the entries are what
/// the *object* holds, and nothing makes one prove the other. A test of what a
/// reader does when they disagree needs to be able to make them disagree.
pub struct Covering<'a> {
    pub on: &'a TopicId,
    pub from: i64,
    pub upto: i64,
    pub previous: Option<ObjectKey>,
    pub name: &'a str,
}

pub async fn build_manifest(broker: &Broker, covering: Covering<'_>) -> ObjectKey {
    let Covering {
        on,
        from,
        upto,
        previous,
        name,
    } = covering;
    let upto = Offset::new(upto).expect("a valid offset");
    let mut builder = PartitionManifestBuilder::new();
    if let Some(previous) = previous {
        builder = builder.spilling_from(previous);
    }
    let start = Offset::new(from).expect("a valid offset");
    for batch in broker
        .index
        .find_batches(on, partition(), start, u64::MAX)
        .expect("a folded partition")
    {
        if batch.reference().base_offset() >= upto {
            break;
        }
        builder
            .push(entry_for(broker, on, batch.reference()).await)
            .expect("contiguous, in order");
    }
    let key =
        ObjectKey::new(format!("{}-{name}-{}", on.as_str(), upto.get())).expect("a valid key");
    broker
        .store
        .put(&key, builder.seal().expect("a sealed manifest"), None)
        .await
        .expect("a manifest lands");
    key
}

/// Folds the publication that points the index at `key`.
pub fn fold_publication(broker: &Broker, on: &TopicId, key: &ObjectKey, upto: i64) {
    let version = broker
        .index
        .applied_upto()
        .expect("the fixture committed")
        .get();
    broker
        .index
        .apply(&[MetadataEntry::new(
            CommitVersion::new(version + 1),
            MetadataRecord::ManifestPublished {
                topic: on.clone(),
                partition: partition(),
                manifest: key.clone(),
                upto: Offset::new(upto).expect("a valid offset"),
            },
        )])
        .expect("a manifest meeting the boundary is folded");
}

/// One manifest entry for an object the index names.
///
/// ⚠️ **The range comes from the object's own footer, because a history entry
/// has none** (`ADR-0022`) — which is the whole reason a manifest is worth
/// writing: it is where that resolution stops being a whole-object read per
/// fetch. A compaction gets these ranges from the objects it wrote; a fixture
/// over objects it did not write reads them back the one way there is.
pub async fn entry_for(broker: &Broker, on: &TopicId, reference: &ObjectRef) -> ManifestEntry {
    let whole = broker
        .store
        .get(reference.object(), ByteRange::Full)
        .await
        .expect("an object the index named");
    let regions = parse_footer(&whole, whole.len() as u64).expect("an object this broker wrote");
    let region = regions
        .iter()
        .find(|region| region.topic() == on && region.partition() == partition())
        .expect("the partition the index said was in it");
    ManifestEntry::new(
        reference.object().clone(),
        reference.base_offset(),
        reference.record_count(),
        region.bytes(),
    )
    .expect("a valid entry")
}

/// A `Fetch` naming its own `partition_max_bytes`, so a test can stop the read
/// after a chosen number of objects.
pub async fn bounded(
    dispatcher: &Dispatcher,
    broker: &Broker,
    offset: i64,
    max_bytes: i32,
) -> Vec<u8> {
    use kafka_protocol::messages::fetch_request::{FetchPartition, FetchTopic};
    use kafka_protocol::messages::{FetchRequest, FetchResponse};
    use kafka_protocol::protocol::{Decodable, Encodable};
    use oqueue_codec::apikey::ApiKey;

    let mut request = FetchRequest::default();
    request.max_wait_ms = 0;
    request.min_bytes = 1;
    let mut t = FetchTopic::default();
    t.topic_id = broker.cluster.topic_id("t").await.expect("a hosted topic");
    let mut p = FetchPartition::default();
    p.partition = 0;
    p.fetch_offset = offset;
    p.partition_max_bytes = max_bytes;
    t.partitions.push(p);
    request.topics.push(t);
    let mut body = Vec::new();
    request.encode(&mut body, FETCH_VERSION).expect("encodes");
    let reply = ask(dispatcher, framed(ApiKey::Fetch, FETCH_VERSION, &body)).await;
    let mut rest = body_of(&reply, true);
    let response = FetchResponse::decode(&mut rest, FETCH_VERSION).expect("decodes");
    response.responses[0].partitions[0]
        .records
        .as_ref()
        .map_or_else(Vec::new, |bytes| bytes.to_vec())
}

/// ⚠️ **A manifest holding more than it was published for serves nothing
/// twice.** The index names everything from `upto` up, so an entry above it is
/// named by both tiers — and a fetch returning those records twice is
/// indistinguishable, to every reader downstream, from a partition that
/// genuinely holds them twice. It is the shape a compaction produces whenever
/// the tail advances between building a manifest and publishing it, so the
/// reader checks rather than assuming.
#[tokio::test]
async fn a_manifest_reaching_past_its_boundary_serves_nothing_twice() {
    let (broker, dispatcher) = loaded().await;
    // The object names two; the publication claims one.
    let key = build_manifest(
        &broker,
        Covering {
            on: &topic(),
            from: 0,
            upto: RECORDS_PER_OBJECT * 2,
            previous: None,
            name: "reaching",
        },
    )
    .await;
    fold_publication(&broker, &topic(), &key, RECORDS_PER_OBJECT);

    let whole = bounded(&dispatcher, &broker, 0, 1 << 20).await;
    let second = bounded(&dispatcher, &broker, RECORDS_PER_OBJECT, 1).await;
    assert!(!second.is_empty(), "the second object is served at all");
    let occurrences = whole
        .windows(second.len())
        .filter(|window| *window == second.as_slice())
        .count();
    assert_eq!(occurrences, 1, "the second object's records appear once");
}

/// ⚠️ **The row's first acceptance: the same records, byte for byte.** A
/// manifest changes where a reader looks, never what it finds.
///
/// ⚠️ **Compared one object at a time, not one fetch at a time.** How many
/// batches a single fetch names is a paging question — the manifest tier and
/// the index tier each contribute a page, so a read that crosses the boundary
/// returns more batches than the same read did before, and comparing whole
/// responses would be asserting that the paging did not change rather than
/// that the records did not.
#[tokio::test]
async fn a_history_fetch_returns_the_same_records_through_a_manifest() {
    let (broker, dispatcher) = loaded().await;
    let covered = [0, RECORDS_PER_OBJECT];
    let mut before = Vec::new();
    for offset in covered {
        let records = bounded(&dispatcher, &broker, offset, 1).await;
        assert!(!records.is_empty(), "the fixture serves offset {offset}");
        before.push(records);
    }

    publish(&broker, RECORDS_PER_OBJECT * 2, None).await;
    for (which, offset) in covered.into_iter().enumerate() {
        assert_eq!(
            bounded(&dispatcher, &broker, offset, 1).await,
            before[which],
            "offset {offset}: the same bytes, read a different way"
        );
    }
}
