//! One partition's answer: the index lookup, the read, and the refusals.
//!
//! ⚠️ **Its own module because every refusal here is a decision about silence.**
//! Doc 12 §4.6 names the failure mode this milestone is written against — a
//! successful poll returning no records — and two of five bugs an independent
//! audit found in a peer system were exactly that. So a fault answers an error
//! code, an unhosted partition answers its own refusal, and only a partition
//! genuinely at its watermark answers `NONE` with nothing.

// ⚠️ `pub(crate)` inside a private module: `unreachable_pub` denies the `pub`
// clippy's `redundant_pub_crate` asks for.
#![allow(clippy::redundant_pub_crate)]

use crate::cluster::Cluster;
use oqueue_codec::error_codes;
use oqueue_core::{Offset, PartitionId, TopicId};

/// What one partition's read may price itself against.
///
/// ⚠️ **A constant because the request's own number is not available here**,
/// and that is `M3.22`'s row rather than a shortcut: `oqueue-codec`'s `Fetch`
/// decoder reads and discards both `max_bytes` fields (`fetch.rs`'s
/// `_max_bytes`), so plumbing the client's budget through is a codec change
/// and a handler change together. One MiB is librdkafka's default
/// `message.max.bytes`, so it is the number a default client already expects.
///
/// ⚠️ It bounds what `find_batches` can **price**, not what this handler will
/// spend — `ADR-0022`. The reader's own budget is the other half of `M3.22`.
const READ_BUDGET_BYTES: u64 = 1024 * 1024;

/// One partition's outcome, owned so the borrowed [`FetchResponsePartition`]
/// can point into the concatenated batch bytes.
pub(crate) struct PartitionOutcome {
    pub(crate) index: i32,
    pub(crate) error_code: i16,
    pub(crate) high_watermark: i64,
    pub(crate) last_stable_offset: i64,
    pub(crate) log_start_offset: i64,
    pub(crate) records: Vec<u8>,
}

