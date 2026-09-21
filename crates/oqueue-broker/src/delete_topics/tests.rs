#![allow(clippy::expect_used)]

use super::delete_one;
use crate::cluster::Cluster;
use crate::testing::fixture;
use oqueue_codec::delete_topics::DeletableTopic;
use oqueue_codec::error_codes;
use oqueue_core::{
    BoxFuture, CatalogEntry, Error, FakeTopicCatalog, Principal, TopicCatalog, TopicCreateOutcome,
    TopicDeleteOutcome, TopicGrants, TopicId, TopicRetentionUpdate,
};
use std::sync::RwLock;
use std::time::Duration;

fn topic(name: &str) -> TopicId {
    TopicId::new(name).expect("topic")
}

#[tokio::test]
async fn deletion_hides_the_topic_revokes_grants_and_evicts_the_cache() {
    let fixture = fixture(&[]).await;
    let alice = Principal::new("alice").expect("principal");
    let name = topic("orders");
    let entry = fixture
        .cluster
        .create_topic_owned(&name, 3, &alice)
        .await
        .expect("create")
        .entry()
        .clone();
    assert_eq!(fixture.cluster.partition_count("orders").await, Some(3));
    let grants = RwLock::new(TopicGrants::new());
    grants
        .write()
        .expect("grants")
        .grant(alice.clone(), name.clone());

    let result = delete_one(
        &fixture.cluster,
        &DeletableTopic {
            name: Some("orders".to_owned()),
            topic_id: entry.id().to_be_bytes(),
        },
        6,
        true,
        &grants,
        tokio::time::Instant::now() + Duration::from_secs(1),
    )
    .await;

    assert_eq!(result.error_code, error_codes::NONE);
    assert_eq!(fixture.cluster.partition_count("orders").await, None);
    assert_eq!(
        fixture
            .cluster
            .topic_name_by_id(uuid::Uuid::from_u128(entry.id()))
            .await,
        None
    );
    assert!(!grants.read().expect("grants").can_see(&alice, &name));
    assert_eq!(
        fixture.cluster.creator_topics(&alice).await,
        Some(Vec::new())
    );
}

#[tokio::test]
async fn a_stale_uuid_refuses_without_mutating_the_live_topic() {
    let fixture = fixture(&[]).await;
    let name = topic("orders");
    let entry = fixture
        .cluster
        .create_topic(&name, 1)
        .await
        .expect("create");
    let grants = RwLock::new(TopicGrants::new());
    let result = delete_one(
        &fixture.cluster,
        &DeletableTopic {
            name: Some("orders".to_owned()),
            topic_id: (entry.id() ^ 1).to_be_bytes(),
        },
        6,
        true,
        &grants,
        tokio::time::Instant::now() + Duration::from_secs(1),
    )
    .await;

    assert_eq!(result.error_code, error_codes::UNKNOWN_TOPIC_ID);
    assert!(result.error_message.is_some(), "v6 carries diagnostics");
    assert_eq!(fixture.cluster.partition_count("orders").await, Some(1));

    let v4 = delete_one(
        &fixture.cluster,
        &DeletableTopic {
            name: Some("orders".to_owned()),
            topic_id: [0; 16],
        },
        4,
        true,
        &grants,
        tokio::time::Instant::now() + Duration::from_secs(1),
    )
    .await;
    assert!(v4.error_message.is_none(), "v4 has no diagnostic field");
}

#[tokio::test]
async fn denied_deletion_changes_neither_catalog_nor_grants() {
    let fixture = fixture(&[]).await;
    let name = topic("orders");
    let entry = fixture
        .cluster
        .create_topic(&name, 1)
        .await
        .expect("create");
    let grants = RwLock::new(TopicGrants::new());
    let result = delete_one(
        &fixture.cluster,
        &DeletableTopic {
            name: Some("orders".to_owned()),
            topic_id: entry.id().to_be_bytes(),
        },
        6,
        false,
        &grants,
        tokio::time::Instant::now() + Duration::from_secs(1),
    )
    .await;

    assert_eq!(result.error_code, error_codes::TOPIC_AUTHORIZATION_FAILED);
    assert_eq!(fixture.cluster.partition_count("orders").await, Some(1));
}

