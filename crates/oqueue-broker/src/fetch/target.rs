//! What makes a response worth sending: the byte target, and reaching it.
//!
//! ⚠️ **Its own module because "wait" and "enough" are different questions.**
//! `park.rs` decides when to look again; everything here decides whether what
//! was found is worth answering with — the client's `min_bytes`, what a single
//! response can actually hold, and what happens when those two disagree.

// ⚠️ `pub(crate)` inside a private module: `unreachable_pub` denies the `pub`
// clippy's `redundant_pub_crate` asks for.
#![allow(clippy::redundant_pub_crate)]

use super::TopicOutcome;
use crate::fetch::partition::READ_BUDGET_BYTES;
use oqueue_codec::error_codes;

/// What `min_bytes` a response could plausibly reach.
///
/// ⚠️ **Clamped, but not made reachable, and the difference matters.** One
/// partition's read is bounded by [`READ_BUDGET_BYTES`], so a target above
/// that is certainly unreachable and clamping it costs nothing. ⚠️ **A
/// clamped target can still be unreachable**: `find_batches` also caps a page
/// at `MAX_BATCHES_PER_PAGE`, so a partition of small batches returns less
/// than a megabyte however long anyone waits. That is what
/// [`MAX_READS_PER_REQUEST`] is for — this clamp alone would leave the park
/// bounded in time and unbounded in reads.
///
/// ⚠️ **And a clamped target can still be unreachable for a *second* reason,
/// which no clamp here can see** (`M3.26`). This counts bytes a response
/// *carries*; [`Budget`] counts bytes a read *pulled*, and on the history tier
/// a batch is one partition's region of a bundle covering every partition that
/// flush wrote — so a pass can spend a megabyte to hand back an eighth of one.
/// ⚠️ **What answers it is the object cache, not arithmetic**: the pass after
/// takes everything already fetched for no budget and walks strictly further,
/// so the target is reached across passes rather than within one. A request
/// whose target is unreachable even across all of them parks to its deadline —
/// which is what `max_wait_ms` means, and it costs sleeping rather than reads.
///
/// ⚠️ **Clamped against the *request's* ceiling since `M3.22`**, not against
/// the constant alone. A client may name `fetch.max.bytes = 65536` and
/// `fetch.min.bytes = 1048576` — both legal, and neither client validates one
/// against the other — and a response that can hold 64 KiB will never reach a
/// megabyte. Left unclamped, every poll on a *full* log would park to its
/// deadline and pay `MAX_READS_PER_REQUEST` reads instead of one, forever.
pub(crate) fn satisfiable_min_bytes(min_bytes: i32, budget: &Budget) -> u64 {
    // ⚠️ `try_from` and `min`, not comparisons: a negative `min_bytes` is a
    // client bug meaning "no minimum", which is exactly what a failed
    // conversion falls back to, and the ceilings are `min`s rather than `if`s
    // so there is no boundary to get wrong at exactly the budget.
    u64::try_from(min_bytes)
        .unwrap_or(0)
        .min(READ_BUDGET_BYTES)
        .min(budget.total())
}

/// How many times one `Fetch` may read before it must answer.
///
/// ⚠️ **A bound on the broker's work per client request, and it is the reason
/// the park cannot be turned into an amplifier.** `min_bytes` is not always
/// reachable — [`satisfiable_min_bytes`] clamps it to the read budget, but
/// `find_batches` also caps a page at `MAX_BATCHES_PER_PAGE`, so a partition of
/// small batches can return less than the target however long it waits. Left
/// unbounded, every commit to a partition this request named would re-read
/// from the same `fetch_offset`: quadratic GETs from one request, with the
/// count set by *producers* rather than by the client that asked.
///
/// ⚠️ **The cost is answering below `min_bytes` before the deadline** — a
/// short response, which every client already handles, and which
/// `docs/protocol-support.md` records. The alternative is a client's read
/// volume being set by somebody else's write rate.
pub(crate) const MAX_READS_PER_REQUEST: u32 = 4;

