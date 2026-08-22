//! S3's own numbers and precondition encoding for `put` — see
//! `crate::multipart` for the backend-agnostic size/precondition decision
//! this feeds, and `s3.rs`'s own `put`/`put_multipart` for the network calls
//! that carry it out. ADR-0013 is why multipart completion is unconditional
//! only.

// Same reasoning `classify.rs` already gives.
#![allow(clippy::redundant_pub_crate)]

use object_store::{PutMode, PutOptions, UpdateVersion};
use oqueue_core::{MultipartLimits, Precondition};

/// S3's real multipart bounds (doc 04 §5): 5 MiB minimum part size except the
/// last, 5 GiB maximum part size — not coincidentally the same limit a single
/// non-multipart `PutObject` may ever carry, which is why this is also the
/// threshold `crate::multipart::put_strategy_for` switches on — 10,000
/// maximum parts, 5 TiB maximum object size.
pub(crate) const S3_MULTIPART_LIMITS: MultipartLimits = MultipartLimits {
    min_part_size: 5 * 1024 * 1024,
    max_part_size: 5 * 1024 * 1024 * 1024,
    max_parts: 10_000,
    max_object_size: 5 * 1024 * 1024 * 1024 * 1024,
    // A single `PutObject` carries at most 5 GiB (ADR-0013's Decision
    // records the cap; doc 04 §5 states it as the max *part* size, the same
    // number by S3's own design) -- on S3 the
    // single-request ceiling and the multipart switchover genuinely
    // coincide, which is what let `M1.53`'s divergence hide (`M2.3`).
    max_single_put: 5 * 1024 * 1024 * 1024,
};

/// The `PutOptions` a `put_opts` call sends for `precondition`.
///
/// ⚠️ **S3-specific**, unlike its siblings in `crate::classify`/`crate::get`/
/// `crate::multipart`: a conditional `Update` carries the caller's token as
/// `UpdateVersion::e_tag`, not `::version` — `object_store`'s S3 client
/// reads `e_tag` for this (`aws/mod.rs`'s `put_opts`); GCS's own `put.rs`
/// reads `version` instead (`gcp/client.rs`), so the two cannot share this
/// one function despite sharing everything else.
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

#[cfg(test)]
mod tests {
    // Same justification as `tls.rs`'s: the one `expect` below is on a value
    // this module just constructed from a literal it controls.
    #![allow(clippy::expect_used)]

    use super::{S3_MULTIPART_LIMITS, put_options_for};
    use crate::multipart::PutStrategy;
    use crate::s3::S3Store;
    use crate::tls::install_ring_provider;
    use object_store::aws::AmazonS3Builder;
    use object_store::{PutMode, UpdateVersion};
    use oqueue_core::{MultipartLimits, Precondition, PreconditionToken};

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
                max_single_put: 5_368_709_120, // 5 GiB
                min_part_size: 5_242_880,      // 5 MiB
                max_part_size: 5_368_709_120,  // 5 GiB
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
    /// approaching real S3's numbers.
    fn tiny_limits() -> MultipartLimits {
        MultipartLimits {
            max_single_put: 20,
            min_part_size: 5,
            max_part_size: 10,
            max_parts: 4,
            max_object_size: 35,
        }
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
