//! What a leader is told when its `SyncComplete` cannot be made durable.
//!
//! ⚠️ **Its own file because the question is not about the assignment.**
//! Its siblings ask which bytes a member gets and which generation they
//! belong to; these ask whether the group's own record survived the
//! submission at all. The leader's map is correct in every test here — what
//! is wrong is that nothing recorded the group reaching `Stable`, and a
//! handler that answers `NONE` anyway has told every member to start
//! consuming against a coordinator that will refuse them.

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
