//! How long a `JoinGroup` round stays open, and why the ceiling is where it
//! is.
//!
//! ⚠️ **Its own module for `fetch::deadline`'s own reason**: a ceiling on a
//! client's number is a decision, not a helper, and this is the one place
//! this broker disagrees with what a client asked for.

// ⚠️ `pub(crate)` inside a private module: `unreachable_pub` denies the `pub`
// clippy's `redundant_pub_crate` asks for.
#![allow(clippy::redundant_pub_crate)]

/// How long a round may stay open, from what the joining member asked for.
///
/// ⚠️ **Clamped at both ends**, `fetch::deadline::park_ms`'s own shape: a
/// non-positive value is a client bug and becomes no wait at all rather than
/// an arithmetic surprise (or, for `rebalance_timeout_ms`, the wire's own
/// absent-below-v1 default of `0` — the caller's job to substitute
/// `session_timeout_ms` for that case before this function ever sees it,
/// [`super::round::effective_timeout_ms`]); anything above
/// [`MAX_BARRIER_MS`] becomes that ceiling.
pub(crate) fn barrier_ms(rebalance_timeout_ms: i32) -> u64 {
    u64::from(rebalance_timeout_ms.clamp(0, MAX_BARRIER_MS).unsigned_abs())
}

/// The longest a `JoinGroup` round may stay open, whatever a member asks
/// for.
///
/// ⚠️ **A ceiling on a client's number**, `fetch::deadline::MAX_PARK_MS`'s
/// own argument applied to a second wire field: `rebalance_timeout_ms` is an
/// `i32`, so a client may name twenty-four days, and every member of the
/// round would hold its connection that long. Real Kafka's own consumer
/// default (`max.poll.interval.ms`, which seeds the wire value) is five
/// minutes; this is ten times that, comfortably above anything a real
/// client configures and comfortably below anything that looks like a
/// stuck connection.
pub(crate) const MAX_BARRIER_MS: i32 = 3_000_000;

#[cfg(test)]
mod tests {
    use super::{MAX_BARRIER_MS, barrier_ms};

    #[test]
    fn a_negative_value_waits_not_at_all() {
        assert_eq!(barrier_ms(-1), 0);
        assert_eq!(barrier_ms(i32::MIN), 0);
    }

    #[test]
    fn an_ordinary_value_passes_through() {
        assert_eq!(barrier_ms(60_000), 60_000);
    }

    #[test]
    fn a_value_above_the_ceiling_is_clamped() {
        assert_eq!(
            barrier_ms(i32::MAX),
            u64::try_from(MAX_BARRIER_MS).unwrap_or(0)
        );
    }
}
