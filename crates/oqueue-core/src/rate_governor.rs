//! Per-prefix request-rate governance: admit, or say how long to wait.
//!
//! ⚠️ **A decision, not a wait.** Same shape as [`crate::RetryPolicy`]: this
//! computes whether a request may proceed and, if not, for how long — it
//! never sleeps, because sleeping touches a clock this crate does not read
//! (NFR-51). The caller (`oqueue-store`'s backends, `M1.15`/`M1.17`) is the
//! one with a runtime to wait on.

use crate::{Clock, Timestamp};
use std::num::NonZeroU32;
use std::time::Duration;

/// Which class of request a rate check is for — S3 sizes writes and reads
/// differently (doc 04 §3: 3,500 write/sec, 5,500 read/sec per partitioned
/// prefix).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum OpClass {
    /// `PUT`/`POST`/`COPY`/`DELETE`.
    Write,
    /// `GET`/`HEAD`.
    Read,
}

/// How a prefix's ceiling is sized, and whether it changes over time.
///
/// ⚠️ **Two backends, two distinct shapes — this is why it is a policy a
/// caller chooses, not one hardcoded rule.** S3's ceiling is fixed per op
/// class from the moment a prefix exists. GCS's is not: a freshly-created
/// key range starts with a much lower ceiling and needs to **ramp**
/// gradually — Google's own guidance is to double the request rate roughly
/// every 20 minutes, or expect elevated latency/errors (doc 04 §3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateLimitPolicy {
    /// A constant ceiling per op class, from the first request onward — S3's
    /// shape.
    FixedCeiling {
        /// Writes admitted per second.
        writes_per_sec: NonZeroU32,
        /// Reads admitted per second.
        reads_per_sec: NonZeroU32,
    },
    /// A ceiling that starts at a per-op-class initial rate and doubles
    /// every `doubling_period` — GCS's shape. ⚠️ **Split by op class,
    /// deliberately** — doc 04 §3 documents GCS buckets starting at roughly
    /// 1,000 write/sec and 5,000 read/sec of "free" capacity, a 5x
    /// difference, not one shared rate. Found by M1's checkpoint review
    /// (`M1.33`): an earlier version of this variant carried a single
    /// `initial_per_sec` on a citation to this same section that the section
    /// does not actually support.
    Ramping {
        /// The write ceiling at the moment the governor was created.
        initial_writes_per_sec: NonZeroU32,
        /// The read ceiling at the moment the governor was created.
        initial_reads_per_sec: NonZeroU32,
        /// How often the ceiling doubles, the same cadence for both op
        /// classes — doc 04 §3's ramp guidance names one cadence, not one
        /// per class.
        doubling_period: Duration,
    },
}

impl RateLimitPolicy {
    /// The ceiling for `op_class`, `elapsed` after the governor holding
    /// this policy was created.
    fn ceiling(&self, op_class: OpClass, elapsed: Duration) -> u32 {
        match self {
            Self::FixedCeiling {
                writes_per_sec,
                reads_per_sec,
            } => match op_class {
                OpClass::Write => writes_per_sec.get(),
                OpClass::Read => reads_per_sec.get(),
            },
            Self::Ramping {
                initial_writes_per_sec,
                initial_reads_per_sec,
                doubling_period,
            } => {
                let initial_per_sec = match op_class {
                    OpClass::Write => *initial_writes_per_sec,
                    OpClass::Read => *initial_reads_per_sec,
                };
                if doubling_period.is_zero() {
                    // A zero period has no meaningful doubling cadence;
                    // treat it as "already fully ramped" rather than divide
                    // by zero.
                    return u32::MAX;
                }
                // ⚠️ A step function, not a smooth curve — `doublings` is a
                // whole number of elapsed periods, so the ceiling jumps by a
                // full power of two at each period boundary rather than
                // ramping continuously between them (matches
                // `ramping_policy_doubles_after_one_period`'s own test: the
                // bucket admits 4 in a row the instant the clock crosses the
                // boundary, the new ceiling in full, not a gradual approach
                // to it). Integer-only — a ratio of millisecond counts — to
                // avoid floating point entirely; "roughly doubling every 20
                // minutes" (doc 04 §3) does not
                // demand finer granularity than that.
                let elapsed_ms = u128::from(u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX));
                let period_ms = u128::from(u64::try_from(doubling_period.as_millis()).unwrap_or(1));
                let doublings = elapsed_ms / period_ms.max(1);
                // Cap the shift so this never overflows `u64` regardless of
                // how long the governor has been alive.
                let doublings = u32::try_from(doublings).unwrap_or(u32::MAX).min(32);
                let ramped = u64::from(initial_per_sec.get())
                    .saturating_mul(1u64 << doublings)
                    .min(u64::from(u32::MAX));
                // The `.min(u64::from(u32::MAX))` just above makes this
                // always fit.
                u32::try_from(ramped).unwrap_or(u32::MAX)
            }
        }
    }
}

/// Whether a request may proceed now.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateDecision {
    /// Proceed now.
    Admit,
    /// Wait this long before trying again.
    Delay(Duration),
}

