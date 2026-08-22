//! GCS's own numbers and precondition encoding for `put` — see
//! `crate::multipart` for the backend-agnostic size/precondition decision
//! this feeds, and `gcs.rs`'s own `put`/`put_multipart` for the network
//! calls that carry it out. ADR-0013 (written against S3, confirmed to apply
//! here too — see this module's own `put_options_for` doc comment) is why
//! multipart completion is unconditional only.

// Same reasoning `classify.rs` already gives.
#![allow(clippy::redundant_pub_crate)]

use object_store::{PutMode, PutOptions, UpdateVersion};
use oqueue_core::{MultipartLimits, Precondition};

/// GCS's own multipart bounds (doc 04 §5, plus GCP's own published quota for
/// the one figure doc 04 does not state).
///
/// ⚠️ **A genuinely different shape than S3's, not just different numbers.**
/// GCS resumable-upload chunks must be a *multiple* of 256 KiB, not merely
/// under some maximum — a constraint [`MultipartLimits`]'s fields do not
/// encode directly (`M1.9`'s own doc comment already flagged this gap:
/// "GCS resumable upload bounds a chunk to a multiple of 256 KiB with no
/// part-count analogue at all"). `crate::multipart::put_strategy_for` slices
/// every non-last part at exactly `max_part_size`, so choosing
/// `max_part_size` itself as a multiple of 256 KiB (8 MiB here — this
/// project's own choice of chunk size, not a GCS-imposed ceiling, since none
/// is documented) is what actually satisfies the real constraint; nothing
/// enforces "multiple of 256 KiB" as a first-class check the way
/// `min_part_size`/`max_parts`/`max_object_size` are. And doc 04 §5 states
/// GCS has **no part-count analogue at all** — `max_parts` is set to
/// `u32::MAX` rather than a real cap, the honest way to represent
/// "unbounded" in a field this type requires a value for.
pub(crate) const GCS_MULTIPART_LIMITS: MultipartLimits = MultipartLimits {
    min_part_size: 256 * 1024,
    max_part_size: 8 * 1024 * 1024,
    max_parts: u32::MAX,
    // ⚠️ The 8 MiB above is this project's chunking preference; what one
    // request may carry is bounded only by the object cap below -- GCS
    // documents no smaller single-request ceiling. `M2.3`: the conditional
    // write ceiling is this field, so a conditional 9 MiB segment is one
    // request here, exactly as it is on S3.
    max_single_put: 5 * 1024 * 1024 * 1024 * 1024,
    // GCP's published per-object quota (cloud.google.com/storage/quotas:
    // "Maximum size for a single object: 5 TiB") — not itself in doc 04,
    // unlike every other number here.
    max_object_size: 5 * 1024 * 1024 * 1024 * 1024,
};

/// The `PutOptions` a `put_opts` call sends for `precondition`.
///
/// ⚠️ **GCS-specific**, unlike its siblings in `crate::classify`/`crate::get`/
/// `crate::multipart`: a conditional `Update` carries the caller's token as
/// `UpdateVersion::version`, not `::e_tag` — `object_store`'s GCS client
/// reads `v.version` for this (`gcp/client.rs`'s `put_opts`, erroring
/// `MissingVersion` if absent); S3's own `put.rs` reads `e_tag` instead
/// (`aws/mod.rs`), so the two cannot share this one function despite sharing
/// everything else. `PutMode::Create` losing its race remaps to
/// `AlreadyExists` here exactly as it does for S3 (confirmed against
/// `gcp/client.rs`) — `crate::classify`'s shared classification already
/// covers that, unchanged.
pub(crate) fn put_options_for(precondition: Option<&Precondition>) -> PutOptions {
    let mode = match precondition {
        None => PutMode::Overwrite,
        Some(Precondition::IfAbsent) => PutMode::Create,
        Some(Precondition::IfMatches(token)) => PutMode::Update(UpdateVersion {
            e_tag: None,
            version: Some(token.as_str().to_owned()),
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

    use super::{GCS_MULTIPART_LIMITS, put_options_for};
    use crate::gcs::GcsStore;
    use crate::multipart::PutStrategy;
    use crate::tls::install_ring_provider;
    use object_store::gcp::GoogleCloudStorageBuilder;
    use object_store::{PutMode, UpdateVersion};
    use oqueue_core::{MultipartLimits, Precondition, PreconditionToken};

    /// ⚠️ **Written as plain decimal literals, deliberately** — the same
    /// reasoning `s3::put`'s matching test gives, and the same bug class it
    /// caught there: this pins the real numbers independent of the
    /// multiplication that produced them.
    #[test]
    fn gcs_multipart_limits_match_the_documented_numbers() {
        assert_eq!(
            GCS_MULTIPART_LIMITS,
            MultipartLimits {
                min_part_size: 262_144,             // 256 KiB
                max_part_size: 8_388_608,           // 8 MiB
                max_parts: 4_294_967_295,           // u32::MAX -- no real cap exists
                max_object_size: 5_497_558_138_880, // 5 TiB
                max_single_put: 5_497_558_138_880,  // the object cap itself
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
    fn put_options_for_if_matches_updates_with_the_tokens_generation() {
        let precondition = Precondition::IfMatches(PreconditionToken::new("generation-123"));
        assert_eq!(
            put_options_for(Some(&precondition)).mode,
            PutMode::Update(UpdateVersion {
                e_tag: None,
                version: Some("generation-123".to_owned()),
            }),
            "GCS reads UpdateVersion::version, not ::e_tag -- the opposite of S3"
        );
    }

    /// Small enough that a test can build a payload for it in memory without
    /// approaching real GCS's numbers.
    fn tiny_limits() -> MultipartLimits {
        MultipartLimits {
            max_single_put: 20,
            min_part_size: 5,
            max_part_size: 10,
            max_parts: 4,
            max_object_size: 35,
        }
    }

    /// `GoogleCloudStorageBuilder::build` does no I/O for a plain bucket
    /// name plus no credentials — it falls back to a lazily-resolved
    /// instance credential provider exactly as `AmazonS3Builder` does
    /// (`s3::put`'s matching helper) — so this needs no network and no
    /// literal credential (`security.md`).
    fn store_with_no_credentials() -> GcsStore {
        install_ring_provider();
        let inner = GoogleCloudStorageBuilder::new()
            .with_bucket_name("unused-in-this-test")
            .build()
            .expect("building against a bucket name alone needs no network");
        GcsStore {
            inner,
            multipart_limits: GCS_MULTIPART_LIMITS,
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
