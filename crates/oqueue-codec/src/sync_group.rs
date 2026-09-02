//! `SyncGroup` v0-5, hand-rolled (`ADR-0019`).
//!
//! ⚠️ **Flexible from v4** — confirmed against the dependency's own
//! generated source, not assumed from this crate's own usual v2/v3 or
//! `join_group`'s own v6 cutover; every message here seems to pick its own.
//!
//! ⚠️ **`group_instance_id` (v3+) is decoded and discarded, never carried
//! in [`SyncGroupRequest`]** — static membership (`KIP-345`) is out of
//! scope for this milestone (`ADR-0033`'s own "classic protocol only, no
//! static membership" doesn't name it, but nothing in `M4`'s eighteen
//! tasks builds a caller for it either); still consumed off the wire so the
//! cursor lands correctly for the fields after it.
//!
//! ⚠️ **The assignment is opaque bytes, never parsed** — `join_group.rs`'s
//! own "subscription metadata is opaque" contract applied to the other
//! half of the classic protocol: this broker relays what the leader
//! computed, and never runs an assignor of its own (`M4.8`'s own
//! acceptance criterion, `ADR-0033`).

use crate::flex::{
    TaggedFields, put_bytes, put_nullable_string, put_tagged_fields, read_array_len, read_bytes,
    read_nullable_string, read_string, read_tagged_fields,
};
use crate::wire::{Cursor, DecodeError, put_i16, put_i32};

/// One member's own assignment, as the leader submits it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncGroupRequestAssignment<'a> {
    /// The member this assignment is for.
    pub member_id: &'a str,
    /// Opaque, assignor-defined assignment bytes.
    pub assignment: &'a [u8],
}

/// One member's own submission: the leader's own full assignment, or a
/// follower's own empty one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncGroupRequest<'a> {
    /// The group being synced.
    pub group_id: &'a str,
    /// The generation this submission belongs to — fenced against the
    /// coordinator's own current one (`M4.11`'s own audited path, not this
    /// module's to check).
    pub generation_id: i32,
    /// This member's own id.
    pub member_id: &'a str,
    /// The negotiated protocol family — absent below v5.
    pub protocol_type: Option<&'a str>,
    /// The negotiated protocol name — absent below v5.
    pub protocol_name: Option<&'a str>,
    /// Every member's own assignment — populated only by the leader's own
    /// submission; empty on every follower's.
    pub assignments: Vec<SyncGroupRequestAssignment<'a>>,
}

/// Decodes a `SyncGroup` request.
///
/// # Errors
/// [`DecodeError`] on any malformed length, truncation, or a null
/// non-nullable field.
pub fn decode_request(body: &[u8], version: i16) -> Result<SyncGroupRequest<'_>, DecodeError> {
    let flexible = version >= 4;
    let mut cur = Cursor::new(body);
    let group_id = read_string(&mut cur, flexible)?;
    let generation_id = cur.read_i32()?;
    let member_id = read_string(&mut cur, flexible)?;
    if version >= 3 {
        // `group_instance_id`: consumed off the wire, never kept (module
        // doc — static membership is out of this milestone's scope).
        let _ = read_nullable_string(&mut cur, flexible)?;
    }
    let protocol_type = if version >= 5 {
        read_nullable_string(&mut cur, flexible)?
    } else {
        None
    };
    let protocol_name = if version >= 5 {
        read_nullable_string(&mut cur, flexible)?
    } else {
        None
    };

    let count = read_array_len(&mut cur, flexible)?.unwrap_or(0);
    let mut assignments = Vec::with_capacity(count);
    for _ in 0..count {
        let member_id = read_string(&mut cur, flexible)?;
        let assignment = read_bytes(&mut cur, flexible)?;
        if flexible {
            read_tagged_fields(&mut cur)?;
        }
        assignments.push(SyncGroupRequestAssignment {
            member_id,
            assignment,
        });
    }

    if flexible {
        read_tagged_fields(&mut cur)?;
    }
    Ok(SyncGroupRequest {
        group_id,
        generation_id,
        member_id,
        protocol_type,
        protocol_name,
        assignments,
    })
}

/// This member's own answer: its slice of the leader's submitted
/// assignment, or a refusal.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyncGroupResponse<'a> {
    /// `0` on success, or why this member's own sync failed.
    pub error_code: i16,
    /// The negotiated protocol family — absent below v5.
    pub protocol_type: Option<&'a str>,
    /// The negotiated protocol name — absent below v5.
    pub protocol_name: Option<&'a str>,
    /// This member's own slice of the leader's submitted assignment —
    /// empty on a refusal.
    pub assignment: &'a [u8],
}

/// Encodes a `SyncGroup` response.
pub fn encode_response(out: &mut Vec<u8>, version: i16, resp: &SyncGroupResponse<'_>) {
    let flexible = version >= 4;
    if version >= 1 {
        // throttle_time_ms: no quota mechanism to charge against yet
        // (join_group.rs's own precedent) -- zero, honestly.
        put_i32(out, 0);
    }
    put_i16(out, resp.error_code);
    if version >= 5 {
        put_nullable_string(out, flexible, resp.protocol_type);
        put_nullable_string(out, flexible, resp.protocol_name);
    }
    put_bytes(out, flexible, resp.assignment);
    if flexible {
        put_tagged_fields(out, &TaggedFields::default());
    }
}

#[cfg(test)]
mod tests;
