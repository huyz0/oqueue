#![allow(clippy::expect_used)]

use super::handle;
use crate::connection::HandlerResponse;
use crate::testing::fixture;
use kafka_protocol::messages::HeartbeatRequest as KpHeartbeatRequest;
use kafka_protocol::messages::HeartbeatResponse as KpHeartbeatResponse;
use kafka_protocol::messages::JoinGroupRequest as KpJoinRequest;
use kafka_protocol::messages::JoinGroupResponse as KpJoinResponse;
use kafka_protocol::messages::SyncGroupRequest as KpSyncRequest;
use kafka_protocol::messages::join_group_request::JoinGroupRequestProtocol as KpProtocol;
use kafka_protocol::messages::sync_group_request::SyncGroupRequestAssignment as KpAssignment;
use kafka_protocol::protocol::{Decodable, Encodable, StrBytes};
use oqueue_codec::frame::RequestPrelude;
use oqueue_core::GroupState;

const JOIN_VERSION: i16 = 5;
const SYNC_VERSION: i16 = 3;
const HEARTBEAT_VERSION: i16 = 2;

fn prelude(api_key: i16, version: i16) -> RequestPrelude {
    RequestPrelude {
        api_key,
        api_version: version,
        correlation_id: 1,
    }
}

fn group(name: &str) -> oqueue_core::GroupId {
    oqueue_core::GroupId::new(name).expect("valid")
}

/// One member's own `JoinGroup` -> its own minted member id.
async fn join(
    cluster: &crate::cluster::Cluster,
    group: &str,
    session_timeout_ms: i32,
    rebalance_timeout_ms: i32,
) -> String {
    let mut protocol = KpProtocol::default();
    protocol.name = StrBytes::from_static_str("range");
    protocol.metadata = bytes::Bytes::from_static(b"m");
    let request = KpJoinRequest::default()
        .with_group_id(kafka_protocol::messages::GroupId(StrBytes::from_string(
            group.to_owned(),
        )))
        .with_session_timeout_ms(session_timeout_ms)
        .with_rebalance_timeout_ms(rebalance_timeout_ms)
        .with_member_id(StrBytes::from_static_str(""))
        .with_protocol_type(StrBytes::from_static_str("consumer"))
        .with_protocols(vec![protocol]);
    let mut body = Vec::new();
    request.encode(&mut body, JOIN_VERSION).expect("encodes");

    let HandlerResponse::Reply(out) =
        crate::join_group::handle(cluster, prelude(11, JOIN_VERSION), &body).await
    else {
        panic!("a JoinGroup replies");
    };
    let mut rest = &out[4..]; // JoinGroup is flexible from v6; JOIN_VERSION (5) is not.
    let response = KpJoinResponse::decode(&mut rest, JOIN_VERSION).expect("decodes");
    response.member_id.to_string()
}

/// A single-member group's own `SyncGroup` -- a trivial self-assignment,
/// enough to reach `Stable`.
async fn sync(cluster: &crate::cluster::Cluster, group: &str, member_id: &str, generation: i32) {
    let mut assignment = KpAssignment::default();
    assignment.member_id = StrBytes::from_string(member_id.to_owned());
    assignment.assignment = bytes::Bytes::from_static(b"a");
    let request = KpSyncRequest::default()
        .with_group_id(kafka_protocol::messages::GroupId(StrBytes::from_string(
            group.to_owned(),
        )))
        .with_generation_id(generation)
        .with_member_id(StrBytes::from_string(member_id.to_owned()))
        .with_assignments(vec![assignment]);
    let mut body = Vec::new();
    request.encode(&mut body, SYNC_VERSION).expect("encodes");

    let HandlerResponse::Reply(_) =
        crate::sync_group::handle(cluster, prelude(14, SYNC_VERSION), &body).await
    else {
        panic!("a SyncGroup replies");
    };
}

fn heartbeat_body(group: &str, member_id: &str, generation: i32) -> Vec<u8> {
    let request = KpHeartbeatRequest::default()
        .with_group_id(kafka_protocol::messages::GroupId(StrBytes::from_string(
            group.to_owned(),
        )))
        .with_generation_id(generation)
        .with_member_id(StrBytes::from_string(member_id.to_owned()));
    let mut body = Vec::new();
    request
        .encode(&mut body, HEARTBEAT_VERSION)
        .expect("encodes");
    body
}

