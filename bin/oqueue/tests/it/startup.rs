//! The acceptance criterion, run against the real binary.

// The sites below are on a process this test just spawned.
#![allow(clippy::expect_used)]

/// ⚠️ `cargo run -p oqueue` exits 0 with a version line and no other side
/// effect. Asserted against the built binary rather than by calling `main`,
/// because the exit code and the absence of stderr are the parts that matter
/// and neither is observable from inside the process.
#[test]
fn it_prints_a_version_line_and_exits_zero() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_oqueue"))
        .output()
        .expect("the binary runs");

    assert!(output.status.success(), "exit status: {:?}", output.status);
    // ⚠️ Pinned exactly, including the provider name: this is the assertion
    // that would fail if the default wiring were ever swapped for one that does
    // not refuse — ADR-0006's plaintext-DEK outcome.
    assert_eq!(
        String::from_utf8_lossy(&output.stdout),
        format!("oqueue {} (NoOpKeyProvider)\n", env!("CARGO_PKG_VERSION"))
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "",
        "a version line is the whole output"
    );
}