/// One partition's answer: every batch and the watermark, or a refusal.
///
/// ⚠️ **The unset-offset sentinel is `-1`, not `0`** — the protocol's own
/// convention (`kafka-protocol`'s generated `PartitionData::default()`
/// agrees), and every refusal path below leaves `last_stable_offset` and
/// `log_start_offset` there rather than guessing a real value for a
/// partition that was never read. `high_watermark` is the one field a
/// refusal may still answer honestly (`OFFSET_OUT_OF_RANGE` reports the
/// real watermark the client overshot); the other two refusals report a
/// partition this broker never touched, so `0` would be as much a fiction
/// as `-1` is a confession.
pub(crate) async fn one_partition(
    cluster: &Cluster,
    topic: Option<&str>,
    unknown: i16,
    index: i32,
    fetch_offset: i64,
) -> PartitionOutcome {
    const OFFSET_UNSET: i64 = -1;
    let refused = |error_code: i16, high_watermark: i64| PartitionOutcome {
        index,
        error_code,
        high_watermark,
        last_stable_offset: OFFSET_UNSET,
        log_start_offset: OFFSET_UNSET,
        records: Vec::new(),
    };

    let Some(name) = topic else {
        return refused(unknown, 0);
    };
    let hosted = usize::try_from(index)
        .ok()
        .zip(cluster.partition_count(name))
        .is_some_and(|(index, count)| index < count);
    let Some((topic_id, partition)) = hosted
        .then(|| {
            TopicId::new(name.to_owned())
                .ok()
                .zip(PartitionId::new(index).ok())
        })
        .flatten()
    else {
        return refused(error_codes::UNKNOWN_TOPIC_OR_PARTITION, 0);
    };
    let high = cluster.high_watermark(&topic_id, partition).get();
    let Ok(start) = Offset::new(fetch_offset) else {
        // Negative: the client's bug, and the watermark is honest about where
        // the partition actually is.
        return refused(error_codes::OFFSET_OUT_OF_RANGE, high);
    };
    if fetch_offset > high {
        return refused(error_codes::OFFSET_OUT_OF_RANGE, high);
    }

    // ⚠️ The idle-poll case, and FR-12's zero-GET claim: at the watermark the
    // index names no batch, so `read` returns empty without touching object
    // storage. Asserted in this crate's suite through a counting store rather
    // than argued here.
    cluster
        .read(&topic_id, partition, start, READ_BUDGET_BYTES)
        .await
        .map_or_else(
            // ⚠️ **Never an empty partition.** A read that failed is a fault, and
            // doc 12 §4.6 names silent wrongness — a successful poll returning no
            // records — as the failure mode this whole milestone is written
            // against. `LEADER_NOT_AVAILABLE` sends the client to `Metadata` and
            // back rather than letting it conclude it is caught up.
            |_| refused(error_codes::LEADER_NOT_AVAILABLE, high),
            |records| PartitionOutcome {
                index,
                error_code: error_codes::NONE,
                high_watermark: high,
                // No transactions yet, so the stable offset IS the watermark —
                // and `ADR-0021`'s hazard H3 is that it must never be *ahead* of
                // it, which equality satisfies by construction.
                last_stable_offset: high,
                log_start_offset: 0,
                records,
            },
        )
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use crate::fetch::tests::{by_id, fetch_body, hosted, produced, replied};
    use crate::testing::{fixture, golden_batch, produce_one};
    use kafka_protocol::messages::FetchRequest;
    use kafka_protocol::messages::fetch_request::{FetchPartition, FetchTopic};
    use kafka_protocol::protocol::Encodable;
    use oqueue_core::Operation;

    /// ⚠️ **A read that failed is never an empty partition.** doc 12 §4.6
    /// names silent wrongness — a successful poll returning no records — as
    /// the failure mode this milestone is written against, and two of five
    /// bugs an independent audit found in a peer system were exactly that. A
    /// consumer told `NONE` with no records concludes it is caught up.
    #[tokio::test]
    async fn a_fetch_whose_object_read_failed_is_refused_not_answered_empty() {
        let fixture = fixture(&["t"]).await;
        produce_one(&fixture, "t", golden_batch()).await;
        fixture.break_store(4);

        let response = replied(&fixture, 13, &fetch_body(13, by_id(hosted(&fixture)), 0, 0)).await;

        let p = &response.responses[0].partitions[0];
        assert_eq!(
            p.error_code,
            kafka_protocol::error::ResponseError::LeaderNotAvailable.code(),
            "a fault must not look like the end of the log"
        );
        assert!(p.records.as_ref().is_none_or(bytes::Bytes::is_empty));
        assert_eq!(
            p.high_watermark, 2,
            "and the watermark still says where the log really is"
        );
    }
    /// ⚠️ **The same boundary as produce's.** A one-partition topic hosts `0`
    /// and not `1`; a fetch for `1` that answered `NONE` with empty records
    /// would tell a consumer it was caught up on a partition that is not there.
    #[tokio::test]
    async fn a_partition_past_the_topics_count_is_refused_not_answered_empty() {
        let fixture = produced().await;
        let mut request = FetchRequest::default();
        let mut t = FetchTopic::default();
        t.topic_id = hosted(&fixture);
        let mut p = FetchPartition::default();
        p.partition = 1;
        p.fetch_offset = 0;
        p.partition_max_bytes = 1 << 20;
        t.partitions.push(p);
        request.topics.push(t);
        let mut body = Vec::new();
        request.encode(&mut body, 13).expect("encodes");

        let response = replied(&fixture, 13, &body).await;

        let p = &response.responses[0].partitions[0];
        assert_eq!(
            p.error_code,
            kafka_protocol::error::ResponseError::UnknownTopicOrPartition.code()
        );
        // ⚠️ **`-1`, not `0`.** The protocol's own unset sentinel: this broker
        // never touched the partition, so `0` would be as much a fiction as
        // `-1` is a confession — and a consumer that read `0` as a real
        // position would rewind a partition it was never serving.
        assert_eq!(p.last_stable_offset, -1);
        assert_eq!(p.log_start_offset, -1);
    }
    /// ⚠️ **FR-12: a fetch at the high watermark issues zero GETs.** The index
    /// names no batch there, so the read path never reaches the store — the
    /// idle poll every consumer runs costs one index lookup and nothing else.
    #[tokio::test]
    async fn a_fetch_at_the_watermark_is_empty_and_issues_no_gets() {
        let fixture = produced().await;
        let before = fixture.store.counts().count(Operation::Get);

        let response = replied(&fixture, 13, &fetch_body(13, by_id(hosted(&fixture)), 2, 0)).await;

        let p = &response.responses[0].partitions[0];
        assert_eq!(p.error_code, 0);
        assert_eq!(p.high_watermark, 2);
        assert!(p.records.as_ref().is_none_or(bytes::Bytes::is_empty));
        assert_eq!(
            fixture.store.counts().count(Operation::Get),
            before,
            "the idle poll must not touch object storage"
        );
    }
}
