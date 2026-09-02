#![allow(clippy::expect_used)]

use super::handle;
use crate::testing::{at, fixture};
use kafka_protocol::messages::metadata_request::MetadataRequestTopic;
use kafka_protocol::messages::{MetadataRequest, MetadataResponse, TopicName};
use kafka_protocol::protocol::{Decodable, Encodable, StrBytes};
use oqueue_codec::apikey::ApiKey;
use oqueue_codec::frame::RequestPrelude;

fn request_bytes(version: i16, topics: Option<Vec<&str>>, allow_create: bool) -> Vec<u8> {
    let mut request = MetadataRequest::default();
    request.topics = topics.map(|names| {
        names
            .into_iter()
            .map(|n| {
                let mut t = MetadataRequestTopic::default();
                t.name = Some(TopicName(StrBytes::from_string(n.to_owned())));
                t
            })
            .collect()
    });
    request.allow_auto_topic_creation = allow_create;
    let mut out = Vec::new();
    request.encode(&mut out, version).expect("encodes");
    out
}

fn prelude(version: i16) -> RequestPrelude {
    RequestPrelude {
        api_key: 3,
        api_version: version,
        correlation_id: 11,
    }
}

/// The reply's bytes, or a panic naming the other verdict — no
/// credential source configured, `M9.9`'s own fail-open default, so
/// every existing test here keeps its pre-`M9` meaning unchanged.
fn answered(cluster: &crate::Cluster, prelude: RequestPrelude, body: &[u8]) -> Vec<u8> {
    answered_as(
        cluster,
        prelude,
        body,
        &super::AuthzContext {
            principal: None,
            credentials_configured: false,
            topic_grants: &oqueue_core::TopicGrants::default(),
        },
    )
}

/// `answered`, with authorization actually live — `M9.9`'s own tests.
fn answered_as(
    cluster: &crate::Cluster,
    prelude: RequestPrelude,
    body: &[u8],
    authz: &super::AuthzContext<'_>,
) -> Vec<u8> {
    match handle(cluster, prelude, body, authz) {
        crate::connection::HandlerResponse::Reply(out) => out,
        other => panic!("expected a reply, got {other:?}"),
    }
}

fn decode(bytes: &[u8], version: i16) -> MetadataResponse {
    // Metadata responses: header v0 below v9, v1 (tagged) from v9.
    let header_len = if version >= 9 { 5 } else { 4 };
    let mut rest = &bytes[header_len..];
    let r = MetadataResponse::decode(&mut rest, version).expect("decodes");
    assert!(rest.is_empty());
    r
}

#[tokio::test]
async fn v12_with_the_flag_creates_and_answers() {
    let fixture = at("h.example", 9092, &[]).await;
    let cluster = &fixture.cluster;
    let body = request_bytes(12, Some(vec!["orders"]), true);
    let out = answered(cluster, prelude(12), &body);
    let response = decode(&out, 12);
    assert_eq!(response.brokers.len(), 1);
    assert_eq!(response.brokers[0].port, 9092);
    assert_eq!(response.topics.len(), 1);
    assert_eq!(response.topics[0].error_code, 0);
    assert_eq!(response.topics[0].partitions.len(), 1);
    assert_eq!(cluster.partition_count("orders"), Some(1));
    // From v10 the topic's id rides along -- how id-addressed produce
    // and fetch learn their targets.
    assert_eq!(
        response.topics[0].topic_id,
        cluster.topic_id("orders").expect("created with an id")
    );
    assert_ne!(response.topics[0].topic_id, uuid::Uuid::nil());
}

#[tokio::test]
async fn v12_without_the_flag_refuses_the_missing_topic() {
    let fixture = fixture(&[]).await;
    let cluster = &fixture.cluster;
    let body = request_bytes(12, Some(vec!["ghost"]), false);
    let out = answered(cluster, prelude(12), &body);
    let response = decode(&out, 12);
    assert_eq!(
        response.topics[0].error_code,
        kafka_protocol::error::ResponseError::UnknownTopicOrPartition.code()
    );
    assert_eq!(
        cluster.partition_count("ghost"),
        None,
        "nothing was created"
    );
}

#[tokio::test]
async fn a_null_topic_list_answers_everything() {
    let fixture = fixture(&[]).await;
    let cluster = &fixture.cluster;
    cluster.ensure_topic("a");
    cluster.ensure_topic("b");
    let body = request_bytes(12, None, false);
    let out = answered(cluster, prelude(12), &body);
    let response = decode(&out, 12);
    let mut names: Vec<String> = response
        .topics
        .iter()
        .map(|t| t.name.as_ref().expect("named").to_string())
        .collect();
    names.sort();
    assert_eq!(names, ["a", "b"]);
}

