//! The pure decision logic behind a `put` — whether it needs multipart, and
//! what to send either way — kept apart from the network calls that carry it
//! out. See `s3.rs`'s own `put` for those, and ADR-0013 for why multipart
//! completion here is unconditional only.

// Same reasoning `classify.rs` already gives.
#![allow(clippy::redundant_pub_crate)]

use object_store::{PutMode, PutOptions, UpdateVersion};
use oqueue_core::{Error, MultipartLimits, MultipartSession, Precondition, Result};
use std::ops::Range;

/// S3's real multipart bounds (doc 04 §5): 5 MiB minimum part size except the
/// last, 5 GiB maximum part size — not coincidentally the same limit a single
/// non-multipart `PutObject` may ever carry, which is why this is also the
/// threshold `put_strategy_for` switches on — 10,000 maximum parts, 5 TiB
/// maximum object size.
pub(crate) const S3_MULTIPART_LIMITS: MultipartLimits = MultipartLimits {
    min_part_size: 5 * 1024 * 1024,
    max_part_size: 5 * 1024 * 1024 * 1024,
    max_parts: 10_000,
    max_object_size: 5 * 1024 * 1024 * 1024 * 1024,
};

/// The `PutOptions` a `put_opts` call sends for `precondition`.
///
/// ⚠️ Pulled out for the same reason `get.rs`'s helpers are: the field it
/// assembles is only observable against a live backend, which
/// `s3_minio.rs`'s `conditional_write_*` cases already exercise — this
/// function exists so the assembly itself is also reachable by a T0 unit
/// test, independent of that network round trip.
pub(crate) fn put_options_for(precondition: Option<&Precondition>) -> PutOptions {
    let mode = match precondition {
        None => PutMode::Overwrite,
        Some(Precondition::IfAbsent) => PutMode::Create,
        Some(Precondition::IfMatches(token)) => PutMode::Update(UpdateVersion {
            e_tag: Some(token.as_str().to_owned()),
            version: None,
        }),
    };
    PutOptions {
        mode,
        ..PutOptions::default()
    }
}

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
/// `limits.max_part_size` — this backend cannot condition a multipart
/// completion (ADR-0013), and retrying the same oversized, conditioned
/// payload will never change that. Otherwise, whatever
/// [`MultipartSession::add_part`]/[`MultipartSession::finish`] reject as a
/// bound violation — unreachable in practice at `S3_MULTIPART_LIMITS`' real
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
    // Same justification as `tls.rs`'s: the one `expect` below is on a value
    // this module just constructed from a literal it controls.
    #![allow(clippy::expect_used)]

    use super::{PutStrategy, S3_MULTIPART_LIMITS, put_options_for, put_strategy_for};
    use crate::s3::S3Store;
    use crate::tls::install_ring_provider;
    use object_store::aws::AmazonS3Builder;
    use object_store::{PutMode, UpdateVersion};
    use oqueue_core::{Error, MultipartLimits, Precondition, PreconditionToken};

    /// ⚠️ **Written as plain decimal literals, deliberately** — not by
    /// re-evaluating the same multiplication `S3_MULTIPART_LIMITS` itself
    /// uses. A test that recomputed `5 * 1024 * 1024` on both sides would
    /// pass under a mutated `*` too, since the mutation would apply to both
    /// expressions identically wherever cargo-mutants finds them — the
    /// point of this test is to pin the real numbers doc 04 §5 documents
    /// against arithmetic that can drift independently of them. Found the
    /// hard way: the constant this test now pins actually read 25 TiB
    /// (`5 * 1024⁴ * 5`, an errant trailing `* 5`) until writing this test's
    /// literal caught it — a bound five times looser than S3's own limit,
    /// silently accepted by every other test in this file because none of
    /// them exercise a payload anywhere near `max_object_size`.
    #[test]
    fn s3_multipart_limits_match_the_real_s3_numbers() {
        assert_eq!(
            S3_MULTIPART_LIMITS,
            MultipartLimits {
                min_part_size: 5_242_880,     // 5 MiB
                max_part_size: 5_368_709_120, // 5 GiB
                max_parts: 10_000,
                max_object_size: 5_497_558_138_880, // 5 TiB
            }
        );
    }

    #[test]
    fn put_options_for_no_precondition_overwrites() {
        assert_eq!(put_options_for(None).mode, PutMode::Overwrite);
    }

    #[test]
    fn put_options_for_if_absent_creates() {
        assert_eq!(
            put_options_for(Some(&Precondition::IfAbsent)).mode,
            PutMode::Create
        );
    }

    #[test]
    fn put_options_for_if_matches_updates_with_the_tokens_etag() {
        let precondition = Precondition::IfMatches(PreconditionToken::new("etag-123"));
        assert_eq!(
            put_options_for(Some(&precondition)).mode,
            PutMode::Update(UpdateVersion {
                e_tag: Some("etag-123".to_owned()),
                version: None,
            })
        );
    }

    /// Small enough that a test can build a payload for it in memory without
    /// approaching real S3's numbers — `put_strategy_for` is exercised
    /// against these, never against `S3_MULTIPART_LIMITS` itself, which is
    /// `s3_minio.rs`'s job against a real endpoint.
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
            "this backend cannot condition a multipart completion (ADR-0013)"
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

    /// `AmazonS3Builder::build` does no I/O — it only validates local
    /// config — so a store built from nothing but a bucket name is enough
    /// to exercise this without touching the network or a literal
    /// credential (`security.md`: never a literal, even a fake one, in a
    /// test fixture). Same helper `s3.rs`'s own test module defines, for the
    /// same reason — kept separate rather than shared across a module
    /// boundary that would otherwise need to exist only for this.
    fn store_with_no_credentials() -> S3Store {
        install_ring_provider();
        let inner = AmazonS3Builder::new()
            .with_bucket_name("unused-in-this-test")
            .build()
            .expect("building against a bucket name alone needs no network");
        S3Store {
            inner,
            multipart_limits: S3_MULTIPART_LIMITS,
        }
    }

    #[test]
    fn with_multipart_limits_overrides_the_default() {
        let default = store_with_no_credentials();
        assert_eq!(default.strategy_for(11), Ok(PutStrategy::Single));

        let overridden = store_with_no_credentials().with_multipart_limits(tiny_limits());
        assert_eq!(
            overridden.strategy_for(11),
            Ok(PutStrategy::Multipart(vec![0..10, 10..11]))
        );
    }
}
