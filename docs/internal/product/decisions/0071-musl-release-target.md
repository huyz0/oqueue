# 0071. Linux musl release target

Status: accepted
Date: 2026-09-22
Requirements: NFR-40, NFR-42

## Context

M13 must settle build question #37 before it promises a release matrix. The
question was initially phrased as whether `object_store` cross-compiles to
`aarch64-unknown-linux-musl`, but M1.42 showed that the existing aarch64 GNU
check was already red when the `ring` dependency needed a target C compiler.
The repository now has free native ARM runners, while its portability research
records a 10–40× concurrent allocator penalty for musl's default allocator.

## Decision

M13 ships the native `aarch64-unknown-linux-gnu` artifact with the glibc 2.28
floor. It does **not** ship an `aarch64-unknown-linux-musl` artifact and does
not promise a musl cross-compilation path. The native ARM release job restores
coverage of `oqueue-store` with its full test target set.

The answer to build question #37 is therefore: no, musl is not a supported
release target in this artifact matrix; the release path avoids the cross
toolchain question by building the supported ARM target natively.

## Alternatives considered

- **Ship musl with Rust's default allocator.** Rejected because the documented
  concurrent penalty would make the artifact violate the performance intent of
  the broker without a measured mitigation.
- **Adopt musl with a non-default allocator now.** Rejected because that would
  require a new allocator decision, concurrency measurements, and a second
  release matrix before any product requirement calls for a static artifact.
- **Cross-compile the supported ARM artifact.** Rejected because native ARM
  runners are available and avoid the target C toolchain and dependency
  failure mode that already removed `oqueue-store` from the old cross-check.

## Consequences

The release matrix is smaller and its ARM coverage is stronger: the binary and
`oqueue-store` tests run on the architecture that ships. A static musl image is
not available from M13, and reopening that choice requires allocator evidence
and a new release decision rather than silently adding a target.
