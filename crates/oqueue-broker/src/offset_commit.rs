//! `OffsetCommit` (8), v2-9 — `M4.12`, `M4.14`.
//!
//! ⚠️ **Durable in the same sense `oqueue_coordinator::Coordinator`'s own
//! topic-offset allocation already is — `ADR-0035`.** Every commit is
//! appended to a [`oqueue_core::GroupMetadataLog`] *before* it is
//! acknowledged (`ADR-0020`'s own "the ack IS the durable write" instinct,
//! applied here rather than reinvented): the response this handler builds
//! cannot be constructed before the corresponding
//! [`oqueue_core::GroupMetadataLog::append`] future resolves `Ok`, because
//! nothing in this module has anything to construct one from before then.
//! `FakeGroupMetadataLog` is this milestone's own only implementation, the
//! same status `FakeMetadataLog` already has — a restart survives once
//! `M6`'s real engine exists for either seam, not before; `ADR-0035`'s own
//! "durable in sense, not yet in substrate" is the honest framing this
//! module doc used to overstate as "in-memory only" before this task.
//!
//! ⚠️ **Two fencing decisions, not one — `M9.7`'s seam reused, not a
//! second one invented.** Every commit is checked against `crate::fencing`
//! (`M4.11`'s own audited path: is this member tracked, is the generation
//! current, is the group `Stable`) *before* any topic is looked at, and
//! then each named topic is checked against `crate::authz::topic_authorized`
//! (`M9.7`/`M9.9`'s own decision point, the same one `Produce`/`Fetch`/
//! `ListOffsets`/`Metadata` already route through) — a principal cannot
//! commit an offset for a topic it cannot see, whatever its own group
//! membership. The two are independent: a fencing refusal answers every
//! named topic/partition the same code without ever reaching the
//! per-topic authorization check; a topic-authorization refusal is
//! per-topic, so one unauthorized topic in a batch does not refuse the
//! others.
//!
//! ⚠️ **No topic- or partition-existence check.** Real Kafka can answer
//! `UNKNOWN_TOPIC_OR_PARTITION` for a partition this broker does not
//! serve; this task's own acceptance criterion is principal and
//! generation/member fencing, not topic-catalog validation, so a commit
//! for a topic/partition this broker has never heard of still lands —
//! named rather than silently assumed, `M4.10`'s own "removal, not
//! fencing" scoping discipline applied here to catalog membership instead
//! of group membership.
//!
//! ⚠️ **A durable log failure answers `UNKNOWN_SERVER_ERROR`, per
//! partition** — `join_group.rs`'s own precedent for an internal/storage
//! failure that is not the client's own mistake: the request was
//! well-formed, the fencing and authorization checks passed, and the
//! append itself is what could not be honoured.

#![allow(clippy::redundant_pub_crate)]

use crate::authz::{AuthzContext, topic_authorized};
use crate::cluster::Cluster;
use crate::connection::HandlerResponse;
use crate::fencing::{FencingContext, fence};
use oqueue_codec::apikey::ApiKey;
use oqueue_codec::error_codes;
use oqueue_codec::frame::{RequestPrelude, encode_response_header};
use oqueue_codec::offset_commit::{
    OffsetCommitRequest, OffsetCommitRequestTopic, OffsetCommitResponse,
    OffsetCommitResponsePartition, OffsetCommitResponseTopic, decode_request, encode_response,
};
use oqueue_core::{
    CommitVersion, Error, GroupId, GroupMetadataEntry, GroupMetadataLog, GroupMetadataRecord,
    GroupState, Result, TopicId,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex, PoisonError};

/// How many entries [`CommittedOffsets::replay`] reads per page —
/// `oqueue-index`'s own applier precedent for "read in bounded pages, a
/// short page means done" (`GroupMetadataLog`'s own guarantee 5).
const REPLAY_PAGE_SIZE: usize = 256;

