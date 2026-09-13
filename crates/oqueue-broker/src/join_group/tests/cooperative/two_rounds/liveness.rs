//! Which members a round waits for, and what makes one stop being waited for.
//!
//! ⚠️ **These tests are about `OpenRound::awaiting` being pruned by
//! *liveness*, not by tracking.** Nothing reaps in the background — `sweep`
//! runs only when some other member of the same group heartbeats — so a
//! crashed consumer stays tracked indefinitely, and a round that waits for
//! every tracked id waits out its whole `rebalance_timeout_ms` on every
//! crash-restart.
//!
//! ⚠️ **One pruning site, `GroupJoins::reprune_awaiting`, and that is the
//! point.** Liveness was once tested in three places — at install, on every
//! enrolment, and in the re-prune — and review showed the first two could
//! each be deleted with the whole suite still green, because the re-prune
//! reached the same state one hop later. A branch nothing can pin is a
//! branch nothing is checking, so the two redundant copies are gone rather
//! than papered over with tests that would have passed either way. What
//! these tests pin is the behaviour: a round stops waiting for members that
//! have died, whenever they die.

#![allow(clippy::expect_used)]

use super::super::{VERSION, join};
use super::{request_body_offering, request_body_offering_with_timeout, round_of};
use crate::testing::fixture;

/// **A consumer that restarts does not stall the group for a full rebalance
/// timeout.** Round two's `MAJOR`, and the hazard the `awaiting` rule
/// introduced before it was pruned.
///
/// ⚠️ **A returning consumer never reuses its member id.** It has no memberId
/// after a restart — and none after being fenced `UNKNOWN_MEMBER_ID`, which is
/// the ordinary recovery path — so it sends `member_id=""` and is minted a new
/// one. Waiting for the *old* id to reappear means waiting for something that
/// never will: the round closes only on its deadline, and with the Java
/// consumer's default `rebalance_timeout_ms` of 300 s the whole group consumes
/// nothing for five minutes, on every restart. So a dead id is struck off
/// `awaiting` by `GroupJoins::reprune_awaiting`.
#[tokio::test(start_paused = true)]
async fn a_restarted_consumer_does_not_stall_the_group() {
    let fixture = std::sync::Arc::new(fixture(&[]).await);
    let cluster = std::sync::Arc::clone(&fixture.cluster);

    let first = round_of(&cluster, &[("", b"owns:"), ("", b"owns:")]).await;
    let ids: Vec<String> = first.iter().map(|r| r.member_id.to_string()).collect();

    // ⚠️ **The member is *crashed*, not gracefully gone — which is the shape
    // that actually stalls.** An earlier version of this test called
    // `heartbeats().leave(...)`, which untracks the id synchronously: the
    // graceful-shutdown shape, and the one case the first fix already handled.
    // A `SIGKILL`ed consumer sends no `LeaveGroup` and nothing reaps it in the
    // background — `sweep` runs only when another member of the same group
    // heartbeats — so its id stays tracked and a round keyed on *tracked* ids
    // waits out its whole `rebalance_timeout_ms`. Letting the session lapse is
    // that shape. Found by review.
    let group = oqueue_core::GroupId::new("orders").expect("valid");
    tokio::time::advance(std::time::Duration::from_secs(31)).await;
    assert!(
        cluster.heartbeats().is_tracked(&group, &ids[1]),
        "the crashed member is still *tracked* — nothing has reaped it, which \
         is the whole point of this case"
    );
    assert!(
        !cluster.heartbeats().is_live(&group, &ids[1]),
        "but its session has lapsed, so no round should wait for it"
    );

    // ⚠️ The survivor is alive: its own heartbeats have been renewing its
    // session all along, which is exactly what distinguishes it from the
    // crashed one. Advancing the clock lapsed both, so this puts the survivor
    // back where its heartbeats would have kept it.
    cluster.heartbeats().register(&group, &ids[0], 30_000);

    // The survivor rejoins alongside the restarted consumer's *new* identity.
    let newcomer = {
        let cluster = std::sync::Arc::clone(&cluster);
        let body =
            request_body_offering("orders", "", &[("cooperative-sticky", b"owns:")], VERSION);
        tokio::spawn(async move { join(&cluster, body, VERSION).await })
    };
    tokio::task::yield_now().await;
    let survivor = {
        let cluster = std::sync::Arc::clone(&cluster);
        let body = request_body_offering(
            "orders",
            &ids[0],
            &[("cooperative-sticky", b"owns:p0,p1")],
            VERSION,
        );
        tokio::spawn(async move { join(&cluster, body, VERSION).await })
    };
    // ⚠️ **Six seconds: past `SLOT_WAIT`, well short of the round's own
    // deadline.** The joins park briefly on the group's in-flight slot, which
    // needs a timer to lapse, so "finished with the clock untouched" is not
    // the right discriminator. What distinguishes the fix from the bug is the
    // *round deadline*: this harness sets it to 10 s, so a round that closes
    // because its roster is complete is done by now, and one that is still
    // waiting for a dead member's id is not.
    tokio::time::advance(std::time::Duration::from_secs(6)).await;
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }

    assert!(
        newcomer.is_finished() && survivor.is_finished(),
        "the round must close once every *live* member has rejoined — waiting \
         for an id that is tracked but dead means a full rebalance_timeout_ms \
         of silence on every crash-restart"
    );
    for joiner in [newcomer, survivor] {
        let response = joiner.await.expect("joins");
        assert_eq!(response.error_code, 0, "both present members join");
    }
}

