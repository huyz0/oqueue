//! Per-request delay drawn from doc 04 §2's measured percentiles.
//!
//! ⚠️ **A distribution, not a constant.** A model that answers every request at
//! the p50 hides every schedule the tail decides — which is the class
//! `M10.md`'s risk list calls a harness that runs and finds nothing. The
//! numbers here are doc 04 §2's, not round ones chosen for readability:
//!
//! | | p50 | p90 | p99 |
//! |---|---|---|---|
//! | GET | 15–45 ms | ~60 ms | 85–300 ms |
//! | PUT | 60–70 ms | — | 130–250 ms |
//!
//! ⚠️ **Five of the nine constants are the doc's; four are not, and saying
//! which is the point of this paragraph.** Where a range is published the
//! midpoint is taken — GET p50 30, p90 60, p99 192.5; PUT p50 65, p99 190 —
//! and that is already a choice, because the doc reports spreads across object
//! sizes and methodologies while a simulation needs one curve.
//!
//! ⚠️ **The p0 and p100 endpoints are invented**, because the doc publishes
//! none and a piecewise curve needs ends. `5` and `20` ms are floors chosen to
//! be below every published median without reaching S3 Express territory (§2's
//! only single-digit GET figure is 3 ms and it is a different storage class);
//! `300` and `250` are the *tops* of the same p99 ranges whose midpoints were
//! taken above, so one range is read two ways on purpose. ⚠️ **These four are
//! round numbers presented as such**, which is the distinction the row this
//! file closes exists to keep.
//!
//! ⚠️ **Its own RNG, not the runtime's.** `ADR-0028` seeds `tokio`'s scheduler,
//! which decides *branch order*; nothing there is reachable as a number
//! generator. So a latency draw is seeded separately, and a run is reproducible
//! only when both seeds are.

// ⚠️ `pub(crate)` inside a private module: `unreachable_pub` denies the bare
// `pub` clippy's `redundant_pub_crate` asks for — the trade `model.rs` makes
// beside it, for the same reason.
#![allow(clippy::redundant_pub_crate)]

use std::time::Duration;

/// Doc 04 §2's GET percentiles, in milliseconds.
///
/// ⚠️ Read as `(percentile, millis)` and interpolated between, so a draw at
/// p70 lands between the p50 and p90 points rather than snapping to one.
const GET_MS: &[(f64, f64)] = &[
    (0.0, 5.0),
    (0.50, 30.0),
    (0.90, 60.0),
    (0.99, 192.5),
    (1.0, 300.0),
];

/// Doc 04 §2's PUT percentiles, in milliseconds.
///
/// ⚠️ **No p90 is published for PUT, and running p50 → p99 straight is not
/// neutral.** Linear across a 49-percentile gap puts the model's p90 at
/// ~166 ms and its p75 at ~128 ms — measured — so about a quarter of simulated
/// PUTs sit at or above the *bottom* of the documented p99 range, where the
/// real distribution would have a fraction of that. The body is too heavy and
/// the top is capped at 250 ms, which is the opposite shape to the heavy tail
/// §2 describes. ⚠️ **Recorded rather than fixed**: inventing a p90 would be a
/// round number wearing a citation, and `the_put_body_is_known_to_be_heavy`
/// pins the distortion so it cannot drift unnoticed.
const PUT_MS: &[(f64, f64)] = &[(0.0, 20.0), (0.50, 65.0), (0.99, 190.0), (1.0, 250.0)];

/// Which curve a request draws from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Op {
    /// The GET curve. ⚠️ **What the model's dispatch actually sends here is
    /// everything that is not `PUT` or `POST`** — so a single-object `DELETE`
    /// draws from it too. That arm is unreachable through `S3Store`, which
    /// answers deletes as a bulk `POST`; a case that reaches it would be timed
    /// against the wrong curve.
    Read,
    /// The PUT curve: `PUT`, and the bulk-delete `POST`.
    Write,
}