/// How many times [`CommittedOffsets::commit`] retries a version conflict
/// against a racing writer before giving up. ⚠️ Bounded, `security.md`
/// rule 13's instinct: a caller that never stops retrying is an unbounded
/// hold wearing a retry loop.
///
/// ⚠️ **One log, every group** — `Cluster` holds a single
/// `Arc<dyn GroupMetadataLog>`, so a version conflict here is contention
/// across every group committing at once, not only within one key. A busy
/// cluster could in principle exhaust one commit's retry budget on
/// contention it was never itself part of, once `M6`'s real engine adds
/// genuine round-trip latency between attempts — named rather than solved
/// here, since fixing it means either partitioning the log or building
/// backoff, and this task's own acceptance criterion is durability and
/// replay, not contention behaviour under load.
const MAX_COMMIT_RETRIES: u32 = 8;

/// One group's committed position for one `(topic, partition)`, keyed the
/// same way [`CommittedOffsets`] is.
type OffsetKey = (GroupId, TopicId, i32);

/// A committed offset, paired with the [`CommitVersion`] it was durably
/// appended at — [`apply`]'s own guard against the race
/// [`CommittedOffsets`]'s own doc names.
type VersionedOffset = (CommitVersion, i64);

/// Every group's own committed offsets, by `(group, topic, partition)`,
/// each paired with the [`CommitVersion`] it was durably appended at —
/// backed by a [`GroupMetadataLog`].
///
/// ⚠️ **The version is not bookkeeping — it is the guard against a real
/// race.** `commit` appends before it inserts, but two commits to the
/// *same* key racing past their own `.await` (real Kafka clients pipeline,
/// and `connection.rs` gives each request its own task) can resolve their
/// appends in one order and then run their post-append inserts in the
/// other — a plain `insert` with no ordering check would let the causally
/// earlier append overwrite the later one in memory, serving a stale
/// offset until the next commit or a restart-and-replay corrected it. The
/// stored version is what [`apply`] compares against to refuse exactly
/// that: an insert only lands if nothing newer already has.
#[derive(Debug)]
pub(crate) struct CommittedOffsets {
    offsets: Mutex<HashMap<OffsetKey, VersionedOffset>>,
    log: Arc<dyn GroupMetadataLog>,
}

impl CommittedOffsets {
    /// Opens `log` and replays every already-committed offset into memory
    /// before returning — `M4.14`'s own restart-replay guarantee, and
    /// [`Self::new_empty`] plus [`Self::replay`] in one call. ⚠️ **Test-only
    /// since `M4.15a`**: `Cluster::new` now calls the two halves apart
    /// (`replay` in a background task, after returning), so nothing in
    /// production code needs the combined form any more — every existing
    /// test that does not care about the mid-replay window still does. An
    /// empty log opens to an empty map at once.
    ///
    /// # Errors
    ///
    /// Whatever [`Self::replay`] fails with.
    #[cfg(test)]
    pub(crate) async fn open(log: Arc<dyn GroupMetadataLog>) -> Result<Self> {
        let this = Self::new_empty(log);
        this.replay().await?;
        Ok(this)
    }

    /// An empty store over `log`, not yet replayed — no I/O. `Cluster::new`'s
    /// own split half of what `open` used to do in one step (`M4.15a`): a
    /// caller that wants to become queryable *before* replay finishes
    /// (so it can answer `COORDINATOR_LOAD_IN_PROGRESS` for the window
    /// [`Self::replay`] takes, rather than block existing entirely) needs
    /// the two apart.
    pub(crate) fn new_empty(log: Arc<dyn GroupMetadataLog>) -> Self {
        Self {
            offsets: Mutex::new(HashMap::new()),
            log,
        }
    }

