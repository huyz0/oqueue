//! What bounds a follower's wait, and where that number comes from.
//!
//! ⚠️ **Its own file because the concept is new, not because another one
//! was full** (`code-structure.md` rule 18). Every other file here asks what
//! a follower is *told*; these two ask how long it waits to be told, which
//! `M4.66` made a property of the group rather than a constant.
//!
//! ⚠️ **The two halves are in two modules and neither test covers both.**
//! `join_group::round` is what knows `rebalance_timeout_ms` and records it;
//! `sync_group` is what reads it back and parks on it. A test that only
//! drove the second would pass against a broker where nothing ever wrote the
//! value, which is exactly the state this file was added to end.

#![allow(clippy::expect_used)]

use super::{join_as_a_newcomer, join_asking, seat, sync};
use crate::testing::fixture;
use oqueue_codec::error_codes;
use std::time::Duration;

/// ⚠️ **The writer half**: a round opened through the real `JoinGroup`
/// handler records what it was opened with, so the sync phase has a number
/// to be bounded by at all.
#[tokio::test(start_paused = true)]
async fn a_round_opened_by_a_real_join_records_what_it_was_opened_with() {
    let fixture = fixture(&[]).await;
    let g = oqueue_core::GroupId::new("orders-consumers").expect("valid");
    assert_eq!(
        fixture.cluster.sync_groups().rebalance_timeout(&g),
        None,
        "a group no round has opened has no number of its own, and falls back to the ceiling"
    );

    join_as_a_newcomer(&fixture.cluster, "orders-consumers").await;

    assert_eq!(
        fixture.cluster.sync_groups().rebalance_timeout(&g),
        Some(Duration::from_secs(30)),
        "the value the joining member asked for, carried to the phase that cannot read it \
         off its own request"
    );
}

/// ⚠️ **The reader half, measured.** Before `M4.66` this follower waited
/// `MAX_SYNC_WAIT_MS` — fifty minutes — whatever its group had asked for,
/// and nothing else bounded the phase: a parked follower is not
/// heartbeating, so `Heartbeats::sweep` is not running on its behalf.
///
/// ⚠️ **The assertion is the elapsed time, not the code.** Every other test
/// in this module asserts `REBALANCE_IN_PROGRESS`, which
/// `a_follower_that_waits_out_the_deadline_is_told_to_rejoin_not_given_a_fatal_code`
/// already gets from the ceiling — so asserting only the code here would
/// pass with this row reverted. That is the vacuity `M4.58`'s first test had
/// and its row records.
#[tokio::test(start_paused = true)]
async fn a_followers_wait_is_its_groups_own_number_not_the_ceiling() {
    let fixture = fixture(&[]).await;
    let g = oqueue_core::GroupId::new("orders-consumers").expect("valid");
    seat(&fixture.cluster, "orders-consumers", &["m1", "m2"]);
    fixture
        .cluster
        .sync_groups()
        .note_rebalance_timeout(&g, Duration::from_secs(45));

    // Nobody submits and nobody leaves: the deadline is the only thing that
    // can answer, which is what makes the elapsed time the measurement.
    let started = tokio::time::Instant::now();
    let follower = sync(
        &fixture.cluster,
        super::super::tests::follower_body_at("orders-consumers", "m2", 1),
    )
    .await;
    let waited = tokio::time::Instant::now() - started;

    assert_eq!(follower.error_code, error_codes::REBALANCE_IN_PROGRESS);
    assert!(
        waited >= Duration::from_secs(45),
        "the group asked for 45 s and must actually get it, not be answered early: {waited:?}"
    );
    assert!(
        waited < Duration::from_mins(1),
        "and must not be held to the 3_000_000 ms ceiling its group never asked for: {waited:?}"
    );
}

/// ⚠️ **One member's small number must not become the group's**, which the
/// first version of the writer let it: the value was last-writer-wins, so
/// whichever member happened to open the round set the bound every other
/// member's follower ran under. Both reference clients seed
/// `rebalance_timeout_ms` from `max.poll.interval.ms`, which an application
/// may legitimately set to a second.
#[tokio::test(start_paused = true)]
async fn a_members_short_number_does_not_truncate_its_groups() {
    let fixture = fixture(&[]).await;
    let g = oqueue_core::GroupId::new("orders-consumers").expect("valid");
    let groups = fixture.cluster.sync_groups();

    groups.note_rebalance_timeout(&g, Duration::from_secs(45));
    groups.note_rebalance_timeout(&g, Duration::from_secs(1));

    assert_eq!(
        groups.rebalance_timeout(&g),
        Some(Duration::from_secs(45)),
        "the fold is the maximum over every member that asked, not the last one to ask"
    );
}

