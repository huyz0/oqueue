#![allow(clippy::expect_used)]

use super::{CreateContext, create_one, deadline_at, remaining_until, timeout_duration};
use crate::testing::fixture;
use oqueue_codec::create_topics::CreatableTopic;
use oqueue_codec::error_codes;
use oqueue_core::TopicGrants;
use std::sync::RwLock;
use std::time::Duration;

fn requested(name: &str, partitions: i32) -> CreatableTopic {
    CreatableTopic {
        name: name.to_owned(),
        num_partitions: partitions,
        replication_factor: -1,
        assignments: Vec::new(),
        configs: Vec::new(),
    }
}

#[tokio::test]
async fn authorized_creation_is_idempotent_and_catalog_only() {
    let fixture = fixture(&[]).await;
    let grants = RwLock::new(TopicGrants::new());
    let principal = oqueue_core::Principal::new("alice").expect("principal");
    let context = CreateContext {
        cluster: &fixture.cluster,
        create_timeout: None,
        validate_only: false,
        authorized: true,
        principal: Some(&principal),
        topic_grants: &grants,
    };
    let first = create_one(&requested("orders", 3), &context).await;
    let second = create_one(&requested("orders", 3), &context).await;
    assert_eq!(first.error_code, error_codes::NONE);
    assert_eq!(second.error_code, error_codes::TOPIC_ALREADY_EXISTS);
    assert_eq!(first.replication_factor, 1);
    assert_eq!(
        first.topic_id,
        fixture
            .cluster
            .topic_id("orders")
            .await
            .expect("topic")
            .as_u128()
            .to_be_bytes()
    );
    assert_eq!(fixture.cluster.partition_count("orders").await, Some(3));
    assert_eq!(
        grants
            .read()
            .expect("grants")
            .topics_for(&principal)
            .count(),
        1
    );
}

#[tokio::test]
async fn an_existing_topic_does_not_grant_a_second_creator() {
    let fixture = fixture(&[]).await;
    let alice = oqueue_core::Principal::new("alice").expect("principal");
    let bob = oqueue_core::Principal::new("bob").expect("principal");
    let grants = RwLock::new(TopicGrants::new());
    let alice_context = CreateContext {
        cluster: &fixture.cluster,
        create_timeout: None,
        validate_only: false,
        authorized: true,
        principal: Some(&alice),
        topic_grants: &grants,
    };
    assert_eq!(
        create_one(&requested("orders", 1), &alice_context)
            .await
            .error_code,
        error_codes::NONE
    );
    let bob_context = CreateContext {
        principal: Some(&bob),
        ..alice_context
    };
    assert_eq!(
        create_one(&requested("orders", 1), &bob_context)
            .await
            .error_code,
        error_codes::TOPIC_ALREADY_EXISTS
    );
    assert!(
        !grants
            .read()
            .expect("grants")
            .can_see(&bob, &oqueue_core::TopicId::new("orders").expect("topic"))
    );
}

#[tokio::test]
async fn denied_creation_changes_neither_catalog_nor_grants() {
    let fixture = fixture(&[]).await;
    let grants = RwLock::new(TopicGrants::new());
    let context = CreateContext {
        cluster: &fixture.cluster,
        create_timeout: None,
        validate_only: false,
        authorized: false,
        principal: None,
        topic_grants: &grants,
    };
    let result = create_one(&requested("orders", 3), &context).await;
    assert_eq!(result.error_code, error_codes::TOPIC_AUTHORIZATION_FAILED);
    assert_eq!(fixture.cluster.partition_count("orders").await, None);
}

#[tokio::test]
async fn validation_only_does_not_write_the_catalog() {
    let fixture = fixture(&[]).await;
    let grants = RwLock::new(TopicGrants::new());
    let context = CreateContext {
        cluster: &fixture.cluster,
        create_timeout: None,
        validate_only: true,
        authorized: true,
        principal: None,
        topic_grants: &grants,
    };
    let result = create_one(&requested("orders", 3), &context).await;
    assert_eq!(result.error_code, error_codes::NONE);
    assert_eq!(fixture.cluster.partition_count("orders").await, None);
}

