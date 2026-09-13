//! Which generation's assignment a member is answered with.
//!
//! ⚠️ **Its own file because the generation is a separate property from the
//! slice.** Its sibling asks whether a member gets *its own* bytes out of
//! the leader's submission; these ask whether those bytes belong to the
//! generation the member actually asked about. The barrier holds one
//! generation at a time, so both of the ways that can go wrong — being
//! answered from an older generation, and being left waiting for one the
//! group has moved past — live here.

#![allow(clippy::expect_used)]

use super::{follower_body_at, leader_body_at, seat, sync};
use crate::testing::fixture;

/// Drives a `Stable` group through one more round, to the next generation.
///
/// ⚠️ The leader's own `SyncGroup` fires `SyncComplete` itself, so a group
/// whose assignment has landed is already `Stable` — this is `seat`'s second
/// half, for a group that has one generation behind it.
fn rebalance_again(cluster: &crate::cluster::Cluster, group: &str) {
    let g = oqueue_core::GroupId::new(group).expect("valid");
    cluster
        .group_coordinator()
        .transition(&g, oqueue_core::GroupEvent::Join)
        .expect("Stable -> PreparingRebalance is legal");
    cluster
        .group_coordinator()
        .transition(&g, oqueue_core::GroupEvent::JoinBarrierComplete)
        .expect("PreparingRebalance -> CompletingRebalance is legal");
}

/// ⚠️ **A follower syncing for generation N must never be answered with
/// generation N-1's assignment.** The barrier a follower waits on holds one
/// map per group, and until `M4.17` it held no generation — so a follower
/// that reached `SyncGroup` for the *next* generation before its leader did
/// found the previous generation's map already present and was answered
/// from it at once, without waiting.
///
/// ⚠️ **The cost is a consumer that never gets anything, and a real client
/// found it.** A consumer added to a working group is a newcomer in the
/// round that admits it, and a cooperative assignor gives a newcomer nothing
/// in that round — so the stale map holds its own *empty* slice. Answered
/// from it, the consumer concluded it owned nothing, went `steady`, and
/// stayed idle for as long as the group ran while the incumbents kept every
/// partition. Two of these in a row is how `M4.17`'s librdkafka leg failed.
#[tokio::test(start_paused = true)]
async fn a_follower_is_never_answered_with_a_previous_generations_assignment() {
    let fixture = fixture(&[]).await;
    let group = "orders-consumers";
    seat(&fixture.cluster, group, &["m1", "m2"]);

    // Generation 1 completes: the newcomer "m2" is assigned nothing, exactly
    // as a cooperative assignor leaves it in the round that admits it.
    let leader = sync(
        &fixture.cluster,
        leader_body_at(group, "m1", &[("m1", b"gen1-m1"), ("m2", b"")], 1),
    )
    .await;
    assert_eq!(leader.error_code, 0, "the leader's own submission lands");

    rebalance_again(&fixture.cluster, group);

    // "m2" reaches SyncGroup for generation 2 before its leader does. The
    // stale map is still sitting on the barrier, and it says "m2" owns
    // nothing.
    let follower = {
        let cluster = std::sync::Arc::clone(&fixture.cluster);
        let body = follower_body_at(group, "m2", 2);
        tokio::spawn(async move { sync(&cluster, body).await })
    };
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }
    assert!(
        !follower.is_finished(),
        "the follower must still be waiting — generation 2's leader has not \
         submitted, and generation 1's map is not an answer to a generation \
         2 request"
    );

    let leader = sync(
        &fixture.cluster,
        leader_body_at(group, "m1", &[("m1", b"gen2-m1"), ("m2", b"gen2-m2")], 2),
    )
    .await;
    assert_eq!(leader.error_code, 0, "generation 2's leader submits");

    for _ in 0..64 {
        tokio::task::yield_now().await;
    }
    let follower = follower.await.expect("the follower is answered");
    assert_eq!(follower.error_code, 0, "the follower syncs");
    assert_eq!(
        follower.assignment.as_ref(),
        b"gen2-m2",
        "its own generation's slice, not the empty one the previous \
         generation left it"
    );
}