#[tokio::test]
async fn below_v4_the_wire_has_no_flag_and_creation_is_the_default() {
    let fixture = fixture(&[]).await;
    let cluster = &fixture.cluster;
    // The field is not on the v1 wire at all -- the dependency's encoder
    // refuses a non-default value there, which itself proves the claim --
    // and the decoder defaults it true, the historical behaviour this
    // handler inherits.
    let body = request_bytes(1, Some(vec!["implicit"]), true);
    let out = answered(cluster, prelude(1), &body);
    let response = decode(&out, 1);
    assert_eq!(response.topics[0].error_code, 0);
    assert_eq!(cluster.partition_count("implicit"), Some(1));
}

#[tokio::test]
async fn v0_empty_array_means_all_topics() {
    let fixture = fixture(&[]).await;
    let cluster = &fixture.cluster;
    cluster.ensure_topic("v0-visible");
    let body = request_bytes(0, Some(vec![]), true);
    let out = answered(cluster, prelude(0), &body);
    let response = decode(&out, 0);
    assert_eq!(response.topics.len(), 1, "v0's empty array is all-topics");
}

#[tokio::test]
async fn from_v1_an_empty_array_means_no_topics() {
    let fixture = fixture(&[]).await;
    let cluster = &fixture.cluster;
    cluster.ensure_topic("hidden");
    let body = request_bytes(12, Some(vec![]), true);
    let out = answered(cluster, prelude(12), &body);
    let response = decode(&out, 12);
    assert!(response.topics.is_empty());
}

/// The whole route through the dispatcher — `supports()` gate, header
/// decode, body slice — not just the handler (round 1's review noted a
/// mis-slice would close every connection with nothing failing here).
#[tokio::test]
async fn metadata_routes_through_the_dispatcher() {
    use kafka_protocol::messages::RequestHeader;
    let fixture = at("routed.example", 7, &[]).await;
    let cluster = std::sync::Arc::clone(&fixture.cluster);
    let dispatcher = crate::Dispatcher::new(std::sync::Arc::clone(&cluster));

    let mut request = Vec::new();
    let mut header = RequestHeader::default();
    header.request_api_key = 3;
    header.request_api_version = 12;
    header.correlation_id = 21;
    header
        .encode(&mut request, ApiKey::Metadata.request_header_version(12))
        .expect("header encodes");
    request.extend_from_slice(&request_bytes(12, Some(vec!["routed"]), true));

    let out = match dispatcher.dispatch(request).await {
        crate::connection::HandlerResponse::Reply(out) => out,
        other => panic!("expected a reply, got {other:?}"),
    };
    let response = decode(&out, 12);
    assert_eq!(response.brokers[0].host.as_str(), "routed.example");
    assert_eq!(response.topics[0].error_code, 0);
    assert_eq!(cluster.partition_count("routed"), Some(1));
}

#[tokio::test]
async fn the_advertised_identity_is_the_configured_one() {
    let fixture = at("adv.example.test", 31234, &[]).await;
    let cluster = &fixture.cluster;
    let body = request_bytes(9, None, false);
    let out = answered(cluster, prelude(9), &body);
    let response = decode(&out, 9);
    assert_eq!(response.brokers[0].host.as_str(), "adv.example.test");
    assert_eq!(response.brokers[0].port, 31234);
    let _ = ApiKey::Metadata;
}

/// `M9.9`'s own tests: an explicitly-named topic, scoped through
/// `TopicGrants`, once a credential source makes authorization live.
mod authorization {
    use super::{answered_as, decode, prelude, request_bytes};
    use crate::metadata::AuthzContext;
    use crate::testing::fixture;
    use oqueue_core::{Principal, TopicGrants, TopicId};

    fn alice() -> Principal {
        Principal::new("alice").expect("valid")
    }

