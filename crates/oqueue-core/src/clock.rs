//! The only way to read time, and the fake that makes it testable.

use crate::{Error, Result};
use core::sync::atomic::{AtomicI64, Ordering};

/// A wall-clock instant, in milliseconds since the Unix epoch.
///
/// # Invariant
///
/// **Never negative.** Backed by `i64` because that is the protocol's type for
/// a record timestamp, so no conversion is needed at the wire boundary.
///
/// ⚠️ Kafka uses `-1` on the wire to mean "no timestamp". That is a sentinel,
/// not a time, and mapping it to `None` is `M2`'s job rather than something
/// this type should be able to represent.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct Timestamp(i64);

impl Timestamp {
    /// The Unix epoch.
    pub const EPOCH: Self = Self(0);

    /// Builds a timestamp.
    ///
    /// # Errors
    ///
    /// [`Error::NegativeTimestamp`] if `millis` is negative.
    pub const fn from_millis(millis: i64) -> Result<Self> {
        if millis < 0 {
            return Err(Error::NegativeTimestamp { got: millis });
        }
        Ok(Self(millis))
    }

    /// Milliseconds since the Unix epoch, always `>= 0`.
    #[must_use]
    pub const fn as_millis(self) -> i64 {
        self.0
    }
}

impl core::fmt::Display for Timestamp {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        write!(f, "{}ms", self.0)
    }
}

/// Reads the current time.
///
/// ⚠️ **This is the only way anything in this workspace reads a clock**
/// (NFR-51). `scripts/check-sans-io.sh` refuses `SystemTime::now` and
/// `Instant::now` everywhere but the crates that are allowed to implement this
/// trait, which is what makes every time-dependent behaviour testable without
/// waiting and without a flaky threshold.
///
/// # What an implementor must guarantee
///
/// See ADR-0004. In short: `now` is **cheap**, **infallible**, and
/// **non-decreasing** — ⚠️ **a property of the clock, not of a thread**: if one
/// call returns `t`, any later call *on any thread* returns a value `>= t`.
/// This said "two calls in program order", which is the weaker per-thread
/// property, and an implementor satisfying the doc comment would have violated
/// ADR-0004 guarantee 5 — the guarantee that costs something to implement, and
/// the one review already caught `FakeClock` breaking with a load-check-store
/// `advance`. It is *not* required to be strictly increasing; two calls may
/// return the same instant, and a caller that needs distinct values must not
/// get them from here.
///
/// ADR-0004 is `docs/internal/product/decisions/0004-clock-seam.md` in this
/// repository.
pub trait Clock: Send + Sync + core::fmt::Debug {
    /// The current wall-clock time.
    fn now(&self) -> Timestamp;
}

/// A [`Clock`] that only moves when told to.
///
/// ⚠️ **Lives here, beside the trait, and not in `oqueue-testkit`** —
/// `contracts.md` rule 9 and `testing.md` rule 4. That is what lets a
/// downstream crate test its time-dependent logic without taking a dependency
/// on the testkit.
///
/// # Fidelity to the contract
///
/// It is non-decreasing under concurrent use because [`FakeClock::advance`] is
/// the only mutator, is a single atomic read-modify-write, and cannot move time
/// backwards — so a test cannot accidentally build a clock
/// that violates the guarantee its implementors are held to.
#[derive(Debug)]
pub struct FakeClock {
    millis: AtomicI64,
}

impl FakeClock {
    /// A fake starting at the Unix epoch.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            millis: AtomicI64::new(0),
        }
    }

    /// A fake starting at `start`.
    #[must_use]
    pub const fn starting_at(start: Timestamp) -> Self {
        Self {
            millis: AtomicI64::new(start.as_millis()),
        }
    }

    /// Moves time forward.
    ///
    /// # Errors
    ///
    /// [`Error::NegativeClockAdvance`] if `millis` is negative — ⚠️ moving a clock
    /// backwards would let a test construct a state no real implementor can
    /// produce, which is the failure mode a fake exists to prevent rather than
    /// to enable.
    ///
    /// [`Error::TimestampOverflow`] if the result would leave the `i64` range.
    pub fn advance(&self, millis: i64) -> Result<Timestamp> {
        if millis < 0 {
            return Err(Error::NegativeClockAdvance { got: millis });
        }
        // ⚠️ One atomic read-modify-write, not load-check-store. This fake is
        // `Send + Sync` and takes `&self` precisely so an `Arc<FakeClock>` can
        // be shared, and a non-atomic sequence loses updates *and* lets an
        // observer see time move backwards — which is the single guarantee the
        // `Clock` contract has. Found in review, by running it on four threads.
        match self
            .millis
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |before| {
                before.checked_add(millis)
            }) {
            // `checked_add` succeeded inside the closure, so this cannot wrap.
            Ok(before) => Ok(Timestamp(before.saturating_add(millis))),
            Err(before) => Err(Error::TimestampOverflow {
                base: before,
                delta: millis,
            }),
        }
    }
}

impl Default for FakeClock {
    fn default() -> Self {
        Self::new()
    }
}

impl Clock for FakeClock {
    fn now(&self) -> Timestamp {
        Timestamp(self.millis.load(Ordering::SeqCst))
    }
}
