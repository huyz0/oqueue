#![allow(clippy::expect_used)]

mod durability;

use super::state::RoundOutcome;
use super::{GroupJoins, JoinOutcome, RoundMember, effective_timeout_ms, mint_member_id};
use crate::group_transitions::GroupTransitions;
use oqueue_core::{
    FakeGroupCoordinator, FakeGroupMetadataLog, GroupCoordinator, GroupEvent, GroupId,
    GroupMetadataLog, GroupMetadataRecord, GroupState,
};
use std::sync::{Arc, OnceLock};
use std::time::Duration;

pub(super) fn group(name: &str) -> GroupId {
    GroupId::new(name).expect("valid")
}

pub(super) fn member(id: &str, protocols: &[&str]) -> RoundMember {
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

/// A live [`GroupTransitions`] actor over a fresh coordinator and log.
///
/// ⚠️ **Every test needs one since `M4.15d`**, because the round's own
/// transitions no longer touch the coordinator directly — they are enqueued
/// to this task, which validates, durably appends, and only then applies.
/// A test that built `GroupJoins` alone would hang on the first join: the
/// actor is the only thing that ever answers.
pub(super) struct Harness {
    joins: GroupJoins,
    heartbeats: crate::heartbeat::Heartbeats,
    transitions: GroupTransitions,
    coordinator: Arc<FakeGroupCoordinator>,
    log: Arc<FakeGroupMetadataLog>,
    _task: tokio::task::JoinHandle<()>,
}

impl Harness {
    pub(super) fn new() -> Self {
        let coordinator = Arc::new(FakeGroupCoordinator::new());
        let log = Arc::new(FakeGroupMetadataLog::new());
        let (transitions, task) = GroupTransitions::new();
        let serving = tokio::spawn(task.serve(
            Arc::clone(&coordinator) as Arc<dyn GroupCoordinator>,
            Arc::clone(&log) as Arc<dyn GroupMetadataLog>,
        ));
        Self {
            joins: GroupJoins::default(),
            heartbeats: crate::heartbeat::Heartbeats::default(),
            transitions,
            coordinator,
            log,
            _task: serving,
        }
    }

    fn coordination(&self) -> super::Coordination<'_> {
        super::Coordination {
            transitions: &self.transitions,
            coordinator: self.coordinator.as_ref(),
            heartbeats: &self.heartbeats,
        }
    }

    /// ⚠️ **Registers the member so a group looks established.** Since
    /// `M4.16` a round waits only for members the group still *has*, which it
    /// reads from `Heartbeats` — so a harness that never registered anyone
    /// would leave every group looking brand new and no round would ever close
    /// early. ⚠️ **Not the same as what `join_group::handle` does**, and the
    /// difference is worth knowing: the handler registers only after a
    /// successful close, never on a refusal, and with the request's own
    /// session timeout. This registers unconditionally with a fixed one — so a
    /// member refused here becomes tracked, where in production it would not.
    /// Round-closure behaviour these tests show for a group that has refused a
    /// member does not transfer. Found by review.
    pub(super) async fn join(&self, g: &GroupId, m: RoundMember, timeout: Duration) -> JoinOutcome {
        let member_id = m.member_id.clone();
        let outcome = self.joins.join(self.coordination(), g, m, timeout).await;
        self.heartbeats.register(g, &member_id, 30_000);
        outcome
    }

    pub(super) async fn close_on_deadline(&self, g: &GroupId, own: &Arc<OnceLock<RoundOutcome>>) {
        self.joins
            .close_on_deadline(self.coordination(), g, own)
            .await;
    }

    pub(super) fn state(&self, g: &GroupId) -> Option<GroupState> {
        self.coordinator.record(g).map(|r| r.state)
    }

    /// Every event this group has durably recorded, in log order — the
    /// property `M4.15d` exists to create.
    pub(super) async fn durable_events(&self, g: &GroupId) -> Vec<GroupEvent> {
        let entries = self
            .log
            .read_from(oqueue_core::CommitVersion::ZERO, 1024)
            .await
            .expect("the fake log never fails");
        entries
            .iter()
            .filter_map(|e| match e.record() {
                GroupMetadataRecord::GroupTransitioned { group, event } if group == g => {
                    Some(*event)
                }
                _ => None,
            })
            .collect()
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
    let h = Harness::new();
    let g = group("orders");

    let JoinOutcome::Pending { outcome, .. } = h
        .join(&g, member("m1", &["range"]), Duration::from_secs(1))
        .await
    else {
        panic!("a fresh group's first-ever round has no early-close signal");
    };
    assert_eq!(h.state(&g), Some(GroupState::PreparingRebalance));

    h.close_on_deadline(&g, &outcome).await;
    assert_eq!(h.state(&g), Some(GroupState::CompletingRebalance));
}

#[tokio::test(start_paused = true)]
async fn every_member_of_the_same_round_shares_one_outcome() {
    let h = Harness::new();
    let g = group("orders");
    let timeout = Duration::from_secs(1);

    let JoinOutcome::Pending {
        outcome: outcome_a, ..
    } = h.join(&g, member("a", &["range"]), timeout).await
    else {
        panic!("expected pending");
    };
    let JoinOutcome::Pending {
        outcome: outcome_b, ..
    } = h.join(&g, member("b", &["range"]), timeout).await
    else {
        panic!("expected pending -- the second member joins the same open round");
    };

    h.close_on_deadline(&g, &outcome_a).await;

    let close_a = outcome_a
        .get()
        .expect("published")
        .as_ref()
        .expect("closed, not abandoned");
    let close_b = outcome_b
        .get()
        .expect("published")
        .as_ref()
        .expect("closed, not abandoned");
    assert!(Arc::ptr_eq(close_a, close_b), "one shared close, not two");
    assert_eq!(close_a.members.len(), 2);
}

#[tokio::test(start_paused = true)]
async fn a_member_sharing_no_protocol_with_the_round_is_refused_and_never_enrolled() {
    let h = Harness::new();
    let g = group("orders");
    let timeout = Duration::from_secs(1);

    let JoinOutcome::Pending { outcome, .. } = h.join(&g, member("a", &["range"]), timeout).await
    else {
        panic!("expected pending");
    };
    let refused = h.join(&g, member("b", &["sticky"]), timeout).await;
    assert!(matches!(refused, JoinOutcome::Refused));

    h.close_on_deadline(&g, &outcome).await;
    // Round 1 closed with exactly one member ("a" alone -- "b" was refused
    // and never enrolled), so round 2's own `expected` is `Some(1)`: "a"
    // rejoining (state is `CompletingRebalance`, so this is
    // `MemberJoinedDuringSync`) closes it immediately, alone.
    let JoinOutcome::Ready(close) = h.join(&g, member("a", &["range"]), timeout).await else {
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
    let h = Harness::new();
    let g = group("orders");
    let timeout = Duration::from_mins(1);

    // Round 1: two members, closed at the deadline (a fresh group's own
    // round has no early-close signal).
    h.join(&g, member("a", &["range"]), timeout).await;
    let JoinOutcome::Pending { outcome, .. } = h.join(&g, member("b", &["range"]), timeout).await
    else {
        panic!("expected pending -- the second member joins the same open round");
    };
    h.close_on_deadline(&g, &outcome).await;
    assert_eq!(h.state(&g), Some(GroupState::CompletingRebalance));

    // The sync phase this milestone does not build a handler for yet
    // (`M4.8`) -- driven directly against the coordinator, `M4.2`'s own
    // fake, to reach `Stable` the way a real `SyncGroup` eventually will.
    h.coordinator
        .transition(&g, GroupEvent::SyncComplete)
        .expect("CompletingRebalance -> Stable is legal");

    // Round 2: the same two members rejoin. The first alone must not close
    // it -- `expected` is now `Some(2)`, the previous round's own size.
    let pending = h.join(&g, member("a", &["range"]), timeout).await;
    assert!(
        matches!(pending, JoinOutcome::Pending { .. }),
        "one of two expected members must not close the round"
    );
    let ready = h.join(&g, member("b", &["range"]), timeout).await;
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
///
/// ⚠️ **"b" enrols first in round two and is still not its leader**, which
/// is what makes this test sharper than it used to be. A round's members
/// are ordered by the previous roster (`finalize_close`), so leadership
/// follows *group* join order the way Kafka's does, not the order of the
/// race to enrol in one round. This test asserted the opposite until
/// `M4.17` found what that cost: the member that opens a round enrols
/// first, so the newest consumer became leader of every group it joined,
/// its assignor had no memory of the previous assignment, and the consumer
/// that had just joined was assigned nothing.
#[tokio::test(start_paused = true)]
async fn a_closed_rounds_own_protocol_type_is_the_leaders_own() {
    let h = Harness::new();
    let g = group("orders");
    let timeout = Duration::from_mins(1);

    // Round 1: two members, so round 2's own `expected` is `Some(2)` --
    // needed so round 2 stays open long enough for both of its own members
    // to be distinguishable by join order.
    h.join(&g, member("a", &["range"]), timeout).await;
    let JoinOutcome::Pending { outcome, .. } = h.join(&g, member("b", &["range"]), timeout).await
    else {
        panic!("expected pending -- the second member joins the same open round");
    };
    h.close_on_deadline(&g, &outcome).await;
    h.coordinator
        .transition(&g, GroupEvent::SyncComplete)
        .expect("CompletingRebalance -> Stable is legal");

    // Round 2: "b" enrols first this time, and is deliberately *not* the
    // leader -- round 1's roster was ["a", "b"], so "a" keeps the lead.
    h.join(&g, member_with_type("b", "typeB", &["range"]), timeout)
        .await;
    let JoinOutcome::Ready(close) = h
        .join(&g, member_with_type("a", "typeA", &["range"]), timeout)
        .await
    else {
        panic!("round 2's own expected size (2) is met by this second join");
    };
    assert_eq!(
        close.leader, "a",
        "the incumbent leader keeps the lead; enrolling first does not win it"
    );
    assert_eq!(
        close.protocol_type, "typeA",
        "the leader's (\"a\"'s) own protocol_type, not the first enroller's (\"b\"'s)"
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
    let h = Harness::new();
    let g = group("orders");
    let timeout = Duration::from_mins(1);

    // Round 1: two members, so round 2's own `expected` is `Some(2)` --
    // needed so round 2 stays open once only one of its own two expected
    // members has (re)joined.
    h.join(&g, member("a", &["range"]), timeout).await;
    let JoinOutcome::Pending {
        outcome: stale_outcome,
        ..
    } = h.join(&g, member("b", &["range"]), timeout).await
    else {
        panic!("expected pending -- the second member joins the same open round");
    };
    h.close_on_deadline(&g, &stale_outcome).await;
    h.coordinator
        .transition(&g, GroupEvent::SyncComplete)
        .expect("CompletingRebalance -> Stable is legal");

    // Round 2: one of its own two expected members rejoins -- still open,
    // short of `expected`.
    let JoinOutcome::Pending {
        outcome: round_2_outcome,
        ..
    } = h.join(&g, member("a", &["range"]), timeout).await
    else {
        panic!("one of two expected members must not close the round");
    };

    // The attack this test exists to rule out: a round-1 waiter's own
    // (stale, already-closed) outcome handle reaches `close_on_deadline`
    // while round 2 is open. It must be a no-op.
    h.close_on_deadline(&g, &stale_outcome).await;
    assert!(
        round_2_outcome.get().is_none(),
        "round 2 must still be open -- a stale round-1 deadline closed it"
    );
    assert_eq!(
        h.state(&g),
        Some(GroupState::PreparingRebalance),
        "round 2 must still be preparing, not already completing"
    );

    // Round 2's own legitimate close still works afterward, on its own
    // (correct) membership -- the stale call did not corrupt it.
    h.close_on_deadline(&g, &round_2_outcome).await;
    let close = round_2_outcome
        .get()
        .expect("published")
        .as_ref()
        .expect("round 2 now closes for real");
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
    let h = Harness::new();
    let g = group("orders");

    // The shape `heartbeat.rs`'s own sweep produces: the coordinator moves
    // to PreparingRebalance directly (here, `Empty -> Join`, the same
    // transition a `Stable` group's own partial eviction fires), with no
    // call through `GroupJoins::join` and so no locally-open round.
    h.coordinator
        .transition(&g, GroupEvent::Join)
        .expect("Empty -> Join is legal");

    let outcome = h
        .join(&g, member("survivor", &["range"]), Duration::from_secs(1))
        .await;
    assert!(
        matches!(outcome, JoinOutcome::Pending { .. } | JoinOutcome::Ready(_)),
        "a PreparingRebalance group with no locally-open round must still accept a join, not refuse one forever"
    );
}