/// ⚠️ **A member that dies *after* the round opened must stop being waited
/// for.** `awaiting` is seeded with the previous roster verbatim and
/// `plan_join` strikes off only the enrolling member's own id, so a member
/// that was alive when the round opened and dies before it rejoins is struck
/// off by nothing but `GroupJoins::reprune_awaiting`.
///
/// ⚠️ **It is the *survivor's* own wake that closes this round, not the
/// newcomer's enrolment**, and an earlier version of this comment had that
/// backwards. The survivor is parked in `wait_for_close`, whose every pass
/// re-prunes and re-arms at the latest deadline among the members still
/// awaited; when the dead member's session lapses that wake finds nothing
/// left to wait for and closes. The newcomer's join is what gives the round a
/// second member to close *onto*, not what notices the death. Shown by
/// review, which moved the newcomer's spawn later and watched the round close
/// anyway.
///
/// ⚠️ **The whole case has to fit inside the round's own 10 s deadline**, or
/// the round closes on the deadline and the test proves nothing — which is
/// what a first version of it did, by advancing 31 s to lapse the harness's
/// 30 s session. The dying member is re-registered at the 6 s floor instead.
#[tokio::test(start_paused = true)]
async fn a_member_dying_after_the_round_opened_stops_being_waited_for() {
    let fixture = std::sync::Arc::new(fixture(&[]).await);
    let cluster = std::sync::Arc::clone(&fixture.cluster);
    let group = oqueue_core::GroupId::new("orders").expect("valid");

    let first = round_of(&cluster, &[("", b"owns:"), ("", b"owns:")]).await;
    let ids: Vec<String> = first.iter().map(|r| r.member_id.to_string()).collect();

    // A 1 s session for the one about to die, a fresh 30 s for the survivor.
    // 6 s is `heartbeat::deadline::MIN_SESSION_TIMEOUT_MS`, the shortest
    // session this broker will grant; anything lower clamps up to it, which
    // is what made a 1 s attempt here still live at 2 s. Spelled as a literal
    // because that module is private to `heartbeat`.
    cluster.heartbeats().register(&group, &ids[1], 6_000);
    cluster.heartbeats().register(&group, &ids[0], 30_000);

    // The survivor rejoins, installing the next round. `awaiting` is the
    // previous roster verbatim, so it holds both ids.
    let survivor = {
        let cluster = std::sync::Arc::clone(&cluster);
        let body = request_body_offering(
            "orders",
            &ids[0],
            &[("cooperative-sticky", b"owns:p0,p1")],
            VERSION,
        );
        tokio::spawn(async move { join(&cluster, body, VERSION).await })
    };
    tokio::task::yield_now().await;
    assert!(
        cluster.heartbeats().is_live(&group, &ids[1]),
        "the second member must still be live when the round is installed — \
         the case is a member that dies mid-round, not one already dead"
    );

    // Now it dies: no LeaveGroup, no sweep, just a lapsed session, with its
    // id already inside this round's `awaiting`.
    tokio::time::advance(std::time::Duration::from_secs(7)).await;
    assert!(
        cluster.heartbeats().is_tracked(&group, &ids[1])
            && !cluster.heartbeats().is_live(&group, &ids[1]),
        "tracked but not live is the shape that stalls"
    );

    // A newcomer gives the round a second member to close onto. The close
    // itself comes from the survivor's own re-prune wake.
    let newcomer = {
        let cluster = std::sync::Arc::clone(&cluster);
        let body =
            request_body_offering("orders", "", &[("cooperative-sticky", b"owns:")], VERSION);
        tokio::spawn(async move { join(&cluster, body, VERSION).await })
    };
    // Still inside the round's 10 s deadline: finishing here means the roster
    // completed, not that the round timed out.
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }

    assert!(
        newcomer.is_finished() && survivor.is_finished(),
        "the round must close on the two live members — an id that died after \
         the round opened is still in `awaiting`, and only `reprune_awaiting` \
         strikes it off"
    );
    for joiner in [newcomer, survivor] {
        let response = joiner.await.expect("joins");
        assert_eq!(response.error_code, 0, "both live members join");
    }
}

