//! Every point on the write path where a crash changes what the world sees.
//!
//! ⚠️ **`Cluster::flush` is the write path, and it has two `.await`s** — the
//! PUT and the commit — so there are three windows and not four: before the
//! object is durable, after it is durable and before its position is
//! journalled, and after the journal and before the client hears about it.
//! `M10.9` is the enumeration and the two that were not injectable.
//!
//! ⚠️ **The "all of them" half is argued, not checked, and saying so is the
//! point.** What a compiler enforces here is that every [`CrashPoint`] has an
//! [`Aftermath`] — add a variant and this file stops building — and what
//! nothing enforces is that a fourth `.await` appearing in `flush` would add a
//! variant. A gate leg counting await points on that function is what would
//! close it; `M10.15` is where such a leg would live, and until then this
//! sentence is the record that it does not exist.
//!
//! ⚠️ **Three windows, five enumerated faults, and the difference matters.**
//! The middle window has *three* causes a client can tell apart, not by luck
//! but because each leaves something different behind: the store lost the
//! acknowledgement after a durable write (`FlushError::Store`, answered
//! `NOT_ENOUGH_REPLICAS`), the journal refused the position
//! (`FlushError::Commit`, answered `LEADER_NOT_AVAILABLE`), and the broker
//! itself is gone before either of those — no code at all (`M10.30`). ⚠️
//! **This file enumerated only the first of the middle window's causes for a
//! round**, which would have handed `M10.15` a leg asserting a code the
//! broker does not send for FR-10's canonical crash — the one
//! `crash_after_put_before_ack` produces and `m3-complete.sh`'s FR-10 leg
//! already runs. ⚠️ **A third cause in an existing window is a different gap
//! from a fourth window**, and `m10-complete.sh`'s own leg only counts
//! `Cluster::flush`'s `.await` points — it would not have caught this one,
//! because nothing about a broker dying between an existing pair of await
//! points changes how many there are.
//!
//! ⚠️ **They are not five severities of the same thing.** They differ in what
//! survives, and a broker that treated any two alike would be right about one
//! of them by luck: nothing; an object nothing references, three times over
//! for three different reasons; and records a client will send again.

#![allow(clippy::expect_used)]
// ⚠️ `pub` here is `pub(crate)` in effect — see `roundtrip.rs`.
#![allow(unreachable_pub)]

mod broker_died;

use crate::roundtrip::{fetch, produce, produce_frame};
use crate::support::{Broker, broker};
use oqueue_broker::{ConnectionLimits, Dispatcher, serve_connection};
use oqueue_codec::error_codes::{LEADER_NOT_AVAILABLE, NOT_ENOUGH_REPLICAS};
use oqueue_core::{FaultConfig, LogFaults, Operation, StormKind};
use std::sync::Arc;
use tokio::io::AsyncWriteExt as _;

/// A window on `Cluster::flush` in which a crash is observable.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CrashPoint {
    /// The PUT never landed.
    BeforePut,
    /// The object is durable and the journal refused its position.
    BetweenPutAndCommitJournalRefused,
    /// The object is durable and the store lost the acknowledgement, so the
    /// broker never reached the commit.
    ///
    /// ⚠️ **The same window as above and a different answer.**
    /// `ADR-0005` guarantee 2's unknown state arrives as `FlushError::Store`,
    /// because from `flush`'s side the PUT failed — the bytes landing anyway
    /// is exactly what the caller cannot know. So the client is told
    /// `NOT_ENOUGH_REPLICAS` and the world is left as if the journal had
    /// refused.
    BetweenPutAndCommitAckLost,
    /// The object is durable and the broker itself is gone before the commit
    /// — a *third* cause of this window, not a fourth `.await` in a new one.
    ///
    /// ⚠️ **`M10.30`, from `M10.29`'s own finding that the tree already
    /// produces this.** The two causes above both leave a *living* broker
    /// that answers something — `NOT_ENOUGH_REPLICAS` or
    /// `LEADER_NOT_AVAILABLE`. This one has nobody left to answer at all:
    /// `ADR-0005` guarantee 2's added clause is the `put` future dropped
    /// after the object landed, and here the broker's own handler task is
    /// what gets dropped, aborted mid-flight the way a process death would
    /// leave it. `error_code: None` for the same reason `AfterCommitBeforeAck`
    /// has one — no reply is ever built — but `position_journalled: false`
    /// is what tells the two apart: this window is before the commit, that
    /// one is after.
    BetweenPutAndCommitBrokerDied,
    /// The position is journalled and the client never heard.
    AfterCommitBeforeAck,
}

