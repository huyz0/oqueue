//! Registers `oqueue-core`'s in-memory fake with the conformance suite —
//! `M1.10`. `M1.15`/`M1.17` add the S3 and GCS backends beside this.

use crate::conformance::{Capabilities, record::record_backend_run, run_conformance_suite};
use oqueue_core::FakeObjectStore;

/// The full suite, against the fake, with every capability declared.
#[test]
fn fake_passes_the_full_conformance_suite() {
    let store = FakeObjectStore::new();
    let report = run_conformance_suite("fake", &store, Capabilities::FULL);

    assert_eq!(report.backend_name, "fake");
    assert!(
        report.skipped.is_empty(),
        "the fake declares every capability, so nothing should be skipped: {:?}",
        report.skipped
    );
    assert!(
        !report.ran.is_empty(),
        "the suite must have actually run at least one case"
    );

    record_backend_run("fake");
}

/// A backend that declares a capability unsupported has the matching case
/// skipped, and it is recorded rather than silently omitted — proven here
/// against the fake with a capability claim narrower than its real behaviour,
/// specifically to exercise the skip machinery itself.
#[test]
fn a_declared_unsupported_capability_is_skipped_and_recorded() {
    let store = FakeObjectStore::new();
    let limited = Capabilities {
        conditional_writes: false,
        ranged_reads: false,
    };

    let report = run_conformance_suite("fake-with-declared-gaps", &store, limited);

    assert!(
        report.skipped.contains(&"conditional_write_if_absent"),
        "a conditional-write case must be skipped when the capability is declared off"
    );
    assert!(
        report
            .skipped
            .contains(&"ranged_get_reads_exactly_the_requested_slice"),
        "a ranged-read case must be skipped when the capability is declared off"
    );
    assert!(
        report.ran.contains(&"put_get_roundtrip"),
        "a case with no capability requirement must still run"
    );
}