/// ⚠️ **A round whose last enrolment leaves a dead id behind must not wait
/// out its whole `rebalance_timeout_ms`.** The common way to leave a dead id
/// behind is a consumer restarting *inside* its own session window: it comes
/// back under a new minted id while the old one is still live at the instant
/// the round is installed. It then lapses with nobody left to
/// enrol and notice.
///
/// The count-based rule this replaced closed such a round at once, so the
/// stall is one the roster rule introduced and has to answer for itself:
/// `wait_for_close` wakes at the latest deadline among the members still
/// awaited and closes if every one of them has gone. Found by review.
#[tokio::test(start_paused = true)]
async fn a_round_whose_whole_awaited_roster_dies_closes_without_waiting_for_the_deadline() {
    let fixture = std::sync::Arc::new(fixture(&[]).await);
    let cluster = std::sync::Arc::clone(&fixture.cluster);
    let group = oqueue_core::GroupId::new("orders").expect("valid");

    let first = round_of(&cluster, &[("", b"owns:"), ("", b"owns:")]).await;
    let ids: Vec<String> = first.iter().map(|r| r.member_id.to_string()).collect();

    // Both ids live, both on the shortest session this broker grants, so the
    // whole case fits inside the harness's 10 s round deadline.
    for id in &ids {
        cluster.heartbeats().register(&group, id, 6_000);
    }

    // The restarted consumer returns under a *new* id, and the survivor never
    // rejoins at all — it is the one that dies. `awaiting` is installed as
    // both old ids, and the newcomer's enrolment strikes off neither: it is
    // not either of them.
    let newcomer = {
        let cluster = std::sync::Arc::clone(&cluster);
        let body =
            request_body_offering("orders", "", &[("cooperative-sticky", b"owns:")], VERSION);
        tokio::spawn(async move { join(&cluster, body, VERSION).await })
    };
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
    assert!(
        !newcomer.is_finished(),
        "the round is open and waiting for two live ids — closing here would \
         be the newcomer closing a round on its own, the hole `awaiting` exists \
         to shut"
    );

    // Both awaited ids now lapse, with no further enrolment to notice.
    tokio::time::advance(std::time::Duration::from_secs(7)).await;
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }

    // 7 s, inside the round's own 10 s deadline: finishing here means the
    // dead roster was noticed, not that the round timed out.
    assert!(
        newcomer.is_finished(),
        "every awaited member is dead, so there is nobody left to wait for — \
         waiting to the deadline stalls the group for a full \
         rebalance_timeout_ms (300 s with the Java consumer's defaults) on \
         every consumer restart"
    );
    let response = newcomer.await.expect("joins");
    assert_eq!(response.error_code, 0, "the newcomer joins");
}

