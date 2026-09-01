//! `Fetch` v4-17 request decode and response encode, hand-rolled
//! (`ADR-0019`).
//!
//! ⚠️ **This is the message whose owning by a dependency caused the `DoS`
//! `M2.26` found.** `kafka-protocol`'s generated array decoder allocated a
//! `Vec` from an untrusted element count with no bound against the input —
//! a 64-byte Fetch v16 frame demanded ~30 GB. Every array and string here
//! goes through [`crate::flex`]'s bound-before-allocate combinators instead,
//! so that shape of defect cannot exist in this decoder by construction.
//!
//! ⚠️ **Fetch goes flexible at v12, not v9** — the one message whose cutover
//! differs from Produce/Metadata/ApiVersions (`versions.rs`'s
//! `flexible_from`). Topics address by name through v12 and by id from v13;
//! `session_id`/`session_epoch` (v7+) and `forgotten_topics_data` (v7+, an
//! incremental-fetch-session feature this broker does not implement) are
//! decoded past and discarded — sessions are always declined, so nothing
//! downstream reads them.

use crate::flex::{
    TaggedFields, put_array_len, put_string, put_tagged_fields, read_array_len,
    read_nullable_string, read_tagged_fields,
};
use crate::metadata::{TopicId, TopicIdentity, read_topic_id};
use crate::wire::{Cursor, DecodeError, put_i16, put_i32, put_i64};

/// The fields of a `Fetch` request this broker acts on.
///
/// The isolation level and the per-topic, per-partition offsets requested.
/// ⚠️ ~~`max_wait_ms` and `min_bytes` are decoded past but not kept~~ — **kept
/// since `M3.20`**, the commit that gave the broker something to wait *on*.
/// They were skipped while nothing could park, which is a different thing from
/// their not being on the wire.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchRequest<'a> {
    /// How long the broker may hold this request waiting for records, in
    /// milliseconds. ⚠️ **The client's number, and the broker's whole timer.**
    /// `M3.20` wires it to a park on the coordinator's index; a broker that
    /// substituted a poll interval of its own would turn one wakeup into a
    /// choice between latency and wasted work.
    pub max_wait_ms: i32,
    /// How many bytes must accumulate before the broker answers early. A fetch
    /// holds until this is met or `max_wait_ms` expires, whichever is first.
    pub min_bytes: i32,
    /// The most bytes the whole response may carry (v3+; absent below).
    ///
    /// ⚠️ **Per *request*, and that is the point of keeping it** (`M3.22`). A
    /// `Fetch` may name many partitions, so a per-partition bound multiplied
    /// by a client-chosen count is not a bound at all — one frame becomes
    /// partitions × budget bytes of object-storage reads, all concatenated in
    /// memory before the response is framed.
    pub max_bytes: i32,
    /// `0` = read uncommitted, `1` = read committed.
    pub isolation_level: i8,
    /// The topics fetched from.
    pub topics: Vec<FetchTopic<'a>>,
}

/// One topic in a `Fetch` request: by name through v12, by id from v13.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FetchTopic<'a> {
    /// The topic name, or `None` (v13+ addresses by id).
    pub name: Option<&'a str>,
    /// The topic id, nil below v13.
    pub topic_id: TopicId,
    /// The partitions fetched from.
    pub partitions: Vec<FetchPartition>,
}

/// One partition in a `Fetch` request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FetchPartition {
    /// The partition index.
    pub index: i32,
    /// The offset to fetch from.
    pub fetch_offset: i64,
    /// The most bytes this partition may contribute to the response.
    ///
    /// ⚠️ **A per-partition share of the request's own bound, not a second
    /// bound.** Kafka's own broker treats it that way, and a reader that
    /// honoured only this one would be back to partitions × budget.
    pub partition_max_bytes: i32,
}