/// A deterministic latency source.
///
/// ⚠️ **`xorshift64*`, hand-written, so this adds no dependency.** It is not a
/// cryptographic generator and does not need to be: what is required is that
/// one seed gives one sequence, which is the whole of `M10.4`'s criterion.
#[derive(Debug)]
pub(crate) struct Latency {
    state: u64,
}

impl Latency {
    /// ⚠️ A zero seed is remapped, because `xorshift` is stuck at zero — a
    /// caller passing `0` would otherwise get every request at the same
    /// percentile and no error saying so.
    pub(crate) const fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 {
                0x9E37_79B9_7F4A_7C15
            } else {
                seed
            },
        }
    }

    /// The next uniform draw in `[0, 1)`.
    fn next_unit(&mut self) -> f64 {
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        let x = self.state.wrapping_mul(0x2545_F491_4F6C_DD1D);
        // 53 bits is what an f64 mantissa holds exactly.
        #[expect(
            clippy::cast_precision_loss,
            reason = "53 bits is exactly what the mantissa represents"
        )]
        {
            // 2^21 * 2^21 * 2^11 = 2^53, which is what `x >> 11` spans. A
            // first version divided by 2^45 and returned draws up to 256, so
            // every percentile clamped to the curve's last point.
            (x >> 11) as f64
                / f64::from(1_u32 << 21)
                / f64::from(1_u32 << 21)
                / f64::from(1_u32 << 11)
        }
    }

    /// A delay for one request of `op`.
    pub(crate) fn draw(&mut self, op: Op) -> Duration {
        let curve = match op {
            Op::Read => GET_MS,
            Op::Write => PUT_MS,
        };
        let p = self.next_unit();
        // Every curve point is a positive millisecond value under 1000, so the
        // product is far inside `u64` and non-negative; the truncation is to
        // whole microseconds, which is the resolution a simulated delay needs.
        #[expect(
            clippy::cast_possible_truncation,
            clippy::cast_sign_loss,
            reason = "curve values are bounded positive milliseconds"
        )]
        Duration::from_micros((interpolate(curve, p) * 1000.0) as u64)
    }
}

/// The millisecond value at percentile `p`, linear between recorded points.
#[expect(
    clippy::indexing_slicing,
    reason = "the loop bounds are the slice's own length"
)]
fn interpolate(curve: &[(f64, f64)], p: f64) -> f64 {
    for i in 1..curve.len() {
        let (hi_p, hi_ms) = curve[i];
        if p <= hi_p {
            let (lo_p, lo_ms) = curve[i - 1];
            let span = hi_p - lo_p;
            // Two points at one percentile would divide by zero; the curves
            // above are strictly increasing, and this is what keeps a future
            // edit from being a silent NaN.
            if span <= 0.0 {
                return hi_ms;
            }
            return (hi_ms - lo_ms).mul_add((p - lo_p) / span, lo_ms);
        }
    }
    curve.last().map_or(0.0, |&(_, ms)| ms)
}

#[cfg(test)]
mod tests {
    #![allow(clippy::expect_used)]

    use super::{Latency, Op};

