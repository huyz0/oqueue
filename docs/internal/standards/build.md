# Build

Profiles, toolchain, workspace layout, and hygiene. Evidence in
[docs/researches/18](../../researches/18-rust-performance-methodology.md) §3;
this file is the rules.

## Toolchain

1. **`rust-toolchain.toml` pins the compiler**, and bumping it is a task with a
   commit — never a side effect of someone running `rustup update`. An MSRV
   field plus a CI install step is not a pin.
2. **`Cargo.lock` is committed**, and CI builds with `--locked` so a stale
   lockfile fails loudly rather than resolving silently.
3. **`cargo build` needs only cargo and a C compiler.** ⚠️ The sole exception is
   the FIPS build, which needs CMake and Go — and that requirement is precisely
   why FIPS is a separate artifact rather than a feature of the default one.
4. **A dependency that adds a host-tool requirement is a decision** and gets an
   ADR. → `security.md` rule 22 for the untrusted-input case

## Profiles

Defined at the workspace root only; member-crate `[profile]` sections are
ignored.

5. **`lto = "thin"` in release is mandatory, not an optimization.** ⚠️ Crate
   boundaries are optimization barriers by default: a non-generic `pub fn`
   without `#[inline]` is not inlinable across crates without LTO. In a
   multi-crate workspace this decides whether the codec inlines at all.
6. ⚠️ **`lto = false` with `codegen-units = 1` performs no LTO whatsoever** —
   strictly worse than the stock profile. Setting cgu=1 without setting `lto` is
   a mistake that looks like tuning.
7. **`overflow-checks = true` in every profile, including release.** →
   `security.md` rule 4
8. **`panic = "unwind"`** — a decoder panic must not kill a node serving other
   tenants.
9. **`debug = "line-tables-only"` in release**, so production flamegraphs
   symbolicate without carrying full debuginfo.
10. **Five profiles**: `dev`, `release`, `dist` (fat LTO, tagged artifacts),
    `bench` (= `dist` + `debug = true`, nothing else), `release-checked`
    (release codegen + assertions, a CI safety net).
11. **`[profile.dev.package."*"] debug = false`** — dependencies are ~90% of the
    compiled code and debuginfo is 60–70% of `target/debug`.

## Workspace layout

12. **Keep the dependency DAG wide and shallow.** Depth, not crate count, sets
    the floor on build time, because a dependent starts as soon as its
    dependency's `.rmeta` exists. → `check-layering.sh`
13. **Proc-macro crates are few, tiny, and at the root.** They break pipelining:
    a dependent cannot even parse until the macro crate is fully compiled and
    linked.
14. **Public generic functions delegate to a private non-generic inner
    function**, so only a thin adapter is monomorphized per type.
15. **One integration-test binary per crate** — `tests/it/main.rs` with `mod a;
    mod b;`. Each `tests/*.rs` is otherwise a separate crate, separate link, and
    separate copy of the debuginfo.
16. **`cargo-hakari` unifies workspace features**, so `cargo test -p a` followed
    by `-p b` does not rebuild shared dependencies.

## Hygiene

⚠️ **Cargo has no `target/` garbage collection**, on any channel, and none is
planned. Artifacts are fingerprint-keyed and orphaned forever when a dependency
version bumps.

17. **`cargo clean` is the wrong tool** — all-or-nothing, so it costs a full
    rebuild, so nobody runs it, so disk grows. Prune by age instead
    (`cargo-sweep` for `target/`, `cargo-cache` for `$CARGO_HOME`).
18. **Coverage runs under a separate `CARGO_TARGET_DIR`.** `-C
    instrument-coverage` changes the fingerprint, so a shared directory makes
    instrumented and normal builds evict each other on every alternation.
19. **All test scratch lands under `target/tmp`**, via `CARGO_TARGET_TMPDIR` or
    the workspace-relative fallback — never the system temp directory, which is
    commonly a RAM-backed tmpfs. → `testing.md`
20. ⚠️ **Reap scratch by age at suite start.** `Drop` does not survive SIGKILL,
    and the test runner kills on timeout, which is exactly how orphans
    accumulate.

## CI

21. **`CARGO_INCREMENTAL=0` in CI.** Incremental compilation is pure overhead
    where every build starts clean.
22. **Hooks and CI call the same scripts.** ⚠️ A check implemented twice drifts,
    and the version that matters is whichever one was not run.
23. **No gate may be triggered by a pull request.** Commits go directly to
    `main`; a PR-triggered gate silently never runs. → `check-drift.sh`
24. **Pre-commit may scope to changed crates; CI must not.** Pre-commit is
    feedback, CI is truth.

## What has no gate

**Whether a dependency is worth its build cost.** Dependency count dominates
compile time more than any profile setting, but "is this crate worth 400 lines
of transitive build" is a judgement. `cargo build --timings` shows the critical
path; the decision is review's.

## See also

- Build configuration evidence: [docs/researches/18](../../researches/18-rust-performance-methodology.md) §3
- Artifacts, cross-compilation, glibc floor: [portability.md](portability.md)
