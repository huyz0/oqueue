# Release artifact matrix

This document is the release-target contract for M13. The shipped Linux
artifacts are built on runners with the same architecture as the artifact:
cross compilation is not the release path. A cross toolchain may still be
used for a diagnostic build, but it cannot produce a shipped artifact.

| Artifact tier | Runner | Rust target | Release artifact | Purpose |
|---|---|---|---|---|
| Linux x86_64 | `ubuntu-latest` | `x86_64-unknown-linux-gnu` | yes | primary x86_64 deployment artifact |
| Linux aarch64 | `ubuntu-24.04-arm` | `aarch64-unknown-linux-gnu` | yes | primary Graviton/aarch64 deployment artifact |
| macOS | `macos-latest` | host target | no | fast portability and test validation |
| Windows | no release runner | none | no | development and WSL smoke coverage only |

The two Linux target triples are pinned in `rust-toolchain.toml`. The release
workflow keeps one native job per shipped architecture. The Linux artifact
jobs are deliberately separate rather than a cross-compiled matrix so that
architecture-specific code generation and linker behavior run in the
environment that will execute the binary. Startup and request checks are
added to the retained artifacts by the later M13 smoke tasks; this target
matrix does not claim those checks by itself.

The glibc floor, ISA baseline, allocator mode, artifact naming, signing, and
container packaging are separate M13 contracts. They must not be inferred
from the runner image or from this target matrix; the later M13 tasks record
and verify each one explicitly.

## glibc build floor

The Linux release command is `cargo zigbuild --locked --release` with the
target suffix `.2.28`, for example
`--target x86_64-unknown-linux-gnu.2.28`. `scripts/release-build.sh` pins
`cargo-zigbuild` 0.20.1 and Zig 0.14.1, and refuses to build when either tool
reports a different version. The suffix selects the glibc 2.28 sysroot; the
runner's libc is not the release link target.

Release code generation is pinned by target: x86_64 uses
`-C target-cpu=x86-64-v2`, while aarch64 uses `-C target-feature=+lse,+crc`.
The command also passes `--no-default-features`; the shipping path excludes
the optional `heap-profiling` feature. A profiling build is not a release
artifact and, if requested for diagnostics, must set
`JEMALLOC_SYS_WITH_LG_PAGE=16` as required by ADR-0007.

The default build also runs in `docker/release-default.Dockerfile`, which
contains cargo, the pinned Rust toolchain, and a C compiler but no CMake or Go.
`scripts/release-clean-build.sh` checks that boundary before running the
locked, default-feature-free release build. FIPS tooling belongs to a separate
builder and cannot leak into this job.
