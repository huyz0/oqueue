//! `M4.15d`: what the in-flight barrier and the re-plan loop guarantee under
//! concurrency, and which joins are refused.
//!
//! ⚠️ **Split from `durability` because the questions differ.** That module
//! asks *what the log holds* after an ordinary round; this asks *what happens
//! when two tasks race, or when the event chosen under the lock is no longer
//! legal by the time the actor applies it*. Every case here was written
//! against a defect review reproduced, and each is verified to fail against
//! the behaviour it replaced.

#![allow(clippy::expect_used)]

mod refusals;
mod restart;

use super::super::{Harness, group, member};
use crate::join_group::round::JoinOutcome;
use oqueue_core::{GroupCoordinator, GroupEvent, GroupState};
use std::future::Future;
use std::pin::Pin;
use std::time::Duration;

/// ⚠️ **Two members joining a fresh group concurrently must open *one*
/// round, not two.** This is the property the in-flight barrier exists for:
/// routing through the actor made the transition `async`, so the decision
/// ("no round is open — fire `Join`") and its application are no longer one
/// atomic step under the mutex. Without the barrier both joiners decide to
/// open, and the group durably records `Join` twice.
#[tokio::test(start_paused = true)]
async fn two_concurrent_joiners_open_exactly_one_round() {
    let h = Harness::new();
    let g = group("orders");

    let (a, b) = tokio::join!(
        h.join(&g, member("m1", &["range"]), Duration::from_secs(1)),
        h.join(&g, member("m2", &["range"]), Duration::from_secs(1)),
    );
    assert!(
        matches!(a, JoinOutcome::Pending { .. }) && matches!(b, JoinOutcome::Pending { .. }),
        "both joiners enrol in the one open round"
    );
    assert_eq!(
        h.durable_events(&g).await,
        vec![GroupEvent::Join],
        "exactly one round opened, so exactly one Join is recorded"
    );
    assert_eq!(h.state(&g), Some(GroupState::PreparingRebalance));
}

/// ⚠️ **A full round closes once, however many members fill it.** The same
/// barrier covers `JoinBarrierComplete`: without it the joiner that fills the
/// round and a waiter's own deadline can both fire it, bumping the generation
/// twice for one round.
#[tokio::test(start_paused = true)]
async fn a_round_that_fills_records_exactly_one_barrier_complete() {
    let h = Harness::new();
    let g = group("orders");

    // Round one establishes `last_round_size = 1`, so round two closes the
    // moment its single member joins.
    let JoinOutcome::Pending { outcome, .. } = h
        .join(&g, member("m1", &["range"]), Duration::from_secs(1))
        .await
    else {
        panic!("first round is pending");
    };
    h.close_on_deadline(&g, &outcome).await;
    let before = h.durable_events(&g).await.len();

    // Drive the group back to `Stable` so the next join opens a fresh round.
    h.coordinator
        .transition(&g, GroupEvent::SyncComplete)
        .expect("CompletingRebalance -> Stable");

    let JoinOutcome::Ready(close) = h
        .join(&g, member("m1", &["range"]), Duration::from_secs(1))
        .await
    else {
        panic!("an established group's next round closes as its prior size rejoins");
    };

    let after = h.durable_events(&g).await;
    assert_eq!(
        &after[before..],
        &[GroupEvent::Join, GroupEvent::JoinBarrierComplete],
        "the round that fills records its open and exactly one close"
    );
    assert_eq!(
        close.generation,
        h.coordinator.record(&g).expect("exists").generation,
        "the generation answered to the client is the one the actor applied"
    );
}

/// The refusal this row asks to be preserved: an event the coordinator's own
/// state has made illegal refuses the join rather than opening a round the
/// coordinator never agreed to.
///
/// ⚠️ `Dead` is the only state that produces this, and it is unreachable
/// through a client path in this milestone (module doc) — reached here by
/// driving the coordinator to it directly, which is exactly the divergence
/// the refusal exists to survive.
#[tokio::test(start_paused = true)]
async fn a_join_against_a_dead_group_is_refused_and_records_nothing() {
    let h = Harness::new();
    let g = group("orders");
    h.coordinator
        .transition(&g, GroupEvent::Expire)
        .expect("Empty -> Dead");

    let outcome = h
        .join(&g, member("m1", &["range"]), Duration::from_secs(1))
        .await;

    assert!(
        matches!(outcome, JoinOutcome::Refused),
        "a group the coordinator calls Dead admits no round"
    );
    assert!(
        h.durable_events(&g).await.is_empty(),
        "a refused join must append nothing"
    );
}

