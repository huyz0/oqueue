//! Key-domain routing tests for the produce planner.

#![allow(clippy::expect_used)]
#![allow(clippy::redundant_pub_crate)]

use super::tests::{body_for, produce_body, replied, verdict};
use super::{Pending, PendingRecord};
use crate::cluster::{RegionSealer, SealedRegionOwned};
use crate::testing::{Fixture, fixture, golden_batch, topic, with_store_and_catalog_and_sealer};
use kafka_protocol::messages::TopicName;
use kafka_protocol::messages::produce_request::TopicProduceData;
use kafka_protocol::protocol::StrBytes;
use oqueue_core::{
    ByteRange, FakeObjectStore, FakeTopicCatalog, KeyDomain, KeyId, ObjectStore, Operation,
    ParsedNonce, PartitionId, PushedRecords, Redacted, RegionAlg, RegionEnvelope, WrappedKey,
    parse_footer,
};
use std::sync::Arc;

#[derive(Debug)]
struct TestRegionSealer;

impl RegionSealer for TestRegionSealer {
    fn seal<'a>(
        &'a self,
        domain: &'a KeyDomain,
        _topic: &'a oqueue_core::TopicId,
        _partition: PartitionId,
        records: &'a [u8],
    ) -> oqueue_core::BoxFuture<'a, oqueue_core::Result<SealedRegionOwned>> {
        let key_id = domain.key_id().expect("customer domain").clone();
        Box::pin(async move {
            Ok(SealedRegionOwned::new(
                records.to_vec(),
                RegionAlg::Aes256Gcm,
                RegionEnvelope::new(
                    key_id,
                    WrappedKey::new(Redacted::new(vec![1, 2, 3])),
                    ParsedNonce::decode([0; 12]),
                )
                .expect("a representable envelope"),
            ))
        })
    }
}

#[tokio::test]
async fn pending_routes_each_domain_to_its_own_bundle() {
    let mut pending = Pending::new();
    let sealer = TestRegionSealer;
    assert!(pending.is_empty());
    pending
        .push(
            &sealer,
            KeyDomain::default_domain(),
            PendingRecord {
                topic: topic("plain"),
                partition: PartitionId::new(0).expect("a partition"),
                pushed: PushedRecords {
                    count: 1,
                    producer: None,
                },
                records: b"plain",
            },
        )
        .await
        .expect("the default region fits");
    pending
        .push(
            &sealer,
            KeyDomain::customer(KeyId::new("customer-kek").expect("a key id")),
            PendingRecord {
                topic: topic("private"),
                partition: PartitionId::new(0).expect("a partition"),
                pushed: PushedRecords {
                    count: 1,
                    producer: None,
                },
                records: b"private",
            },
        )
        .await
        .expect("the customer region fits");

    assert_eq!(pending.bundles.len(), 2);
    assert_eq!(pending.bundles[0].len(), 1);
    assert_eq!(pending.bundles[1].len(), 1);
    assert_eq!(pending.routes, vec![vec![0], vec![1]]);
}

#[tokio::test]
async fn no_object_mixes_key_domains() {
    let mixed = byok_fixture().await;
    let response = replied(&mixed, 9, &body_for(9, mixed_topics(), -1)).await;

    assert_eq!(response.responses[0].partition_responses[0].error_code, 0);
    assert_eq!(response.responses[1].partition_responses[0].error_code, 0);
    assert_eq!(mixed.store.counts().count(Operation::Put), 2);
    for key in mixed.store.inner().keys() {
        let bytes = object_bytes(&mixed, &key).await;
        let regions = parse_footer(&bytes, bytes.len() as u64).expect("a valid footer");
        let plain = regions
            .iter()
            .all(|region| region.topic() == &topic("plain"));
        let private = regions
            .iter()
            .all(|region| region.topic() == &topic("private"));
        assert!(plain || private, "one object contains two key domains");
        if private {
            assert!(
                regions
                    .iter()
                    .all(|region| region.alg() == RegionAlg::Aes256Gcm)
            );
        }
    }
}

#[tokio::test]
async fn byok_topics_leave_the_default_path_untouched() {
    let baseline = fixture(&["plain"]).await;
    let baseline_body = produce_body(9, "plain", -1, golden_batch());
    let baseline_response = replied(&baseline, 9, &baseline_body).await;
    assert_eq!(verdict(&baseline_response).0, 0);
    let baseline_key = baseline.store.inner().keys().pop().expect("one object");
    let baseline_bytes = object_bytes(&baseline, &baseline_key).await;

    let mixed = byok_fixture().await;
    let mixed_response = replied(&mixed, 9, &body_for(9, mixed_topics(), -1)).await;

    assert_eq!(
        mixed_response.responses[0].partition_responses[0].error_code,
        0
    );
    assert_eq!(
        mixed_response.responses[1].partition_responses[0].error_code,
        0
    );
    assert_eq!(mixed.store.counts().count(Operation::Put), 2);
    let mut mixed_plain = None;
    for key in mixed.store.inner().keys() {
        let bytes = object_bytes(&mixed, &key).await;
        let regions = parse_footer(&bytes, bytes.len() as u64).expect("a valid footer");
        if regions
            .iter()
            .any(|region| region.topic() == &topic("plain"))
        {
            mixed_plain = Some(bytes);
        }
    }
    assert_eq!(mixed_plain.as_deref(), Some(baseline_bytes.as_slice()));
}

async fn byok_fixture() -> Fixture {
    let catalog = Arc::new(FakeTopicCatalog::new());
    catalog.create_with_key_domain(
        &topic("private"),
        1,
        KeyDomain::customer(KeyId::new("customer-kek").expect("a key id")),
    );
    with_store_and_catalog_and_sealer(
        &["plain", "private"],
        Arc::new(crate::testing::TestStore::new(FakeObjectStore::new())),
        catalog,
        Arc::new(TestRegionSealer),
    )
    .await
}

async fn object_bytes(fixture: &Fixture, key: &oqueue_core::ObjectKey) -> Vec<u8> {
    fixture
        .store
        .inner()
        .get(key, ByteRange::Full)
        .await
        .expect("the object exists")
}

fn mixed_topics() -> Vec<(TopicProduceData, Vec<u8>)> {
    ["plain", "private"]
        .into_iter()
        .map(|name| {
            let mut topic = TopicProduceData::default();
            topic.name = TopicName(StrBytes::from_string(name.to_owned()));
            (topic, golden_batch())
        })
        .collect()
}
