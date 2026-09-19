#![allow(clippy::expect_used)]

use super::Dispatcher;
use crate::connection::HandlerResponse;
use crate::testing::{Fixture, fixture};
use std::sync::Arc;

/// ⚠️ The fixture is returned alongside, and holding it is not a
/// formality: dropping it aborts the coordinator loop, and a dispatcher
/// over a dead coordinator answers every produce with a refusal.
async fn dispatcher() -> (Dispatcher, Fixture) {
    let fixture = fixture(&[]).await;
    let dispatcher = Dispatcher::new(Arc::clone(&fixture.cluster));
    (dispatcher, fixture)
}

/// The reply's bytes, or a panic naming the other verdict.
async fn replied(dispatcher: &Dispatcher, request: Vec<u8>) -> Vec<u8> {
    match dispatcher.dispatch(request).await {
        HandlerResponse::Reply(out) => out,
        other => panic!("expected a reply, got {other:?}"),
    }
}
use kafka_protocol::messages::ApiVersionsResponse;
use kafka_protocol::protocol::Decodable;
use oqueue_codec::wire::{Cursor, put_i16, put_i32};

fn api_versions_request(version: i16, correlation_id: i32) -> Vec<u8> {
    let mut body = Vec::new();
    put_i16(&mut body, 18);
    put_i16(&mut body, version);
    put_i32(&mut body, correlation_id);
    // client_id: legacy i16-length nullable string, null here.
    put_i16(&mut body, -1);
    if version >= 3 {
        body.push(0x00); // empty tagged fields (v2 request header)
    }
    body
}

fn decode_response(bytes: &[u8], body_version: i16) -> (i32, ApiVersionsResponse) {
    // ApiVersions responses carry a v0 header at every version: just
    // the correlation id.
    let mut cur = Cursor::new(bytes);
    let correlation = cur.read_i32().expect("a correlation id");
    let mut rest = &bytes[4..];
    let response = ApiVersionsResponse::decode(&mut rest, body_version).expect("a decodable body");
    assert!(rest.is_empty(), "nothing after the body");
    (correlation, response)
}

#[tokio::test]
async fn a_supported_version_is_answered_at_that_version() {
    let (dispatcher, _fixture) = dispatcher().await;
    let out = replied(&dispatcher, api_versions_request(3, 77)).await;
    let (correlation, response) = decode_response(&out, 3);
    assert_eq!(correlation, 77);
    assert_eq!(response.error_code, 0);
    let produce = response
        .api_keys
        .iter()
        .find(|k| k.api_key == 0)
        .expect("produce is advertised");
    assert_eq!((produce.min_version, produce.max_version), (3, 13));
}

/// ⚠️ **`M9.6`'s own check**: `ApiVersions` still answers with no `SaslHandshake`
/// or `SaslAuthenticate` exchange having happened at all — not TLS-terminated,
/// no credentials configured, nothing. `M9.3`-`M9.5` added a real SASL
/// mechanism and TLS capability beside this dispatcher; neither moved
/// `ApiVersions` behind them. `M9.7`'s future authorization gate is the
/// thing this test is written to catch, should it ever wrap `ApiVersions`
/// by accident rather than by the deliberate exemption doc 02 §1.4 point 2
/// requires.
#[tokio::test]
async fn api_versions_answers_with_no_sasl_exchange_and_no_tls() {
    let (dispatcher, _fixture) = dispatcher().await;
    let out = replied(&dispatcher, api_versions_request(3, 1)).await;
    let (_correlation, response) = decode_response(&out, 3);
    assert_eq!(
        response.error_code, 0,
        "unauthenticated, unencrypted, still answered"
    );
}

#[tokio::test]
async fn an_unsupported_version_gets_the_v0_bodied_fallback() {
    let (dispatcher, _fixture) = dispatcher().await;
    let out = replied(&dispatcher, api_versions_request(99, 5)).await;
    let (correlation, response) = decode_response(&out, 0);
    assert_eq!(correlation, 5);
    assert_eq!(
        response.error_code,
        kafka_protocol::error::ResponseError::UnsupportedVersion.code()
    );
    assert!(
        !response.api_keys.is_empty(),
        "the table rides the fallback so the client can retry"
    );
}

#[tokio::test]
async fn an_unknown_api_key_closes_the_connection() {
    let mut body = Vec::new();
    put_i16(&mut body, 0x7F00);
    put_i16(&mut body, 0);
    put_i32(&mut body, 1);
    let (dispatcher, _fixture) = dispatcher().await;
    assert_eq!(dispatcher.dispatch(body).await, HandlerResponse::Close);
}

