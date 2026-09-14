//! What a follower is told when the barrier is reopened under it.
//!
//! ⚠️ **Split from `durability.rs` by the boundary that file's own doc
//! already drew.** It names two shapes and this is the second: where a
//! leader *does* submit, its map is correct and what is wrong is that
//! nothing recorded the group reaching `Stable`; here there is no map and
//! no submission at all, because the leader left, or was evicted, or a
//! newcomer reopened the round on top of the one it was still computing.
//! What is wrong is that nothing told the members already waiting.
//!
//! ⚠️ **Its own file rather than a longer one**: `M4.58` took
//! `durability.rs` past `code-structure.md` rule 16's 500 lines, and rule
//! 18 asks for a concept rather than a page count. The concept was already
//! written down one paragraph up.
//!
//! ⚠️ **The routes were found one at a time, and the tests here do not map
//! onto them one apiece.** `M4.43` gave the barrier a refusal and wired it
//! to `submit_assignment` — that route's own test stays in `durability.rs`,
//! because what fails there is the append. `M4.47` found every route a
//! member is *lost* by going through `Heartbeats::remove_where` with the
//! cell left holding nothing, and has two tests here: the transition
//! landing, and the transition itself failing, which its first version got
//! wrong. `M4.58` found the last route, where nothing is lost at all and a
//! newcomer simply arrives mid-sync. The remaining case is the floor under
//! all of them — what a follower is told when nothing reopens the barrier
//! at all and its own deadline is what answers.

#![allow(clippy::expect_used)]

use super::{seat, sync};
use crate::testing::fixture;
use oqueue_codec::error_codes;
use oqueue_core::GroupState;

/// ⚠️ **`M4.47`'s own acceptance criterion, and `M4.43`'s defect through the
/// route it did not cover.** That row gave the barrier a way to say a
/// generation was refused and wired it to the one place a refusal is
/// issued: `submit_assignment`. Every *other* way out of
/// `CompletingRebalance` — `LeaveGroup`, the session-timeout sweep, and a
/// group emptying — goes through `Heartbeats::remove_where`, which reopens
/// the barrier in the coordinator (`M4.34`) and left the cell holding
/// nothing.
///
/// ⚠️ **Measured before the fix**: the coordinator moves to
/// `PreparingRebalance`, the follower is not released, and it is answered
/// `-1` after `waited_ms=3000000` — fifty minutes to a code the Java
/// consumer raises out of `poll()` and never retries. Found by M4's final
/// boundary review, which is the fourth time this milestone has caught a
/// repair applied to the instance a review named rather than to the class.
#[tokio::test(start_paused = true)]
async fn a_parked_follower_is_woken_when_the_leader_leaves_instead_of_submitting() {
    let fixture = fixture(&[]).await;
    let g = oqueue_core::GroupId::new("orders-consumers").expect("valid");
    seat(&fixture.cluster, "orders-consumers", &["m1", "m2"]);

    let waiting = sync(
        &fixture.cluster,
        super::super::tests::follower_body_at("orders-consumers", "m2", 1),
    );
    tokio::pin!(waiting);
    assert!(
        crate::testing::poll_once(&mut waiting).is_none(),
        "the follower must park on the barrier rather than answer at once"
    );

    // The leader leaves rather than submitting. `M4.34` makes the
    // coordinator reopen the barrier; the question is whether anything
    // tells the member already waiting on it.
    fixture
        .cluster
        .heartbeats()
        .leave(&g, &["m1"], crate::heartbeat::Removal::of(&fixture.cluster))
        .await;

    let follower = waiting.await;
    assert_eq!(
        follower.error_code,
        error_codes::REBALANCE_IN_PROGRESS,
        "a follower whose leader left must be told to rejoin, not left for MAX_SYNC_WAIT_MS"
    );
    assert!(
        follower.assignment.is_empty(),
        "a refusal carries no assignment"
    );
}

