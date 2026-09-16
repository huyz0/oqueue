#![allow(clippy::expect_used)]

mod deadline;
mod durability;
mod generations;
mod parked;
mod reopened;

use super::handle;
use crate::connection::HandlerResponse;
use crate::testing::fixture;
use kafka_protocol::messages::SyncGroupRequest as KpRequest;
use kafka_protocol::messages::SyncGroupResponse as KpResponse;
use kafka_protocol::messages::sync_group_request::SyncGroupRequestAssignment as KpAssignment;
use kafka_protocol::protocol::{Decodable, Encodable, StrBytes};
use oqueue_codec::frame::RequestPrelude;

const VERSION: i16 = 3;

fn prelude() -> RequestPrelude {
    RequestPrelude {
        api_key: 14,
        api_version: VERSION,
        correlation_id: 9,
    }
}

/// A follower's own request: no assignments.
fn follower_body(group: &str, member_id: &str) -> Vec<u8> {
    let request = KpRequest::default()
        .with_group_id(kafka_protocol::messages::GroupId(StrBytes::from_string(
            group.to_owned(),
        )))
        .with_generation_id(1)
        .with_member_id(StrBytes::from_string(member_id.to_owned()));
    let mut out = Vec::new();
    request.encode(&mut out, VERSION).expect("encodes");
    out
}

/// The leader's own request: one assignment per member.
fn leader_body(group: &str, leader_id: &str, assignments: &[(&str, &[u8])]) -> Vec<u8> {
    let entries = assignments
        .iter()
        .map(|&(id, bytes)| {
            let mut a = KpAssignment::default();
            a.member_id = StrBytes::from_string(id.to_owned());
            a.assignment = bytes::Bytes::from(bytes.to_vec());
            a
        })
        .collect();
    let request = KpRequest::default()
        .with_group_id(kafka_protocol::messages::GroupId(StrBytes::from_string(
            group.to_owned(),
        )))
        .with_generation_id(1)
        .with_member_id(StrBytes::from_string(leader_id.to_owned()))
        .with_assignments(entries);
    let mut out = Vec::new();
    request.encode(&mut out, VERSION).expect("encodes");
    out
}

/// `follower_body`/`leader_body` at a named generation — the tests that are
/// *about* generations, rather than the ones that only need a valid one.
fn follower_body_at(group: &str, member_id: &str, generation: i32) -> Vec<u8> {
    let request = KpRequest::default()
        .with_group_id(kafka_protocol::messages::GroupId(StrBytes::from_string(
            group.to_owned(),
        )))
        .with_generation_id(generation)
        .with_member_id(StrBytes::from_string(member_id.to_owned()));
    let mut out = Vec::new();
    request.encode(&mut out, VERSION).expect("encodes");
    out
}

fn leader_body_at(
    group: &str,
    leader_id: &str,
    assignments: &[(&str, &[u8])],
    generation: i32,
) -> Vec<u8> {
    let entries = assignments
        .iter()
        .map(|&(id, bytes)| {
            let mut a = KpAssignment::default();
            a.member_id = StrBytes::from_string(id.to_owned());
            a.assignment = bytes::Bytes::from(bytes.to_vec());
            a
        })
        .collect();
    let request = KpRequest::default()
        .with_group_id(kafka_protocol::messages::GroupId(StrBytes::from_string(
            group.to_owned(),
        )))
        .with_generation_id(generation)
        .with_member_id(StrBytes::from_string(leader_id.to_owned()))
        .with_assignments(entries);
    let mut out = Vec::new();
    request.encode(&mut out, VERSION).expect("encodes");
    out
}

