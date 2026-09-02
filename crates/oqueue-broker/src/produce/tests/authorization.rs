use super::{
    decode, golden_batch, handle, partition, prelude, produce_body, produce_body_by_id, topic,
};
use crate::authz::AuthzContext;
use crate::connection::HandlerResponse;
use crate::testing::fixture;
use oqueue_core::{Principal, TopicGrants, TopicId};

fn alice() -> Principal {
    Principal::new("alice").expect("valid")
}

#[tokio::test]
async fn an_unconfigured_broker_answers_every_topic_regardless_of_grants() {
    let fixture = fixture(&["t"]).await;
    let body = produce_body(9, "t", -1, golden_batch());
    let authz = AuthzContext {
        principal: None,
        credentials_configured: false,
        topic_grants: &TopicGrants::default(),
    };
    let HandlerResponse::Reply(out) = handle(
        &fixture.cluster,
        &fixture.session,
        prelude(9),
        &body,
        &authz,
    )
    .await
    else {
        panic!("expected a reply");
    };
    let response = decode(&out, 9);
    assert_eq!(response.responses[0].partition_responses[0].error_code, 0);
}

#[tokio::test]
async fn a_configured_broker_refuses_an_ungranted_topic() {
    let fixture = fixture(&["t"]).await;
    let body = produce_body(9, "t", -1, golden_batch());
    let grants = TopicGrants::new();
    let authz = AuthzContext {
        principal: Some(&alice()),
        credentials_configured: true,
        topic_grants: &grants,
    };
    let HandlerResponse::Reply(out) = handle(
        &fixture.cluster,
        &fixture.session,
        prelude(9),
        &body,
        &authz,
    )
    .await
    else {
        panic!("expected a reply");
    };
    let response = decode(&out, 9);
    assert_eq!(
        response.responses[0].partition_responses[0].error_code,
        oqueue_codec::error_codes::TOPIC_AUTHORIZATION_FAILED,
    );
    assert_eq!(
        fixture
            .cluster
            .high_watermark(&topic("t"), partition(0))
            .get(),
        0,
        "a refused produce writes nothing"
    );
}

#[tokio::test]
async fn a_configured_broker_answers_a_granted_topic() {
    let fixture = fixture(&["t"]).await;
    let body = produce_body(9, "t", -1, golden_batch());
    let mut grants = TopicGrants::new();
    grants.grant(alice(), TopicId::new("t").expect("valid"));
    let authz = AuthzContext {
        principal: Some(&alice()),
        credentials_configured: true,
        topic_grants: &grants,
    };
    let HandlerResponse::Reply(out) = handle(
        &fixture.cluster,
        &fixture.session,
        prelude(9),
        &body,
        &authz,
    )
    .await
    else {
        panic!("expected a reply");
    };
    let response = decode(&out, 9);
    assert_eq!(response.responses[0].partition_responses[0].error_code, 0);
}

/// ⚠️ **Round 1 review's own finding, pinned rather than left implicit.**
/// At v13+ the wire addresses topics by id, resolved against the
/// registry before anything else runs — an id that resolves to nothing
/// answers `UNKNOWN_TOPIC_ID`, never `TOPIC_AUTHORIZATION_FAILED`, even
/// with a credential source configured and nothing granted. Below v13
/// the same logical case (a name that does not exist, also ungranted)
/// answers the opposite way, `topic_authorized` run against the raw
/// unresolved name — a real, version-dependent asymmetry this handler's
/// own `one_topic` doc names as structural (`TopicGrants` is keyed by
/// name, so an id must resolve to one before authorization can run at
/// all), not an oversight in either path.
#[tokio::test]
async fn at_or_above_v13_an_unresolvable_id_answers_unknown_not_authorization_failed() {
    let fixture = fixture(&[]).await;
    let grants = TopicGrants::new();
    let authz = AuthzContext {
        principal: Some(&alice()),
        credentials_configured: true,
        topic_grants: &grants,
    };
    let ghost = uuid::Uuid::from_u128(0xDEAD_BEEF);
    let body = produce_body_by_id(13, ghost, -1, golden_batch());
    let HandlerResponse::Reply(out) = handle(
        &fixture.cluster,
        &fixture.session,
        prelude(13),
        &body,
        &authz,
    )
    .await
    else {
        panic!("expected a reply");
    };
    let response = decode(&out, 13);
    assert_eq!(
        response.responses[0].partition_responses[0].error_code,
        kafka_protocol::error::ResponseError::UnknownTopicId.code(),
        "existence is resolved before authorization at v13+"
    );
}

/// The v13+ side of the ordinary granted case: a real topic id,
/// granted, still succeeds.
#[tokio::test]
async fn at_or_above_v13_a_granted_topic_by_id_answers() {
    let fixture = fixture(&["t"]).await;
    let id = fixture
        .cluster
        .topic_id("t")
        .expect("created by the fixture");
    let mut grants = TopicGrants::new();
    grants.grant(alice(), TopicId::new("t").expect("valid"));
    let authz = AuthzContext {
        principal: Some(&alice()),
        credentials_configured: true,
        topic_grants: &grants,
    };
    let body = produce_body_by_id(13, id, -1, golden_batch());
    let HandlerResponse::Reply(out) = handle(
        &fixture.cluster,
        &fixture.session,
        prelude(13),
        &body,
        &authz,
    )
    .await
    else {
        panic!("expected a reply");
    };
    let response = decode(&out, 13);
    assert_eq!(response.responses[0].partition_responses[0].error_code, 0);
}