/// ⚠️ **And when the transition itself fails.** `members.retain` drops the
/// member before the event is ever enqueued, so a refused or unavailable
/// transition leaves it gone all the same: the coordinator was not moved,
/// but the member was, and its generation's assignment is not coming
/// either.
///
/// ⚠️ **The first version of `M4.47` refused only on `Ok` and said in a
/// comment that this path owed nothing.** Review disproved it by refusing
/// the metadata append and watching the follower wait out
/// `MAX_SYNC_WAIT_MS` for `-1` — the row's own defect, one path over, under
/// a comment asserting it could not happen. `submit_assignment`'s `Err(_)`
/// arm refuses for this identical outage, so the same dependency failure
/// was being handled two opposite ways.
#[tokio::test(start_paused = true)]
async fn a_parked_follower_is_woken_even_when_the_leave_transition_fails() {
    let fixture = fixture(&[]).await;
    let g = oqueue_core::GroupId::new("orders-consumers").expect("valid");
    seat(&fixture.cluster, "orders-consumers", &["m1", "m2"]);

    let waiting = sync(
        &fixture.cluster,
        super::super::tests::follower_body_at("orders-consumers", "m2", 1),
    );
    tokio::pin!(waiting);
    assert!(crate::testing::poll_once(&mut waiting).is_none());

    fixture.refuse_group_metadata_append();
    fixture
        .cluster
        .heartbeats()
        .leave(&g, &["m1"], crate::heartbeat::Removal::of(&fixture.cluster))
        .await;

    assert!(
        !fixture.cluster.heartbeats().is_tracked(&g, "m1"),
        "the member is dropped before the enqueue, whatever the transition answers"
    );
    assert_eq!(
        fixture
            .cluster
            .group_coordinator()
            .record(&g)
            .map(|r| r.state),
        Some(GroupState::CompletingRebalance),
        "and the coordinator was not moved -- which is what made this look harmless"
    );

    let follower = waiting.await;
    assert_eq!(
        follower.error_code,
        error_codes::REBALANCE_IN_PROGRESS,
        "the follower is still told to rejoin: while the log is down no next generation can \
         release it, so this is the path with the longest wait, not the shortest"
    );
}

/// ⚠️ **`M4.48`'s own acceptance criterion: the last fatal give-up code in
/// the group protocol.** A follower that waits out `MAX_SYNC_WAIT_MS` was
/// answered `UNKNOWN_SERVER_ERROR`, which this module's own doc records the
/// Java consumer raising out of `poll()` and never retrying — and every
/// sibling give-up arm had already been changed away from it, each with the
/// reason written out: `join_group/mod.rs:120` ("the difference is whether
/// the client comes back"), `barrier.rs`'s `Known::Superseded`,
/// `join_group/mod.rs:221`. This one was missed, and nothing enforced the
/// rule because `check-fencing-seam.sh` covers `Refusal`'s six codes and
/// this is not one of them.
///
/// ⚠️ **Reachable with nothing wrong.** `M4.47`'s route reaches it when a
/// leader leaves during a log outage, and a leader that dies between
/// `JoinBarrierComplete` and its own `SyncGroup` reaches it with no outage
/// at all — nothing reaps in the background, and a parked follower is not
/// heartbeating, so nothing sweeps on its behalf. Found by M4's final
/// boundary review.
#[tokio::test(start_paused = true)]
async fn a_follower_that_waits_out_the_deadline_is_told_to_rejoin_not_given_a_fatal_code() {
    let fixture = fixture(&[]).await;
    seat(&fixture.cluster, "orders-consumers", &["m1", "m2"]);

    // Nobody submits, nobody leaves, nothing is refused: the leader simply
    // never arrives, which is what a SIGKILL between the barrier closing
    // and its own SyncGroup looks like from here.
    let follower = sync(
        &fixture.cluster,
        super::super::tests::follower_body_at("orders-consumers", "m2", 1),
    )
    .await;

    assert_eq!(
        follower.error_code,
        error_codes::REBALANCE_IN_PROGRESS,
        "a wait that runs out is nobody's fault, and must not reach a code the client cannot \
         retry"
    );
    assert!(
        follower.assignment.is_empty(),
        "a refusal carries no assignment"
    );
}

