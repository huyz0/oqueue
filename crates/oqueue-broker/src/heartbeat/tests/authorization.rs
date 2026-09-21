#![allow(clippy::expect_used)]

use super::{group, handle, heartbeat_body, prelude};
use crate::connection::HandlerResponse;
use crate::testing::fixture;
use kafka_protocol::messages::HeartbeatResponse as KpHeartbeatResponse;
use kafka_protocol::protocol::Decodable;

const HEARTBEAT_VERSION: i16 = 2;

#[tokio::test]
async fn a_cross_principal_group_is_refused() {
    let fixture = fixture(&[]).await;
    let bob = oqueue_core::Principal::new("bob").expect("valid principal");
    let grants = oqueue_core::GroupGrants::new();
    let authz = crate::authz::GroupAuthzContext {
        principal: Some(&bob),
        credentials_configured: true,
        group_grants: &grants,
    };
    let response = handle(
        &fixture.cluster,
        prelude(12, HEARTBEAT_VERSION),
        &heartbeat_body("alice-group", "member", 1),
        &authz,
    )
    .await;
    let HandlerResponse::Reply(out) = response else {
        panic!("a refused Heartbeat still replies");
    };
    let mut rest = &out[4..];
    let response = KpHeartbeatResponse::decode(&mut rest, HEARTBEAT_VERSION).expect("decodes");
    assert_eq!(
        response.error_code,
        oqueue_codec::error_codes::GROUP_AUTHORIZATION_FAILED
    );
    assert!(
        !fixture
            .cluster
            .heartbeats()
            .is_tracked(&group("alice-group"), "member")
    );
}