/// What the world holds once the dust settles.
///
/// ⚠️ **Every field is *observed*, and one of them was not.** `acknowledged`
/// began as `error_code == 0 && point != AfterCommitBeforeAck`, which made the
/// third point's whole claim true by construction — review measured it: a
/// broker answering `LEADER_NOT_AVAILABLE` to every durable, committed produce
/// left that case green. A field the case computes from the answer it wants is
/// not evidence, and this file is the enumeration everything else about the
/// write path will be checked against.
#[derive(Debug, PartialEq, Eq)]
struct Aftermath {
    /// The Kafka error code the client was handed, or `None` if the client
    /// was gone before there was one.
    ///
    /// ⚠️ **The code, not a boolean**, because the first two points differ in
    /// exactly this and nothing else the client can see: `answer.rs` maps a
    /// failed PUT to `NOT_ENOUGH_REPLICAS` and a refused commit to
    /// `LEADER_NOT_AVAILABLE`, and its own doc says a broker answering them
    /// alike would be right about one by luck. A boolean erased that.
    ///
    /// ⚠️ **`Option`, so the third point's answer is *absent* rather than
    /// asserted.** A `bool` beside a code let the case decide what "the client
    /// never heard" meant; a code that does not exist because the caller was
    /// dropped mid-flight is the observation itself.
    error_code: Option<i16>,
    /// Whether the bytes reached object storage at all.
    object_written: bool,
    /// Whether anything was journalled — a position naming an object.
    position_journalled: bool,
    /// Whether a reader can see the records afterwards.
    records_readable: bool,
}

impl CrashPoint {
    /// ⚠️ **The exhaustive match is the enumeration's only enforced half.** A
    /// fourth variant does not compile until it says what it leaves behind.
    const fn expected(self) -> Aftermath {
        match self {
            Self::BeforePut => Aftermath {
                error_code: Some(NOT_ENOUGH_REPLICAS),
                object_written: false,
                position_journalled: false,
                records_readable: false,
            },
            Self::BetweenPutAndCommitJournalRefused => Aftermath {
                error_code: Some(LEADER_NOT_AVAILABLE),
                object_written: true,
                position_journalled: false,
                records_readable: false,
            },
            Self::BetweenPutAndCommitAckLost => Aftermath {
                error_code: Some(NOT_ENOUGH_REPLICAS),
                object_written: true,
                position_journalled: false,
                records_readable: false,
            },
            Self::BetweenPutAndCommitBrokerDied => Aftermath {
                // ⚠️ **No code, definitional rather than measured, the same
                // way `AfterCommitBeforeAck`'s is** — the handler task is
                // aborted, so nothing here builds a reply to *not* answer
                // wrongly. What distinguishes this from that point is
                // `position_journalled: false`.
                error_code: None,
                object_written: true,
                position_journalled: false,
                records_readable: false,
            },
            Self::AfterCommitBeforeAck => Aftermath {
                // ⚠️ **No code at all**, and ⚠️ **this one is definitional
                // rather than measured**, which is worth saying where the other
                // two are the opposite: a client that was dropped unread has no
                // code by construction, so nothing here would notice a broker
                // that answered a durable, committed produce with an error. The
                // code on that path is pinned by
                // `the_retry_after_a_lost_ack_duplicates_the_records` below and
                // by `roundtrip.rs`, which is where that coverage is and where
                // review's own mutation of `answer.rs` fails.
                error_code: None,
                object_written: true,
                position_journalled: true,
                records_readable: true,
            },
        }
    }
}

