//! A run drawn from a seed, and the reduction of a failing one.
//!
//! ⚠️ **Two halves of one row, and the order is the argument.** `M10.11` left
//! the harness *positioned* on a seeded choice and unable to diverge: its run
//! was a written sequence, so a seed changed branch order and nothing
//! observable. A [`Schedule`] is what makes a seed change what the run *does*
//! — and only then is there anything for [`shrink`] to reduce. Shrinking an
//! interleaving nobody can replay is work spent on an artifact that is not yet
//! evidence, which is why `M10.12` follows the corpus rather than preceding it.
//!
//! ⚠️ **Sans-I/O, so the reduction is testable without a broker.** [`shrink`]
//! takes a predicate: whatever "still fails" means is the caller's, and here it
//! is a synthetic one, which is what lets the reducer's own behaviour be pinned
//! rather than inferred from a system it is reducing against.

use core::fmt;

/// One thing a generated run can do.
///
/// ⚠️ **`Heal` is a step so a fault can be *cleared*, and neither fault is
/// one-shot in the sense that would make this true without it.**
/// `RefuseJournal` arms a sticky `AtomicBool` — every commit fails until a
/// `Heal` — and `StormStore` is consumed by exactly one store call. So a
/// schedule's meaning still depends on how far back the reader looks for the
/// nearer of a fault or a `Heal`; what `Heal` buys is that the look terminates
/// rather than reaching past the start of the schedule. Recorded here rather
/// than glossed over, because getting this wrong is what produced the
/// blocking finding this row's own review made: an executor that appended
/// steps without a `Heal` first inherited whatever the draw left armed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Step {
    /// Produce one batch and observe.
    Produce,
    /// Park a fetch and produce underneath it — the one step whose outcome a
    /// schedule choice can decide.
    RacedProduce,
    /// Make the next store call fail before writing.
    StormStore,
    /// Make the journal refuse every commit, until a [`Step::Heal`].
    ///
    /// ⚠️ **Not "the next one"** — `LogFaults::refuse_append` is a sticky
    /// `AtomicBool`, read on every append. `RefuseJournal,Produce,Produce`
    /// refuses two commits, not one.
    RefuseJournal,
    /// Clear every injected fault.
    Heal,
    /// Observe without acting.
    Look,
}

impl Step {
    /// Every variant, in a fixed order.
    ///
    /// ⚠️ **Hand-written and pinned by a test**, because an added variant that
    /// nobody put here would silently never be drawn — a generator quietly
    /// covering less than it claims, which is the failure mode a harness can
    /// least afford.
    pub const ALL: [Self; 6] = [
        Self::Produce,
        Self::RacedProduce,
        Self::StormStore,
        Self::RefuseJournal,
        Self::Heal,
        Self::Look,
    ];
}

/// A run, as a list of steps.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Schedule {
    steps: Vec<Step>,
}

impl fmt::Display for Schedule {
    /// ⚠️ **One line, so a failing schedule fits in a panic message.** A
    /// reduced schedule is the artifact a corpus entry points at, and one that
    /// has to be reassembled from a `Debug` dump across forty lines is one
    /// nobody copies.
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for (at, step) in self.steps.iter().enumerate() {
            if at > 0 {
                write!(f, ",")?;
            }
            write!(f, "{step:?}")?;
        }
        Ok(())
    }
}

impl Schedule {
    /// A schedule of exactly these steps.
    #[must_use]
    pub const fn of(steps: Vec<Step>) -> Self {
        Self { steps }
    }