/// What [`super::barrier::SyncGroups::submit`]'s fourth argument is in a test
/// that drives the cell directly.
///
/// ⚠️ **`true`, and not vacuous.** The closure exists so a submission can be
/// refused when the *coordinator* has left the generation (`M4.65`); these
/// call sites reach the cell directly rather than through a handler, so no
/// coordinator was consulted to reach them and there is no record for the
/// closure to read. What they pin is the generation ordering beside it, the
/// `>` guard. ⚠️ Not "hold no coordinator", which an earlier wording of this
/// said and `parked.rs`'s fixture disproves: holding one is not consulting
/// one. `sync_group.rs`'s real call site is where the record is read, and
/// `reopened::a_leader_that_resumes_after_the_group_left_does_not_overwrite_the_refusal`
/// is what constrains it.
fn still_current() -> bool {
    true
}

/// One newcomer's own `JoinGroup` through the real handler, so the round is
/// opened by the path `apply_open` actually takes rather than by a
/// coordinator transition this file fired itself.
///
/// ⚠️ **Its own copy rather than `heartbeat/tests.rs`'s `join`**, which is
/// `pub(super)` to that module. Three lines of encoding against a helper
/// visible from here is the cheaper of the two, and `code-structure.md`
/// rule 8 keeps a test helper beside the tests that use it.
///
/// ⚠️ **Here rather than in `reopened.rs`, where `M4.58` first wrote it.**
/// `M4.66` gave it a second caller in `deadline.rs`, and a helper a
/// sibling file reaches for through `super::reopened::` belongs beside
/// `seat` and `sync` — which is also what took `reopened.rs` back under
/// `code-structure.md` rule 16's 500 lines.
pub(super) async fn join_as_a_newcomer(cluster: &crate::cluster::Cluster, group: &str) {
    join_asking(cluster, group, "range", 30_000).await;
}

/// [`join_as_a_newcomer`] with the two fields `M4.66` is about spelled out:
/// which protocol the member advertises, and what `rebalance_timeout_ms` it
/// asks for.
///
/// ⚠️ **The protocol is a parameter because refusing a join needs one.**
/// `JoinOutcome::Refused` is "shares no protocol with the round's own running
/// candidate set", so a second member naming a different one is the only way
/// to reach that arm without reaching into the coordinator.
pub(super) async fn join_asking(
    cluster: &crate::cluster::Cluster,
    group: &str,
    protocol_name: &'static str,
    rebalance_timeout_ms: i32,
) {
    join_asking_as(cluster, group, "", protocol_name, rebalance_timeout_ms).await;
}

/// [`join_asking`] for a member that already has an id — a *rejoin*, which is
/// the only way to reach `join`'s `Step::Close` arm with a single member
/// (`M4.74`): the next round's `awaiting` is seeded from the last round's
/// roster, so the one member rejoining empties it.
///
/// Returns the decoded response, because the minted `member_id` is what the
/// rejoin has to carry.
pub(super) async fn join_asking_as(
    cluster: &crate::cluster::Cluster,
    group: &str,
    member_id: &str,
    protocol_name: &'static str,
    rebalance_timeout_ms: i32,
) -> kafka_protocol::messages::JoinGroupResponse {
    use kafka_protocol::messages::JoinGroupRequest as KpJoinRequest;
    use kafka_protocol::messages::join_group_request::JoinGroupRequestProtocol as KpProtocol;
    use kafka_protocol::protocol::{Encodable, StrBytes};

    const JOIN_VERSION: i16 = 5;
    let mut protocol = KpProtocol::default();
    protocol.name = StrBytes::from_static_str(protocol_name);
    protocol.metadata = bytes::Bytes::from_static(b"m");
    let request = KpJoinRequest::default()
        .with_group_id(kafka_protocol::messages::GroupId(StrBytes::from_string(
            group.to_owned(),
        )))
        .with_session_timeout_ms(30_000)
        .with_rebalance_timeout_ms(rebalance_timeout_ms)
        .with_member_id(StrBytes::from_string(member_id.to_owned()))
        .with_protocol_type(StrBytes::from_static_str("consumer"))
        .with_protocols(vec![protocol]);
    let mut body = Vec::new();
    request.encode(&mut body, JOIN_VERSION).expect("encodes");
    let reply = crate::join_group::handle(
        cluster,
        RequestPrelude {
            api_key: 11,
            api_version: JOIN_VERSION,
            correlation_id: 1,
        },
        &body,
    )
    .await;
    let HandlerResponse::Reply(out) = reply else {
        panic!("a JoinGroup replies");
    };
    let mut rest = &out[4..];
    kafka_protocol::messages::JoinGroupResponse::decode(&mut rest, JOIN_VERSION).expect("decodes")
}