/// Applies `point`'s fault, produces, and returns the code the client got.
///
/// ⚠️ **Its own function because `crash_at` outgrew fifty lines** the moment
/// the middle window turned out to have two causes — `code-structure.md`'s
/// design signal, and here it is a real seam: injecting is one concern and
/// reading the aftermath is another.
async fn inject_and_produce(point: CrashPoint, broker: &Broker) -> Option<i16> {
    let dispatcher = Dispatcher::new(Arc::clone(&broker.cluster));
    match point {
        CrashPoint::BeforePut => {
            broker.store.inner().set_faults(FaultConfig {
                storm: Some((StormKind::Transient, 1)),
                ..FaultConfig::default()
            });
            Some(
                produce(&dispatcher, broker, &["orders"]).await.responses[0].partition_responses[0]
                    .error_code,
            )
        }
        CrashPoint::BetweenPutAndCommitJournalRefused => {
            broker.log.set_faults(LogFaults {
                refuse_append: true,
                ..LogFaults::default()
            });
            Some(
                produce(&dispatcher, broker, &["orders"]).await.responses[0].partition_responses[0]
                    .error_code,
            )
        }
        CrashPoint::BetweenPutAndCommitAckLost => {
            broker.store.inner().set_faults(FaultConfig {
                crash_after_put_before_ack: 1,
                ..FaultConfig::default()
            });
            Some(
                produce(&dispatcher, broker, &["orders"]).await.responses[0].partition_responses[0]
                    .error_code,
            )
        }
        // ⚠️ **Not dispatched here.** This point needs a store that can hang
        // a `put` after it writes, which `broker`'s own `FakeObjectStore` has
        // no fault for — see `crash_the_broker_after_the_put_lands`'s own doc
        // for why that fault does not live there. `crash_at` special-cases
        // this variant before it ever reaches this function.
        CrashPoint::BetweenPutAndCommitBrokerDied => {
            unreachable!("crash_at handles this point directly, on its own cluster")
        }
        CrashPoint::AfterCommitBeforeAck => {
            kill_the_client_once_committed(broker).await;
            None
        }
    }
}

/// Produces one batch into `topic` under `point`'s fault and reports what is
/// left.
///
/// ⚠️ **The code is what the *client* got**, so the last point reports
/// `error_code: None` even though the broker built a successful reply: the
/// window is the reply not arriving. That is the whole difficulty of that
/// point and the reason it is not one of the middle two.
async fn crash_at(point: CrashPoint, broker: &Broker) -> Aftermath {
    // ⚠️ **Its own cluster, checked before anything below.** Every other
    // point reads `broker`'s own store and log afterwards; this one's
    // injection cannot use `broker`'s store at all (see
    // `crash_the_broker_after_the_put_lands`'s doc), so it builds and
    // inspects a wholly separate one and returns without touching `broker`.
    if point == CrashPoint::BetweenPutAndCommitBrokerDied {
        return broker_died::crash_the_broker_after_the_put_lands().await;
    }
    let dispatcher = Dispatcher::new(Arc::clone(&broker.cluster));
    let error_code = inject_and_produce(point, broker).await;

    broker.store.inner().set_faults(FaultConfig::default());
    broker.log.set_faults(LogFaults::default());

    // ⚠️ **The store's contents, not its call count.** `CountingObjectStore`
    // counts calls rather than successes, so a `put` the storm refused still
    // increments it — the first version of this line read `count(Put) > 0` and
    // reported an object written for the crash point whose whole subject is
    // that none was.
    let object_written = !broker.store.inner().is_empty();
    // ⚠️ **Asked separately from `records_readable`**, because a fetch that
    // *errors* also reads as "no records" — so a `flush` that journalled a
    // position before writing the object would look like "nothing survived"
    // to the read path alone, which is doc 13 §8's inversion passing as the
    // easy case.
    let position_journalled = !broker.log.inner().is_empty();
    let readable = fetch(&dispatcher, broker, "orders", 0).await;
    let records_readable = readable.responses[0].partitions[0]
        .records
        .as_ref()
        .is_some_and(|r| !r.is_empty());

    Aftermath {
        error_code,
        object_written,
        position_journalled,
        records_readable,
    }
}