async fn heartbeat(
    cluster: &crate::cluster::Cluster,
    group: &str,
    member_id: &str,
    generation: i32,
) -> i16 {
    let body = heartbeat_body(group, member_id, generation);
    let HandlerResponse::Reply(out) = handle(cluster, prelude(12, HEARTBEAT_VERSION), &body).await
    else {
        panic!("a Heartbeat replies");
    };
    let mut rest = &out[4..]; // v2 is not flexible: a 4-byte header.
    KpHeartbeatResponse::decode(&mut rest, HEARTBEAT_VERSION)
        .expect("decodes")
        .error_code
}

/// Joins two members to `group`, each with its own `session_timeout_ms`
/// (`[leader, follower]`), both under paused time (a fresh group's own
/// first round has no early-close signal, `join_group::round`'s own module
/// doc — advancing past `rebalance_timeout_ms` is what closes it), then
/// syncs both to `Stable` at generation 1. Returns `(leader, follower)`.
async fn a_stable_group_of_two(
    cluster: &std::sync::Arc<crate::cluster::Cluster>,
    group: &'static str,
    session_timeout_ms: [i32; 2],
) -> (String, String) {
    const REBALANCE_TIMEOUT_MS: i32 = 1_000;
    let joiners: Vec<_> = session_timeout_ms
        .into_iter()
        .map(|timeout| {
            let cluster = std::sync::Arc::clone(cluster);
            tokio::spawn(async move { join(&cluster, group, timeout, REBALANCE_TIMEOUT_MS).await })
        })
        .collect();
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    let mut ids = Vec::new();
    for joiner in joiners {
        ids.push(joiner.await.expect("the joiner task joins"));
    }
    let (leader, follower) = (ids[0].clone(), ids[1].clone());
    sync(cluster, group, &leader, 1).await;
    sync(cluster, group, &follower, 1).await;
    (leader, follower)
}

/// ⚠️ **`M4.9`'s own acceptance criterion, half one**: a member absent past
/// its own session timeout is evicted, and the group leaves `Stable` --
/// this task's own reading of "a generation bump," since `GenerationId`
/// itself only advances at the *next* completed join (`ADR-0033`, `M4.1`),
/// not at the moment eviction is noticed.
#[tokio::test(start_paused = true)]
async fn a_member_absent_past_its_own_timeout_is_evicted_and_the_group_leaves_stable() {
    let fixture = std::sync::Arc::new(fixture(&[]).await);
    let (leader, follower) =
        a_stable_group_of_two(&fixture.cluster, "orders", [60_000, 10_000]).await;
    assert_eq!(
        fixture
            .cluster
            .group_coordinator()
            .record(&group("orders"))
            .map(|r| r.state),
        Some(GroupState::Stable)
    );

    // The follower sends no further heartbeat; advance well past its own
    // 10s session timeout.
    tokio::time::sleep(std::time::Duration::from_secs(11)).await;

    // The leader's own heartbeat is what notices the follower's silence.
    let error_code = heartbeat(&fixture.cluster, "orders", &leader, 1).await;
    assert_eq!(
        error_code,
        oqueue_codec::error_codes::REBALANCE_IN_PROGRESS,
        "the group left Stable the instant this call swept the follower out"
    );

    let group = group("orders");
    assert!(
        !fixture.cluster.heartbeats().is_tracked(&group, &follower),
        "the follower must no longer be tracked -- evicted"
    );
    assert!(
        fixture.cluster.heartbeats().is_tracked(&group, &leader),
        "the leader, which just heartbeat, must still be tracked"
    );
    assert_eq!(
        fixture
            .cluster
            .group_coordinator()
            .record(&group)
            .map(|r| r.state),
        Some(GroupState::PreparingRebalance)
    );
}