    /// Reads `self`'s own log from the start and replaces the in-memory
    /// map with what it finds — `Cluster::new`'s other split half.
    ///
    /// ⚠️ **Not safe to call concurrently with itself or with [`Self::commit`]
    /// on the same instance** — it replaces the whole map rather than
    /// merging into it, so a commit landing mid-replay would be silently
    /// discarded by the replacement. `M4.15a`'s own caller relies on this:
    /// nothing can reach `commit` until `crate::replay_gate::ReplayGate`
    /// reports ready, and this is the only thing that marks it so — see
    /// `cluster.rs`.
    ///
    /// # Errors
    ///
    /// Whatever `log.read_from` fails with — implementation-defined,
    /// `GroupMetadataLog`'s own doc; the fake here never does.
    pub(crate) async fn replay(&self) -> Result<()> {
        let mut offsets = HashMap::new();
        let mut start = CommitVersion::ZERO;
        loop {
            let page = self.log.read_from(start, REPLAY_PAGE_SIZE).await?;
            let got = page.len();
            for entry in &page {
                apply(&mut offsets, entry.version(), entry.record());
            }
            // ⚠️ **A short page means the log has no more** —
            // `GroupMetadataLog`'s own guarantee 5, not "fewer than asked
            // for this time." A full page means there may be more, so the
            // next read starts just past the last entry this one saw.
            let Some(last) = page.last() else {
                break;
            };
            if got < REPLAY_PAGE_SIZE {
                break;
            }
            start = last.version().advance(1)?;
        }
        let mut held = self.lock();
        *held = offsets;
        drop(held);
        Ok(())
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<OffsetKey, VersionedOffset>> {
        self.offsets.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Durably records `offset` as `group`'s own committed position for
    /// `topic`/`partition` — appended to the log *before* the in-memory
    /// map is updated, module doc's own "the ack IS the durable write"
    /// guarantee. A later commit for the same key overwrites the earlier
    /// one, the same "last write wins" behaviour real Kafka's own
    /// `__consumer_offsets` compaction gives it — "later" meaning the
    /// version it durably appended at, not the order its own post-append
    /// insert happened to run in; see [`apply`].
    ///
    /// ⚠️ **No lock held across the `.await`** (`async-concurrency.md`
    /// rule 6) — versions are assigned optimistically (read
    /// `last_version`, try to append there) and a losing race retries
    /// rather than serializing through a held mutex, which rule 8 would
    /// call a sequencing problem wearing a locking primitive. The log's
    /// own [`Error::NonMonotonicCommitVersion`] validation is the
    /// serialization point, not a lock this module holds.
    ///
    /// # Errors
    ///
    /// Whatever `log.append` fails with, or [`Error::Transient`] if
    /// [`MAX_COMMIT_RETRIES`] racing writers are lost to in a row.
    async fn commit(
        &self,
        group: &GroupId,
        topic: &TopicId,
        partition: i32,
        offset: i64,
    ) -> Result<()> {
        let record = GroupMetadataRecord::OffsetCommitted {
            group: group.clone(),
            topic: topic.clone(),
            partition,
            offset,
        };
        for _ in 0..MAX_COMMIT_RETRIES {
            let next = match self.log.last_version().await? {
                Some(last) => last.advance(1)?,
                None => CommitVersion::ZERO,
            };
            let entry = GroupMetadataEntry::new(next, record.clone());
            match self.log.append(std::slice::from_ref(&entry)).await {
                Ok(()) => {
                    let mut offsets = self.lock();
                    apply(&mut offsets, next, &record);
                    drop(offsets);
                    return Ok(());
                }
                Err(Error::NonMonotonicCommitVersion { .. }) => {
                    // A racing writer landed first; retry against the new
                    // last_version rather than fail a well-formed commit.
                }
                Err(other) => return Err(other),
            }
        }
        Err(Error::Transient)
    }

    /// `group`'s own committed offset for `topic`/`partition`, or `None`
    /// if nothing has ever committed one — `M4.13`'s own real caller:
    /// `OffsetFetch`'s explicit-topics form reads this for every partition
    /// a client names.
    pub(crate) fn get(&self, group: &GroupId, topic: &TopicId, partition: i32) -> Option<i64> {
        let offsets = self.lock();
        let value = offsets
            .get(&(group.clone(), topic.clone(), partition))
            .map(|&(_, offset)| offset);
        drop(offsets);
        value
    }

    /// Every topic/partition/offset `group` has ever committed, grouped by
    /// topic and sorted (topic name, then partition index) for a
    /// deterministic answer — `M4.13`'s own real caller: `OffsetFetch`'s
    /// all-topics form (`topics: None` on the wire) reads this instead of
    /// looking up one partition at a time.
    pub(crate) fn topics_for_group(&self, group: &GroupId) -> Vec<(TopicId, Vec<(i32, i64)>)> {
        let offsets = self.lock();
        let mut by_topic: HashMap<TopicId, Vec<(i32, i64)>> = HashMap::new();
        for ((g, topic, partition), (_, offset)) in offsets.iter() {
            if g == group {
                by_topic
                    .entry(topic.clone())
                    .or_default()
                    .push((*partition, *offset));
            }
        }
        drop(offsets);
        let mut result: Vec<_> = by_topic.into_iter().collect();
        result.sort_by(|(a, _), (b, _)| a.as_str().cmp(b.as_str()));
        for (_, partitions) in &mut result {
            partitions.sort_by_key(|&(index, _)| index);
        }
        result
    }
}

/// Folds one already-durable record, appended at `version`, into `offsets`
/// — `CommittedOffsets::replay`'s own step, and [`CommittedOffsets::commit`]'s
/// own post-append step for the entry it just wrote. A `match` with one arm
/// on purpose: [`GroupMetadataRecord`] is exhaustive, and a second variant
/// landing here without a new arm is exactly the silent-replay bug its own
/// doc warns against.
///
/// ⚠️ **Guarded by `version`, not a plain overwrite** — two calls for the
/// same key can arrive out of the order their own versions imply (`commit`'s
/// own doc names the race: two concurrent commits to one key can append in
/// one order and then reach this function in the other, each from its own
/// task, after its own `.await`). An entry only lands if the key holds
/// nothing yet or what it holds is from an earlier version — so whichever
/// call's own version is durably later always wins in memory too, matching
/// what a restart-and-replay would converge to regardless of which call's
/// insert happened to run last.
fn apply(
    offsets: &mut HashMap<OffsetKey, VersionedOffset>,
    version: CommitVersion,
    record: &GroupMetadataRecord,
) {
    match record {
        GroupMetadataRecord::OffsetCommitted {
            group,
            topic,
            partition,
            offset,
        } => {
            let key = (group.clone(), topic.clone(), *partition);
            let stale = matches!(offsets.get(&key), Some(&(existing, _)) if existing >= version);
            if !stale {
                offsets.insert(key, (version, *offset));
            }
        }
        // `M4.15c`'s own event, replayed by `crate::group_transitions`'s
        // own reader of this same log — nothing this map holds. An
        // explicit no-op arm, not a `_ =>`, so a third variant lands here
        // as a compile error rather than a silent absorption.
        GroupMetadataRecord::GroupTransitioned { .. } => {}
    }
}

/// Decodes, fences on group/member/generation, commits every named
/// partition whose own topic this principal may see, and answers — or
/// closes the connection on a malformed body.
pub(crate) async fn handle(
    cluster: &Cluster,
    prelude: RequestPrelude,
    body: &[u8],
    authz: &AuthzContext<'_>,
) -> HandlerResponse {
    let version = prelude.api_version;
    let Ok(request) = decode_request(body, version) else {
        return HandlerResponse::Close;
    };
    let Ok(group) = GroupId::new(request.group_id) else {
        return reply(
            prelude,
            version,
            &top_level_refusal(&request, error_codes::INVALID_REQUEST),
        );
    };

    if let Err(refused) = fence_commit(cluster, &group, request.generation_id, request.member_id) {
        return reply(
            prelude,
            version,
            &top_level_refusal(&request, refused.error_code()),
        );
    }

    let mut topics = Vec::with_capacity(request.topics.len());
    for topic in &request.topics {
        topics.push(commit_topic(cluster, &group, topic, authz).await);
    }
    reply(prelude, version, &OffsetCommitResponse { topics })
}

/// `M4.11`'s own seam: is this member tracked, is the generation current,
/// is the group `Stable` — `heartbeat.rs`'s own acceptable-states choice,
/// reused (a commit is no more valid mid-rebalance than a heartbeat is).
fn fence_commit(
    cluster: &Cluster,
    group: &GroupId,
    generation_id: i32,
    member_id: &str,
) -> std::result::Result<(), crate::fencing::Refusal> {
    let member_tracked = cluster.heartbeats().is_tracked(group, member_id);
    let record = cluster.group_coordinator().record(group);
    let ctx = FencingContext::for_this_node(
        cluster.replay_in_progress(),
        member_tracked,
        record.as_ref(),
        Some(generation_id),
        Some(&[GroupState::Stable]),
    );
    fence(&ctx)
}

/// One topic's own commit: refused wholesale if this principal cannot see
/// it, else every named partition is durably recorded and answered `NONE`
/// — or `UNKNOWN_SERVER_ERROR` if the durable append itself failed.
async fn commit_topic<'a>(
    cluster: &Cluster,
    group: &GroupId,
    topic: &OffsetCommitRequestTopic<'a>,
    authz: &AuthzContext<'_>,
) -> OffsetCommitResponseTopic<'a> {
    if !topic_authorized(topic.name, authz) {
        return refused_topic(topic, error_codes::TOPIC_AUTHORIZATION_FAILED);
    }
    let Ok(topic_id) = TopicId::new(topic.name) else {
        return refused_topic(topic, error_codes::UNKNOWN_TOPIC_OR_PARTITION);
    };
    let mut partitions = Vec::with_capacity(topic.partitions.len());
    for partition in &topic.partitions {
        let error_code = match cluster
            .committed_offsets()
            .commit(
                group,
                &topic_id,
                partition.partition_index,
                partition.committed_offset,
            )
            .await
        {
            Ok(()) => error_codes::NONE,
            Err(_) => error_codes::UNKNOWN_SERVER_ERROR,
        };
        partitions.push(OffsetCommitResponsePartition {
            partition_index: partition.partition_index,
            error_code,
        });
    }
    OffsetCommitResponseTopic {
        name: topic.name,
        partitions,
    }
}