#[tokio::test]
async fn an_advertised_api_at_an_unadvertised_version_closes() {
    let mut body = Vec::new();
    put_i16(&mut body, 0); // Produce
    put_i16(&mut body, 2); // removed by KIP-896, below the floor
    put_i32(&mut body, 1);
    let (dispatcher, _fixture) = dispatcher().await;
    assert_eq!(
        dispatcher.dispatch(body).await,
        HandlerResponse::Close,
        "outside ApiVersions' fallback there is nothing parsable to say"
    );
}

/// End to end through the connection task: the dispatcher is a Handler,
/// and the bytes on the wire carry the v0 header both ways.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_versions_round_trips_through_a_connection() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let (mut client, server) = tokio::io::duplex(4096);
    let limits = crate::ConnectionLimits {
        max_frame: 1024,
        max_in_flight: 4,
        idle_timeout: std::time::Duration::from_hours(1),
    };
    let (dispatcher, _fixture) = dispatcher().await;
    let conn = tokio::spawn(crate::serve_connection(
        server,
        Arc::new(dispatcher),
        limits,
    ));

    let body = api_versions_request(3, 42);
    let mut frame = Vec::new();
    put_i32(&mut frame, i32::try_from(body.len()).expect("small"));
    frame.extend_from_slice(&body);
    client.write_all(&frame).await.expect("request written");

    let mut size = [0u8; 4];
    client.read_exact(&mut size).await.expect("a response size");
    let mut response = vec![0u8; u32::from_be_bytes(size) as usize];
    client.read_exact(&mut response).await.expect("a response");
    let (correlation, decoded) = decode_response(&response, 3);
    assert_eq!(correlation, 42);
    assert_eq!(decoded.error_code, 0);

    drop(client);
    conn.await.expect("joins").expect("clean close");
}

/// The close decision travels the whole stack: an unknown api key ends
/// the connection promptly — the client sees EOF, not a timeout — which
/// is the seam hole round 1's review found (no test drove a `None`
/// through `serve_connection`).
#[tokio::test(start_paused = true)]
async fn an_unknown_key_closes_the_connection_promptly() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let (mut client, server) = tokio::io::duplex(4096);
    let limits = crate::ConnectionLimits {
        max_frame: 1024,
        max_in_flight: 4,
        idle_timeout: std::time::Duration::from_hours(1),
    };
    let (dispatcher, _fixture) = dispatcher().await;
    let conn = tokio::spawn(crate::serve_connection(
        server,
        Arc::new(dispatcher),
        limits,
    ));

    let mut body = Vec::new();
    put_i16(&mut body, 0x7F00);
    put_i16(&mut body, 0);
    put_i32(&mut body, 9);
    let mut frame = Vec::new();
    put_i32(&mut frame, i32::try_from(body.len()).expect("small"));
    frame.extend_from_slice(&body);
    client.write_all(&frame).await.expect("request written");

    // EOF, promptly: under paused time a wrong implementation would sit
    // until the hour-long idle timeout; a correct one closes now.
    let mut buf = [0u8; 1];
    let read = client.read(&mut buf).await.expect("a read completes");
    assert_eq!(read, 0, "the broker closed rather than answering");
    conn.await
        .expect("joins")
        .expect("a clean, deliberate close");
}

/// `M9.7`'s own tests: the authorization decision point, exercised
/// through `Dispatcher::dispatch` rather than `sasl_authenticate::handle`
/// directly, so the wiring line itself — `self.tls`, `&self.credentials`,
/// `&self.session` all threaded through — is what is under test, not
/// just the function it calls.
mod authorization {
    use super::{Dispatcher, HandlerResponse, dispatcher};
    use crate::sasl_authenticate::{PlainCredential, PlainCredentials};
    use kafka_protocol::messages::metadata_request::MetadataRequestTopic;
    use kafka_protocol::messages::{
        InitProducerIdRequest, MetadataRequest, MetadataResponse, RequestHeader, TopicName,
    };
    use kafka_protocol::protocol::{Decodable, Encodable, StrBytes};
    use oqueue_codec::apikey::ApiKey;
    use oqueue_core::{Principal, Redacted, TopicGrants, TopicId};

    fn framed(api_key: ApiKey, version: i16, body: &[u8]) -> Vec<u8> {
        let mut frame = Vec::new();
        let mut header = RequestHeader::default();
        header.request_api_key = api_key.as_i16();
        header.request_api_version = version;
        header.correlation_id = 7;
        header
            .encode(&mut frame, api_key.request_header_version(version))
            .expect("header encodes");
        frame.extend_from_slice(body);
        frame
    }

