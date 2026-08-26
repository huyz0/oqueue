//! `ListOffsets` v1-9: where a partition begins and ends, answered
//! authoritatively.
//!
//! ⚠️ **Hazard H1, and it is the reason this API is not served off a cache.**
//! A consumer computes lag as `log_end_offset - position`. An end below where
//! the consumer already stands makes that subtraction negative — and a
//! dashboard showing negative lag looks like a system with nothing to do,
//! which is the worst possible disguise for a system that is behind. Doc 12
//! §4.6 lists it first, and `M3.md`'s risks say plainly: route it to the
//! authoritative coordinator, never derive it from cache.
//!
//! ⚠️ **In this broker the authoritative source and the local index are the
//! same object**, because the coordinator folds before it acks and nothing
//! else writes. That makes the round trip free *today* and is exactly why the
//! decision still goes through [`CacheState::admits`](oqueue_core::CacheState::admits): `M7`'s follower has an
//! index that is a cache, and a handler that had read `end_offset` directly
//! would inherit an answer only a coordinator may give, with nothing to
//! notice.
//!
//! ⚠️ **A real timestamp is not answered with a guess.** `EARLIEST` and
//! `LATEST` are sentinels this broker can answer exactly; "the first offset at
//! or after 10:32" needs a time index nothing here keeps, so it is refused
//! rather than approximated — `M5`'s retention work is where an index by time
//! would come from, and answering the wrong offset would send a consumer to
//! the wrong place silently.

use crate::cluster::Cluster;
use crate::connection::HandlerResponse;
use oqueue_codec::apikey::ApiKey;
use oqueue_codec::error_codes;
use oqueue_codec::frame::{RequestPrelude, encode_response_header};
use oqueue_codec::listoffsets::{
    EARLIEST_TIMESTAMP, LATEST_TIMESTAMP, ListOffsetsResponse, ListOffsetsResponsePartition,
    ListOffsetsResponseTopic, decode_request,
};
use oqueue_core::{PartitionId, ReadMode, RefreshReason, TopicId};

/// ⚠️ **The unset sentinel is `-1`**, and it is what every refusal answers
/// with — a plausible-looking `0` on an error path is the inversion doc 13 §8
/// records, and for this API a fabricated `0` end offset is negative lag for
/// every consumer past the beginning.
const UNSET: i64 = -1;

/// One topic's response, owned so the borrowed response can point into its
/// echoed name.
struct TopicOutcome {
    name: Option<String>,
    partitions: Vec<ListOffsetsResponsePartition>,
}

/// Every requested topic and partition, answered.
///
/// ⚠️ **Split out so `handle` stays inside `code-structure.md`'s fifty
/// lines**, and because the per-topic hoist below is the interesting part
/// rather than a detail of framing a response.
fn answer_all(
    cluster: &Cluster,
    request: &oqueue_codec::listoffsets::ListOffsetsRequest<'_>,
) -> Vec<TopicOutcome> {
    request
        .topics
        .iter()
        .map(|topic| TopicOutcome {
            name: topic.name.map(str::to_owned),
            partitions: {
                let topic_id = topic
                    .name
                    .and_then(|name| TopicId::new(name.to_owned()).ok());
                let count = topic.name.and_then(|name| cluster.partition_count(name));
                topic
                    .partitions
                    .iter()
                    .map(|partition| {
                        let hosted = count.is_some_and(|count| {
                            usize::try_from(partition.index).is_ok_and(|index| index < count)
                        });
                        one_partition(
                            cluster,
                            topic_id.as_ref(),
                            hosted,
                            partition.index,
                            partition.timestamp,
                        )
                    })
                    .collect()
            },
        })
        .collect()
}

/// Decodes, resolves, answers — or closes on a malformed body.
pub(crate) fn handle(cluster: &Cluster, prelude: RequestPrelude, body: &[u8]) -> HandlerResponse {
    let version = prelude.api_version;
    let Ok(request) = decode_request(body, version) else {
        return HandlerResponse::Close;
    };
    // The two isolation levels the protocol defines; anything else is a
    // malformed request from a client that negotiated fine, and closes — the
    // dispatcher's policy for every unanswerable shape.
    if !matches!(request.isolation_level, 0 | 1) {
        return HandlerResponse::Close;
    }
    // ⚠️ **A null topic name has no answer, so it closes.** The field is
    // nullable on the request wire and **not** nullable on the response wire,
    // so echoing a null back frames a body no client can parse: the Java
    // client throws in its response parser and librdkafka reports a protocol
    // read error, and either way the connection dies having been sent bytes it
    // could not read. Closing is the same policy every other unanswerable
    // shape gets, and it is the one that leaves the client able to tell what
    // happened.
    if request.topics.iter().any(|topic| topic.name.is_none()) {
        return HandlerResponse::Close;
    }

    let outcomes = answer_all(cluster, &request);

    let response = ListOffsetsResponse {
        topics: outcomes
            .iter()
            .map(|topic| ListOffsetsResponseTopic {
                name: topic.name.as_deref(),
                partitions: topic.partitions.clone(),
            })
            .collect(),
    };

    let mut out = Vec::new();
    if encode_response_header(
        &mut out,
        ApiKey::ListOffsets,
        version,
        prelude.correlation_id,
    )
    .is_err()
    {
        return HandlerResponse::Close;
    }
    oqueue_codec::listoffsets::encode_response(&mut out, version, &response);
    HandlerResponse::Reply(out)
}

