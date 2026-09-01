//! `Produce` v3-13 against the real write path: verify, bundle, PUT, commit.
//!
//! ⚠️ **One request is one object and one metadata record** — FR-32. Every
//! partition that passes the ingest rule goes into a single
//! [`BundleBuilder`], so a produce spanning N topics costs one PUT rather than
//! N. Doc 12 prices a PUT far above the bytes in it; that ratio is the cost
//! model, and this handler is where it is spent.
//!
//! ⚠️ **The offset a client is told is the one the *commit* assigned**, not
//! one this handler picked. `ADR-0020` puts the serialization point at the log
//! append, so the base offsets arrive in the
//! [`CommitAck`](oqueue_coordinator::CommitAck) after the object is
//! durable — which is the whole of "no offset is externally visible before its
//! log record commits". Every refusal answers `-1`, never `0`: a
//! plausible-looking offset on an error path turns an availability bug into a
//! safety bug (doc 13 §8).
//!
//! ⚠️ **`acks=0` sends no response at all** — the protocol's fire-and-forget
//! mode, and the case that forced the [`crate::Handler`] seam's final shape:
//! answering it would desync the client's read stream, so the outcome is
//! `Silent`, not an empty reply. ⚠️ It still flushes, and still waits for the
//! commit before returning: the alternative is a handler that returns while a
//! flush it owns is unowned, which `async-concurrency.md` rule 13 refuses.

mod answer;

use crate::cluster::{Cluster, FlushError};
use crate::connection::HandlerResponse;
use crate::ingest::verify;
use crate::session::Session;
use answer::{PartitionSlot, PushOutcome, Slot, answer};
use oqueue_codec::apikey::ApiKey;
use oqueue_codec::error_codes;
use oqueue_codec::frame::{RequestPrelude, encode_response_header};
use oqueue_codec::metadata::TopicIdentity;
use oqueue_codec::produce::{ProduceResponse, ProduceResponseTopic, decode_request};
use oqueue_core::{BundleBuilder, PartitionId, PushedRecords, TopicId};
use std::collections::HashSet;

/// The bundle this request is filling, and what is already in it.
///
/// ⚠️ **The set is not bookkeeping.** A bundle's regions carry no offsets —
/// offsets are assigned at commit, after the object is written — so two
/// regions for one `(topic, partition)` in one object leave the read path
/// nothing to say which of them an `ObjectRef` names. `Cluster::read` refuses
/// such an object; this is what stops one being written. A `Produce` request
/// carrying a partition twice is malformed, and the second entry is what is
/// refused, so the first still lands.
struct Pending {
    bundle: BundleBuilder,
    seen: HashSet<(String, i32)>,
}

/// One topic's response, owned so the borrowed [`ProduceResponseTopic`] can
/// point into its echoed name.
struct TopicOutcome {
    name: Option<String>,
    topic_id: [u8; 16],
    partitions: Vec<PartitionSlot>,
}

