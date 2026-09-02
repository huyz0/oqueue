//! The park: how long a `Fetch` waits, and what makes it worth answering.
//!
//! ⚠️ **Its own module because waiting is a different question from
//! answering.** `mod.rs` walks a request and shapes a response; everything
//! here is about *when* — the client's deadline, the byte target it named, and
//! the one wakeup the broker contributes instead of a poll interval of its own
//! (`M3.md` task 17).
//!
//! ⚠️ **The cost argument lives here too.** One watch serves every partition
//! on a shard, so most wakeups belong to somebody else's commit — and a
//! handler that re-read on every one of them would turn a single parked fetch
//! into an object-storage read per shard commit, each of which can be a
//! whole-object GET on the history tier. A client's own request rate would
//! stop bounding the broker's read volume. Watermarks are checked first
//! because they are index lookups.

// ⚠️ `pub(crate)` inside a private module: `unreachable_pub` denies the `pub`
// clippy's `redundant_pub_crate` asks for.
#![allow(clippy::redundant_pub_crate)]

use super::deadline::park_ms;
use super::partition;
use super::target::Budget;
use super::target::{MAX_READS_PER_REQUEST, satisfiable_min_bytes, worth_answering};
use super::{TopicOutcome, read_all, watermarks};
use crate::cluster::Cluster;
use crate::read::FetchedObjects;
use crate::session::Session;
use oqueue_codec::error_codes;
use std::time::Duration;
use tokio::time::Instant;

