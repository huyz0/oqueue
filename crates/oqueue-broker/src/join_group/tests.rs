#![allow(clippy::expect_used)]

mod cooperative;

use super::handle;
use crate::connection::HandlerResponse;
use crate::testing::fixture;
use kafka_protocol::messages::JoinGroupRequest as KpRequest;
use kafka_protocol::messages::JoinGroupResponse as KpResponse;
use kafka_protocol::messages::join_group_request::JoinGroupRequestProtocol as KpProtocol;
use kafka_protocol::protocol::{Decodable, Encodable, StrBytes};
use oqueue_codec::frame::RequestPrelude;

pub(super) const VERSION: i16 = 5;

fn prelude(version: i16) -> RequestPrelude {
    RequestPrelude {
        api_key: 11,
        api_version: version,
        correlation_id: 7,
    }
}

/// One member's own `JoinGroup` request body, as the dependency's own
/// encoder writes it — `ADR-0017`'s golden-frame precedent, the same shape
/// `oqueue_codec::join_group`'s own tests already use.
fn request_body(group: &str, protocol: &str, rebalance_timeout_ms: i32, version: i16) -> Vec<u8> {
    let mut p = KpProtocol::default();
    p.name = StrBytes::from_string(protocol.to_owned());
    p.metadata = bytes::Bytes::from(format!("meta-{protocol}").into_bytes());
    let request = KpRequest::default()
        .with_group_id(kafka_protocol::messages::GroupId(StrBytes::from_string(
            group.to_owned(),
        )))
        .with_session_timeout_ms(30_000)
        .with_rebalance_timeout_ms(rebalance_timeout_ms)
        .with_member_id(StrBytes::from_static_str(""))
        .with_protocol_type(StrBytes::from_static_str("consumer"))
        .with_protocols(vec![p]);
    let mut out = Vec::new();
    request.encode(&mut out, version).expect("encodes");
    out
}

fn decode_response(bytes: &[u8], version: i16) -> KpResponse {
    // JoinGroup goes flexible (tagged response header) at v6.
    let header_len = if version >= 6 { 5 } else { 4 };
    let mut rest = &bytes[header_len..];
    let response = KpResponse::decode(&mut rest, version).expect("decodes");
    assert!(rest.is_empty());
    response
}

pub(super) async fn join(
    cluster: &crate::cluster::Cluster,
    body: Vec<u8>,
    version: i16,
) -> KpResponse {
    let HandlerResponse::Reply(out) = handle(cluster, prelude(version), &body).await else {
        panic!("a JoinGroup replies");
    };
    decode_response(&out, version)
}

/// ⚠️ **`M4.7`'s own acceptance criterion, verbatim**: N members' own
/// `JoinGroup` frames, driven through a real handler over a real cluster —
/// the leader alone receives every member's own metadata, and every
/// follower receives none. A fresh group's own first round has no roster to
/// wait for and so no early-close signal (`round`'s own module doc): it
/// closes on its deadline under paused time rather than the moment the last
/// member joins — the deadline path, not the early-close one (`round::tests`
/// already covers that one directly). ⚠️ Since `M4.29` that deadline is
/// `INITIAL_REBALANCE_DELAY` rather than the requested
/// `rebalance_timeout_ms`, so the round closes well before the sleep below
/// elapses; the sleep is left at the requested timeout because this case is
/// about *who gets whose metadata*, not about when the round closes.
#[tokio::test(start_paused = true)]
async fn n_members_join_together_the_leader_alone_gets_every_metadata() {
    const REBALANCE_TIMEOUT_MS: i32 = 10_000;
    let fixture = std::sync::Arc::new(fixture(&[]).await);
    let names = ["range", "range", "range"];

    let joiners: Vec<_> = names
        .iter()
        .map(|&protocol| {
            let cluster = std::sync::Arc::clone(&fixture.cluster);
            let body = request_body("orders-consumers", protocol, REBALANCE_TIMEOUT_MS, VERSION);
            tokio::spawn(async move { join(&cluster, body, VERSION).await })
        })
        .collect();

    // Let every member reach its own park before the clock moves.
    for _ in 0..names.len() {
        tokio::task::yield_now().await;
    }
    tokio::time::sleep(std::time::Duration::from_secs(10)).await;

    let mut responses = Vec::new();
    for joiner in joiners {
        responses.push(joiner.await.expect("the joiner task joins"));
    }

    assert_eq!(responses.len(), 3);
    assert_every_response_agrees(&responses);

    let leaders: Vec<_> = responses.iter().filter(|r| !r.members.is_empty()).collect();
    assert_eq!(
        leaders.len(),
        1,
        "exactly one response carries the full roster"
    );
    assert_eq!(leaders[0].members.len(), 3, "the leader sees every member");
    // ⚠️ Not just the right member count -- the right bytes, per member: a
    // decode/lookup bug that handed back the *wrong* metadata (or empty
    // bytes) for a real member would still pass a members.len() check.
    for m in &leaders[0].members {
        assert_eq!(
            m.metadata.as_ref(),
            b"meta-range",
            "the leader's own view carries this member's real metadata, not empty or another's"
        );
    }

    let followers: Vec<_> = responses.iter().filter(|r| r.members.is_empty()).collect();
    assert_eq!(followers.len(), 2, "every non-leader response carries none");

    // The leader's own member_id echoes in its own response.
    assert_eq!(leaders[0].member_id, leaders[0].leader);
    // Every follower's own member_id is distinct (minted, never echoed
    // client input -- every request here sent an empty member_id).
    let mut ids: Vec<_> = responses.iter().map(|r| r.member_id.to_string()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), 3, "every member got its own distinct minted id");
}

