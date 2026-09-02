//! FR-2: every advertised API version is genuinely served.
//!
//! `ApiVersions` advertises a table; this test holds the broker to it.
//! For every `(api, version)` pair in `ADVERTISED`, a minimal valid
//! request through the public [`Handler`] seam gets a real answer —
//! never a closed connection, never error 35 — which is
//! `m2-complete.sh`'s "the matrix is honest" leg. ⚠️ **One row's own
//! documented behaviour is a refusal, not a success** —
//! [`expected_error_code`] is where that is named rather than assumed
//! away.

#![allow(clippy::expect_used)]

use crate::support::broker;
use kafka_protocol::messages::fetch_request::{FetchPartition, FetchTopic};
use kafka_protocol::messages::produce_request::{PartitionProduceData, TopicProduceData};
use kafka_protocol::messages::{
    ApiVersionsRequest, FetchRequest, InitProducerIdRequest, MetadataRequest, ProduceRequest,
    RequestHeader, SaslAuthenticateRequest, SaslHandshakeRequest, TopicName,
};
use kafka_protocol::protocol::{Decodable, Encodable, StrBytes};
use oqueue_broker::{Cluster, Dispatcher, Handler, HandlerResponse};
use oqueue_codec::apikey::ApiKey;
use oqueue_codec::versions::ADVERTISED;
use std::sync::Arc;

/// A full request frame: header at the API's header version, then `body`.
fn framed(api_key: ApiKey, version: i16, body: &[u8]) -> Vec<u8> {
    let mut frame = Vec::new();
    let mut header = RequestHeader::default();
    header.request_api_key = api_key.as_i16();
    header.request_api_version = version;
    header.correlation_id = 99;
    header
        .encode(&mut frame, api_key.request_header_version(version))
        .expect("header encodes");
    frame.extend_from_slice(body);
    frame
}

/// The smallest `ListOffsets` that asks a real question: partition 0 of `"t"`
/// at LATEST.
///
/// ⚠️ **Its own function so `minimal_body` stays under fifty lines**, and
/// because this is the one API whose minimal body is not `Default::default()`
/// — an empty topic list would be answered without touching a partition, which
/// is not what FR-2's matrix is asking.
fn list_offsets_body(out: &mut Vec<u8>, version: i16) {
    use kafka_protocol::messages::list_offsets_request::{ListOffsetsPartition, ListOffsetsTopic};
    let mut request = kafka_protocol::messages::ListOffsetsRequest::default();
    request.replica_id = kafka_protocol::messages::BrokerId(-1);
    let mut topic = ListOffsetsTopic::default();
    topic.name = TopicName(StrBytes::from_static_str("t"));
    let mut partition = ListOffsetsPartition::default();
    partition.partition_index = 0;
    // ⚠️ `-1` is LATEST, the sentinel a real consumer sends; a wall
    // clock timestamp is the one shape this broker refuses, and this
    // matrix asks for a real answer.
    partition.timestamp = -1;
    topic.partitions.push(partition);
    request.topics.push(topic);
    request.encode(out, version).expect("encodes");
}

/// Non-transactional: the only case `M11.4` mints a real identity for, so
/// it is the one this matrix must find served.
///
/// ⚠️ **Its own function for the same reason as `list_offsets_body`**: the
/// `minimal_body` match stays under fifty lines. ⚠️ The dependency's own
/// `Default` for `transactional_id` is `Some("")`, not `None` — set
/// explicitly or this becomes the refused, transactional case instead.
fn init_producer_id_body(out: &mut Vec<u8>, version: i16) {
    let mut request = InitProducerIdRequest::default();
    request.transactional_id = None;
    request.transaction_timeout_ms = 30_000;
    request.encode(out, version).expect("encodes");
}

/// `SaslHandshake`'s minimal body: the one mechanism `ADR-0032` enables.
fn sasl_handshake_body(out: &mut Vec<u8>, version: i16) {
    SaslHandshakeRequest::default()
        .with_mechanism(StrBytes::from_static_str("PLAIN"))
        .encode(out, version)
        .expect("encodes");
}

