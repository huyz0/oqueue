# 0056. Key-domain sealing is a broker composition seam

Status: accepted
Date: 2026-09-20
Requirements: FR-42, NFR-14

## Context

M8.6 separates a produce flush by the topic's catalog key domain. A customer
domain must not enter the ordinary `BundleBuilder::push` path: that path writes
plaintext, while M8.12 makes the reader require a sealed region for customer
metadata. The broker also cannot name a concrete KMS provider or nonce allocator;
those choices belong to the composition root.

## Decision

`oqueue-broker` exposes `RegionSealer`, an async seam returning an owned sealed
region. The produce planner calls it for customer domains and sends the result
through `BundleBuilder::push_sealed`; default-domain records retain the existing
`push` path and bytes. `Cluster` starts with `RejectingRegionSealer`, which
returns `EncryptionDisabled` rather than persisting customer plaintext. A
composition root with a configured KMS-backed sealer installs it through
`Cluster::with_region_sealer`.

The planner keeps one bundle and one flush per key domain. A mixed request
therefore cannot put default and customer regions in one object, while the
default-only object remains byte-identical.

## Alternatives considered

- **Write customer records through the default builder until KMS wiring exists.**
  Rejected: the domain-aware reader would refuse those bytes, and a plaintext
  fallback violates the encryption boundary.
- **Put a concrete KMS provider in `oqueue-broker`.** Rejected: the broker is
  a composer over seams, and provider choice belongs to `bin/oqueue`.
- **Seal every region regardless of domain.** Rejected: it changes the default
  object format and cost for the common path, violating NFR-14.

## Consequences

The routing and fail-closed behavior are available in the broker without
coupling it to a provider. The shipped composition currently has no KMS
configuration and therefore refuses customer-domain produces; a deployment
must supply the sealer before enabling customer-key topics. The boundary is
explicit and safe, and the M8.6 tests use a sealed seam implementation to
verify the persisted object split.
