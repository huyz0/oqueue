//! Push for the tail: what a parked reader waits on, and what a follower gets.
//!
//! `M3.9`, and doc 12 §4.4's already-settled split — push the tail, pull the
//! history. What is tested here is the property `M3.md` task 17 is really
//! about: a fetch racing a produce is woken **by the commit**, not by a poll
//! interval, and it is woken only once the index can actually answer it.

// Every `expect` is on a value the suite itself built, or on a step whose
// failure *is* the test failing.
#![allow(clippy::expect_used)]

use crate::support::{object, parked, partition, span, start_indexed, topic};
use oqueue_core::{
    CacheState, CommitVersion, MAX_METADATA_STALENESS_MS, Offset, ReadMode, RefreshReason,
};

/// ⚠️ The read-your-writes case (hazard H2) with **no wait at all**: by the
/// time a producer holds its ack, the coordinator's index has already folded
/// the record. A design that folded after acking would leave a window in which
/// the position is durable and unqueryable, which is the same silent wrongness
/// from the other direction.
#[tokio::test]
async fn an_ack_implies_the_index_can_already_answer_for_it() {
    let (coordinator, index, driver) = start_indexed().await;

    let ack = coordinator
        .commit(object(0), vec![span(4)])
        .await
        .expect("the commit lands");

    assert_eq!(index.applied_upto(), Some(ack.version()));
    assert_eq!(
        index.end_offset(&topic(), partition()),
        Offset::new(4).expect("a valid offset"),
        "the high watermark moved before the producer was told its offset"
    );

    drop(coordinator);
    driver.await.expect("the loop ends");
}

/// FR-12's zero-GET case at the seam that decides it: a reader already at the
/// high watermark is told to read nothing, so there is nothing to GET.
#[tokio::test]
async fn a_reader_at_the_high_watermark_is_given_no_object_to_read() {
    let (coordinator, index, driver) = start_indexed().await;
    coordinator
        .commit(object(0), vec![span(3)])
        .await
        .expect("the commit lands");

    let hwm = index.end_offset(&topic(), partition());
    assert!(
        index
            .find_batches(&topic(), partition(), hwm, u64::MAX)
            .expect("a page")
            .is_empty()
    );

    drop(coordinator);
    driver.await.expect("the loop ends");
}

/// ⚠️ **The park, and the whole of `M3.md` task 17.** The waiter is parked
/// before the produce exists; nothing polls; the commit is what wakes it. A
/// poll-interval design would pass this test only by waiting — which is why
/// there is no sleep anywhere in it, and why `tokio::join!` rather than a
/// timeout is what proves the wakeup happened.
#[tokio::test(start_paused = true)]
async fn a_reader_parked_ahead_of_the_log_is_woken_by_the_commit() {
    let (coordinator, index, driver) = start_indexed().await;
    let mut watch = coordinator.watch();
    assert_eq!(watch.applied(), None, "nothing has been folded yet");

    let producer = coordinator.clone();
    let ((), parked) = tokio::join!(
        async move {
            producer
                .commit(object(0), vec![span(2)])
                .await
                .expect("the commit lands");
        },
        async {
            // Version 0 does not exist yet when this future is created.
            assert!(
                parked(watch.wait_for(CommitVersion::ZERO)).await,
                "the wait ended because the version arrived, not because the \
                 coordinator stopped"
            );
            index.end_offset(&topic(), partition())
        }
    );

    assert_eq!(
        parked,
        Offset::new(2).expect("a valid offset"),
        "woken only once the index could answer, never merely once the log \
         held the entry"
    );

    drop(coordinator);
    driver.await.expect("the loop ends");
}

/// A version already folded resolves without waiting for a further change —
/// the level-triggered half, and what stops a fetch for an old offset from
/// parking until its deadline.
#[tokio::test(start_paused = true)]
async fn a_reader_parked_behind_the_index_is_not_made_to_wait() {
    let (coordinator, _index, driver) = start_indexed().await;
    coordinator
        .commit(object(0), vec![span(1)])
        .await
        .expect("the commit lands");
    coordinator
        .commit(object(1), vec![span(1)])
        .await
        .expect("the commit lands");

    let mut watch = coordinator.watch();
    // No further commit will ever happen, so this resolving at all is the
    // assertion.
    assert!(parked(watch.wait_for(CommitVersion::ZERO)).await);
    assert_eq!(watch.applied(), Some(CommitVersion::new(1)));

    drop(coordinator);
    driver.await.expect("the loop ends");
}

/// ⚠️ A wait that ends because the coordinator stopped says so, rather than
/// letting a caller read "I waited" as "it arrived" — which for an
/// `AtLeast(v)` read is hazard H2 with extra steps.
#[tokio::test(start_paused = true)]
async fn a_wait_the_coordinator_outlives_reports_that_it_did_not_arrive() {
    let (coordinator, _index, driver) = start_indexed().await;
    let mut watch = coordinator.watch();
    drop(coordinator);
    driver.await.expect("the loop ends");

    assert!(!parked(watch.wait_for(CommitVersion::new(7))).await);
    assert_eq!(watch.applied(), None, "and nothing was ever folded");
}