/// `SaslAuthenticate`'s minimal body — a synthetic credential `M9.3`'s own
/// handler refuses regardless, per `expected_error_code`.
fn sasl_authenticate_body(out: &mut Vec<u8>, version: i16) {
    SaslAuthenticateRequest::default()
        .with_auth_bytes(bytes::Bytes::from_static(b"\x00alice\x00secret"))
        .encode(out, version)
        .expect("encodes");
}

/// The smallest valid body for `api_key` at `version`, against a cluster
/// that has topic `"t"` — enough for a real answer, not an error dance.
fn minimal_body(api_key: ApiKey, version: i16, cluster: &Cluster) -> Vec<u8> {
    let mut body = Vec::new();
    match api_key {
        ApiKey::ApiVersions => {
            ApiVersionsRequest::default()
                .encode(&mut body, version)
                .expect("encodes");
        }
        ApiKey::ListOffsets => list_offsets_body(&mut body, version),
        ApiKey::Metadata => {
            MetadataRequest::default()
                .encode(&mut body, version)
                .expect("encodes");
        }
        ApiKey::Produce => produce_body(&mut body, version, cluster),
        ApiKey::Fetch => fetch_body(&mut body, version, cluster),
        ApiKey::InitProducerId => init_producer_id_body(&mut body, version),
        ApiKey::SaslHandshake => sasl_handshake_body(&mut body, version),
        ApiKey::SaslAuthenticate => sasl_authenticate_body(&mut body, version),
        ApiKey::FindCoordinator => find_coordinator_body(&mut body, version),
        ApiKey::JoinGroup => join_group_body(&mut body, version),
        ApiKey::SyncGroup => sync_group_body(&mut body, version),
        ApiKey::Heartbeat => heartbeat_body(&mut body, version),
        ApiKey::LeaveGroup => leave_group_body(&mut body, version),
    }
    body
}

/// `JoinGroup`'s own minimal body — one member, its own compatible
/// protocol, a rebalance timeout of `1`ms so a fresh group's own round (no
/// early-close signal, `join_group::round`'s own module doc) closes at its
/// own deadline almost immediately rather than holding this test open.
/// Every later version reuses the same group name, so from v1 on the round
/// closes the instant this single member rejoins (`expected` is `Some(1)`
/// from the version before) — its own function for the same
/// fifty-line-limit reason `list_offsets_body` is.
fn join_group_body(out: &mut Vec<u8>, version: i16) {
    use kafka_protocol::messages::JoinGroupRequest;
    use kafka_protocol::messages::join_group_request::JoinGroupRequestProtocol;
    let mut protocol = JoinGroupRequestProtocol::default();
    protocol.name = StrBytes::from_static_str("range");
    protocol.metadata = bytes::Bytes::from_static(b"m");
    let request = JoinGroupRequest::default()
        .with_group_id(kafka_protocol::messages::GroupId(
            StrBytes::from_static_str("matrix-group"),
        ))
        .with_session_timeout_ms(1)
        .with_rebalance_timeout_ms(1)
        .with_member_id(StrBytes::from_static_str(""))
        .with_protocol_type(StrBytes::from_static_str("consumer"))
        .with_protocols(vec![protocol]);
    request.encode(out, version).expect("encodes");
}

/// `SyncGroup`'s own minimal body -- a single member submitting its own
/// (non-empty) assignment, the shape `sync_group.rs`'s own handler treats
/// as the assignment-bearing submission regardless of `JoinGroup` ever
/// having run for this group (no fencing checked yet, `M4.11`'s own
/// scope), so this answers `error_code == 0` with no other setup.
fn sync_group_body(out: &mut Vec<u8>, version: i16) {
    use kafka_protocol::messages::SyncGroupRequest;
    use kafka_protocol::messages::sync_group_request::SyncGroupRequestAssignment;
    let mut assignment = SyncGroupRequestAssignment::default();
    assignment.member_id = StrBytes::from_static_str("m1");
    assignment.assignment = bytes::Bytes::from_static(b"a");
    let request = SyncGroupRequest::default()
        .with_group_id(kafka_protocol::messages::GroupId(
            StrBytes::from_static_str("matrix-sync-group"),
        ))
        .with_generation_id(1)
        .with_member_id(StrBytes::from_static_str("m1"))
        .with_assignments(vec![assignment]);
    request.encode(out, version).expect("encodes");
}

