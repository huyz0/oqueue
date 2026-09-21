#![allow(clippy::expect_used)]

use super::{answer, describe_one, supported_configs};
use crate::authz::{AdminAuthzContext, AuthzContext, topic_authorized};
use crate::testing::fixture;
use oqueue_codec::describe_configs::ConfigResource;
use oqueue_codec::error_codes;
use oqueue_codec::flex::{put_array_len, put_string};
use oqueue_codec::frame::RequestPrelude;
use oqueue_codec::wire::{put_bool, put_i8};
use oqueue_core::{AdminGrants, Principal, TopicGrants, TopicId};

fn authz<'a>(
    grants: &'a TopicGrants,
    principal: Option<&'a Principal>,
    configured: bool,
) -> AuthzContext<'a> {
    AuthzContext {
        principal,
        credentials_configured: configured,
        topic_grants: grants,
    }
}

fn topic_resource(names: Option<Vec<String>>) -> ConfigResource {
    ConfigResource {
        resource_type: 2,
        resource_name: "orders".to_owned(),
        config_names: names,
    }
}

fn topic(name: &str) -> TopicId {
    TopicId::new(name).expect("topic")
}

#[tokio::test]
async fn authorized_topic_reports_the_authoritative_retention_default() {
    let fixture = fixture(&[]).await;
    fixture
        .cluster
        .create_topic(&topic("orders"), 1)
        .await
        .expect("topic");
    let grants = TopicGrants::new();
    let result = describe_one(
        &fixture.cluster,
        &topic_resource(None),
        true,
        &authz(&grants, None, false),
    )
    .await;
    assert_eq!(result.error_code, error_codes::NONE);
    assert_eq!(result.configs.len(), 1);
    assert_eq!(result.configs[0].name, "retention.ms");
    assert_eq!(result.configs[0].value.as_deref(), Some("604800000"));
    assert!(result.configs[0].read_only);
    assert!(!result.configs[0].is_sensitive);
}

#[tokio::test]
async fn an_unauthorized_topic_is_not_resolved_or_enumerated() {
    let fixture = fixture(&[]).await;
    fixture
        .cluster
        .create_topic(&topic("orders"), 1)
        .await
        .expect("topic");
    let grants = TopicGrants::new();
    let result = describe_one(
        &fixture.cluster,
        &topic_resource(None),
        true,
        &authz(
            &grants,
            Some(&Principal::new("alice").expect("principal")),
            true,
        ),
    )
    .await;
    assert_eq!(result.error_code, error_codes::TOPIC_AUTHORIZATION_FAILED);
    assert!(result.configs.is_empty());
}

#[tokio::test]
async fn an_unsupported_resource_is_rejected_before_visibility_is_consulted() {
    let fixture = fixture(&[]).await;
    let grants = TopicGrants::new();
    let mut resource = topic_resource(None);
    resource.resource_type = 4;
    let result = describe_one(
        &fixture.cluster,
        &resource,
        true,
        &authz(
            &grants,
            Some(&Principal::new("alice").expect("principal")),
            true,
        ),
    )
    .await;
    assert_eq!(result.error_code, error_codes::INVALID_REQUEST);
}

#[tokio::test]
async fn a_visible_topic_still_requires_describe_configs_admin_authority() {
    use kafka_protocol::messages::DescribeConfigsResponse;
    use kafka_protocol::protocol::Decodable;

    let fixture = fixture(&[]).await;
    fixture
        .cluster
        .create_topic(&topic("orders"), 1)
        .await
        .expect("topic");
    let principal = Principal::new("alice").expect("principal");
    let mut topic_grants = TopicGrants::new();
    topic_grants.grant(principal.clone(), topic("orders"));
    let admin_grants = AdminGrants::new();
    let admin_authz = AdminAuthzContext {
        principal: Some(&principal),
        credentials_configured: true,
        admin_grants: &admin_grants,
    };
    let topic_authz = authz(&topic_grants, Some(&principal), true);
    let mut body = Vec::new();
    put_array_len(&mut body, false, Some(1));
    put_i8(&mut body, 2);
    put_string(&mut body, false, "orders");
    put_array_len(&mut body, false, None);
    put_bool(&mut body, false);
    let out = answer(
        &fixture.cluster,
        RequestPrelude {
            api_key: 32,
            api_version: 1,
            correlation_id: 3,
        },
        &body,
        &admin_authz,
        &topic_authz,
    )
    .await
    .expect("response");
    let mut response_bytes = &out[4..];
    let response = DescribeConfigsResponse::decode(&mut response_bytes, 1).expect("response");
    assert_eq!(
        response.results[0].error_code,
        error_codes::TOPIC_AUTHORIZATION_FAILED
    );
}

#[tokio::test]
async fn unsupported_and_secret_keys_have_a_generic_error_without_echoing_them() {
    let fixture = fixture(&[]).await;
    fixture
        .cluster
        .create_topic(&topic("orders"), 1)
        .await
        .expect("topic");
    let grants = TopicGrants::new();
    let resource = topic_resource(Some(vec!["ssl.keystore.password".to_owned()]));
    let result = describe_one(
        &fixture.cluster,
        &resource,
        true,
        &authz(&grants, None, false),
    )
    .await;
    assert_eq!(result.error_code, error_codes::INVALID_CONFIG);
    let message = result.error_message.expect("diagnostic");
    assert_eq!(message, "configuration is unsupported");
    assert!(!message.contains("password"));
}

#[test]
fn only_the_authoritative_setting_is_reported() {
    assert!(supported_configs(None).is_ok_and(|configs| configs.len() == 1));
    assert!(supported_configs(Some(&[])).is_ok_and(|configs| configs.is_empty()));
    assert!(supported_configs(Some(&[String::from("retention.ms")])).is_ok());
    assert!(supported_configs(Some(&[String::from("segment.bytes")])).is_err());
}

#[test]
fn configured_topic_visibility_stays_separate_from_admin_authority() {
    let grants = TopicGrants::new();
    let principal = Principal::new("alice").expect("principal");
    let context = authz(&grants, Some(&principal), true);
    assert!(!topic_authorized("orders", &context));
}
