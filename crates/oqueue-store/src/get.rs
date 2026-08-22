//! The pure decision logic behind a `get` — everything that runs before (and
//! immediately after) the one network call, kept apart from it so a T0 test
//! can reach it — plus, since `M2.4`, the one shared *error-path* network
//! helper: [`disambiguate_failed_ranged_get`], which runs only after a
//! ranged request already failed and is what closed the gap the first
//! sentence used to call the one this logic cannot close on its own.
//!
//! ⚠️ **Shared across every `object_store`-backed backend**, same reasoning
//! as `classify.rs`: `GetOptions`/`GetRange` are `object_store`'s own
//! generic types, and `ByteRange`/`Error`/`ObjectKey` are `oqueue-core`'s —
//! nothing here names S3 or GCS.

// Same reasoning `classify.rs` already gives.
#![allow(clippy::redundant_pub_crate)]

use object_store::ObjectStoreExt;
use object_store::{GetOptions, GetRange};
use oqueue_core::{ByteRange, Error, ObjectKey, Result};
use std::ops::Range;

/// The `u64` byte range a [`ByteRange`] asks for — `None` for
/// [`ByteRange::Full`].
///
/// # Errors
///
/// [`Error::ByteRangeOutOfBounds`] if `offset + length` overflows `u64` — an
/// address no real object could ever have, so `object_size` reports
/// `u64::MAX` rather than a value nothing behind it could substantiate.
pub(crate) fn requested_range(range: ByteRange, key: &ObjectKey) -> Result<Option<Range<u64>>> {
    match range {
        ByteRange::Full => Ok(None),
        ByteRange::Bounded(bounded) => {
            let start = bounded.offset();
            let end =
                start
                    .checked_add(bounded.length())
                    .ok_or_else(|| Error::ByteRangeOutOfBounds {
                        key: key.clone(),
                        offset: bounded.offset(),
                        length: bounded.length(),
                        object_size: u64::MAX,
                    })?;
            Ok(Some(start..end))
        }
    }
}

/// The `GetOptions` a `get_opts` call sends for `requested` — `None` for
/// [`ByteRange::Full`], `Some` for [`ByteRange::Bounded`].
///
/// ⚠️ **Pulled out of `get` on its own**, not merely for readability: the
/// rest of `get` only runs against a live backend, which this crate's own
/// mutation-testing budget cannot afford per mutant (`scripts/mutants.sh`'s
/// own header) — a struct-literal field silently going missing there is
/// only observable over the network, `s3_minio.rs`'s job, not this gate's.
/// A pure function with the same struct literal keeps the mutation site
/// reachable by a T0 unit test instead.
pub(crate) fn get_options_for(requested: Option<&Range<u64>>) -> GetOptions {
    GetOptions {
        range: requested.cloned().map(GetRange::Bounded),
        ..GetOptions::default()
    }
}

/// If `actual` (what `object_store` reports it actually returned) is not
/// exactly `requested`, the [`Error::ByteRangeOutOfBounds`] this backend
/// raises instead of silently accepting a truncated read — see `get`'s own
/// doc comment for why `object_store`'s own, more lenient contract is not
/// enough here. `None` if `actual` matches `requested` exactly.
pub(crate) fn truncated_range_error(
    key: &ObjectKey,
    requested: &Range<u64>,
    actual: &Range<u64>,
    object_size: u64,
) -> Option<Error> {
    if actual == requested {
        return None;
    }
    Some(Error::ByteRangeOutOfBounds {
        key: key.clone(),
        offset: requested.start,
        length: requested.end - requested.start,
        object_size,
    })
}

/// The [`Error::ByteRangeOutOfBounds`] for a `requested` range no object of
/// `object_size` bytes can satisfy — `None` if the range fits. Pure, so the
/// boundary is T0-pinned; [`disambiguate_failed_ranged_get`] is the network
/// wrapper that feeds it a live size.
///
/// ⚠️ `end > object_size`, not `start >= object_size`: `oqueue-core`'s
/// contract (`slice_range`, `M1.3`) is that *any* requested end past the
/// true size is out of bounds, the same strictness `truncated_range_error`
/// already enforces on the success path.
pub(crate) fn range_beyond_size_error(
    key: &ObjectKey,
    requested: &Range<u64>,
    object_size: u64,
) -> Option<Error> {
    if requested.end <= object_size {
        return None;
    }
    Some(Error::ByteRangeOutOfBounds {
        key: key.clone(),
        offset: requested.start,
        length: requested.end - requested.start,
        object_size,
    })
}

