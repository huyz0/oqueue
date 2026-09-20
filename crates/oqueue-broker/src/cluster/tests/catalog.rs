//! `M7.2`: topics are read through the catalog seam, and the cache holds only
//! what this node served (`ADR-0049` point 2).

use super::cluster_still_loading;
use oqueue_core::{FakeTopicCatalog, TopicCatalog, TopicId, topic_uuid};
use std::sync::Arc;
use uuid::Uuid;

fn topic(name: &str) -> TopicId {
    TopicId::new(name).expect("a non-empty name")
}

#[tokio::test(start_paused = true)]
async fn an_id_created_on_another_node_resolves_through_the_shared_catalog() {
    let catalog: Arc<dyn TopicCatalog> = Arc::new(FakeTopicCatalog::new());
    let creator = cluster_still_loading()
        .await
        .with_catalog(Arc::clone(&catalog));
    let reader = cluster_still_loading()
        .await
        .with_catalog(Arc::clone(&catalog));
    assert!(creator.ensure_topic("orders").await);
    let id = Uuid::from_u128(topic_uuid(&topic("orders")));
    assert_eq!(reader.cached_topics(), 0, "nothing served here yet");
    assert_eq!(reader.topic_name_by_id(id).await.as_deref(), Some("orders"));
}

#[tokio::test(start_paused = true)]
async fn the_cache_holds_only_topics_this_node_served() {
    let catalog = Arc::new(FakeTopicCatalog::new());
    for name in ["a", "b", "c"] {
        catalog.create(&topic(name), 1).await.expect("created");
    }
    let cluster = cluster_still_loading()
        .await
        .with_catalog(Arc::clone(&catalog) as Arc<dyn TopicCatalog>);
    assert_eq!(cluster.partition_count("b").await, Some(1));
    assert_eq!(cluster.cached_topics(), 1);
}

#[tokio::test(start_paused = true)]
async fn an_unresolvable_topic_has_no_key_domain() {
    let cluster = cluster_still_loading().await;
    assert!(
        cluster
            .topic_key_domain(&topic("never-created"))
            .await
            .is_none()
    );
}

/// ⚠️ **A limit past one catalog page is met exactly** (`M7.4`): the last page
/// asks only for what is still wanted.
#[tokio::test(start_paused = true)]
async fn a_listing_past_one_page_stops_at_its_limit() {
    let catalog = Arc::new(FakeTopicCatalog::new());
    for i in 0..2_500 {
        catalog
            .create(&topic(&format!("t{i:05}")), 1)
            .await
            .expect("created");
    }
    let cluster = cluster_still_loading()
        .await
        .with_catalog(Arc::clone(&catalog) as Arc<dyn TopicCatalog>);
    let names = cluster.topic_names(1_500).await;
    assert_eq!(names.len(), 1_500);
    assert_eq!(names.last().map(String::as_str), Some("t01499"));
}
