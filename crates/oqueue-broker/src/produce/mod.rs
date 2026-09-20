//! `Produce` v3-13 against the real write path: verify, bundle, PUT, commit.
//!
//! ⚠️ **One request is one object per key domain** — FR-32 and FR-42. Every
//! partition that passes the ingest rule goes into the [`BundleBuilder`] for
//! its topic's domain, so a default-only produce spanning N topics still costs
//! one PUT while a mixed default/BYOK produce costs one PUT per domain. Doc 12
//! prices a PUT far above the bytes in it; that ratio is the cost model, and
//! this handler is where it is spent.
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

use crate::authz::{AuthzContext, topic_authorized};
use crate::cluster::{Cluster, FlushError, RegionSealer};
use crate::connection::HandlerResponse;
use crate::ingest::verify;
use crate::session::Session;
use answer::{PartitionSlot, PushOutcome, Slot, answer};
use oqueue_codec::apikey::ApiKey;
use oqueue_codec::error_codes;
use oqueue_codec::frame::{RequestPrelude, encode_response_header};
use oqueue_codec::metadata::TopicIdentity;
use oqueue_codec::produce::{ProduceResponse, ProduceResponseTopic, decode_request};
use oqueue_core::{BundleBuilder, KeyDomain, PartitionId, PushedRecords, TopicId};
use std::collections::HashMap;
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
    bundles: Vec<BundleBuilder>,
    by_domain: HashMap<KeyDomain, usize>,
    routes: Vec<Vec<usize>>,
    pushed: usize,
    seen: HashSet<(String, i32)>,
}

struct PendingRecord<'a> {
    topic: TopicId,
    partition: PartitionId,
    pushed: PushedRecords,
    records: &'a [u8],
}

#[derive(Debug)]
enum PendingPushError {
    InvalidRecord,
    EncryptionUnavailable,
}

fn failed_outcomes(outcomes: Vec<Option<PushOutcome>>, code: i16) -> Vec<PushOutcome> {
    outcomes
        .into_iter()
        .map(|outcome| outcome.unwrap_or(PushOutcome::Failed(code)))
        .collect()
}

const fn failure_code(error: &FlushError) -> i16 {
    match error {
        FlushError::Store(_) => error_codes::NOT_ENOUGH_REPLICAS,
        FlushError::Commit(_) => error_codes::LEADER_NOT_AVAILABLE,
    }
}

impl Pending {
    fn new() -> Self {
        Self {
            bundles: Vec::new(),
            by_domain: HashMap::new(),
            routes: Vec::new(),
            pushed: 0,
            seen: HashSet::new(),
        }
    }

    const fn is_empty(&self) -> bool {
        self.pushed == 0
    }