    /// `len` steps drawn from `seed`.
    ///
    /// ⚠️ **Its own generator, not `tokio`'s RNG.** `ADR-0028` seeds branch
    /// order and exposes no number generator, so a schedule is drawn
    /// separately — the same split `latency.rs` makes, and a run replays only
    /// when both seeds are the same one.
    #[must_use]
    pub fn draw(seed: u64, len: usize) -> Self {
        let mut rng = Xorshift::new(seed);
        let steps = (0..len)
            .map(|_| {
                let at = rng.next() % Step::ALL.len() as u64;
                // The modulus is the array's own length, so this indexes.
                #[expect(
                    clippy::indexing_slicing,
                    reason = "the index is taken modulo the slice's length"
                )]
                Step::ALL[usize::try_from(at).unwrap_or(0)]
            })
            .collect();
        Self { steps }
    }

    /// The steps, in order.
    #[must_use]
    // ⚠️ Not `const`: `Deref` on `Vec` is not a const trait yet, and clippy's
    // `missing_const_for_fn` does not know that.
    pub fn steps(&self) -> &[Step] {
        &self.steps
    }

    /// How many steps.
    #[expect(
        clippy::missing_const_for_fn,
        reason = "Vec's Deref is not const, and the lint does not see through it"
    )]
    #[must_use]
    pub fn len(&self) -> usize {
        self.steps.len()
    }

    /// Whether there are none.
    #[expect(
        clippy::missing_const_for_fn,
        reason = "Vec's Deref is not const, and the lint does not see through it"
    )]
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }

    /// This schedule without the step at `at`.
    ///
    /// ⚠️ **No bounds guard, deliberately.** The only caller is [`shrink`]'s
    /// loop, whose index is bounded by `best.len()`, and a guard that silently
    /// returned the schedule unchanged for an out-of-range index would turn a
    /// reducer bug into a reduction that quietly stopped reducing. `remove`
    /// panics, which in a test harness is the failure being visible.
    #[must_use]
    fn without(&self, at: usize) -> Self {
        let mut steps = self.steps.clone();
        steps.remove(at);
        Self { steps }
    }
}

/// Reduces `schedule` to one no single further deletion keeps failing.
///
/// ⚠️ **Deletion only, and that is a deliberate limit rather than an
/// unfinished algorithm.** Delta debugging also *simplifies* elements — a
/// `RacedProduce` weakened to a `Produce`, say — and doing that here would need
/// an ordering over steps that says which is simpler, which is a claim about
/// the system rather than about the schedule. What deletion gives is a
/// schedule in which **every remaining step is necessary**, which is the
/// property a corpus entry has to have to be worth keeping.
///
/// ⚠️ **`still_fails` is called with candidates and must be deterministic.** A
/// flaky predicate makes the reducer delete a step that mattered and keep one
/// that did not, and the result is a smaller schedule that reproduces nothing —
/// worse than no reduction, because it looks like evidence.
///
/// ⚠️ **The caller's schedule is returned unchanged if it does not fail.**
/// Reducing something that passes would otherwise walk all the way to the empty
/// schedule and report it as minimal.
#[must_use]
pub fn shrink<F>(schedule: &Schedule, mut still_fails: F) -> Schedule
where
    F: FnMut(&Schedule) -> bool,
{
    if !still_fails(schedule) {
        return schedule.clone();
    }
    let mut best = schedule.clone();
    // ⚠️ **Both loops are bounded ranges, and that is a mutation-testing
    // decision as much as a correctness one.** The obvious shape — a `while`
    // over an index advanced by `at += 1` — has a mutant (`at *= 1`) that never
    // advances, and `cargo mutants` reports it as a *timeout* rather than a
    // survivor: the suite hangs instead of failing, which is the one outcome a
    // harness must not have. A pass count bounded by the input's length can
    // only terminate. ⚠️ **`+ 1` because the last pass is the one that removes
    // nothing** and is what proves the result minimal.
    for _ in 0..=schedule.len() {
        let mut removed = false;
        // ⚠️ **Backwards, so a removal does not disturb the indices still to
        // be visited.** Forwards, deleting at `at` shifts everything after it
        // down and the next index skips a step — which would leave a schedule
        // the reducer called minimal with a removable step in it.
        for at in (0..best.len()).rev() {
            let candidate = best.without(at);
            if still_fails(&candidate) {
                best = candidate;
                removed = true;
            }
        }
        if !removed {
            break;
        }
    }
    best
}

/// The same `xorshift64*` the simulated store's latency uses.
///
/// ⚠️ **Duplicated rather than shared, and that is worth saying.** The other
/// copy is in `oqueue-store`'s test tree, which this crate must not depend on
/// (`check-layering.sh`), and lifting it into `oqueue-core` would put a test
/// generator in the crate every seam lives in. Two twelve-line copies is the
/// cheaper of the two mistakes.
struct Xorshift {
    state: u64,
}

impl Xorshift {
    const fn new(seed: u64) -> Self {
        Self {
            state: if seed == 0 {
                0x9E37_79B9_7F4A_7C15
            } else {
                seed
            },
        }
    }

    const fn next(&mut self) -> u64 {
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        self.state.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
}

#[cfg(test)]
mod tests {
    // A panic in a test is the test failing, which is what it is for.
    #![allow(clippy::expect_used)]

    use super::{Schedule, Step, shrink};

