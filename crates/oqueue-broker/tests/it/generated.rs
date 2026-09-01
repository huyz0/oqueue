//! A run drawn from a seed, and a failing one reduced to what it needs.
//!
//! ⚠️ **This is what `M10.11` handed on.** That row left the harness
//! *positioned* on a seeded choice and unable to diverge: its run was a written
//! sequence, so a seed changed `park.rs`'s branch order and nothing observable.
//! A `Schedule` is drawn from the seed, so a seed now changes what the run
//! *does* — and only then is there anything for `shrink` to reduce.
//!
//! ⚠️ **The invariants are the same three**, checked after every step exactly
//! as `invariants.rs`'s written run checks them. What is new is that the steps
//! are not written down.

#![allow(clippy::expect_used)]
// ⚠️ `pub` here is `pub(crate)` in effect — see `roundtrip.rs`.
#![allow(unreachable_pub)]
#![allow(clippy::redundant_pub_crate)]

use crate::invariants::{
    Run, a_parked_fetch_races_a_commit, observe, seed, starts_at_the_beginning,
};
use crate::support::{Broker, broker};
use oqueue_core::{FaultConfig, LogFaults, StormKind};
use oqueue_testkit::{Schedule, Step, shrink};

/// How many steps a drawn run takes.
///
/// ⚠️ **Long enough to contain a fault and its consequence, short enough that
/// each candidate is cheap.** Every step is in-memory, so the cost is real but
/// small: measured at 33 predicate calls / 21 ms to shrink one run at this
/// length, and 249 calls / 3.16 s at ten times it — proportional, not the
/// order-of-magnitude margin an earlier draft of this comment claimed.
const STEPS: usize = 24;

/// Runs `schedule` against `broker` and reports what it left behind.
///
/// ⚠️ **Takes the broker rather than building one**, which lets a test built
/// on `execute` inspect what it left behind — `storm_store_fails_the_next_produce`
/// checks the store's own contents afterward. `shrink`'s own caller
/// (`execute_fresh`, below) is what builds a *fresh* broker per candidate,
/// because a predicate carrying state between calls would make a step look
/// necessary for what an earlier candidate did.
async fn execute(schedule: &Schedule, broker: &Broker) -> Outcome {
    let mut run = Run::new(broker);
    for step in schedule.steps() {
        match *step {
            Step::Produce => run.step(broker).await,
            Step::RacedProduce => a_parked_fetch_races_a_commit(broker, &mut run).await,
            Step::StormStore => broker.store.inner().set_faults(FaultConfig {
                storm: Some((StormKind::Transient, 1)),
                ..FaultConfig::default()
            }),
            Step::RefuseJournal => broker.log.set_faults(LogFaults {
                refuse_append: true,
                ..LogFaults::default()
            }),
            Step::Heal => {
                broker.store.inner().set_faults(FaultConfig::default());
                broker.log.set_faults(LogFaults::default());
            }
            Step::Look => {
                if let Some(at) = observe(&run.dispatcher, broker, run.orphans).await {
                    starts_at_the_beginning(&at);
                    run.invariants
                        .check(&at)
                        .unwrap_or_else(|broken| panic!("{schedule}: {broken}"));
                }
            }
        }
    }
    Outcome {
        orphans: run.orphans,
        checks: run.invariants.steps(),
        races: run.races,
    }
}

/// A fresh broker per call, for `shrink`'s predicate — see `execute`'s doc.
async fn execute_fresh(schedule: &Schedule) -> Outcome {
    let broker = broker(&["orders"]).await;
    execute(schedule, &broker).await
}

/// What one execution left behind.
struct Outcome {
    /// Objects that landed and were never named by a commit.
    orphans: i64,
    /// How many observations were checked.
    checks: u64,
    /// How many `RacedProduce` steps genuinely raced.
    races: u32,
}

/// ⚠️ **The invariants hold whatever the seed draws**, which is the claim a
/// generated run exists to make and the written one could not: the written run
/// checks the sequence somebody thought of.
#[test]
fn a_drawn_schedule_holds_the_invariants() {
    let schedule = Schedule::draw(seed(), STEPS);
    let outcome = oqueue_testkit::run_seeded(seed(), || execute_fresh(&schedule));
    assert!(
        outcome.checks > 0,
        "{schedule} checked nothing — a run that observes nowhere holds \
         everything vacuously"
    );
}

/// ⚠️ **`StormStore` genuinely fails the next store call.** Review measured
/// that gutting its arm to a no-op left the other tests here green, because
/// nothing downstream of the executor checks that a storm was ever armed.
#[test]
fn storm_store_fails_the_next_produce() {
    let object_written = oqueue_testkit::run_seeded(0, || async {
        let broker = broker(&["orders"]).await;
        let schedule = Schedule::of(vec![Step::StormStore, Step::Produce]);
        execute(&schedule, &broker).await;
        !broker.store.inner().is_empty()
    });
    assert!(
        !object_written,
        "a storm answers before the write, so nothing should land — a \
         gutted StormStore arm lets the PUT through"
    );
}