/// ⚠️ **A follower whose generation the group moves past *while it is
/// parked* is released at once, retriably — not held to the deadline and
/// then killed.** The assignment barrier holds one generation at a time, so
/// a member that parked for generation N while the group rebalanced again
/// is waiting for something that is never coming.
///
/// ⚠️ **It has to go stale *after* it parks**, which is what this sets up: a
/// request whose generation is already behind the coordinator is refused
/// `ILLEGAL_GENERATION` by `fence` and never reaches the barrier at all. The
/// interesting member is the one that was current when it arrived — and the
/// only way the group leaves it behind is `MemberJoinedDuringSync`, since
/// every other route out of `CompletingRebalance` runs through the leader's
/// own submission, which would have answered it.
///
/// ⚠️ **The wrong answer here is fatal, which is what makes it worth a
/// test.** Waiting out `MAX_SYNC_WAIT_MS` — 50 minutes — and then answering
/// `UNKNOWN_SERVER_ERROR` is what the code did before: the Java consumer
/// raises that out of `poll()` and never retries, so one overtaken follower
/// loses its consumer for good. `REBALANCE_IN_PROGRESS` is what real Kafka
/// answers and what a client rejoins on. `M4.15d` learned this four separate
/// times on the `JoinGroup` side; this is the `SyncGroup` side of it. Found
/// by review.
#[tokio::test(start_paused = true)]
async fn a_follower_overtaken_by_a_later_generation_is_told_to_rejoin() {
    let fixture = fixture(&[]).await;
    let group = "orders-consumers";
    seat(&fixture.cluster, group, &["m1", "m2"]);

    // "m2" syncs at generation 1 while that is still current, and parks:
    // generation 1's leader has not submitted.
    let straggler = {
        let cluster = std::sync::Arc::clone(&fixture.cluster);
        let body = follower_body_at(group, "m2", 1);
        tokio::spawn(async move { sync(&cluster, body).await })
    };
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }
    assert!(
        !straggler.is_finished(),
        "it must genuinely be parked — if it were refused up front this test \
         would prove nothing about the barrier"
    );

    // ⚠️ **`MemberJoinedDuringSync` is the reachable way past it**, and the
    // only one: every other route out of `CompletingRebalance` needs
    // `SyncComplete`, which the leader's own submission fires — and that
    // submission would have answered this follower correctly on the way. A
    // consumer joining while the group is mid-sync advances the generation
    // with no leader submission at all, stranding whoever was already
    // parked.
    let g = oqueue_core::GroupId::new(group).expect("valid");
    fixture
        .cluster
        .group_coordinator()
        .transition(&g, oqueue_core::GroupEvent::MemberJoinedDuringSync)
        .expect("CompletingRebalance -> PreparingRebalance is legal");
    // ⚠️ The generation advances here and nowhere else (`ADR-0033`): the
    // event above re-opens the barrier at the *same* generation, and only
    // closing it moves to 2.
    fixture
        .cluster
        .group_coordinator()
        .transition(&g, oqueue_core::GroupEvent::JoinBarrierComplete)
        .expect("PreparingRebalance -> CompletingRebalance is legal");
    let leader = sync(
        &fixture.cluster,
        leader_body_at(group, "m1", &[("m1", b"gen2-m1"), ("m2", b"gen2-m2")], 2),
    )
    .await;
    assert_eq!(leader.error_code, 0, "generation 2's leader submits");

    // No clock advance at all: the submission that overtook it is what must
    // release it, not the 50-minute deadline.
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }
    assert!(
        straggler.is_finished(),
        "an overtaken follower must be released immediately — holding it to \
         `MAX_SYNC_WAIT_MS` is 50 minutes of a consumer doing nothing"
    );
    let straggler = straggler.await.expect("answered");
    assert_eq!(
        straggler.error_code,
        oqueue_codec::error_codes::REBALANCE_IN_PROGRESS,
        "retriable: the client rejoins and syncs at the generation the group \
         is actually on. UNKNOWN_SERVER_ERROR here is fatal to the Java \
         consumer and loses it for good"
    );
}

