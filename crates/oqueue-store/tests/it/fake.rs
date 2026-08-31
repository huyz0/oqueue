//! Registers `oqueue-core`'s in-memory fake with the conformance suite —
//! `M1.10`. `M1.15`/`M1.17` add the S3 and GCS backends beside this.

use crate::conformance::{
    Capabilities, Harness, record::record_backend_run, run_conformance_suite,
};
use oqueue_core::{FakeObjectStore, FaultConfig};

/// The full suite, against the fake, with every capability declared.
///
/// ⚠️ **Including `injectable_ack_loss`, which only the fake can offer**
/// (`M3.15`). The obligation comes from `ADR-0005` guarantee 1's annotation —
/// `M1.37` measured a suite with two flags, neither about durability, and
/// `crash_after_put_before_ack` exercised by no case at all, so the promised
/// explicit skip did not exist. ⚠️ **What the flag now carries is guarantee
/// *2*'s case**, a failed `put` not being proof of absence; guarantee 1's
/// crash clause — `Ok` surviving a process death — is not something any
/// single-process suite can run, and stays `M15`'s (`M1.44`). What is
/// discharged is the flag and the explicit skip, not that clause.
#[test]
fn fake_passes_the_full_conformance_suite() {
    let store = FakeObjectStore::new();
    let arm = || {
        store.set_faults(FaultConfig {
            crash_after_put_before_ack: 1,
            ..FaultConfig::default()
        });
    };
    let harness = Harness::new(&store).with_crash_after_put(&arm);
    let report = run_conformance_suite("fake", &harness, Capabilities::FULL);

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
    // ⚠️ **Named, not counted** (`M10.22`, from `M3.46`).
    // `m3-complete.sh`'s FR-10 leg prints "ack-loss case included" over this
    // test, and the two assertions above are true of a suite that no longer
    // contains the case: delete `a_failed_put_is_not_proof_of_absence` from
    // `cases()` and nothing here notices, while the gate keeps claiming it.
    // `ADR-0005` guarantee 2 is what the case is, and `M3.15` is the row that
    // put it here — so the leg's claim is asserted where it can be checked.
    assert!(
        report.ran.contains(&"a_failed_put_is_not_proof_of_absence"),
        "the ack-loss case is what m3-complete.sh's FR-10 leg claims ran: {:?}",
        report.ran
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
        injectable_ack_loss: false,
    };

    let harness = Harness::new(&store);
    let report = run_conformance_suite("fake-with-declared-gaps", &harness, limited);

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
        report
            .skipped
            .contains(&"a_failed_put_is_not_proof_of_absence"),
        "and so must the injectable-ack-loss case — the explicit skip \
         `ADR-0005` guarantee 1's annotation asked for, carrying guarantee 2's \
         assertion"
    );
    assert!(
        report.ran.contains(&"put_get_roundtrip"),
        "a case with no capability requirement must still run"
    );
}
