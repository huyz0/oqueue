//! `M4.15d`: which joins are refused, and with which wire code.
//!
//! ⚠️ **Split from `concurrency` because the question is the answer, not the
//! race.** Those cases ask whether two tasks can corrupt one round; these ask
//! what a *single* join is told when the event it chose is no longer legal, or
//! when the log underneath is broken. ⚠️ Every case here exists because a
//! review round found the code answering `INCONSISTENT_GROUP_PROTOCOL` — which
//! the Java consumer raises out of `poll()` and never retries — to something
//! that was not the client's fault. Three separate rounds found three
//! separate paths to it.

#![allow(clippy::expect_used)]

use super::super::super::{Harness, group, member};
use super::futures_lite_poll_once;
use crate::group_transitions::GroupTransitions;
use crate::join_group::round::{Coordination, GroupJoins, JoinOutcome};
use oqueue_core::{
    FakeGroupCoordinator, FakeGroupMetadataLog, FaultGroupMetadataLog, GroupCoordinator,
    GroupEvent, GroupMetadataLog,
};
use std::sync::Arc;
use std::time::Duration;

/// **The refusal this row actually names**, which the `Dead`-group case above
/// does not reach: the event was *legal when chosen* and the coordinator's
/// own state made it illegal before the actor applied it.
///
/// ⚠️ This is the window the restructuring opened. Under the old code the
/// decision and the transition were one step under one mutex, so no state
/// could change between them; routing through the actor put an `.await`
/// there, and the row asks for the refusal to survive it. Driving the group
/// to `Dead` while the joiner is parked on the actor is that window, made
/// deterministic.
///
/// ⚠️ **Which of the two guards answers is deliberately not asserted, because
/// measuring showed it is not the obvious one.** `apply_open` returning
/// `false` is the direct guard, but mutating it to return `true` leaves this
/// test green: the loop re-plans, `opening_event` reads `Dead` on the second
/// pass, and refuses there instead. So the refusal is defended twice and this
/// pins the outcome — refused, nothing appended — rather than the mechanism.
/// Claiming it pinned `apply_open`'s return would have been false; the
/// mutation is what showed that.
#[tokio::test(start_paused = true)]
async fn a_join_whose_event_turns_illegal_while_parked_is_refused() {
    let h = Harness::new();
    let g = group("orders");

    // `plan_join` reads `Empty` and chooses `Join`; the actor is what will
    // refuse it, because this lands first.
    let joining = h.join(&g, member("m1", &["range"]), Duration::from_secs(1));
    tokio::pin!(joining);
    // Let the join reach its own `.await` on the actor, then move the group
    // out from under it.
    assert!(
        futures_lite_poll_once(&mut joining).is_none(),
        "the join must park on the actor rather than complete synchronously"
    );
    h.coordinator
        .transition(&g, GroupEvent::Expire)
        .expect("Empty -> Dead");

    let outcome = joining.await;
    assert!(
        matches!(outcome, JoinOutcome::Refused),
        "an event the coordinator's state has since made illegal must refuse the join"
    );
    assert!(
        h.durable_events(&g).await.is_empty(),
        "a refused transition appends nothing"
    );
}

/// **A failing log answers `Unavailable`, not the fatal refusal.** Round
/// four's `BLOCKING`, and round five's `MAJOR` that nothing pinned it: a
/// storage outage used to be indistinguishable from a lost race, so it
/// re-planned eight times and then answered `INCONSISTENT_GROUP_PROTOCOL` —
/// which the Java consumer raises out of `poll()`, so every consumer in the
/// group dies and none returns when storage does.
#[tokio::test(start_paused = true)]
async fn a_failing_log_makes_a_join_unavailable_not_refused() {
    let g = group("orders");
    // ⚠️ Built inline rather than through `Harness`: this is the only test
    // that needs a log it can break, and making the shared harness generic
    // over its log to serve one case cost more than it bought.
    let coordinator = Arc::new(FakeGroupCoordinator::new());
    let log = Arc::new(FaultGroupMetadataLog::new(FakeGroupMetadataLog::new()));
    let (transitions, task) = GroupTransitions::new();
    let _serving = tokio::spawn(task.serve(
        Arc::clone(&coordinator) as Arc<dyn GroupCoordinator>,
        Arc::clone(&log) as Arc<dyn GroupMetadataLog>,
    ));
    let joins = GroupJoins::default();
    let heartbeats = crate::heartbeat::Heartbeats::default();
    let co = Coordination {
        transitions: &transitions,
        coordinator: coordinator.as_ref(),
        heartbeats: &heartbeats,
    };
    log.refuse_append();

    let outcome = joins
        .join(co, &g, member("m1", &["range"]), Duration::from_secs(1))
        .await;

    assert!(
        matches!(outcome, JoinOutcome::Unavailable),
        "a durable-append failure is a dependency problem, not a protocol one"
    );

    // It recovers: the same joiner succeeds once the log heals.
    log.heal();
    let outcome = joins
        .join(co, &g, member("m1", &["range"]), Duration::from_secs(1))
        .await;
    assert!(
        !matches!(outcome, JoinOutcome::Unavailable | JoinOutcome::Refused),
        "the group is joinable again once its log is"
    );
}