    /// Draws `n` samples and returns them sorted, in milliseconds.
    fn sorted_ms(seed: u64, op: Op, n: usize) -> Vec<f64> {
        let mut l = Latency::new(seed);
        let mut v: Vec<f64> = (0..n).map(|_| l.draw(op).as_secs_f64() * 1000.0).collect();
        v.sort_by(f64::total_cmp);
        v
    }

    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        clippy::cast_precision_loss,
        reason = "an index into a sample whose length the caller controls"
    )]
    fn at(sorted: &[f64], p: f64) -> f64 {
        let i = ((sorted.len() as f64 - 1.0) * p) as usize;
        sorted.get(i).copied().expect("a non-empty sample")
    }

    /// ⚠️ **The bands are doc 04 §2's, not the midpoints this file interpolates
    /// through.** Asserting the midpoint back would test the constant against
    /// itself; asserting the published range tests that the curve produces a
    /// distribution the document would recognise.
    #[test]
    fn get_latency_lands_in_the_documented_bands() {
        let s = sorted_ms(7, Op::Read, 10_000);
        let p50 = at(&s, 0.50);
        let p99 = at(&s, 0.99);
        assert!(
            (15.0..=45.0).contains(&p50),
            "p50 was {p50} ms, doc 04 §2 says 15-45"
        );
        assert!(
            (85.0..=300.0).contains(&p99),
            "p99 was {p99} ms, doc 04 §2 says 85-300"
        );
    }

    #[test]
    fn put_latency_lands_in_the_documented_bands() {
        let s = sorted_ms(7, Op::Write, 10_000);
        let p50 = at(&s, 0.50);
        let p99 = at(&s, 0.99);
        assert!(
            (60.0..=70.0).contains(&p50),
            "p50 was {p50} ms, doc 04 §2 says 60-70"
        );
        assert!(
            (130.0..=250.0).contains(&p99),
            "p99 was {p99} ms, doc 04 §2 says 130-250"
        );
    }

    /// ⚠️ **A write is slower than a read at the same percentile**, which is the
    /// one relationship between the two curves the document states outright —
    /// and the one a transposed pair of constants would break silently.
    #[test]
    fn a_write_costs_more_than_a_read_at_the_median() {
        let reads = sorted_ms(3, Op::Read, 10_000);
        let writes = sorted_ms(3, Op::Write, 10_000);
        assert!(at(&writes, 0.50) > at(&reads, 0.50));
    }

    /// The draws in the order they were made, which is what a replay needs.
    fn sequence_ms(seed: u64, op: Op, n: usize) -> Vec<f64> {
        let mut l = Latency::new(seed);
        (0..n).map(|_| l.draw(op).as_secs_f64() * 1000.0).collect()
    }

    /// ⚠️ **Unsorted, deliberately.** Comparing sorted samples pins the
    /// multiset and not the order — a change that returned the same draws in a
    /// different order would stay green while a replay assigned different
    /// delays to different requests, which is the property the row closes on.
    #[test]
    fn one_seed_gives_one_sequence() {
        assert_eq!(sequence_ms(11, Op::Read, 64), sequence_ms(11, Op::Read, 64));
        assert_ne!(sequence_ms(11, Op::Read, 64), sequence_ms(12, Op::Read, 64));
    }

    /// ⚠️ The tail is the point: a model whose slowest draw is near its median
    /// would satisfy the band assertions above while hiding every schedule the
    /// tail decides.
    #[test]
    fn the_tail_is_far_from_the_median() {
        let s = sorted_ms(5, Op::Read, 10_000);
        assert!(
            at(&s, 0.999) > at(&s, 0.50) * 3.0,
            "p999 {} should be well past p50 {}",
            at(&s, 0.999),
            at(&s, 0.50)
        );
    }

    /// ⚠️ **Pins a known distortion, not a desired property.** `PUT_MS` has no
    /// published p90, so linear interpolation across p50 → p99 makes the body
    /// heavier than the real distribution. This is here so the number cannot
    /// move without someone deciding to move it — and so a later curve with a
    /// real p90 fails it, which is the signal to delete this test.
    #[test]
    fn the_put_body_is_known_to_be_heavy() {
        let s = sorted_ms(7, Op::Write, 10_000);
        let p90 = at(&s, 0.90);
        assert!(
            (150.0..=180.0).contains(&p90),
            "p90 was {p90} ms; the linear p50-to-p99 curve puts it near 166"
        );
    }

    #[test]
    fn a_zero_seed_still_varies() {
        let s = sorted_ms(0, Op::Read, 64);
        assert!(
            s.first() < s.last(),
            "a zero seed must not freeze the generator"
        );
    }
}