/// `Heartbeat`'s own minimal body -- against a group nothing ever joined,
/// so this genuinely answers `REBALANCE_IN_PROGRESS` (`expected_error_code`
/// names this the same way `SaslAuthenticate`'s own row is named: a
/// documented, non-zero, "genuinely served" answer, not `UNSUPPORTED_VERSION`).
fn heartbeat_body(out: &mut Vec<u8>, version: i16) {
    use kafka_protocol::messages::HeartbeatRequest;
    let request = HeartbeatRequest::default()
        .with_group_id(kafka_protocol::messages::GroupId(
            StrBytes::from_static_str("matrix-heartbeat-group"),
        ))
        .with_generation_id(1)
        .with_member_id(StrBytes::from_static_str("m1"));
    request.encode(out, version).expect("encodes");
}

/// `LeaveGroup`'s own minimal body -- one member leaving a group nothing
/// ever joined. Answers `error_code == 0` unconditionally: removal, not
/// fencing (`leave_group.rs`'s own doc) -- whether the member was ever
/// really there is `M4.11`'s own question.
fn leave_group_body(out: &mut Vec<u8>, version: i16) {
    use kafka_protocol::messages::LeaveGroupRequest;
    let request = if version <= 2 {
        LeaveGroupRequest::default()
            .with_group_id(kafka_protocol::messages::GroupId(
                StrBytes::from_static_str("matrix-leave-group"),
            ))
            .with_member_id(StrBytes::from_static_str("m1"))
    } else {
        use kafka_protocol::messages::leave_group_request::MemberIdentity;
        let mut member = MemberIdentity::default();
        member.member_id = StrBytes::from_static_str("m1");
        LeaveGroupRequest::default()
            .with_group_id(kafka_protocol::messages::GroupId(
                StrBytes::from_static_str("matrix-leave-group"),
            ))
            .with_members(vec![member])
    };
    request.encode(out, version).expect("encodes");
}

/// `Produce`'s own minimal body -- its own function for the same
/// fifty-line-limit reason `list_offsets_body` is.
fn produce_body(out: &mut Vec<u8>, version: i16, cluster: &Cluster) {
    let mut request = ProduceRequest::default();
    request.acks = -1;
    let mut topic = TopicProduceData::default();
    if version >= 13 {
        topic.topic_id = cluster.topic_id("t").expect("t exists");
    } else {
        topic.name = TopicName(StrBytes::from_static_str("t"));
    }
    let mut partition = PartitionProduceData::default();
    partition.index = 0;
    partition.records = Some(bytes::Bytes::from(golden_batch()));
    topic.partition_data.push(partition);
    request.topic_data.push(topic);
    request.encode(out, version).expect("encodes");
}

/// `Fetch`'s own minimal body -- its own function for the same
/// fifty-line-limit reason `list_offsets_body` is.
fn fetch_body(out: &mut Vec<u8>, version: i16, cluster: &Cluster) {
    let mut request = FetchRequest::default();
    request.max_wait_ms = 0;
    let mut topic = FetchTopic::default();
    if version >= 13 {
        topic.topic_id = cluster.topic_id("t").expect("t exists");
    } else {
        topic.topic = TopicName(StrBytes::from_static_str("t"));
    }
    let mut partition = FetchPartition::default();
    partition.partition = 0;
    partition.partition_max_bytes = 1 << 20;
    topic.partitions.push(partition);
    request.topics.push(topic);
    request.encode(out, version).expect("encodes");
}

/// `FindCoordinator`'s own minimal body — its own function for the same
/// fifty-line-limit reason `list_offsets_body` is.
fn find_coordinator_body(out: &mut Vec<u8>, version: i16) {
    let request = if version >= 4 {
        kafka_protocol::messages::FindCoordinatorRequest::default()
            .with_coordinator_keys(vec![StrBytes::from_static_str("g")])
    } else {
        kafka_protocol::messages::FindCoordinatorRequest::default()
            .with_key(StrBytes::from_static_str("g"))
    };
    request.encode(out, version).expect("encodes");
}