/// ⚠️ **`Look` genuinely observes**, distinct from an arm that advances nothing.
/// A schedule ending in `Look` checks once more than the same schedule without
/// it — gutting the arm to `{}` leaves both counts equal.
#[test]
fn look_actually_checks() {
    let with_look = Schedule::of(vec![Step::Produce, Step::Look]);
    let without_look = Schedule::of(vec![Step::Produce]);
    let checks_with = oqueue_testkit::run_seeded(0, || execute_fresh(&with_look)).checks;
    let checks_without = oqueue_testkit::run_seeded(0, || execute_fresh(&without_look)).checks;
    assert!(
        checks_with > checks_without,
        "Look added no check: with={checks_with} without={checks_without}"
    );
}

/// ⚠️ **`RacedProduce` genuinely parks a fetch, and a fallback to `run.step`
/// would still pass every other test here.** Review measured it: gutting the
/// arm to `run.step` left `a_drawn_schedule_holds_the_invariants` and the
/// shrinking test both green, because both only need *a* produce to happen,
/// not a parked one — and every assertion inside
/// `a_parked_fetch_races_a_commit` is on data a fallback never produces, so no
/// external check can reach it either. `Run::races` is the one fact only the
/// real path increments.
#[test]
fn raced_produce_actually_races() {
    let schedule = Schedule::of(vec![Step::RacedProduce, Step::RacedProduce]);
    let outcome = oqueue_testkit::run_seeded(0, || execute_fresh(&schedule));
    assert_eq!(outcome.races, 2, "two RacedProduce steps must race twice");
}

/// ⚠️ **The reducer, over real runs rather than a synthetic predicate.**
/// `schedule.rs`'s own tests pin what `shrink` does to a list; this pins that
/// it does it to *this system*, where each candidate costs a broker.
///
/// ⚠️ **The property reduced is a real observable and not a bug**, because
/// there is no bug to reduce: a refused journal leaves an object nothing
/// references (`M10.9`), and the smallest schedule that produces one is a
/// refusal followed by something that writes. A reducer that could only be
/// demonstrated against a defect would be a reducer nobody could demonstrate.
///
/// ⚠️ **The assertion is the reducer's contract, not a literal.** A first
/// version expected exactly `RefuseJournal,Produce` and failed under seed 1,
/// which reduced to `RefuseJournal,RacedProduce` — equally minimal, because a
/// raced produce writes too. Pinning one spelling would have made the test a
/// statement about which step the draw happened to leave, so what is asserted
/// is what `shrink` promises: the result still fails, and **no single deletion
/// keeps it failing**.
///
/// ⚠️ **The drawn schedule is extended rather than skipped.** Not every seed
/// draws a refusal before a write — seed 10 does not — and returning early
/// there made the test vacuous for the default seed, which is the one CI runs.
///
/// ⚠️ **`Heal` first, and a first version omitted it.** `RefuseJournal` arms a
/// sticky refusal (`fault_metadata_log.rs`'s `refuse_append` is an
/// `AtomicBool` read on every append) and `StormStore` a one-shot storm, both
/// consumed by nothing before the schedule ends unless the draw happened to
/// leave the store clear — measured to fail 37 of 1000 seeds without the heal,
/// including several the corpus's own replay would exercise. `Heal` makes the
/// extension's precondition independent of what the draw left armed, which is
/// what "always holds" actually needs.
#[test]
fn shrinking_reduces_a_drawn_schedule_to_steps_that_are_all_necessary() {
    let mut steps = Schedule::draw(seed(), STEPS).steps().to_vec();
    steps.extend([Step::Heal, Step::RefuseJournal, Step::Produce]);
    let drawn = Schedule::of(steps);
    let leaves_an_orphan =
        |s: &Schedule| oqueue_testkit::run_seeded(seed(), || execute_fresh(s)).orphans > 0;
    assert!(
        leaves_an_orphan(&drawn),
        "{drawn} leaves no orphan to reduce"
    );

    let reduced = shrink(&drawn, leaves_an_orphan);

    assert!(
        reduced.len() < drawn.len(),
        "{drawn} was not reduced at all"
    );
    assert!(leaves_an_orphan(&reduced), "{reduced} no longer reproduces");
    for at in 0..reduced.len() {
        let mut without = reduced.steps().to_vec();
        without.remove(at);
        let without = Schedule::of(without);
        assert!(
            !leaves_an_orphan(&without),
            "{reduced} is not minimal: dropping step {at} leaves {without}, \
             which still reproduces"
        );
    }
}