/// Every partition `topic` named, answered the same refusal code.
fn refused_topic<'a>(
    topic: &OffsetCommitRequestTopic<'a>,
    error_code: i16,
) -> OffsetCommitResponseTopic<'a> {
    OffsetCommitResponseTopic {
        name: topic.name,
        partitions: topic
            .partitions
            .iter()
            .map(|partition| OffsetCommitResponsePartition {
                partition_index: partition.partition_index,
                error_code,
            })
            .collect(),
    }
}

/// A refusal reached before any topic was looked at (a malformed
/// `group_id`, or a fencing refusal) — every named topic/partition
/// answers the same code, since `OffsetCommitResponse` carries no
/// top-level error field of its own to answer with instead.
fn top_level_refusal<'a>(
    request: &OffsetCommitRequest<'a>,
    error_code: i16,
) -> OffsetCommitResponse<'a> {
    OffsetCommitResponse {
        topics: request
            .topics
            .iter()
            .map(|topic| refused_topic(topic, error_code))
            .collect(),
    }
}

fn reply(
    prelude: RequestPrelude,
    version: i16,
    response: &OffsetCommitResponse<'_>,
) -> HandlerResponse {
    let mut out = Vec::new();
    if encode_response_header(
        &mut out,
        ApiKey::OffsetCommit,
        version,
        prelude.correlation_id,
    )
    .is_err()
    {
        return HandlerResponse::Close;
    }
    encode_response(&mut out, version, response);
    HandlerResponse::Reply(out)
}

#[cfg(test)]
mod tests;