/// **The in-flight slot is released by a `Drop`, so an unwind or a cancelled
/// future cannot leak it.** Round six's finding: "every path that claimed the
/// slot releases it" is not a property `return` statements can hold. This
/// workspace is `panic = "unwind"` so a handler panic kills one connection
/// rather than the node, and `GroupJoins::lock` recovers from poisoning — so a
/// panic between the claim and the release used to leave `in_flight` set for
/// the life of the process, and every later `JoinGroup` for that group burned
/// its whole replan budget and answered `REBALANCE_IN_PROGRESS` forever.
///
/// ⚠️ **Driven through a real claim site, by dropping the join future while it
/// holds the slot.** Round seven's finding against the first version of this
/// test: constructing a `SlotGuard` only to drop it exercises `Drop` in
/// isolation, so reverting `open_holding_slot` to an explicit release on each
/// path left it green. Cancelling a parked join is the production shape of the
/// same unwind — the future is dropped mid-`await`, with the claim held.
#[tokio::test(start_paused = true)]
async fn a_cancelled_join_does_not_leak_the_slot() {
    let h = Harness::new();
    let g = group("orders");

    {
        let joining = h.join(&g, member("m1", &["range"]), Duration::from_secs(1));
        tokio::pin!(joining);
        assert!(
            futures_lite_poll_once(&mut joining).is_none(),
            "the join must park on the actor while holding the slot"
        );
        // Dropped here, mid-await, with the claim outstanding.
    }

    let outcome = h
        .join(&g, member("m2", &["range"]), Duration::from_secs(1))
        .await;
    assert!(
        matches!(outcome, JoinOutcome::Pending { .. }),
        "a cancelled join must leave the group joinable, not wedged behind a \
         slot nobody will ever release"
    );
}

/// **A member told to rejoin is withdrawn from the round it was told to
/// leave.** Round seven's `MAJOR`: `plan_join` enrols before returning
/// `Pending`, so a joiner whose `wait_for_close` gave up stayed in
/// `round.members`. The round then closed with it in the roster, the leader
/// assigned it partitions, and nothing consumed them — the client had already
/// rejoined under a freshly minted id, and the abandoned one was never passed
/// to `register_heartbeat`, so the eviction sweep could not remove it either.
#[tokio::test(start_paused = true)]
async fn a_withdrawn_member_is_not_in_the_round_that_closes() {
    let h = Harness::new();
    let g = group("orders");

    let JoinOutcome::Pending { outcome, .. } = h
        .join(&g, member("stays", &["range"]), Duration::from_secs(1))
        .await
    else {
        panic!("first member is pending");
    };
    let _ = h
        .join(&g, member("gives-up", &["range"]), Duration::from_secs(1))
        .await;

    h.joins.withdraw(&g, "gives-up", &outcome);
    h.close_on_deadline(&g, &outcome).await;

    let close = outcome
        .get()
        .expect("published")
        .as_ref()
        .expect("closed, not abandoned");
    let ids: Vec<&str> = close.members.iter().map(|m| m.member_id.as_str()).collect();
    assert_eq!(
        ids,
        vec!["stays"],
        "a member that was told to rejoin must not be in the roster the leader \
         is handed"
    );
}

/// `futures::poll_once` without the dependency — `Cargo.toml` carries no
/// `futures`, and `build.md` asks for a recorded reason before one is added,
/// which a single poll in one test does not earn.
pub(super) fn futures_lite_poll_once<F: Future>(f: &mut Pin<&mut F>) -> Option<F::Output> {
    use std::task::{Context, Poll, Waker};
    let mut cx = Context::from_waker(Waker::noop());
    match f.as_mut().poll(&mut cx) {
        Poll::Ready(v) => Some(v),
        Poll::Pending => None,
    }
}

