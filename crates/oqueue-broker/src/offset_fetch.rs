//! `OffsetFetch` (9), v1-7 — `M4.13`.
//!
//! ⚠️ **No group/member/generation fencing** — deliberately, unlike
//! `OffsetCommit`'s own `crate::fencing` seam (`M4.12`). Real Kafka lets
//! any authenticated client fetch a group's own committed offsets without
//! joining it — that is how offset-lag monitoring tools work — so the only
//! seam this handler routes through is `crate::authz::topic_authorized`
//! (`M9.7`/`M9.9`'s own decision point), per topic.
//!
//! ⚠️ **Real Kafka layers a *second*, group-level check on top of that —
//! `Group:Describe` — and this broker has nothing to check it against.**
//! `oqueue_core::TopicGrants` scopes topics; nothing in this codebase
//! scopes *groups*. The consequence, round-1 review's own finding: a
//! principal holding a grant on topic `"orders"` can fetch **any** group's
//! own committed offset for `"orders"`, including a group it was never a
//! member of and has no relationship to at all — this task's own scope
//! ("`OffsetFetch`... scoped by principal") is topic scoping, the same
//! shape `Metadata`/`Produce`/`Fetch`/`ListOffsets` already have, and does
//! not extend to inventing a second, group-level grants type nothing else
//! in this milestone's own eighteen tasks asks for. Named here rather
//! than left implicit in "any authenticated client may fetch" above,
//! which is true but was read, by the reviewer, as claiming more scoping
//! than this handler actually does. A group-level `GroupGrants` seam,
//! mirroring `TopicGrants`'s own shape, is real, standing, unscheduled
//! work — not tied to a specific next task.
//!
//! ⚠️ **Two shapes, not one standing in for the other** — `M9.9`/`M9.10`'s
//! own `Metadata` precedent, the shape this task's own acceptance
//! criterion names by name: an explicitly-named unauthorized topic answers
//! `TOPIC_AUTHORIZATION_FAILED` per partition; the all-topics form
//! (`topics: None` on the wire) *silently omits* a topic this principal
//! cannot see, rather than naming it as refused — the same "the
//! null-topic-array case is the dangerous one, test it explicitly" finding
//! `Metadata`'s own two forms already established, now proven for a second
//! API rather than assumed to carry over.
//!
//! ⚠️ **A never-committed partition answers `-1`, `NONE`** — not a refusal.
//! `oqueue_coordinator::UNASSIGNED_OFFSET`, the same sentinel
//! `produce/answer.rs` already reuses for "no such partition", real
//! Kafka's own answer for a group that has never committed here.

#![allow(clippy::redundant_pub_crate)]

use crate::authz::{AuthzContext, topic_authorized};
use crate::cluster::Cluster;
use crate::connection::HandlerResponse;
use oqueue_codec::apikey::ApiKey;
use oqueue_codec::error_codes;
use oqueue_codec::frame::{RequestPrelude, encode_response_header};
use oqueue_codec::offset_fetch::{
    OffsetFetchRequestTopic, OffsetFetchResponse, OffsetFetchResponsePartition,
    OffsetFetchResponseTopic, decode_request, encode_response,
};
use oqueue_coordinator::UNASSIGNED_OFFSET;
use oqueue_core::{GroupId, TopicId};

/// Decodes, answers every named (or every ever-committed) topic scoped by
/// this principal's own grants, and replies — or closes the connection on
/// a malformed body.
///
/// ⚠️ **Not `async`** — `heartbeat.rs`/`leave_group.rs`/`offset_commit.rs`'s
/// own precedent: nothing here parks.
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
            &OffsetFetchResponse {
                error_code: error_codes::INVALID_REQUEST,
                topics: Vec::new(),
            },
        );
    };

    // ⚠️ **`M4.15a`'s own gate, checked here rather than through
    // `crate::fencing`** — this handler routes through no other part of
    // that seam (module doc's own "no group/member/generation fencing"),
    // but `cluster.committed_offsets()` below is exactly the map
    // `M4.15a`'s replay window is about: read before replay finishes, it
    // would answer "never committed" for an offset that is actually just
    // not loaded yet, which is precisely the silently-wrong-answer
    // `behavior.md` rule 11 forbids. `OffsetFetchResponse` carries its own
    // top-level `error_code`, unlike `OffsetCommitResponse`, so this
    // refuses the whole request there rather than per topic/partition.
    // ⚠️ **`crate::fencing::Refusal`'s own code, not the raw constant** —
    // `check-fencing-seam.sh` refuses any of the five codes constructed
    // outside `fencing.rs` itself, this handler included even though it
    // routes through no other part of that seam.
    if cluster.replay_in_progress() {
        return reply(
            prelude,
            version,
            &OffsetFetchResponse {
                error_code: crate::fencing::Refusal::CoordinatorLoadInProgress.error_code(),
                topics: Vec::new(),
            },
        );
    }

    // ⚠️ **Bound here, not inside `all_topics`.** `CommittedOffsets` hands
    // back owned `TopicId`s, not borrows into any longer-lived storage —
    // `committed` is what the all-topics response's own
    // `OffsetFetchResponseTopic<'_>` borrows its names from, so it must
    // outlive the `reply` call below (a helper that built and returned
    // owned `String`s of its own would either leak them or copy every
    // topic name a second time for no reason). Computed unconditionally,
    // even for the explicit-topics form that never reads it: one more
    // uncontended, in-memory scan is cheaper than threading an `Option`
    // through the match below for it.
    let committed = cluster.committed_offsets().topics_for_group(&group);
    let topics = request.topics.as_ref().map_or_else(
        || all_topics(&committed, authz),
        |explicit| {
            explicit
                .iter()
                .map(|topic| explicit_topic(cluster, &group, topic, authz))
                .collect()
        },
    );
    reply(
        prelude,
        version,
        &OffsetFetchResponse {
            error_code: error_codes::NONE,
            topics,
        },
    )
}

