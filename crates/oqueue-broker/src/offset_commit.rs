//! `OffsetCommit` (8), v2-9 — `M4.12`.
//!
//! ⚠️ **In-memory only.** Durable storage plus replay on coordinator
//! takeover is `M4.14`'s own separate task — the same split `M4.7`-`M4.11`
//! already established for every other piece of this milestone's own
//! bookkeeping (`heartbeat.rs`'s `Heartbeats`, `join_group::round`'s
//! `GroupJoins`, `sync_group.rs`'s `SyncGroups`), none of which had a
//! durable home before this milestone gave them one either. A restart
//! loses every commit this module holds until `M4.14` lands.
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
use oqueue_core::{GroupId, GroupState, TopicId};
use std::collections::HashMap;
use std::sync::{Mutex, PoisonError};

/// Every group's own committed offsets, by `(group, topic, partition)`.
#[derive(Debug, Default)]
pub(crate) struct CommittedOffsets {
    offsets: Mutex<HashMap<(GroupId, TopicId, i32), i64>>,
}

impl CommittedOffsets {
    fn lock(&self) -> std::sync::MutexGuard<'_, HashMap<(GroupId, TopicId, i32), i64>> {
        self.offsets.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Records `offset` as `group`'s own committed position for
    /// `topic`/`partition` — a later commit for the same key overwrites
    /// the earlier one, the same "last write wins" behaviour real Kafka's
    /// own `__consumer_offsets` compaction gives it.
    fn commit(&self, group: &GroupId, topic: &TopicId, partition: i32, offset: i64) {
        let mut offsets = self.lock();
        offsets.insert((group.clone(), topic.clone(), partition), offset);
        drop(offsets);
    }

    /// `group`'s own committed offset for `topic`/`partition`, or `None`
    /// if nothing has ever committed one — test-only introspection;
    /// `M4.13`'s own `OffsetFetch` is the real caller a later task adds.
    #[cfg(test)]
    pub(crate) fn get(&self, group: &GroupId, topic: &TopicId, partition: i32) -> Option<i64> {
        let offsets = self.lock();
        let value = offsets
            .get(&(group.clone(), topic.clone(), partition))
            .copied();
        drop(offsets);
        value
    }
}

/// Decodes, fences on group/member/generation, commits every named
/// partition whose own topic this principal may see, and answers — or
/// closes the connection on a malformed body.
///
/// ⚠️ **Not `async`** — `heartbeat.rs`/`leave_group.rs`'s own precedent:
/// nothing here parks.
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

    let topics = request
        .topics
        .iter()
        .map(|topic| commit_topic(cluster, &group, topic, authz))
        .collect();
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
) -> Result<(), crate::fencing::Refusal> {
    let member_tracked = cluster.heartbeats().is_tracked(group, member_id);
    let record = cluster.group_coordinator().record(group);
    let ctx = FencingContext::for_this_node(
        member_tracked,
        record.as_ref(),
        Some(generation_id),
        Some(&[GroupState::Stable]),
    );
    fence(&ctx)
}

/// One topic's own commit: refused wholesale if this principal cannot see
/// it, else every named partition is recorded and answered `NONE`.
fn commit_topic<'a>(
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
    let partitions = topic
        .partitions
        .iter()
        .map(|partition| {
            cluster.committed_offsets().commit(
                group,
                &topic_id,
                partition.partition_index,
                partition.committed_offset,
            );
            OffsetCommitResponsePartition {
                partition_index: partition.partition_index,
                error_code: error_codes::NONE,
            }
        })
        .collect();
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