/// A token bucket for one [`OpClass`], refilled continuously against the
/// policy's ceiling at the moment of refill.
///
/// ⚠️ **Two bugs review found here, in two different rounds, and the shape
/// below is what surviving both left.** The first: unconditionally resetting
/// `last_refill` to `now` on every call discarded whatever fraction of
/// elapsed time had not yet earned a whole token — at a `ceiling` low enough
/// that `1000 / ceiling` exceeds the interval between calls, that fraction
/// truncated to zero *every single call*, and no token ever accrued no
/// matter how much wall-clock time actually passed (reproduced: 2,000 calls
/// 1ms apart at 7/sec, zero admits instead of ~14). The fix that followed
/// tried to bank the unspent remainder by converting `earned` tokens back
/// into milliseconds (`earned * 1000 / ceiling`) and only advancing the
/// reference point by that — which introduced the second bug: that
/// back-conversion is itself a *second*, independent floor division whenever
/// `1000 % ceiling != 0`, and it systematically under-charges elapsed time
/// on every call. The discount compounds across sustained calls and lets the
/// long-run admitted rate settle well above the ceiling — reproduced: 512/sec
/// configured, 999 admitted in one simulated second; modelling S3's own
/// 3,500/sec ceiling under concurrent bursts pushed the achieved rate to
/// roughly +471%. `carry_millitokens` below is what actually closes both:
/// track the remainder in the *same scaled units* the earned-token division
/// already uses, so nothing is ever converted back through a second,
/// information-losing division.
#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens: u32,
    last_refill: Timestamp,
    /// The unconverted remainder from the last refill, in milli-tokens —
    /// `elapsed_ms * ceiling`, before dividing by 1000 to get whole tokens.
    /// Exact and lossless *as long as `ceiling` does not change* between the
    /// call that produced it and the call that consumes it; see `admit`'s
    /// handling of `carry_ceiling` for what happens when it does (`Ramping`).
    carry_millitokens: u64,
    /// The ceiling `carry_millitokens` was computed against.
    carry_ceiling: u32,
}

impl Bucket {
    fn admit(&mut self, now: Timestamp, ceiling: u32) -> RateDecision {
        if ceiling == 0 {
            // A ceiling of zero never admits anything; there is no future
            // moment at which waiting would help, and no elapsed time is
            // worth banking.
            return RateDecision::Delay(Duration::MAX);
        }

        // ⚠️ Integer milliseconds throughout, deliberately — floating-point
        // token accounting would make "admits at exactly the ceiling"
        // dependent on rounding, which is exactly the flakiness `tdd.md`'s
        // no-flake rule warns about. `Clock::now` is documented
        // non-decreasing, so this subtraction does not go negative in
        // practice; the trailing `.max(0)` is what actually clamps it to
        // zero if that guarantee were ever violated by a bad implementor —
        // `saturating_sub` on its own only guards `i64::MIN`/`MAX` overflow,
        // not negative results.
        let elapsed_ms = now
            .as_millis()
            .saturating_sub(self.last_refill.as_millis())
            .max(0);
        #[allow(clippy::cast_sign_loss)] // just clamped to `>= 0` above
        let elapsed_ms = elapsed_ms as u64;
        self.last_refill = now;

        // ⚠️ **The exact-arithmetic core.** `carry_millitokens` is only
        // meaningful at the ceiling it was computed against — if `ceiling`
        // changed since the last call (only `Ramping` ever does this), the
        // stale carry is discarded rather than reinterpreted at the new
        // rate. That loses at most one call's worth of sub-millitoken
        // precision, a single bounded event at a ramp boundary — not the
        // per-call, compounding bias the second bug produced.
        let carried = if self.carry_ceiling == ceiling {
            self.carry_millitokens
        } else {
            0
        };
        let millitokens = carried.saturating_add(elapsed_ms.saturating_mul(u64::from(ceiling)));
        let earned = u32::try_from(millitokens / 1000).unwrap_or(u32::MAX);
        self.carry_millitokens = millitokens % 1000;
        self.carry_ceiling = ceiling;

        self.tokens = self.tokens.saturating_add(earned).min(ceiling);

        if self.tokens >= 1 {
            self.tokens -= 1;
            RateDecision::Admit
        } else {
            // How long until one token accrues, at the current ceiling.
            let wait_ms = 1000u64.div_ceil(u64::from(ceiling));
            RateDecision::Delay(Duration::from_millis(wait_ms))
        }
    }
}

/// Governs request admission against one [`RateLimitPolicy`], per prefix.
#[derive(Debug, Clone)]
pub struct RateGovernor {
    policy: RateLimitPolicy,
    created_at: Timestamp,
    write_bucket: Bucket,
    read_bucket: Bucket,
}

impl RateGovernor {
    /// A governor starting full — the first requests up to the ceiling are
    /// admitted immediately, matching a prefix that has not been throttled
    /// yet.
    #[must_use]
    pub fn new(policy: RateLimitPolicy, clock: &dyn Clock) -> Self {
        let now = clock.now();
        let initial_write = policy.ceiling(OpClass::Write, Duration::ZERO);
        let initial_read = policy.ceiling(OpClass::Read, Duration::ZERO);
        Self {
            policy,
            created_at: now,
            write_bucket: Bucket {
                tokens: initial_write,
                last_refill: now,
                carry_millitokens: 0,
                carry_ceiling: initial_write,
            },
            read_bucket: Bucket {
                tokens: initial_read,
                last_refill: now,
                carry_millitokens: 0,
                carry_ceiling: initial_read,
            },
        }
    }

    /// Whether a request of `op_class` may proceed now.
    pub fn admit(&mut self, op_class: OpClass, clock: &dyn Clock) -> RateDecision {
        let now = clock.now();
        let elapsed = Duration::from_millis(
            u64::try_from(
                now.as_millis()
                    .saturating_sub(self.created_at.as_millis())
                    .max(0),
            )
            .unwrap_or(0),
        );
        let ceiling = self.policy.ceiling(op_class, elapsed);
        let bucket = match op_class {
            OpClass::Write => &mut self.write_bucket,
            OpClass::Read => &mut self.read_bucket,
        };
        bucket.admit(now, ceiling)
    }
}