/// Sends a produce over a real connection and kills the client the moment its
/// position is journalled, before it has read anything back.
///
/// ⚠️ **This is the injection, and there was none before.** Review measured
/// that the third point asserted its own conclusion: nothing was injected and
/// `acknowledged` was reduced to `false` by the point's own name.
///
/// ⚠️ **And the window is not where the first attempt at this looked.** Driving
/// the *request future* by hand and dropping it after the commit cannot reach
/// it: `flush` awaits the commit and then builds the answer with no suspension
/// point in between, so the produce goes from "commit pending" to "answered"
/// inside one poll — measured, as a failing assertion, before this was
/// rewritten. ⚠️ **So the third window is the delivery and not the broker**:
/// the answer exists and the client is gone, which is `connection.rs`'s write
/// and not `flush.rs`'s.
///
/// ⚠️ **What is observed is that the client never *read* an answer**, which is
/// the whole of what a dead client sees. Whether the bytes reached the pipe's
/// buffer before the socket died is not the property and is not asserted.
async fn kill_the_client_once_committed(broker: &Broker) {
    let (mut client, server) = tokio::io::duplex(4096);
    let dispatcher = Arc::new(Dispatcher::new(Arc::clone(&broker.cluster)));
    let conn = tokio::spawn(serve_connection(server, dispatcher, LIMITS));

    let body = produce_frame(broker, "orders");
    let mut framed = Vec::new();
    framed.extend_from_slice(
        &i32::try_from(body.len())
            .expect("a small frame")
            .to_be_bytes(),
    );
    framed.extend_from_slice(&body);
    client.write_all(&framed).await.expect("the write succeeds");

    for _ in 0..POLL_BUDGET {
        if !broker.log.inner().is_empty() {
            drop(client);
            let _ = conn.await;
            return;
        }
        tokio::task::yield_now().await;
    }
    panic!("the commit never landed within {POLL_BUDGET} yields");
}

/// How many yields the loop above will spend before calling it a hang.
///
/// ⚠️ **A budget rather than an unbounded loop**, so a change that stops the
/// commit landing fails this file instead of hanging the suite inside
/// `check-budget.sh`'s ten-second ceiling.
const POLL_BUDGET: usize = 1_000;

/// ⚠️ **An hour, so nothing here depends on a timer.** The connection under
/// test is killed by the client going away, never by an idle deadline.
const LIMITS: ConnectionLimits = ConnectionLimits {
    max_frame: 1 << 20,
    max_in_flight: 8,
    idle_timeout: core::time::Duration::from_hours(1),
};

/// ⚠️ **Nothing lands, and the client is told so.** The easy case, and the one
/// a harness built for kills would be complete against.
#[tokio::test]
async fn a_crash_before_the_put_leaves_nothing_and_acknowledges_nothing() {
    let broker = broker(&["orders"]).await;
    assert_eq!(
        crash_at(CrashPoint::BeforePut, &broker).await,
        CrashPoint::BeforePut.expected()
    );
}

/// ⚠️ **An object nothing references — garbage, not corruption**, which is
/// what `flush`'s own doc says and what `M5`'s reaper collects. The commit is
/// refused *by a living coordinator*, which is the shape `M10.7`'s review
/// routed here: a dead loop and a refused journal are not the same fault, and
/// only the second reaches `serve.rs`'s `CoordinatorError::Journal`.
#[tokio::test]
async fn a_refused_journal_between_the_put_and_the_commit_leaves_an_unreferenced_object() {
    let broker = broker(&["orders"]).await;
    assert_eq!(
        crash_at(CrashPoint::BetweenPutAndCommitJournalRefused, &broker).await,
        CrashPoint::BetweenPutAndCommitJournalRefused.expected()
    );
}

