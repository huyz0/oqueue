#![allow(clippy::expect_used)]

use super::{GroupJoins, JoinOutcome, RoundMember, effective_timeout_ms, mint_member_id};
use oqueue_core::{FakeGroupCoordinator, GroupCoordinator, GroupEvent, GroupId, GroupState};
use std::time::Duration;

fn group(name: &str) -> GroupId {
    GroupId::new(name).expect("valid")
}

fn member(id: &str, protocols: &[&str]) -> RoundMember {
    member_with_type(id, "consumer", protocols)
}

fn member_with_type(id: &str, protocol_type: &str, protocols: &[&str]) -> RoundMember {
    RoundMember {
        member_id: id.to_owned(),
        protocol_type: protocol_type.to_owned(),
        protocols: protocols
            .iter()
            .map(|&p| (p.to_owned(), Vec::new()))
            .collect(),
    }
}

#[test]
fn mint_member_id_never_collides_across_calls() {
    let a = mint_member_id();
    let b = mint_member_id();
    assert_ne!(a, b);
}

#[test]
fn effective_timeout_falls_back_to_the_session_timeout_below_v1() {
    assert_eq!(effective_timeout_ms(0, 30_000), 30_000);
    assert_eq!(effective_timeout_ms(60_000, 30_000), 60_000);
}

#[tokio::test(start_paused = true)]
async fn a_fresh_groups_first_member_is_pending_until_its_own_deadline() {
    let coordinator = FakeGroupCoordinator::new();
    let joins = GroupJoins::default();
    let g = group("orders");

    let JoinOutcome::Pending { outcome, .. } = joins.join(
        &coordinator,
        &g,
        member("m1", &["range"]),
        Duration::from_secs(1),
    ) else {
        panic!("a fresh group's first-ever round has no early-close signal");
    };
    assert_eq!(
        coordinator.record(&g).map(|r| r.state),
        Some(GroupState::PreparingRebalance)
    );

    joins.close_on_deadline(&coordinator, &g, &outcome);
    assert_eq!(
        coordinator.record(&g).map(|r| r.state),
        Some(GroupState::CompletingRebalance)
    );
}

