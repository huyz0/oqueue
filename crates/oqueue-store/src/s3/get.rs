//! The pure decision logic behind a `get` — everything that runs before (and
//! immediately after) the one network call, kept apart from it so a T0 test
//! can reach it. See `s3.rs`'s own doc comment on `get` for the network call
//! itself and the one gap this logic cannot close on its own.

// Same reasoning `classify.rs` already gives.
#![allow(clippy::redundant_pub_crate)]

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