/// One partition's answer.
///
/// ⚠️ **The topic is resolved once per *topic*, not once per partition.** The
/// request's partition count is the client's to choose and a 16 MiB frame can
/// name a million of them, so a registry lock and a `String` allocation per
/// entry would hold a runtime worker for hundreds of milliseconds with no
/// yield point (`async-concurrency.md` rule 3). `fetch`'s walk already has
/// this shape.
fn one_partition(
    cluster: &Cluster,
    topic_id: Option<&TopicId>,
    hosted: bool,
    index: i32,
    timestamp: i64,
) -> ListOffsetsResponsePartition {
    let refused = |error_code: i16| ListOffsetsResponsePartition {
        index,
        error_code,
        timestamp: UNSET,
        offset: UNSET,
    };

    let Some((topic_id, partition)) = topic_id
        .zip(PartitionId::new(index).ok())
        .filter(|_| hosted)
    else {
        return refused(error_codes::UNKNOWN_TOPIC_OR_PARTITION);
    };

    let offset = match timestamp {
        // ⚠️ **`0`, and it will not always be.** Nothing is deleted yet, so the
        // first offset still held is the first ever assigned; `M5`'s retention
        // is where a log start offset stops being a constant, and this is the
        // one place that has to change when it does.
        EARLIEST_TIMESTAMP => 0,
        LATEST_TIMESTAMP => match authoritative_end_offset(cluster, topic_id, partition) {
            Ok(offset) => offset,
            Err(code) => return refused(code),
        },
        // A real timestamp needs an index by time. Refusing names the gap;
        // approximating would send a consumer to the wrong offset in silence.
        //
        // ⚠️ **`UNSUPPORTED_VERSION`, not `UNSUPPORTED_FOR_MESSAGE_FORMAT`**,
        // and the difference is which caller hears it. The Java consumer maps
        // code 43 in a `ListOffsets` response to *no offset for that
        // partition* — `offsetsForTimes` returns null, which an application
        // cannot tell from a truthful "no record at or after that time", so it
        // takes its fallback branch and silently skips the data it meant to
        // replay. That is the silence this refusal exists to avoid, arriving
        // by another door. Code 35 is raised by the consumer and the admin
        // client alike.
        _ => return refused(error_codes::UNSUPPORTED_VERSION),
    };
    ListOffsetsResponsePartition {
        index,
        error_code: error_codes::NONE,
        // ⚠️ `-1`: the timestamp of the record *at* `offset`, which needs the
        // record. For `LATEST` there is no such record at all.
        timestamp: UNSET,
        offset,
    }
}

