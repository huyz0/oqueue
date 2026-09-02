//! `JoinGroup` v0-9, hand-rolled (`ADR-0019`).
//!
//! ⚠️ **Flexible from v6, not the usual v2/v3 cutover.** Every other codec
//! in this crate goes compact at v2 or v3; `JoinGroup`'s own schema does not
//! — confirmed against the dependency's generated source, not assumed by
//! pattern-matching this crate's other modules.
//!
//! ⚠️ **Subscription metadata is opaque bytes, never parsed** — `M4.5`'s own
//! acceptance criterion, `oqueue_codec::sasl_authenticate`'s own instinct
//! for a mechanism-defined blob this crate cannot interpret applied to an
//! assignor-defined one instead. `JoinGroupRequestProtocol::metadata` and
//! `JoinGroupResponseMember::metadata` carry whatever bytes the client's own
//! assignor produced; only `M4.6`'s leader election reads `name`, and only
//! the elected leader's own client-side code ever reads `metadata` back.
//!
//! ⚠️ **Wire types only — no handler logic lives here.** `rebalance_timeout_ms`
//! absent below v1, `group_instance_id` absent below v5, and `reason` absent
//! below v8 all decode to their absent-field default (`0`/`None`) rather
//! than a synthesized value; whether `M4.7`'s handler treats an absent
//! `rebalance_timeout_ms` as `session_timeout_ms` is that task's own
//! decision, not this module's to make silently.

use crate::flex::{
    TaggedFields, put_array_len, put_bytes, put_nullable_string, put_string, put_tagged_fields,
    read_array_len, read_bytes, read_nullable_string, read_string, read_tagged_fields,
};
use crate::wire::{Cursor, DecodeError, put_bool, put_i16, put_i32};

/// One protocol a member advertises support for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JoinGroupRequestProtocol<'a> {
    /// The protocol name — `M4.6`'s own leader election reads this.
    pub name: &'a str,
    /// Opaque, assignor-defined subscription metadata.
    pub metadata: &'a [u8],
}

/// A member's own request to join (or rejoin) a group.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinGroupRequest<'a> {
    /// The group to join.
    pub group_id: &'a str,
    /// The coordinator considers this member dead past this timeout with no
    /// heartbeat.
    pub session_timeout_ms: i32,
    /// How long the coordinator waits for every expected member to
    /// (re)join — absent below v1, decoded as `0`.
    pub rebalance_timeout_ms: i32,
    /// Empty on a first join; a previously-minted id on rejoin.
    pub member_id: &'a str,
    /// A static membership identity (`KIP-345`) — absent below v5.
    pub group_instance_id: Option<&'a str>,
    /// The protocol family this member speaks, e.g. `"consumer"`.
    pub protocol_type: &'a str,
    /// Every protocol this member supports, each with its own opaque
    /// subscription metadata.
    pub protocols: Vec<JoinGroupRequestProtocol<'a>>,
    /// Why this member is (re)joining — absent below v8.
    pub reason: Option<&'a str>,
}

/// Decodes a `JoinGroup` request.
///
/// # Errors
/// [`DecodeError`] on any malformed length, truncation, or a null
/// non-nullable field.
pub fn decode_request(body: &[u8], version: i16) -> Result<JoinGroupRequest<'_>, DecodeError> {
    let flexible = version >= 6;
    let mut cur = Cursor::new(body);
    let group_id = read_string(&mut cur, flexible)?;
    let session_timeout_ms = cur.read_i32()?;
    let rebalance_timeout_ms = if version >= 1 { cur.read_i32()? } else { 0 };
    let member_id = read_string(&mut cur, flexible)?;
    let group_instance_id = if version >= 5 {
        read_nullable_string(&mut cur, flexible)?
    } else {
        None
    };
    let protocol_type = read_string(&mut cur, flexible)?;

    let protocol_count = read_array_len(&mut cur, flexible)?.unwrap_or(0);
    let mut protocols = Vec::with_capacity(protocol_count);
    for _ in 0..protocol_count {
        let name = read_string(&mut cur, flexible)?;
        let metadata = read_bytes(&mut cur, flexible)?;
        if flexible {
            read_tagged_fields(&mut cur)?;
        }
        protocols.push(JoinGroupRequestProtocol { name, metadata });
    }

    let reason = if version >= 8 {
        read_nullable_string(&mut cur, flexible)?
    } else {
        None
    };
    if flexible {
        read_tagged_fields(&mut cur)?;
    }
    Ok(JoinGroupRequest {
        group_id,
        session_timeout_ms,
        rebalance_timeout_ms,
        member_id,
        group_instance_id,
        protocol_type,
        protocols,
        reason,
    })
}

/// One member's own entry in the leader's own view of the group — present
/// only in the response the elected leader receives.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct JoinGroupResponseMember<'a> {
    /// The member's own id.
    pub member_id: &'a str,
    /// A static membership identity (`KIP-345`) — absent below v5.
    pub group_instance_id: Option<&'a str>,
    /// The member's own opaque subscription metadata, for the negotiated
    /// protocol.
    pub metadata: &'a [u8],
}

/// The whole answer: the negotiated generation and protocol, who leads, and
/// — for the leader alone — every member's own metadata.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JoinGroupResponse<'a> {
    /// `0` on success, or why joining failed.
    pub error_code: i16,
    /// The generation this member joined.
    pub generation_id: i32,
    /// The negotiated protocol family — absent below v7.
    pub protocol_type: Option<&'a str>,
    /// The one protocol name every member supports — `None` when
    /// `error_code` is set.
    pub protocol_name: Option<&'a str>,
    /// The elected leader's own member id.
    pub leader: &'a str,
    /// `true` if the leader must skip its own client-side assignment —
    /// absent below v9; this broker never sets it, since it never computes
    /// an assignment itself (the classic protocol's own leader-computes
    /// shape, `M4.8`).
    pub skip_assignment: bool,
    /// This member's own (possibly freshly minted) id.
    pub member_id: &'a str,
    /// Every member's own metadata — populated for the leader alone,
    /// empty for every follower (`M4.7`'s own handler decision).
    pub members: Vec<JoinGroupResponseMember<'a>>,
}

/// Encodes a `JoinGroup` response.
pub fn encode_response(out: &mut Vec<u8>, version: i16, resp: &JoinGroupResponse<'_>) {
    let flexible = version >= 6;
    if version >= 2 {
        // throttle_time_ms: no quota mechanism to charge against yet
        // (M9.16's own quota is per-principal in-flight, not a throttle
        // duration) -- zero, honestly, rather than a number nothing
        // computes.
        put_i32(out, 0);
    }
    put_i16(out, resp.error_code);
    put_i32(out, resp.generation_id);
    if version >= 7 {
        put_nullable_string(out, flexible, resp.protocol_type);
    }
    put_nullable_string(out, flexible, resp.protocol_name);
    put_string(out, flexible, resp.leader);
    if version >= 9 {
        put_bool(out, resp.skip_assignment);
    }
    put_string(out, flexible, resp.member_id);
    put_array_len(out, flexible, Some(resp.members.len()));
    for m in &resp.members {
        put_string(out, flexible, m.member_id);
        if version >= 5 {
            put_nullable_string(out, flexible, m.group_instance_id);
        }
        put_bytes(out, flexible, m.metadata);
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
