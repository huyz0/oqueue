//! What a leader is told when the group moved while it was parked.
//!
//! ⚠️ **Its own file because the subject is the leader, not the bytes.**
//! Its sibling asks which generation's assignment a *follower* is answered
//! with; these ask what the *leader* is told when the round it submitted
//! for is no longer the one the group is on. Every test here opens the
//! window the same way — `crate::testing::poll_once` stops the submission
//! at its first pending await, which `M4.15d` put between the fence's state
//! read and its application — and then moves the group underneath it.
//!
//! ⚠️ **"Moves the group", not "moves the coordinator", because three of
//! the four do one and the fourth does the other.** The first test below
//! publishes the newer generation's assignment with
//! `sync_groups().submit(..)`; its three siblings drive
//! `group_coordinator().transition(..)`. Both bypass the transitions
//! queue, which is the property that matters and why either works. The
//! narrower wording stood here while the test below it said the opposite,
//! so one file asserted both. `M4.63`.
//! ⚠️ **Not "parks on the actor", which `M4.42`'s commit body records as
//! misstating the mechanism**: the transitions channel is an mpsc the actor
//! drains in order, so nothing can be applied ahead of an event already
//! enqueued. The reachable window is *before* the send, and a reader
//! auditing reachability from the old sentence deletes the refusal arm as
//! dead code. `M4.55`.
//!
//! ⚠️ **Four tests, four different distances the group can travel** in that
//! window, and each was a separate defect: `PreparingRebalance` at the same
//! generation (`M4.42`), `Stable` at a later one (`M4.42`'s own conjunct,
//! which review found pinned by nothing), `CompletingRebalance` at a later
//! one — where the parked event becomes *legal* again (`M4.45`) — and a
//! later generation's map already in the cell (`M4.35`).
//!
//! Split out when `generations.rs` reached `code-structure.md` rule 16's
//! 500-line limit.

#![allow(clippy::expect_used)]

use super::{leader_body_at, seat, sync};
use crate::testing::fixture;

/// ⚠️ **The client-visible half, which review found nothing executed.**
/// The guard above is pinned by `generations.rs`'s
/// `a_late_submission_does_not_replace_a_newer_generations_assignment`
/// — ⚠️ **named rather than linked**, because that test is in a *sibling*
/// module and the intra-doc link this carried resolved to nothing, so
/// rustdoc rendered it as plain code (`M4.55`) —
/// but what the stale leader is *told* was not: planting a `panic!` in the
/// refusal arm left the whole suite green, so the code could have been any
/// of them — including `UNKNOWN_SERVER_ERROR`, which the Java consumer
/// raises out of `poll()` and never retries.
///
/// ⚠️ **The window is opened deterministically rather than waited for.**
/// `handle` reads the group's record under `fence`, then awaits the
/// transitions actor; polling it once stops it at that await, **with its
/// own event enqueued but not yet applied**, and the newer generation's
/// move lands in the window that opens — here, generation 2's assignment
/// published through `sync_groups().submit(..)`. ⚠️ **Which of the two
/// queue-bypassing moves each test uses is the module doc's, not repeated
/// here**, because a count kept in two places is a count that goes wrong in
/// one: adding a fifth test would have to update both. What matters at this
/// call site is *why* the move must bypass the queue at all: `enqueue` is
/// an unbounded mpsc send that never awaits,
/// so the parked handler's own event is already *ahead* of anything sent
/// afterwards — firing the competing one through
/// `group_transitions().transition(..)` would let the actor drain
/// `SyncComplete` first, the window would never open, and the test would
/// assert `REBALANCE_IN_PROGRESS` against a `NONE`. ⚠️ A first version of
/// this paragraph said the competing event here goes through
/// `group_coordinator()`, which is true of the siblings and not of this
/// test; repairing the test to match would have deleted the only coverage
/// of `M4.35`'s cell guard. ⚠️ **No count of its own revisions is kept
/// here**, and one was, wrongly: this sentence has been rewritten enough
/// times that an ordinal in it goes stale the next time it is touched,
/// which `M4.53`'s leg count and `heartbeat.rs`'s revision count each
/// taught separately. What is worth recording is *what* was wrong, above,
/// not how many times. `M4.62`, `M4.63`. This is
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
        "the submission must stop at its first pending await rather than complete synchronously"
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