/// What one partition's read may spend.
#[derive(Debug, Clone, Copy)]
pub struct Allowance {
    /// The bytes this read is bounded by.
    pub bytes: u64,
    /// Whether this read may return one batch past that bound.
    ///
    /// ⚠️ **At most one read per *response* may return a batch past the
    /// bound.** A partition whose very first batch exceeds the allowance must
    /// still be readable or its consumer parks at that offset forever;
    /// granting that per *partition* would let a client naming one partition
    /// two hundred times collect two hundred whole batches for a `max_bytes`
    /// of one.
    ///
    /// ⚠️ **It is not "at most one read"**, which is what this said until
    /// `M3.37`: since `M3.39` a read that returns *nothing* does not spend the
    /// flag, so several failing partitions can each cross the line before one
    /// succeeds. What bounds that is the failure cap, and
    /// the object cache is where the memory
    /// consequence is written down.
    pub may_overshoot: bool,
}

/// What is left of one request's byte allowance.
///
/// ⚠️ **Per request, not per partition, and that is the whole point.** A
/// `Fetch` may name any number of partitions, so a per-partition bound
/// multiplied by a client-chosen count is not a bound: one 16 MiB frame
/// becomes partitions × budget bytes of object-storage reads, concatenated in
/// memory before the response is framed, with `max_in_flight` such requests in
/// flight per connection. `M3.14`'s review found exactly that.
///
/// ⚠️ **Both of the client's numbers bind, and the smaller wins.** It names a
/// ceiling for the response and one for each partition; honouring only the
/// second is the failure above, and honouring only the first would let one
/// greedy partition take the whole response.
#[derive(Debug)]
pub(crate) struct Budget {
    total: u64,
    left: u64,
    returned_anything: bool,
}

impl Budget {
    /// A budget from the client's request-level `max_bytes`.
    ///
    /// ⚠️ Clamped to [`READ_BUDGET_BYTES`], which is what this broker will
    /// spend on one response whatever a client asks for: `max_bytes` is an
    /// `i32`, so a client may name two gigabytes, and a broker that obliged
    /// would let one frame decide how much memory it uses. A negative value is
    /// a client bug and buys nothing beyond the first-batch exception every
    /// read keeps.
    pub(crate) fn new(max_bytes: i32) -> Self {
        let total = u64::try_from(max_bytes).unwrap_or(0).min(READ_BUDGET_BYTES);
        Self {
            total,
            left: total,
            returned_anything: false,
        }
    }

    /// Whether a partition may still be served one batch over its share.
    ///
    /// ⚠️ **Once per *response*, not once per partition**, and the difference
    /// is whether the budget is a bound at all. Per partition, a client that
    /// names the same partition two hundred times in one frame gets two
    /// hundred whole batches for a `max_bytes` of one — measured, not argued.
    /// `ADR-0022` states the exemption as a property of *a fetch*, and Kafka's
    /// own `readFromLocalLog` clears `minOneMessage` after the first non-empty
    /// partition for the same reason.
    ///
    /// ⚠️ **Nothing starves.** A partition that comes back empty under an
    /// exhausted budget is served on the client's next fetch, and both the
    /// Java consumer and librdkafka rotate their fetchable-partition order
    /// precisely so that a bounded response cannot always favour the same
    /// partitions.
    pub(crate) const fn may_overshoot(&self) -> bool {
        !self.returned_anything
    }

    /// The whole allowance this request started with — what a response could
    /// hold at most, which is the ceiling a `min_bytes` has to be reachable
    /// under.
    pub(crate) const fn total(&self) -> u64 {
        self.total
    }

    /// What one partition may spend: the smaller of what it asked for and what
    /// the request has left.
    pub(crate) fn share(&self, partition_max_bytes: i32) -> u64 {
        u64::try_from(partition_max_bytes)
            .unwrap_or(0)
            .min(self.left)
    }