/// The one store call, and the watermark it raises.
///
/// ⚠️ **Only when something passed.** An empty bundle cannot be sealed —
/// `Error::EmptyBundle` — and committing no spans would burn a `CommitVersion`
/// on a record saying nothing happened.
async fn flush(
    cluster: &Cluster,
    session: &Session,
    pending: Pending,
) -> Result<Vec<PushOutcome>, FlushError> {
    if pending.bundle.is_empty() {
        return Ok(Vec::new());
    }
    cluster.flush(pending.bundle).await.map(|ack| {
        // ⚠️ **Remembered before the response is written**, which is the
        // ordering hazard H2 turns on: a client that reads on the same
        // connection the moment it sees this ack must find the bar already
        // raised. Setting it after the write would leave a window in which the
        // client has the offset and this broker has not yet promised to serve
        // it.
        //
        // ⚠️ **Only when something was actually assigned** (`M11.6` review).
        // An all-rejected commit (`Allocator::admit` excluded every span) can
        // leave `CoordinatorLoop::serve` with nothing ever staged, journaled,
        // or published — its ack's version is `last_committed`'s fallback for
        // a coordinator that has *never* committed anything, which is never
        // itself published. Observing that watermark regardless would raise
        // this session's read-your-writes bar to a version `IndexWatch`'s own
        // `AtLeast` wait can never see satisfied (it requires the index to
        // have applied *something* first, whatever the version), stalling
        // this connection's next fetch for a write that never happened. A
        // produce answered entirely in refusals has nothing for read-your-
        // writes to guarantee, so it raises no bar at all.
        if ack
            .outcomes()
            .iter()
            .any(|outcome| matches!(outcome, oqueue_coordinator::SpanOutcome::Assigned(_)))
        {
            session.observe(ack.watermark());
        }
        // ⚠️ **`outcomes`, not `assignments`, from `M11.6`** — a rejected
        // producer sequence excludes its span from `assignments` entirely
        // (`CommitAck`'s own doc), which would silently shift every later
        // region's offset onto the wrong pushed position. `outcomes` has one
        // entry per pushed region, in push order, with no exclusions.
        ack.outcomes()
            .iter()
            .map(|outcome| match outcome {
                oqueue_coordinator::SpanOutcome::Assigned(assignment) => {
                    PushOutcome::Assigned(assignment.base_offset().get())
                }
                oqueue_coordinator::SpanOutcome::Rejected(reason) => {
                    PushOutcome::from_rejection(*reason)
                }
            })
            .collect()
    })
}

/// Whether this request has a shape no response can express.
///
/// ⚠️ **A null topic name has no answer, so it closes** — the third handler to
/// need this, and the reason `M3.40` also gave the codec a writer that cannot
/// express a null. Below v13 the field is nullable on the request wire and
/// **not** nullable on the response wire, so echoing it frames a body no client
/// can parse: the Java client throws in its response parser and librdkafka
/// reports a protocol read error, and either way the connection dies having
/// been handed bytes it could not read.
///
/// ⚠️ **Below v13 only, because above it there is no name to be null**: the
/// wire carries an id, the decoder reads no name, and the response echoes the
/// id. A check at every version would be a branch nothing can take.
fn unanswerable(request: &oqueue_codec::produce::ProduceRequest<'_>, version: i16) -> bool {
    version <= 12 && request.topics.iter().any(|topic| topic.name.is_none())
}

/// Decodes, verifies, flushes, answers — or stays silent for `acks=0`.
pub(crate) async fn handle(
    cluster: &Cluster,
    session: &Session,
    prelude: RequestPrelude,
    body: &[u8],
) -> HandlerResponse {
    let version = prelude.api_version;
    let Ok(request) = decode_request(body, version) else {
        return HandlerResponse::Close;
    };
    if unanswerable(&request, version) {
        return HandlerResponse::Close;
    }
    // The protocol's three acks: 0, 1, -1. Anything else is answered
    // INVALID_REQUIRED_ACKS per partition with nothing stored.
    let acks_valid = matches!(request.acks, -1..=1);

    let mut pending = Pending {
        bundle: BundleBuilder::new(),
        seen: HashSet::new(),
    };
    let outcomes: Vec<TopicOutcome> = request
        .topics
        .iter()
        .map(|topic| one_topic(cluster, &mut pending, topic, version, acks_valid))
        .collect();

    let flushed = flush(cluster, session, pending).await;

    if request.acks == 0 {
        return HandlerResponse::Silent;
    }
    let response = ProduceResponse {
        topics: outcomes
            .iter()
            // ⚠️ **`filter_map`, not `expect`** (`M3.41`, `region.rs`'s own
            // precedent). `unanswerable` above already refused any request
            // whose topic name was `None` at a version that needs one, so a
            // `Name(None)` here cannot happen — and a panic one refactor away
            // from being reachable is what `error-handling.md` forbids even
            // for a proven-safe path. Named as unreachable rather than
            // asserted: dropped from the reply, not a crash of the
            // connection carrying every other topic's answer.
            .filter_map(|topic| {
                let identity = if version <= 12 {
                    TopicIdentity::Name(topic.name.as_deref()?)
                } else {
                    TopicIdentity::Id(topic.topic_id)
                };
                Some(ProduceResponseTopic {
                    identity,
                    partitions: topic
                        .partitions
                        .iter()
                        .map(|p| answer(p, flushed.as_ref()))
                        .collect(),
                })
            })
            .collect(),
    };

    let mut out = Vec::new();
    if encode_response_header(&mut out, ApiKey::Produce, version, prelude.correlation_id).is_err() {
        return HandlerResponse::Close;
    }
    oqueue_codec::produce::encode_response(&mut out, version, &response);
    HandlerResponse::Reply(out)
}