    /// `InitProducerId`'s minimal non-transactional body — the smallest
    /// gated request in the matrix, `matrix.rs`'s own reason to give it a
    /// dedicated encoder too.
    fn init_producer_id_frame() -> Vec<u8> {
        let mut body = Vec::new();
        let mut request = InitProducerIdRequest::default();
        request.transactional_id = None;
        request.transaction_timeout_ms = 30_000;
        request.encode(&mut body, 4).expect("encodes");
        framed(ApiKey::InitProducerId, 4, &body)
    }

    fn sasl_authenticate_frame(authcid: &str, password: &str) -> Vec<u8> {
        let mut body = Vec::new();
        let mut auth_bytes = vec![0u8];
        auth_bytes.extend_from_slice(authcid.as_bytes());
        auth_bytes.push(0);
        auth_bytes.extend_from_slice(password.as_bytes());
        kafka_protocol::messages::SaslAuthenticateRequest::default()
            .with_auth_bytes(bytes::Bytes::from(auth_bytes))
            .encode(&mut body, 1)
            .expect("encodes");
        framed(ApiKey::SaslAuthenticate, 1, &body)
    }

    fn one_credential() -> PlainCredentials {
        PlainCredentials::new(vec![PlainCredential {
            principal: Principal::new("alice").expect("valid"),
            password: Redacted::new("secret".to_owned()),
        }])
    }

    /// A `Metadata` request naming exactly `topic`, v12.
    fn metadata_frame(topic: &str) -> Vec<u8> {
        let mut body = Vec::new();
        let mut request = MetadataRequest::default();
        let mut entry = MetadataRequestTopic::default();
        entry.name = Some(TopicName(StrBytes::from_string(topic.to_owned())));
        request.topics = Some(vec![entry]);
        request.encode(&mut body, 12).expect("encodes");
        framed(ApiKey::Metadata, 12, &body)
    }

    fn metadata_response(out: &[u8]) -> MetadataResponse {
        // v12 carries a v1 (tagged) response header: 5 bytes.
        let mut rest = &out[5..];
        let response = MetadataResponse::decode(&mut rest, 12).expect("decodes");
        assert!(rest.is_empty());
        response
    }

    /// The `credentials`-empty default (`M9.3`'s and `M9.4`'s own shipped
    /// state, `matrix.rs`'s own fixture) is not newly locked out by this
    /// gate — every existing call site keeps answering with no SASL
    /// exchange at all.
    #[tokio::test]
    async fn an_unconfigured_broker_answers_with_no_sasl_exchange() {
        let (dispatcher, _fixture) = dispatcher().await;
        assert!(matches!(
            dispatcher.dispatch(init_producer_id_frame()).await,
            HandlerResponse::Reply(_)
        ));
    }

    /// Once a credential source exists, an unauthenticated connection is
    /// refused every other API — `oqueue_core::authorize`'s fail-closed
    /// half, reached through the real dispatcher wiring this time.
    #[tokio::test]
    async fn a_configured_broker_refuses_an_unauthenticated_connection() {
        let (dispatcher, _fixture) = dispatcher().await;
        let dispatcher = Dispatcher {
            credentials: std::sync::Arc::new(one_credential()),
            ..dispatcher
        };
        assert_eq!(
            dispatcher.dispatch(init_producer_id_frame()).await,
            HandlerResponse::Close,
        );
    }

    /// And once that same connection completes `SaslAuthenticate` over
    /// TLS, the same request the previous test closed now answers —
    /// `Session::authenticate`'s write is what the gate reads.
    #[tokio::test]
    async fn a_configured_broker_answers_after_a_successful_exchange() {
        let (dispatcher, _fixture) = dispatcher().await;
        let dispatcher = Dispatcher {
            tls: true,
            credentials: std::sync::Arc::new(one_credential()),
            ..dispatcher
        };
        let auth = dispatcher
            .dispatch(sasl_authenticate_frame("alice", "secret"))
            .await;
        assert!(matches!(auth, HandlerResponse::Reply(_)), "SASL succeeds");

        assert!(matches!(
            dispatcher.dispatch(init_producer_id_frame()).await,
            HandlerResponse::Reply(_)
        ));
    }

