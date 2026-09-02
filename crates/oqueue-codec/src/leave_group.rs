//! `LeaveGroup` v0-5, hand-rolled (`ADR-0019`).
//!
//! ⚠️ **One function, two frame shapes** — `find_coordinator.rs`'s own
//! precedent applied to a different cutover: below v3 the wire carries
//! exactly one member, in the singular `member_id` field; from v3 it
//! carries a batch, under `members` (real Kafka's own batched form, added
//! ahead of `FindCoordinator`'s KIP-699 batching). Both shapes normalize to
//! the same in-memory type — [`LeaveGroupRequest::members`] is always a
//! `Vec`, of length exactly one below v3.
//!
//! ⚠️ **Flexible from v4** — confirmed against the dependency's own
//! generated source, `heartbeat.rs`'s own precedent for a fourth distinct
//! cutover among this milestone's own five group-protocol messages.
//!
//! ⚠️ **`group_instance_id`/`reason` are decoded and discarded, never
//! carried in [`MemberIdentity`]** — static membership is out of this
//! milestone's scope, `sync_group.rs`'s/`heartbeat.rs`'s own precedent for
//! a field this crate consumes off the wire but never keeps.

use crate::flex::{
    TaggedFields, put_array_len, put_nullable_string, put_string, put_tagged_fields,
    read_array_len, read_nullable_string, read_string, read_tagged_fields,
};
use crate::wire::{Cursor, DecodeError, put_i16, put_i32};

/// One member this request names as leaving.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemberIdentity<'a> {
    /// The departing member's own id.
    pub member_id: &'a str,
}

/// One or more members leaving a group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaveGroupRequest<'a> {
    /// The group being left.
    pub group_id: &'a str,
    /// Every departing member — length exactly one below v3, the wire's
    /// own singular `member_id` field normalized into this shape.
    pub members: Vec<MemberIdentity<'a>>,
}

/// Decodes a `LeaveGroup` request.
///
/// # Errors
/// [`DecodeError`] on any malformed length, truncation, or a null
/// non-nullable field.
pub fn decode_request(body: &[u8], version: i16) -> Result<LeaveGroupRequest<'_>, DecodeError> {
    let flexible = version >= 4;
    let mut cur = Cursor::new(body);
    let group_id = read_string(&mut cur, flexible)?;
    let members = if version <= 2 {
        let member_id = read_string(&mut cur, flexible)?;
        vec![MemberIdentity { member_id }]
    } else {
        let count = read_array_len(&mut cur, flexible)?.unwrap_or(0);
        let mut members = Vec::with_capacity(count);
        for _ in 0..count {
            let member_id = read_string(&mut cur, flexible)?;
            // `group_instance_id`: consumed, never kept (module doc).
            let _ = read_nullable_string(&mut cur, flexible)?;
            if version >= 5 {
                // `reason`: consumed, never kept (module doc).
                let _ = read_nullable_string(&mut cur, flexible)?;
            }
            if flexible {
                read_tagged_fields(&mut cur)?;
            }
            members.push(MemberIdentity { member_id });
        }
        members
    };
    if flexible {
        read_tagged_fields(&mut cur)?;
    }
    Ok(LeaveGroupRequest { group_id, members })
}

/// One member's own answer: `0` on success, or why it could not leave.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MemberLeft<'a> {
    /// The member this answer is for.
    pub member_id: &'a str,
    /// `0` on success, or why this member's own departure failed.
    pub error_code: i16,
}

/// The whole answer: a top-level code (the only signal below v3) and one
/// per-member answer (v3+, in request order).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LeaveGroupResponse<'a> {
    /// `0` on success, or a request-level refusal (a malformed `group_id`,
    /// for instance) that never reached any per-member answer.
    pub error_code: i16,
    /// Every member's own answer — empty below v3, where the top-level
    /// `error_code` alone is the wire's own signal.
    pub members: Vec<MemberLeft<'a>>,
}

/// Encodes a `LeaveGroup` response.
pub fn encode_response(out: &mut Vec<u8>, version: i16, resp: &LeaveGroupResponse<'_>) {
    let flexible = version >= 4;
    if version >= 1 {
        // throttle_time_ms: no quota mechanism to charge against yet
        // (join_group.rs's own precedent) -- zero, honestly.
        put_i32(out, 0);
    }
    put_i16(out, resp.error_code);
    if version >= 3 {
        put_array_len(out, flexible, Some(resp.members.len()));
        for m in &resp.members {
            put_string(out, flexible, m.member_id);
            put_nullable_string(out, flexible, None); // group_instance_id: never tracked.
            put_i16(out, m.error_code);
            if flexible {
                put_tagged_fields(out, &TaggedFields::default());
            }
        }
    }
    if flexible {
        put_tagged_fields(out, &TaggedFields::default());
    }
}

#[cfg(test)]
mod tests;
