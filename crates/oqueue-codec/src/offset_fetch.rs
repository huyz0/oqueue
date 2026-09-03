//! `OffsetFetch` v1-7, hand-rolled (`ADR-0019`).
//!
//! ⚠️ **From v1, not v0.** The dependency's own generated `OffsetFetchRequest`
//! only models v1-9 at all — v0 answered a different response shape (no
//! per-partition `error_code`) nothing since Kafka 0.10.2 sends. Flexible
//! from v6, confirmed against the dependency's own generated source.
//!
//! ⚠️ **v8-9 (KIP-709, batched *groups*) is not advertised — a materially
//! different feature, not this module's own cutover to widen.** Every
//! other batched form this crate has built (`FindCoordinator`'s v4 keys,
//! `LeaveGroup`'s v3 members) batches multiple items of the *same kind*
//! within one group's own request; v8-9 batches multiple *groups*, each
//! its own fencing/authorization context, in one request. `M4.13`'s own
//! scope is one group's own committed offsets, the same "one group per
//! request" shape every other handler in this milestone has; a true
//! multi-group `OffsetFetch` is a bigger feature than a version bump,
//! named here rather than half-built.
//!
//! ⚠️ **`require_stable` (v7) is decoded and discarded** — this broker has
//! no notion of an "unstable" (still-committing) offset to hold back;
//! nothing here ever answers one differently for it.
//!
//! ⚠️ **`topics: None` means "every topic this group has ever committed
//! an offset for"** — the wire's own null-array sentinel, decoded here as
//! `Option<Vec<_>>` rather than folded into an empty `Vec`, because the two
//! mean opposite things and the caller (`crate::offset_fetch`'s own
//! handler) must be able to tell them apart. `Some(vec![])` — an explicit,
//! empty topic list — is a real, if pointless, "nothing" request.

use crate::flex::{
    TaggedFields, put_array_len, put_nullable_string, put_string, put_tagged_fields,
    read_array_len, read_string, read_tagged_fields,
};
use crate::wire::{Cursor, DecodeError, put_i16, put_i32, put_i64};

/// One topic's own partitions to fetch committed offsets for.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OffsetFetchRequestTopic<'a> {
    /// The topic name.
    pub name: &'a str,
    /// The partitions this topic is asking about.
    pub partition_indexes: Vec<i32>,
}

/// A group's own request to fetch committed offsets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OffsetFetchRequest<'a> {
    /// The group being asked about.
    pub group_id: &'a str,
    /// The topics to fetch, or `None` for every topic this group has ever
    /// committed an offset for — module doc's own "null means all" note.
    pub topics: Option<Vec<OffsetFetchRequestTopic<'a>>>,
}

/// Decodes an `OffsetFetch` request.
///
/// # Errors
/// [`DecodeError`] on any malformed length, truncation, or a null
/// non-nullable field.
pub fn decode_request(body: &[u8], version: i16) -> Result<OffsetFetchRequest<'_>, DecodeError> {
    let flexible = version >= 6;
    let mut cur = Cursor::new(body);
    let group_id = read_string(&mut cur, flexible)?;

    let topics = match read_array_len(&mut cur, flexible)? {
        None => None,
        Some(count) => {
            let mut topics = Vec::with_capacity(count);
            for _ in 0..count {
                let name = read_string(&mut cur, flexible)?;
                let partition_count = read_array_len(&mut cur, flexible)?.unwrap_or(0);
                let mut partition_indexes = Vec::with_capacity(partition_count);
                for _ in 0..partition_count {
                    partition_indexes.push(cur.read_i32()?);
                }
                if flexible {
                    read_tagged_fields(&mut cur)?;
                }
                topics.push(OffsetFetchRequestTopic {
                    name,
                    partition_indexes,
                });
            }
            Some(topics)
        }
    };

    if version >= 7 {
        // `require_stable`: module doc -- discarded.
        let _ = cur.read_bool()?;
    }
    if flexible {
        read_tagged_fields(&mut cur)?;
    }
    Ok(OffsetFetchRequest { group_id, topics })
}

/// One partition's own committed offset, or why it has none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OffsetFetchResponsePartition {
    /// The partition index, echoed from the request (or enumerated, for
    /// the all-topics form).
    pub partition_index: i32,
    /// The committed offset, or `-1` if this group has never committed
    /// one for this partition — real Kafka's own sentinel.
    pub committed_offset: i64,
    /// `0` on success, or why this partition's own answer was refused.
    pub error_code: i16,
}

/// One topic's own per-partition answers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OffsetFetchResponseTopic<'a> {
    /// The topic name.
    pub name: &'a str,
    /// Every partition answered for this topic.
    pub partitions: Vec<OffsetFetchResponsePartition>,
}

/// The answer to an `OffsetFetch` request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OffsetFetchResponse<'a> {
    /// `0` on success, or a group-level refusal (a malformed `group_id`) —
    /// `error_codes::INVALID_REQUEST`'s own case, not a per-topic one.
    pub error_code: i16,
    /// Every topic answered.
    pub topics: Vec<OffsetFetchResponseTopic<'a>>,
}

/// Encodes an `OffsetFetch` response.
pub fn encode_response(out: &mut Vec<u8>, version: i16, resp: &OffsetFetchResponse<'_>) {
    let flexible = version >= 6;
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
            put_i64(out, partition.committed_offset);
            if version >= 5 {
                // committed_leader_epoch: not tracked, module doc.
                put_i32(out, -1);
            }
            // metadata: not tracked, module doc -- always null.
            put_nullable_string(out, flexible, None);
            put_i16(out, partition.error_code);
            if flexible {
                put_tagged_fields(out, &TaggedFields::default());
            }
        }
        if flexible {
            put_tagged_fields(out, &TaggedFields::default());
        }
    }
    if version >= 2 {
        put_i16(out, resp.error_code);
    }
    if flexible {
        put_tagged_fields(out, &TaggedFields::default());
    }
}

#[cfg(test)]
mod tests;