/// ⚠️ **A survivor's own rejoin after an eviction must actually succeed —
/// not be refused forever.** `M4.9`'s own round-1 review finding: without
/// `join_group::round::open_round`'s own fix, every `JoinGroup` this
/// survivor sent after the eviction above would see the coordinator
/// already `PreparingRebalance` (from `sweep`'s own `GroupEvent::Join`)
/// with no locally-open round, and refuse it as an internal-invariant
/// bug — wedging the group permanently, since nothing else can ever fire
/// `JoinBarrierComplete` for a round `join_group::round` never opened.
/// Reproduces the reviewer's own repro: evict, then rejoin, and the
/// rejoin must not be `INCONSISTENT_GROUP_PROTOCOL`.
#[tokio::test(start_paused = true)]
async fn a_survivor_can_rejoin_after_an_eviction_the_group_is_not_permanently_wedged() {
    let fixture = std::sync::Arc::new(fixture(&[]).await);
    let (leader, _follower) =
        a_stable_group_of_two(&fixture.cluster, "orders", [60_000, 10_000]).await;
    tokio::time::sleep(std::time::Duration::from_secs(11)).await;
    assert_eq!(
        heartbeat(&fixture.cluster, "orders", &leader, 1).await,
        oqueue_codec::error_codes::REBALANCE_IN_PROGRESS,
        "the eviction itself, asserted already by the test above"
    );

    // The survivor does what a real client does on REBALANCE_IN_PROGRESS:
    // rejoins. `expected` for this round is still `Some(2)` (this
    // bookkeeping's own stale prior-round size, `open_round`'s own doc),
    // so a lone rejoiner waits out the round's own deadline rather than
    // closing early -- the point here is that it is *accepted* at all,
    // not refused outright.
    let mut protocol = KpProtocol::default();
    protocol.name = StrBytes::from_static_str("range");
    protocol.metadata = bytes::Bytes::from_static(b"m");
    let request = KpJoinRequest::default()
        .with_group_id(kafka_protocol::messages::GroupId(
            StrBytes::from_static_str("orders"),
        ))
        .with_session_timeout_ms(60_000)
        .with_rebalance_timeout_ms(1_000)
        .with_member_id(StrBytes::from_string(leader.clone()))
        .with_protocol_type(StrBytes::from_static_str("consumer"))
        .with_protocols(vec![protocol]);
    let mut body = Vec::new();
    request.encode(&mut body, JOIN_VERSION).expect("encodes");

    let cluster = std::sync::Arc::clone(&fixture.cluster);
    let rejoin = tokio::spawn(async move {
        let HandlerResponse::Reply(out) =
            crate::join_group::handle(&cluster, prelude(11, JOIN_VERSION), &body).await
        else {
            panic!("a JoinGroup replies");
        };
        out
    });
    tokio::task::yield_now().await;
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    let out = rejoin.await.expect("the rejoin task joins");
    let mut rest = &out[4..];
    let response = KpJoinResponse::decode(&mut rest, JOIN_VERSION).expect("decodes");
    assert_eq!(
        response.error_code, 0,
        "the survivor's own rejoin must be accepted, not refused as INCONSISTENT_GROUP_PROTOCOL forever"
    );
}

/// ⚠️ **Half two**: a stale-generation heartbeat from a member this broker
/// still tracks is refused `ILLEGAL_GENERATION`, not treated as a fresh
/// join -- `M4.11`'s own distinction from an *untracked* member's own
/// `UNKNOWN_MEMBER_ID` (`fencing/tests.rs`'s own table covers that case;
/// this one is specifically "tracked, but stale"). The group's own state
/// and generation are unchanged by the refused call either way.
#[tokio::test(start_paused = true)]
async fn a_stale_generation_heartbeat_is_refused_not_treated_as_a_fresh_join() {
    let fixture = std::sync::Arc::new(fixture(&[]).await);
    let (leader, _follower) =
        a_stable_group_of_two(&fixture.cluster, "orders", [30_000, 30_000]).await;

    let error_code = heartbeat(&fixture.cluster, "orders", &leader, 0).await;
    assert_eq!(error_code, oqueue_codec::error_codes::ILLEGAL_GENERATION);

    let group = group("orders");
    let record = fixture
        .cluster
        .group_coordinator()
        .record(&group)
        .expect("the group still exists");
    assert_eq!(
        record.state,
        GroupState::Stable,
        "a refused heartbeat must not itself move the group -- not treated as a fresh join"
    );
    assert_eq!(record.generation.get(), 1, "the generation is unchanged");
}