    /// Everything a read needs to know about what it may spend.
    ///
    /// ⚠️ **The two travel together because they are one decision**: how many
    /// bytes, and whether this read is the one allowed to cross the line by a
    /// batch. Passed apart, a caller can hand a partition its share and the
    /// *response*'s overshoot without noticing they came from different
    /// questions — which is the shape the per-partition exemption had.
    pub(crate) fn allowance(&self, partition_max_bytes: i32) -> Allowance {
        Allowance {
            bytes: self.share(partition_max_bytes),
            may_overshoot: self.may_overshoot(),
        }
    }

    /// Records what a partition cost, and whether it returned anything.
    ///
    /// ⚠️ **Two facts, because they stopped being the same one.** This took a
    /// single `cost` and flipped `returned_anything` whenever it was nonzero —
    /// true while only a *successful* read cost anything. `M3.26` gave a
    /// failed read a nonzero cost, and the two came apart: a partition that
    /// returned no records consumed the response's once-per-response
    /// exemption, so a corrupt first partition left a healthy second one
    /// framed `NONE` with no records. ⚠️ **A response in which no partition
    /// makes progress is doc 12 §4.6's silent wrongness**, and the exemption
    /// exists precisely so that cannot happen.
    ///
    /// ⚠️ **The cost is still charged.** A read that pulled a whole bundle and
    /// could not use it spent the request's bytes whatever it returned; what
    /// it did not do is take the one exemption away from somebody who could
    /// have used it.
    pub(crate) const fn spend(&mut self, cost: u64, returned_records: bool) {
        self.left = self.left.saturating_sub(cost);
        if returned_records {
            self.returned_anything = true;
        }
    }
}

