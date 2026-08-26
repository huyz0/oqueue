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
/// `M3.22` replaces the constant with the client's own `max_bytes`, and this
/// clamp is what stops the two disagreeing silently.
pub(crate) fn satisfiable_min_bytes(min_bytes: i32) -> u64 {
    // ⚠️ `try_from` and `min`, not comparisons: a negative `min_bytes` is a
    // client bug meaning "no minimum", which is exactly what a failed
    // conversion falls back to, and the ceiling is a `min` rather than an `if`
    // so there is no boundary to get wrong at exactly the budget.
    u64::try_from(min_bytes).unwrap_or(0).min(READ_BUDGET_BYTES)
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

    /// ⚠️ **The clamp, on its own**, because the behaviour it buys is hard to
    /// see from outside: a client asking for more bytes than a response can
    /// ever hold would otherwise park until its deadline having already read
    /// everything, and re-read on every wakeup.
    #[test]
    fn a_minimum_larger_than_a_response_is_clamped_to_what_one_holds() {
        assert_eq!(super::satisfiable_min_bytes(0), 0, "zero means answer now");
        assert_eq!(super::satisfiable_min_bytes(1), 1);
        assert_eq!(
            super::satisfiable_min_bytes(-7),
            0,
            "a client bug is no minimum"
        );
        assert_eq!(
            super::satisfiable_min_bytes(i32::MAX),
            super::READ_BUDGET_BYTES,
            "the ceiling is what one read can price, not what was asked"
        );
        assert_eq!(
            super::satisfiable_min_bytes(
                i32::try_from(super::READ_BUDGET_BYTES).expect("a megabyte fits")
            ),
            super::READ_BUDGET_BYTES,
            "exactly the budget is still satisfiable"
        );
    }
}