fn decode_response(bytes: &[u8]) -> KpResponse {
    // SyncGroup goes flexible (tagged response header) at v4; VERSION is 3.
    let mut rest = &bytes[4..];
    let response = KpResponse::decode(&mut rest, VERSION).expect("decodes");
    assert!(rest.is_empty());
    response
}

async fn sync(cluster: &crate::cluster::Cluster, body: Vec<u8>) -> KpResponse {
    let HandlerResponse::Reply(out) = handle(cluster, prelude(), &body).await else {
        panic!("a SyncGroup replies");
    };
    decode_response(&out)
}

/// Registers each of `member_ids` with `heartbeat.rs`'s own tracking and
/// drives the coordinator to `CompletingRebalance` at generation `1` --
/// `M4.11`'s own fencing seam now refuses every test below unless the
/// members it names are genuinely tracked at the generation each request
/// hardcodes, which is what every `leader_body`/`follower_body` call here
/// already assumes. A direct `register`/`transition` pair rather than a
/// real `join_group::handle` round: this file's own tests care about
/// `SyncGroup`'s behaviour, not re-deriving `JoinGroup`'s
/// (`heartbeat/tests.rs`'s own `join` helper is where the full path
/// matters).
fn seat(cluster: &crate::cluster::Cluster, group: &str, member_ids: &[&str]) {
    let g = oqueue_core::GroupId::new(group).expect("valid");
    for &member_id in member_ids {
        cluster.heartbeats().register(&g, member_id, 30_000);
    }
    cluster
        .group_coordinator()
        .transition(&g, oqueue_core::GroupEvent::Join)
        .expect("Empty -> Join is legal");
    cluster
        .group_coordinator()
        .transition(&g, oqueue_core::GroupEvent::JoinBarrierComplete)
        .expect("PreparingRebalance -> CompletingRebalance is legal");
}

/// ⚠️ **`M4.8`'s own acceptance criterion, half one**: each follower's own
/// response carries exactly its own slice of the leader's submitted
/// assignment, never another member's.
#[tokio::test(start_paused = true)]
async fn a_followers_own_slice_is_exactly_its_own_never_anothers() {
    let fixture = fixture(&[]).await;
    seat(&fixture.cluster, "orders-consumers", &["m1", "m2"]);
    let leader = sync(
        &fixture.cluster,
        leader_body(
            "orders-consumers",
            "m1",
            &[("m1", b"assignment-for-m1"), ("m2", b"assignment-for-m2")],
        ),
    )
    .await;
    assert_eq!(leader.error_code, 0);
    assert_eq!(leader.assignment.as_ref(), b"assignment-for-m1");

    let follower = sync(&fixture.cluster, follower_body("orders-consumers", "m2")).await;
    assert_eq!(follower.error_code, 0);
    assert_eq!(follower.assignment.as_ref(), b"assignment-for-m2");
}

