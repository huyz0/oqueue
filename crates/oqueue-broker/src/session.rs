//! What one connection remembers between requests.
//!
//! ⚠️ **One thing, and it is hazard H2.** A producer that writes and then
//! immediately reads must see its own record. Doc 12 §4.6 calls that
//! read-your-writes and names it the second of the staleness hazards, because
//! the failure is silent: a successful poll returning no records, which every
//! client treats as "nothing was written".
//!
//! The mechanism is `ADR-0023`'s: a [`CommitAck`] carries a
//! [`SessionWatermark`] — a version **and the epoch it belongs to** — the
//! produce path stores it here, and the next fetch on this connection asks the
//! index to be at least that far before it answers.
//!
//! ⚠️ **A version alone would be the bug rather than the fix.** Two
//! incarnations of a coordinator count from their own beginnings, so a
//! watermark compared across a failover is comparing two unrelated counters —
//! and the half of that mistake that feels safe is the half that answers a
//! reader with data missing its own write. `CacheState::admits` refuses the
//! comparison outright when the epochs differ.
//!
//! [`CommitAck`]: oqueue_coordinator::CommitAck

use crate::cluster::Cluster;
use oqueue_coordinator::IndexWatch;
use oqueue_core::{ReadMode, RefreshReason, SessionWatermark};
use std::cmp::Ordering;
use std::sync::{Mutex, PoisonError};
use std::time::Duration;
use tokio::time::Instant;

/// One connection's memory.
///
/// ⚠️ A std `Mutex` (`async-concurrency.md` rules 6 and 8): the critical
/// section is a `Copy` read or write with no `.await` inside, and the lock
/// protects data rather than control flow. Requests on one connection are
/// pipelined and so genuinely concurrent, which is why there is a lock at all.
#[derive(Debug, Default)]
pub struct Session {
    watermark: Mutex<Option<SessionWatermark>>,
}

impl Session {
    /// Remembers `watermark` if it is newer than what this session holds.
    ///
    /// ⚠️ **Monotonic within an epoch, replaced across one.** Pipelined
    /// produces can ack out of order, so a bare store would let an older
    /// watermark overwrite a newer one and drop the freshness bar below a
    /// write this client has already been told about. A watermark from a
    /// *different* epoch is not older or newer — it is incomparable
    /// (`ADR-0023`) — and the newer incarnation is the one that matters, so it
    /// replaces.
    pub fn observe(&self, watermark: SessionWatermark) {
        let mut held = self.lock();
        // ⚠️ **Epoch first and by ordering, not by equality-then-compare.** A
        // version is only meaningful within its own incarnation (`ADR-0023`),
        // so the epochs decide first: an ack from an *older* one is stale
        // whatever its number, a *newer* one replaces whatever is held, and
        // only within one epoch does the version comparison mean anything.
        let keep = held.is_some_and(|current| match current.epoch().cmp(&watermark.epoch()) {
            Ordering::Greater => true,
            Ordering::Less => false,
            Ordering::Equal => current.version() >= watermark.version(),
        });
        if !keep {
            *held = Some(watermark);
        }
    }

    /// The watermark this connection's next read must be at least as fresh as.
    #[must_use]
    pub fn watermark(&self) -> Option<SessionWatermark> {
        *self.lock()
    }