    /// ⚠️ **A literal, so the generator itself is pinned.** Comparing two draws
    /// from one seed passes under any mutation of `Xorshift` — both sides move
    /// together — which is how three of its operators survived `cargo mutants`.
    /// A recorded sequence is what makes the arithmetic load-bearing.
    #[test]
    fn a_known_seed_draws_a_known_schedule() {
        assert_eq!(
            Schedule::draw(7, 8).to_string(),
            "Heal,StormStore,Produce,Produce,Look,Heal,Look,Heal",
            "the generator's arithmetic changed, and every recorded seed now \
             names a different run"
        );
    }

    #[test]
    fn a_schedule_with_steps_is_not_empty() {
        assert!(!Schedule::draw(1, 3).is_empty());
        assert!(Schedule::of(Vec::new()).is_empty());
    }

    #[test]
    fn one_seed_gives_one_schedule() {
        assert_eq!(Schedule::draw(7, 40), Schedule::draw(7, 40));
        assert_ne!(Schedule::draw(7, 40), Schedule::draw(8, 40));
    }

    /// ⚠️ **Every variant is reachable**, which `Step::ALL` being hand-written
    /// makes worth checking: a variant nobody added to it is one the generator
    /// silently never draws.
    #[test]
    fn a_long_draw_reaches_every_step() {
        let drawn = Schedule::draw(3, 400);
        for step in Step::ALL {
            assert!(
                drawn.steps().contains(&step),
                "{step:?} was never drawn in 400 steps"
            );
        }
    }

    /// ⚠️ **The reducer's whole contract**: what comes back still fails, and
    /// nothing in it can be removed.
    #[test]
    fn shrinking_keeps_only_what_the_failure_needs() {
        let schedule = Schedule::draw(11, 60);
        let fails = |s: &Schedule| s.steps().contains(&Step::RefuseJournal);
        let reduced = shrink(&schedule, fails);
        assert_eq!(reduced, Schedule::of(vec![Step::RefuseJournal]));
    }

    /// A failure needing two steps in order keeps both, and only those.
    #[test]
    fn a_failure_needing_two_steps_keeps_two() {
        let schedule = Schedule::draw(5, 80);
        let fails = |s: &Schedule| {
            s.steps()
                .windows(2)
                .any(|pair| pair == [Step::StormStore, Step::Produce])
        };
        let reduced = shrink(&schedule, fails);
        assert_eq!(reduced, Schedule::of(vec![Step::StormStore, Step::Produce]));
    }

    /// ⚠️ **One pass is not always enough**, and this is the case that proves
    /// the outer loop is doing something. A backward pass handles a removal
    /// that enables an *earlier* one within the same pass; what needs a second
    /// pass is a removal that enables a *later* one. Here dropping the leading
    /// `Heal` is what makes `StormStore` first, and only then is the trailing
    /// `Produce` removable. ⚠️ **Without this, `cargo mutants` deletes the `!`
    /// in `if !removed` — stop after the first productive pass — and every
    /// other case stays green while the reducer under-reduces.
    #[test]
    fn a_removal_that_enables_a_later_one_needs_a_second_pass() {
        // ⚠️ The predicate has to hold for the *starting* schedule too, or
        // `shrink` returns it untouched and the case proves nothing — which is
        // what a first version of this did.
        let schedule = Schedule::of(vec![Step::Heal, Step::StormStore, Step::Produce]);
        let fails = |s: &Schedule| s.steps().first() == Some(&Step::StormStore) || s.len() >= 3;
        let reduced = shrink(&schedule, fails);
        assert_eq!(reduced, Schedule::of(vec![Step::StormStore]));
    }

    /// ⚠️ **A schedule that does not fail comes back untouched.** Reducing one
    /// that passes would walk to the empty schedule and report it as minimal —
    /// a "reduction" that is a lie about what was reproduced.
    #[test]
    fn a_passing_schedule_is_not_reduced() {
        let schedule = Schedule::draw(2, 12);
        let reduced = shrink(&schedule, |_| false);
        assert_eq!(reduced, schedule);
    }

    /// ⚠️ **A predicate that always fails reduces to nothing**, which is the
    /// right answer and worth pinning: the empty schedule genuinely is the
    /// minimal reproduction of "everything fails".
    #[test]
    fn a_failure_needing_nothing_reduces_to_nothing() {
        let reduced = shrink(&Schedule::draw(4, 20), |_| true);
        assert!(reduced.is_empty());
    }

    /// ⚠️ **One line, because a corpus entry points at it.**
    #[test]
    fn a_schedule_prints_on_one_line() {
        let printed = Schedule::of(vec![Step::StormStore, Step::Produce]).to_string();
        assert_eq!(printed, "StormStore,Produce");
    }
}