/// **Each refusal answers its own code, and only one of the three is fatal.**
/// Three review rounds each found a different non-client-fault condition being
/// answered `INCONSISTENT_GROUP_PROTOCOL`, which the Java consumer raises out
/// of `poll()` and never retries: a lost race, a broken log, and a contended
/// group. This pins the mapping so a fourth cannot appear by deleting an arm.
#[test]
fn every_refusal_answers_the_code_its_cause_deserves() {
    use crate::join_group::refusal_code;
    use oqueue_codec::error_codes;

    assert_eq!(
        refusal_code(&JoinOutcome::Unavailable),
        error_codes::COORDINATOR_NOT_AVAILABLE,
        "a dependency failure is retriable — the request was fine"
    );
    assert_eq!(
        refusal_code(&JoinOutcome::Busy),
        error_codes::REBALANCE_IN_PROGRESS,
        "a contended group asks the client to rejoin, it does not kill it"
    );
    assert_eq!(
        refusal_code(&JoinOutcome::Refused),
        error_codes::INCONSISTENT_GROUP_PROTOCOL,
        "the one refusal that really is about this request"
    );
}

/// **A transient append failure must not cost the group its round size.**
/// Round nine's finding: `abandon_round` cleared `last_round_members` for every
/// refused barrier. That is right when the barrier was *illegal* — the group
/// moved on and its members really are gone — and wrong when the append merely
/// failed, because then every member is sitting in the next round already. A
/// round with no roster to wait for has no early-close signal, so it waits out
/// its initial-rebalance delay instead — `M4.29` bounded that at 3 s, where it
/// used to be `rebalance_timeout_ms` in full, 300 s with either real client's
/// default. A storage blip that healed in milliseconds should still not cost
/// the group a fresh rebalance it did not need.
///
/// ⚠️ **The round is opened before the log is broken, on purpose.** Refusing
/// appends up front fails the *opening* transition too, so the join answers
/// `Unavailable` and the barrier path is never reached — the first version of
/// this test did exactly that and passed against the defect it was written to
/// catch. A two-member round lets the open succeed and only the barrier fail.
#[tokio::test(start_paused = true)]
async fn a_transient_append_failure_keeps_the_rounds_known_roster() {
    let g = group("orders");
    let coordinator = Arc::new(FakeGroupCoordinator::new());
    let log = Arc::new(FaultGroupMetadataLog::new(FakeGroupMetadataLog::new()));
    let (transitions, task) = GroupTransitions::new();
    let _serving = tokio::spawn(task.serve(
        Arc::clone(&coordinator) as Arc<dyn GroupCoordinator>,
        Arc::clone(&log) as Arc<dyn GroupMetadataLog>,
    ));
    let joins = GroupJoins::default();
    let heartbeats = crate::heartbeat::Heartbeats::default();
    let co = Coordination {
        transitions: &transitions,
        coordinator: coordinator.as_ref(),
        heartbeats: &heartbeats,
    };

    // Establish a round size of 2.
    let JoinOutcome::Pending { outcome, .. } = joins
        .join(co, &g, member("a", &["range"]), Duration::from_secs(1))
        .await
    else {
        panic!("first round is pending");
    };
    let _ = joins
        .join(co, &g, member("b", &["range"]), Duration::from_secs(1))
        .await;
    heartbeats.register(&g, "b", 30_000);
    joins.close_on_deadline(co, &g, &outcome).await;
    coordinator
        .transition(&g, GroupEvent::SyncComplete)
        .expect("CompletingRebalance -> Stable");

    // The next round opens cleanly, then its barrier's append fails.
    let _ = joins
        .join(co, &g, member("a", &["range"]), Duration::from_secs(1))
        .await;
    heartbeats.register(&g, "a", 30_000);
    log.refuse_append();
    let _ = joins
        .join(co, &g, member("b", &["range"]), Duration::from_secs(1))
        .await;
    heartbeats.register(&g, "b", 30_000);
    log.heal();

    // ⚠️ `b`'s failed join already re-planned into a fresh round and enrolled
    // there, so it is `a` arriving that fills it. With the size forgotten that
    // round's `expected` is `None` and `a` would park until the deadline
    // instead.
    let outcome = joins
        .join(co, &g, member("a", &["range"]), Duration::from_secs(1))
        .await;
    heartbeats.register(&g, "a", 30_000);
    assert!(
        matches!(outcome, JoinOutcome::Ready(_)),
        "the group's known roster must survive a transient append failure, or \
         a blip that healed in milliseconds costs the group a fresh round it \
         did not need"
    );
}
