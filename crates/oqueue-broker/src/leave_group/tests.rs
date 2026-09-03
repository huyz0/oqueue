#![allow(clippy::expect_used)]

use super::handle;
use crate::connection::HandlerResponse;
use crate::testing::fixture;
use kafka_protocol::messages::LeaveGroupRequest as KpRequest;
use kafka_protocol::messages::LeaveGroupResponse as KpResponse;
use kafka_protocol::messages::leave_group_request::MemberIdentity as KpMember;
use kafka_protocol::protocol::{Decodable, Encodable, StrBytes};
use oqueue_codec::frame::RequestPrelude;
use oqueue_core::GroupState;

const VERSION: i16 = 5;

fn prelude() -> RequestPrelude {
    RequestPrelude {
        api_key: 13,
        api_version: VERSION,
        correlation_id: 1,
    }
}

fn group(name: &str) -> oqueue_core::GroupId {
    oqueue_core::GroupId::new(name).expect("valid")
}

/// Drives `g` to `Stable`, generation 1 — directly against the live
/// coordinator, `M4.15c`'s own module doc: nothing this test needs
/// depends on the seed itself being durable.
fn seat_stable(cluster: &crate::cluster::Cluster, g: &oqueue_core::GroupId) {
    cluster
        .group_coordinator()
        .transition(g, oqueue_core::GroupEvent::Join)
        .expect("Empty -> Join is legal");
    cluster
        .group_coordinator()
        .transition(g, oqueue_core::GroupEvent::JoinBarrierComplete)
        .expect("PreparingRebalance -> CompletingRebalance is legal");
    cluster
        .group_coordinator()
        .transition(g, oqueue_core::GroupEvent::SyncComplete)
        .expect("CompletingRebalance -> Stable is legal");
}

fn batched_body(group: &str, member_ids: &[&str]) -> Vec<u8> {
    let members = member_ids
        .iter()
        .map(|&id| {
            let mut m = KpMember::default();
            m.member_id = StrBytes::from_string(id.to_owned());
            m
        })
        .collect();
    let request = KpRequest::default()
        .with_group_id(kafka_protocol::messages::GroupId(StrBytes::from_string(
            group.to_owned(),
        )))
        .with_members(members);
    let mut out = Vec::new();
    request.encode(&mut out, VERSION).expect("encodes");
    out
}

async fn leave(cluster: &crate::cluster::Cluster, body: &[u8]) -> KpResponse {
    let HandlerResponse::Reply(out) = handle(cluster, prelude(), body).await else {
        panic!("a LeaveGroup replies");
    };
    let mut rest = &out[5..]; // v5 is flexible: a 5-byte response header.
    let response = KpResponse::decode(&mut rest, VERSION).expect("decodes");
    assert!(rest.is_empty());
    response
}

/// ⚠️ **`M4.10`'s own acceptance criterion, verbatim**: a batched
/// `LeaveGroup` naming three members produces one rebalance, not three —
/// `FakeGroupCoordinator::transition_calls` is what makes "exactly one"
/// observable at all (the final *state* looks identical whether the
/// implementation called `transition` once correctly or three times with
/// two silently refused).
#[tokio::test(start_paused = true)]
async fn three_batched_members_leaving_produces_exactly_one_rebalance() {
    let fixture = fixture(&[]).await;
    let g = group("orders");
    // ⚠️ **A fourth member stays behind.** Removing every tracked member
    // would empty the group and fire `AllMembersGone` instead
    // (`Heartbeats::remove_where`'s own "emptied" branch,
    // `heartbeat.rs`'s module doc) — a *different* transition than the one
    // this test means to observe. Leaving one member behind keeps this a
    // partial removal, so the transition under test is `GroupEvent::Join`
    // (`Stable` -> `PreparingRebalance`), matching a real rebalance caused
    // by some, not all, members leaving.
    for member_id in ["m1", "m2", "m3", "m4"] {
        fixture.cluster.heartbeats().register(&g, member_id, 30_000);
    }
    seat_stable(&fixture.cluster, &g);
    let calls_before = fixture.group_coordinator.transition_calls();

    let response = leave(
        &fixture.cluster,
        &batched_body("orders", &["m1", "m2", "m3"]),
    )
    .await;

    assert_eq!(response.error_code, 0);
    assert_eq!(response.members.len(), 3);
    for member in &response.members {
        assert_eq!(member.error_code, 0);
    }
    assert_eq!(
        fixture.group_coordinator.transition_calls(),
        calls_before + 1,
        "three members named in one request must trigger exactly one rebalance"
    );
    for member_id in ["m1", "m2", "m3"] {
        assert!(
            !fixture.cluster.heartbeats().is_tracked(&g, member_id),
            "{member_id} must no longer be tracked"
        );
    }
    assert_eq!(
        fixture
            .cluster
            .group_coordinator()
            .record(&g)
            .map(|r| r.state),
        Some(GroupState::PreparingRebalance),
        "the group left Stable exactly once"
    );
}

/// The pre-batching wire shape (v0-2): a single `member_id` field,
/// normalized the same way.
#[tokio::test(start_paused = true)]
async fn the_singular_v0_2_form_also_leaves() {
    const V1: i16 = 1;
    let fixture = fixture(&[]).await;
    let g = group("orders");
    fixture.cluster.heartbeats().register(&g, "m1", 30_000);

    let request = KpRequest::default()
        .with_group_id(kafka_protocol::messages::GroupId(
            StrBytes::from_static_str("orders"),
        ))
        .with_member_id(StrBytes::from_static_str("m1"));
    let mut body = Vec::new();
    request.encode(&mut body, V1).expect("encodes");

    let prelude = RequestPrelude {
        api_key: 13,
        api_version: V1,
        correlation_id: 1,
    };
    let HandlerResponse::Reply(out) = handle(&fixture.cluster, prelude, &body).await else {
        panic!("a LeaveGroup replies");
    };
    let mut rest = &out[4..]; // v1 is not flexible: a 4-byte header.
    let response = KpResponse::decode(&mut rest, V1).expect("decodes");
    assert!(rest.is_empty());
    assert_eq!(response.error_code, 0);
    assert!(!fixture.cluster.heartbeats().is_tracked(&g, "m1"));
}

/// ⚠️ **`M4.11`'s own fencing seam**: a member this broker never tracked
/// is told `UNKNOWN_MEMBER_ID`, per member, rather than `NONE` — the
/// request as a whole still succeeds (top-level `error_code` stays `NONE`;
/// `leave_group.rs`'s own module doc), so a client can tell "you were
/// never here" from a genuine departure, without the request itself being
/// refused.
#[tokio::test(start_paused = true)]
async fn leaving_an_untracked_member_is_told_unknown_member_id() {
    let fixture = fixture(&[]).await;
    let response = leave(&fixture.cluster, &batched_body("orders", &["ghost"])).await;
    assert_eq!(response.error_code, 0, "the request itself still succeeds");
    assert_eq!(response.members.len(), 1);
    assert_eq!(
        response.members[0].error_code,
        oqueue_codec::error_codes::UNKNOWN_MEMBER_ID
    );
}

/// A malformed body closes the connection rather than answering — every
/// other handler's own policy for a frame this broker cannot decode.
#[tokio::test(start_paused = true)]
async fn a_malformed_body_closes_rather_than_panicking() {
    let fixture = fixture(&[]).await;
    let response = handle(&fixture.cluster, prelude(), &[0xFF; 3]).await;
    assert!(matches!(response, HandlerResponse::Close));
}
