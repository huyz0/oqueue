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

## Musl decision and native ARM coverage

M13.6 rejects `aarch64-unknown-linux-musl` as a shipped release artifact. The
default musl allocator is documented at 10–40× worse under concurrent load for
this workload, and adopting musl would therefore require a separately measured
non-default allocator and a second release matrix. The supported ARM artifact
is native `aarch64-unknown-linux-gnu` with the pinned glibc floor; the native
ARM release job runs `cargo test --locked -p oqueue-store --all-targets` so the
crate previously excluded from cross-target checks is covered on the
architecture that ships it. The decision and its alternatives are recorded in
[ADR-0071](../internal/product/decisions/0071-musl-release-target.md).

`aarch64-unknown-linux-musl` is therefore **not a shipped release artifact**.
It may be evaluated again if a product requirement or measured allocator work
justifies reopening the decision.

## Oldest-distribution smoke

The release smoke image uses Rocky Linux 8, whose glibc is the 2.28 floor
committed by NFR-41. It builds the default release artifact inside that image
and runs the real binary through the role smoke harness, including a Kafka
ApiVersions request over the listening socket. A successful link on the host
does not satisfy this leg.

## CI matrix

The release workflow builds and tests the Linux x86_64 and native aarch64
artifacts. The push-triggered OS smoke workflow runs the portable shell smoke
tier on Linux, macOS, and Windows, and runs the fast workspace test tier on
Linux and macOS. macOS is a development platform only and produces no release
artifact; Windows uses the portable shell tier because WSL2 is the supported
development path.

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

## FIPS variant and object-format differential

The FIPS artifact is built only by `docker/release-fips.Dockerfile` through
`scripts/release-fips-build.sh`. That builder adds CMake and Go for
`aws-lc-rs`; the default builder does not. The composition-root command is
`cargo build --locked --release --no-default-features --features fips -p oqueue`,
which selects AWS-LC for both region AEAD and Rustls and runs the binary's
runtime `fips_mode_enabled()` assertion at startup. The TLS seams also verify
the installed process provider's `CryptoProvider::fips()` result and fail
closed if another provider was installed first.

The FIPS and default builds share the region format, not an implementation
format: both use the same AES-256-GCM algorithm code, nonce, associated-data
encoding, tag, and ciphertext layout. `crates/oqueue-crypto/tests/it/region.rs`
pins one ciphertext literal and opens that literal; the test is run in both
feature configurations. A change that makes either provider produce a
different region representation fails the pinned-vector test.
