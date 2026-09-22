# 0070. Linux release glibc floor

Status: accepted
Date: 2026-09-22
Requirements: NFR-40, NFR-41, NFR-42

## Context

M13 must turn the workspace into release artifacts that run on the supported
Linux deployment platforms. glibc symbol versions are forward-compatible only:
an artifact built against a newer glibc can fail before startup on an older
supported distribution. The product requirement already says that binaries
run on glibc 2.28 and newer, and the portability standard already names 2.28
as the pinned floor. M13 therefore needs an explicit decision record before
the build matrix is implemented, so the release configuration cannot drift to
the host's libc by accident.

## Decision

Release Linux artifacts target a **glibc 2.28 floor**. The release build uses
an explicit target suffix or equivalent pinned toolchain configuration, and the
oldest-supported-distribution smoke test runs against that floor. The floor is
part of the artifact matrix and is not inherited from the runner image.

## Alternatives considered

- **glibc 2.34.** Rejected because it would abandon the RHEL 8 compatibility
  named by the existing portability research and would contradict agreed
  NFR-41 without a product change.
- **Build on the CI host's native glibc.** Rejected because the host image is
  an implementation detail and can silently raise the symbol-version floor.
- **Use musl as the default Linux release target.** Rejected for the baseline
  artifact because musl changes allocator and runtime behavior; M13 evaluates
  musl separately rather than treating it as a substitute for the glibc
  requirement.

## Consequences

The release workflow needs a pinned zig/cargo-zigbuild path or an equivalent
toolchain that can produce the 2.28 target, plus a smoke image at that floor.
Some newer platform APIs cannot be assumed merely because they exist on the
runner. The decision preserves the supported deployment range and makes a
future floor increase an explicit requirement and ADR change.