/// ⚠️ **A previous roster that is already dead when the round opens must not
/// stall it either.** The sibling above covers a roster that dies *during* a
/// round; this covers one already gone when the round is installed, which is
/// the ordinary single-consumer restart: nothing else is left to heartbeat,
/// so nothing sweeps the old id, and the returning consumer arrives under a
/// new one.
///
/// Both cases are the same state, and until this test they behaved two
/// different ways depending only on when it was observed — `install_round`
/// collapsed a wholly-dead roster to `None`, the same value a brand-new group
/// gets, so there was nothing left for `next_wake` to re-prune and the round
/// waited out its deadline. A roster that died one instant later closed at
/// once. Found by review.
#[tokio::test(start_paused = true)]
async fn a_roster_already_dead_when_the_round_opens_does_not_stall_it() {
    let fixture = std::sync::Arc::new(fixture(&[]).await);
    let cluster = std::sync::Arc::clone(&fixture.cluster);
    let group = oqueue_core::GroupId::new("orders").expect("valid");

    let first = round_of(&cluster, &[("", b"owns:p0,p1")]).await;
    let old_id = first[0].member_id.to_string();

    // The sole consumer crashes: no LeaveGroup, and with nobody else in the
    // group nothing will ever sweep it. Its session simply lapses.
    tokio::time::advance(std::time::Duration::from_secs(31)).await;
    assert!(
        cluster.heartbeats().is_tracked(&group, &old_id)
            && !cluster.heartbeats().is_live(&group, &old_id),
        "tracked but dead, with nothing left to reap it — the restart shape"
    );

    // It comes back under a new minted id. Its old id is the whole of
    // `last_round_members`, and it is dead at this instant.
    let restarted = {
        let cluster = std::sync::Arc::clone(&cluster);
        let body =
            request_body_offering("orders", "", &[("cooperative-sticky", b"owns:")], VERSION);
        tokio::spawn(async move { join(&cluster, body, VERSION).await })
    };
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }

    // No clock advance at all: the round must close on this enrolment, not on
    // its 10 s deadline.
    assert!(
        restarted.is_finished(),
        "the only member of the previous roster is dead, so there is nobody to \
         revoke anything from and nothing to wait for — waiting to the deadline \
         is a full rebalance_timeout_ms of silence on every restart of a \
         single-consumer group"
    );
    let response = restarted.await.expect("joins");
    assert_eq!(response.error_code, 0, "the restarted consumer joins");
}

/// ⚠️ **A live awaited member must not push the answer past the round's own
/// deadline.** `next_wake` wakes at the latest deadline among the members
/// still awaited, and a member's session can be far longer than the round's
/// `rebalance_timeout_ms` — Kafka's own defaults are the other way round, but
/// nothing enforces that, and this harness's 30 s session against a 10 s
/// round deadline is exactly the inverted shape.
///
/// Without the `.min(deadline)` in `next_wake`, the wake lands at the
/// *session* deadline and the join is held roughly three times past the
/// timeout the client asked for — the client has long since given up. Nothing
/// asserted this: removing the clamp left every test passing, and only the
/// suite's wall-clock moved. Found by review.
#[tokio::test(start_paused = true)]
async fn a_live_awaited_member_does_not_hold_a_join_past_the_round_deadline() {
    let fixture = std::sync::Arc::new(fixture(&[]).await);
    let cluster = std::sync::Arc::clone(&fixture.cluster);

    let first = round_of(&cluster, &[("", b"owns:"), ("", b"owns:")]).await;
    let ids: Vec<String> = first.iter().map(|r| r.member_id.to_string()).collect();

    // One member rejoins; the other never does, and stays alive throughout —
    // its 30 s session outlasts the 10 s round deadline by 20 s.
    let survivor = {
        let cluster = std::sync::Arc::clone(&cluster);
        let body = request_body_offering(
            "orders",
            &ids[0],
            &[("cooperative-sticky", b"owns:p0,p1")],
            VERSION,
        );
        tokio::spawn(async move { join(&cluster, body, VERSION).await })
    };

    // Past the round's own 10 s deadline, plus room for the two
    // `close_on_deadline` passes' own `SLOT_WAIT` budgets — and still far
    // short of the absent member's 30 s session, which is the discriminator:
    // an unclamped wake lands at that session deadline, well beyond here.
    for _ in 0..4 {
        tokio::time::advance(std::time::Duration::from_secs(4)).await;
        for _ in 0..64 {
            tokio::task::yield_now().await;
        }
    }

    assert!(
        survivor.is_finished(),
        "16 s in, past the round's own 10 s deadline and short of the awaited \
         member's 30 s session: the deadline is the promise made to the \
         client, and an awaited member whose session outlasts it must not \
         extend the wait"
    );
}