    /// Waits until the index holds whatever this connection has already been
    /// promised.
    ///
    /// ⚠️ **Bounded by the *staleness* budget, not by the client's
    /// `max_wait_ms`.** Read-your-writes is a correctness guarantee, and a
    /// non-blocking poll — `fetch.max.wait.ms = 0`, which every client sends
    /// routinely — would otherwise switch it off entirely: no wait, no
    /// re-check, and a partition answered `NONE` without the client's own
    /// write in it. That is hazard H2 delivered by the mechanism meant to
    /// prevent it. `ADR-0021`'s `MAX_METADATA_STALENESS_MS` is the system's
    /// own bound on how far behind a read may be, and it is the right one
    /// here because the question is how stale this broker may be, not how long
    /// this client is willing to block.
    ///
    /// ⚠️ **The decision is `CacheState::admits`, not a bare version compare**,
    /// and `ADR-0023` is why: a watermark carried across a failover belongs to
    /// another incarnation's counter, so it is not *behind* — it is
    /// incomparable, and comparing it would answer a reader with data missing
    /// its own write while looking satisfied.
    ///
    /// ⚠️ **`wait_for`'s answer is used**, which is the whole reason it returns
    /// a `bool`: "I waited" and "it arrived" are different, and for an
    /// `AtLeast(v)` read the difference is an answer versus hazard H2. Even a
    /// `true` is re-checked, because a version *number* from another epoch can
    /// be reached by this one without meaning anything.
    ///
    /// ⚠️ **In this broker the wait is always already over**, because the
    /// coordinator folds before it acks and the index here is that
    /// coordinator's. It becomes load-bearing when the reader is a different
    /// process (`M7`), and wiring it now is what makes that a configuration
    /// change rather than a correctness change.
    ///
    /// # Errors
    ///
    /// The [`RefreshReason`] that is still unmet when the budget runs out. A
    /// caller must **not** answer records then: a partition served short of a
    /// promise already made is the silent wrongness doc 12 §4.6 names.
    pub(crate) async fn catch_up(
        &self,
        cluster: &Cluster,
        watch: &mut IndexWatch,
    ) -> Result<(), RefreshReason> {
        let Some(want) = self.watermark() else {
            return Ok(());
        };
        let mode = ReadMode::AtLeast(want);
        let refusal = match cluster.cache_state().admits(mode, cluster.epoch()) {
            Ok(()) => return Ok(()),
            Err(reason) => reason,
        };
        let deadline =
            Instant::now() + Duration::from_millis(oqueue_core::MAX_METADATA_STALENESS_MS);
        // ⚠️ `select!` against the budget: a version this coordinator will
        // never reach — one from an incarnation that is gone — must cost a
        // bounded wait and a named failure, never a hang.
        let arrived = tokio::select! {
            () = tokio::time::sleep_until(deadline) => false,
            arrived = watch.wait_for(want.version()) => arrived,
        };
        if !arrived {
            return Err(refusal);
        }
        // ⚠️ **One wait, then one re-check, and no loop.** Reaching the version
        // and still being refused means the number arrived on a different line
        // (`ADR-0023`) or the stream has gone silent — and waiting longer
        // cannot mend either, so a loop here would be a way to spend the whole
        // budget discovering the same thing repeatedly.
        cluster.cache_state().admits(mode, cluster.epoch())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Option<SessionWatermark>> {
        self.watermark
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::Instant;
    use super::Session;
    use oqueue_core::{CommitVersion, CoordinatorEpoch, RefreshReason, SessionWatermark};

    fn watermark(epoch: u64, version: u64) -> SessionWatermark {
        SessionWatermark::new(CoordinatorEpoch::new(epoch), CommitVersion::new(version))
    }

    /// ⚠️ **An older ack must not lower the bar.** Produces on one connection
    /// are pipelined and can ack out of order, so a bare store would let a
    /// late-arriving older watermark drop the freshness requirement below a
    /// write this client has already been told about — read-your-writes lost
    /// to a race the client cannot see.
    #[test]
    fn an_older_watermark_from_the_same_epoch_does_not_replace_a_newer_one() {
        let session = Session::default();
        session.observe(watermark(1, 9));
        session.observe(watermark(1, 4));
        assert_eq!(session.watermark(), Some(watermark(1, 9)));
    }

    #[test]
    fn a_newer_watermark_from_the_same_epoch_does_replace() {
        let session = Session::default();
        session.observe(watermark(1, 4));
        session.observe(watermark(1, 9));
        assert_eq!(session.watermark(), Some(watermark(1, 9)));
    }

    /// ⚠️ **Across epochs there is no older or newer** (`ADR-0023`): two
    /// incarnations count from their own beginnings, so a version comparison
    /// between them is a comparison of unrelated counters. The newer
    /// incarnation is the one this session now talks to, so it replaces —
    /// even when its version number is smaller, which is exactly the case a
    /// version-only rule gets wrong.
    #[test]
    fn a_watermark_from_a_newer_epoch_replaces_a_higher_version_from_an_older_one() {
        let session = Session::default();
        session.observe(watermark(1, 9_000));
        session.observe(watermark(2, 1));
        assert_eq!(session.watermark(), Some(watermark(2, 1)));
    }

    /// And an older *epoch* does not take over from a newer one either.
    #[test]
    fn a_watermark_from_an_older_epoch_is_ignored() {
        let session = Session::default();
        session.observe(watermark(2, 1));
        session.observe(watermark(1, 9_000));
        assert_eq!(session.watermark(), Some(watermark(2, 1)));
    }

    #[test]
    fn a_session_that_has_produced_nothing_requires_nothing() {
        assert_eq!(Session::default().watermark(), None);
    }

    /// ⚠️ **A session with nothing to catch up to does not wait**, which is
    /// the common case and must cost nothing: a consumer that never produced
    /// has no write of its own to read.
    #[tokio::test(start_paused = true)]
    async fn catching_up_to_nothing_returns_at_once() {
        let fixture = crate::testing::fixture(&["t"]).await;
        let mut watch = fixture.cluster.watch();
        let started = Instant::now();

        let outcome = Session::default()
            .catch_up(&fixture.cluster, &mut watch)
            .await;

        assert!(outcome.is_ok());
        assert_eq!(Instant::now() - started, std::time::Duration::ZERO);
    }

    /// ⚠️ **And one whose index is already that far does not wait either.**
    /// In this broker that is every case, because the coordinator folds before
    /// it acks — which is exactly why the wait must be *decided* rather than
    /// skipped: `M7`'s follower reads an index that is a cache.
    #[tokio::test(start_paused = true)]
    async fn catching_up_to_something_already_folded_returns_at_once() {
        let fixture = crate::testing::fixture(&["t"]).await;
        crate::testing::produce_one(&fixture, "t", crate::testing::golden_batch()).await;
        let session = Session::default();
        let applied = fixture
            .cluster
            .watch()
            .applied()
            .expect("the produce was folded before it acked");
        session.observe(SessionWatermark::new(fixture.cluster.epoch(), applied));
        let mut watch = fixture.cluster.watch();
        let started = Instant::now();

        let outcome = session.catch_up(&fixture.cluster, &mut watch).await;

        assert!(outcome.is_ok(), "already folded: nothing to wait for");
        assert_eq!(Instant::now() - started, std::time::Duration::ZERO);
    }

    /// ⚠️ **A watermark this coordinator will never reach costs the staleness
    /// budget and then says so.** A session carried across a failover holds a
    /// version from an incarnation that is gone; `ADR-0023` says that is
    /// incomparable rather than behind, so there is nothing to wait *for* — a
    /// wait with no bound would hold the connection until the client gave up,
    /// and a *silent* return would hand it records missing its own write.
    #[tokio::test(start_paused = true)]
    async fn a_watermark_that_can_never_arrive_costs_the_budget_and_is_reported() {
        let fixture = crate::testing::fixture(&["t"]).await;
        let session = Session::default();
        session.observe(SessionWatermark::new(
            fixture.cluster.epoch(),
            CommitVersion::new(9_999),
        ));
        let mut watch = fixture.cluster.watch();
        let started = Instant::now();

        let outcome = session.catch_up(&fixture.cluster, &mut watch).await;

        assert!(
            matches!(outcome, Err(RefreshReason::NotYetVisible { .. })),
            "an unmet promise is named, not swallowed: {outcome:?}"
        );
        assert_eq!(
            Instant::now() - started,
            std::time::Duration::from_millis(oqueue_core::MAX_METADATA_STALENESS_MS),
            "bounded by the staleness budget, not by any client's patience"
        );
    }

    /// ⚠️ **And the positive case: a promise not yet met is *waited for*, and
    /// the commit that meets it releases the wait.** This is the behaviour
    /// `catch_up` exists for, and the one every other test here approaches
    /// from the outside — without it, deleting the wait entirely would leave
    /// the suite green.
    #[tokio::test(start_paused = true)]
    async fn a_promise_the_index_has_not_reached_is_waited_for_and_then_met() {
        let fixture = std::sync::Arc::new(crate::testing::fixture(&["t"]).await);
        crate::testing::produce_one(&fixture, "t", crate::testing::golden_batch()).await;
        let applied = fixture
            .cluster
            .watch()
            .applied()
            .expect("the first produce folded");
        let session = Session::default();
        // One past what the index holds: exactly what an ack from a commit
        // this reader has not seen yet would carry.
        session.observe(SessionWatermark::new(
            fixture.cluster.epoch(),
            applied.advance(1).expect("a small version"),
        ));

        let waiter = {
            let fixture = std::sync::Arc::clone(&fixture);
            tokio::spawn(async move {
                let mut watch = fixture.cluster.watch();
                session.catch_up(&fixture.cluster, &mut watch).await
            })
        };
        // Under paused time the clock only advances when every task is idle,
        // so this resolving is the proof that the waiter reached its wait.
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        assert!(!waiter.is_finished(), "the promise is not met yet");

        crate::testing::produce_one(&fixture, "t", crate::testing::golden_batch()).await;

        assert!(
            waiter.await.expect("the waiter joins").is_ok(),
            "the commit that meets the promise releases the wait"
        );
    }

    /// ⚠️ **A fetch that cannot keep this connection's promise refuses, and
    /// does not answer an empty partition.** This is hazard H2's failure mode:
    /// a successful poll with no records is what every client reads as
    /// "nothing was written". The session is set one version past what the
    /// index holds — which is what an ack from a commit this reader has not
    /// seen would carry — so the wait cannot be met.
    ///
    /// ⚠️ **`max_wait_ms` is zero on purpose.** Read-your-writes is not the
    /// client's to switch off with a non-blocking poll, and a `catch_up` bound
    /// by the fetch deadline would do nothing at all here.
    #[tokio::test(start_paused = true)]
    async fn a_fetch_that_cannot_meet_this_sessions_promise_refuses_rather_than_answering_empty() {
        let fixture = crate::testing::fixture(&["t"]).await;
        crate::testing::produce_one(&fixture, "t", crate::testing::golden_batch()).await;
        let applied = fixture
            .cluster
            .watch()
            .applied()
            .expect("the produce folded");
        fixture.session.observe(SessionWatermark::new(
            fixture.cluster.epoch(),
            applied.advance(1).expect("a small version"),
        ));
        let started = Instant::now();

        let response = crate::fetch::tests::replied(
            &fixture,
            13,
            &crate::fetch::tests::fetch_body(13, crate::fetch::tests::by_id(&fixture), 0, 0),
        )
        .await;

        let p = &response.responses[0].partitions[0];
        assert_eq!(
            p.error_code,
            kafka_protocol::error::ResponseError::OffsetNotAvailable.code(),
            "a promise this broker cannot keep is an error, never empty records"
        );
        assert!(p.records.as_ref().is_none_or(bytes::Bytes::is_empty));
        assert_eq!(
            p.high_watermark, 2,
            "and the client still hears where the log is"
        );
        // ⚠️ **`-1`, not `0`.** This broker did not read the partition, so it
        // has no stable or start offset to report — and a `0` is the doc 13 §8
        // inversion: a plausible-looking value on an error path.
        assert_eq!(p.last_stable_offset, -1);
        assert_eq!(p.log_start_offset, -1);
        assert_eq!(
            Instant::now() - started,
            std::time::Duration::from_millis(oqueue_core::MAX_METADATA_STALENESS_MS),
            "it waited the staleness budget, not the client's zero"
        );
    }

    /// ⚠️ **A refusal is addressed the way the request was.** Below v13 the
    /// wire carries names, so the response must echo the name back; a refusal
    /// that dropped it would leave the client unable to tell which topic it
    /// was about — and a client that cannot attribute an error retries the
    /// wrong thing.
    #[tokio::test(start_paused = true)]
    async fn a_refusal_below_v13_still_names_its_topic() {
        let fixture = crate::testing::fixture(&["t"]).await;
        crate::testing::produce_one(&fixture, "t", crate::testing::golden_batch()).await;
        let applied = fixture
            .cluster
            .watch()
            .applied()
            .expect("the produce folded");
        fixture.session.observe(SessionWatermark::new(
            fixture.cluster.epoch(),
            applied.advance(1).expect("a small version"),
        ));

        let by_name = crate::fetch::tests::replied(
            &fixture,
            11,
            &crate::fetch::tests::fetch_body(11, by_name_topic("t"), 0, 0),
        )
        .await;

        assert_eq!(
            by_name.responses[0].topic.0.as_str(),
            "t",
            "a refusal below v13 still names its topic"
        );
        assert_eq!(
            by_name.responses[0].partitions[0].error_code,
            kafka_protocol::error::ResponseError::OffsetNotAvailable.code()
        );
    }

    /// A `FetchTopic` addressed the pre-v13 way, by name.
    fn by_name_topic(name: &str) -> kafka_protocol::messages::fetch_request::FetchTopic {
        let mut topic = kafka_protocol::messages::fetch_request::FetchTopic::default();
        topic.topic = kafka_protocol::messages::TopicName(
            kafka_protocol::protocol::StrBytes::from_string(name.to_owned()),
        );
        topic
    }
}
