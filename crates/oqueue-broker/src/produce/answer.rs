//! What a client is told about one requested partition.
//!
//! ⚠️ **Its own module because a refusal is a decision, not a fallthrough.**
//! Every arm below chooses an error code a Kafka client already knows how to
//! act on, and the one thing none of them may do is answer `NONE` with an
//! offset nothing journalled. Doc 13 §8 records what that costs in a peer
//! system: a plausible-looking offset on an error path turns an availability
//! bug into a safety bug.

// ⚠️ `pub(crate)` inside a private module: `unreachable_pub` denies the `pub`
// clippy's `redundant_pub_crate` asks for.
#![allow(clippy::redundant_pub_crate)]

use crate::cluster::FlushError;
use oqueue_codec::error_codes;
use oqueue_codec::produce::ProduceResponsePartition;

/// Where one requested partition's answer comes from.
pub(crate) enum Slot {
    /// Refused before anything was written; this is its error code.
    Refused(i16),
    /// Pushed into the bundle as its `n`th region, so the `n`th assignment in
    /// the ack is its base offset.
    Pushed(usize),
}

/// One requested partition, and where its answer comes from.
pub(crate) struct PartitionSlot {
    pub(crate) index: i32,
    pub(crate) slot: Slot,
}

/// One partition's answer: its own refusal, or the offset the commit gave it.
///
/// ⚠️ **A failed flush refuses every partition that was in it**, and with the
/// error the failure's own shape earns: a write that did not land is
/// `NOT_ENOUGH_REPLICAS` — the code a Kafka client already retries — while a
/// commit that did not journal is `LEADER_NOT_AVAILABLE`, which sends the
/// client to `Metadata` and back. Neither is `NONE` with a guessed offset.
pub(crate) fn answer(
    slot: &PartitionSlot,
    flushed: Result<&Vec<i64>, &FlushError>,
) -> ProduceResponsePartition {
    let (error_code, base_offset) = match (&slot.slot, flushed) {
        (Slot::Refused(code), _) => (*code, UNASSIGNED),
        (Slot::Pushed(nth), Ok(offsets)) => offsets
            .get(*nth)
            .map_or((error_codes::UNKNOWN_SERVER_ERROR, UNASSIGNED), |offset| {
                (error_codes::NONE, *offset)
            }),
        (Slot::Pushed(_), Err(FlushError::Store(_))) => {
            (error_codes::NOT_ENOUGH_REPLICAS, UNASSIGNED)
        }
        (Slot::Pushed(_), Err(FlushError::Commit(_))) => {
            (error_codes::LEADER_NOT_AVAILABLE, UNASSIGNED)
        }
    };
    ProduceResponsePartition {
        index: slot.index,
        error_code,
        base_offset,
    }
}

/// ⚠️ **`-1`, never `0`** — doc 13 §8: a plausible-looking offset on an error
/// path turns an availability bug into a safety bug. The same constant the
/// coordinator names.
const UNASSIGNED: i64 = oqueue_coordinator::UNASSIGNED_OFFSET;

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use crate::produce::tests::{produce_body, replied, verdict};
    use crate::testing::golden_batch;
    use crate::testing::{fixture, partition, topic, with_broken_store, with_dead_coordinator};
    use oqueue_core::Operation;

    /// ⚠️ **A produce whose object never landed is never acknowledged.** This
    /// is the doc 13 §8 inversion in its most direct form: answering `NONE`
    /// with a plausible offset here would turn a backend outage — an
    /// availability problem — into records a client believes are durable and
    /// that do not exist. `NOT_ENOUGH_REPLICAS` is the code every Kafka client
    /// already retries.
    #[tokio::test]
    async fn a_produce_whose_put_failed_is_refused_with_no_offset() {
        let fixture = with_broken_store(&["t"], 4).await;
        let body = produce_body(9, "t", -1, golden_batch());

        let response = replied(&fixture, 9, &body).await;

        assert_eq!(
            verdict(&response),
            (
                kafka_protocol::error::ResponseError::NotEnoughReplicas.code(),
                -1
            )
        );
        assert_eq!(
            fixture
                .cluster
                .high_watermark(&topic("t"), partition(0))
                .get(),
            0,
            "nothing was committed either"
        );
    }

    /// ⚠️ **FR-10, in the words `requirements.md` uses**: kill between PUT and
    /// ack, and no acknowledged record is lost. The object *is* durable here —
    /// `crash_after_put_before_ack` writes it and then loses the reply — so
    /// this is the case where a broker is most tempted to acknowledge: the
    /// bytes are in the bucket, and only the confirmation is missing.
    ///
    /// ⚠️ **It must not, and the reason is not caution.** Nothing committed,
    /// so the object nobody references holds no offsets; acknowledging would
    /// hand a client an offset that no metadata record backs and that a
    /// restart would hand to somebody else. What is lost is an *object* —
    /// garbage, which `M5`'s reaper collects — and never an acknowledged
    /// record, which is exactly the property FR-10 asks for.
    ///
    /// ⚠️ **A different fault from the one above.** A storm fails the `put`
    /// before the write; this one writes and then fails. A broker that
    /// answered them alike would be right about one by luck — and the
    /// assertion that separates them is the one reading the store's *contents*
    /// rather than its call count, because a counter cannot tell a `put` that
    /// failed early from one that failed late.
    #[tokio::test]
    async fn a_produce_whose_ack_was_lost_after_a_durable_write_is_still_refused() {
        let fixture = fixture(&["t"]).await;
        fixture.lose_the_next_ack();
        let body = produce_body(9, "t", -1, golden_batch());

        let response = replied(&fixture, 9, &body).await;

        assert_eq!(
            verdict(&response),
            (
                kafka_protocol::error::ResponseError::NotEnoughReplicas.code(),
                -1
            ),
            "the bytes are durable and the ack is not — a client hears the ack"
        );
        assert_eq!(
            fixture
                .cluster
                .high_watermark(&topic("t"), partition(0))
                .get(),
            0,
            "no offset was committed, so none may be handed out"
        );
        // ⚠️ **The store's contents, not its call count.** `CountingObjectStore`
        // counts on *invocation*, so a `put` that failed before writing counts
        // the same as one that failed after — which would make this case a
        // duplicate of the storm test above rather than the thing that
        // separates them.
        assert_eq!(
            fixture.store.inner().len(),
            1,
            "the bytes are in the bucket; only the acknowledgement was lost"
        );
    }

    /// ⚠️ **And a produce whose object landed but whose position did not is a
    /// *different* answer.** The records may be in the bucket; what is missing
    /// is the commit, so the client is sent to `Metadata` and back rather than
    /// told to retry into a coordinator that is not answering. Telling it
    /// `NONE` would hand out an offset nothing journalled.
    #[tokio::test]
    async fn a_produce_whose_commit_failed_is_refused_and_says_so_differently() {
        let fixture = with_dead_coordinator(&["t"]).await;
        let body = produce_body(9, "t", -1, golden_batch());

        let response = replied(&fixture, 9, &body).await;

        assert_eq!(
            verdict(&response),
            (
                kafka_protocol::error::ResponseError::LeaderNotAvailable.code(),
                -1
            )
        );
        assert_eq!(
            fixture.store.counts().count(Operation::Put),
            1,
            "the object did land — which is exactly why this is not the same \
             failure as a store that refused it"
        );
    }
}
