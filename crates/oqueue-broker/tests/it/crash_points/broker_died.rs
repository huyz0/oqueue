//! `CrashPoint::BetweenPutAndCommitBrokerDied`'s own machinery, split out
//! from `crash_points.rs` at the 500-line limit (`M10.30`).
//!
//! ⚠️ **Its own file rather than a trim**, because it genuinely is a
//! separate concept from the rest of that file: every other crash point
//! injects a fault into `support::broker`'s own cluster and reads that
//! cluster's state back; this one needs a store that can hang a `put` after
//! it writes, which means its own cluster, its own store, and its own
//! minimal produce/fetch wire encoding — see [`HangingStore`]'s own doc for
//! why that capability could not live in `oqueue-core`'s shared
//! `FakeObjectStore` instead. `crash_at` (in the parent module) is this
//! file's only caller.

use super::{Aftermath, POLL_BUDGET};
use oqueue_broker::{Cluster, Dispatcher, Handler as _, Seams, Sequencing, WriterId};
use oqueue_coordinator::Coordinator;
use oqueue_core::{
    BoxFuture, ByteRange, CoordinatorEpoch, FakeMaterializedIndex, FakeMetadataLog, ObjectKey,
    ObjectMeta, ObjectStore, Precondition, Result,
};
use std::sync::Arc;

/// An [`ObjectStore`] that writes through to `inner` and then never resolves
/// `put` at all — the object lands, and the caller hears nothing, ever.
///
/// ⚠️ **Test-only, deliberately not a `FakeObjectStore` fault** (`M10.30`).
/// A first attempt added a `hang_after_put` field beside
/// `crash_after_put_before_ack` in `oqueue-core`'s own `FaultConfig` —
/// symmetrical with that field, and wrong for a reason specific to *hanging*:
/// a mutant that made the new field's guard unconditionally true hung
/// *every* `put` in the workspace, not just the one this test arms, and
/// `cargo mutants` runs a package's whole test suite as one process that
/// does not stop at the first binary's failure — so the mutant surfaced only
/// as every other `block_on`-driven `put` test timing out together, which
/// `cargo mutants` reports as `TIMEOUT` rather than `MISSED`, and
/// `check-mutants.sh` only reads `MISSED` lines against `baselines/mutants.txt`
/// — a `TIMEOUT` is a *tool* failure to that gate, not an arguable survivor.
/// `crash_after_put_before_ack`'s own mutants have no such problem, because
/// returning `Err` is instant; only a *hang*-shaped fault takes its whole
/// binary down with it. A type that exists only in this test tree carries
/// none of that risk — `cargo mutants` mutates library and binary sources,
/// never `tests/`, so nothing here is ever a mutation target at all.
#[derive(Debug)]
struct HangingStore<S> {
    inner: S,
}

impl<S: ObjectStore> ObjectStore for HangingStore<S> {
    fn get<'a>(&'a self, key: &'a ObjectKey, range: ByteRange) -> BoxFuture<'a, Result<Vec<u8>>> {
        self.inner.get(key, range)
    }

    fn put<'a>(
        &'a self,
        key: &'a ObjectKey,
        payload: Vec<u8>,
        precondition: Option<Precondition>,
    ) -> BoxFuture<'a, Result<ObjectMeta>> {
        Box::pin(async move {
            self.inner.put(key, payload, precondition).await?;
            core::future::pending().await
        })
    }

    fn delete<'a>(&'a self, keys: &'a [ObjectKey]) -> BoxFuture<'a, Result<()>> {
        self.inner.delete(keys)
    }

    /// ⚠️ **Delegated, and deliberately not made to hang.** This double hangs
    /// the *ack* after a durable `put`, which is the crash point it exists
    /// for; nothing in the broker streams an object, and a hang here would be
    /// a crash point nothing reaches.
    fn open_multipart<'a>(
        &'a self,
        key: &'a ObjectKey,
    ) -> BoxFuture<'a, Result<Box<dyn oqueue_core::MultipartWriter<'a> + 'a>>> {
        self.inner.open_multipart(key)
    }
}

/// Builds its own cluster, produces into it, and aborts the handler task the
/// moment the object lands — before the commit ever runs, and before any
/// reply is built. The broker dying mid-flight, not the client going away.
///
/// ⚠️ **Its own cluster, not `support::broker`'s** — `HangingStore` needs to
/// be `broker`'s store to make `crash_at`'s shared read-back
/// (`broker.store.inner()`, `broker.log.inner()`, a `fetch` through
/// `broker.cluster`) see this crash point's world, and `Broker`'s `store`
/// field is the concrete `CountingObjectStore<FakeObjectStore>` every other
/// point relies on for call counting — changing its type to fit one point
/// would touch every test file that names it. Duplicating the small setup
/// `support::broker` does, with `HangingStore` wrapping the store instead, is
/// the narrower change; `crash_at` reads this function's own return value
/// directly rather than `broker`'s state for this one point.
///
/// ⚠️ **A genuine suspension point exists here, unlike `AfterCommitBeforeAck`'s
/// window** (see that function's own doc, in the parent module):
/// `HangingStore::put` writes through and then never resolves, so the
/// handler task is parked exactly there — `flush` has not reached the
/// commit, let alone built an answer — and aborting the task is the closest
/// a single process gets to modelling its own death mid-request.
type MiniCluster = (
    Arc<Cluster>,
    Arc<FakeMetadataLog>,
    Arc<HangingStore<oqueue_core::FakeObjectStore>>,
    tokio::task::JoinHandle<()>,
);

