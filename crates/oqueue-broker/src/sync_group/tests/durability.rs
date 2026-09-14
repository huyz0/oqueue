//! A durable append that failed, and the publishing rule underneath it.
//!
//! ⚠️ **Its own file because the subject is absence, not bytes.** Its
//! siblings ask which slice a member gets and which generation those bytes
//! belong to; these ask what a member waiting on an assignment is told when
//! there will not be one.
//!
//! **Two halves, and neither is a superset of the other.** The first drives
//! the real handler with the group metadata log refusing: the leader's map
//! is correct and what is wrong is that nothing recorded the group reaching
//! `Stable` (`M4.33`, `M4.43`) — a handler answering `NONE` there has told
//! every member to start consuming against a coordinator that will refuse
//! them. The second drives `SyncGroups::submit`/`refuse` directly, with no
//! leader and no handler at all, and asks which generation's cell a refusal
//! may overwrite.
//!
//! ⚠️ **The second half is here rather than in `reopened.rs` on purpose**:
//! the overwrite guard is a property of the publishing rule, not of the
//! route that reached it, and every route in that file depends on it
//! holding. Naming this file after the first half alone was the wording
//! `M4.58`'s review caught, false for four of its eight tests.
//!
//! ⚠️ **What is *not* here since `M4.58`**: a follower parked where there is
//! no map and no submission at all, because the leader left, was evicted,
//! or a newcomer reopened the round under it. That is `reopened.rs`. This
//! doc has now been wrong about its own contents twice — the half claiming
//! a correct leader map in *every* test survived a round of review after
//! the tests it described had stopped having one, and the title survived
//! the split that took half its subject away.

#![allow(clippy::expect_used)]

use super::{leader_body_at, seat, sync};
use crate::testing::fixture;
use oqueue_codec::error_codes;
use oqueue_core::GroupState;

fn state(cluster: &crate::cluster::Cluster, group: &str) -> Option<GroupState> {
    let g = oqueue_core::GroupId::new(group).expect("valid");
    cluster.group_coordinator().record(&g).map(|r| r.state)
}

/// ⚠️ **`M4.33`'s own acceptance criterion.** A refused durable append is a
/// broken dependency, not a lost race, and the two must not be conflated —
/// `join_group`'s two transition call sites were both fixed for exactly
/// this and `sync_group` was the third, which nothing noticed until M4's
/// closing review read the three together.
///
/// ⚠️ **`NONE` here is the worst answer available**, which is why this
/// asserts the code rather than merely that something failed. The leader
/// and every follower it feeds start consuming; the coordinator never left
/// `CompletingRebalance`, so `offset_commit`'s own `[Stable]` fence refuses
/// every commit and every heartbeat answers `REBALANCE_IN_PROGRESS` — and
/// nothing re-fires `SyncComplete` when the log heals, so the group only
/// recovers by burning a whole extra generation.
#[tokio::test(start_paused = true)]
async fn a_leader_whose_sync_cannot_be_made_durable_is_not_told_it_succeeded() {
    let fixture = fixture(&[]).await;
    seat(&fixture.cluster, "orders-consumers", &["m1", "m2"]);
    fixture.refuse_group_metadata_append();

    let leader = sync(
        &fixture.cluster,
        leader_body_at(
            "orders-consumers",
            "m1",
            &[("m1", b"gen1-m1"), ("m2", b"gen1-m2")],
            1,
        ),
    )
    .await;

    assert_eq!(
        leader.error_code,
        error_codes::COORDINATOR_NOT_AVAILABLE,
        "a leader whose SyncComplete never reached the log must be told to retry, not NONE"
    );
    assert!(
        leader.assignment.is_empty(),
        "a refusal carries no assignment -- a member handed bytes alongside an error code is \
         being invited to use them"
    );
    assert_eq!(
        state(&fixture.cluster, "orders-consumers"),
        Some(GroupState::CompletingRebalance),
        "the group must not appear to have synced"
    );
}