#[tokio::test(start_paused = true)]
async fn every_member_of_the_same_round_shares_one_outcome() {
    let coordinator = FakeGroupCoordinator::new();
    let joins = GroupJoins::default();
    let g = group("orders");
    let timeout = Duration::from_secs(1);

    let JoinOutcome::Pending {
        outcome: outcome_a, ..
    } = joins.join(&coordinator, &g, member("a", &["range"]), timeout)
    else {
        panic!("expected pending");
    };
    let JoinOutcome::Pending {
        outcome: outcome_b, ..
    } = joins.join(&coordinator, &g, member("b", &["range"]), timeout)
    else {
        panic!("expected pending -- the second member joins the same open round");
    };

    joins.close_on_deadline(&coordinator, &g, &outcome_a);

    let close_a = outcome_a.get().expect("closed");
    let close_b = outcome_b.get().expect("closed");
    assert!(
        std::sync::Arc::ptr_eq(close_a, close_b),
        "one shared close, not two"
    );
    assert_eq!(close_a.members.len(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_member_sharing_no_protocol_with_the_round_is_refused_and_never_enrolled() {
    let coordinator = FakeGroupCoordinator::new();
    let joins = GroupJoins::default();
    let g = group("orders");
    let timeout = Duration::from_secs(1);

    let JoinOutcome::Pending { outcome, .. } =
        joins.join(&coordinator, &g, member("a", &["range"]), timeout)
    else {
        panic!("expected pending");
    };
    let refused = joins.join(&coordinator, &g, member("b", &["sticky"]), timeout);
    assert!(matches!(refused, JoinOutcome::Refused));

    joins.close_on_deadline(&coordinator, &g, &outcome);
    // Round 1 closed with exactly one member ("a" alone -- "b" was refused
    // and never enrolled), so round 2's own `expected` is `Some(1)`: "a"
    // rejoining (state is `CompletingRebalance`, so this is
    // `MemberJoinedDuringSync`) closes it immediately, alone.
    let JoinOutcome::Ready(close) = joins.join(&coordinator, &g, member("a", &["range"]), timeout)
    else {
        panic!("round 2's own expected size (1) is met by \"a\" alone");
    };
    assert_eq!(
        close.members.len(),
        1,
        "\"b\" was never enrolled, in either round"
    );
}

#[tokio::test(start_paused = true)]
async fn an_established_groups_next_round_closes_early_once_its_prior_size_rejoins() {
    let coordinator = FakeGroupCoordinator::new();
    let joins = GroupJoins::default();
    let g = group("orders");
    let timeout = Duration::from_mins(1);

    // Round 1: two members, closed at the deadline (a fresh group's own
    // round has no early-close signal).
    joins.join(&coordinator, &g, member("a", &["range"]), timeout);
    let JoinOutcome::Pending { outcome, .. } =
        joins.join(&coordinator, &g, member("b", &["range"]), timeout)
    else {
        panic!("expected pending -- the second member joins the same open round");
    };
    joins.close_on_deadline(&coordinator, &g, &outcome);
    assert_eq!(
        coordinator.record(&g).map(|r| r.state),
        Some(GroupState::CompletingRebalance)
    );

    // The sync phase this milestone does not build a handler for yet
    // (`M4.8`) -- driven directly against the coordinator, `M4.2`'s own
    // fake, to reach `Stable` the way a real `SyncGroup` eventually will.
    coordinator
        .transition(&g, GroupEvent::SyncComplete)
        .expect("CompletingRebalance -> Stable is legal");

    // Round 2: the same two members rejoin. The first alone must not close
    // it -- `expected` is now `Some(2)`, the previous round's own size.
    let pending = joins.join(&coordinator, &g, member("a", &["range"]), timeout);
    assert!(
        matches!(pending, JoinOutcome::Pending { .. }),
        "one of two expected members must not close the round"
    );
    let ready = joins.join(&coordinator, &g, member("b", &["range"]), timeout);
    assert!(
        matches!(ready, JoinOutcome::Ready(_)),
        "the second of two expected members closes it without waiting for the deadline"
    );
}

/// A closed round's own `protocol_type` is the *leader's* own value, never
/// a follower's — `close_locked`'s own `find(|m| m.member_id == leader)`.
/// Every member in a real group advertises the same protocol family, so
/// this gives each one a distinct one specifically to make picking the
/// wrong member's value a visible, assertable difference.
#[tokio::test(start_paused = true)]
async fn a_closed_rounds_own_protocol_type_is_the_leaders_own() {
    let coordinator = FakeGroupCoordinator::new();
    let joins = GroupJoins::default();
    let g = group("orders");
    let timeout = Duration::from_mins(1);

    // Round 1: two members, so round 2's own `expected` is `Some(2)` --
    // needed so round 2 stays open long enough for both of its own members
    // to be distinguishable by join order.
    joins.join(&coordinator, &g, member("a", &["range"]), timeout);
    let JoinOutcome::Pending { outcome, .. } =
        joins.join(&coordinator, &g, member("b", &["range"]), timeout)
    else {
        panic!("expected pending -- the second member joins the same open round");
    };
    joins.close_on_deadline(&coordinator, &g, &outcome);
    coordinator
        .transition(&g, GroupEvent::SyncComplete)
        .expect("CompletingRebalance -> Stable is legal");

    // Round 2: "b" joins first this time, so "b" -- not "a" -- is this
    // round's own leader.
    joins.join(
        &coordinator,
        &g,
        member_with_type("b", "typeB", &["range"]),
        timeout,
    );
    let JoinOutcome::Ready(close) = joins.join(
        &coordinator,
        &g,
        member_with_type("a", "typeA", &["range"]),
        timeout,
    ) else {
        panic!("round 2's own expected size (2) is met by this second join");
    };
    assert_eq!(close.leader, "b");
    assert_eq!(
        close.protocol_type, "typeB",
        "the leader's (\"b\"'s) own protocol_type, not the follower's (\"a\"'s)"
    );
}

/// ⚠️ **A stale waiter's own deadline must never close a *different, later*
/// round for the same group.** `entry.open` is replaced by `GroupId` alone
/// each time a round opens, so a waiter that missed the `Notify` for its
/// own round's close (`tokio::sync::Notify::notify_waiters` wakes only
/// tasks already registered as waiters at that instant) would, on its own
/// stale deadline, otherwise call `close_on_deadline` against whatever
/// round the group has moved on to — closing it early, on the wrong
/// membership, and bumping its generation on nobody's authority. This test
/// reproduces the shape directly: a round-1 waiter's own (already-closed)
/// `outcome` handle is handed to `close_on_deadline` while round 2 is still
/// genuinely open and short of its own `expected` count.
#[tokio::test(start_paused = true)]
async fn a_stale_rounds_own_deadline_never_closes_a_later_round() {
    let coordinator = FakeGroupCoordinator::new();
    let joins = GroupJoins::default();
    let g = group("orders");
    let timeout = Duration::from_mins(1);

    // Round 1: two members, so round 2's own `expected` is `Some(2)` --
    // needed so round 2 stays open once only one of its own two expected
    // members has (re)joined.
    joins.join(&coordinator, &g, member("a", &["range"]), timeout);
    let JoinOutcome::Pending {
        outcome: stale_outcome,
        ..
    } = joins.join(&coordinator, &g, member("b", &["range"]), timeout)
    else {
        panic!("expected pending -- the second member joins the same open round");
    };
    joins.close_on_deadline(&coordinator, &g, &stale_outcome);
    coordinator
        .transition(&g, GroupEvent::SyncComplete)
        .expect("CompletingRebalance -> Stable is legal");

    // Round 2: one of its own two expected members rejoins -- still open,
    // short of `expected`.
    let JoinOutcome::Pending {
        outcome: round_2_outcome,
        ..
    } = joins.join(&coordinator, &g, member("a", &["range"]), timeout)
    else {
        panic!("one of two expected members must not close the round");
    };

    // The attack this test exists to rule out: a round-1 waiter's own
    // (stale, already-closed) outcome handle reaches `close_on_deadline`
    // while round 2 is open. It must be a no-op.
    joins.close_on_deadline(&coordinator, &g, &stale_outcome);
    assert!(
        round_2_outcome.get().is_none(),
        "round 2 must still be open -- a stale round-1 deadline closed it"
    );
    assert_eq!(
        coordinator.record(&g).map(|r| r.state),
        Some(GroupState::PreparingRebalance),
        "round 2 must still be preparing, not already completing"
    );

    // Round 2's own legitimate close still works afterward, on its own
    // (correct) membership -- the stale call did not corrupt it.
    joins.close_on_deadline(&coordinator, &g, &round_2_outcome);
    let close = round_2_outcome.get().expect("round 2 now closes for real");
    assert_eq!(close.members.len(), 1, "only \"a\" ever joined round 2");
}

/// ⚠️ **A group an *external* actor moved to `PreparingRebalance` is not a
/// permanent bug.** `M4.9`'s own finding: `heartbeat.rs`'s eviction sweep
/// fires `GroupEvent::Join` directly against the coordinator, without ever
/// opening a round here — so `entry.open` genuinely is `None` the first
/// time a real client's own `JoinGroup` reaches this module afterward,
/// even though the coordinator already reports `PreparingRebalance`. This
/// must start collecting rather than refuse, or the group is wedged
/// forever (nothing else can ever fire `JoinBarrierComplete` for a round
/// this module never opened).
#[tokio::test(start_paused = true)]
async fn a_group_moved_to_preparing_rebalance_by_an_external_actor_still_accepts_a_join() {
    let coordinator = FakeGroupCoordinator::new();
    let joins = GroupJoins::default();
    let g = group("orders");

    // The shape `heartbeat.rs`'s own sweep produces: the coordinator moves
    // to PreparingRebalance directly (here, `Empty -> Join`, the same
    // transition a `Stable` group's own partial eviction fires), with no
    // call through `GroupJoins::join` and so no locally-open round.
    coordinator
        .transition(&g, GroupEvent::Join)
        .expect("Empty -> Join is legal");

    let outcome = joins.join(
        &coordinator,
        &g,
        member("survivor", &["range"]),
        Duration::from_secs(1),
    );
    assert!(
        matches!(outcome, JoinOutcome::Pending { .. } | JoinOutcome::Ready(_)),
        "a PreparingRebalance group with no locally-open round must still accept a join, not refuse one forever"
    );
}