/// The setup half of [`crash_the_broker_after_the_put_lands`], split out
/// because that function outgrew fifty lines the moment this and the
/// abort-and-read-back logic both needed a place — `code-structure.md`'s
/// design signal, and here it is a real seam: building the cluster is one
/// concern, driving and observing one request through it is another.
async fn mini_cluster_with_hanging_put() -> MiniCluster {
    let log = Arc::new(FakeMetadataLog::new());
    let index = Box::new(FakeMaterializedIndex::new());
    let (coordinator, serving, reader) = Coordinator::open(
        Arc::clone(&log) as Arc<dyn oqueue_core::MetadataLog>,
        index,
        CoordinatorEpoch::new(1),
        Arc::new(oqueue_core::FakeClock::new()),
    )
    .await
    .expect("an empty log opens");
    let store = Arc::new(HangingStore {
        inner: oqueue_core::FakeObjectStore::new(),
    });
    let shared: Arc<dyn ObjectStore> = Arc::clone(&store) as Arc<dyn ObjectStore>;
    let sequencing = Sequencing::new(coordinator, reader);
    let cluster = Arc::new(
        Cluster::new(
            "h",
            1,
            sequencing,
            Seams {
                store: shared,
                group_coordinator: Arc::new(oqueue_core::FakeGroupCoordinator::new()),
                group_metadata_log: Arc::new(oqueue_core::FakeGroupMetadataLog::new()),
            },
            &WriterId::mint(),
        )
        .await
        .expect("a minted identity is a usable key component, and an empty log opens"),
    );
    // `M4.15a` moved offset replay to a background task; wait for it so
    // this fixture is not flaky against a request that reaches the server
    // before the (near-instant, empty-log) replay task gets scheduled.
    cluster.wait_until_replayed().await;
    cluster.ensure_topic("orders").await;
    let serving_task = tokio::spawn(serving.run());
    (cluster, log, store, serving_task)
}

pub(super) async fn crash_the_broker_after_the_put_lands() -> Aftermath {
    let (cluster, log, store, serving_task) = mini_cluster_with_hanging_put().await;

    let dispatcher = Arc::new(Dispatcher::new(Arc::clone(&cluster)));
    let frame = produce_frame_against(&cluster, "orders").await;
    let task = {
        let dispatcher = Arc::clone(&dispatcher);
        tokio::spawn(async move { dispatcher.handle(frame).await })
    };

    let mut landed = false;
    for _ in 0..POLL_BUDGET {
        if !store.inner.is_empty() {
            landed = true;
            break;
        }
        tokio::task::yield_now().await;
    }
    task.abort();
    let _ = task.await;
    assert!(landed, "the put never landed within {POLL_BUDGET} yields");

    let object_written = !store.inner.is_empty();
    let position_journalled = !log.is_empty();
    let readable = fetch_against(&dispatcher, &cluster, "orders", 0).await;
    let records_readable = readable.responses[0].partitions[0]
        .records
        .as_ref()
        .is_some_and(|r| !r.is_empty());

    serving_task.abort();
    Aftermath {
        error_code: None,
        object_written,
        position_journalled,
        records_readable,
    }
}

/// A produce frame for `topic`, built against `cluster` directly rather than
/// through `roundtrip::produce_frame`'s `&Broker` — this crash point's own
/// cluster is not a `Broker` (see `HangingStore`'s own doc for why).
async fn produce_frame_against(cluster: &Cluster, topic: &'static str) -> Vec<u8> {
    use kafka_protocol::messages::ProduceRequest;
    use kafka_protocol::messages::produce_request::{PartitionProduceData, TopicProduceData};
    use kafka_protocol::protocol::Encodable;

    let mut request = ProduceRequest::default();
    request.acks = -1;
    let mut t = TopicProduceData::default();
    t.topic_id = cluster.topic_id(topic).await.expect("a hosted topic");
    let mut p = PartitionProduceData::default();
    p.index = 0;
    p.records = Some(bytes::Bytes::from(crate::support::golden_batch(&[
        topic.as_bytes(),
        b"world",
    ])));
    t.partition_data.push(p);
    request.topic_data.push(t);
    let mut body = Vec::new();
    request
        .encode(&mut body, crate::roundtrip::PRODUCE_VERSION)
        .expect("encodes");
    crate::roundtrip::framed(
        oqueue_codec::apikey::ApiKey::Produce,
        crate::roundtrip::PRODUCE_VERSION,
        &body,
    )
}

/// The `Fetch` counterpart of [`produce_frame_against`].
async fn fetch_against(
    dispatcher: &Dispatcher,
    cluster: &Cluster,
    topic: &str,
    offset: i64,
) -> kafka_protocol::messages::FetchResponse {
    use kafka_protocol::messages::FetchRequest;
    use kafka_protocol::messages::fetch_request::{FetchPartition, FetchTopic};
    use kafka_protocol::protocol::{Decodable, Encodable};

    let mut request = FetchRequest::default();
    request.max_wait_ms = 0;
    request.min_bytes = 1;
    let mut t = FetchTopic::default();
    t.topic_id = cluster.topic_id(topic).await.expect("a hosted topic");
    let mut p = FetchPartition::default();
    p.partition = 0;
    p.fetch_offset = offset;
    p.partition_max_bytes = 1 << 20;
    t.partitions.push(p);
    request.topics.push(t);
    let mut body = Vec::new();
    request
        .encode(&mut body, crate::roundtrip::FETCH_VERSION)
        .expect("encodes");
    let reply = crate::roundtrip::ask(
        dispatcher,
        crate::roundtrip::framed(
            oqueue_codec::apikey::ApiKey::Fetch,
            crate::roundtrip::FETCH_VERSION,
            &body,
        ),
    )
    .await;
    let mut rest = crate::roundtrip::body_of(&reply, true);
    let response =
        kafka_protocol::messages::FetchResponse::decode(&mut rest, crate::roundtrip::FETCH_VERSION)
            .expect("decodes");
    assert!(rest.is_empty());
    response
}