/// ⚠️ **And a member asking for nothing must not answer somebody else's
/// follower instantly.** Review drove a `JoinGroup` with
/// `rebalance_timeout_ms=0` through the real handler against the first
/// version of this row and measured a different member's follower answered
/// `REBALANCE_IN_PROGRESS` at `waited = 0ns` — handed a rejoin instead of
/// the slice a leader submitted a millisecond later. `barrier_ms` clamps at
/// zero and needs no floor because a zero there shortens only the asking
/// member's own round; this phase is where the same zero reaches everyone.
#[tokio::test(start_paused = true)]
async fn a_member_asking_for_nothing_does_not_answer_another_members_follower_at_once() {
    let fixture = fixture(&[]).await;
    let g = oqueue_core::GroupId::new("orders-consumers").expect("valid");
    seat(&fixture.cluster, "orders-consumers", &["m1", "m2"]);
    fixture
        .cluster
        .sync_groups()
        .note_rebalance_timeout(&g, Duration::ZERO);

    let started = tokio::time::Instant::now();
    let follower = sync(
        &fixture.cluster,
        super::super::tests::follower_body_at("orders-consumers", "m2", 1),
    )
    .await;
    let waited = tokio::time::Instant::now() - started;

    assert_eq!(follower.error_code, error_codes::REBALANCE_IN_PROGRESS);
    assert!(
        waited >= Duration::from_millis(super::super::deadline::MIN_SYNC_WAIT_MS),
        "a zero must become the floor, not no wait at all: {waited:?}"
    );
}

/// ⚠️ **A member that was never enrolled must not set the group's bound.**
/// Review measured the first version of this row doing exactly that: a join
/// refused for an unusable protocol while asking
/// `rebalance_timeout_ms=3_000_000` left the group at the ceiling for the
/// rest of the process, so every later follower whose leader died between
/// `JoinBarrierComplete` and its own `SyncGroup` parked fifty minutes —
/// `waited_ms=3000000`, the `M4.47` stranding this row exists to end, set by
/// a member that never joined.
#[tokio::test(start_paused = true)]
async fn a_refused_join_does_not_set_the_groups_number() {
    let fixture = fixture(&[]).await;
    let g = oqueue_core::GroupId::new("orders-consumers").expect("valid");

    let opener = spawn_join(&fixture, "range", 30_000);
    settle().await;
    assert_eq!(
        fixture.cluster.sync_groups().rebalance_timeout(&g),
        Some(Duration::from_secs(30)),
        "the opening member's own number, before anyone else has asked"
    );

    // Shares no protocol with the round's running candidate set, so it is
    // never enrolled -- `JoinOutcome::Refused`.
    let refused = spawn_join(&fixture, "sticky", 3_000_000);
    settle().await;

    assert_eq!(
        fixture.cluster.sync_groups().rebalance_timeout(&g),
        Some(Duration::from_secs(30)),
        "the refused member's 3_000_000 ms must not have become the group's bound"
    );
    opener.abort();
    refused.abort();
}

/// ⚠️ **And a member joining a round somebody else opened must reach the
/// fold**, which is what puts the call in `join`'s `Step::Done` arm rather
/// than in `apply_open`. Review measured the `apply_open` placement leaving
/// the whole suite green: every other test here drives one member, which is
/// always the round-*opening* member, so nothing distinguished the two
/// placements. Unconstrained, a member opening a round at 6 s bounds the
/// sync phase of a member that asked for the reference clients' own 300 s
/// default.
#[tokio::test(start_paused = true)]
async fn a_member_joining_an_open_round_raises_the_groups_number() {
    let fixture = fixture(&[]).await;
    let g = oqueue_core::GroupId::new("orders-consumers").expect("valid");

    let opener = spawn_join(&fixture, "range", 6_000);
    settle().await;
    assert_eq!(
        fixture.cluster.sync_groups().rebalance_timeout(&g),
        Some(Duration::from_secs(6)),
        "the opening member's own number, before anyone else has asked"
    );

    let later = spawn_join(&fixture, "range", 300_000);
    settle().await;

    assert_eq!(
        fixture.cluster.sync_groups().rebalance_timeout(&g),
        Some(Duration::from_mins(5)),
        "a member that joined the open round raises the fold; it does not go unrecorded"
    );
    opener.abort();
    later.abort();
}

/// A `JoinGroup` in flight, so a second one can arrive while the first
/// member's round is still open.
///
/// ⚠️ **Spawned rather than `poll_once`d.** `join_group::handle` awaits
/// before it reaches `GroupJoins::join`, so a single poll stops short of the
/// step that records anything; and awaiting the join to completion closes
/// the round, after which the next member opens a *new* one and the case
/// under test never happens.
fn spawn_join(
    fixture: &crate::testing::Fixture,
    protocol: &'static str,
    rebalance_timeout_ms: i32,
) -> tokio::task::JoinHandle<()> {
    let cluster = std::sync::Arc::clone(&fixture.cluster);
    tokio::spawn(async move {
        join_asking(&cluster, "orders-consumers", protocol, rebalance_timeout_ms).await;
    })
}