/// One topic's outcome: resolve its addressing, then every partition.
fn one_topic(
    cluster: &Cluster,
    pending: &mut Pending,
    topic: &oqueue_codec::produce::ProduceTopic<'_>,
    version: i16,
    acks_valid: bool,
) -> TopicOutcome {
    // From v13 the wire addresses topics by id, not name — echo what the wire
    // carries at this version, and resolve the id against the registry.
    let (name, resolved_name) = if version >= 13 {
        (
            None,
            cluster.topic_name_by_id(uuid::Uuid::from_bytes(topic.topic_id)),
        )
    } else {
        (topic.name.clone(), topic.name.clone())
    };
    let partitions = topic
        .partitions
        .iter()
        .map(|p| PartitionSlot {
            index: p.index,
            slot: match (&resolved_name, acks_valid) {
                (_, false) => Slot::Refused(error_codes::INVALID_REQUIRED_ACKS),
                // An id this broker never issued: its own error (100), mapped
                // back through the echoed id.
                (None, true) => Slot::Refused(error_codes::UNKNOWN_TOPIC_ID),
                (Some(topic_name), true) => {
                    one_partition(cluster, pending, topic_name, p.index, p.records)
                }
            },
        })
        .collect();
    TopicOutcome {
        name,
        topic_id: topic.topic_id,
        partitions,
    }
}

/// One partition's verdict: the ingest rule, then a region in the bundle.
fn one_partition(
    cluster: &Cluster,
    pending: &mut Pending,
    topic: &str,
    index: i32,
    records: Option<&[u8]>,
) -> Slot {
    let Ok(partition) = PartitionId::new(index) else {
        return Slot::Refused(error_codes::UNKNOWN_TOPIC_OR_PARTITION);
    };
    let hosted = usize::try_from(index)
        .ok()
        .zip(cluster.partition_count(topic))
        .is_some_and(|(index, count)| index < count);
    if !hosted {
        return Slot::Refused(error_codes::UNKNOWN_TOPIC_OR_PARTITION);
    }
    let Some(records) = records else {
        // A produce with no records is a client bug; nothing to store.
        return Slot::Refused(error_codes::INVALID_RECORD);
    };
    let verified = match verify(records) {
        Ok(verified) => verified,
        Err(code) => return Slot::Refused(code),
    };
    let Ok(topic_id) = TopicId::new(topic.to_owned()) else {
        return Slot::Refused(error_codes::UNKNOWN_TOPIC_OR_PARTITION);
    };
    if !pending.seen.insert((topic.to_owned(), index)) {
        // A partition named twice in one request — see `Pending`.
        return Slot::Refused(error_codes::INVALID_RECORD);
    }
    let nth = pending.bundle.len();
    // ⚠️ The bytes go in **unstamped**: the offset is not known until the
    // commit, which is after this object is written. `Cluster::read` is where
    // the assigned offset is stamped in, and the CRC never covered those
    // twelve bytes (doc 18 §4.4), so neither end recomputes anything.
    let pushed = PushedRecords {
        count: verified.records,
        producer: verified.producer,
    };
    match pending.bundle.push(topic_id, partition, pushed, records) {
        Ok(()) => Slot::Pushed(nth),
        // Only a region this handler could not have built: an empty range, a
        // zero record count, or a topic name past the format's ceiling. None
        // is the client's fault in a way a retry mends.
        Err(_) => Slot::Refused(error_codes::INVALID_RECORD),
    }
}

#[cfg(test)]
mod idempotent;
#[cfg(test)]
pub(crate) mod tests;