    async fn push(
        &mut self,
        sealer: &dyn RegionSealer,
        domain: KeyDomain,
        record: PendingRecord<'_>,
    ) -> Result<usize, PendingPushError> {
        let PendingRecord {
            topic,
            partition,
            pushed,
            records,
        } = record;
        let sealed_region = if domain.requires_sealing() {
            Some(
                sealer
                    .seal(&domain, &topic, partition, records)
                    .await
                    .map_err(|_| PendingPushError::EncryptionUnavailable)?,
            )
        } else {
            None
        };
        let bundle_index = if let Some(index) = self.by_domain.get(&domain).copied() {
            index
        } else {
            let index = self.bundles.len();
            self.by_domain.insert(domain, index);
            self.bundles.push(BundleBuilder::new());
            self.routes.push(Vec::new());
            index
        };
        let local = self.bundles[bundle_index].len();
        if let Some(sealed) = sealed_region {
            self.bundles[bundle_index]
                .push_sealed(topic, partition, pushed, sealed.as_region())
                .map_err(|_| PendingPushError::EncryptionUnavailable)?;
        } else {
            self.bundles[bundle_index]
                .push(topic, partition, pushed, records)
                .map_err(|_| PendingPushError::InvalidRecord)?;
        }
        let global = self.pushed;
        self.pushed += 1;
        self.routes[bundle_index].push(global);
        debug_assert_eq!(local, self.routes[bundle_index].len() - 1);
        Ok(global)
    }
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
async fn flush(
    cluster: &Cluster,
    session: &Session,
    pending: Pending,
) -> Result<Vec<PushOutcome>, FlushError> {
    if pending.is_empty() {
        return Ok(Vec::new());
    }
    let Pending {
        bundles,
        routes,
        pushed,
        ..
    } = pending;
    let mut outcomes: Vec<Option<PushOutcome>> = vec![None; pushed];
    for (bundle_index, bundle) in bundles.into_iter().enumerate() {
        let ack = match cluster.flush(bundle).await {
            Ok(ack) => ack,
            Err(error) => return Ok(failed_outcomes(outcomes, failure_code(&error))),
        };
        if ack
            .outcomes()
            .iter()
            .any(|outcome| matches!(outcome, oqueue_coordinator::SpanOutcome::Assigned(_)))
        {
            session.observe(ack.watermark());
        }
        let route = routes
            .get(bundle_index)
            .ok_or(FlushError::Store(oqueue_core::Error::IndexObjectMismatch))?;
        for (local, outcome) in ack.outcomes().iter().enumerate() {
            let global = route
                .get(local)
                .copied()
                .ok_or(FlushError::Store(oqueue_core::Error::IndexObjectMismatch))?;
            outcomes[global] = Some(match outcome {
                oqueue_coordinator::SpanOutcome::Assigned(assignment) => {
                    PushOutcome::Assigned(assignment.base_offset().get())
                }
                oqueue_coordinator::SpanOutcome::Rejected(reason) => {
                    PushOutcome::from_rejection(*reason)
                }
            });
        }
    }
    outcomes
        .into_iter()
        .map(|outcome| outcome.ok_or(FlushError::Store(oqueue_core::Error::IndexObjectMismatch)))
        .collect()
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
    authz: &AuthzContext<'_>,
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

    let mut pending = Pending::new();
    let mode = TopicMode {
        version,
        acks_valid,
    };
    let mut outcomes: Vec<TopicOutcome> = Vec::with_capacity(request.topics.len());
    for topic in &request.topics {
        outcomes.push(one_topic(cluster, &mut pending, topic, &mode, authz).await);
    }

    let flushed = flush(cluster, session, pending).await;

    if request.acks == 0 {
        return HandlerResponse::Silent;
    }
    let response = ProduceResponse {
        topics: build_response_topics(&outcomes, version, flushed.as_ref()),
    };

    let mut out = Vec::new();
    if encode_response_header(&mut out, ApiKey::Produce, version, prelude.correlation_id).is_err() {
        return HandlerResponse::Close;
    }
    oqueue_codec::produce::encode_response(&mut out, version, &response);
    HandlerResponse::Reply(out)
}

/// Every topic's own response entry — split out of `handle` purely to keep
/// that function under the fifty-line limit once `M9.12` added `authz`
/// threading beside everything already there.
///
/// ⚠️ **`filter_map`, not `expect`** (`M3.41`, `region.rs`'s own
/// precedent). `unanswerable` (`handle`'s own guard) already refused any
/// request whose topic name was `None` at a version that needs one, so a
/// `Name(None)` here cannot happen — and a panic one refactor away from
/// being reachable is what `error-handling.md` forbids even for a
/// proven-safe path. Named as unreachable rather than asserted: dropped
/// from the reply, not a crash of the connection carrying every other
/// topic's answer.
fn build_response_topics<'a>(
    outcomes: &'a [TopicOutcome],
    version: i16,
    flushed: Result<&'a Vec<PushOutcome>, &'a FlushError>,
) -> Vec<ProduceResponseTopic<'a>> {
    outcomes
        .iter()
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
                    .map(|p| answer(p, flushed))
                    .collect(),
            })
        })
        .collect()
}

/// The per-request scalars `one_topic` needs beyond `topic` itself, bundled
/// to stay under `rust-style.md`'s argument-count limit once `M9.12` added
/// `authz` beside them.
struct TopicMode {
    version: i16,
    acks_valid: bool,
}