/// After a *ranged* `get` already failed as transient: one `HEAD`, to tell a
/// range that can never satisfy apart from a network blip. `Some` replaces
/// the transient error with [`Error::ByteRangeOutOfBounds`]; `None` keeps it.
///
/// `M2.4`, closing `M1.54`'s divergence. Measured against `MinIO`: the vendor
/// answer for start-past-size is a 416 wrapped in a `Generic` whose inner
/// `RetryError` shows `retries: 0` — `object_store` itself never retries it,
/// but `classify`'s `Generic` arm made it `Error::Transient`, so *our*
/// retry policy did, against a request that can never succeed. ⚠️ **The
/// status code is deliberately not inspected**: `RetryError` lives in
/// `object_store`'s `pub(crate)` `client::retry` module, so the type cannot
/// be named for a downcast (measured on 0.14.1 — the same dead end `s3.rs`'s
/// original comment recorded), and string-matching the rendered error is the
/// fragility `error-handling.md` rule 8 exists to rule out. A `HEAD` on the
/// error path costs one round trip precisely where a retry cycle was about
/// to be spent, and answers from ground truth instead.
///
/// ⚠️ The `HEAD` races whatever caused the failure: it reports the object's
/// *current* size. For a store of immutable segments that is a curiosity,
/// not a hazard. If the `HEAD` itself fails, `None` — the original transient
/// classification stands, because nothing was disproved.
///
/// ⚠️ Reachable only against a live backend (the fake never classifies a
/// range error as transient), so the replacing branch is pinned by the
/// conformance suite's `ranged_get_starting_past_the_object_size_is_out_of_
/// bounds` case against `MinIO`; the pure boundary above is T0-pinned —
/// `testing.md` rule 20a's stated reason.
pub(crate) async fn disambiguate_failed_ranged_get(
    inner: &impl ObjectStoreExt,
    key: &ObjectKey,
    path: &object_store::path::Path,
    requested: &Range<u64>,
) -> Option<Error> {
    match inner.head(path).await {
        Ok(meta) => range_beyond_size_error(key, requested, meta.size),
        Err(_undisproved) => None,
    }
}

#[cfg(test)]
mod tests {
    // Same justification as `tls.rs`'s: every `expect` below is on a value
    // this module just constructed from a literal it controls.
    #![allow(clippy::expect_used)]

    use super::{get_options_for, requested_range, truncated_range_error};
    use object_store::GetRange;
    use oqueue_core::{ByteRange, Error, ObjectKey};

    fn key() -> ObjectKey {
        ObjectKey::new("classify-test").expect("a non-empty key")
    }

    #[test]
    fn requested_range_of_full_is_none() {
        let k = key();
        assert_eq!(requested_range(ByteRange::Full, &k), Ok(None));
    }

    #[test]
    fn requested_range_of_bounded_is_the_half_open_interval() {
        let k = key();
        let range = ByteRange::bounded(2, 3).expect("a valid range");
        assert_eq!(requested_range(range, &k), Ok(Some(2..5)));
    }

    #[test]
    fn requested_range_overflow_is_out_of_bounds_not_a_panic() {
        let k = key();
        let range = ByteRange::bounded(u64::MAX, 1).expect("a valid range");
        assert_eq!(
            requested_range(range, &k),
            Err(Error::ByteRangeOutOfBounds {
                key: k,
                offset: u64::MAX,
                length: 1,
                object_size: u64::MAX,
            })
        );
    }

    #[test]
    fn get_options_for_a_bounded_range_carries_it() {
        let requested = 2..5;
        assert_eq!(
            get_options_for(Some(&requested)).range,
            Some(GetRange::Bounded(requested))
        );
    }

    #[test]
    fn get_options_for_a_full_read_has_no_range() {
        assert_eq!(get_options_for(None).range, None);
    }

    #[test]
    fn a_range_the_object_can_satisfy_is_not_disproved() {
        let k = key();
        // Exactly at the boundary: end == size fits.
        assert_eq!(super::range_beyond_size_error(&k, &(2..10), 10), None);
    }

    #[test]
    fn a_range_past_the_size_is_out_of_bounds_with_the_measured_size() {
        let k = key();
        assert_eq!(
            super::range_beyond_size_error(&k, &(10..15), 3),
            Some(Error::ByteRangeOutOfBounds {
                key: k.clone(),
                offset: 10,
                length: 5,
                object_size: 3,
            })
        );
        // Start in bounds, end past: still unsatisfiable under the strict
        // contract, same as `truncated_range_error`'s success-path check.
        assert_eq!(
            super::range_beyond_size_error(&k, &(2..10), 5),
            Some(Error::ByteRangeOutOfBounds {
                key: k,
                offset: 2,
                length: 8,
                object_size: 5,
            })
        );
    }

    #[test]
    fn a_range_matching_what_was_requested_is_not_an_error() {
        let k = key();
        assert_eq!(truncated_range_error(&k, &(2..10), &(2..10), 20), None);
    }

    #[test]
    fn a_truncated_range_becomes_out_of_bounds() {
        let k = key();
        let err = truncated_range_error(&k, &(2..10), &(2..7), 7)
            .expect("a range object_store did not fully honour is reported");
        assert_eq!(
            err,
            Error::ByteRangeOutOfBounds {
                key: k,
                offset: 2,
                length: 8,
                object_size: 7,
            }
        );
    }
}