/// Response header length: v0 is 4 bytes of correlation id, v1 adds the
/// empty tagged-fields byte. `ApiVersions`/`SaslHandshake` stay v0 at
/// every version — the generated `response_header_version` carries both
/// special cases.
fn response_header_len(api_key: ApiKey, version: i16) -> usize {
    if api_key.response_header_version(version) >= 1 {
        5
    } else {
        4
    }
}

/// Decode the reply body with the client half at the request's version —
/// a reply that only *looks* like one (wrong header, mis-versioned body)
/// fails here rather than passing as opaque bytes. Returns `ApiVersions`'
/// top-level error code, 0 for the APIs that have none.
fn decode_reply(api_key: ApiKey, version: i16, reply: &[u8]) -> i16 {
    use kafka_protocol::messages::{FetchResponse, MetadataResponse, ProduceResponse};
    let mut rest = &reply[response_header_len(api_key, version)..];
    let error_code = match api_key {
        ApiKey::ApiVersions => {
            kafka_protocol::messages::ApiVersionsResponse::decode(&mut rest, version)
                .expect("ApiVersions reply decodes")
                .error_code
        }
        ApiKey::ListOffsets => {
            kafka_protocol::messages::ListOffsetsResponse::decode(&mut rest, version)
                .expect("ListOffsets reply decodes")
                .topics[0]
                .partitions[0]
                .error_code
        }
        ApiKey::Metadata => decode_ignoring_body::<MetadataResponse>(&mut rest, api_key, version),
        ApiKey::Produce => decode_ignoring_body::<ProduceResponse>(&mut rest, api_key, version),
        ApiKey::Fetch => decode_ignoring_body::<FetchResponse>(&mut rest, api_key, version),
        ApiKey::InitProducerId => {
            kafka_protocol::messages::InitProducerIdResponse::decode(&mut rest, version)
                .expect("InitProducerId reply decodes")
                .error_code
        }
        ApiKey::SaslHandshake => {
            kafka_protocol::messages::SaslHandshakeResponse::decode(&mut rest, version)
                .expect("SaslHandshake reply decodes")
                .error_code
        }
        ApiKey::SaslAuthenticate => {
            kafka_protocol::messages::SaslAuthenticateResponse::decode(&mut rest, version)
                .expect("SaslAuthenticate reply decodes")
                .error_code
        }
        ApiKey::FindCoordinator => find_coordinator_error_code(&mut rest, version),
        ApiKey::JoinGroup | ApiKey::SyncGroup | ApiKey::Heartbeat => {
            group_protocol_error_code(api_key, &mut rest, version)
        }
        ApiKey::LeaveGroup => {
            kafka_protocol::messages::LeaveGroupResponse::decode(&mut rest, version)
                .expect("LeaveGroup reply decodes")
                .error_code
        }
    };
    assert!(
        rest.is_empty(),
        "{api_key:?} v{version}: nothing after the body"
    );
    error_code
}

/// `FindCoordinator`'s own decode -- its own function for the same
/// fifty-line-limit reason `list_offsets_body` is.
fn find_coordinator_error_code(rest: &mut &[u8], version: i16) -> i16 {
    let response = kafka_protocol::messages::FindCoordinatorResponse::decode(rest, version)
        .expect("FindCoordinator reply decodes");
    // v0-3: the top-level field. v4+: the one requested key's own entry --
    // the top-level field does not exist on the wire past v3.
    if version >= 4 {
        response.coordinators[0].error_code
    } else {
        response.error_code
    }
}

/// `JoinGroup`/`SyncGroup`/`Heartbeat`'s own decode -- one function since
/// all three answer a bare `error_code`, its own function for the same
/// fifty-line-limit reason `list_offsets_body` is.
fn group_protocol_error_code(api_key: ApiKey, rest: &mut &[u8], version: i16) -> i16 {
    match api_key {
        ApiKey::JoinGroup => {
            kafka_protocol::messages::JoinGroupResponse::decode(rest, version)
                .expect("JoinGroup reply decodes")
                .error_code
        }
        ApiKey::SyncGroup => {
            kafka_protocol::messages::SyncGroupResponse::decode(rest, version)
                .expect("SyncGroup reply decodes")
                .error_code
        }
        ApiKey::Heartbeat => {
            kafka_protocol::messages::HeartbeatResponse::decode(rest, version)
                .expect("Heartbeat reply decodes")
                .error_code
        }
        other => unreachable!("group_protocol_error_code called for {other:?}"),
    }
}

