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
        format!(
            "oqueue {} (NoOpKeyProvider, FakeObjectStore)\n",
            env!("CARGO_PKG_VERSION")
        )
    );
    assert_eq!(
        String::from_utf8_lossy(&output.stderr),
        "",
        "a version line is the whole output"
    );
}

/// ⚠️ **`M4.30`'s own acceptance criterion, and nothing pinned it.** The
/// banner is the only thing that tells an operator *which* variable was
/// wrong, and it is in `main`, which no unit test can reach — so review of
/// `M4.30` reverted it to the version that printed an empty string for an
/// undecodable value and watched the whole crate suite stay green.
///
/// ⚠️ **`Command::env` takes an `OsStr`**, so the undecodable value is set on
/// the child without touching this process's own environment and without the
/// `unsafe` that `std::env::set_var` now requires and this binary forbids.
#[cfg(unix)]
#[test]
fn an_undecodable_store_selection_names_the_variable_and_exits_non_zero() {
    use std::os::unix::ffi::OsStringExt as _;

    let output = std::process::Command::new(env!("CARGO_BIN_EXE_oqueue"))
        .env(
            "OQUEUE_STORE",
            std::ffi::OsString::from_vec(b"s3\xff".to_vec()),
        )
        .output()
        .expect("the binary runs");

    assert!(
        !output.status.success(),
        "a value the process cannot read must not start a broker: {:?}",
        output.status
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("OQUEUE_STORE"),
        "the operator must be told which variable was wrong, got {stderr:?}"
    );
    assert!(
        stderr.contains("s3\u{fffd}"),
        "and what it was set to -- an empty string here is the defect M4.30 fixed, got {stderr:?}"
    );
}

/// ⚠️ **And the value is escaped**, so a newline in it cannot forge a line
/// that reads as the broker's own. Review of `M4.30` set `OQUEUE_STORE` to
/// `s3\noqueue: using S3Store (durable, encrypted)` and the banner printed
/// that second line verbatim, asserting the opposite of what had happened.
#[test]
fn a_newline_in_the_store_selection_cannot_forge_a_broker_line() {
    let output = std::process::Command::new(env!("CARGO_BIN_EXE_oqueue"))
        .env(
            "OQUEUE_STORE",
            "s3\noqueue: using S3Store (durable, encrypted)",
        )
        .output()
        .expect("the binary runs");

    assert!(!output.status.success());
    let stderr = String::from_utf8_lossy(&output.stderr);
    // ⚠️ Not a line count: the banner is followed by a second line listing
    // the valid values, so two lines is correct. What must not happen is a
    // line the *value* wrote.
    assert!(
        !stderr
            .lines()
            .any(|line| line.starts_with("oqueue: using ")),
        "the value must not be able to write a line that reads as the broker's own, got {stderr:?}"
    );
    assert!(
        stderr.contains("\\n"),
        "the newline is escaped rather than printed, got {stderr:?}"
    );
}