/// ⚠️ **What a session carries is the pair, not the number** (`ADR-0023`).
/// A reader that remembered only the version and met a coordinator from
/// another incarnation would compare across two independent counters, and the
/// half of that mistake which feels safe is the half that answers it with data
/// missing its own write.
#[tokio::test(start_paused = true)]
async fn an_ack_hands_back_a_watermark_the_cache_can_be_asked_about() {
    let (coordinator, index, driver) = start_indexed().await;

    let ack = coordinator
        .commit(object(0), vec![span(2)])
        .await
        .expect("the commit lands");
    let watermark = ack.watermark();
    assert_eq!(watermark.epoch(), coordinator.epoch());
    assert_eq!(watermark.version(), ack.version());

    // The coordinator's own index has already folded it, so read-your-writes
    // is admitted with no round trip at all.
    let cache = CacheState::new(coordinator.epoch(), index.applied_upto(), 0);
    assert_eq!(
        cache.admits(ReadMode::AtLeast(watermark), coordinator.epoch()),
        Ok(())
    );

    // A cache that has heard nothing for longer than the limit is refused
    // whatever its version says — the breaker is not a freshness clause.
    let silent = CacheState::new(
        coordinator.epoch(),
        index.applied_upto(),
        MAX_METADATA_STALENESS_MS + 1,
    );
    assert_eq!(
        silent.admits(ReadMode::AtLeast(watermark), coordinator.epoch()),
        Err(RefreshReason::PushStreamSilent {
            silent_for_ms: MAX_METADATA_STALENESS_MS + 1,
            limit_ms: MAX_METADATA_STALENESS_MS,
        })
    );

    drop(coordinator);
    driver.await.expect("the loop ends");
}

/// ⚠️ **What a long-poll fetch waits on**, and it is a different question from
/// [`IndexWatch::wait_for`]'s. A fetch at the high watermark cannot name the
/// version it is waiting for — it wants whatever comes next — so `wait_past`
/// takes what the caller has already seen and resolves on anything beyond it.
#[tokio::test(start_paused = true)]
async fn a_wait_past_resolves_on_the_next_commit_whatever_it_turns_out_to_be() {
    let (coordinator, _index, driver) = start_indexed().await;
    let mut watch = coordinator.watch();
    assert_eq!(watch.applied(), None, "nothing folded yet");

    let waiter = {
        let mut watch = coordinator.watch();
        tokio::spawn(async move { watch.wait_past(None).await })
    };
    tokio::task::yield_now().await;
    assert!(!waiter.is_finished(), "nothing has been committed yet");

    coordinator
        .commit(object(0), vec![span(1)])
        .await
        .expect("the commit lands");

    assert!(
        waiter.await.expect("the waiter joins"),
        "woken by the commit"
    );
    assert!(parked(watch.wait_past(None)).await);
    assert_eq!(watch.applied(), Some(CommitVersion::ZERO));

    drop(coordinator);
    driver.await.expect("the loop ends");
}

/// ⚠️ **Strictly past, not "at least".** A caller hands in what it has already
/// seen; resolving on that same version would wake a fetch that has nothing
/// new to read, and a handler that re-parked immediately would spin.
#[tokio::test(start_paused = true)]
async fn a_wait_past_the_version_already_seen_does_not_resolve_on_it() {
    let (coordinator, _index, driver) = start_indexed().await;
    coordinator
        .commit(object(0), vec![span(1)])
        .await
        .expect("the first commit lands");
    let watch = coordinator.watch();
    let seen = watch.applied().expect("something was folded");

    let waiter = {
        let mut watch = coordinator.watch();
        tokio::spawn(async move { watch.wait_past(Some(seen)).await })
    };
    tokio::task::yield_now().await;
    assert!(
        !waiter.is_finished(),
        "the version it has already seen must not wake it"
    );

    coordinator
        .commit(object(1), vec![span(1)])
        .await
        .expect("the second commit lands");

    assert!(parked(waiter).await.expect("the waiter joins"));
    assert_eq!(watch.applied(), Some(CommitVersion::new(1)));

    drop(coordinator);
    driver.await.expect("the loop ends");
}

/// ⚠️ **A dead coordinator answers `false` rather than parking forever**, the
/// same way [`IndexWatch::wait_for`] does and for the same reason: a fetch
/// answers from what the index already holds rather than waiting out a
/// deadline nothing can satisfy.
#[tokio::test(start_paused = true)]
async fn a_wait_past_a_stopped_coordinator_reports_that_nothing_arrived() {
    let (coordinator, _index, driver) = start_indexed().await;
    let mut watch = coordinator.watch();
    drop(coordinator);
    driver.await.expect("the loop ends");

    assert!(!parked(watch.wait_past(None)).await);
    assert!(!parked(watch.wait_past(Some(CommitVersion::ZERO))).await);
}