#[tokio::test]
async fn cleanup_error_after_tombstone_evicts_serving_state_and_grants() {
    let catalog = std::sync::Arc::new(CleanupErrorCatalog {
        inner: FakeTopicCatalog::new(),
        tombstone_before_error: true,
    });
    let store = std::sync::Arc::new(crate::testing::TestStore::new(
        oqueue_core::FakeObjectStore::new(),
    ));
    let fixture = crate::testing::with_store_and_catalog("h", 1, &[], store, catalog).await;
    let alice = Principal::new("alice").expect("principal");
    let name = topic("orders");
    let entry = fixture
        .cluster
        .create_topic_owned(&name, 1, &alice)
        .await
        .expect("create")
        .entry()
        .clone();
    let grants = RwLock::new(TopicGrants::new());
    grants.write().expect("grants").grant(alice, name.clone());
    assert_eq!(fixture.cluster.partition_count("orders").await, Some(1));

    let result = delete_one(
        &fixture.cluster,
        &DeletableTopic {
            name: Some("orders".to_owned()),
            topic_id: entry.id().to_be_bytes(),
        },
        6,
        true,
        &grants,
        tokio::time::Instant::now() + Duration::from_secs(1),
    )
    .await;

    assert_eq!(result.error_code, error_codes::UNKNOWN_SERVER_ERROR);
    assert_eq!(fixture.cluster.partition_count("orders").await, None);
    assert_eq!(
        fixture
            .cluster
            .topic_name_by_id(uuid::Uuid::from_u128(entry.id()))
            .await,
        None
    );
    assert!(
        !grants
            .read()
            .expect("grants")
            .can_see(&Principal::new("alice").expect("principal"), &name)
    );
}

#[tokio::test]
async fn delete_error_before_tombstone_preserves_serving_state_and_grants() {
    let catalog = std::sync::Arc::new(CleanupErrorCatalog {
        inner: FakeTopicCatalog::new(),
        tombstone_before_error: false,
    });
    let store = std::sync::Arc::new(crate::testing::TestStore::new(
        oqueue_core::FakeObjectStore::new(),
    ));
    let fixture = crate::testing::with_store_and_catalog("h", 1, &[], store, catalog).await;
    let alice = Principal::new("alice").expect("principal");
    let name = topic("orders");
    let entry = fixture
        .cluster
        .create_topic_owned(&name, 1, &alice)
        .await
        .expect("create")
        .entry()
        .clone();
    let grants = RwLock::new(TopicGrants::new());
    grants
        .write()
        .expect("grants")
        .grant(alice.clone(), name.clone());

    let result = delete_one(
        &fixture.cluster,
        &DeletableTopic {
            name: Some("orders".to_owned()),
            topic_id: entry.id().to_be_bytes(),
        },
        6,
        true,
        &grants,
        tokio::time::Instant::now() + Duration::from_secs(1),
    )
    .await;

    assert_eq!(result.error_code, error_codes::UNKNOWN_SERVER_ERROR);
    assert_eq!(fixture.cluster.partition_count("orders").await, Some(1));
    assert!(grants.read().expect("grants").can_see(&alice, &name));
}

