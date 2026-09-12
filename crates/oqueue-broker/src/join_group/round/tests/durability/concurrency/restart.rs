//! `M4.15b`'s own acceptance criterion, which `M4.15d` completes: an
//! interleaved multi-group sequence driven through the real `GroupJoins` path,
//! replayed from the log alone into a coordinator that never saw it live.
//!
//! ⚠️ **Its own file because it is the acceptance test, not a defect test.**
//! Its siblings each pin one thing a review round found broken; this one
//! answers the question the task was opened to answer.

#![allow(clippy::expect_used)]

use super::super::{Harness, group, member};
use crate::group_transitions::GroupTransitions;
use crate::join_group::round::JoinOutcome;
use oqueue_core::{FakeGroupCoordinator, GroupCoordinator, GroupEvent};
use std::time::Duration;

/// Two groups' rounds interleaved, so the log holds each group's events
/// mixed with the other's rather than in two clean runs — which is what
/// makes replay's per-group bookkeeping do any work at all.
async fn drive_interleaved(
    h: &Harness,
    orders: &oqueue_core::GroupId,
    payments: &oqueue_core::GroupId,
) {
    // orders: open a round, close it on its deadline -> CompletingRebalance.
    let JoinOutcome::Pending { outcome: o1, .. } = h
        .join(orders, member("m1", &["range"]), Duration::from_secs(1))
        .await
    else {
        panic!("orders' first round is pending");
    };
    // payments: a round of its own, opened between orders' two steps.
    let JoinOutcome::Pending { outcome: p1, .. } = h
        .join(payments, member("p1", &["range"]), Duration::from_secs(1))
        .await
    else {
        panic!("payments' first round is pending");
    };
    h.close_on_deadline(orders, &o1).await;
    // orders: a member arrives mid-sync -> MemberJoinedDuringSync.
    let _ = h
        .join(orders, member("m2", &["range"]), Duration::from_secs(1))
        .await;
    h.close_on_deadline(payments, &p1).await;
}

/// **`M4.15b`'s own acceptance criterion, which `M4.15d` completes.** An
/// interleaved multi-group sequence driven through the real `GroupJoins`
/// path, then replayed from the log alone into a coordinator that never saw
/// any of it live — every group's record must match what it was before.
///
/// ⚠️ **`M4.15c` could not write this test.** Its three migrated call sites
/// durably recorded `SyncComplete` and the eviction events, but the `Join`
/// and `JoinBarrierComplete` that *precede* them came from this module and
/// were recorded nowhere — so replaying any real group's log hit
/// `IllegalGroupTransition` on its very first entry and the group was
/// poisoned rather than reconstructed. The sequence below is the one that
/// used to produce exactly that.
///
/// ⚠️ **The record is the durable state; membership is not.** `RoundMember`
/// lists live in this module's own per-node bookkeeping, which
/// `round.rs`'s module doc calls "async, per-node, and never durable" by
/// design — the group *state* that membership drives is what survives, and
/// is what this asserts.
#[tokio::test(start_paused = true)]
async fn an_interleaved_multi_group_sequence_replays_to_the_same_records() {
    let h = Harness::new();
    let orders = group("orders");
    let payments = group("payments");

    drive_interleaved(&h, &orders, &payments).await;

    let before: Vec<_> = [&orders, &payments]
        .iter()
        .map(|g| h.coordinator.record(g).expect("both groups exist"))
        .collect();
    // Every one of the three events this row names must be in the log, or
    // the sequence above did not exercise what it claims to.
    let events = h.durable_events(&orders).await;
    for expected in [
        GroupEvent::Join,
        GroupEvent::JoinBarrierComplete,
        GroupEvent::MemberJoinedDuringSync,
    ] {
        assert!(
            events.contains(&expected),
            "the sequence must exercise {expected:?}, but orders recorded {events:?}"
        );
    }

    // The restart: a coordinator that never saw any of it, rebuilt from the
    // log alone.
    let restarted = FakeGroupCoordinator::new();
    let (_transitions, task) = GroupTransitions::new();
    task.replay(&restarted, h.log.as_ref())
        .await
        .expect("replay reads the log");

    for (g, was) in [&orders, &payments].iter().zip(before) {
        let now = restarted
            .record(g)
            .unwrap_or_else(|| panic!("{g:?} must replay into existence, not be poisoned"));
        assert_eq!(now.state, was.state, "{g:?}'s state must survive a restart");
        assert_eq!(
            now.generation, was.generation,
            "{g:?}'s generation must survive a restart"
        );
        assert_eq!(
            now.assignment_epoch, was.assignment_epoch,
            "{g:?}'s assignment epoch must survive a restart"
        );
    }
}
