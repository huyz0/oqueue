//! FR-35's safety inequality, as constants and a check (`M5.22`).
//!
//! `deletion_delay > max_metadata_staleness + max_in_flight_fetch_duration +
//! clock_skew` — doc 12 §4.6. A reader resolves a reference from an index at
//! most `max_metadata_staleness` old, then fetches for at most
//! `max_in_flight_fetch_duration`; the deleter's clock may run ahead of the
//! reader's by `clock_skew`. An object deleted later than all three after the
//! index stopped naming it is one no reader inside those bounds can reach.
//!
//! ⚠️ **The bounds are what make the sum mean anything**, and each has its own
//! enforcement elsewhere: staleness by the cache refusing to serve past
//! [`MAX_METADATA_STALENESS_MS`](oqueue_core::MAX_METADATA_STALENESS_MS)
//! (`ADR-0021`), and a reader outside the bounds by 404 ⇒ refresh: the broker
//! answers a reaped object `OFFSET_OUT_OF_RANGE` and the client re-resolves.
//! Non-reusable keys (`ADR-0037`) are the third enabler: a key deleted is
//! never written again, so a late GET cannot find someone else's bytes.

use oqueue_core::{Error, MAX_METADATA_STALENESS_MS, Result};

/// The longest a fetch may hold a resolved reference before its GET lands.
///
/// ⚠️ **The broker's fetch park ceiling plus the store's retry window**: 60 s
/// of `MAX_PARK_MS` (a parked fetch may resolve before it waits) and 180 s,
/// `object_store`'s default `retry_timeout`, which `retry_config_for` leaves
/// at the vendor default. A literal because this crate cannot name the
/// broker's constant, and the sum is the claim to check if either moves.
pub const MAX_IN_FLIGHT_FETCH_MS: i64 = 240_000;

/// How far the deleter's clock may run ahead of a reader's.
///
/// ⚠️ **UNDERIVED**: NTP-disciplined hosts hold well under a second; ten is a
/// margin for a host that is not, not a measurement.
pub const MAX_CLOCK_SKEW_MS: i64 = 10_000;

/// How long an object waits after the index stops naming it.
///
/// ⚠️ **More than twice the bound**, so a term doubling does not silently
/// break the inequality — [`GcTerms::check`] refuses it loudly instead.
pub const DELETION_DELAY_MS: i64 = 600_000;

/// The three terms a deletion delay must exceed the sum of.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GcTerms {
    /// How stale an index a reader may resolve from.
    pub metadata_staleness_ms: i64,
    /// How long a resolved reference may be in flight.
    pub fetch_duration_ms: i64,
    /// How far the deleter's clock may lead a reader's.
    pub clock_skew_ms: i64,
}

impl GcTerms {
    /// The terms this build runs with.
    pub const CONFIGURED: Self = Self {
        #[expect(
            clippy::cast_possible_wrap,
            reason = "5_000 fits an i64; a const cannot call try_from"
        )]
        metadata_staleness_ms: MAX_METADATA_STALENESS_MS as i64,
        fetch_duration_ms: MAX_IN_FLIGHT_FETCH_MS,
        clock_skew_ms: MAX_CLOCK_SKEW_MS,
    };

    /// The sum a deletion delay must exceed.
    #[must_use]
    pub const fn bound(self) -> i64 {
        self.metadata_staleness_ms
            .saturating_add(self.fetch_duration_ms)
            .saturating_add(self.clock_skew_ms)
    }

    /// Refuses a delay that does not strictly exceed [`bound`](Self::bound).
    ///
    /// # Errors
    ///
    /// [`Error::GcInequalityViolated`] when `delay_ms <= bound()`.
    pub const fn check(self, delay_ms: i64) -> Result<()> {
        let bound_ms = self.bound();
        if delay_ms > bound_ms {
            Ok(())
        } else {
            Err(Error::GcInequalityViolated { delay_ms, bound_ms })
        }
    }
}
