#![allow(clippy::expect_used)]

use super::Dispatcher;
use crate::connection::HandlerResponse;
use crate::health::NodeRole;
use crate::testing::fixture;
use oqueue_codec::wire::{put_i16, put_i32};
use std::sync::Arc;

#[tokio::test]
async fn a_data_plane_role_rejects_coordinator_apis_before_decoding_the_body() {
    let fixture = fixture(&[]).await;
    let dispatcher = Dispatcher::new(Arc::clone(&fixture.cluster)).with_role(NodeRole::DataPlane);
    let mut request = Vec::new();
    put_i16(&mut request, 19); // CreateTopics
    put_i16(&mut request, 0);
    put_i32(&mut request, 1);
    assert_eq!(dispatcher.dispatch(request).await, HandlerResponse::Close);
}

#[tokio::test]
async fn role_modes_reject_the_wrong_request_family_and_combined_accepts_discovery() {
    let coordinator_fixture = fixture(&[]).await;
    let coordinator =
        Dispatcher::new(Arc::clone(&coordinator_fixture.cluster)).with_role(NodeRole::Coordinator);
    let mut produce = Vec::new();
    put_i16(&mut produce, 0); // Produce
    put_i16(&mut produce, 0);
    put_i32(&mut produce, 1);
    assert_eq!(coordinator.dispatch(produce).await, HandlerResponse::Close);

    let combined_fixture = fixture(&[]).await;
    let combined =
        Dispatcher::new(Arc::clone(&combined_fixture.cluster)).with_role(NodeRole::Combined);
    assert!(matches!(
        combined.dispatch(super::api_versions_request(3, 1)).await,
        HandlerResponse::Reply(_)
    ));
}
