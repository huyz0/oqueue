//! The measurement `ADR-0028` and `M10.5`'s clock rule both rest on.
//!
//! ⚠️ **Three files asserted this in prose before anything checked it** — the
//! ADR, `check-sans-io.sh`'s header, and `M10.5`'s backlog row all cited a
//! measurement — and review found the test they cited did not exist. It does now, and this file is the artifact those
//! sentences point at.
//!
//! ⚠️ **It reads the wall clock deliberately**, which is why it is named in
//! `check-sans-io.sh`'s `REAL_CLOCK_EXEMPT`: proving that virtual time costs
//! no real time is a wall-clock measurement, and there is no other way to make
//! it.
//!
//! What it establishes is `M10.5`'s premise: the broker's own timers —
//! `session.rs`'s `sleep_until`, `park.rs`'s deadline, `connection.rs`'s
//! `timeout` — are all `tokio::time`, so a seeded run advances them without
//! waiting. That is why `M10.5` changed no broker code.

// A panic in a test harness is the test failing, which is what it is for.
#![allow(clippy::expect_used)]

use std::time::{Duration, Instant};

/// ⚠️ **Generous, and deliberately not tight.** The claim is "no real waiting",
/// not "under N microseconds": a loaded CI box can lose tens of milliseconds to
/// scheduling without the property being false, and a threshold tight enough to
/// be impressive is one that fails for reasons the test is not about.
const NO_REAL_WAITING: Duration = Duration::from_secs(1);

/// The virtual span the run is asked to cover.
///
/// ⚠️ **A minute, not the hour a first version used.** The margin over
/// `NO_REAL_WAITING` is what matters — sixty times is already beyond mistaking
/// — and the hour had a cost the assertions hide: a `seeded_runtime` that lost
/// `start_paused` would make this test *hang* for an hour inside a pre-commit
/// suite `check-budget.sh` holds to ten seconds, rather than fail. The
/// regression this exists to catch should present as a failure.
const VIRTUAL_SPAN: Duration = Duration::from_mins(1);

#[test]
fn a_minute_of_virtual_time_costs_no_real_waiting() {
    let wall = Instant::now();

    let virtual_elapsed = oqueue_testkit::run_seeded(1, || async {
        let started = tokio::time::Instant::now();
        tokio::time::sleep(VIRTUAL_SPAN).await;
        started.elapsed()
    });

    assert_eq!(
        virtual_elapsed, VIRTUAL_SPAN,
        "the paused clock must advance by exactly what was slept"
    );
    assert!(
        wall.elapsed() < NO_REAL_WAITING,
        "a minute of virtual time took {:?} of real time — the runtime is not paused, \
         and no seeded run replays once a real clock is in the loop",
        wall.elapsed()
    );
}

/// ⚠️ **`timeout` too, and separately**, because it is the shape
/// `connection.rs` uses three times and `sleep` is not evidence for it: a
/// timeout that expired against the real clock would still return `Err`, so
/// only the wall-clock cost distinguishes the two.
#[test]
fn a_timeout_expires_on_the_virtual_clock() {
    let wall = Instant::now();

    let timed_out = oqueue_testkit::run_seeded(2, || async {
        tokio::time::timeout(VIRTUAL_SPAN, std::future::pending::<()>())
            .await
            .is_err()
    });

    assert!(timed_out, "the timeout must fire rather than hang");
    assert!(
        wall.elapsed() < NO_REAL_WAITING,
        "a minute-long timeout took {:?} of real time",
        wall.elapsed()
    );
}
