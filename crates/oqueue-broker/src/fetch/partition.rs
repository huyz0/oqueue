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

use super::{TopicOutcome, resolved_name};
use crate::cluster::Cluster;
use crate::read::Spend;
use oqueue_codec::error_codes;
use oqueue_core::{Error, Offset, PartitionId, TopicId};

/// What one partition's read may price itself against.
///
/// ⚠️ ~~**A constant because the request's own number is not available
/// here**~~ — **the client's number is decoded now** (`M3.22`), and this is
/// what it is *clamped to*. `max_bytes` is an `i32`, so a client may name two
/// gigabytes; a broker that obliged would let one frame decide how much memory
/// it uses. One MiB is librdkafka's default `message.max.bytes`, so it is the
/// number a default client already expects to be able to receive.
///
/// ⚠️ **Do not remove the clamp on the grounds that the client's number is
/// available.** Its being available is exactly why a ceiling is needed.
pub(crate) const READ_BUDGET_BYTES: u64 = 1024 * 1024;

/// One partition's outcome, owned so the borrowed [`FetchResponsePartition`]
/// can point into the concatenated batch bytes.
pub(crate) struct PartitionOutcome {
    /// What this partition cost the request's budget.
    ///
    /// ⚠️ **Not the same as the bytes returned, and that difference is a
    /// bound.** A read that *failed* returns nothing but has already paid a
    /// GET — two on the 404 path — so charging only the bytes would let a
    /// client repeat a failing partition for free and turn one frame into as
    /// many object-storage reads as it has entries. A refusal decided *before*
    /// a read costs nothing: it touched no store, and charging it would let one
    /// unknown topic starve the partitions behind it.
    pub(crate) cost: u64,
    pub(crate) index: i32,
    pub(crate) error_code: i16,
    pub(crate) high_watermark: i64,
    pub(crate) last_stable_offset: i64,
    pub(crate) log_start_offset: i64,
    pub(crate) records: Vec<u8>,
}

/// Every requested partition, refused with one code.
///
/// ⚠️ **Shaped like a read rather than like an error**, because a client parses
/// one response either way: the same topics in the same order, each partition
/// carrying the refusal instead of records.
pub(crate) fn refuse_all(
    cluster: &Cluster,
    request: &oqueue_codec::fetch::FetchRequest<'_>,
    version: i16,
    error_code: i16,
) -> Vec<TopicOutcome> {
    request
        .topics
        .iter()
        .map(|topic| {
            // ⚠️ **Resolved once per topic, not once per partition.** The
            // partition count is the client's to choose and a 16 MiB frame can
            // name hundreds of thousands, so a registry lock and a `String`
            // allocation per entry would be paid on the path that returns *no*
            // records — the cheap answer costing more contention than the read
            // it replaced. `one_topic` and `watermarks`, both in `mod.rs`, have
            // the same shape for the same reason.
            let resolved = resolved_name(cluster, topic, version);
            TopicOutcome {
                name: if version >= 13 {
                    None
                } else {
                    topic.name.map(str::to_owned)
                },
                topic_id: topic.topic_id,
                partitions: topic
                    .partitions
                    .iter()
                    .map(|partition| {
                        refused(cluster, resolved.as_deref(), partition.index, error_code)
                    })
                    .collect(),
            }
        })
        .collect()
}

/// One partition, refused — with the real watermark when this broker knows it.
///
/// ⚠️ **The watermark is answered even on a refusal, when it is known.** A
/// client that hears "come back" still needs to know where the log is; the two
/// *offset* fields stay at the unset sentinel because this broker did not read
/// the partition, and inventing a `0` for them is the doc 13 §8 inversion.
pub(crate) fn refused(
    cluster: &Cluster,
    topic: Option<&str>,
    index: i32,
    error_code: i16,
) -> PartitionOutcome {
    let high = topic
        .and_then(|name| TopicId::new(name.to_owned()).ok())
        .zip(PartitionId::new(index).ok())
        .map_or(0, |(topic, partition)| {
            cluster.high_watermark(&topic, partition).get()
        });
    PartitionOutcome {
        cost: 0,
        index,
        error_code,
        high_watermark: high,
        last_stable_offset: -1,
        log_start_offset: -1,
        records: Vec::new(),
    }
}

