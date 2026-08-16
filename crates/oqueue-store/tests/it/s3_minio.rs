//! The S3 backend against a real `MinIO` container — `M1.15`.
//!
//! ⚠️ **T2, not T0/T1** (`testing.md`'s tier table): needs Docker and the
//! network, so it is `#[ignore]`d rather than gated by a feature or a
//! runtime environment check (`testing.md` rule 2 — capability, never
//! `#[cfg]`). CI's T2 step starts `MinIO` and sets every `AWS_*` variable
//! this backend reads **before the test process launches**, then runs with
//! `--ignored` — this file never calls `std::env::set_var` itself
//! (`testing.md` rule 12: mutating the environment at runtime is unsafe and
//! racy against every other test in the same binary). A developer without
//! Docker running locally never opts in; nothing here needs remembering to
//! turn off.
//!
//! ⚠️ **Must run on a multi-thread Tokio runtime.** The conformance suite's
//! own case bodies drive their futures with a hand-rolled, no-allocation
//! busy-poll loop (`conformance::block_on`) instead of `.await` — exactly
//! right for `oqueue-core`'s and the fake's zero-I/O cases, since nothing
//! there ever needs a reactor. `object_store`'s S3 client polls real
//! sockets, which need Tokio's I/O driver actually turning. On a
//! `current_thread` runtime the one OS thread never reaches that turn while
//! stuck spinning this crate's busy-poll loop; `multi_thread` leaves other
//! worker threads free to drive it while this task spins. `oqueue-store`'s
//! own `Cargo.toml` pins `rt-multi-thread` for exactly this reason.

// The workspace denies `expect_used`; the one site below is on a
// precondition CI's T2 step is responsible for holding (`AWS_*` describing a
// reachable endpoint) before this test ever runs — a panic here means the
// test environment is broken, not that this test should recover.
#![allow(clippy::expect_used)]

use crate::conformance::{Capabilities, record::record_backend_run, run_conformance_suite};
use oqueue_store::S3Store;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore = "T2: needs Docker MinIO and AWS_* env set by CI before the process starts"]
async fn s3_backend_passes_the_full_conformance_suite_against_minio() {
    let store = S3Store::from_env().expect(
        "AWS_* environment variables must describe a reachable MinIO endpoint \
         when this test is run with --ignored",
    );
    let report = run_conformance_suite("s3", &store, Capabilities::FULL);

    assert_eq!(report.backend_name, "s3");
    assert!(
        report.skipped.is_empty(),
        "S3Store declares every capability, so nothing should be skipped: {:?}",
        report.skipped
    );
    assert!(
        !report.ran.is_empty(),
        "the suite must have actually run at least one case"
    );

    record_backend_run("s3");
}