/// One newcomer's own `JoinGroup` through the real handler, so the round is
/// opened by the path `apply_open` actually takes rather than by a
/// coordinator transition this file fired itself.
///
/// ⚠️ **Its own copy rather than `heartbeat/tests.rs`'s `join`**, which is
/// `pub(super)` to that module. Three lines of encoding against a helper
/// visible from here is the cheaper of the two, and `code-structure.md`
/// rule 8 keeps a test helper beside the tests that use it.
async fn join_as_a_newcomer(cluster: &crate::cluster::Cluster, group: &str) {
    use kafka_protocol::messages::JoinGroupRequest as KpJoinRequest;
    use kafka_protocol::messages::join_group_request::JoinGroupRequestProtocol as KpProtocol;
    use kafka_protocol::protocol::{Encodable, StrBytes};

    const JOIN_VERSION: i16 = 5;
    let mut protocol = KpProtocol::default();
    protocol.name = StrBytes::from_static_str("range");
    protocol.metadata = bytes::Bytes::from_static(b"m");
    let request = KpJoinRequest::default()
        .with_group_id(kafka_protocol::messages::GroupId(StrBytes::from_string(
            group.to_owned(),
        )))
        .with_session_timeout_ms(30_000)
        .with_rebalance_timeout_ms(30_000)
        .with_member_id(StrBytes::from_static_str(""))
        .with_protocol_type(StrBytes::from_static_str("consumer"))
        .with_protocols(vec![protocol]);
    let mut body = Vec::new();
    request.encode(&mut body, JOIN_VERSION).expect("encodes");
    let _ = crate::join_group::handle(
        cluster,
        oqueue_codec::frame::RequestPrelude {
            api_key: 11,
            api_version: JOIN_VERSION,
            correlation_id: 1,
        },
        &body,
    )
    .await;
}

/// ⚠️ **`M4.58`'s own acceptance criterion, and the last route out of
/// `CompletingRebalance`.** `M4.43` gave the barrier a refusal and wired it
/// to `submit_assignment`; `M4.47` wired it to every route a member is
/// *lost* by, through `Heartbeats::remove_where`. A newcomer's `JoinGroup`
/// is neither: `join_group/round/plan.rs`'s `opening_event` picks
/// `MemberJoinedDuringSync`, which reopens the barrier from
/// `CompletingRebalance` while the previous generation's leader is still
/// computing an assignment it will now never submit.
///
/// ⚠️ **Measured before the fix**: the follower parked at generation 1 is
/// not released by the join at all, because the join opens a round and
/// publishes nothing — `Known::Waiting` survives it, and the member waits
/// out `MAX_SYNC_WAIT_MS`. It is a lesser defect than `M4.47`'s only
/// because the next generation's leader eventually supersedes it, which is
/// why it was its own row rather than a blocking finding.
///
/// ⚠️ **The generation refused is the interrupted one.** `group_state.rs`
/// advances the generation only at `JoinBarrierComplete`, so the record
/// read back after `MemberJoinedDuringSync` still names generation 1 — the
/// one this follower is parked on.
///
/// ⚠️ **The error code alone cannot carry this test, and the first version
/// of it was vacuous for exactly that reason.** `MAX_SYNC_WAIT_MS` also
/// answers `REBALANCE_IN_PROGRESS` (its own test, above), and under
/// `start_paused` the newcomer's round advances the clock past that
/// deadline on its own — so asserting the code passed with the wake
/// commented out, measured. What separates being *woken* from *timing out*
/// is that the answer is ready without the clock moving: `poll_once`
/// immediately after the join, and the barrier cell reading `Refused` for
/// generation 1 rather than `Waiting`.
#[tokio::test(start_paused = true)]
async fn a_parked_follower_is_woken_when_a_newcomer_reopens_the_barrier() {
    let fixture = fixture(&[]).await;
    let g = oqueue_core::GroupId::new("orders-consumers").expect("valid");
    seat(&fixture.cluster, "orders-consumers", &["m1", "m2"]);

    let waiting = sync(
        &fixture.cluster,
        super::super::tests::follower_body_at("orders-consumers", "m2", 1),
    );
    tokio::pin!(waiting);
    assert!(
        crate::testing::poll_once(&mut waiting).is_none(),
        "the follower must park on the barrier rather than answer at once"
    );

    // A newcomer arrives while generation 1 is still syncing. Nothing about
    // this member is wrong and nothing is lost — the round is simply
    // reopened on top of one the leader had not finished.
    join_as_a_newcomer(&fixture.cluster, "orders-consumers").await;

    let (assignments, _) = fixture.cluster.sync_groups().entry_for(&g);
    assert!(
        matches!(
            super::super::assignment_for(&assignments, 1),
            super::super::Known::Refused
        ),
        "the interrupted generation's cell must say Refused, not leave its followers Waiting"
    );
    drop(assignments);

    let follower = crate::testing::poll_once(&mut waiting)
        .expect("the follower must already be answerable, without waiting out MAX_SYNC_WAIT_MS");
    assert_eq!(
        follower.error_code,
        error_codes::REBALANCE_IN_PROGRESS,
        "a follower whose generation was interrupted by a newcomer must be told to rejoin"
    );
    assert!(
        follower.assignment.is_empty(),
        "a refusal carries no assignment"
    );
}

