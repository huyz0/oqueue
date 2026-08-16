//! "Which backends has this run against" — durable, so
//! `scripts/gates/m1-complete.sh` (`M1.21`) can read it from outside the
//! test process that produced it.
//!
//! ⚠️ **A deliberate exception to `testing.md` rule 9 and `build.md` rule
//! 19** — "no shared filesystem paths, test scratch under `target/tmp`,
//! reaped by age." This is not scratch: `m1-complete.sh` reads it as the
//! record `milestones/M1.md`'s completion condition names, so it must
//! survive past the test run that wrote it and be visible to a process
//! outside that run, which `target/tmp`'s reap-by-age contract explicitly
//! does not promise. `target/conformance/` — a fourth root-level artifact
//! tree alongside `target/review/`, `target/mutants/` and `target/llvm-cov-target/`.

// The workspace denies `expect_used`; every site here is filesystem I/O
// against `target/`, which this test run itself created — a failure here
// means the test environment is broken, not that a caller should recover.
#![allow(clippy::expect_used)]

use std::io::Write;
use std::path::PathBuf;
use std::sync::Mutex;

/// Serializes every read-modify-write of the artifact file within this
/// process.
///
/// ⚠️ **Necessary, not paranoid.** `build.md` rule 15 keeps every crate's
/// integration tests in one binary, and `cargo test`'s default runner
/// executes that binary's `#[test]` functions concurrently, as threads
/// within one process — `fake.rs`'s own module doc already says `M1.15`/
/// `M1.17` add more callers of this function to the same binary. Without
/// this lock, two calls racing a read of the same pre-write file each append
/// their own name and then each overwrite the file with their own list,
/// silently dropping whichever call's write lost the race — a corrupted
/// roster, and exactly the false "everything is fine" this artifact exists
/// to prevent.
static LOCK: Mutex<()> = Mutex::new(());

/// `target/conformance/backends.txt`, workspace-root-relative.
///
/// ⚠️ Not `CARGO_MANIFEST_DIR` alone — that is this **crate's** directory
/// (`crates/oqueue-store`), and `target/` is the workspace's single shared
/// directory (`.gitignore`'s own comment: "Per-crate and per-tool artifact
/// trees" are the exception, not the rule; this is not one). Two levels up
/// from a crate under `crates/` reaches the workspace root.
fn artifact_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/conformance/backends.txt")
}

/// Appends `backend_name` to the recorded roster.
///
/// Deduplicated and sorted — so re-running one backend's test twice does not
/// double-count it, and the file reads the same regardless of test execution
/// order.
///
/// ⚠️ **Records that the suite ran, not that every case passed.** A backend
/// whose test panicked never reaches this call, since it runs after
/// `run_conformance_suite` returns; a partial roster is exactly the signal
/// `m1-complete.sh` needs, not a false "everything is fine".
///
/// # Panics
///
/// If `target/conformance/` cannot be created or written — a broken test
/// environment, not a case for a caller to recover from.
pub fn record_backend_run(backend_name: &str) {
    // ⚠️ Held across the whole read-modify-write — see `LOCK`'s doc comment.
    // Poison is recovered rather than propagated: a poisoned lock here means
    // some *other* test panicked while holding it, and the file on disk is
    // never read or written mid-operation (each critical section either
    // completes or the process is already dying), so the recovered state is
    // sound — the same reasoning `oqueue-core`'s `FakeObjectStore::lock`
    // already uses for the same shape of poison.
    let _guard = LOCK
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner);

    let path = artifact_path();
    let dir = path
        .parent()
        .expect("artifact_path always has a parent directory");
    std::fs::create_dir_all(dir).expect("target/ is writable during a test run");

    let mut names: Vec<String> = std::fs::read_to_string(&path)
        .unwrap_or_default()
        .lines()
        .map(str::to_owned)
        .collect();
    if !names.iter().any(|n| n == backend_name) {
        names.push(backend_name.to_owned());
    }
    names.sort_unstable();

    let mut file = std::fs::File::create(&path).expect("target/conformance/ is writable");
    for name in names {
        writeln!(file, "{name}").expect("writing the recorded roster succeeds");
    }
}

/// Many threads recording distinct names concurrently must not corrupt the
/// roster or drop an entry — the exact race `LOCK` exists to close.
#[test]
fn concurrent_recordings_lose_no_entry() {
    let expected_names: Vec<String> = (0..16).map(|i| format!("concurrent-{i}")).collect();

    std::thread::scope(|scope| {
        for name in &expected_names {
            scope.spawn(|| record_backend_run(name));
        }
    });

    let recorded: Vec<String> = std::fs::read_to_string(artifact_path())
        .expect("the artifact exists after every thread has recorded")
        .lines()
        .map(str::to_owned)
        .collect();
    for name in &expected_names {
        assert!(
            recorded.contains(name),
            "{name} must survive concurrent recording, got {recorded:?}"
        );
    }
}