/// ⚠️ **`M4.42`'s own acceptance criterion.** The illegal-transition arm was
/// argued for one case — already `Stable`, a double submission racing
/// another connection — and caught a second the argument does not cover:
/// a newcomer's `JoinGroup` landing between the fence's state read and the
/// transitions actor fires `MemberJoinedDuringSync`, which is legal, and
/// the leader's own `SyncComplete` is then illegal from
/// `PreparingRebalance`.
///
/// ⚠️ **Swallowing that one answers `NONE` for a generation the group has
/// left.** The leader and every follower it feeds start consuming
/// generation N while the coordinator assembles N+1, in which the newcomer
/// gets some of the same partitions — which is FR-20's revoke-before-
/// reassign, the property the whole milestone exists for. Real Kafka
/// answers `REBALANCE_IN_PROGRESS`. `M4.35`'s own guard does not reach it:
/// the cell holds nothing newer than N, so the submission is accepted.
#[tokio::test(start_paused = true)]
async fn a_leader_whose_group_reopened_its_barrier_while_parked_is_told_to_rejoin() {
    let fixture = fixture(&[]).await;
    let g = oqueue_core::GroupId::new("orders-consumers").expect("valid");
    seat(&fixture.cluster, "orders-consumers", &["m1", "m2"]);

    let submitting = sync(
        &fixture.cluster,
        leader_body_at(
            "orders-consumers",
            "m1",
            &[("m1", b"gen1-m1"), ("m2", b"gen1-m2")],
            1,
        ),
    );
    tokio::pin!(submitting);
    assert!(
        crate::testing::poll_once(&mut submitting).is_none(),
        "the submission must stop at its first pending await"
    );

    // A newcomer joins: legal from `CompletingRebalance`, and it reopens the
    // barrier under the parked leader.
    fixture
        .cluster
        .group_coordinator()
        .transition(&g, oqueue_core::GroupEvent::MemberJoinedDuringSync)
        .expect("CompletingRebalance -> PreparingRebalance is legal");

    let response = submitting.await;
    assert_eq!(
        response.error_code,
        oqueue_codec::error_codes::REBALANCE_IN_PROGRESS,
        "a leader whose group reopened its barrier must rejoin, not be told its assignment stands"
    );

    let (assignments, _) = fixture.cluster.sync_groups().entry_for(&g);
    assert!(
        matches!(
            super::super::assignment_for(&assignments, 1),
            super::super::Known::Refused
        ),
        "no assignment was published for the generation the group left, and -- since \
         `M4.43` -- the barrier says so rather than leaving a parked follower on Waiting"
    );
}

/// ⚠️ **The generation half of that guard, which nothing pinned.** Review of
/// `M4.42` found the split's test green with
/// `r.generation.get() == request.generation_id` deleted, and then built the
/// window where it is load-bearing: the group leaves generation N *and*
/// N+1's leader completes its own sync, so the re-read sees `Stable` — at
/// the wrong generation. Answering with the map then tells N's leader its
/// assignment stands and hands N's own parked followers that map. ⚠️ **Not
/// a harm to N+1's followers**, which this doc claimed until `M4.45`'s
/// review measured it: their cell reads `Waiting` under the bug too, and
/// they are released normally by their own leader's submission.
///
/// ⚠️ **`M4.35`'s guard cannot see it**: N+1's leader has transitioned but
/// not yet submitted, so the cell holds nothing and there is no later
/// generation to compare against.
#[tokio::test(start_paused = true)]
async fn a_leader_is_not_answered_because_some_later_generation_reached_stable() {
    let fixture = fixture(&[]).await;
    let g = oqueue_core::GroupId::new("orders-consumers").expect("valid");
    seat(&fixture.cluster, "orders-consumers", &["m1", "m2"]);

    let submitting = sync(
        &fixture.cluster,
        leader_body_at("orders-consumers", "m1", &[("m1", b"gen1-m1")], 1),
    );
    tokio::pin!(submitting);
    assert!(
        crate::testing::poll_once(&mut submitting).is_none(),
        "the submission must stop at its first pending await"
    );

    // The group runs a whole further round while the leader is parked, and
    // generation 2's own leader syncs -- so the record reads `Stable`, which
    // is half of what the guard looks for, at a generation this submission
    // has nothing to do with.
    for event in [
        oqueue_core::GroupEvent::MemberJoinedDuringSync,
        oqueue_core::GroupEvent::JoinBarrierComplete,
        oqueue_core::GroupEvent::SyncComplete,
    ] {
        fixture
            .cluster
            .group_coordinator()
            .transition(&g, event)
            .expect("a legal round");
    }
    let record = fixture
        .cluster
        .group_coordinator()
        .record(&g)
        .expect("the group exists");
    assert_eq!(record.state, oqueue_core::GroupState::Stable);
    assert_eq!(
        record.generation.get(),
        2,
        "a generation the leader never saw"
    );

    let response = submitting.await;
    assert_eq!(
        response.error_code,
        oqueue_codec::error_codes::REBALANCE_IN_PROGRESS,
        "Stable alone is not the double-submission case -- the generation has to match"
    );

    // ⚠️ **On generation 1, not 2**, for the reason `M4.45`'s own test
    // records: `submit` tags the cell with the *request's* generation, so
    // asking about generation 2 reads `Waiting` whether the bug is present
    // or not. Generation 1 separates them — `Mine(gen1-m1)` under the bug,
    // `Refused` under the fix — and pins `M4.43`'s `refuse` call here too.
    let (assignments, _) = fixture.cluster.sync_groups().entry_for(&g);
    assert!(
        matches!(
            super::super::assignment_for(&assignments, 1),
            super::super::Known::Refused
        ),
        "generation 1's map must not be published, and its own followers must be told to rejoin"
    );
}