/// ⚠️ **`M4.11`'s other half of the distinction**: a member this broker
/// never tracked at all -- not stale, never enrolled -- is told
/// `UNKNOWN_MEMBER_ID`, not `ILLEGAL_GENERATION` or `REBALANCE_IN_PROGRESS`.
#[tokio::test(start_paused = true)]
async fn a_heartbeat_from_a_never_tracked_member_is_told_unknown_member_id() {
    let fixture = fixture(&[]).await;
    let error_code = heartbeat(&fixture.cluster, "orders", "ghost", 0).await;
    assert_eq!(error_code, oqueue_codec::error_codes::UNKNOWN_MEMBER_ID);
}

/// A successful heartbeat genuinely extends its own member's own deadline
/// -- not a no-op, and not backward. The follower heartbeats once at the
/// halfway point of its own original session timeout; if `renew` failed to
/// push its deadline forward (or pushed it backward), it would already be
/// evicted by the time this test checks, past the *original* deadline but
/// well short of the *renewed* one.
#[tokio::test(start_paused = true)]
async fn a_successful_heartbeat_extends_its_own_members_own_deadline() {
    let fixture = std::sync::Arc::new(fixture(&[]).await);
    let (leader, follower) =
        a_stable_group_of_two(&fixture.cluster, "orders", [60_000, 10_000]).await;

    // Halfway through the follower's own original 10s timeout, it
    // heartbeats -- renewing its own deadline to (now + 10s).
    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
    assert_eq!(
        heartbeat(&fixture.cluster, "orders", &follower, 1).await,
        oqueue_codec::error_codes::NONE
    );

    // 6 more seconds: past the follower's own *original* deadline
    // (registration + 10s), but 4s short of the deadline the heartbeat
    // above just renewed it to (its own moment + 10s).
    tokio::time::sleep(std::time::Duration::from_secs(6)).await;
    let error_code = heartbeat(&fixture.cluster, "orders", &leader, 1).await;
    assert_eq!(
        error_code,
        oqueue_codec::error_codes::NONE,
        "the renewal must have held -- the group is still Stable"
    );
    assert!(
        fixture
            .cluster
            .heartbeats()
            .is_tracked(&group("orders"), &follower),
        "the follower's own renewal must still be in effect"
    );
}

/// ⚠️ **The exact deadline instant is itself expired, not "not yet."**
/// `Heartbeats::sweep`'s own retain predicate must be a strict `>` (deadline
/// strictly after now survives); advancing time by *exactly* the follower's
/// own session timeout, no more, and no less, is what tells `>` and `>=`
/// apart -- under `>=` a member whose deadline exactly equals now would
/// wrongly survive one more sweep.
#[tokio::test(start_paused = true)]
async fn a_member_at_exactly_its_own_deadline_is_evicted() {
    let fixture = std::sync::Arc::new(fixture(&[]).await);
    let (leader, follower) =
        a_stable_group_of_two(&fixture.cluster, "orders", [60_000, 10_000]).await;

    // No intervening heartbeat, no intervening sleep beyond this one --
    // virtual time advances only on an explicit sleep under paused time, so
    // "now" at the sweep below is exactly the follower's own registered
    // deadline (registration time + its own 10s timeout).
    tokio::time::sleep(std::time::Duration::from_secs(10)).await;
    let error_code = heartbeat(&fixture.cluster, "orders", &leader, 1).await;
    assert_eq!(error_code, oqueue_codec::error_codes::REBALANCE_IN_PROGRESS);
    assert!(
        !fixture
            .cluster
            .heartbeats()
            .is_tracked(&group("orders"), &follower),
        "exactly-at-deadline must count as expired, not \"not yet\""
    );
}

/// A malformed body closes the connection rather than answering — every
/// other handler's own policy for a frame this broker cannot decode.
#[tokio::test(start_paused = true)]
async fn a_malformed_body_closes_rather_than_panicking() {
    let fixture = fixture(&[]).await;
    let response = handle(&fixture.cluster, prelude(12, HEARTBEAT_VERSION), &[0xFF; 3]).await;
    assert!(matches!(response, HandlerResponse::Close));
}
