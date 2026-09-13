//! Losing a member while a rebalance is already under way.
//!
//! ⚠️ **Its own file because the state the group is *in* is the variable.**
//! Its sibling asks whether a silent member is evicted at the right moment;
//! these ask what the coordinator is told when it is, from a state other
//! than `Stable`. `Heartbeats::remove_where` fired `GroupEvent::Join` for
//! every partial loss and the transition table admits `Join` only from
//! `Empty | Stable`, so from either rebalancing state the coordinator was
//! never told at all — `M4.34`, found by M4's closing review.

#![allow(clippy::expect_used)]

use super::{group, heartbeat, join};
use crate::testing::fixture;
use oqueue_core::GroupState;

/// Joins two members and stops before either syncs, leaving the group at
/// `CompletingRebalance` — the window between the join barrier closing and
/// the leader's `SyncGroup` landing, which is where a real leader's death
/// is most costly because the group has elected it and is waiting on it.
///
/// Returns `(leader, follower)` in join order.
async fn a_group_waiting_on_its_leaders_sync(
    cluster: &std::sync::Arc<crate::cluster::Cluster>,
    group_name: &'static str,
    session_timeout_ms: [i32; 2],
) -> (String, String) {
    const REBALANCE_TIMEOUT_MS: i32 = 1_000;
    let joiners: Vec<_> = session_timeout_ms
        .into_iter()
        .map(|timeout| {
            let cluster = std::sync::Arc::clone(cluster);
            tokio::spawn(
                async move { join(&cluster, group_name, timeout, REBALANCE_TIMEOUT_MS).await },
            )
        })
        .collect();
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;
    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
    let mut ids = Vec::new();
    for joiner in joiners {
        ids.push(joiner.await.expect("the joiner task joins"));
    }
    (ids[0].clone(), ids[1].clone())
}

fn state(cluster: &crate::cluster::Cluster, group_name: &str) -> Option<GroupState> {
    cluster
        .group_coordinator()
        .record(&group(group_name))
        .map(|r| r.state)
}

/// ⚠️ **`M4.34`'s own acceptance criterion.** A group that has elected a
/// leader and is waiting on its `SyncGroup` must not be stuck there when
/// that leader dies.
///
/// ⚠️ **The cost of the refused transition is not a log line.** The
/// coordinator holds `CompletingRebalance` at the old generation with no
/// leader; `offset_commit`'s fence is `[Stable]`, so the surviving member's
/// every commit is refused, its every heartbeat answers
/// `REBALANCE_IN_PROGRESS`, and a follower parked in `SyncGroup` waits out
/// `MAX_SYNC_WAIT_MS` for an assignment nobody will ever submit. Nothing
/// broker-side recovers it — real clients escape only because their own
/// request timeout fires and they send a fresh `JoinGroup`.
#[tokio::test(start_paused = true)]
async fn a_leader_dying_mid_sync_reopens_the_join_barrier() {
    let fixture = std::sync::Arc::new(fixture(&[]).await);
    let (leader, follower) =
        a_group_waiting_on_its_leaders_sync(&fixture.cluster, "orders", [10_000, 60_000]).await;
    assert_eq!(
        state(&fixture.cluster, "orders"),
        Some(GroupState::CompletingRebalance),
        "the barrier closed and the group is waiting on the leader"
    );

    // The leader sends no `SyncGroup` and no further heartbeat; advance
    // past its own 10s session timeout.
    tokio::time::sleep(std::time::Duration::from_secs(11)).await;

    // The follower's own heartbeat is the only thing that sweeps -- nothing
    // reaps in the background (`Heartbeats::is_live`'s own doc).
    let error_code = heartbeat(&fixture.cluster, "orders", &follower, 1).await;

    assert!(
        !fixture
            .cluster
            .heartbeats()
            .is_tracked(&group("orders"), &leader),
        "the sweep did evict the leader -- if this fails the test proves nothing about the event"
    );
    assert_eq!(
        state(&fixture.cluster, "orders"),
        Some(GroupState::PreparingRebalance),
        "a group that lost the leader it was waiting on must reopen its join barrier, not hold \
         CompletingRebalance with nobody to sync it"
    );
    assert_eq!(
        error_code,
        oqueue_codec::error_codes::REBALANCE_IN_PROGRESS,
        "and the survivor is told to rejoin"
    );
}

/// ⚠️ **The same for a member lost while the barrier is still collecting
/// joins**, which `Join` was refused from too.
///
/// ⚠️ **The state cannot be the assertion here, and the first version of
/// this test pretended otherwise.** `MemberLeft` from `PreparingRebalance`
/// is a self-transition, so a group that took it and a group whose event
/// was refused look identical from `record()` — review of `M4.34` proved
/// that version passed unchanged against the old `GroupEvent::Join`. What
/// separates them is whether the coordinator was *called*:
/// `group_transitions::handle_one` runs the pure transition first and
/// returns on `Err` before either the durable append or
/// `coordinator.transition`, so a refused event never reaches the counter.
/// That is the divergence `M4.15d` put an actor there to prevent — the
/// round's own `awaiting` set has already stopped waiting for the dead
/// member (`M4.16`), and the coordinator disagreeing about whether it was
/// told is what nobody could see.
#[tokio::test(start_paused = true)]
async fn a_member_lost_while_the_barrier_is_open_reaches_the_coordinator() {
    let fixture = std::sync::Arc::new(fixture(&[]).await);
    let (leader, follower) =
        a_group_waiting_on_its_leaders_sync(&fixture.cluster, "orders", [10_000, 60_000]).await;

    // Back to a collecting barrier: the survivor rejoins, which is what a
    // real client does on `REBALANCE_IN_PROGRESS`. ⚠️ A 60s rebalance
    // timeout, so the round is still *open* when the leader's own 10s
    // session runs out below -- with the 1s this file's other setup uses,
    // the round closes first and the sweep fires from
    // `CompletingRebalance`, which is the sibling test over again.
    let cluster = std::sync::Arc::clone(&fixture.cluster);
    let rejoin = tokio::spawn(async move { join(&cluster, "orders", 60_000, 60_000).await });
    tokio::task::yield_now().await;
    tokio::task::yield_now().await;
    assert_eq!(
        state(&fixture.cluster, "orders"),
        Some(GroupState::PreparingRebalance),
        "a join against a Completing group reopens the barrier"
    );

    tokio::time::sleep(std::time::Duration::from_secs(11)).await;
    assert_eq!(
        state(&fixture.cluster, "orders"),
        Some(GroupState::PreparingRebalance),
        "the round is still collecting -- otherwise this tests the sibling's path"
    );

    let calls_before = fixture.group_coordinator.transition_calls();
    // The parked rejoiner is a different request from this heartbeat; the
    // member is still tracked, so its heartbeat is what sweeps.
    let _ = heartbeat(&fixture.cluster, "orders", &follower, 1).await;

    assert!(
        !fixture
            .cluster
            .heartbeats()
            .is_tracked(&group("orders"), &leader),
        "the sweep did evict the leader -- if this fails the test proves nothing about the event"
    );
    assert!(
        fixture.group_coordinator.transition_calls() > calls_before,
        "a member lost from PreparingRebalance must reach the coordinator, not be refused by the \
         transition table with nobody reading the result"
    );

    rejoin.abort();
}
