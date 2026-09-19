//! Degraded startup (`M6.10`, `M6.md` task 14): a node whose store is
//! unreachable at boot starts, answers what it can, and serves writes once the
//! store is back — with no restart.

#![allow(clippy::expect_used)]

use crate::roundtrip::{PRODUCE_VERSION, ask, body_of, framed};
use crate::support::golden_batch;
use kafka_protocol::messages::produce_request::{PartitionProduceData, TopicProduceData};
use kafka_protocol::messages::{ApiVersionsRequest, ProduceRequest, ProduceResponse};
use kafka_protocol::protocol::{Decodable, Encodable};
use oqueue_broker::{Cluster, DEGRADED_RETRY, Dispatcher, Seams, Sequencing, WriterId};
use oqueue_codec::apikey::ApiKey;
use oqueue_coordinator::Coordinator;
use oqueue_core::{
    CoordinatorEpoch, FakeClock, FakeGroupCoordinator, FakeMaterializedIndex, FakeObjectStore,
    FaultConfig, ObjectStore, ObjectStoreGroupMetadataLog, ObjectStoreMetadataLog, StormKind,
};
use std::sync::Arc;

async fn produce_once(dispatcher: &Dispatcher, cluster: &Cluster) -> i16 {
    let mut request = ProduceRequest::default();
    request.acks = -1;
    let mut topic = TopicProduceData::default();
    topic.topic_id = cluster.topic_id("orders").await.expect("a hosted topic");
    let mut partition = PartitionProduceData::default();
    partition.index = 0;
    partition.records = Some(bytes::Bytes::from(golden_batch(&[b"after the outage"])));
    topic.partition_data.push(partition);
    request.topic_data.push(topic);
    let mut body = Vec::new();
    request.encode(&mut body, PRODUCE_VERSION).expect("encodes");
    let reply = ask(dispatcher, framed(ApiKey::Produce, PRODUCE_VERSION, &body)).await;
    let mut rest = body_of(&reply, true);
    let response = ProduceResponse::decode(&mut rest, PRODUCE_VERSION).expect("decodes");
    response.responses[0].partition_responses[0].error_code
}

async fn api_versions(dispatcher: &Dispatcher) -> Vec<u8> {
    let mut body = Vec::new();
    ApiVersionsRequest::default()
        .encode(&mut body, 3)
        .expect("encodes");
    ask(dispatcher, framed(ApiKey::ApiVersions, 3, &body)).await
}

/// ⚠️ **No hard dependency at boot**: every store call fails while the node
/// starts, and it starts anyway, answers `ApiVersions`, and — once the store
/// answers again — replays its logs and serves a produce, all without being
/// restarted.
#[tokio::test(start_paused = true)]
async fn a_node_boots_degraded_without_its_coordinator() {
    let fake = Arc::new(FakeObjectStore::new());
    fake.set_faults(FaultConfig {
        storm: Some((StormKind::Transient, u32::MAX)),
        ..FaultConfig::default()
    });
    let store: Arc<dyn ObjectStore> = Arc::clone(&fake) as Arc<dyn ObjectStore>;

    let log = Arc::new(ObjectStoreMetadataLog::deferred(
        Arc::clone(&store),
        "meta/0",
    ));
    let (coordinator, serving, reader) = Coordinator::open_deferred(
        log,
        Box::new(FakeMaterializedIndex::new()),
        CoordinatorEpoch::new(1),
        Arc::new(FakeClock::new()),
    );
    let serving = tokio::spawn(serving.run_retrying(|| tokio::time::sleep(DEGRADED_RETRY)));
    let cluster = Arc::new(
        Cluster::new(
            "h",
            1,
            Sequencing::new(coordinator, reader),
            Seams {
                store: Arc::clone(&store),
                group_coordinator: Arc::new(FakeGroupCoordinator::new()),
                group_metadata_log: Arc::new(ObjectStoreGroupMetadataLog::deferred(
                    Arc::clone(&store),
                    "groups/0",
                )),
            },
            &WriterId::mint(),
        )
        .await
        .expect("the node boots with its store unreachable"),
    );
    cluster.ensure_topic("orders").await;
    let dispatcher = Dispatcher::new(Arc::clone(&cluster));

    let reply = api_versions(&dispatcher).await;
    assert!(!reply.is_empty(), "a degraded node still answers");
    tokio::time::sleep(DEGRADED_RETRY * 5).await;

    // The store comes back; the node recovers on its own.
    fake.set_faults(FaultConfig::default());
    tokio::time::sleep(DEGRADED_RETRY * 3).await;
    cluster.wait_until_replayed().await;
    assert_eq!(
        produce_once(&dispatcher, &cluster).await,
        0,
        "and a produce is served with no restart"
    );

    drop((dispatcher, cluster));
    serving.abort();
}