    /// ⚠️ **`M9.9`'s own end-to-end wiring test.** Every seam this milestone
    /// built, driven together: `tls_terminated`, `with_credentials`,
    /// `with_topic_grants`, a real `SaslAuthenticate` exchange, and then a
    /// real `Metadata` request — a granted topic resolves, an ungranted one
    /// answers `TOPIC_AUTHORIZATION_FAILED`, on the same authenticated
    /// connection, in the same response.
    #[tokio::test]
    async fn a_metadata_request_after_authentication_is_scoped_by_topic_grants() {
        let (dispatcher, fixture) = dispatcher().await;
        fixture.cluster.ensure_topic("seen").await;
        fixture.cluster.ensure_topic("unseen").await;
        let mut grants = TopicGrants::new();
        grants.grant(
            Principal::new("alice").expect("valid"),
            TopicId::new("seen").expect("valid"),
        );
        let dispatcher = Dispatcher {
            tls: true,
            credentials: std::sync::Arc::new(one_credential()),
            topic_grants: std::sync::Arc::new(grants),
            ..dispatcher
        };
        let auth = dispatcher
            .dispatch(sasl_authenticate_frame("alice", "secret"))
            .await;
        assert!(matches!(auth, HandlerResponse::Reply(_)), "SASL succeeds");

        let HandlerResponse::Reply(seen_out) = dispatcher.dispatch(metadata_frame("seen")).await
        else {
            panic!("expected a reply");
        };
        assert_eq!(metadata_response(&seen_out).topics[0].error_code, 0);

        let HandlerResponse::Reply(unseen_out) =
            dispatcher.dispatch(metadata_frame("unseen")).await
        else {
            panic!("expected a reply");
        };
        assert_eq!(
            metadata_response(&unseen_out).topics[0].error_code,
            oqueue_codec::error_codes::TOPIC_AUTHORIZATION_FAILED,
        );
    }

    /// A `Produce` request naming exactly `topic`, one partition, one
    /// record, `acks=-1`.
    fn produce_frame(topic: &str) -> Vec<u8> {
        use kafka_protocol::messages::ProduceRequest;
        use kafka_protocol::messages::produce_request::{PartitionProduceData, TopicProduceData};

        let mut body = Vec::new();
        let mut request = ProduceRequest::default();
        request.acks = -1;
        let mut t = TopicProduceData::default();
        t.name = TopicName(StrBytes::from_string(topic.to_owned()));
        let mut p = PartitionProduceData::default();
        p.index = 0;
        p.records = Some(bytes::Bytes::from(crate::testing::golden_batch()));
        t.partition_data.push(p);
        request.topic_data.push(t);
        request.encode(&mut body, 9).expect("encodes");
        framed(ApiKey::Produce, 9, &body)
    }

    fn produce_response(out: &[u8]) -> kafka_protocol::messages::ProduceResponse {
        // v9 is flexible: the response header carries the tagged-fields
        // byte, 5 bytes total.
        let mut rest = &out[5..];
        let response =
            kafka_protocol::messages::ProduceResponse::decode(&mut rest, 9).expect("decodes");
        assert!(rest.is_empty());
        response
    }

    /// ⚠️ **`M9.12`'s own end-to-end wiring test.** The same real stack
    /// `M9.9`'s test drives, this time through `Produce`: a granted topic's
    /// write lands, an ungranted one is refused before it ever reaches the
    /// bundle — proven by the watermark never moving for it.
    #[tokio::test]
    async fn a_produce_request_after_authentication_is_scoped_by_topic_grants() {
        let (dispatcher, fixture) = dispatcher().await;
        fixture.cluster.ensure_topic("seen").await;
        fixture.cluster.ensure_topic("unseen").await;
        let mut grants = TopicGrants::new();
        grants.grant(
            Principal::new("alice").expect("valid"),
            TopicId::new("seen").expect("valid"),
        );
        let dispatcher = Dispatcher {
            tls: true,
            credentials: std::sync::Arc::new(one_credential()),
            topic_grants: std::sync::Arc::new(grants),
            ..dispatcher
        };
        let auth = dispatcher
            .dispatch(sasl_authenticate_frame("alice", "secret"))
            .await;
        assert!(matches!(auth, HandlerResponse::Reply(_)), "SASL succeeds");

        let HandlerResponse::Reply(unseen_out) = dispatcher.dispatch(produce_frame("unseen")).await
        else {
            panic!("expected a reply");
        };
        assert_eq!(
            produce_response(&unseen_out).responses[0].partition_responses[0].error_code,
            oqueue_codec::error_codes::TOPIC_AUTHORIZATION_FAILED,
        );
        assert_eq!(
            fixture
                .cluster
                .high_watermark(
                    &TopicId::new("unseen").expect("valid"),
                    crate::testing::partition(0)
                )
                .get(),
            0,
            "a refused produce writes nothing"
        );

        let HandlerResponse::Reply(seen_out) = dispatcher.dispatch(produce_frame("seen")).await
        else {
            panic!("expected a reply");
        };
        assert_eq!(
            produce_response(&seen_out).responses[0].partition_responses[0].error_code,
            0
        );
    }
}
