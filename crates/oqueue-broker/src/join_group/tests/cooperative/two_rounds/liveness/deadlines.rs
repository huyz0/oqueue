//! How long a round waits — the bounds, rather than who it waits for.
//!
//! ⚠️ **Its sibling `liveness.rs` asks *who* a round waits for; this asks
//! *how long*.** The two are separable: a round with a roster waits for that
//! roster and is bounded by the client's own `rebalance_timeout_ms`, while a
//! round with none is bounded by `INITIAL_REBALANCE_DELAY` instead. Both
//! bounds are promises to a client, and both were once wrong in the
//! direction that makes a real consumer wait minutes.

#![allow(clippy::expect_used)]

use super::super::super::super::{VERSION, join};
use super::super::super::{request_body_offering, request_body_offering_with_timeout, round_of};
use crate::testing::fixture;

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