/// Decodes a reply this matrix does not otherwise inspect, purely to prove
/// the bytes are well-formed -- `decode_reply`'s own three identical arms
/// (`Metadata`, `Produce`, `Fetch`), factored out for the fifty-line limit.
fn decode_ignoring_body<T: Decodable>(rest: &mut &[u8], api_key: ApiKey, version: i16) -> i16 {
    T::decode(rest, version).unwrap_or_else(|e| panic!("{api_key:?} reply decodes: {e}"));
    0
}

/// The error code a well-formed **minimal** request must answer with, per
/// API. `0` for every API that genuinely succeeds against it; a named
/// non-zero code for the one that does not by design.
///
/// ⚠️ **`SaslAuthenticate`, not `0`.** `M9.3`'s own handler refuses every
/// exchange — no credential source is configured yet (`M9.4`'s own scope)
/// — so the minimal, synthetic credential this matrix constructs is
/// *correctly* refused, not merely "not yet implemented." `FR-2`'s bar is
/// "genuinely served": reachable, decodable, and answering per its actual
/// documented behaviour — never `UNSUPPORTED_VERSION` (35), which this
/// still is not. `error_code == 0` for every other row is what "genuinely
/// succeeds" happens to mean for them; it does not mean the same thing
/// for an authentication check whose whole point is refusing what it does
/// not recognise.
const fn expected_error_code(api_key: ApiKey) -> i16 {
    match api_key {
        ApiKey::SaslAuthenticate => oqueue_codec::error_codes::SASL_AUTHENTICATION_FAILED,
        // Against a group nothing ever joined -- `heartbeat_body`'s own
        // doc, the same "genuinely served, documented non-zero answer"
        // shape `SaslAuthenticate`'s own row already is.
        ApiKey::Heartbeat => oqueue_codec::error_codes::REBALANCE_IN_PROGRESS,
        _ => 0,
    }
}

#[tokio::test]
async fn every_advertised_version_is_served() {
    let broker = broker(&["t"]).await;
    let cluster = Arc::clone(&broker.cluster);
    let dispatcher = Dispatcher::new(Arc::clone(&cluster));

    for advertised in ADVERTISED {
        for version in advertised.min..=advertised.max {
            let body = minimal_body(advertised.api_key, version, &cluster);
            let frame = framed(advertised.api_key, version, &body);
            let reply = match dispatcher.handle(frame).await {
                HandlerResponse::Reply(reply) => reply,
                other => panic!(
                    "{:?} v{version}: not answered: {other:?}",
                    advertised.api_key
                ),
            };
            let error_code = decode_reply(advertised.api_key, version, &reply);
            assert_eq!(
                error_code,
                expected_error_code(advertised.api_key),
                "{:?} v{version}: an advertised version is never error 35, and answers what it documents",
                advertised.api_key
            );
        }
    }
}

/// The dependency's own encoder building the batch produce needs — the
/// same authority as the corpus and the unit fixtures (`ADR-0017`).
fn golden_batch() -> Vec<u8> {
    use kafka_protocol::records::{
        Compression, Record, RecordBatchEncoder, RecordEncodeOptions, TimestampType,
    };
    let record = Record {
        transactional: false,
        control: false,
        partition_leader_epoch: 0,
        producer_id: -1,
        producer_epoch: -1,
        timestamp_type: TimestampType::Creation,
        offset: 0,
        sequence: 0,
        delete_horizon: false,
        timestamp: 1_700_000_000_000,
        key: None,
        value: Some(bytes::Bytes::from_static(b"matrix")),
        headers: kafka_protocol::indexmap::IndexMap::default(),
    };
    let mut buf = bytes::BytesMut::new();
    RecordBatchEncoder::encode(
        &mut buf,
        &[record],
        &RecordEncodeOptions {
            version: 2,
            compression: Compression::None,
        },
    )
    .expect("the dependency encodes its own records");
    buf.to_vec()
}
