//! `Heartbeat` v0-4, hand-rolled (`ADR-0019`).
//!
//! ⚠️ **Flexible from v4** — confirmed against the dependency's own
//! generated source, `sync_group.rs`'s own precedent: every message in
//! this milestone's own group protocol seems to pick a different cutover.
//!
//! ⚠️ **`group_instance_id` (v3+) is decoded and discarded, never carried
//! in [`HeartbeatRequest`]** — static membership is out of this
//! milestone's scope, `sync_group.rs`'s own precedent for a field this
//! crate consumes off the wire but never keeps.
//!
//! ⚠️ **No `session_timeout_ms` on this wire shape at all.** A member's own
//! session timeout is negotiated once, in its own `JoinGroup` request, and
//! never resent — this module carries no session-timeout tracking of its
//! own; that is `crates/oqueue-broker/src/heartbeat.rs`'s own job.

use crate::flex::{
    TaggedFields, put_tagged_fields, read_nullable_string, read_string, read_tagged_fields,
};
use crate::wire::{Cursor, DecodeError, put_i16, put_i32};

/// One member's own "I'm still here."
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeartbeatRequest<'a> {
    /// The group this member belongs to.
    pub group_id: &'a str,
    /// The generation this member believes it is in.
    pub generation_id: i32,
    /// This member's own id.
    pub member_id: &'a str,
}

/// Decodes a `Heartbeat` request.
///
/// # Errors
/// [`DecodeError`] on any malformed length, truncation, or a null
/// non-nullable field.
pub fn decode_request(body: &[u8], version: i16) -> Result<HeartbeatRequest<'_>, DecodeError> {
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
    if flexible {
        read_tagged_fields(&mut cur)?;
    }
    Ok(HeartbeatRequest {
        group_id,
        generation_id,
        member_id,
    })
}

/// Encodes a `Heartbeat` response: just an error code, or none.
pub fn encode_response(out: &mut Vec<u8>, version: i16, error_code: i16) {
    let flexible = version >= 4;
    if version >= 1 {
        // throttle_time_ms: no quota mechanism to charge against yet
        // (join_group.rs's own precedent) -- zero, honestly.
        put_i32(out, 0);
    }
    put_i16(out, error_code);
    if flexible {
        put_tagged_fields(out, &TaggedFields::default());
    }
}

#[cfg(test)]
mod tests;
