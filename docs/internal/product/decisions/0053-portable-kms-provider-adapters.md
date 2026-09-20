# 0053. Portable KMS provider adapters stop at encrypt and decrypt

Status: accepted
Date: 2026-09-20
Requirements: FR-41, FR-42

## Context

`M8.7` needs both AWS KMS and GCP Cloud KMS behind the `oqueue-core::KeyProvider`
seam. The providers must wrap and unwrap locally generated DEKs, while the
milestone has no cloud credentials and must not add a vendor SDK or network I/O
to `oqueue-crypto`. The two APIs do not have identical operation sets: AWS has
`GenerateDataKey`, but GCP Cloud KMS does not expose that equivalent.

## Decision

`oqueue-crypto` exposes one small API trait for each provider, each containing
only `encrypt` and `decrypt`. `AwsKmsProvider` and `GcpKmsProvider` adapt those
traits to the same `KeyProvider` contract. The adapters receive an already
generated DEK and return a wrapped or unwrapped value; they never generate key
material and never own a network client.

M8 supplies in-process simulations of each API and runs one round-trip test
body through `dyn KeyProvider`. Real SDK implementations and cloud round trips
remain M15's evidence, as recorded in ADR-0050 point 6.

## Alternatives considered

- **Expose the union of both vendor APIs, including `GenerateDataKey`.**
  Rejected: it would make the portable seam depend on an AWS-only operation and
  would force GCP implementations to invent or reject a method they cannot
  provide.
- **Put vendor SDK clients directly in `oqueue-crypto`.** Rejected: it would
  add network and credential concerns to library logic, make the M8 tests
  depend on cloud services, and couple the crate to SDK build requirements.
- **Use one generic provider trait with a vendor enum.** Rejected: vendor
  request shapes and error mapping would leak into the common seam; separate
  narrow API traits keep each adapter honest while `KeyProvider` remains the
  shared contract.

## Consequences

The common seam can test AWS and GCP portability without a cloud dependency,
and DEK generation stays in the local crypto path. A composition root must
later supply real SDK-backed implementations, and M15 must verify their real
round trips. Provider-specific features outside encrypt/decrypt require a new
decision rather than silently widening the portable contract.