#[tokio::test]
async fn validation_enforces_topic_and_partition_bounds() {
    let fixture = fixture(&[]).await;
    let grants = RwLock::new(TopicGrants::new());
    let context = CreateContext {
        cluster: &fixture.cluster,
        create_timeout: None,
        validate_only: true,
        authorized: true,
        principal: None,
        topic_grants: &grants,
    };

    let mut maximum_name = requested(&"a".repeat(249), 1);
    assert_eq!(
        create_one(&maximum_name, &context).await.error_code,
        error_codes::NONE
    );
    maximum_name.name.push('a');
    assert_eq!(
        create_one(&maximum_name, &context).await.error_code,
        error_codes::INVALID_TOPIC_EXCEPTION
    );

    assert_eq!(
        create_one(&requested("zero", 0), &context).await.error_code,
        error_codes::INVALID_PARTITIONS
    );
    assert_eq!(
        create_one(&requested("too-many", 1_001), &context)
            .await
            .error_code,
        error_codes::INVALID_PARTITIONS
    );
    assert_eq!(
        create_one(&requested("maximum", 1_000), &context)
            .await
            .error_code,
        error_codes::NONE
    );
    for invalid in [".", "..", "orders/slash", "orders space", "orders!bang"] {
        assert_eq!(
            create_one(&requested(invalid, 1), &context)
                .await
                .error_code,
            error_codes::INVALID_TOPIC_EXCEPTION,
            "{invalid:?} is not a legal Kafka topic name"
        );
    }
    let defaulted = create_one(&requested("default", -1), &context).await;
    assert_eq!(defaulted.error_code, error_codes::NONE);
    assert_eq!(defaulted.num_partitions, 1);
}

#[tokio::test]
async fn validation_rejects_unsupported_replication_and_nested_fields() {
    let fixture = fixture(&[]).await;
    let grants = RwLock::new(TopicGrants::new());
    let context = CreateContext {
        cluster: &fixture.cluster,
        create_timeout: None,
        validate_only: true,
        authorized: true,
        principal: None,
        topic_grants: &grants,
    };

    let mut replication = requested("replication", 1);
    replication.replication_factor = 1;
    assert_eq!(
        create_one(&replication, &context).await.error_code,
        error_codes::NONE
    );
    replication.replication_factor = 0;
    assert_eq!(
        create_one(&replication, &context).await.error_code,
        error_codes::INVALID_REPLICATION_FACTOR
    );

    let mut assignment = requested("assignment", 1);
    assignment
        .assignments
        .push(oqueue_codec::create_topics::CreatableReplicaAssignment {
            partition_index: 0,
            broker_ids: vec![1],
        });
    assert_eq!(
        create_one(&assignment, &context).await.error_code,
        error_codes::INVALID_REQUEST
    );

    let mut config = requested("config", 1);
    config
        .configs
        .push(oqueue_codec::create_topics::CreatableTopicConfig {
            name: "cleanup.policy".to_owned(),
            value: Some("compact".to_owned()),
        });
    assert_eq!(
        create_one(&config, &context).await.error_code,
        error_codes::INVALID_REQUEST
    );
}

#[test]
fn timeout_math_preserves_the_request_deadline() {
    let now = tokio::time::Instant::now();
    let timeout = Duration::from_millis(25);
    let deadline = deadline_at(now, timeout);
    assert_eq!(deadline.duration_since(now), timeout);
    assert_eq!(remaining_until(deadline, now), timeout);
    assert_eq!(remaining_until(now, now), Duration::ZERO);
    assert_eq!(timeout_duration(1234), Duration::from_millis(1234));
    assert_eq!(timeout_duration(-1), Duration::ZERO);
}

#[tokio::test]
async fn an_unauthenticated_authorized_path_uses_the_legacy_catalog_create() {
    let fixture = fixture(&[]).await;
    let grants = RwLock::new(TopicGrants::new());
    let context = CreateContext {
        cluster: &fixture.cluster,
        create_timeout: None,
        validate_only: false,
        authorized: true,
        principal: None,
        topic_grants: &grants,
    };
    let result = create_one(&requested("legacy", 1), &context).await;
    assert_eq!(result.error_code, error_codes::NONE);
    assert_eq!(fixture.cluster.partition_count("legacy").await, Some(1));
}

#[tokio::test]
async fn cluster_creator_listing_is_cached_after_loading_all_pages() {
    let fixture = fixture(&[]).await;
    let alice = oqueue_core::Principal::new("alice").expect("principal");
    let topic = oqueue_core::TopicId::new("owned").expect("topic");
    fixture
        .cluster
        .create_topic_owned(&topic, 1, &alice)
        .await
        .expect("topic created");

    assert_eq!(
        fixture.cluster.creator_topics(&alice).await,
        Some(vec![topic.clone()])
    );
    assert_eq!(
        fixture.cluster.creator_topics(&alice).await,
        Some(vec![topic])
    );
}
