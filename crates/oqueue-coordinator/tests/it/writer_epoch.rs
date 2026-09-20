//! Durable writer-epoch allocation races.

#![allow(clippy::expect_used)]

use oqueue_coordinator::DurableWriterEpochAllocator;
use oqueue_core::{
    ByteRange, Error, FakeObjectStore, FaultConfig, MaintenanceStore, ObjectKey, ObjectStore,
};
use std::sync::Arc;

#[tokio::test]
async fn concurrent_writers_and_a_restart_receive_distinct_epochs() {
    let store = Arc::new(FakeObjectStore::with_faults(FaultConfig {
        latency_polls: 1,
        storm: None,
        crash_after_put_before_ack: 0,
        put_latency_polls: 1,
    }));
    let object_store: Arc<dyn ObjectStore> = Arc::clone(&store) as _;
    let maintenance: Arc<dyn MaintenanceStore> = Arc::clone(&store) as _;
    let first = DurableWriterEpochAllocator::new(
        Arc::clone(&object_store),
        Arc::clone(&maintenance),
        "meta/0",
    );
    let second = DurableWriterEpochAllocator::new(object_store, maintenance, "meta/0");

    let (first_epoch, second_epoch) = tokio::join!(first.allocate(), second.allocate());
    let first_epoch = first_epoch.expect("the first writer allocates");
    let second_epoch = second_epoch.expect("the second writer allocates");
    assert_ne!(first_epoch, second_epoch);

    let object_store: Arc<dyn ObjectStore> = Arc::clone(&store) as _;
    let maintenance: Arc<dyn MaintenanceStore> = Arc::clone(&store) as _;
    let restarted = DurableWriterEpochAllocator::new(object_store, maintenance, "meta/0");
    let restarted_epoch = restarted.allocate().await.expect("the restart allocates");
    assert!(![first_epoch, second_epoch].contains(&restarted_epoch));
    assert_eq!(restarted_epoch.get(), 2);
}

#[tokio::test]
async fn a_missing_first_term_is_refused_instead_of_reusing_epoch_zero() {
    let store = Arc::new(FakeObjectStore::new());
    let store_trait: Arc<dyn ObjectStore> = Arc::clone(&store) as _;
    let maintenance: Arc<dyn MaintenanceStore> = Arc::clone(&store) as _;
    let allocator =
        DurableWriterEpochAllocator::new(Arc::clone(&store_trait), maintenance, "meta/0");
    allocator.allocate().await.expect("epoch zero");
    allocator.allocate().await.expect("epoch one");

    let first =
        ObjectKey::new("meta/0/writer-epoch/00000000000000000000").expect("a valid term key");
    store
        .delete(&[first])
        .await
        .expect("deletion simulates corruption");
    store
        .delete(&[ObjectKey::new("meta/0/writer-epoch/marker").expect("a valid marker key")])
        .await
        .expect("deletion simulates a partial restore");
    assert_eq!(
        allocator.allocate().await,
        Err(Error::MalformedMetadataSegment { at: 0 })
    );
    assert_eq!(
        store_trait
            .get(
                &ObjectKey::new("meta/0/writer-epoch/00000000000000000001")
                    .expect("a valid term key"),
                ByteRange::Full,
            )
            .await
            .expect("term one remains"),
        b"OQWE"
    );
}

#[tokio::test]
async fn malformed_marker_metadata_is_refused() {
    let store = Arc::new(FakeObjectStore::new());
    let object_store: Arc<dyn ObjectStore> = Arc::clone(&store) as _;
    let maintenance: Arc<dyn MaintenanceStore> = Arc::clone(&store) as _;
    let allocator = DurableWriterEpochAllocator::new(object_store, maintenance, "meta/0");
    allocator.allocate().await.expect("epoch zero");
    store
        .put(
            &ObjectKey::new("meta/0/writer-epoch/marker").expect("a valid marker key"),
            b"corrupt".to_vec(),
            None,
        )
        .await
        .expect("corruption is installed");
    assert_eq!(
        allocator.allocate().await,
        Err(Error::MalformedMetadataSegment { at: 0 })
    );
}

#[tokio::test]
async fn malformed_interior_term_metadata_is_refused() {
    let store = Arc::new(FakeObjectStore::new());
    let object_store: Arc<dyn ObjectStore> = Arc::clone(&store) as _;
    let maintenance: Arc<dyn MaintenanceStore> = Arc::clone(&store) as _;
    let allocator = DurableWriterEpochAllocator::new(object_store, maintenance, "meta/0");
    allocator.allocate().await.expect("epoch zero");
    allocator.allocate().await.expect("epoch one");
    allocator.allocate().await.expect("epoch two");
    store
        .put(
            &ObjectKey::new("meta/0/writer-epoch/00000000000000000001").expect("a valid term key"),
            b"corrupt".to_vec(),
            None,
        )
        .await
        .expect("corruption is installed");
    assert_eq!(
        allocator.allocate().await,
        Err(Error::MalformedMetadataSegment { at: 0 })
    );
}
