//! One seed, one run, and the seed printed when the run fails.
//!
//! ⚠️ **A failing seed that cannot be replayed is not a test** (`M10.md` task
//! 2), which is why this module is the first thing `M10` builds after the
//! decision that makes it possible.
//!
//! `ADR-0028` is that decision: `tokio` draws `select!` branch order from a
//! per-runtime RNG, and `--cfg tokio_unstable` is what lets a caller seed it.
//! Everything here is a thin wrapper over
//! [`tokio::runtime::Builder::rng_seed`], and the value is not the wrapper —
//! it is that one place decides what a *run* is, so two harnesses cannot
//! disagree about it.

use std::future::Future;
use std::panic::{AssertUnwindSafe, catch_unwind};

use tokio::runtime::{Builder, RngSeed, Runtime};

/// A runtime whose scheduling decisions follow from `seed` alone.
///
/// ⚠️ **`current_thread`, and that is not a preference.** A work-stealing
/// scheduler makes the interleaving depend on how many workers happened to be
/// free, which no seed can capture. ⚠️ **`multi_thread` here is a runtime
/// panic, not a compile error** — that is true of `#[tokio::test]`'s macro and
/// not of `Builder`, which documents "construction of the runtime will panic
/// otherwise", so the `expect` below would report tokio's message rather than
/// its own.
///
/// ⚠️ **Time is paused**, so a run advances the clock only when every task is
/// idle. A wall-clock read would make two runs of one seed differ in a value
/// nothing here chose.
///
/// # Panics
///
/// If the runtime cannot be built, which on a supported platform means the
/// process is out of file descriptors or threads.
///
/// ⚠️ A panic and not a `Result`: `error-handling.md` reserves a panic for a
/// bug or an exhausted process, and a test harness that could not start its
/// own runtime is the second. A `Result` here would be threaded through every
/// caller to be unwrapped anyway.
#[must_use]
#[expect(
    clippy::expect_used,
    reason = "see Panics above — a harness that cannot build a runtime has no \
              caller that could do anything with a Result"
)]
pub fn seeded_runtime(seed: u64) -> Runtime {
    Builder::new_current_thread()
        .enable_all()
        .start_paused(true)
        // ⚠️ **Little-endian, and the length is a choice rather than a
        // conversion.** `RngSeed::from_bytes` hashes an arbitrary-length slice
        // with `DefaultHasher`, so a caller handing it more or fewer bytes for
        // the same `u64` gets a different schedule — which is why this
        // encoding is written down rather than left to each caller, and why a
        // later edit "using the whole seed" would silently invalidate every
        // seed `M10.11` has recorded. See `ADR-0028`'s Consequences: the hash
        // is not stable across toolchains either.
        .rng_seed(RngSeed::from_bytes(&seed.to_le_bytes()))
        .build()
        .expect("a current-thread runtime builds")
}

/// What a failing run prints, so a test can assert on it.
///
/// ⚠️ **Extracted because nothing could constrain it inline**, and the
/// extraction is half a fix rather than a whole one. Measured after it:
/// `cargo mutants -p oqueue-testkit --file src/seed.rs` reports 4 mutants, 2
/// caught, 2 unviable — the two on the functions returning `Runtime` and an
/// unbounded `T`, neither of which implements `Default`. ⚠️ **What is still
/// held by nothing is that the message is *printed*.** `cargo mutants` does
/// not mutate a macro statement, so deleting the `eprintln!` below, or sending
/// it to stdout where `cargo test` swallows it on a pass, leaves every test
/// green — verified. Closing that needs an injectable sink, not another
/// sentence.
#[must_use]
pub fn seed_message(seed: u64) -> String {
    format!("SEED {seed} — replay with this value to reproduce")
}

/// Runs `body` under [`seeded_runtime`], naming the seed if it panics.
///
/// ⚠️ **The message is the deliverable.** A harness that fails without saying
/// which seed failed has produced a flake rather than a reproduction, and
/// `M10.md`'s first risk is that a "usually deterministic" harness is worse
/// than an honestly non-deterministic one because failures get dismissed.
///
/// # Panics
///
/// Re-panics with the original payload after printing the seed, so the test
/// framework still reports the original failure.
/// ⚠️ **The body is `async`, and the first version's was not.** With
/// `F: FnOnce() -> T` an async block compiles, is never polled, and the test
/// passes having run nothing — measured: `run_seeded(7, || async {
/// panic!("never runs") })` was green, and a `tokio::spawn` inside a
/// synchronous body never ran either, because `block_on` returned the instant
/// the body did and dropped the runtime. Every row from `M10.5` on is written
/// against this wrapper, so that shape was a false-green generator for the
/// whole milestone.
pub fn run_seeded<T, F, Fut>(seed: u64, body: F) -> T
where
    F: FnOnce() -> Fut,
    Fut: Future<Output = T>,
{
    let outcome = catch_unwind(AssertUnwindSafe(|| seeded_runtime(seed).block_on(body())));
    match outcome {
        Ok(value) => value,
        Err(payload) => {
            // ⚠️ `eprintln!`, because `cargo test` captures stdout on a passing
            // test and shows stderr on a failing one — printing to stdout here
            // would hide the seed exactly when it is needed.
            eprintln!("\n{}\n", seed_message(seed));
            std::panic::resume_unwind(payload)
        }
    }
}