/// Lets every spawned join reach its own first park, without advancing the
/// paused clock — `generations.rs`'s own idiom, and the reason these tests
/// assert on a recorded value rather than on elapsed time.
async fn settle() {
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }
}

/// ⚠️ **The round-*closing* member is enrolled and must be folded in**, which
/// `M4.74` found and `M4.66` had missed: the recording sat in `join`'s
/// `Step::Done` arm and matched `Ready | Pending`, but every `Step::Done` in
/// that module carries `Refused` or `Pending` — `Ready` comes back from the
/// `Step::Close` arm, which returned around it. The arm matched a variant it
/// could never see, and the one member whose join closes a round never
/// recorded what it asked for.
///
/// ⚠️ **A rejoin is the only single-member way to reach that arm.** The next
/// round's `awaiting` is seeded from the last round's roster, so the one
/// member coming back empties it and closes the round on the spot.
#[tokio::test(start_paused = true)]
async fn a_member_whose_rejoin_closes_the_round_is_folded_in() {
    let fixture = fixture(&[]).await;
    let g = oqueue_core::GroupId::new("orders-consumers").expect("valid");

    let first =
        super::join_asking_as(&fixture.cluster, "orders-consumers", "", "range", 6_000).await;
    assert_eq!(
        fixture.cluster.sync_groups().rebalance_timeout(&g),
        Some(Duration::from_secs(6)),
        "the opening join records its own number"
    );
    let member_id = first.member_id.to_string();
    assert!(
        !member_id.is_empty(),
        "the broker mints an id to rejoin with"
    );

    super::join_asking_as(
        &fixture.cluster,
        "orders-consumers",
        &member_id,
        "range",
        300_000,
    )
    .await;

    assert_eq!(
        fixture.cluster.sync_groups().rebalance_timeout(&g),
        Some(Duration::from_mins(5)),
        "a rejoin that closes the round is enrolled, and its number joins the fold"
    );
}

/// ⚠️ **One member must not be able to pin the group at the ceiling** —
/// `M4.76`, and the defect it closes is the stranding six rows were written to
/// end, restored by a single request.
///
/// The fold is a maximum that never resets, so before this a `JoinGroup`
/// asking `rebalance_timeout_ms = 3_000_000` left every later follower of that
/// group parking `MAX_SYNC_WAIT_MS` on the one route the derived wait exists
/// for — the leader dying between `JoinBarrierComplete` and its own
/// `SyncGroup`. The member need only reach `Pending` and may then disconnect;
/// nothing un-notes it, and nothing short of a restart recovers.
///
/// ⚠️ **Two assertions, and the second is the one that names the harm.** The
/// fold is checked first and is what fires when the clamp is removed
/// (`Some(3000s)`); the elapsed time of a *later* follower is checked after,
/// and is independently constraining — review measured it failing at `3000s`
/// with the fold assertion deleted. Both are here because the fold alone would
/// pass against a broker that clamped on read and left the value poisoned,
/// and the elapsed time alone would not say which member's ask caused it.
#[tokio::test(start_paused = true)]
async fn one_member_asking_for_the_ceiling_does_not_pin_the_group_at_it() {
    let fixture = fixture(&[]).await;
    let g = oqueue_core::GroupId::new("orders-consumers").expect("valid");

    let greedy = spawn_join(&fixture, "range", 3_000_000);
    settle().await;
    let noted = fixture.cluster.sync_groups().rebalance_timeout(&g);
    assert!(
        noted.is_some_and(|d| d < Duration::from_millis(super::super::deadline::MAX_SYNC_WAIT_MS)),
        "a single member's ask must not reach the ceiling: {noted:?}"
    );
    greedy.abort();

    // A follower of that group, with nobody submitting and nobody leaving, so
    // its own deadline is the only thing that can answer. ⚠️ Not `seat`: the
    // greedy join has already taken the group to `PreparingRebalance`, where
    // `Join` is illegal, so only the barrier-complete half applies here.
    for id in ["m1", "m2"] {
        fixture.cluster.heartbeats().register(&g, id, 30_000);
    }
    fixture
        .cluster
        .group_coordinator()
        .transition(&g, oqueue_core::GroupEvent::JoinBarrierComplete)
        .expect("PreparingRebalance -> CompletingRebalance is legal");
    let started = tokio::time::Instant::now();
    let follower = sync(
        &fixture.cluster,
        super::super::tests::follower_body_at("orders-consumers", "m2", 1),
    )
    .await;
    let waited = tokio::time::Instant::now() - started;

    assert_eq!(follower.error_code, error_codes::REBALANCE_IN_PROGRESS);
    assert!(
        waited < Duration::from_millis(super::super::deadline::MAX_SYNC_WAIT_MS),
        "a later follower must not inherit the ceiling from one member's ask: {waited:?}"
    );
}