/// Where the partition ends, from the source a `Linearizable` read requires.
///
/// ⚠️ **A `match`, not an assertion**, and the difference is the whole finding
/// that made it one: a `debug_assert!` on a condition that is a tautology
/// today is compiled out of release builds, so it enforces nothing when it
/// stops being a tautology — and what it would do in a debug build is panic
/// inside a request handler on ordinary client input.
///
/// The arms are what a `Linearizable` read means:
///
/// - [`RefreshReason::Authoritative`] is the only refusal this mode ever gets
///   today, and it says "go to the source". In this broker the source is the
///   coordinator's own index, because the coordinator folds before it acks and
///   nothing else writes — so the round trip is a local read. `M7`'s follower
///   is where it becomes a message.
/// - **Any other refusal is a cache that is wrong rather than merely not
///   authoritative** — a different epoch, or a push stream that has gone
///   silent — and this broker's index *is* the coordinator's, so hearing one
///   means the process is not the one it believes it is. Answering an offset
///   then is how a stale log-end-offset becomes negative consumer lag, so it
///   refuses instead.
/// - `Ok(())` cannot happen for `Linearizable` and would mean a cache had been
///   allowed to answer. It is refused rather than trusted, for the same reason.
///
/// # Errors
///
/// The Kafka error code to refuse this partition with.
fn authoritative_end_offset(
    cluster: &Cluster,
    topic: &TopicId,
    partition: PartitionId,
) -> Result<i64, i16> {
    match cluster
        .cache_state()
        .admits(ReadMode::Linearizable, cluster.epoch())
    {
        Err(RefreshReason::Authoritative) => Ok(cluster.high_watermark(topic, partition).get()),
        Err(_) | Ok(()) => Err(error_codes::LEADER_NOT_AVAILABLE),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::handle;
    use crate::connection::HandlerResponse;
    use crate::testing::{Fixture, fixture, golden_batch, produce_one};
    use kafka_protocol::messages::list_offsets_request::{ListOffsetsPartition, ListOffsetsTopic};
    use kafka_protocol::messages::{ListOffsetsRequest, ListOffsetsResponse, TopicName};
    use kafka_protocol::protocol::{Decodable, Encodable, StrBytes};
    use oqueue_codec::frame::RequestPrelude;

    const VERSION: i16 = 9;

    fn body(topic: &str, partition: i32, timestamp: i64) -> Vec<u8> {
        let mut request = ListOffsetsRequest::default();
        request.replica_id = kafka_protocol::messages::BrokerId(-1);
        let mut t = ListOffsetsTopic::default();
        t.name = TopicName(StrBytes::from_string(topic.to_owned()));
        let mut p = ListOffsetsPartition::default();
        p.partition_index = partition;
        p.timestamp = timestamp;
        t.partitions.push(p);
        request.topics.push(t);
        let mut out = Vec::new();
        request.encode(&mut out, VERSION).expect("encodes");
        out
    }

    fn replied(fixture: &Fixture, body: &[u8]) -> ListOffsetsResponse {
        let prelude = RequestPrelude {
            api_key: 2,
            api_version: VERSION,
            correlation_id: 11,
        };
        let HandlerResponse::Reply(out) = handle(&fixture.cluster, prelude, body) else {
            panic!("a ListOffsets with a legal isolation level replies");
        };
        let mut rest = &out[5..];
        let response = ListOffsetsResponse::decode(&mut rest, VERSION).expect("decodes");
        assert!(rest.is_empty());
        response
    }

    /// ⚠️ **Hazard H1: the end offset is the real one, not a cached one.**
    /// After three produces the log ends at 6, and a client computing lag from
    /// anything smaller gets a negative number — the silent wrongness this
    /// milestone is written against.
    #[tokio::test]
    async fn latest_is_the_offset_after_the_last_record() {
        let fixture = fixture(&["t"]).await;
        for _ in 0..3 {
            produce_one(&fixture, "t", golden_batch()).await;
        }

        let response = replied(&fixture, &body("t", 0, -1));

        let p = &response.topics[0].partitions[0];
        assert_eq!(p.error_code, 0);
        assert_eq!(p.offset, 6, "two records per produce, three produces");
    }

    /// ⚠️ **And it tracks, rather than being read once.** A `ListOffsets` after
    /// a further produce must move; one that did not would be exactly the
    /// stale answer H1 describes.
    #[tokio::test]
    async fn latest_moves_when_the_log_does() {
        let fixture = fixture(&["t"]).await;
        produce_one(&fixture, "t", golden_batch()).await;
        assert_eq!(
            replied(&fixture, &body("t", 0, -1)).topics[0].partitions[0].offset,
            2
        );

        produce_one(&fixture, "t", golden_batch()).await;

        assert_eq!(
            replied(&fixture, &body("t", 0, -1)).topics[0].partitions[0].offset,
            4
        );
    }

    /// ⚠️ **`EARLIEST` is where the log still begins**, which is `0` until
    /// something is deleted — `M5`'s retention is what makes it move.
    #[tokio::test]
    async fn earliest_is_the_first_offset_still_held() {
        let fixture = fixture(&["t"]).await;
        produce_one(&fixture, "t", golden_batch()).await;

        let p = &replied(&fixture, &body("t", 0, -2)).topics[0].partitions[0];
        assert_eq!(p.error_code, 0);
        assert_eq!(p.offset, 0);
    }

    /// ⚠️ **An empty partition ends where it begins.** `EARLIEST == LATEST ==
    /// 0` is what tells a consumer there is nothing rather than something it
    /// cannot see.
    #[tokio::test]
    async fn an_empty_partition_reports_zero_both_ways() {
        let fixture = fixture(&["t"]).await;
        for timestamp in [-1_i64, -2] {
            let p = &replied(&fixture, &body("t", 0, timestamp)).topics[0].partitions[0];
            assert_eq!(p.error_code, 0, "ts {timestamp}");
            assert_eq!(p.offset, 0, "ts {timestamp}");
        }
    }

    /// ⚠️ **A wall-clock timestamp is refused, not approximated.** Answering
    /// the nearest offset would send a consumer somewhere it did not ask for,
    /// silently; there is no index by time to answer it from.
    ///
    /// ⚠️ **And refused with a code the consumer *raises*.** The obvious
    /// choice, `UNSUPPORTED_FOR_MESSAGE_FORMAT`, is the one the Java consumer
    /// turns back into silence: `offsetsForTimes` records no offset for the
    /// partition and returns null, which an application cannot tell from a
    /// truthful "nothing at or after that time". Picking a code the client
    /// swallows would undo the whole argument for refusing.
    #[tokio::test]
    async fn a_real_timestamp_is_refused_with_a_code_the_client_cannot_swallow() {
        let fixture = fixture(&["t"]).await;
        produce_one(&fixture, "t", golden_batch()).await;

        let p = &replied(&fixture, &body("t", 0, 1_700_000_000_000)).topics[0].partitions[0];
        assert_eq!(
            p.error_code,
            kafka_protocol::error::ResponseError::UnsupportedVersion.code()
        );
        assert_ne!(
            p.error_code,
            kafka_protocol::error::ResponseError::UnsupportedForMessageFormat.code(),
            "code 43 is the one `offsetsForTimes` turns into a null"
        );
        assert_eq!(p.offset, -1, "a refusal answers -1, never a plausible 0");
    }

    /// ⚠️ **An unknown topic or partition is refused with `-1`.** A `0` here is
    /// the doc 13 §8 inversion in its most damaging place: a consumer told a
    /// partition it cannot reach ends at 0 computes negative lag forever.
    #[tokio::test]
    async fn an_unknown_topic_or_partition_answers_minus_one() {
        let fixture = fixture(&["t"]).await;
        for (topic, partition) in [("ghost", 0), ("t", 1), ("t", -1)] {
            let p = &replied(&fixture, &body(topic, partition, -1)).topics[0].partitions[0];
            assert_eq!(
                p.error_code,
                kafka_protocol::error::ResponseError::UnknownTopicOrPartition.code(),
                "{topic}/{partition}"
            );
            assert_eq!(p.offset, -1, "{topic}/{partition}");
        }
    }

    /// ⚠️ **A null topic name closes rather than being echoed.** The field is
    /// nullable on the request wire and not nullable on the response wire, so
    /// echoing it frames a body no client can parse — the Java client throws
    /// in its response parser, librdkafka reports a protocol read error, and
    /// the connection dies having been handed bytes it could not read. A close
    /// is the answer every other unanswerable shape gets.
    #[tokio::test]
    async fn a_null_topic_name_closes_rather_than_framing_an_unparsable_reply() {
        let fixture = fixture(&["t"]).await;
        // The dependency's encoder will not write a null here, so the bytes
        // are hand-built: `replica_id`, `isolation_level`, then a compact
        // array of one topic whose name is the null marker.
        let mut out = Vec::new();
        out.extend_from_slice(&(-1_i32).to_be_bytes()); // replica_id
        out.push(0); // isolation_level
        out.push(2); // compact array length 1
        out.push(0); // compact nullable string: null
        out.push(1); // compact array of partitions: empty
        out.push(0); // tagged fields (topic)
        out.push(0); // tagged fields (request)
        let prelude = RequestPrelude {
            api_key: 2,
            api_version: VERSION,
            correlation_id: 11,
        };

        assert!(matches!(
            handle(&fixture.cluster, prelude, &out),
            HandlerResponse::Close
        ));
    }

    /// ⚠️ **An undefined isolation level closes**, the dispatcher's policy for
    /// every shape it cannot answer.
    #[tokio::test]
    async fn an_undefined_isolation_level_closes_the_connection() {
        let fixture = fixture(&["t"]).await;
        let mut request = ListOffsetsRequest::default();
        request.replica_id = kafka_protocol::messages::BrokerId(-1);
        request.isolation_level = 7;
        let mut out = Vec::new();
        request.encode(&mut out, VERSION).expect("encodes");
        let prelude = RequestPrelude {
            api_key: 2,
            api_version: VERSION,
            correlation_id: 11,
        };
        assert!(matches!(
            handle(&fixture.cluster, prelude, &out),
            HandlerResponse::Close
        ));
    }
}