    fn authz<'a>(
        principal: Option<&'a Principal>,
        credentials_configured: bool,
        topic_grants: &'a TopicGrants,
    ) -> AuthzContext<'a> {
        AuthzContext {
            principal,
            credentials_configured,
            topic_grants,
        }
    }

    #[tokio::test]
    async fn an_unconfigured_broker_answers_every_explicit_topic_regardless_of_grants() {
        let fixture = fixture(&["t"]).await;
        let body = request_bytes(12, Some(vec!["t"]), false);
        let out = answered_as(
            &fixture.cluster,
            prelude(12),
            &body,
            &authz(None, false, &TopicGrants::default()),
        );
        let response = decode(&out, 12);
        assert_eq!(response.topics[0].error_code, 0, "fail-open, unconfigured");
    }

    #[tokio::test]
    async fn a_configured_broker_answers_a_granted_topic() {
        let fixture = fixture(&["t"]).await;
        let mut grants = TopicGrants::new();
        grants.grant(alice(), TopicId::new("t").expect("valid"));
        let body = request_bytes(12, Some(vec!["t"]), false);
        let out = answered_as(
            &fixture.cluster,
            prelude(12),
            &body,
            &authz(Some(&alice()), true, &grants),
        );
        let response = decode(&out, 12);
        assert_eq!(response.topics[0].error_code, 0);
    }

    #[tokio::test]
    async fn a_configured_broker_refuses_an_ungranted_topic() {
        let fixture = fixture(&["t"]).await;
        let grants = TopicGrants::new();
        let body = request_bytes(12, Some(vec!["t"]), false);
        let out = answered_as(
            &fixture.cluster,
            prelude(12),
            &body,
            &authz(Some(&alice()), true, &grants),
        );
        let response = decode(&out, 12);
        assert_eq!(
            response.topics[0].error_code,
            oqueue_codec::error_codes::TOPIC_AUTHORIZATION_FAILED,
        );
    }

    /// ⚠️ **A refusal never auto-creates.** `resolve_topic`'s own
    /// `allow_auto_topic_creation` path must not run at all for a topic
    /// this principal was never granted — an unauthorized principal
    /// naming a topic that does not exist must not be the way it comes
    /// to exist.
    #[tokio::test]
    async fn refusing_an_ungranted_topic_does_not_create_it() {
        let fixture = fixture(&[]).await;
        let grants = TopicGrants::new();
        let body = request_bytes(12, Some(vec!["ghost"]), true);
        let out = answered_as(
            &fixture.cluster,
            prelude(12),
            &body,
            &authz(Some(&alice()), true, &grants),
        );
        let response = decode(&out, 12);
        assert_eq!(
            response.topics[0].error_code,
            oqueue_codec::error_codes::TOPIC_AUTHORIZATION_FAILED,
        );
        assert_eq!(
            fixture.cluster.partition_count("ghost"),
            None,
            "an unauthorized request must not create the topic it named"
        );
    }

    #[tokio::test]
    async fn a_configured_broker_with_no_principal_refuses_every_explicit_topic() {
        let fixture = fixture(&["t"]).await;
        let mut grants = TopicGrants::new();
        grants.grant(alice(), TopicId::new("t").expect("valid"));
        let body = request_bytes(12, Some(vec!["t"]), false);
        // ⚠️ Should be unreachable through the real dispatcher — `M9.7`'s
        // gate refuses an unauthenticated connection once credentials
        // are configured — but the handler must not panic or fail open
        // if it is ever reached this way regardless.
        let out = answered_as(
            &fixture.cluster,
            prelude(12),
            &body,
            &authz(None, true, &grants),
        );
        let response = decode(&out, 12);
        assert_eq!(
            response.topics[0].error_code,
            oqueue_codec::error_codes::TOPIC_AUTHORIZATION_FAILED,
        );
    }

    /// Two explicitly-named topics, one granted and one not, each get
    /// their own independent verdict — one refusal does not taint the
    /// other entry in the same response.
    #[tokio::test]
    async fn each_explicit_topic_is_scoped_independently() {
        let fixture = fixture(&["seen", "unseen"]).await;
        let mut grants = TopicGrants::new();
        grants.grant(alice(), TopicId::new("seen").expect("valid"));
        let body = request_bytes(12, Some(vec!["seen", "unseen"]), false);
        let out = answered_as(
            &fixture.cluster,
            prelude(12),
            &body,
            &authz(Some(&alice()), true, &grants),
        );
        let response = decode(&out, 12);
        let by_name = |name: &str| {
            response
                .topics
                .iter()
                .find(|t| t.name.as_deref().is_some_and(|n| n == name))
                .expect("named entry present")
        };
        assert_eq!(by_name("seen").error_code, 0);
        assert_eq!(
            by_name("unseen").error_code,
            oqueue_codec::error_codes::TOPIC_AUTHORIZATION_FAILED,
        );
    }

    /// ⚠️ **`M9.10`'s own known gap, asserted rather than assumed.** The
    /// null-topic-array case is not yet scoped — a configured,
    /// authenticated principal with zero grants still sees every topic
    /// through this path today. This test exists to fail loudly the
    /// moment `M9.10` closes it, so that task cannot land without
    /// updating the one test that documented the gap it is fixing.
    #[tokio::test]
    async fn the_null_topic_array_is_not_yet_scoped() {
        let fixture = fixture(&["unrelated"]).await;
        let grants = TopicGrants::new();
        let body = request_bytes(12, None, false);
        let out = answered_as(
            &fixture.cluster,
            prelude(12),
            &body,
            &authz(Some(&alice()), true, &grants),
        );
        let response = decode(&out, 12);
        assert_eq!(
            response.topics.len(),
            1,
            "every topic in the system, not yet every topic alice can see"
        );
    }
}