/// Decodes a `Fetch` request.
///
/// # Errors
/// [`DecodeError`] on any malformed length or truncation — every count and
/// length is bounded against the input before anything is grown, the
/// discipline that dissolves the class of `DoS` `M2.26` found.
pub fn decode_request(body: &[u8], version: i16) -> Result<FetchRequest<'_>, DecodeError> {
    let flexible = version >= 12;
    let mut cur = Cursor::new(body);

    // `replica_id` sits on the wire only through v14; from v15 the client
    // is always a real consumer (`-1`) and the field is dropped, replaced
    // by a per-partition `replica_state` this broker does not read.
    if version <= 14 {
        let _replica_id = cur.read_i32()?;
    }
    let max_wait_ms = cur.read_i32()?;
    let min_bytes = cur.read_i32()?;
    // ⚠️ **Unconditional, and the version gate is in `ADVERTISED` instead.**
    // The field arrives at v3 and this broker's Fetch floor is v4, so there is
    // no version reaching here without it — a `version >= 3` guard would be a
    // branch nothing can take, which is worse than no guard: it reads as a
    // case that has been handled.
    let max_bytes = cur.read_i32()?;
    let isolation_level = cur.read_i8()?;
    if version >= 7 {
        let _session_id = cur.read_i32()?;
        let _session_epoch = cur.read_i32()?;
    }

    let topic_count = read_array_len(&mut cur, flexible)?.unwrap_or(0);
    let mut topics = Vec::new();
    for _ in 0..topic_count {
        topics.push(read_fetch_topic(&mut cur, version, flexible)?);
    }

    // forgotten_topics_data (v7+): an incremental-fetch-session list this
    // broker never populates a session against, decoded past to reach
    // rack_id and the top-level tagged fields correctly.
    if version >= 7 {
        skip_forgotten_topics(&mut cur, version, flexible)?;
    }
    if version >= 11 {
        let _rack_id = read_nullable_string(&mut cur, flexible)?;
    }
    if flexible {
        let _: TaggedFields = read_tagged_fields(&mut cur)?;
    }

    Ok(FetchRequest {
        max_wait_ms,
        min_bytes,
        max_bytes,
        isolation_level,
        topics,
    })
}

/// Reads one `FetchTopic` entry: its addressing, then every partition.
fn read_fetch_topic<'a>(
    cur: &mut Cursor<'a>,
    version: i16,
    flexible: bool,
) -> Result<FetchTopic<'a>, DecodeError> {
    let name = if version <= 12 {
        read_nullable_string(cur, flexible)?
    } else {
        None
    };
    let topic_id = if version >= 13 {
        read_topic_id(cur)?
    } else {
        [0u8; 16]
    };
    let partition_count = read_array_len(cur, flexible)?.unwrap_or(0);
    let mut partitions = Vec::new();
    for _ in 0..partition_count {
        let index = cur.read_i32()?;
        if version >= 9 {
            let _current_leader_epoch = cur.read_i32()?;
        }
        let fetch_offset = cur.read_i64()?;
        if version >= 12 {
            let _last_fetched_epoch = cur.read_i32()?;
        }
        if version >= 5 {
            let _log_start_offset = cur.read_i64()?;
        }
        let partition_max_bytes = cur.read_i32()?;
        if flexible {
            let _: TaggedFields = read_tagged_fields(cur)?;
        }
        partitions.push(FetchPartition {
            index,
            fetch_offset,
            partition_max_bytes,
        });
    }
    if flexible {
        let _: TaggedFields = read_tagged_fields(cur)?;
    }
    Ok(FetchTopic {
        name,
        topic_id,
        partitions,
    })
}

/// Skips one `forgotten_topics_data` entry list (v7+): each entry names a
/// topic (by name through v12, by id from v13) and a plain `i32` partition
/// index array — a fetch-session feature this broker never establishes a
/// session for, so nothing is kept.
fn skip_forgotten_topics(
    cur: &mut Cursor<'_>,
    version: i16,
    flexible: bool,
) -> Result<(), DecodeError> {
    let count = read_array_len(cur, flexible)?.unwrap_or(0);
    for _ in 0..count {
        if version <= 12 {
            let _ = read_nullable_string(cur, flexible)?;
        }
        if version >= 13 {
            let _ = read_topic_id(cur)?;
        }
        let partition_count = read_array_len(cur, flexible)?.unwrap_or(0);
        for _ in 0..partition_count {
            let _ = cur.read_i32()?;
        }
        if flexible {
            let _: TaggedFields = read_tagged_fields(cur)?;
        }
    }
    Ok(())
}

