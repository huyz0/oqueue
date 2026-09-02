#![allow(clippy::expect_used)]

use super::handle;
use crate::connection::HandlerResponse;
use crate::testing::at;
use kafka_protocol::messages::{FindCoordinatorRequest, FindCoordinatorResponse};
use kafka_protocol::protocol::{Decodable, Encodable, StrBytes};
use oqueue_codec::frame::RequestPrelude;

const VERSION: i16 = 1;

fn prelude() -> RequestPrelude {
    RequestPrelude {
        api_key: 10,
        api_version: VERSION,
        correlation_id: 13,
    }
}

fn request_bytes(key: &str) -> Vec<u8> {
    let request = FindCoordinatorRequest::default().with_key(StrBytes::from_string(key.to_owned()));
    let mut out = Vec::new();
    request.encode(&mut out, VERSION).expect("encodes");
    out
}

fn answered(cluster: &crate::Cluster, body: &[u8]) -> FindCoordinatorResponse {
    let HandlerResponse::Reply(out) = handle(cluster, prelude(), body) else {
        panic!("a well-formed FindCoordinator replies");
    };
    let mut rest = &out[4..];
    let response = FindCoordinatorResponse::decode(&mut rest, VERSION).expect("decodes");
    assert!(rest.is_empty());
    response
}

/// The response names this node's own advertised identity, not a
/// hardcoded value -- two differently-configured clusters answer with
/// their own host/port, not each other's.
#[tokio::test]
async fn the_response_names_this_node_s_own_advertised_identity() {
    let a = at("first-host", 1111, &[]).await;
    let b = at("second-host", 2222, &[]).await;

    let from_a = answered(&a.cluster, &request_bytes("orders"));
    assert_eq!(from_a.node_id.0, a.cluster.node_id);
    assert_eq!(from_a.host.as_str(), "first-host");
    assert_eq!(from_a.port, 1111);

    let from_b = answered(&b.cluster, &request_bytes("orders"));
    assert_eq!(from_b.host.as_str(), "second-host");
    assert_eq!(from_b.port, 2222);
}

/// The acceptance criterion this task's own backlog row names: the response
/// never varies by group name -- `ADR-0033`'s single-node answer, not a
/// table lookup that happens to always return the same row.
#[tokio::test]
async fn the_response_is_identical_regardless_of_the_group_name() {
    let f = at("h", 9092, &[]).await;

    let one = answered(&f.cluster, &request_bytes("alice-group"));
    let other = answered(&f.cluster, &request_bytes("zzz-completely-different-group"));

    assert_eq!(one.node_id, other.node_id);
    assert_eq!(one.host, other.host);
    assert_eq!(one.port, other.port);
    assert_eq!(one.error_code, other.error_code);
}

/// A transaction-type key is refused, not answered as if this broker had a
/// real transaction coordinator -- `init_producer_id.rs`'s own precedent
/// for a transactional call, found by round 1 review: answering this the
/// same way a group lookup is answered would tell a client this node
/// coordinates its transaction, only for `InitProducerId` to refuse the
/// very `transactional_id` that answer implied would work.
#[tokio::test]
async fn a_transaction_key_type_is_refused() {
    let f = at("h", 9092, &[]).await;
    let request = FindCoordinatorRequest::default()
        .with_key(StrBytes::from_static_str("txn-1"))
        .with_key_type(1);
    let mut body = Vec::new();
    request.encode(&mut body, VERSION).expect("encodes");

    let response = answered(&f.cluster, &body);
    assert_eq!(
        response.error_code,
        oqueue_codec::error_codes::INVALID_REQUEST
    );
    // The refusal carries no real coordinator identity -- the protocol's own
    // "no coordinator" sentinel, not this node's own node_id/port leaking
    // out alongside a refusal.
    assert_eq!(response.node_id.0, -1);
    assert_eq!(response.port, -1);
}

/// A group-type key (the default, and the only supported one) still
/// resolves to this node -- `ADR-0033`'s own decision, unaffected by the
/// transaction-type refusal beside it.
#[tokio::test]
async fn a_group_key_type_resolves_to_this_node() {
    let f = at("h", 9092, &[]).await;
    let request = FindCoordinatorRequest::default()
        .with_key(StrBytes::from_static_str("orders"))
        .with_key_type(0);
    let mut body = Vec::new();
    request.encode(&mut body, VERSION).expect("encodes");

    let response = answered(&f.cluster, &body);
    assert_eq!(response.error_code, 0);
    assert_eq!(response.node_id.0, f.cluster.node_id);
}

#[tokio::test]
async fn a_malformed_body_closes_the_connection() {
    let f = at("h", 9092, &[]).await;
    assert!(matches!(
        handle(&f.cluster, prelude(), &[0xFF]),
        HandlerResponse::Close
    ));
}