/// ⚠️ **What this measures is that a produce whose client dies is still
/// durable, journalled and readable** — FR-10's converse, which it does not
/// claim: durable does not mean acknowledged, and the cost is a duplicate
/// rather than a loss.
///
/// ⚠️ **And what it does not measure is the kill.** Review mutated the case two
/// ways — reading the whole reply before dropping, and dropping immediately
/// instead of once committed — and it stayed green both times, because
/// in-flight handlers are not aborted by a client going away and the asserted
/// aftermath is a successful produce's. So the machinery below *reaches* the
/// window and no assertion here depends on it; the assertions that do are
/// `the_retry_after_a_lost_ack_duplicates_the_records` and `roundtrip.rs`.
/// ⚠️ **Recorded rather than dressed up**, because the alternative is a case
/// that reads as constraining three points when it constrains two.
#[tokio::test]
async fn a_crash_after_the_commit_leaves_records_the_client_never_heard_about() {
    let broker = broker(&["orders"]).await;
    assert_eq!(
        crash_at(CrashPoint::AfterCommitBeforeAck, &broker).await,
        CrashPoint::AfterCommitBeforeAck.expected()
    );
}

/// ⚠️ **What the third point actually costs, made visible.** A client that
/// never saw its ack retries, and the write path is not idempotent — so the
/// records arrive twice and both are readable. `M11`'s idempotent produce is
/// what closes this, and until then it is a property rather than a bug, which
/// is worth pinning so that closing it is a change someone notices.
#[tokio::test]
async fn the_retry_after_a_lost_ack_duplicates_the_records() {
    let broker = broker(&["orders"]).await;
    let dispatcher = Dispatcher::new(Arc::clone(&broker.cluster));

    let first = produce(&dispatcher, &broker, &["orders"]).await;
    assert_eq!(first.responses[0].partition_responses[0].error_code, 0);
    // The ack is lost here: the client saw nothing, so it sends the same
    // batch again.
    let second = produce(&dispatcher, &broker, &["orders"]).await;
    assert_eq!(second.responses[0].partition_responses[0].error_code, 0);

    assert_ne!(
        first.responses[0].partition_responses[0].base_offset,
        second.responses[0].partition_responses[0].base_offset,
        "a retried produce takes new offsets, which is what makes it a duplicate \
         rather than a repeat of the same record"
    );
    assert_eq!(
        broker.store.counts().count(Operation::Put),
        2,
        "two flushes, two objects — the second write is not deduplicated"
    );
}

/// ⚠️ **The same window, the other cause, and a different code on the wire.**
/// `crash_after_put_before_ack` is the fault `M1.8` built and `m3-complete.sh`
/// runs for FR-10, and it reaches the client as `NOT_ENOUGH_REPLICAS` because
/// from `flush`'s side the PUT failed — the bytes landing anyway is precisely
/// what the caller cannot know (`ADR-0005` guarantee 2). An enumeration naming
/// one code for this window would be wrong for the canonical crash.
#[tokio::test]
async fn a_lost_put_ack_leaves_the_same_world_under_a_different_code() {
    let broker = broker(&["orders"]).await;
    assert_eq!(
        crash_at(CrashPoint::BetweenPutAndCommitAckLost, &broker).await,
        CrashPoint::BetweenPutAndCommitAckLost.expected()
    );
}

/// ⚠️ **A third cause in the middle window, not a fourth `.await`** (`M10.30`).
/// The two causes above both leave a living broker that answers something;
/// this one leaves nobody to answer at all, which is what `error_code: None`
/// alongside `position_journalled: false` distinguishes it from both its
/// neighbours in this file.
#[tokio::test]
async fn the_broker_dying_after_the_put_leaves_an_unreferenced_object_and_no_answer() {
    let broker = broker(&["orders"]).await;
    assert_eq!(
        crash_at(CrashPoint::BetweenPutAndCommitBrokerDied, &broker).await,
        CrashPoint::BetweenPutAndCommitBrokerDied.expected()
    );
}