/// ⚠️ **A leader resubmitting for the same generation replaces what the
/// group's cell holds; it does not silently no-op.** Both submissions below
/// are at generation 1 — `leader_body`/`follower_body` hardcode it — so this
/// is a *resubmission*, which is why the name says so.
///
/// ⚠️ **It was called `a_second_rebalance_...` and it never was one**, which
/// `M4.17` caught: it had been written against the old `OnceLock` cell, where
/// `set` on an already-set cell no-ops and only a whole-entry rotation could
/// replace a map, so same-generation and next-generation looked like one
/// case. They are not, and the cell now carries its generation —
/// `a_follower_is_never_answered_with_a_previous_generations_assignment` is
/// the one that crosses a generation boundary. A test whose name describes a
/// scenario it does not run is worse than no test, because it is counted.
#[tokio::test(start_paused = true)]
async fn a_resubmitted_assignment_for_the_same_generation_replaces_the_first() {
    let fixture = fixture(&[]).await;
    seat(&fixture.cluster, "orders-consumers", &["m1", "m2"]);
    sync(
        &fixture.cluster,
        leader_body(
            "orders-consumers",
            "m1",
            &[("m1", b"first-m1"), ("m2", b"first-m2")],
        ),
    )
    .await;
    sync(&fixture.cluster, follower_body("orders-consumers", "m2")).await;

    // The leader submits again, at the same generation.
    let leader = sync(
        &fixture.cluster,
        leader_body(
            "orders-consumers",
            "m1",
            &[("m1", b"second-m1"), ("m2", b"second-m2")],
        ),
    )
    .await;
    assert_eq!(leader.assignment.as_ref(), b"second-m1");

    let follower = sync(&fixture.cluster, follower_body("orders-consumers", "m2")).await;
    assert_eq!(
        follower.assignment.as_ref(),
        b"second-m2",
        "the resubmitted assignment, not the first one it replaced"
    );
}

/// ⚠️ **Half two**: a follower's own `SyncGroup` arriving before the
/// leader's parks rather than answering early (with empty or wrong bytes).
#[tokio::test(start_paused = true)]
async fn a_follower_arriving_before_the_leader_parks_then_gets_its_own_slice() {
    let fixture = std::sync::Arc::new(fixture(&[]).await);
    seat(&fixture.cluster, "orders-consumers", &["m1", "m2"]);
    let follower = {
        let cluster = std::sync::Arc::clone(&fixture.cluster);
        let body = follower_body("orders-consumers", "m2");
        tokio::spawn(async move { sync(&cluster, body).await })
    };

    // Let the follower reach its own park before the leader arrives.
    tokio::task::yield_now().await;
    assert!(
        !follower.is_finished(),
        "the follower must be parked, not answered, before the leader's own submission"
    );

    let leader = sync(
        &fixture.cluster,
        leader_body(
            "orders-consumers",
            "m1",
            &[("m1", b"leader-slice"), ("m2", b"follower-slice")],
        ),
    )
    .await;
    assert_eq!(leader.assignment.as_ref(), b"leader-slice");

    let follower = follower.await.expect("the follower task joins");
    assert_eq!(follower.error_code, 0);
    assert_eq!(
        follower.assignment.as_ref(),
        b"follower-slice",
        "woken with its own real slice, not empty or the leader's"
    );
}

/// A follower's own `SyncGroup` arriving *after* the leader's is answered
/// at once from the already-known assignment map, never parked.
#[tokio::test(start_paused = true)]
async fn a_follower_arriving_after_the_leader_is_answered_at_once() {
    let fixture = fixture(&[]).await;
    seat(&fixture.cluster, "orders-consumers", &["m1", "m2"]);
    sync(
        &fixture.cluster,
        leader_body(
            "orders-consumers",
            "m1",
            &[("m1", b"leader-slice"), ("m2", b"follower-slice")],
        ),
    )
    .await;

    let started = tokio::time::Instant::now();
    let follower = sync(&fixture.cluster, follower_body("orders-consumers", "m2")).await;
    assert_eq!(
        tokio::time::Instant::now() - started,
        std::time::Duration::ZERO,
        "the assignment was already known -- no park"
    );
    assert_eq!(follower.assignment.as_ref(), b"follower-slice");
}

/// A malformed body closes the connection rather than answering — every
/// other handler's own policy for a frame this broker cannot decode.
#[tokio::test(start_paused = true)]
async fn a_malformed_body_closes_rather_than_panicking() {
    let fixture = fixture(&[]).await;
    let response = handle(&fixture.cluster, prelude(), &[0xFF; 3]).await;
    assert!(matches!(response, HandlerResponse::Close));
}