/// What [`a_leader_resuming_after_a_member_was_lost`] observed, so the two
/// harms it produces can be asserted by tests named for one each.
struct Resumed {
    leader: kafka_protocol::messages::SyncGroupResponse,
    follower: kafka_protocol::messages::SyncGroupResponse,
    cell_refused: bool,
    group_state_on_resume: Option<GroupState>,
}

/// Drives the interleaving `M4.65` is about, deterministically.
///
/// A follower parks on the barrier; the leader's submission then stops on
/// the transitions actor's oneshot, which is where the record it will judge
/// `synced` from is still in flight — it arrives *with* the oneshot, and is
/// the state at the moment the actor applied `SyncComplete` rather than the
/// state when this task is next polled. A third member is then lost, which
/// is legal from `Stable` and takes the group off the generation the leader
/// is still submitting for, publishing a refusal for it. Only then does the
/// leader resume, with a record describing a generation the group has left.
///
/// ⚠️ **Nothing here is timing.** The two futures are polled by hand and
/// never spawned, so neither can advance while `leave` is awaited — the
/// order above is the order the runtime must produce, not one it happens to.
async fn a_leader_resuming_after_a_member_was_lost(fixture: &crate::testing::Fixture) -> Resumed {
    let g = oqueue_core::GroupId::new("orders-consumers").expect("valid");
    seat(&fixture.cluster, "orders-consumers", &["m1", "m2", "m3"]);

    let waiting = sync(
        &fixture.cluster,
        super::super::tests::follower_body_at("orders-consumers", "m2", 1),
    );
    tokio::pin!(waiting);
    assert!(
        crate::testing::poll_once(&mut waiting).is_none(),
        "the follower must park on the barrier rather than answer at once"
    );

    let submitting = sync(
        &fixture.cluster,
        super::super::tests::leader_body_at(
            "orders-consumers",
            "m1",
            &[("m1", b"slice-for-m1"), ("m2", b"slice-for-m2")],
            1,
        ),
    );
    tokio::pin!(submitting);
    assert!(
        crate::testing::poll_once(&mut submitting).is_none(),
        "the leader must park on the durable append, or this proves nothing about the race"
    );

    fixture
        .cluster
        .heartbeats()
        .leave(&g, &["m3"], crate::heartbeat::Removal::of(&fixture.cluster))
        .await;
    let group_state_on_resume = fixture
        .cluster
        .group_coordinator()
        .record(&g)
        .map(|r| r.state);

    let leader = submitting.await;
    let (assignments, _) = fixture.cluster.sync_groups().entry_for(&g);
    let cell_refused = matches!(
        super::super::assignment_for(&assignments, 1),
        super::super::Known::Refused
    );
    drop(assignments);

    Resumed {
        leader,
        follower: waiting.await,
        cell_refused,
        group_state_on_resume,
    }
}

/// ⚠️ **The route through the writer `M4.47` added**, and the one the three
/// tests above do not cover: the group leaves the generation while the
/// leader's *own* submission is in flight, and the leader then overwrites
/// the refusal that was published for it.
///
/// `submit_assignment` judged `synced` from the record the transition
/// returned and never re-read it, so it arrived at `barrier::submit` with a
/// question already answered stale — and `submit` had nothing to check it
/// against, its own guard seeing only generations it could compare. `M4.65`
/// gives it the question instead, asked under the cell's own lock, which is
/// the only side of that lock a second task cannot interleave across.
///
/// ⚠️ **Not the mirror of `refuse`'s guard**, which was the obvious repair
/// and is wrong: `{generation, None}` also means this same generation's
/// leader was refused by a transient append failure and is retrying, which
/// `durability::the_same_leader_succeeds_once_the_log_heals` pins.
#[tokio::test(start_paused = true)]
async fn a_leader_that_resumes_after_the_group_left_does_not_overwrite_the_refusal() {
    let fixture = fixture(&[]).await;
    let resumed = a_leader_resuming_after_a_member_was_lost(&fixture).await;

    assert_eq!(
        resumed.group_state_on_resume,
        Some(GroupState::PreparingRebalance),
        "the group must have left generation 1 before the leader resumed"
    );
    assert_eq!(
        resumed.leader.error_code,
        error_codes::REBALANCE_IN_PROGRESS,
        "a leader whose group left the generation mid-submission is told to rejoin"
    );
    assert!(
        resumed.cell_refused,
        "the refusal published for generation 1 must survive the leader's own submission"
    );
}