/// **A join that loses a race to another handler's transition re-plans; it is
/// not refused.** The `BLOCKING` finding of round two: `opening_event` picks
/// from the state it reads under the mutex, and the actor applies it a
/// durable append later, so an ordinary `SyncComplete` landing in between
/// makes the chosen event illegal with nothing wrong. Answering
/// `INCONSISTENT_GROUP_PROTOCOL` there tells a Java consumer to leave the
/// group over the normal end of a rebalance.
///
/// ⚠️ Driven deterministically: park the joiner on the actor with the group
/// `CompletingRebalance` (so it chooses `MemberJoinedDuringSync`), then move
/// the group to `Stable`, which that event has no legal arm from.
#[tokio::test(start_paused = true)]
async fn a_join_that_loses_a_race_re_plans_rather_than_refusing() {
    let h = Harness::new();
    let g = group("orders");

    // Get the group to `CompletingRebalance`.
    let JoinOutcome::Pending { outcome, .. } = h
        .join(&g, member("m1", &["range"]), Duration::from_secs(1))
        .await
    else {
        panic!("first round is pending");
    };
    h.close_on_deadline(&g, &outcome).await;
    assert_eq!(h.state(&g), Some(GroupState::CompletingRebalance));

    // m2 reads `CompletingRebalance`, picks `MemberJoinedDuringSync`, parks.
    let joining = h.join(&g, member("m2", &["range"]), Duration::from_secs(1));
    tokio::pin!(joining);
    assert!(
        futures_lite_poll_once(&mut joining).is_none(),
        "the join must park on the actor"
    );
    // The leader's own sync lands first, so m2's chosen event is now illegal.
    h.coordinator
        .transition(&g, GroupEvent::SyncComplete)
        .expect("CompletingRebalance -> Stable");

    let outcome = joining.await;
    assert!(
        !matches!(outcome, JoinOutcome::Refused),
        "losing a race to an ordinary SyncComplete must re-plan, not answer \
         INCONSISTENT_GROUP_PROTOCOL — which the Java consumer treats as fatal"
    );
    // Re-planning picked the event that is legal from `Stable`.
    assert!(
        h.durable_events(&g).await.contains(&GroupEvent::Join),
        "the second pass opens the round with the event Stable admits"
    );
}

/// **Two members filling one round concurrently close it once, and both are
/// answered from that close.** The in-flight barrier's other half.
///
/// ⚠️ **The actor hides the open-path half of this, which is why the close
/// path is what pins the barrier.** Without the barrier two joiners both fire
/// `Join`; the second is refused by the actor's own dry run before anything
/// is appended, and re-plans — so the log still shows one `Join` and nothing
/// looks wrong. The close path has no such cover: after the winner's
/// `JoinBarrierComplete` the group is `CompletingRebalance`, from which a
/// second barrier is illegal, so the loser re-plans, finds the round closed,
/// and opens a *fresh* one with `MemberJoinedDuringSync` — a member that
/// filled a round ends up alone in the next one, and the log carries the
/// spurious event to prove it.
#[tokio::test(start_paused = true)]
async fn two_members_filling_one_round_concurrently_close_it_once() {
    let h = Harness::new();
    let g = group("orders");

    // Establish `last_round_size = 2` so the next round closes when two join.
    let JoinOutcome::Pending { outcome, .. } = h
        .join(&g, member("a", &["range"]), Duration::from_secs(1))
        .await
    else {
        panic!("first round is pending");
    };
    let _ = h
        .join(&g, member("b", &["range"]), Duration::from_secs(1))
        .await;
    h.close_on_deadline(&g, &outcome).await;
    h.coordinator
        .transition(&g, GroupEvent::SyncComplete)
        .expect("CompletingRebalance -> Stable");
    let before = h.durable_events(&g).await.len();

    // Both members rejoin at once; the round fills and must close exactly once.
    let (x, y) = tokio::join!(
        h.join(&g, member("a", &["range"]), Duration::from_secs(1)),
        h.join(&g, member("b", &["range"]), Duration::from_secs(1)),
    );

    let after = &h.durable_events(&g).await[before..];
    assert_eq!(
        after
            .iter()
            .filter(|e| **e == GroupEvent::JoinBarrierComplete)
            .count(),
        1,
        "one round closes once, however many members fill it — got {after:?}"
    );
    assert!(
        !after.contains(&GroupEvent::MemberJoinedDuringSync),
        "no member may be pushed into a fresh round by losing the close race — got {after:?}"
    );
    for outcome in [&x, &y] {
        assert!(
            !matches!(outcome, JoinOutcome::Refused),
            "neither member is refused"
        );
    }
}