/// One explicitly-named topic's own answer: every partition named,
/// refused wholesale (`TOPIC_AUTHORIZATION_FAILED`) if this principal
/// cannot see it, else each partition's own committed offset (or `-1` if
/// this group never committed one there).
fn explicit_topic<'a>(
    cluster: &Cluster,
    group: &GroupId,
    topic: &OffsetFetchRequestTopic<'a>,
    authz: &AuthzContext<'_>,
) -> OffsetFetchResponseTopic<'a> {
    if !topic_authorized(topic.name, authz) {
        return refused_topic(topic, error_codes::TOPIC_AUTHORIZATION_FAILED);
    }
    // ⚠️ **Matches `offset_commit.rs`'s own `commit_topic` precedent** —
    // an unconstructible name (the empty string; `TopicId::new`'s only
    // rejection) answers `UNKNOWN_TOPIC_OR_PARTITION`, not a silent `-1`.
    // Built once, not once per partition: `topic.name` cannot change
    // between iterations, so a per-partition rebuild was pure waste as
    // well as the thing that let this fall through to `-1` unnoticed.
    let Ok(topic_id) = TopicId::new(topic.name) else {
        return refused_topic(topic, error_codes::UNKNOWN_TOPIC_OR_PARTITION);
    };
    let partitions = topic
        .partition_indexes
        .iter()
        .map(|&partition_index| {
            let committed_offset = cluster
                .committed_offsets()
                .get(group, &topic_id, partition_index)
                .unwrap_or(UNASSIGNED_OFFSET);
            OffsetFetchResponsePartition {
                partition_index,
                committed_offset,
                error_code: error_codes::NONE,
            }
        })
        .collect();
    OffsetFetchResponseTopic {
        name: topic.name,
        partitions,
    }
}

/// Every partition `topic` named, answered the same refusal code —
/// `offset_commit.rs`'s own `refused_topic` shape.
fn refused_topic<'a>(
    topic: &OffsetFetchRequestTopic<'a>,
    error_code: i16,
) -> OffsetFetchResponseTopic<'a> {
    let partitions = topic
        .partition_indexes
        .iter()
        .map(|&partition_index| OffsetFetchResponsePartition {
            partition_index,
            committed_offset: UNASSIGNED_OFFSET,
            error_code,
        })
        .collect();
    OffsetFetchResponseTopic {
        name: topic.name,
        partitions,
    }
}

/// Every topic in `committed` (`handle`'s own already-fetched
/// `CommittedOffsets::topics_for_group` result — see its own binding
/// there for why this takes the borrow rather than fetching it itself),
/// silently omitting one this principal cannot see — module doc's own
/// "silent omission, not a refusal" shape for the null-array form.
fn all_topics<'a>(
    committed: &'a [(TopicId, Vec<(i32, i64)>)],
    authz: &AuthzContext<'_>,
) -> Vec<OffsetFetchResponseTopic<'a>> {
    committed
        .iter()
        .filter(|(topic, _)| topic_authorized(topic.as_str(), authz))
        .map(|(topic, partitions)| OffsetFetchResponseTopic {
            name: topic.as_str(),
            partitions: partitions
                .iter()
                .map(
                    |&(partition_index, committed_offset)| OffsetFetchResponsePartition {
                        partition_index,
                        committed_offset,
                        error_code: error_codes::NONE,
                    },
                )
                .collect(),
        })
        .collect()
}

fn reply(
    prelude: RequestPrelude,
    version: i16,
    response: &OffsetFetchResponse<'_>,
) -> HandlerResponse {
    let mut out = Vec::new();
    if encode_response_header(
        &mut out,
        ApiKey::OffsetFetch,
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
