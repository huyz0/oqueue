---
title: "Portability"
description: >
  Read when a change is OS- or architecture-specific, when touching release artifacts, or when a test behaves differently on macOS.
tags: [delivery, targets, glibc, cross-arch, artifacts]
applies_to: ["scripts/*", ".github/workflows/*", "*/build.rs", ".agents/*", ".claude/*", "AGENTS.md", "CLAUDE.md", "*/SKILL.md"]
---

# Portability

Two different problems that get conflated. Evidence in
[docs/researches/20](../../researches/20-build-and-release-portability.md) and
[19](../../researches/19-workspace-engineering.md) §7.

| | Question | Fails as |
|---|---|---|
| **Build portability** | Can a person or runner *produce* the artifact? | "works on my machine" |
| **Runtime portability** | Will the artifact *run* where it is deployed? | `GLIBC_2.38 not found`, `SIGILL` |

⚠️ Mixing them produces the classic mistake: statically linking everything (a
runtime fix) to solve a build problem, at large and unnecessary cost.

## Targets

1. **Linux x86-64 and aarch64 are both first-class release targets.** aarch64 is
   not an afterthought — Graviton is a plausible deployment for a cost-sensitive
   broker, and LSE atomics are worth more there than any vector kernel.
2. **macOS is a development platform, not a release target.** It must build and
   run the fast test tiers. It produces no release artifact, which means no
   codesigning, no notarization, and no universal binaries.
3. **Windows is not a target.** WSL2 is the supported path for development on
   Windows.
4. **One binary per `(os, arch)` at baseline `x86-64-v2` / aarch64
   `+lse,+crc`**, with everything above the baseline behind runtime dispatch.
   ⚠️ `x86-64-v4` is not a candidate: it `SIGILL`s on every Intel consumer chip
   since Alder Lake and on Zen 2/3 cloud instances.

## Runtime portability

5. **The glibc floor is pinned at 2.28** and declared in CI configuration, not
   inherited from whatever the build host happens to run. ⚠️ glibc symbol
   versioning is forward-compatible only: a binary built on a current
   rolling-release host fails to start on essentially every current LTS server,
   and nothing reveals this until deployment. → `cargo-zigbuild` with the floor
   as a target suffix
6. **A musl artifact must set a non-default global allocator.** ⚠️ musl's
   default allocator costs **10–40× under concurrency**, and a broker is the
   most allocator-contended workload there is. Without one, the binary
   benchmarks fine single-threaded and collapses under load — the worst failure
   shape available. → a CI assertion that the musl build has one
7. **Anything above the ISA baseline goes through runtime dispatch**, resolved
   once into function pointers at startup. → `performance.md` rule 9
8. **Log the selected SIMD backend at startup, with an env override.** It is how
   a backend is benchmarked, how a user reproduces a corruption report, and how
   a benchmark asserts its dispatch path.

## Build portability

9. **Native compilation over cross-compilation.** Free arm64 runners removed the
   historical reason to cross-compile, and native builds avoid sysroot and
   native-dependency failure modes entirely. `cross` remains correct for
   *cross-testing* a target with no runner.
10. **A gate that needs a missing tool skips with a named remedy; it does not
    fail.** ⚠️ A missing tool must never be indistinguishable from a failing
    check — otherwise an absent `protoc` surfaces as "clippy failed" four lines
    into a build-script error, and the time goes to debugging the wrong thing.
11. **Avoid dependencies that add a host toolchain requirement.** → `build.md`
    rules 3–4

## Test portability

12. **Gate tests by capability, never by `#[cfg(target_os)]`.** A
    `cfg(target_os)` test silently does not exist on half your machines and
    nothing reports its absence. Use `#[ignore]` plus `--run-ignored all` in the
    tier that has the capability. ⚠️ Not a feature flag: a feature-gated test
    does not compile unless the feature is on, so it rots invisibly.
13. **Never assert on wall-clock durations.** Schedulers differ across platforms
    and the assertion is a coin flip.
14. **Assume a case-insensitive filesystem.** A test creating `Foo` and `foo`
    passes on Linux and collides on macOS.
15. **Raise the file-descriptor limit explicitly in the test harness.** macOS
    defaults to 256, so a broker holding many segment files hits EMFILE locally
    and never in CI.
16. **Never assume the temp directory path round-trips.** On macOS `/tmp` is a
    symlink, so `canonicalize()` breaks path-equality assertions. → all scratch
    under `target/tmp` anyway (`build.md` rule 19)
17. ⚠️ **Assert the container image architecture in containerized tests.** The
    test-container tooling silently falls back to x86-64 images when no arm64
    variant exists, running them under emulation at roughly 85% slower — which
    presents as flaky tests, not as a portability problem.

## Cross-architecture correctness

18. ⚠️ **ARM CI is a codegen and SIMD-dispatch gate, not the atomics gate.**
    aarch64 is weakly ordered where x86-64 is TSO, so a missing
    `Acquire`/`Release` *can* surface there — but weak-memory bugs are
    probabilistic and rarely reproduce under ordinary test load. Running tests
    on ARM is weak evidence.
19. **`loom` is the atomics gate.** It permutes concurrent executions
    exhaustively under the C11 memory model, deterministically, on any
    architecture. → a `loom` job on the lock-free structures
20. **Keep lock-free structures small and separable**, so `loom` can explore
    them. This is an argument for confining them to `oqueue-buf` and
    `oqueue-core`.

## What has no gate

**Whether the target list is still right.** It is a product decision that should
be revisited when deployment evidence arrives, not an invariant.

## See also

- Artifacts, glibc, cross-compilation: [docs/researches/20](../../researches/20-build-and-release-portability.md)
- Test tiers and the CI matrix: [docs/researches/19](../../researches/19-workspace-engineering.md) §7
