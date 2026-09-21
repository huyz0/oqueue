#![allow(clippy::expect_used)]

use super::{Dispatcher, dispatcher};
use crate::testing::fixture;
use std::sync::Arc;

#[tokio::test]
async fn authentication_hydrates_durable_creator_visibility() {
    let (dispatcher, fixture) = dispatcher().await;
    let alice = oqueue_core::Principal::new("alice").expect("principal");
    let topic = oqueue_core::TopicId::new("durable").expect("topic");
    fixture
        .cluster
        .create_topic_owned(&topic, 1, &alice)
        .await
        .expect("topic created");
    assert!(dispatcher.session.authenticate(alice.clone()));

    dispatcher.hydrate_creator_grants().await;

    assert!(
        dispatcher
            .topic_grants
            .read()
            .expect("grants")
            .can_see(&alice, &topic)
    );
    dispatcher.hydrate_creator_grants().await;
}

#[tokio::test]
async fn durable_creator_visibility_reaches_each_default_dispatcher() {
    let fixture = fixture(&[]).await;
    let alice = oqueue_core::Principal::new("alice").expect("principal");
    let topic = oqueue_core::TopicId::new("durable").expect("topic");
    fixture
        .cluster
        .create_topic_owned(&topic, 1, &alice)
        .await
        .expect("topic created");
    let first = Dispatcher::new(Arc::clone(&fixture.cluster));
    let second = Dispatcher::new(Arc::clone(&fixture.cluster));
    assert!(first.session.authenticate(alice.clone()));
    assert!(second.session.authenticate(alice.clone()));

    first.hydrate_creator_grants().await;
    second.hydrate_creator_grants().await;

    assert!(
        first
            .topic_grants
            .read()
            .expect("grants")
            .can_see(&alice, &topic)
    );
    assert!(
        second
            .topic_grants
            .read()
            .expect("grants")
            .can_see(&alice, &topic)
    );
}

#[tokio::test]
async fn a_new_owned_topic_invalidates_an_empty_creator_cache() {
    let fixture = fixture(&[]).await;
    let alice = oqueue_core::Principal::new("alice").expect("principal");
    let first = Dispatcher::new(Arc::clone(&fixture.cluster));
    assert!(first.session.authenticate(alice.clone()));
    first.hydrate_creator_grants().await;

    let topic = oqueue_core::TopicId::new("created-later").expect("topic");
    fixture
        .cluster
        .create_topic_owned(&topic, 1, &alice)
        .await
        .expect("topic created");

    let second = Dispatcher::new(Arc::clone(&fixture.cluster));
    assert!(second.session.authenticate(alice.clone()));
    second.hydrate_creator_grants().await;
    assert!(
        second
            .topic_grants
            .read()
            .expect("grants")
            .can_see(&alice, &topic)
    );
}