/// The data a `Fetch` response carries: one entry per requested topic and
/// partition, with the outcome the handler resolved.
#[derive(Debug, Clone)]
pub struct FetchResponse<'a> {
    /// One entry per topic, in request order.
    pub topics: Vec<FetchResponseTopic<'a>>,
}

/// One topic in a `Fetch` response.
#[derive(Debug, Clone)]
pub struct FetchResponseTopic<'a> {
    /// The name through v12, the id from v13 — never both, never neither.
    pub identity: TopicIdentity<'a>,
    /// The per-partition outcomes.
    pub partitions: Vec<FetchResponsePartition<'a>>,
}

/// One partition's outcome in a `Fetch` response.
#[derive(Debug, Clone, Copy)]
pub struct FetchResponsePartition<'a> {
    /// The partition index.
    pub index: i32,
    /// The error code (0 = ok).
    pub error_code: i16,
    /// The high watermark.
    pub high_watermark: i64,
    /// The stable offset (no transactions, so this equals the watermark on
    /// success; the caller's call on a refusal — real Kafka brokers leave
    /// it at the protocol's `-1` unset value there, which this type does
    /// not default for the caller).
    pub last_stable_offset: i64,
    /// The log's earliest retained offset, likewise the caller's call.
    pub log_start_offset: i64,
    /// The record batches, concatenated verbatim, or `None`.
    pub records: Option<&'a [u8]>,
}

/// Appends a `Fetch` response body at `version`. The response header is
/// [`crate::frame::encode_response_header`]'s job.
pub fn encode_response(out: &mut Vec<u8>, version: i16, resp: &FetchResponse<'_>) {
    let flexible = version >= 12;
    let empty = TaggedFields::default();

    put_i32(out, 0); // throttle_time_ms
    if version >= 7 {
        put_i16(out, 0); // error_code (top-level; sessions declined, never an error)
        put_i32(out, 0); // session_id: sessions declined
    }

    put_array_len(out, flexible, Some(resp.topics.len()));
    for topic in &resp.topics {
        // ⚠️ **The variant is the only thing decided here** (`M3.41`) — see
        // `produce.rs`'s identical match for the reasoning.
        match &topic.identity {
            TopicIdentity::Name(name) => {
                debug_assert!(version <= 12, "a name identity above v13, which has none");
                put_string(out, flexible, name);
            }
            TopicIdentity::Id(id) => {
                debug_assert!(
                    version >= 13,
                    "an id identity below v13, which has no id field"
                );
                out.extend_from_slice(id);
            }
        }

        put_array_len(out, flexible, Some(topic.partitions.len()));
        for p in &topic.partitions {
            put_i32(out, p.index);
            put_i16(out, p.error_code);
            put_i64(out, p.high_watermark);
            put_i64(out, p.last_stable_offset);
            if version >= 5 {
                put_i64(out, p.log_start_offset);
            }
            put_array_len(out, flexible, Some(0)); // aborted_transactions (empty)
            if version >= 11 {
                put_i32(out, -1); // preferred_read_replica: none
            }
            put_nullable_bytes(out, flexible, p.records);
            if flexible {
                put_tagged_fields(out, &empty);
            }
        }

        if flexible {
            put_tagged_fields(out, &empty);
        }
    }

    if flexible {
        // Top-level tagged fields: `node_endpoints` (v16+, KIP-951) rides
        // here, but only when non-empty — a single-node broker sends none.
        put_tagged_fields(out, &empty);
    }
}

/// Appends nullable bytes: compact when `flexible`, else `i32`-length legacy
/// (`-1` null).
fn put_nullable_bytes(out: &mut Vec<u8>, flexible: bool, value: Option<&[u8]>) {
    if flexible {
        crate::flex::put_compact_nullable_bytes(out, value);
    } else {
        match value {
            None => put_i32(out, -1),
            Some(bytes) => {
                put_i32(out, i32::try_from(bytes.len()).unwrap_or(i32::MAX));
                out.extend_from_slice(bytes);
            }
        }
    }
}

#[cfg(test)]
mod tests;