/// Whether this response is worth sending now rather than parking on.
pub(crate) fn worth_answering(outcomes: &[TopicOutcome], min_bytes: u64) -> bool {
    let mut bytes: u64 = 0;
    for topic in outcomes {
        for partition in &topic.partitions {
            if partition.error_code != error_codes::NONE {
                // ⚠️ An error is news. Holding it costs the client the whole
                // deadline before it can react to something already decided.
                return true;
            }
            bytes += partition.records.len() as u64;
        }
    }
    // ⚠️ **`min_bytes = 0` answers immediately, empty included**, which is what
    // Kafka's broker does — it compares readable bytes against the client's
    // minimum, and zero satisfies zero. A `max(1)` here would read as "wait for
    // one byte" and hold a `fetch.min.bytes=0` client for its whole deadline.
    // `fetch.min.bytes` defaults to 1 in every client, so the park is what a
    // default client gets either way — ⚠️ which is why a request naming *no
    // partitions* is short-circuited in `read_or_park` instead of relying on
    // this comparison: at the default minimum, zero bytes would not satisfy it.
    bytes >= min_bytes
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{Budget, READ_BUDGET_BYTES, satisfiable_min_bytes};

    use crate::fetch::tests::{
        Poll, by_id, by_id_of, fetch_body, hungry_fetch_body, produced, replied,
    };
    use crate::testing::{Fixture, fixture, golden_batch, produce_one};
    use kafka_protocol::messages::FetchRequest;
    use kafka_protocol::messages::fetch_request::{FetchPartition, FetchTopic};
    use kafka_protocol::protocol::Encodable;

    /// How long one produced batch comes back as, so a threshold can straddle
    /// one batch and two.
    async fn single_batch_len(fixture: &Fixture, name: &str) -> usize {
        let id = fixture.cluster.topic_id(name).expect("a hosted topic");
        let response = replied(fixture, 13, &fetch_body(13, by_id_of(id), 0, 0)).await;
        response.responses[0].partitions[0]
            .records
            .as_ref()
            .expect("records")
            .len()
    }

    /// A `Fetch` naming two topics' partition 0, with the wait and the minimum
    /// chosen.
    fn two_topic_body(
        fixture: &Fixture,
        names: &[&str],
        max_wait_ms: i32,
        min_bytes: i32,
    ) -> Vec<u8> {
        let mut request = FetchRequest::default();
        request.max_wait_ms = max_wait_ms;
        request.min_bytes = min_bytes;
        for name in names {
            let mut t = FetchTopic::default();
            t.topic_id = fixture.cluster.topic_id(name).expect("a hosted topic");
            let mut p = FetchPartition::default();
            p.partition = 0;
            p.fetch_offset = 0;
            p.partition_max_bytes = 1 << 20;
            t.partitions.push(p);
            request.topics.push(t);
        }
        let mut out = Vec::new();
        request.encode(&mut out, 13).expect("encodes");
        out
    }

    /// ⚠️ **`min_bytes` is met across the whole response, not per partition.**
    /// A client sizing one round trip asks for bytes, not for bytes-per-
    /// partition, and a broker that split the target would answer four times
    /// as early as asked.
    #[tokio::test(start_paused = true)]
    async fn min_bytes_is_measured_across_the_whole_response() {
        let names = ["a", "b"];
        let fixture = fixture(&names).await;
        for name in names {
            produce_one(&fixture, name, golden_batch()).await;
        }
        // ⚠️ Straddling on purpose: one batch is under the target and two are
        // over it, so a per-partition rule parks here and a whole-response
        // rule answers. Derived from the fixture rather than written down, so
        // a change to the golden batch cannot quietly make it vacuous.
        let one = single_batch_len(&fixture, "a").await;
        assert!(one > 0, "the fixture must have produced something");
        let target = i32::try_from(one + 1).expect("a small batch");

        let started = tokio::time::Instant::now();
        let response = replied(
            &fixture,
            13,
            &two_topic_body(&fixture, &names, 30_000, target),
        )
        .await;

        assert_eq!(
            tokio::time::Instant::now() - started,
            std::time::Duration::ZERO,
            "both partitions' bytes together meet the target, so no park"
        );
        for topic in &response.responses {
            assert_eq!(topic.partitions[0].error_code, 0);
        }
    }

    /// ⚠️ **A request naming no partitions is answered, not parked on.** There
    /// is no commit that could satisfy it, so waiting waits for something that
    /// cannot happen — and at every client's default `min_bytes = 1` the byte
    /// comparison alone would hold it for the whole deadline.
    #[tokio::test(start_paused = true)]
    async fn a_fetch_naming_no_partitions_is_answered_at_once() {
        let fixture = fixture(&["t"]).await;
        let mut request = FetchRequest::default();
        request.max_wait_ms = 30_000;
        request.min_bytes = 1;
        let mut body = Vec::new();
        request.encode(&mut body, 13).expect("encodes");
        let started = tokio::time::Instant::now();

        let response = replied(&fixture, 13, &body).await;

        assert_eq!(
            tokio::time::Instant::now() - started,
            std::time::Duration::ZERO,
            "nothing was asked for, so nothing can arrive"
        );
        assert!(response.responses.is_empty());
    }

    /// ⚠️ **`min_bytes = 0` means answer now, empty included** — what Kafka's
    /// own broker does, and what a client using an empty response as its loop
    /// tick depends on.
    #[tokio::test(start_paused = true)]
    async fn min_bytes_of_zero_answers_at_once_even_with_nothing_to_send() {
        let fixture = produced().await;
        let started = tokio::time::Instant::now();

        let response = replied(
            &fixture,
            13,
            &hungry_fetch_body(
                13,
                by_id(&fixture),
                2,
                0,
                Poll {
                    max_wait_ms: 30_000,
                    min_bytes: 0,
                },
            ),
        )
        .await;

        assert_eq!(
            tokio::time::Instant::now() - started,
            std::time::Duration::ZERO,
            "zero bytes satisfies a zero minimum"
        );
        let p = &response.responses[0].partitions[0];
        assert_eq!(p.error_code, 0);
        assert!(p.records.as_ref().is_none_or(bytes::Bytes::is_empty));
    }

    /// ⚠️ **The budget is the request's, and both of the client's numbers
    /// bind.** Honouring only `partition_max_bytes` is the defect `M3.14`'s
    /// review found — partitions × budget, with nothing capping the product.
    /// Honouring only `max_bytes` would let one greedy partition take the
    /// whole response.
    #[test]
    fn a_budget_is_the_smaller_of_what_the_request_and_the_partition_asked_for() {
        let budget = Budget::new(1_000);
        assert_eq!(budget.share(400), 400, "the partition asked for less");
        assert_eq!(budget.share(9_000), 1_000, "the request asked for less");
    }

    /// ⚠️ **Spending is cumulative across partitions**, which is the whole
    /// difference between a per-request bound and a per-partition one.
    #[test]
    fn what_one_partition_spends_is_gone_from_the_next_partitions_share() {
        let mut budget = Budget::new(1_000);
        budget.spend(600, true);
        assert_eq!(budget.share(9_000), 400);
        budget.spend(400, true);
        assert_eq!(budget.share(9_000), 0, "and it runs out");
        budget.spend(50, true);
        assert_eq!(budget.share(9_000), 0, "without going negative");
    }

    /// ⚠️ **A partition that returned nothing does not take the exemption.**
    /// The bytes it fetched are still gone from the request — a read that
    /// pulled a whole bundle spent them whatever it could do with them — but
    /// the one over-the-line read a response is allowed belongs to whoever can
    /// still use it. ⚠️ **Otherwise a response can carry no records at all**,
    /// which is doc 12 §4.6's silent wrongness produced by the mechanism that
    /// exists to stop a consumer parking at an offset forever.
    #[test]
    fn a_partition_that_returned_nothing_leaves_the_exemption_for_the_next() {
        let mut budget = Budget::new(1_000);
        assert!(budget.may_overshoot(), "nobody has returned anything yet");

        budget.spend(1_000, false);

        assert_eq!(budget.share(9_000), 0, "its bytes are gone all the same");
        assert!(
            budget.may_overshoot(),
            "and the partition behind it can still be served one batch"
        );

        budget.spend(0, true);
        assert!(
            !budget.may_overshoot(),
            "once something is returned, the exemption is spent"
        );
    }

    /// ⚠️ **A client cannot name a bigger allowance than this broker will
    /// spend.** `max_bytes` is an `i32`, so a request may ask for two
    /// gigabytes; obliging would let one frame decide how much memory the
    /// process uses. A negative value is a client bug and buys nothing.
    #[test]
    fn a_clients_allowance_is_clamped_at_both_ends() {
        assert_eq!(Budget::new(i32::MAX).share(i32::MAX), READ_BUDGET_BYTES);
        assert_eq!(Budget::new(-5).share(i32::MAX), 0);
    }

    /// ⚠️ **The clamp, on its own**, because the behaviour it buys is hard to
    /// see from outside: a client asking for more bytes than a response can
    /// ever hold would otherwise park until its deadline having already read
    /// everything, and re-read on every wakeup.
    #[test]
    fn a_minimum_larger_than_a_response_is_clamped_to_what_one_holds() {
        assert_eq!(
            satisfiable_min_bytes(0, &Budget::new(i32::MAX)),
            0,
            "zero means answer now"
        );
        assert_eq!(satisfiable_min_bytes(1, &Budget::new(i32::MAX)), 1);
        assert_eq!(
            satisfiable_min_bytes(-7, &Budget::new(i32::MAX)),
            0,
            "a client bug is no minimum"
        );
        assert_eq!(
            satisfiable_min_bytes(i32::MAX, &Budget::new(i32::MAX)),
            READ_BUDGET_BYTES,
            "the ceiling is what one read can price, not what was asked"
        );
        assert_eq!(
            satisfiable_min_bytes(
                i32::try_from(READ_BUDGET_BYTES).expect("a megabyte fits"),
                &Budget::new(i32::MAX)
            ),
            READ_BUDGET_BYTES,
            "exactly the budget is still satisfiable"
        );
        // ⚠️ **And a minimum above what this *request* can hold is clamped to
        // that.** A client naming `fetch.max.bytes = 4096` with
        // `fetch.min.bytes = 65536` — both legal, and neither validates
        // against the other — would otherwise park to its deadline on every
        // poll of a full log, waiting for bytes its own response cannot carry.
        assert_eq!(
            satisfiable_min_bytes(65_536, &Budget::new(4_096)),
            4_096,
            "the request's own ceiling binds too"
        );
    }
}
