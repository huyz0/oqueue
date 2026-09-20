# 0054. Writer nonce epochs come from coordinator fences

Status: accepted
Date: 2026-09-20
Requirements: FR-42

## Context

`M8.2` makes the nonce triple injective only after the writer epoch is
distinct across incarnations. A process id, wall-clock timestamp, or configured
writer name can repeat after a restart. Reusing one under a live DEK repeats
every AES-GCM nonce that writer mints.

The coordinator already exposes `CoordinatorEpoch` as the fence readers use to
reject state from a previous incarnation. It is the existing monotonic
incarnation vocabulary available to the composer; adding a second restart
counter would create two fences that could drift apart.

## Decision

`oqueue-core::WriterEpoch` is constructed only from `CoordinatorEpoch`, and
`NonceMinter::new` accepts `WriterEpoch` rather than a raw integer. A composer
must obtain a new coordinator-fenced epoch when it restarts or takes over; the
nonce API no longer permits it to substitute a process id, timestamp, or
configuration value. The 40-bit nonce field is still range-checked, and a
coordinator epoch that exceeds it is refused rather than truncated.

The coordinator remains responsible for advancing its fence. This decision
does not make leadership election or durable term allocation part of
`oqueue-core`; it closes the accidental raw-integer path at the nonce seam and
uses the fence already owned by the coordinator.

## Alternatives considered

- **Use `WriterId::mint`'s process id, timestamp, and local counter.**
  Rejected: the identity is useful for object names but is not a durable fence;
  pid reuse and timestamp collisions across restarts remain possible.
- **Add a second durable writer counter beside the coordinator epoch.**
  Rejected: two independent incarnation values can disagree about which node is
  current, recreating the fencing problem under a different name.
- **Keep `NonceMinter::new(u64)` and document the caller obligation.**
  Rejected: every caller could still accidentally pass a repeatable value, and
  the type would claim a safety property while accepting its known failure
  mode.

## Consequences

The nonce seam is harder to misuse and the restart property is testable with
two coordinator epochs without clock or storage I/O. Composers must already
have a valid coordinator fence before they can create a minter; a missing or
reused fence must refuse sealing rather than fall back to an unsafe epoch.