/// Where this partition is, and whether `fetch_offset` is in range.
///
/// ⚠️ **Split out so `one_partition` stays inside `code-structure.md`'s fifty
/// lines**, and because "does this partition exist, and is that offset in
/// range" is a different question from "what does reading it return".
///
/// # Errors
///
/// The Kafka error code to refuse with, and the watermark to report beside it
/// — ⚠️ **the real one wherever this broker knows it**: a client that overshot
/// needs to hear where the log actually ends, and the `0` on the paths that
/// never reached a partition is a confession rather than a guess.
fn resolve(
    cluster: &Cluster,
    topic: Option<&str>,
    unknown: i16,
    index: i32,
    fetch_offset: i64,
) -> Result<(TopicId, PartitionId, i64, Offset), (i16, i64)> {
    let Some(name) = topic else {
        return Err((unknown, 0));
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
        return Err((error_codes::UNKNOWN_TOPIC_OR_PARTITION, 0));
    };
    let high = cluster.high_watermark(&topic_id, partition).get();
    let Ok(start) = Offset::new(fetch_offset) else {
        // Negative: the client's bug, and the watermark is honest about where
        // the partition actually is.
        return Err((error_codes::OFFSET_OUT_OF_RANGE, high));
    };
    if fetch_offset > high {
        return Err((error_codes::OFFSET_OUT_OF_RANGE, high));
    }
    Ok((topic_id, partition, high, start))
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
    asked: &oqueue_codec::fetch::FetchPartition,
    spend: &mut Spend<'_>,
) -> PartitionOutcome {
    const OFFSET_UNSET: i64 = -1;
    let index = asked.index;
    let fetch_offset = asked.fetch_offset;
    // ⚠️ `cost: 0` — every refusal this closure names is decided *before* a
    // read, so none of them touched object storage.
    let refused = |error_code: i16, high_watermark: i64| PartitionOutcome {
        cost: 0,
        index,
        error_code,
        high_watermark,
        last_stable_offset: OFFSET_UNSET,
        log_start_offset: OFFSET_UNSET,
        records: Vec::new(),
    };

    let (topic_id, partition, high, start) =
        match resolve(cluster, topic, unknown, index, fetch_offset) {
            Ok(resolved) => resolved,
            Err((code, high)) => return refused(code, high),
        };

    // ⚠️ The idle-poll case, and FR-12's zero-GET claim: at the watermark the
    // index names no batch, so the read returns empty without touching object
    // storage. Asserted in this crate's suite through a counting store rather
    // than argued here.
    // ⚠️ **A partition with nothing left to spend touches no store**, and the
    // check for that is inside `read` rather than here: its first iteration
    // stops before the GET when the allowance is zero and the one-batch
    // exemption is spent. A guard here as well would be a branch nothing can
    // take, which reads as a case that has been handled.
    match cluster
        .read_or_refresh(&topic_id, partition, start, spend)
        .await
    {
        Ok(read) => PartitionOutcome {
            // ⚠️ **What was fetched, not what is returned.** A history read
            // pulls whole bundles and hands back one partition's slice; a
            // budget charged the slice would let sixty-four bundles come off
            // the store for a megabyte of records.
            cost: read.fetched.max(read.records.len() as u64),
            index,
            error_code: error_codes::NONE,
            high_watermark: high,
            // No transactions yet, so the stable offset IS the watermark — and
            // `ADR-0021`'s hazard H3 is that it must never be *ahead* of it,
            // which equality satisfies by construction.
            last_stable_offset: high,
            log_start_offset: 0,
            records: read.records,
        },
        // ⚠️ **A confirmed reap is `OFFSET_OUT_OF_RANGE`.** Hazard H4: object
        // ids are never reused, so a missing object means those offsets were
        // deleted, not that they have not been written. ⚠️ **What makes it
        // *confirmed* here is that there is nothing to refresh** — the index
        // this read consulted is the coordinator's own, in this process, and
        // nothing in `M3` removes an entry from it. `M7`'s follower reads a
        // cache instead, and `read_or_refresh` is where its round trip goes;
        // until then a second read would pay a GET to be told the same thing. The
        // client's own `auto.offset.reset` is what decides where it goes next,
        // which is the point of telling it the truth rather than an empty
        // partition.
        // ⚠️ **A failed read costs no *budget*, and does not need to.** What
        // stops a client repeating a failing partition is the object cache,
        // which remembers the failure: the second entry issues no GET. Charging
        // the allowance instead would let one transient fault spend a whole
        // request's budget on bytes nobody fetched, and answer every healthy
        // partition behind it with `NONE` and no records — a successful poll
        // returning nothing, from the mechanism written to prevent exactly that.
        Err(Error::ObjectNotFound { .. }) => refused(error_codes::OFFSET_OUT_OF_RANGE, high),
        // ⚠️ **Never an empty partition.** Any other failed read is a fault,
        // and doc 12 §4.6 names silent wrongness — a successful poll returning
        // no records — as the failure mode this whole milestone is written
        // against. `OFFSET_NOT_AVAILABLE` is in the Java consumer's enumerated
        // fetch-error set and retried; it does not let a client conclude it is
        // caught up.
        Err(_) => refused(error_codes::OFFSET_NOT_AVAILABLE, high),
    }
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

        let response = replied(&fixture, 13, &fetch_body(13, by_id(&fixture), 0, 0)).await;

        let p = &response.responses[0].partitions[0];
        assert_eq!(
            p.error_code,
            kafka_protocol::error::ResponseError::OffsetNotAvailable.code(),
            "a fault must not look like the end of the log"
        );
        assert_ne!(
            p.error_code,
            kafka_protocol::error::ResponseError::LeaderNotAvailable.code(),
            "and it must be a code the Java consumer's fetch dispatch knows: \
             code 5 falls through to an IllegalStateException out of poll()"
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

        let response = replied(&fixture, 13, &fetch_body(13, by_id(&fixture), 2, 0)).await;

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