/// One topic's outcome: resolve its addressing, then every partition.
async fn one_topic(
    cluster: &Cluster,
    pending: &mut Pending,
    topic: &oqueue_codec::produce::ProduceTopic<'_>,
    mode: &TopicMode,
    authz: &AuthzContext<'_>,
) -> TopicOutcome {
    // From v13 the wire addresses topics by id, not name — echo what the wire
    // carries at this version, and resolve the id against the registry.
    let (name, resolved_name) = if mode.version >= 13 {
        (
            None,
            cluster
                .topic_name_by_id(uuid::Uuid::from_bytes(topic.topic_id))
                .await,
        )
    } else {
        (topic.name.clone(), topic.name.clone())
    };
    let mut partitions = Vec::with_capacity(topic.partitions.len());
    for p in &topic.partitions {
        let slot = match admitted(resolved_name.as_deref(), mode, authz) {
            Ok(topic_name) => one_partition(cluster, pending, topic_name, p.index, p.records).await,
            Err(code) => Slot::Refused(code),
        };
        partitions.push(PartitionSlot {
            index: p.index,
            slot,
        });
    }
    TopicOutcome {
        name,
        topic_id: topic.topic_id,
        partitions,
    }
}

/// The name a partition may be written under, or the code refusing it.
fn admitted<'n>(
    resolved_name: Option<&'n str>,
    mode: &TopicMode,
    authz: &AuthzContext<'_>,
) -> Result<&'n str, i16> {
    match (resolved_name, mode.acks_valid) {
        (_, false) => Err(error_codes::INVALID_REQUIRED_ACKS),
        // An id this broker never issued: its own error (100), mapped
        // back through the echoed id.
        (None, true) => Err(error_codes::UNKNOWN_TOPIC_ID),
        // ⚠️ **`M9.12`: checked before `one_partition` runs at all** —
        // the same ordering `M9.9` used for `Metadata`, so an
        // unauthorized principal cannot cause a side effect (here,
        // none exists on this path either way, but the ordering is
        // what keeps that true if one is ever added).
        //
        // ⚠️ **Below v13 this checks the client's raw name, never
        // `Cluster::partition_count`** — round 1 review's own
        // finding, named rather than "fixed" into matching v13+:
        // `TopicGrants` is keyed by name, so a name-addressed
        // request can be authorized (or refused) without ever
        // resolving existence first, `Metadata`'s own explicit-name
        // case (`M9.9`) doing exactly this for every version it
        // serves. From v13 the wire carries only an id, and an id
        // cannot be checked against a name-keyed index *before*
        // resolving it to a name — existence-resolution is not a
        // choice there, it is what makes a name to check exist at
        // all. The two paths' shapes genuinely differ by
        // addressing mode, not by an oversight in one of them.
        (Some(topic_name), true) if !topic_authorized(topic_name, authz) => {
            Err(error_codes::TOPIC_AUTHORIZATION_FAILED)
        }
        (Some(topic_name), true) => Ok(topic_name),
    }
}

/// One partition's verdict: the ingest rule, then a region in the bundle.
async fn one_partition(
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
        .zip(cluster.partition_count(topic).await)
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
    // ⚠️ The bytes go in **unstamped**: the offset is not known until the
    // commit, which is after this object is written. `Cluster::read` is where
    // the assigned offset is stamped in, and the CRC never covered those
    // twelve bytes (doc 18 §4.4), so neither end recomputes anything.
    let pushed = PushedRecords {
        count: verified.records,
        producer: verified.producer,
    };
    let Some(domain) = cluster.topic_key_domain(&topic_id).await else {
        return Slot::Refused(error_codes::UNKNOWN_TOPIC_OR_PARTITION);
    };
    append_partition(
        cluster,
        pending,
        domain,
        PendingRecord {
            topic: topic_id,
            partition,
            pushed,
            records,
        },
    )
    .await
}

async fn append_partition(
    cluster: &Cluster,
    pending: &mut Pending,
    domain: KeyDomain,
    record: PendingRecord<'_>,
) -> Slot {
    pending
        .push(cluster.region_sealer(), domain, record)
        .await
        .map_or_else(
            |error| {
                Slot::Refused(match error {
                    PendingPushError::InvalidRecord => error_codes::INVALID_RECORD,
                    PendingPushError::EncryptionUnavailable => error_codes::INVALID_REQUEST,
                })
            },
            Slot::Pushed,
        )
}

#[cfg(test)]
mod domain_tests;
#[cfg(test)]
mod idempotent;
#[cfg(test)]
pub(crate) mod tests;
