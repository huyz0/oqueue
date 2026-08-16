//! Whether a `put` needs multipart, and how to slice a payload into parts —
//! kept apart from the network calls that carry it out and from any
//! backend's own numbers. See `s3.rs`/`gcs.rs`'s own `put_multipart` for the
//! network calls, ADR-0013 for why completion is unconditional only, and
//! each backend's own `put.rs` for the real `MultipartLimits` it passes in.
//!
//! ⚠️ **Shared across every backend**, same reasoning as `classify.rs`/
//! `get.rs`: this is parameterized entirely by [`MultipartLimits`], which
//! each backend already constructs its own instance of (`M1.9`'s own doc
//! comment). Nothing here names S3 or GCS.

// Same reasoning `classify.rs` already gives.
#![allow(clippy::redundant_pub_crate)]

use oqueue_core::{Error, MultipartLimits, MultipartSession, Precondition, Result};
use std::ops::Range;

/// How `put` uploads a payload of a given size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum PutStrategy {
    /// A single `put_opts` call, same as `M1.15`.
    Single,
    /// A multipart upload, unconditional only — see this module's doc
    /// comment and ADR-0013 for why. Each range is a slice into the
    /// caller's payload, in upload order.
    Multipart(Vec<Range<usize>>),
}

/// Decides how `put` should upload a payload of `payload_len` bytes, and — if
/// multipart is required — how to slice it into parts.
///
/// ⚠️ **Pulled out on its own**, same reasoning as this module's other pure
/// helpers: this is the one part of a multipart `put` that involves no
/// network call, so it is what a T0 test can actually reach.
///
/// # Errors
///
/// [`Error::Permanent`] if `precondition` is `Some` and `payload_len` exceeds
/// `limits.max_part_size` — no backend built here can condition a multipart
/// completion (ADR-0013), and retrying the same oversized, conditioned
/// payload will never change that. Otherwise, whatever
/// [`MultipartSession::add_part`]/[`MultipartSession::finish`] reject as a
/// bound violation — unreachable in practice at either backend's real
/// numbers for any payload that fits in this process' memory, but a real
/// possibility for a caller that passes a narrower `MultipartLimits`.
pub(crate) fn put_strategy_for(
    payload_len: usize,
    precondition: Option<&Precondition>,
    limits: MultipartLimits,
) -> Result<PutStrategy> {
    let len = u64::try_from(payload_len).unwrap_or(u64::MAX);
    if len <= limits.max_part_size {
        return Ok(PutStrategy::Single);
    }
    if precondition.is_some() {
        return Err(Error::Permanent);
    }
    let part_size = usize::try_from(limits.max_part_size)
        .unwrap_or(usize::MAX)
        .max(1);
    let mut session = MultipartSession::new(limits);
    let mut chunks = Vec::new();
    let mut offset = 0usize;
    while offset < payload_len {
        let end = (offset + part_size).min(payload_len);
        // `end - offset` never exceeds `usize::MAX`'s `u64` range on the
        // 64-bit hosts NFR-40 targets — same reasoning `store.rs`'s
        // `slice_range` gives for its own `unwrap_or(u64::MAX)`.
        session.add_part(u64::try_from(end - offset).unwrap_or(u64::MAX))?;
        chunks.push(offset..end);
        offset = end;
    }
    session.finish()?;
    Ok(PutStrategy::Multipart(chunks))
}

#[cfg(test)]
mod tests {
    use super::{PutStrategy, put_strategy_for};
    use oqueue_core::{Error, MultipartLimits, Precondition};

    /// Small enough that a test can build a payload for it in memory without
    /// approaching either backend's real numbers. Each backend's own
    /// `put.rs` test module defines its own copy of this same shape, rather
    /// than fighting module privacy to share five lines across a test-only
    /// boundary.
    fn tiny_limits() -> MultipartLimits {
        MultipartLimits {
            min_part_size: 5,
            max_part_size: 10,
            max_parts: 4,
            max_object_size: 35,
        }
    }

    #[test]
    fn a_payload_at_or_under_the_part_size_is_single() {
        assert_eq!(
            put_strategy_for(10, None, tiny_limits()),
            Ok(PutStrategy::Single)
        );
        assert_eq!(
            put_strategy_for(0, Some(&Precondition::IfAbsent), tiny_limits()),
            Ok(PutStrategy::Single),
            "a precondition is fine on the single-shot path"
        );
    }

    #[test]
    fn a_payload_over_the_part_size_with_a_precondition_is_permanent() {
        assert_eq!(
            put_strategy_for(11, Some(&Precondition::IfAbsent), tiny_limits()),
            Err(Error::Permanent),
            "no backend built here can condition a multipart completion (ADR-0013)"
        );
    }

    #[test]
    fn a_payload_over_the_part_size_splits_into_max_sized_chunks() {
        // 22 bytes at a 10-byte part size: two full parts, one remainder --
        // the remainder is the only one allowed under `min_part_size` (5),
        // since it is the last.
        assert_eq!(
            put_strategy_for(22, None, tiny_limits()),
            Ok(PutStrategy::Multipart(vec![0..10, 10..20, 20..22]))
        );
    }

    #[test]
    fn too_many_parts_is_rejected_before_any_request() {
        // A generous `max_object_size` isolates this from the object-size
        // check below -- `tiny_limits`' own 35-byte cap would trip on the
        // object-size check first, at fewer parts than `max_parts` allows.
        let limits = MultipartLimits {
            max_object_size: 1000,
            ..tiny_limits()
        };
        // 31 bytes at a 10-byte part size needs 4 parts, exactly at
        // `max_parts`; one byte more needs a 5th and must fail.
        assert_eq!(
            put_strategy_for(31, None, limits),
            Ok(PutStrategy::Multipart(vec![0..10, 10..20, 20..30, 30..31]))
        );
        assert_eq!(
            put_strategy_for(41, None, limits),
            Err(Error::TooManyParts { max: 4 })
        );
    }

    #[test]
    fn a_total_over_the_max_object_size_is_rejected_before_any_request() {
        // 36 bytes exceeds `tiny_limits`' 35-byte `max_object_size`, on the
        // very last of its 4 parts (10*3 + 6 = 36 > 35).
        assert_eq!(
            put_strategy_for(36, None, tiny_limits()),
            Err(Error::ObjectTooLarge { max: 35 })
        );
    }

    #[test]
    fn a_small_non_last_part_is_rejected_before_any_request() {
        // A `min_part_size` of 5 with a `max_part_size` of 10 cannot itself
        // produce a too-small non-last part by construction (every non-last
        // chunk is exactly `max_part_size`) -- this proves `finish`'s check
        // still runs, using limits where it can actually fire: a
        // `max_part_size` under `min_part_size` forces every non-last part
        // to violate it.
        let limits = MultipartLimits {
            min_part_size: 8,
            max_part_size: 4,
            max_parts: 10,
            max_object_size: 100,
        };
        assert_eq!(
            put_strategy_for(9, None, limits),
            Err(Error::PartTooSmall { bytes: 4, min: 8 })
        );
    }
}