/// ⚠️ **A group nobody has joined before must not hold its first joiner for
/// `rebalance_timeout_ms`.** A brand-new group has no previous roster, so
/// `awaiting` is `None` and nothing can close the round early — it runs to
/// its own deadline, which is the client's `rebalance_timeout_ms`. Real
/// clients seed that from `max.poll.interval.ms`: **300 s** for both
/// librdkafka and the Java consumer, by default.
///
/// ⚠️ **Measured against real librdkafka before this was written** (`M4.29`,
/// whose row asked for exactly that). A `confluent_kafka.Consumer` subscribing
/// to a fresh group got no assignment at all within 20 s and its
/// `JoinGroupRequest` went unanswered; the same consumer with
/// `max.poll.interval.ms=10000` was assigned after ~10 s and consumed every
/// record. The stall is exactly `rebalance_timeout_ms`, so with defaults a
/// consumer takes five minutes to receive its first partition — which makes
/// consumer groups unusable against a real client rather than merely slow.
///
/// ⚠️ **The same `awaiting == None` covers a restarted broker**, which is how
/// `M4.29` framed it: `GroupJoins` is per-node, so a restart loses
/// `last_round_members` and every group's next round looks brand new. The
/// defect is not restart-specific, and one bound answers both.
///
/// Kafka's own answer is `group.initial.rebalance.delay.ms`, 3 s by default,
/// and [`INITIAL_REBALANCE_DELAY`] is that bound here.
#[tokio::test(start_paused = true)]
async fn a_brand_new_group_does_not_hold_its_first_joiner_for_the_rebalance_timeout() {
    let fixture = std::sync::Arc::new(fixture(&[]).await);
    let cluster = std::sync::Arc::clone(&fixture.cluster);

    // `request_body_offering` asks for rebalance_timeout_ms = 10_000, so the
    // pre-fix round closed 10 s after it was installed. 4 s is past the 3 s
    // delay and well short of that.
    let first = {
        let cluster = std::sync::Arc::clone(&cluster);
        let body =
            request_body_offering("orders", "", &[("cooperative-sticky", b"owns:")], VERSION);
        tokio::spawn(async move { join(&cluster, body, VERSION).await })
    };

    // ⚠️ **Yield until the round is installed *before* touching the clock.**
    // An earlier version advanced first and attributed the need for two steps
    // to the in-flight slot; review measured the real reason, which is that
    // the join had not been polled yet, so the clock moved before the round
    // existed and its deadline was set relative to the later instant. With
    // the round installed at t0 its deadline is t0+3 s, the pre-fix deadline
    // is t0+10 s, and a single 4 s advance tells them apart with no ambiguity
    // about when the clock started counting. Found by review.
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }
    tokio::time::advance(std::time::Duration::from_secs(4)).await;
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }

    assert!(
        first.is_finished(),
        "a brand-new group's first round must close on its initial-rebalance \
         delay, not on `rebalance_timeout_ms` — with a real client's 300 s \
         default the consumer waits five minutes for its first partition"
    );
    let response = first.await.expect("joins");
    assert_eq!(response.error_code, 0, "the first joiner joins");
}

/// ⚠️ **A client asking for *less* than the initial delay gets what it asked
/// for.** [`INITIAL_REBALANCE_DELAY`] is a ceiling on the first round's wait,
/// not a floor on it: holding a member 3 s when it named a 1 s barrier would
/// overrun the timeout it set, which is the same promise
/// `a_live_awaited_member_does_not_hold_a_join_past_the_round_deadline` pins
/// from the other direction. The `min` is what makes that true, and nothing
/// pinned it until this test. Found by review.
#[tokio::test(start_paused = true)]
async fn a_barrier_shorter_than_the_initial_delay_is_honoured() {
    let fixture = std::sync::Arc::new(fixture(&[]).await);
    let cluster = std::sync::Arc::clone(&fixture.cluster);

    let joiner = {
        let cluster = std::sync::Arc::clone(&cluster);
        let body = request_body_offering_with_timeout(
            "orders",
            "",
            &[("cooperative-sticky", b"owns:")],
            VERSION,
            1_000,
        );
        tokio::spawn(async move { join(&cluster, body, VERSION).await })
    };
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }

    // 2 s: past the 1 s the client asked for, short of the 3 s delay.
    tokio::time::advance(std::time::Duration::from_secs(2)).await;
    for _ in 0..64 {
        tokio::task::yield_now().await;
    }

    assert!(
        joiner.is_finished(),
        "a 1 s barrier must be answered by 2 s — `INITIAL_REBALANCE_DELAY` \
         bounds how long a first round may wait, it does not extend a wait \
         the client asked to be shorter"
    );
    let response = joiner.await.expect("joins");
    assert_eq!(response.error_code, 0, "the joiner joins");
}
