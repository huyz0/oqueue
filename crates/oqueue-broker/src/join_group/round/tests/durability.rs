//! `M4.15d`: the join path's own transitions are durable.
//!
//! ⚠️ **Its own file because the barrier's tests and the durability tests
//! answer different questions.** `super`'s own cases are about *who is in a
//! round and when it closes* — behaviour that predates this actor entirely.
//! These are about *what the log holds afterwards*, which is the property
//! `M4.15d` created. `code-structure.md` rule 18 calls a 644-line test file
//! a design signal rather than a formatting problem, and this is the seam it
//! was pointing at.

#![allow(clippy::expect_used)]

mod concurrency;

use super::{Harness, group, member};
use crate::join_group::round::JoinOutcome;
use oqueue_core::{GroupEvent, GroupState};
use std::time::Duration;

/// The whole point of `M4.15d`. Before it, `Join` and `JoinBarrierComplete`
/// were applied straight to the live coordinator and written down nowhere —
/// so a restart rebuilt the group with no membership at all, and
/// `GroupTransitionsTask::replay` had to poison essentially every real group
/// it saw, because the *first* durable record for any group a client had
/// joined was a `SyncComplete` with no legal predecessor.
#[tokio::test(start_paused = true)]
async fn a_rounds_own_open_and_close_are_both_durably_recorded() {
    let h = Harness::new();
    let g = group("orders");

    let JoinOutcome::Pending { outcome, .. } = h
        .join(&g, member("m1", &["range"]), Duration::from_secs(1))
        .await
    else {
        panic!("a fresh group's first-ever round has no early-close signal");
    };
    assert_eq!(
        h.durable_events(&g).await,
        vec![GroupEvent::Join],
        "opening a round must durably record the event that opened it"
    );

    h.close_on_deadline(&g, &outcome).await;
    assert_eq!(
        h.durable_events(&g).await,
        vec![GroupEvent::Join, GroupEvent::JoinBarrierComplete],
        "closing a round must durably record its own barrier-complete too, \
         in the order it was applied"
    );
}

/// A member arriving while the group is `CompletingRebalance` opens the next
/// round with `MemberJoinedDuringSync` — the third of the three events this
/// row names, and the one `M4.15c` could not cover because only this module
/// ever fires it.
#[tokio::test(start_paused = true)]
async fn a_join_during_sync_durably_records_member_joined_during_sync() {
    let h = Harness::new();
    let g = group("orders");

    let JoinOutcome::Pending { outcome, .. } = h
        .join(&g, member("m1", &["range"]), Duration::from_secs(1))
        .await
    else {
        panic!("first round is pending");
    };
    h.close_on_deadline(&g, &outcome).await;
    assert_eq!(h.state(&g), Some(GroupState::CompletingRebalance));

    // A second member arrives before the leader has synced.
    let _ = h
        .join(&g, member("m2", &["range"]), Duration::from_secs(1))
        .await;

    // ⚠️ **Four events, not three, and the fourth is correct.** The first
    // round closed with one member, so `last_round_size` is 1 and the round
    // `MemberJoinedDuringSync` opens is *full* the instant m2 enrols — it
    // closes immediately rather than waiting out a deadline. Asserting the
    // three-event prefix would have hidden that; asserting all four says what
    // the group actually recorded.
    assert_eq!(
        h.durable_events(&g).await,
        vec![
            GroupEvent::Join,
            GroupEvent::JoinBarrierComplete,
            GroupEvent::MemberJoinedDuringSync,
            GroupEvent::JoinBarrierComplete,
        ],
        "a join during sync must durably record the event it fired, and the \
         immediate close that follows it for an established group"
    );
}
