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

use crate::authz::{AuthzContext, topic_authorized};
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

/// One partition, refused with no offset — `-1` both ways, the same unset
/// sentinel every refusal in this handler answers with.
const fn refused_partition(index: i32, error_code: i16) -> ListOffsetsResponsePartition {
    ListOffsetsResponsePartition {
        index,
        error_code,
        timestamp: UNSET,
        offset: UNSET,
    }
}

/// Every requested topic and partition, answered.
///
/// ⚠️ **Split out so `handle` stays inside `code-structure.md`'s fifty
/// lines**, and because the per-topic hoist below is the interesting part
/// rather than a detail of framing a response.
fn answer_all(
    cluster: &Cluster,
    request: &oqueue_codec::listoffsets::ListOffsetsRequest<'_>,
    authz: &AuthzContext<'_>,
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
                // ⚠️ **`M9.12`: checked once per topic.** `handle`'s own
                // guard already refuses a null name, so every topic here has
                // one to check — the `is_none_or` fallback exists only for
                // that impossible case, matching `fetch`'s own shape.
                let authorized = topic.name.is_none_or(|name| topic_authorized(name, authz));
                topic
                    .partitions
                    .iter()
                    .map(|partition| {
                        if !authorized {
                            return refused_partition(
                                partition.index,
                                error_codes::TOPIC_AUTHORIZATION_FAILED,
                            );
                        }
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
pub(crate) fn handle(
    cluster: &Cluster,
    prelude: RequestPrelude,
    body: &[u8],
    authz: &AuthzContext<'_>,
) -> HandlerResponse {
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

    let outcomes = answer_all(cluster, &request, authz);

    let response = ListOffsetsResponse {
        topics: outcomes
            .iter()
            // ⚠️ **`filter_map`, not `expect`** (`M3.41`, `region.rs`'s own
            // precedent for the same shape). The early `Close` a few lines up
            // already refused any request whose topic name was `None`, so
            // every `TopicOutcome` reaching this point was built from a name
            // that exists — but a panic here would still be one refactor away
            // from being reachable, and `error-handling.md` does not carry an
            // exception for "proven safe today". Named as unreachable rather
            // than asserted: a topic that somehow had no name is dropped from
            // the reply, not a crash of the connection carrying every other
            // topic's answer.
            .filter_map(|topic| {
                Some(ListOffsetsResponseTopic {
                    name: topic.name.as_deref()?,
                    partitions: topic.partitions.clone(),
                })
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
    let refused = |error_code: i16| refused_partition(index, error_code);

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
        //
        // ⚠️ **A constant here becomes a livelock the day it is wrong.**
        // `M3.22` made a reaped read answer `OFFSET_OUT_OF_RANGE`, and a
        // consumer's escape from that is to ask for the earliest offset: told
        // `0`, it seeks there and is refused again, spending a page of GETs
        // per turn. `M5.md` carries it.
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
///   authoritative** — a different epoch, which since `M3.30` is the *only*
///   refusal this arm can receive: `admits` answers `Linearizable` before it
///   asks any freshness question, so a silent push stream never reaches here.
///   ⚠️ **This named two until `M3.37`** — a different epoch *or* a silent
///   push stream — and the second stopped being reachable at `M3.30`. This broker's index *is* the coordinator's, so
///   hearing one means the process is not the one it believes it is. Answering an offset
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
mod tests;