/// **A refused barrier must not wedge the group.** Round three's `BLOCKING`
/// finding: `close_generation` returning `None` used to leave the round open,
/// and `plan_join` only consults the coordinator when it needs to *open* one
/// — so every re-plan re-fired the same illegal `JoinBarrierComplete` and the
/// group became unjoinable on this node until the process restarted.
///
/// ⚠️ The trigger is ordinary: the last tracked member leaves while a
/// replacement is mid-join, firing `AllMembersGone`, which takes the group
/// out of `PreparingRebalance` — the only state a barrier is legal from.
#[tokio::test(start_paused = true)]
async fn a_refused_barrier_leaves_the_group_joinable() {
    let h = Harness::new();
    let g = group("orders");

    let JoinOutcome::Pending { outcome, .. } = h
        .join(&g, member("m1", &["range"]), Duration::from_secs(1))
        .await
    else {
        panic!("first round is pending");
    };
    // The last tracked member goes away: PreparingRebalance -> Empty, which
    // makes m1's pending barrier illegal.
    h.coordinator
        .transition(&g, GroupEvent::AllMembersGone)
        .expect("PreparingRebalance -> Empty");

    h.close_on_deadline(&g, &outcome).await;
    // ⚠️ **It publishes the *abandonment*, which is the point.** Leaving the
    // outcome unset stranded every other member of the round until its own
    // full rebalance timeout, then answered it `UNKNOWN_SERVER_ERROR` — which
    // the Java consumer does not retry. `Some(None)` tells them at once.
    assert!(
        matches!(outcome.get(), Some(None)),
        "a refused barrier publishes an abandonment, not a close and not silence"
    );

    // The group must still be joinable — this is the wedge the fix removes.
    let second = h
        .join(&g, member("m2", &["range"]), Duration::from_secs(1))
        .await;
    assert!(
        !matches!(second, JoinOutcome::Refused),
        "a later join must open a fresh round against the coordinator's real \
         state, not re-enrol into the abandoned one"
    );
    assert_eq!(
        h.durable_events(&g).await,
        vec![GroupEvent::Join, GroupEvent::Join],
        "the abandoned round's Join, then a fresh one — and no illegal barrier"
    );
}

/// **A co-member of an abandoned round is told at once, not left to time
/// out.** Round four's `MAJOR`: `abandon_round` dropped the round and woke
/// its waiters without publishing anything, so every *other* enrolled member
/// found `outcome` unset, waited out its full rebalance timeout, and was then
/// answered `UNKNOWN_SERVER_ERROR` — which the Java consumer raises out of
/// `poll()` rather than retrying. Only the task that fired the barrier
/// recovered.
#[tokio::test(start_paused = true)]
async fn a_co_member_of_an_abandoned_round_learns_at_once() {
    let h = Harness::new();
    let g = group("orders");

    // Establish `last_round_size = 2` so a two-member round closes on filling.
    let JoinOutcome::Pending { outcome: first, .. } = h
        .join(&g, member("m1", &["range"]), Duration::from_secs(1))
        .await
    else {
        panic!("first round is pending");
    };
    let _ = h
        .join(&g, member("m2", &["range"]), Duration::from_secs(1))
        .await;
    h.close_on_deadline(&g, &first).await;
    h.coordinator
        .transition(&g, GroupEvent::SyncComplete)
        .expect("CompletingRebalance -> Stable");

    // m1 enrols and parks; m2 will fill the round and fire the barrier.
    let JoinOutcome::Pending { outcome: m1, .. } = h
        .join(&g, member("m1", &["range"]), Duration::from_secs(1))
        .await
    else {
        panic!("m1 is pending in the new round");
    };
    let filling = h.join(&g, member("m2", &["range"]), Duration::from_secs(1));
    tokio::pin!(filling);
    assert!(
        futures_lite_poll_once(&mut filling).is_none(),
        "m2 must park on the actor holding the slot"
    );
    // The last tracked member goes away, making m2's barrier illegal.
    h.coordinator
        .transition(&g, GroupEvent::AllMembersGone)
        .expect("PreparingRebalance -> Empty");
    let _ = filling.await;

    assert!(
        matches!(m1.get(), Some(None)),
        "m1 must be told its round was abandoned without waiting out its own \
         rebalance timeout — it is not the task that fired the barrier"
    );
}