/// Every response to the same round agrees on the outcome that closed it —
/// pulled out of `n_members_join_together_the_leader_alone_gets_every_metadata`
/// purely to keep that test under the fifty-line limit.
fn assert_every_response_agrees(responses: &[KpResponse]) {
    for r in responses {
        assert_eq!(r.error_code, 0, "no member is refused");
        assert_eq!(
            r.generation_id, responses[0].generation_id,
            "one round, one generation"
        );
        assert_eq!(
            r.protocol_name.as_ref().map(StrBytes::as_str),
            Some("range")
        );
    }
}

/// A member sharing no protocol with the round's own leader is refused
/// `INCONSISTENT_GROUP_PROTOCOL` and never enrolled — this task's own
/// mapping of `M4.6`'s `elect` returning `None` for the pair onto the wire.
#[tokio::test(start_paused = true)]
async fn a_member_with_no_common_protocol_is_refused_inconsistent_group_protocol() {
    let fixture = fixture(&[]).await;
    let leader = tokio::spawn({
        let cluster = std::sync::Arc::clone(&fixture.cluster);
        let body = request_body("orders-consumers", "range", 5_000, VERSION);
        async move { join(&cluster, body, VERSION).await }
    });
    tokio::task::yield_now().await;

    let refused = join(
        &fixture.cluster,
        request_body("orders-consumers", "sticky", 5_000, VERSION),
        VERSION,
    )
    .await;
    assert_eq!(
        refused.error_code,
        oqueue_codec::error_codes::INCONSISTENT_GROUP_PROTOCOL
    );
    assert_eq!(refused.member_id.as_str(), "");
    assert_eq!(refused.generation_id, -1);

    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    let leader_response = leader.await.expect("the leader task joins");
    assert_eq!(leader_response.error_code, 0);
    assert_eq!(
        leader_response.members.len(),
        1,
        "only the compatible member"
    );
}

/// ⚠️ **`M4.11`'s own fencing check**: a rejoin naming a non-empty
/// `member_id` this broker never enrolled is refused `UNKNOWN_MEMBER_ID`
/// outright, never silently admitted as a fresh member under a made-up
/// identity.
#[tokio::test(start_paused = true)]
async fn a_rejoin_naming_an_unrecognized_member_id_is_refused_unknown_member_id() {
    let fixture = fixture(&[]).await;
    let mut protocol = KpProtocol::default();
    protocol.name = StrBytes::from_static_str("range");
    protocol.metadata = bytes::Bytes::from_static(b"meta");
    let request = KpRequest::default()
        .with_group_id(kafka_protocol::messages::GroupId(
            StrBytes::from_static_str("orders-consumers"),
        ))
        .with_session_timeout_ms(30_000)
        .with_rebalance_timeout_ms(5_000)
        .with_member_id(StrBytes::from_static_str("made-up-id"))
        .with_protocol_type(StrBytes::from_static_str("consumer"))
        .with_protocols(vec![protocol]);
    let mut body = Vec::new();
    request.encode(&mut body, VERSION).expect("encodes");

    let response = join(&fixture.cluster, body, VERSION).await;
    assert_eq!(
        response.error_code,
        oqueue_codec::error_codes::UNKNOWN_MEMBER_ID
    );
    assert_eq!(response.member_id.as_str(), "");
    assert_eq!(response.generation_id, -1);
}

/// A malformed body closes the connection rather than answering — every
/// other handler's own policy for a frame this broker cannot decode.
#[tokio::test(start_paused = true)]
async fn a_malformed_body_closes_rather_than_panicking() {
    let fixture = fixture(&[]).await;
    let response = handle(&fixture.cluster, prelude(VERSION), &[0xFF; 3]).await;
    assert!(matches!(response, HandlerResponse::Close));
}

/// `protocol_type` on the response is absent below v7 and present from it
/// (`oqueue_codec::join_group`'s own doc) — this test is the only one in
/// this file at v7+, and exists specifically so that cutover is exercised
/// on the wire at all, not only in `response_for`'s own in-memory value.
#[tokio::test(start_paused = true)]
async fn protocol_type_is_carried_on_the_wire_from_v7() {
    const V7: i16 = 7;
    let fixture = fixture(&[]).await;
    let body = request_body("orders-consumers", "range", 1, V7);
    let response = join(&fixture.cluster, body, V7).await;
    assert_eq!(
        response.protocol_type.as_ref().map(StrBytes::as_str),
        Some("consumer")
    );
}