/// ⚠️ **And the harm that reaches a real consumer**, which is the reason
/// this is protocol correctness rather than bookkeeping: the followers
/// overwritten here are the ones the same removal has just told to rejoin.
/// Answering them `NONE` with a slice for the generation they were told to
/// leave is the revoke-before-reassign harm `M4.42` and `M4.45` exist to
/// prevent, arriving through the writer `M4.47` added to prevent the
/// opposite one.
#[tokio::test(start_paused = true)]
async fn a_follower_told_to_rejoin_is_not_then_handed_that_generations_slice() {
    let fixture = fixture(&[]).await;
    let resumed = a_leader_resuming_after_a_member_was_lost(&fixture).await;

    assert_eq!(
        resumed.follower.error_code,
        error_codes::REBALANCE_IN_PROGRESS,
        "a follower told to rejoin must not then be handed a slice for the generation it left"
    );
    assert!(
        resumed.follower.assignment.is_empty(),
        "a refusal carries no assignment"
    );
}

/// ⚠️ **The route that has no refusal of its own**, and the one the fix for
/// `M4.65` would have opened had review not measured it.
///
/// `submit` now returns `None` for two reasons, not one. The older reason —
/// the cell already holds a *newer* generation — leaves every follower on
/// this one reading `Known::Superseded`, woken by the submission that put it
/// there, so the arm answering it needed no refusal. The newer reason does
/// not: a newcomer joining a group that has reached `Stable` plans
/// `GroupEvent::Join`, not `MemberJoinedDuringSync`, so `join_group::round`
/// publishes nothing, and the cell holds nothing for the generation the
/// leader was still submitting for.
///
/// ⚠️ **Measured before the arm was given its `refuse` call**: the follower
/// is answered `NONE` with a twelve-byte slice if the closure is stubbed
/// true, and parks out `MAX_SYNC_WAIT_MS` for `REBALANCE_IN_PROGRESS` if it
/// is not — `waited_ms=3000000`, the stranding `M4.43` and `M4.47` exist to
/// end, arriving through the repair for a different one.
#[tokio::test(start_paused = true)]
async fn a_follower_is_woken_when_the_leader_is_refused_by_a_route_that_published_nothing() {
    let fixture = fixture(&[]).await;
    seat(&fixture.cluster, "orders-consumers", &["m1", "m2"]);

    let waiting = sync(
        &fixture.cluster,
        super::super::tests::follower_body_at("orders-consumers", "m2", 1),
    );
    tokio::pin!(waiting);
    assert!(crate::testing::poll_once(&mut waiting).is_none());

    let submitting = sync(
        &fixture.cluster,
        super::super::tests::leader_body_at(
            "orders-consumers",
            "m1",
            &[("m1", b"slice-for-m1"), ("m2", b"slice-for-m2")],
            1,
        ),
    );
    tokio::pin!(submitting);
    assert!(crate::testing::poll_once(&mut submitting).is_none());

    // A newcomer arrives *after* `SyncComplete` has landed, so the group is
    // `Stable` and this plans `Join` -- the route that refuses nothing.
    join_as_a_newcomer(&fixture.cluster, "orders-consumers").await;

    let leader = submitting.await;
    assert_eq!(leader.error_code, error_codes::REBALANCE_IN_PROGRESS);

    let follower = crate::testing::poll_once(&mut waiting)
        .expect("the follower must be answerable at once, not after MAX_SYNC_WAIT_MS");
    assert_eq!(
        follower.error_code,
        error_codes::REBALANCE_IN_PROGRESS,
        "a refused submission must wake the followers parked on it, whatever refused it"
    );
    assert!(
        follower.assignment.is_empty(),
        "a refusal carries no assignment"
    );
}