#[cfg(test)]
mod tests {
    // A panic in a test harness is the test failing, which is what it is for.
    #![allow(clippy::expect_used)]

    use super::{run_seeded, seeded_runtime};

    /// The claim `M10.4` exists for: one seed, one interleaving.
    ///
    /// ⚠️ **Both arms are ready**, so an unseeded runtime picks between them at
    /// random — measured at 102/98 over 200 iterations before `ADR-0028`.
    fn picks(seed: u64) -> String {
        seeded_runtime(seed).block_on(async {
            let mut out = String::new();
            for _ in 0..40 {
                tokio::select! {
                    () = std::future::ready(()) => out.push('a'),
                    () = std::future::ready(()) => out.push('b'),
                }
            }
            out
        })
    }

    #[test]
    fn one_seed_reproduces_one_interleaving() {
        assert_eq!(picks(7), picks(7));
    }

    /// ⚠️ **The other half, and the one that makes the first mean something.**
    /// A `seeded_runtime` that ignored its argument would pass the test above
    /// and be useless: every seed would explore the same single schedule.
    #[test]
    fn a_different_seed_reaches_a_different_interleaving() {
        assert_ne!(picks(7), picks(9));
    }

    #[test]
    fn a_seeded_run_returns_what_its_body_returns() {
        assert_eq!(run_seeded(1, || async { 41 + 1 }), 42);
    }

    /// ⚠️ **The case that would have caught a synchronous body.** With
    /// `F: FnOnce() -> T` the future below is built and dropped unpolled, so
    /// the spawned task never runs and this assertion never fires.
    #[test]
    fn the_body_is_actually_polled_and_can_spawn() {
        let ran = run_seeded(2, || async {
            tokio::spawn(async { 7_u8 })
                .await
                .expect("the spawned task completes")
        });
        assert_eq!(ran, 7);
    }

    #[test]
    fn a_panicking_body_panics_with_its_own_payload() {
        let caught = std::panic::catch_unwind(|| {
            run_seeded(3, || async { panic!("boom-xyz") });
        })
        .expect_err("the original failure must reach the caller");

        // ⚠️ **The payload, not just `is_err()`.** A wrapper that panicked with
        // its own message instead would satisfy `is_err()` while replacing the
        // assertion text and location the framework reports.
        let message = caught
            .downcast_ref::<&str>()
            .copied()
            .expect("the payload is the body's own &str");
        assert_eq!(message, "boom-xyz");
    }

    /// ⚠️ **The seed has to be *in* the message**, which is the whole point: a
    /// failure that does not name its seed is a flake rather than a
    /// reproduction. ⚠️ This pins the message's *content*; that it reaches
    /// stderr at all is still unheld — see `seed_message`'s own doc.
    #[test]
    fn the_failure_message_names_the_seed() {
        assert!(super::seed_message(4812).contains("4812"));
        assert_ne!(super::seed_message(1), super::seed_message(2));
    }

    /// A body whose outcome depends on the schedule: whichever `select!` arm
    /// wins decides the value, so it differs by seed.
    async fn schedule_dependent() -> char {
        tokio::select! {
            () = std::future::ready(()) => 'a',
            () = std::future::ready(()) => 'b',
        }
    }

    /// ⚠️ **The loop the module exists for**, and the acceptance criterion
    /// `M10.4`'s row states: a run that fails under one seed fails *identically*
    /// when replayed from it, and a different seed reaches the other outcome.
    /// The earlier failure-path test panicked unconditionally, so it failed the
    /// same way under every seed and constrained nothing about replay.
    #[test]
    fn a_failure_replays_from_its_seed_and_a_different_seed_does_not() {
        // The two seeds reach different arms — that is what makes the
        // assertion below a *schedule* failure rather than a constant one.
        let seeds: Vec<char> = (0..64).map(|s| run_seeded(s, schedule_dependent)).collect();
        let failing = seeds
            .iter()
            .position(|c| *c == 'b')
            .expect("some seed reaches the second arm");
        let passing = seeds
            .iter()
            .position(|c| *c == 'a')
            .expect("some seed reaches the first arm");

        let assert_is_a = |seed: u64| {
            run_seeded(seed, || async {
                assert_eq!(
                    schedule_dependent().await,
                    'a',
                    "seed {seed} took the second arm"
                );
            });
        };

        // Fails under the failing seed...
        let first = std::panic::catch_unwind(|| assert_is_a(failing as u64));
        assert!(first.is_err());
        // ...identically on replay from the same seed...
        let replay = std::panic::catch_unwind(|| assert_is_a(failing as u64));
        assert!(replay.is_err(), "the same seed must fail the same way");
        // ...and not under a seed that reaches the other arm.
        assert_is_a(passing as u64);
    }
}