/// ⚠️ **`M4.45`'s own acceptance criterion, and the `Ok` arm's version of
/// `M4.42`.** That row split the illegal-transition arm; its sibling has
/// the same hazard through a longer race. If the competing round gets as
/// far as `CompletingRebalance@2` before the parked `SyncComplete` is
/// applied, that event is **legal** again — `CompletingRebalance ->
/// Stable` — so it takes `Ok`, and answering with the map tells the leader
/// generation 1 stands while the group is on generation 2.
///
/// ⚠️ **The harm is to the leader and to generation 1's own followers**, not
/// to generation 2's — review corrected that. Generation 2's followers read
/// `Waiting` and are released normally when their own leader submits. What
/// goes wrong is that generation 1's leader is told `NONE` for a generation
/// the group has left, and generation 1's parked followers are handed its
/// map. ⚠️ **Neither existing guard reaches it**: `M4.35`'s because the
/// cell is empty, so the submission is accepted; `M4.42`'s because the
/// transition *succeeded* and so never entered the arm that checks. Found by review of
/// `M4.42`, which measured `error_code=0 state=Stable gen=2`.
#[tokio::test(start_paused = true)]
async fn a_leader_whose_round_completed_under_it_is_told_to_rejoin() {
    let fixture = fixture(&[]).await;
    let g = oqueue_core::GroupId::new("orders-consumers").expect("valid");
    seat(&fixture.cluster, "orders-consumers", &["m1", "m2"]);

    let submitting = sync(
        &fixture.cluster,
        leader_body_at("orders-consumers", "m1", &[("m1", b"gen1-m1")], 1),
    );
    tokio::pin!(submitting);
    assert!(
        crate::testing::poll_once(&mut submitting).is_none(),
        "the submission must stop at its first pending await"
    );

    // A newcomer joins and the barrier closes again, so the group is back at
    // `CompletingRebalance` -- one generation on. The parked `SyncComplete`
    // is legal from there, which is the whole point: it will take `Ok`.
    for event in [
        oqueue_core::GroupEvent::MemberJoinedDuringSync,
        oqueue_core::GroupEvent::JoinBarrierComplete,
    ] {
        fixture
            .cluster
            .group_coordinator()
            .transition(&g, event)
            .expect("a legal round");
    }

    let response = submitting.await;
    assert_eq!(
        response.error_code,
        oqueue_codec::error_codes::REBALANCE_IN_PROGRESS,
        "a leader whose round completed under it must rejoin, not be told its assignment stands"
    );

    // ⚠️ **On generation 1, not 2, and review caught the difference.** The
    // first version asked whether generation 2's cell was `Mine`, which
    // cannot fail either way: `submit` tags the cell with the *request's*
    // generation, so a bug that publishes generation 1's map still leaves
    // `assignment_for(.., 2)` reading `Waiting`. Asking about generation 1
    // separates the two — `Mine(gen1-m1)` under the bug, `Refused` under
    // the fix — and pins `M4.43`'s `refuse` call on this newly reachable
    // path, which the vacuous form let anyone delete.
    let (assignments, _) = fixture.cluster.sync_groups().entry_for(&g);
    assert!(
        matches!(
            super::super::assignment_for(&assignments, 1),
            super::super::Known::Refused
        ),
        "generation 1's map must not be published, and its own followers must be told to rejoin"
    );
}