/// ⚠️ **`M4.35`'s own acceptance criterion**: a submission for an older
/// generation must not replace a newer generation's map.
///
/// ⚠️ **The window is real, not theoretical.** `handle`'s fence reads the
/// group's record synchronously and the submission then `await`s a full
/// `mpsc` + `oneshot` round trip through one actor doing a `last_version`
/// read and an append against object storage, serialized across every
/// group — `join_group/round/state.rs` calls that wait "tens of
/// milliseconds in the good case and seconds in a plausible bad one". In
/// that window a `MemberJoinedDuringSync` plus a `JoinBarrierComplete` can
/// advance the group and N+1's leader can submit first.
///
/// ⚠️ **Every other post-`await` write in M4 has this guard** —
/// `CommittedOffsets::apply` compares a `CommitVersion`, `is_own_open_round`
/// pointer-compares the round, `close_on_deadline` re-reads `outcome` after
/// the actor returns. `submit` was the one blind overwrite left, and the
/// cost of it is that every N+1 follower not yet answered sees `Waiting`
/// against a cell that has gone backwards, and waits out `MAX_SYNC_WAIT_MS`
/// for an `UNKNOWN_SERVER_ERROR`.
#[tokio::test(start_paused = true)]
async fn a_late_submission_does_not_replace_a_newer_generations_assignment() {
    let fixture = fixture(&[]).await;
    let g = oqueue_core::GroupId::new("orders-consumers").expect("valid");
    let sync_groups = fixture.cluster.sync_groups();

    let second = sync_groups.submit(&g, 2, [("m1".to_owned(), b"gen2".to_vec())].into());
    assert!(second.is_some(), "the current generation's leader submits");

    let late = sync_groups.submit(&g, 1, [("m1".to_owned(), b"gen1".to_vec())].into());
    assert!(
        late.is_none(),
        "a submission for a generation the group has already left is refused, not applied"
    );

    let (assignments, _) = sync_groups.entry_for(&g);
    let still_there = super::super::assignment_for(&assignments, 2);
    assert!(
        matches!(&still_there, super::super::Known::Mine(map)
            if map.get("m1").map(Vec::as_slice) == Some(b"gen2".as_slice())),
        "generation 2's own map must survive the late submission, got {still_there:?}"
    );
}

/// ⚠️ **The same generation resubmitting is still allowed**, which is what
/// stops the guard being written as `>` and silently breaking the
/// leader-retry path `a_resubmitted_assignment_for_the_same_generation_
/// replaces_the_first` pins.
#[tokio::test(start_paused = true)]
async fn the_same_generation_may_still_resubmit() {
    let fixture = fixture(&[]).await;
    let g = oqueue_core::GroupId::new("orders-consumers").expect("valid");
    let sync_groups = fixture.cluster.sync_groups();

    assert!(
        sync_groups
            .submit(&g, 2, [("m1".to_owned(), b"first".to_vec())].into())
            .is_some()
    );
    let again = sync_groups.submit(&g, 2, [("m1".to_owned(), b"second".to_vec())].into());
    assert!(
        again.is_some_and(|map| map.get("m1").map(Vec::as_slice) == Some(b"second".as_slice())),
        "a leader resubmitting its own generation replaces its own map"
    );
}

/// ⚠️ **The client-visible half, which review found nothing executed.**
/// The guard above is pinned by [`a_late_submission_does_not_replace_a_newer_generations_assignment`],
/// but what the stale leader is *told* was not: planting a `panic!` in the
/// refusal arm left the whole suite green, so the code could have been any
/// of them — including `UNKNOWN_SERVER_ERROR`, which the Java consumer
/// raises out of `poll()` and never retries.
///
/// ⚠️ **The window is opened deterministically rather than waited for.**
/// `handle` reads the group's record under `fence`, then awaits the
/// transitions actor; polling it once parks it exactly there, and the
/// newer generation's submission lands while it is parked. This is
/// `join_group`'s own refusal-test idiom (`crate::testing::poll_once`),
/// which `M4.15d` made necessary by putting an `.await` between the
/// decision and its application.
#[tokio::test(start_paused = true)]
async fn a_leader_whose_generation_moved_on_while_it_was_parked_is_told_to_rejoin() {
    let fixture = fixture(&[]).await;
    let g = oqueue_core::GroupId::new("orders-consumers").expect("valid");
    seat(&fixture.cluster, "orders-consumers", &["m1", "m2"]);

    let body = leader_body_at(
        "orders-consumers",
        "m1",
        &[("m1", b"gen1-m1"), ("m2", b"gen1-m2")],
        1,
    );
    let submitting = sync(&fixture.cluster, body);
    tokio::pin!(submitting);
    assert!(
        crate::testing::poll_once(&mut submitting).is_none(),
        "the submission must park on the transitions actor rather than complete synchronously"
    );

    // The group moved on: generation 2's own leader got there first.
    assert!(
        fixture
            .cluster
            .sync_groups()
            .submit(&g, 2, [("m1".to_owned(), b"gen2-m1".to_vec())].into())
            .is_some()
    );

    let response = submitting.await;
    assert_eq!(
        response.error_code,
        oqueue_codec::error_codes::REBALANCE_IN_PROGRESS,
        "a leader whose generation moved on while it was parked must be told to rejoin -- the \
         same answer its own followers get from Known::Superseded"
    );

    let (assignments, _) = fixture.cluster.sync_groups().entry_for(&g);
    assert!(
        matches!(super::super::assignment_for(&assignments, 2), super::super::Known::Mine(map)
            if map.get("m1").map(Vec::as_slice) == Some(b"gen2-m1".as_slice())),
        "and generation 2's map is untouched"
    );
}
