//! NFR-10/NFR-11 (`ADR-0049` point 5): a node's memory is a function of the
//! topics it serves, never of how many topics exist.

use crate::catalog::SyntheticCatalog;
use kafka_protocol::messages::produce_request::{PartitionProduceData, TopicProduceData};
use kafka_protocol::messages::{ProduceRequest, ProduceResponse, RequestHeader};
use kafka_protocol::protocol::{Decodable, Encodable};
use oqueue_broker::{Cluster, Dispatcher, Handler, HandlerResponse, Sequencing, WriterId};
use oqueue_codec::apikey::ApiKey;
use oqueue_coordinator::Coordinator;
use oqueue_core::{
    CoordinatorEpoch, FakeMaterializedIndex, FakeMetadataLog, FakeObjectStore, ObjectStore,
};
use std::sync::Arc;

/// How many topics each node serves: the same indices at both catalog sizes.
const ACTIVE: u64 = 50;

/// How far apart the two nodes' live bytes may be.
///
/// ⚠️ **Measured equal to the byte** in each of six runs (56,548 or 56,651 B,
/// the same at both sizes within a run), so this is headroom for a dependency
/// that later grows a buffer at a slightly different moment, not for anything
/// observed. It is 1 KiB: a node that held
/// even one byte per catalog topic would differ by 9,000,000 B between the two
/// sizes, and one per thousand topics by 9,000 B — both far outside it.
const TOLERANCE: i128 = 1024;

const PRODUCE_VERSION: i16 = 13;

/// A node over `catalog`, with the coordinator loop serving it.
async fn node(catalog: SyntheticCatalog) -> (Arc<Cluster>, tokio::task::JoinHandle<()>) {
    let log: Arc<dyn oqueue_core::MetadataLog> = Arc::new(FakeMetadataLog::new());
    let (coordinator, serving, reader) = Coordinator::open(
        log,
        Box::new(FakeMaterializedIndex::new()),
        CoordinatorEpoch::new(1),
        Arc::new(oqueue_core::FakeClock::new()),
    )
    .await
    .expect("an empty log opens");
    let store: Arc<dyn ObjectStore> = Arc::new(FakeObjectStore::new());
    let cluster = Cluster::new(
        "h",
        1,
        Sequencing::new(coordinator, reader),
        oqueue_broker::Seams {
            store,
            group_coordinator: Arc::new(oqueue_core::FakeGroupCoordinator::new()),
            group_metadata_log: Arc::new(oqueue_core::FakeGroupMetadataLog::new()),
        },
        &WriterId::mint(),
    )
    .await
    .expect("a minted identity is a usable key component, and an empty log opens")
    .with_catalog(Arc::new(catalog));
    cluster.wait_until_replayed().await;
    (Arc::new(cluster), tokio::spawn(serving.run()))
}

/// One small batch to partition 0 of `name`, through the real produce path.
async fn produce(dispatcher: &Dispatcher, cluster: &Cluster, name: &str) {
    let mut partition = PartitionProduceData::default();
    partition.index = 0;
    partition.records = Some(bytes::Bytes::from(batch()));
    let mut topic = TopicProduceData::default();
    topic.topic_id = cluster.topic_id(name).await.expect("a synthetic topic");
    topic.partition_data.push(partition);
    let mut request = ProduceRequest::default();
    request.acks = -1;
    request.topic_data.push(topic);

    let mut frame = Vec::new();
    let mut header = RequestHeader::default();
    header.request_api_key = ApiKey::Produce.as_i16();
    header.request_api_version = PRODUCE_VERSION;
    header
        .encode(
            &mut frame,
            ApiKey::Produce.request_header_version(PRODUCE_VERSION),
        )
        .expect("header encodes");
    request
        .encode(&mut frame, PRODUCE_VERSION)
        .expect("encodes");
    let HandlerResponse::Reply(reply) = dispatcher.handle(frame).await else {
        panic!("expected a reply");
    };
    let mut rest = &reply[5..];
    let response = ProduceResponse::decode(&mut rest, PRODUCE_VERSION).expect("decodes");
    let code = response.responses[0].partition_responses[0].error_code;
    assert_eq!(code, 0, "{name}: the produce was acknowledged");
}

/// One record, encoded by the dependency's own encoder (`ADR-0017`).
fn batch() -> Vec<u8> {
    use kafka_protocol::records::{
        Compression, Record, RecordBatchEncoder, RecordEncodeOptions, TimestampType,
    };
    let record = Record {
        transactional: false,
        control: false,
        partition_leader_epoch: 0,
        producer_id: -1,
        producer_epoch: -1,
        timestamp_type: TimestampType::Creation,
        offset: 0,
        sequence: 0,
        delete_horizon: false,
        timestamp: 1_700_000_000_000,
        key: None,
        value: Some(bytes::Bytes::from_static(b"hello")),
        headers: kafka_protocol::indexmap::IndexMap::default(),
    };
    let mut buf = bytes::BytesMut::new();
    let options = RecordEncodeOptions {
        version: 2,
        compression: Compression::None,
    };
    RecordBatchEncoder::encode(&mut buf, [&record], &options).expect("encodes");
    buf.to_vec()
}

/// The live bytes a node over a `size`-topic catalog holds after serving the
/// same [`ACTIVE`] topics: resolved by name, then produced to.
async fn live_bytes_serving(size: u64) -> i128 {
    let before = crate::live_bytes();
    let (cluster, serving) = node(SyntheticCatalog::new(size)).await;
    let dispatcher = Dispatcher::new(Arc::clone(&cluster));
    for index in 0..ACTIVE {
        let name = SyntheticCatalog::name(index);
        assert_eq!(cluster.partition_count(&name).await, Some(1), "{name}");
        produce(&dispatcher, &cluster, &name).await;
    }
    // Measured with everything the node holds still alive.
    let held = crate::live_bytes() - before;
    drop(dispatcher);
    serving.abort();
    drop(cluster);
    held
}

#[tokio::test]
async fn node_memory_is_flat_as_the_catalog_grows_tenfold() {
    let _serial = crate::serial().await;
    assert_eq!(
        size_of::<SyntheticCatalog>(),
        size_of::<u64>(),
        "the fixture holds its size and nothing else, so it is O(1) at any size"
    );
    // Warm-up: the runtime's and the allocator's one-time growth lands here,
    // not in whichever size is measured first.
    let _ = live_bytes_serving(1_000).await;

    let small = live_bytes_serving(1_000_000).await;
    let large = live_bytes_serving(10_000_000).await;
    println!("node live bytes: 1M topics {small}, 10M topics {large}");
    assert!(
        (small - large).abs() <= TOLERANCE,
        "a node serving {ACTIVE} topics holds {small} B over 1M topics and {large} B over \
         10M: memory must not grow with the catalog (tolerance {TOLERANCE} B)"
    );
}
