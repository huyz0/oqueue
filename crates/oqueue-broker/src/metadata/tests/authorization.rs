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
    )
    .await;
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
    )
    .await;
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
    )
    .await;
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
    )
    .await;
    let response = decode(&out, 12);
    assert_eq!(
        response.topics[0].error_code,
        oqueue_codec::error_codes::TOPIC_AUTHORIZATION_FAILED,
    );
    assert_eq!(
        fixture.cluster.partition_count("ghost").await,
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
    )
    .await;
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
    )
    .await;
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

/// `M9.10`'s own case: a configured, authenticated principal with zero
/// grants sees **zero** topics through the null-topic-array path — not
/// every topic in the system (the pre-`M9.10` gap this test used to
/// document), and not a refusal either. Silent omission at the array
/// level, `M9.1`'s own verified shape.
#[tokio::test]
async fn a_null_topic_array_answers_nothing_for_a_principal_with_no_grants() {
    let fixture = fixture(&["unrelated"]).await;
    let grants = TopicGrants::new();
    let body = request_bytes(12, None, false);
    let out = answered_as(
        &fixture.cluster,
        prelude(12),
        &body,
        &authz(Some(&alice()), true, &grants),
    )
    .await;
    let response = decode(&out, 12);
    assert!(
        response.topics.is_empty(),
        "no grants, no topics — never every topic in the system"
    );
}

/// ⚠️ **Round 1 review's own finding, fixed.** A grant naming a topic
/// that does not exist must not appear at all through the
/// null-topic-array path — not auto-created, not answered
/// `UNKNOWN_TOPIC_OR_PARTITION` for a topic the client never named.
/// Real Kafka's "every topic" never invents one; `allow_auto_topic_creation`
/// is the *explicitly-named* case's own treatment (`M9.9`), a request the
/// client actually made.
#[tokio::test]
async fn a_grant_for_a_topic_that_does_not_exist_is_silently_omitted() {
    let fixture = fixture(&[]).await;
    let mut grants = TopicGrants::new();
    grants.grant(alice(), TopicId::new("never-created").expect("valid"));
    let body = request_bytes(12, None, true);
    let out = answered_as(
        &fixture.cluster,
        prelude(12),
        &body,
        &authz(Some(&alice()), true, &grants),
    )
    .await;
    let response = decode(&out, 12);
    assert!(
        response.topics.is_empty(),
        "a granted-but-nonexistent topic must not appear, auto-created or otherwise"
    );
    assert_eq!(
        fixture.cluster.partition_count("never-created").await,
        None,
        "listing must never create a topic as a side effect"
    );
}

/// A principal with some, but not all, topics granted sees exactly its
/// own through the null-topic-array path.
#[tokio::test]
async fn a_null_topic_array_answers_exactly_the_principals_own_grants() {
    let fixture = fixture(&["seen", "unseen"]).await;
    let mut grants = TopicGrants::new();
    grants.grant(alice(), TopicId::new("seen").expect("valid"));
    let body = request_bytes(12, None, false);
    let out = answered_as(
        &fixture.cluster,
        prelude(12),
        &body,
        &authz(Some(&alice()), true, &grants),
    )
    .await;
    let response = decode(&out, 12);
    let names: Vec<_> = response
        .topics
        .iter()
        .map(|t| t.name.as_ref().expect("named").to_string())
        .collect();
    assert_eq!(names, ["seen"]);
    assert_eq!(response.topics[0].error_code, 0);
}

/// An unconfigured broker's null-topic-array answer is unaffected —
/// `M9.9`'s own precedent, reused here rather than a second rule.
#[tokio::test]
async fn an_unconfigured_broker_answers_every_topic_through_the_null_array() {
    let fixture = fixture(&["a", "b"]).await;
    let body = request_bytes(12, None, false);
    let out = answered_as(
        &fixture.cluster,
        prelude(12),
        &body,
        &authz(None, false, &TopicGrants::default()),
    )
    .await;
    let response = decode(&out, 12);
    assert_eq!(response.topics.len(), 2, "fail-open, unconfigured");
}

/// The defensive no-principal case — should be unreachable past
/// `M9.7`'s own gate — answers empty rather than panicking.
#[tokio::test]
async fn a_configured_broker_with_no_principal_answers_no_topics_through_the_null_array() {
    let fixture = fixture(&["t"]).await;
    let mut grants = TopicGrants::new();
    grants.grant(alice(), TopicId::new("t").expect("valid"));
    let body = request_bytes(12, None, false);
    let out = answered_as(
        &fixture.cluster,
        prelude(12),
        &body,
        &authz(None, true, &grants),
    )
    .await;
    let response = decode(&out, 12);
    assert!(response.topics.is_empty());
}

/// ⚠️ **The cost claim's correctness half** (`M9.17` owns the timing
/// half): a principal's own null-topic-array answer is unaffected by
/// how many other principals or topics exist alongside it — the same
/// property `topic_grants.rs`'s own test asserts at the index level,
/// exercised here through the handler that actually serves a client.
#[tokio::test]
async fn a_principals_null_array_answer_is_unaffected_by_unrelated_grants() {
    let fixture = fixture(&["mine"]).await;
    let mut grants = TopicGrants::new();
    for n in 0..1_000 {
        fixture.cluster.ensure_topic(&format!("t{n}")).await;
        grants.grant(
            Principal::new(format!("tenant-{n}")).expect("valid"),
            TopicId::new(format!("t{n}")).expect("valid"),
        );
    }
    grants.grant(alice(), TopicId::new("mine").expect("valid"));
    let body = request_bytes(12, None, false);
    let before = fixture.cluster.topic_lookups();
    let out = answered_as(
        &fixture.cluster,
        prelude(12),
        &body,
        &authz(Some(&alice()), true, &grants),
    )
    .await;
    let response = decode(&out, 12);
    assert_eq!(response.topics.len(), 1);
    assert_eq!(
        response.topics[0].name.as_ref().expect("named").to_string(),
        "mine"
    );
    assert_eq!(
        fixture.cluster.topic_lookups() - before,
        3,
        "unrelated tenant grants must not add catalog work"
    );
}