#[tokio::test]
async fn retrying_already_deleted_evicts_caches_and_all_creator_loads() {
    let catalog = std::sync::Arc::new(FakeTopicCatalog::new());
    let store = std::sync::Arc::new(crate::testing::TestStore::new(
        oqueue_core::FakeObjectStore::new(),
    ));
    let fixture = crate::testing::with_store_and_catalog(
        "h",
        1,
        &[],
        store,
        std::sync::Arc::clone(&catalog) as std::sync::Arc<dyn TopicCatalog>,
    )
    .await;
    let alice = Principal::new("alice").expect("principal");
    let name = topic("orders");
    let entry = fixture
        .cluster
        .create_topic_owned(&name, 1, &alice)
        .await
        .expect("create")
        .entry()
        .clone();
    assert_eq!(fixture.cluster.partition_count("orders").await, Some(1));
    assert_topic_name_by_id(&fixture.cluster, entry.id(), Some("orders")).await;
    assert_eq!(
        fixture.cluster.creator_topics(&alice).await,
        Some(vec![name.clone()])
    );

    catalog
        .delete(&name, None)
        .await
        .expect("external deletion");
    assert!(matches!(
        fixture.cluster.delete_topic(&name, None).await,
        Ok(TopicDeleteOutcome::AlreadyDeleted { .. })
    ));

    assert_eq!(fixture.cluster.partition_count("orders").await, None);
    assert_topic_name_by_id(&fixture.cluster, entry.id(), None).await;
    assert_eq!(
        fixture.cluster.creator_topics(&alice).await,
        Some(Vec::new())
    );
}

async fn assert_topic_name_by_id(cluster: &Cluster, id: u128, expected: Option<&str>) {
    assert_eq!(
        cluster
            .topic_name_by_id(uuid::Uuid::from_u128(id))
            .await
            .as_deref(),
        expected
    );
}

#[derive(Debug)]
struct CleanupErrorCatalog {
    inner: FakeTopicCatalog,
    tombstone_before_error: bool,
}

impl TopicCatalog for CleanupErrorCatalog {
    fn lookup<'a>(
        &'a self,
        name: &'a TopicId,
    ) -> BoxFuture<'a, oqueue_core::Result<Option<CatalogEntry>>> {
        Box::pin(async move { self.inner.lookup(name).await })
    }

    fn lookup_id(&self, id: u128) -> BoxFuture<'_, oqueue_core::Result<Option<CatalogEntry>>> {
        Box::pin(async move { self.inner.lookup_id(id).await })
    }

    fn create<'a>(
        &'a self,
        name: &'a TopicId,
        partitions: u32,
    ) -> BoxFuture<'a, oqueue_core::Result<CatalogEntry>> {
        Box::pin(async move { self.inner.create(name, partitions).await })
    }

    fn create_owned<'a>(
        &'a self,
        name: &'a TopicId,
        partitions: u32,
        creator: &'a Principal,
    ) -> BoxFuture<'a, oqueue_core::Result<TopicCreateOutcome>> {
        Box::pin(async move { self.inner.create_owned(name, partitions, creator).await })
    }

    fn delete<'a>(
        &'a self,
        name: &'a TopicId,
        expected_id: Option<u128>,
    ) -> BoxFuture<'a, oqueue_core::Result<TopicDeleteOutcome>> {
        Box::pin(async move {
            if self.tombstone_before_error {
                let _ = self.inner.delete(name, expected_id).await?;
            }
            Err(Error::Permanent)
        })
    }

    fn topic_retention_ms<'a>(
        &'a self,
        name: &'a TopicId,
    ) -> BoxFuture<'a, oqueue_core::Result<Option<i64>>> {
        Box::pin(async move { self.inner.topic_retention_ms(name).await })
    }

    fn set_topic_retention_ms<'a>(
        &'a self,
        name: &'a TopicId,
        retention_ms: Option<i64>,
    ) -> BoxFuture<'a, oqueue_core::Result<TopicRetentionUpdate>> {
        Box::pin(async move { self.inner.set_topic_retention_ms(name, retention_ms).await })
    }

    fn list_owned<'a>(
        &'a self,
        creator: &'a Principal,
        after: Option<&'a TopicId>,
        limit: usize,
    ) -> BoxFuture<'a, oqueue_core::Result<Vec<TopicId>>> {
        Box::pin(async move { self.inner.list_owned(creator, after, limit).await })
    }

    fn list<'a>(
        &'a self,
        after: Option<&'a TopicId>,
        limit: usize,
    ) -> BoxFuture<'a, oqueue_core::Result<Vec<TopicId>>> {
        Box::pin(async move { self.inner.list(after, limit).await })
    }
}