/// ⚠️ **The other half: a refusal the coordinator itself issues is still
/// answered with the map.** `sync_group`'s original `let _ =` was written
/// for this case and is right about it — a second submission racing another
/// connection finds the group already `Stable`, which makes `SyncComplete`
/// illegal, and the map that submission carries is still the one to answer
/// with. Conflating it with the case above is what made the bug; this test
/// is what stops the fix from over-correcting into refusing both.
#[tokio::test(start_paused = true)]
async fn a_second_submission_against_an_already_stable_group_is_still_answered() {
    let fixture = fixture(&[]).await;
    seat(&fixture.cluster, "orders-consumers", &["m1", "m2"]);
    let body = || {
        leader_body_at(
            "orders-consumers",
            "m1",
            &[("m1", b"gen1-m1"), ("m2", b"gen1-m2")],
            1,
        )
    };

    let first = sync(&fixture.cluster, body()).await;
    assert_eq!(first.error_code, error_codes::NONE);
    assert_eq!(
        state(&fixture.cluster, "orders-consumers"),
        Some(GroupState::Stable)
    );

    // `Stable -> SyncComplete` is illegal, so the transition is refused --
    // and the reply must survive it.
    let second = sync(&fixture.cluster, body()).await;
    assert_eq!(
        second.error_code,
        error_codes::NONE,
        "a double submission is a lost race, not a broken dependency"
    );
    assert_eq!(second.assignment, b"gen1-m1".as_slice());
}

/// ⚠️ **A retry after the log heals must work**, which is the whole point of
/// answering a retriable code: `COORDINATOR_NOT_AVAILABLE` is what tells a
/// real client to come back rather than to give up on the group.
///
/// ⚠️ **The retry here is the same request, which no real client sends** —
/// both reference clients rejoin on any non-`NONE` `SyncGroup` code, so a
/// real leader spends the generation. What this pins is that the handler
/// has left nothing behind that would refuse the retry, which is the
/// property a client's *rejoined* sync depends on just as much.
#[tokio::test(start_paused = true)]
async fn the_same_leader_succeeds_once_the_log_heals() {
    let fixture = fixture(&[]).await;
    seat(&fixture.cluster, "orders-consumers", &["m1", "m2"]);
    let body = || {
        leader_body_at(
            "orders-consumers",
            "m1",
            &[("m1", b"gen1-m1"), ("m2", b"gen1-m2")],
            1,
        )
    };

    fixture.refuse_group_metadata_append();
    let refused = sync(&fixture.cluster, body()).await;
    assert_eq!(refused.error_code, error_codes::COORDINATOR_NOT_AVAILABLE);

    fixture.heal_group_metadata_log();
    let retried = sync(&fixture.cluster, body()).await;
    assert_eq!(retried.error_code, error_codes::NONE);
    assert_eq!(retried.assignment, b"gen1-m1".as_slice());
    assert_eq!(
        state(&fixture.cluster, "orders-consumers"),
        Some(GroupState::Stable),
        "the handler left nothing behind that refuses the retry"
    );
}

/// ⚠️ **`M4.43`'s own acceptance criterion.** A follower already parked on
/// the barrier when its leader's submission is refused must be told to
/// rejoin, not left waiting for an assignment nobody will ever submit.
///
/// ⚠️ **Nothing releases it otherwise while the outage lasts.** The
/// follower is normally freed as `Known::Superseded` by the *next*
/// generation's submission — but the leader's refusal came from the group
/// metadata log being unreachable, and every path to a next generation goes
/// through the same log, so there is no next generation to free it. It
/// waits out `MAX_SYNC_WAIT_MS` — fifty minutes — and is then answered
/// `UNKNOWN_SERVER_ERROR`, which this module's own doc records the Java
/// consumer raising out of `poll()` and never retrying. Found by review of
/// `M4.33`, which judged the deferral defensible and filed it rather than
/// widening that commit.
#[tokio::test(start_paused = true)]
async fn a_parked_follower_is_woken_when_its_leaders_submission_is_refused() {
    let fixture = fixture(&[]).await;
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

    fixture.refuse_group_metadata_append();
    let leader = sync(
        &fixture.cluster,
        leader_body_at(
            "orders-consumers",
            "m1",
            &[("m1", b"gen1-m1"), ("m2", b"gen1-m2")],
            1,
        ),
    )
    .await;
    assert_eq!(
        leader.error_code,
        error_codes::COORDINATOR_NOT_AVAILABLE,
        "the leader is refused -- otherwise this test proves nothing about the follower"
    );

    let follower = waiting.await;
    assert_eq!(
        follower.error_code,
        error_codes::REBALANCE_IN_PROGRESS,
        "and the follower is told to rejoin rather than left parked for MAX_SYNC_WAIT_MS"
    );
    assert!(
        follower.assignment.is_empty(),
        "a refusal carries no assignment"
    );
}