/// ⚠️ **`M4.11`'s own fencing check**: a member this broker never tracked
/// at all is refused `UNKNOWN_MEMBER_ID` before the submission is ever
/// treated as the assignment-bearing one — the group's own bookkeeping
/// (`SyncGroups`) is never touched by a refused call.
#[tokio::test(start_paused = true)]
async fn a_member_this_broker_never_tracked_is_refused_unknown_member_id() {
    let fixture = fixture(&[]).await;
    let leader = sync(
        &fixture.cluster,
        leader_body("orders-consumers", "ghost", &[("ghost", b"a")]),
    )
    .await;
    assert_eq!(
        leader.error_code,
        oqueue_codec::error_codes::UNKNOWN_MEMBER_ID
    );
}

/// A tracked member naming a generation that is not the group's current
/// one is refused `ILLEGAL_GENERATION`, distinct from an untracked one's
/// own `UNKNOWN_MEMBER_ID`.
#[tokio::test(start_paused = true)]
async fn a_stale_generation_submission_is_refused_illegal_generation() {
    let fixture = fixture(&[]).await;
    seat(&fixture.cluster, "orders-consumers", &["m1"]);
    let request = KpRequest::default()
        .with_group_id(kafka_protocol::messages::GroupId(
            StrBytes::from_static_str("orders-consumers"),
        ))
        .with_generation_id(999)
        .with_member_id(StrBytes::from_static_str("m1"))
        .with_assignments(vec![{
            let mut a = KpAssignment::default();
            a.member_id = StrBytes::from_static_str("m1");
            a.assignment = bytes::Bytes::from_static(b"a");
            a
        }]);
    let mut body = Vec::new();
    request.encode(&mut body, VERSION).expect("encodes");
    let response = sync(&fixture.cluster, body).await;
    assert_eq!(
        response.error_code,
        oqueue_codec::error_codes::ILLEGAL_GENERATION
    );
}

/// `protocol_type`/`protocol_name` are absent below v5 and echoed from v5
/// on (`oqueue_codec::sync_group`'s own doc) — this test is the only one in
/// this file at v5+, and exists specifically so that cutover is exercised
/// on the wire at all, not only in `response_for`'s own in-memory value.
#[tokio::test(start_paused = true)]
async fn protocol_type_and_name_are_echoed_on_the_wire_from_v5() {
    const V5: i16 = 5;
    let fixture = fixture(&[]).await;
    seat(&fixture.cluster, "orders-consumers", &["m1"]);
    let mut assignment = KpAssignment::default();
    assignment.member_id = StrBytes::from_static_str("m1");
    assignment.assignment = bytes::Bytes::from_static(b"a");
    let request = KpRequest::default()
        .with_group_id(kafka_protocol::messages::GroupId(
            StrBytes::from_static_str("orders-consumers"),
        ))
        .with_generation_id(1)
        .with_member_id(StrBytes::from_static_str("m1"))
        .with_protocol_type(Some(StrBytes::from_static_str("consumer")))
        .with_protocol_name(Some(StrBytes::from_static_str("range")))
        .with_assignments(vec![assignment]);
    let mut body = Vec::new();
    request.encode(&mut body, V5).expect("encodes");

    let prelude = RequestPrelude {
        api_key: 14,
        api_version: V5,
        correlation_id: 9,
    };
    let HandlerResponse::Reply(out) = handle(&fixture.cluster, prelude, &body).await else {
        panic!("a SyncGroup replies");
    };
    let mut rest = &out[5..]; // v5 is flexible: a 5-byte response header.
    let response = KpResponse::decode(&mut rest, V5).expect("decodes");
    assert!(rest.is_empty());
    assert_eq!(
        response.protocol_type.as_ref().map(StrBytes::as_str),
        Some("consumer")
    );
    assert_eq!(
        response.protocol_name.as_ref().map(StrBytes::as_str),
        Some("range")
    );
}