/// Reads every requested partition, parking until the request is worth
/// answering or its own deadline expires.
///
/// ⚠️ **The deadline is the client's and so is the clock.** `M3.md` task 17
/// asks for the wakeup to cost about a millisecond and to introduce no new
/// semantics; a poll interval of the broker's own would be a new semantic and
/// a worse one, so what this waits on is the coordinator's index advancing.
///
/// ⚠️ **Re-read after every wakeup, never patched.** One index serves every
/// partition on a shard, so a wakeup may be for a partition this request never
/// asked about — the only way to know is to look again, and looking again is
/// cheap precisely because a fetch at the watermark issues no GETs.
///
/// ⚠️ **`min_bytes` is met by the *whole response*, not per partition** —
/// which is what the protocol says and what a client sizing one round trip
/// expects. A refusal counts as worth answering however few bytes it carries:
/// holding an `UNKNOWN_TOPIC_ID` for half a second would delay the error a
/// client needs to act on.
pub(crate) async fn read_or_park(
    cluster: &Cluster,
    session: &Session,
    request: &oqueue_codec::fetch::FetchRequest<'_>,
    version: i16,
    authz: &crate::authz::AuthzContext<'_>,
) -> Vec<TopicOutcome> {
    // ⚠️ **A request naming nothing is answered, not parked on.** There is no
    // partition whose commit could satisfy it, so waiting is waiting for
    // something that cannot happen — Kafka's own broker short-circuits the
    // same case.
    if request
        .topics
        .iter()
        .all(|topic| topic.partitions.is_empty())
    {
        return Vec::new();
    }
    let mut watch = cluster.watch();
    let deadline = Instant::now() + Duration::from_millis(park_ms(request.max_wait_ms));
    // ⚠️ **Read-your-writes, before anything is read** — hazard H2, and
    // `Session::catch_up` carries the argument. ⚠️ **A failure is answered as
    // one**: a partition served short of a promise this connection has already
    // been given is the silent wrongness doc 12 §4.6 names, so the client is
    // told to go to `Metadata` and come back rather than being handed records
    // that are missing its own write.
    // ⚠️ **An error, not an empty partition.** An empty partition with
    // `error_code` zero is what a client reads as "nothing was written" —
    // exactly the outcome read-your-writes exists to prevent.
    //
    // ⚠️ **`OFFSET_NOT_AVAILABLE`, and the choice is about what clients *do*.**
    // Kafka defines code 78 for a partition whose offsets are not readable
    // yet, which is this situation exactly, and the Java consumer's fetch
    // error dispatch enumerates it and retries. `LEADER_NOT_AVAILABLE` is not
    // in that set: it falls through to an `IllegalStateException` out of
    // `poll()`, so a refusal meant to protect a client would kill it.
    if session.catch_up(cluster, &mut watch).await.is_err() {
        return partition::refuse_all(cluster, request, version, error_codes::OFFSET_NOT_AVAILABLE);
    }
    // ⚠️ Clamped against what a response can hold, which is the request's own
    // `max_bytes`: a `min_bytes` above it is never reachable, and a park
    // waiting for it would run to the deadline on a full log.
    let target = satisfiable_min_bytes(request.min_bytes, &Budget::new(request.max_bytes));
    // ⚠️ Counted up rather than down from a decremented ceiling: the bound is
    // "this many reads", and the test below pins that number exactly.
    let mut reads: u32 = 0;
    // ⚠️ **One object cache for the whole request, every pass included.** It is
    // what bounds the request's GETs — nothing dedups a client's partition
    // list, so the same partition named two hundred times, or four partitions
    // sharing a bundle, must cost the objects behind them once. ⚠️ **Per
    // request rather than per pass**, because a parked fetch reads up to
    // [`MAX_READS_PER_REQUEST`] times over the same offsets: a fresh cache each
    // pass would re-download every bundle per wakeup and turn
    // `MAX_FAILED_FETCHES_PER_REQUEST` into two failures *per pass*. The byte
    // budget is the other way round and `read_all` says why.
    let mut objects = FetchedObjects::default();
    loop {
        // ⚠️ **Both samples come *before* the read, and that ordering is the
        // whole correctness of the park.** `read_all` awaits object-storage
        // GETs, so it can take tens of milliseconds; a commit landing during
        // it would, if the baseline were taken afterwards, already be *in* the
        // baseline — no watermark difference to notice and no version left for
        // `wait_past` to resolve on. The fetch would park on outcomes read
        // before the commit and answer empty at its deadline, for a record
        // that was in the index the whole time.
        let before = watermarks(cluster, request, version);
        let mut applied = watch.applied();
        let outcomes = read_all(cluster, request, version, &mut objects, authz).await;
        reads += 1;
        // ⚠️ **A pass that spent its whole budget is *not* a reason to stop**,
        // and `M3.26` shipped a version that thought it was. The argument was
        // that re-reading the same offsets returns the same bytes, so a pass
        // that came in short under an exhausted budget had proved the target
        // unreachable — and the object cache in this same commit is what makes
        // that false. A second pass takes everything the first fetched out of
        // the cache for *no budget at all*, so it walks strictly further into
        // the page: on the history tier, where a partition can spend a
        // megabyte to hand back an eighth of one, the pass after it reaches
        // twice as far for one GET. Stopping there hands a busy client half its
        // configured batch on every poll and doubles its round trips.
        if worth_answering(&outcomes, target)
            || reads >= MAX_READS_PER_REQUEST
            || Instant::now() >= deadline
        {
            return outcomes;
        }
        // ⚠️ **Park until a partition *this request named* moves.** One watch
        // serves every partition on a shard, so most wakeups are somebody
        // else's commit, and re-reading on those would turn one parked fetch
        // into an object-storage read per shard commit. A watermark is an
        // index lookup and touches no store.
        loop {
            // ⚠️ `select!`, so the deadline is honoured whatever the index
            // does — and `false` from the watch means the coordinator stopped,
            // which is answered from what the index already holds rather than
            // waited out.
            let alive = tokio::select! {
                () = tokio::time::sleep_until(deadline) => false,
                alive = watch.wait_past(applied) => alive,
            };
            if !alive {
                return outcomes;
            }
            applied = watch.applied();
            if watermarks(cluster, request, version) != before {
                break;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use crate::authz::AuthzContext;
    use crate::connection::HandlerResponse;
    use crate::fetch::handle;
    use crate::fetch::tests::{
        Poll, by_id, by_id_of, decode, hungry_fetch_body, prelude, produced, replied,
        waiting_fetch_body,
    };
    use crate::testing::{Fixture, fixture, golden_batch, produce_one};
    use oqueue_core::Operation;

    /// A `Fetch` in flight on its own task, so the test can commit underneath
    /// it.
    ///
    /// ⚠️ **The `Arc` is why this exists**: the handler borrows the cluster for
    /// as long as it is parked, so a spawned reader needs a share of it, and
    /// three tests were repeating the same six lines to get one.
    fn parked_reader(
        fixture: &std::sync::Arc<Fixture>,
        body: Vec<u8>,
    ) -> tokio::task::JoinHandle<kafka_protocol::messages::FetchResponse> {
        let fixture = std::sync::Arc::clone(fixture);
        tokio::spawn(async move {
            let HandlerResponse::Reply(out) = handle(
                &fixture.cluster,
                &fixture.session,
                prelude(13),
                &body,
                &AuthzContext {
                    principal: None,
                    credentials_configured: false,
                    topic_grants: &oqueue_core::TopicGrants::default(),
                },
            )
            .await
            else {
                panic!("a fetch replies");
            };
            decode(&out, 13)
        })
    }

    /// ⚠️ **FR-12 and NFR-2: a fetch with nothing to return parks, and a
    /// produce wakes it.** This is the end-to-end case `M3.9` built
    /// `IndexWatch` for and could not write for want of a composer. Under
    /// `start_paused` the fetch cannot succeed by luck: nothing advances the
    /// clock but the runtime, and the only thing that resolves the park is the
    /// commit.
    #[tokio::test(start_paused = true)]
    async fn a_parked_fetch_is_woken_by_a_concurrent_produce() {
        let fixture = std::sync::Arc::new(fixture(&["t"]).await);
        let id = fixture.cluster.topic_id("t").expect("the fixture's topic");
        let body = waiting_fetch_body(13, by_id_of(id), 0, 0, 30_000);
        let started = tokio::time::Instant::now();

        let reader = parked_reader(&fixture, body);

        // ⚠️ Let the fetch reach its park before producing, so this asserts the
        // wakeup rather than a race the reader happened to win.
        tokio::task::yield_now().await;
        assert!(
            !reader.is_finished(),
            "the fetch must be parked, not answered"
        );
        produce_one(&fixture, "t", golden_batch()).await;

        let response = reader.await.expect("the reader task joins");
        // ⚠️ **Before the deadline, and that is a separate claim.** Under
        // paused time a fetch that woke, decided its records were not worth
        // answering, and parked again would still return — thirty seconds
        // later, with the same bytes. Only the elapsed time separates "woken
        // by the produce" from "waited the client out and happened to have
        // records by then".
        assert!(
            tokio::time::Instant::now() - started < std::time::Duration::from_secs(30),
            "it must have answered on the wakeup, not on the deadline"
        );
        let p = &response.responses[0].partitions[0];
        assert_eq!(p.error_code, 0);
        assert_eq!(p.high_watermark, 2, "the produce it was woken by");
        assert!(
            p.records.as_ref().is_some_and(|r| !r.is_empty()),
            "woken, and with the records — not woken and empty"
        );
    }

    /// ⚠️ **A parked fetch does not re-read when somebody else commits.**
    /// One watch serves every partition on a shard, so most wakeups are for
    /// another partition — and re-reading on those would turn one parked fetch
    /// into an object-storage read per shard commit, each of which can cost a
    /// whole-object GET. A watermark is an index lookup; the read is what must
    /// not happen.
    ///
    /// ⚠️ **The parked partition holds records on purpose.** A fetch parked at
    /// an *empty* partition reads nothing whatever the gate does (FR-12's
    /// zero-GET claim), so the assertion below would hold with the gate
    /// deleted — a regression test that cannot fail. Producing first, and
    /// asking for more bytes than are there, makes every read cost a GET, so
    /// a re-read is visible.
    #[tokio::test(start_paused = true)]
    async fn a_park_woken_by_another_partitions_commit_does_not_re_read() {
        let fixture = std::sync::Arc::new(fixture(&["mine", "theirs"]).await);
        produce_one(&fixture, "mine", golden_batch()).await;
        let id = fixture.cluster.topic_id("mine").expect("a hosted topic");
        let body = hungry_fetch_body(
            13,
            by_id_of(id),
            0,
            0,
            Poll {
                max_wait_ms: 30_000,
                // Far more than one batch, so the read costs a GET and still
                // leaves the request parked.
                min_bytes: 64 * 1024,
            },
        );
        let reader = parked_reader(&fixture, body);
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        assert!(!reader.is_finished(), "parked, short of its minimum");
        let quiet = fixture.store.counts().count(Operation::Get);
        assert!(
            quiet > 0,
            "the first read must have cost a GET to be evidence"
        );

        for _ in 0..5 {
            produce_one(&fixture, "theirs", golden_batch()).await;
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }

        assert!(
            !reader.is_finished(),
            "another topic's records are not this one's"
        );
        assert_eq!(
            fixture.store.counts().count(Operation::Get),
            quiet,
            "five wakeups for another partition must cost this fetch no reads"
        );

        // And its own commit does re-read: the gate is a filter, not a wall.
        produce_one(&fixture, "mine", golden_batch()).await;
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        assert!(
            fixture.store.counts().count(Operation::Get) > quiet,
            "a commit to the fetched partition must be read"
        );

        let response = reader.await.expect("the reader joins");
        assert_eq!(response.responses[0].partitions[0].error_code, 0);
    }

    /// ⚠️ **A commit landing *during* the read is not lost.** `read_all` awaits
    /// object-storage GETs, so it takes real time, and a produce can commit
    /// while it runs. If the handler sampled its watermark baseline and its
    /// watch version *after* the read, that commit would already be in the
    /// baseline: no watermark difference to notice, and no version left for
    /// `wait_past` to resolve on. The fetch would park on outcomes read before
    /// the commit and answer at its deadline missing a record that was in the
    /// index the whole time.
    #[tokio::test(start_paused = true)]
    async fn a_commit_that_lands_during_the_read_is_not_folded_into_the_baseline() {
        let fixture = std::sync::Arc::new(fixture(&["t"]).await);
        produce_one(&fixture, "t", golden_batch()).await;
        let id = fixture.cluster.topic_id("t").expect("a hosted topic");
        let body = hungry_fetch_body(
            13,
            by_id_of(id),
            0,
            0,
            Poll {
                max_wait_ms: 30_000,
                // Above one batch, so the first read leaves the fetch parked.
                min_bytes: 64 * 1024,
            },
        );
        // ⚠️ Enough polls that the read is still open across everything the
        // produce below awaits — the window this test exists to put a commit
        // inside.
        fixture.slow_store(5_000);
        let reader = parked_reader(&fixture, body);
        tokio::task::yield_now().await;
        assert!(
            fixture.store.counts().count(Operation::Get) > 0,
            "the reader must be inside its read for this to be the window"
        );
        // ⚠️ **Healed *before* the produce, and the order is the whole
        // fixture.** The pending GET keeps the poll count it was created with,
        // so it stays open; the produce below runs at full speed and commits
        // while it is still open. Healing afterwards instead would slow the
        // produce down too, and the commit would land after the read finished
        // — which is the ordinary case this test is not about.
        fixture.heal_store();

        produce_one(&fixture, "t", golden_batch()).await;

        // ⚠️ **The answer still comes at the deadline, and that is correct**:
        // `min_bytes` is never met, so the fetch waits its full `max_wait_ms`
        // either way. What the commit must change is the *content* — which is
        // why the assertions below are about what came back and not about
        // when.

        let response = reader.await.expect("the reader joins");
        let p = &response.responses[0].partitions[0];
        assert_eq!(p.error_code, 0);
        assert_eq!(
            p.high_watermark, 4,
            "both produces are in the index by the time it answers"
        );
        let records = p.records.as_ref().expect("records");
        let first = oqueue_codec::batch::decode_batch_header(records).expect("a batch");
        let len = usize::try_from(first.batch_length).expect("small") + 12;
        assert!(
            records.len() > len,
            "the second batch must be in the answer, not only the first"
        );
    }

    /// ⚠️ **One request cannot be turned into unbounded reads by somebody
    /// else's writes.** A `min_bytes` a page can never reach leaves the fetch
    /// parked, and every commit to its own partition is a legitimate wakeup —
    /// so without a cap the read count is set by the producer's rate rather
    /// than by the consumer that asked. `MAX_READS_PER_REQUEST` is the cap.
    #[tokio::test(start_paused = true)]
    async fn a_request_reads_a_bounded_number_of_times_however_much_is_committed() {
        let fixture = std::sync::Arc::new(fixture(&["t"]).await);
        produce_one(&fixture, "t", golden_batch()).await;
        let id = fixture.cluster.topic_id("t").expect("a hosted topic");
        let body = hungry_fetch_body(
            13,
            by_id_of(id),
            0,
            0,
            Poll {
                max_wait_ms: 30_000,
                min_bytes: i32::MAX,
            },
        );
        let reader = parked_reader(&fixture, body);
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;

        // Twenty commits to the very partition being fetched: every one is a
        // real wakeup with real new records, and the cap is what stops them
        // becoming twenty reads.
        for _ in 0..20 {
            produce_one(&fixture, "t", golden_batch()).await;
            tokio::time::sleep(std::time::Duration::from_millis(1)).await;
        }

        let response = reader.await.expect("the reader joins");
        assert_eq!(response.responses[0].partitions[0].error_code, 0);
        // ⚠️ **An exact count, not a bound.** Every batch is in the tail tier
        // and each pass sees exactly one more of them, so a pass costs one GET
        // — the *new* batch — and the ones it re-reads are cache hits. A cap
        // one higher would read a fifth time and cost five; one lower, three.
        // A `<=` here would let the ceiling drift upward unnoticed, which is
        // the whole thing this constant is for.
        //
        // ⚠️ **It was 1+2+3+4 until `M3.26` made the object cache span the
        // request.** The count fell because the re-reads stopped paying, not
        // because the cap moved: a parked fetch re-reads the same offsets by
        // construction, and paying for them again was the park's own
        // amplification hiding inside a bounded read count.
        // ⚠️ **A literal, not `MAX_READS_PER_REQUEST`** (`M3.36`). Deriving
        // the expectation from the constant makes the assertion true for every
        // value of it: raise the cap and this test raises with it, which is a
        // test that cannot fail on the change it exists to catch. The
        // constant's value is held by `check-drift.sh`'s pin map, so the two
        // must be changed together and a reviewer sees both.
        let expected: u64 = 4;
        assert_eq!(
            fixture.store.counts().count(Operation::Get),
            expected,
            "exactly MAX_READS_PER_REQUEST reads, whatever the producer does"
        );
    }

    /// ⚠️ **And the deadline is honoured when nothing comes.** A park that
    /// outlived `max_wait_ms` would hold a connection for as long as the
    /// partition stayed idle, which is a broker that never answers rather than
    /// one that answers empty.
    #[tokio::test(start_paused = true)]
    async fn a_park_that_nothing_wakes_ends_at_the_clients_deadline() {
        let fixture = produced().await;
        let started = tokio::time::Instant::now();

        let response = replied(
            &fixture,
            13,
            &waiting_fetch_body(13, by_id(&fixture), 2, 0, 250),
        )
        .await;

        let waited = tokio::time::Instant::now() - started;
        assert!(
            waited >= std::time::Duration::from_millis(250),
            "it must actually have waited: {waited:?}"
        );
        let p = &response.responses[0].partitions[0];
        assert_eq!(p.error_code, 0, "a deadline is not an error");
        assert!(p.records.as_ref().is_none_or(bytes::Bytes::is_empty));
    }

    /// ⚠️ **A refusal is not parked on.** Holding an `UNKNOWN_TOPIC_ID` for the
    /// client's whole deadline would delay an error it can act on immediately,
    /// and no amount of waiting was ever going to turn it into records.
    #[tokio::test(start_paused = true)]
    async fn a_refusal_is_answered_at_once_however_long_the_client_would_wait() {
        let fixture = fixture(&[]).await;
        let ghost = uuid::Uuid::from_u128(0xBEEF);
        let started = tokio::time::Instant::now();

        let response = replied(
            &fixture,
            13,
            &waiting_fetch_body(13, by_id_of(ghost), 0, 0, 30_000),
        )
        .await;

        assert_eq!(
            tokio::time::Instant::now() - started,
            std::time::Duration::ZERO,
            "an error must not be held"
        );
        assert_eq!(
            response.responses[0].partitions[0].error_code,
            kafka_protocol::error::ResponseError::UnknownTopicId.code()
        );
    }
}