/// ⚠️ **An older generation's map must not suppress the refusal**, which is
/// the case the acceptance test above cannot reach: it starts from a group
/// that has never synced, so the cell is empty and `refuse`'s guard
/// short-circuits before looking at anything. Every group that has run a
/// round leaves a map behind, so this is the *common* shape, and the first
/// version of the guard failed it — the follower parked for
/// `MAX_SYNC_WAIT_MS` and was answered `UNKNOWN_SERVER_ERROR`, exactly what
/// the row exists to remove. Found by review.
#[tokio::test(start_paused = true)]
async fn a_refusal_overwrites_an_older_generations_assignment() {
    let fixture = fixture(&[]).await;
    let g = oqueue_core::GroupId::new("orders-consumers").expect("valid");
    let sync_groups = fixture.cluster.sync_groups();
    assert!(
        sync_groups
            .submit(&g, 1, [("m1".to_owned(), b"gen1".to_vec())].into())
            .is_some(),
        "the group synced once before"
    );

    sync_groups.refuse(&g, 2);

    let (assignments, _) = sync_groups.entry_for(&g);
    assert!(
        matches!(
            super::super::assignment_for(&assignments, 2),
            super::super::Known::Refused
        ),
        "generation 2's followers must be told to rejoin, not left on Waiting behind \
         generation 1's leftovers"
    );
}

/// ⚠️ **A refusal must never overwrite an assignment**, which is the half of
/// `SyncGroups::refuse` that `cargo mutants` found pinned by nothing — its
/// guard survived three mutations. Two shapes, and this covers the later
/// generation's: a refusal arriving for a generation the group has already
/// moved past is the same hazard `M4.35` exists to prevent, reached through
/// the other writer.
#[tokio::test(start_paused = true)]
async fn a_refusal_does_not_overwrite_a_later_generations_assignment() {
    let fixture = fixture(&[]).await;
    let g = oqueue_core::GroupId::new("orders-consumers").expect("valid");
    let sync_groups = fixture.cluster.sync_groups();
    assert!(
        sync_groups
            .submit(&g, 2, [("m1".to_owned(), b"gen2".to_vec())].into())
            .is_some()
    );

    sync_groups.refuse(&g, 1);

    let (assignments, _) = sync_groups.entry_for(&g);
    assert!(
        matches!(super::super::assignment_for(&assignments, 2), super::super::Known::Mine(map)
            if map.get("m1").map(Vec::as_slice) == Some(b"gen2".as_slice())),
        "generation 2's map must survive a late refusal for generation 1"
    );
}

/// ⚠️ **And this generation's, published by whoever got there first.** A
/// second connection's submission can land between this one's failed
/// transition and its `refuse`, and unpublishing it would strand every
/// follower it was about — the exact failure `refuse` was added to end,
/// arrived at from the other side.
#[tokio::test(start_paused = true)]
async fn a_refusal_does_not_unpublish_this_generations_own_assignment() {
    let fixture = fixture(&[]).await;
    let g = oqueue_core::GroupId::new("orders-consumers").expect("valid");
    let sync_groups = fixture.cluster.sync_groups();
    assert!(
        sync_groups
            .submit(&g, 2, [("m1".to_owned(), b"gen2".to_vec())].into())
            .is_some()
    );

    sync_groups.refuse(&g, 2);

    let (assignments, _) = sync_groups.entry_for(&g);
    assert!(
        matches!(super::super::assignment_for(&assignments, 2), super::super::Known::Mine(map)
            if map.get("m1").map(Vec::as_slice) == Some(b"gen2".as_slice())),
        "a refusal for a generation that already has an assignment must leave it alone"
    );
}

/// ⚠️ **And a later generation's *refusal* must survive too**, which is the
/// only case the `>` half of that guard decides on its own: when the cell
/// holds a map, `map.is_some()` answers first, so `cargo mutants` reported
/// the comparison surviving three mutations until this test existed.
///
/// The cost of getting it wrong is the failure `refuse` was added to end,
/// one generation over: a follower of the later generation would read
/// `Known::Waiting` again and park for `MAX_SYNC_WAIT_MS`.
#[tokio::test(start_paused = true)]
async fn a_late_refusal_does_not_reopen_a_later_generations_refusal() {
    let fixture = fixture(&[]).await;
    let g = oqueue_core::GroupId::new("orders-consumers").expect("valid");
    let sync_groups = fixture.cluster.sync_groups();

    sync_groups.refuse(&g, 2);
    sync_groups.refuse(&g, 1);

    let (assignments, _) = sync_groups.entry_for(&g);
    assert!(
        matches!(
            super::super::assignment_for(&assignments, 2),
            super::super::Known::Refused
        ),
        "generation 2's followers must still be told to rejoin, not sent back to Waiting"
    );
}
