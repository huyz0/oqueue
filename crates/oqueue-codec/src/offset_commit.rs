//! `OffsetCommit` v2-9, hand-rolled (`ADR-0019`).
//!
//! ⚠️ **From v2, not v0.** The dependency's own generated
//! `OffsetCommitRequest` only models v2-9 at all — v0/v1 carried a
//! different shape (`retention_time_ms` absent, `timestamp` present on
//! each partition instead) nothing since Kafka 0.11 sends; advertising a
//! version means serving it (`FR-2`, `matrix.rs`), so the floor is where
//! this crate's own oracle begins. Flexible from v8, confirmed against the
//! dependency's own generated source — this module's usual per-message
//! cutover, not assumed from a shared one.
//!
//! ⚠️ **`group_instance_id` (v7+) and `retention_time_ms` (v2-4) are
//! decoded and discarded, never carried in [`OffsetCommitRequest`]** —
//! `sync_group.rs`'s own precedent for a field this milestone's own scope
//! has no caller for: static membership (`group_instance_id`) is out of
//! scope (`ADR-0033`), and a retention policy on committed offsets is
//! nothing `M4.12`-`M4.14` build. `committed_leader_epoch` (v6+) and
//! `committed_metadata` are discarded the same way, per partition — opaque
//! to a broker that does not yet track leader epochs for committed offsets
//! and has no caller that reads a client's own metadata string back.

use crate::flex::{
    TaggedFields, put_array_len, put_string, put_tagged_fields, read_array_len,
    read_nullable_string, read_string, read_tagged_fields,
};
use crate::wire::{Cursor, DecodeError, put_i16, put_i32};

/// One partition's own offset to commit.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OffsetCommitRequestPartition {
    /// The partition index.
    pub partition_index: i32,
    /// The offset being committed.
    pub committed_offset: i64,
}

/// One topic's own partitions to commit offsets for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OffsetCommitRequestTopic<'a> {
    /// The topic name.
    pub name: &'a str,
    /// Every partition this topic is committing an offset for.
    pub partitions: Vec<OffsetCommitRequestPartition>,
}

/// A group's own request to commit one or more partitions' offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OffsetCommitRequest<'a> {
    /// The group committing.
    pub group_id: &'a str,
    /// The generation this commit belongs to — fenced against the
    /// coordinator's own current one (`M4.11`'s own audited path).
    pub generation_id: i32,
    /// The committing member's own id.
    pub member_id: &'a str,
    /// Every topic this request is committing offsets for.
    pub topics: Vec<OffsetCommitRequestTopic<'a>>,
}

/// Decodes an `OffsetCommit` request.
///
/// # Errors
/// [`DecodeError`] on any malformed length, truncation, or a null
/// non-nullable field.
pub fn decode_request(body: &[u8], version: i16) -> Result<OffsetCommitRequest<'_>, DecodeError> {
    let flexible = version >= 8;
    let mut cur = Cursor::new(body);
    let group_id = read_string(&mut cur, flexible)?;
    let generation_id = cur.read_i32()?;
    let member_id = read_string(&mut cur, flexible)?;
    if version >= 7 {
        // `group_instance_id`: module doc -- static membership is out of
        // this milestone's own scope.
        let _ = read_nullable_string(&mut cur, flexible)?;
    }
    if version <= 4 {
        // `retention_time_ms`: module doc -- no retention policy yet.
        let _ = cur.read_i64()?;
    }

    let topic_count = read_array_len(&mut cur, flexible)?.unwrap_or(0);
    let mut topics = Vec::with_capacity(topic_count);
    for _ in 0..topic_count {
        let name = read_string(&mut cur, flexible)?;
        let partition_count = read_array_len(&mut cur, flexible)?.unwrap_or(0);
        let mut partitions = Vec::with_capacity(partition_count);
        for _ in 0..partition_count {
            let partition_index = cur.read_i32()?;
            let committed_offset = cur.read_i64()?;
            if version >= 6 {
                // `committed_leader_epoch`: module doc -- not yet tracked.
                let _ = cur.read_i32()?;
            }
            // `committed_metadata`: module doc -- no caller reads it back.
            let _ = read_nullable_string(&mut cur, flexible)?;
            if flexible {
                read_tagged_fields(&mut cur)?;
            }
            partitions.push(OffsetCommitRequestPartition {
                partition_index,
                committed_offset,
            });
        }
        if flexible {
            read_tagged_fields(&mut cur)?;
        }
        topics.push(OffsetCommitRequestTopic { name, partitions });
    }

    if flexible {
        read_tagged_fields(&mut cur)?;
    }
    Ok(OffsetCommitRequest {
        group_id,
        generation_id,
        member_id,
        topics,
    })
}

/// One partition's own commit outcome.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OffsetCommitResponsePartition {
    /// The partition index, echoed from the request.
    pub partition_index: i32,
    /// `0` on success, or why this partition's own commit was refused.
    pub error_code: i16,
}

/// One topic's own per-partition outcomes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OffsetCommitResponseTopic<'a> {
    /// The topic name, echoed from the request.
    pub name: &'a str,
    /// Every partition this topic named, in the same order.
    pub partitions: Vec<OffsetCommitResponsePartition>,
}

/// The answer to an `OffsetCommit` request: one outcome per partition named.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OffsetCommitResponse<'a> {
    /// Every topic the request named, echoed with its own outcomes.
    pub topics: Vec<OffsetCommitResponseTopic<'a>>,
}

/// Encodes an `OffsetCommit` response.
pub fn encode_response(out: &mut Vec<u8>, version: i16, resp: &OffsetCommitResponse<'_>) {
    let flexible = version >= 8;
    if version >= 3 {
        // throttle_time_ms: no quota mechanism to charge against yet
        // (sync_group.rs's own precedent) -- zero, honestly.
        put_i32(out, 0);
    }
    put_array_len(out, flexible, Some(resp.topics.len()));
    for topic in &resp.topics {
        put_string(out, flexible, topic.name);
        put_array_len(out, flexible, Some(topic.partitions.len()));
        for partition in &topic.partitions {
            put_i32(out, partition.partition_index);
            put_i16(out, partition.error_code);
            if flexible {
                put_tagged_fields(out, &TaggedFields::default());
            }
        }
        if flexible {
            put_tagged_fields(out, &TaggedFields::default());
        }
    }
    if flexible {
        put_tagged_fields(out, &TaggedFields::default());
    }
}

#[cfg(test)]
mod tests;
