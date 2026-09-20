# 0057. Compaction re-sealing is a composition seam

Status: accepted
Date: 2026-09-21
Requirements: FR-41, FR-42

## Context

M8.8 makes compaction safe for customer key domains. Compaction reads an
already-sealed input region and writes it into a new object, so the region's
position in the output object can change. The region associated data includes
that position. Copying the old ciphertext and footer would therefore either
make the output unreadable or preserve metadata that was authenticated for a
different object layout. Compaction also must not name a concrete KMS provider,
and a revoked customer key must stop that domain without making the queue retry
forever.

## Decision

`oqueue-core` exposes `RegionReSealer` as the composition seam used by
`oqueue-compact`. For each
customer-domain input region, compaction supplies the authoritative
`KeyDomain`, topic, partition, input footer region, ciphertext, and explicit
output-region index. The implementation behind the seam opens and re-seals the
region with fresh associated data and a fresh nonce, then returns the new
ciphertext and envelope. Compaction writes that result through
`BundleStream::push_sealed`, the shared durable-format encoder. The seam
propagates `Error::KeyRevoked` unchanged; the caller can record the failed
domain and continue another compaction domain.

The existing `merge` API remains the default-domain path. Customer-domain
compaction must opt into `merge_with_resealer`; if a customer domain has no
re-sealer, compaction fails closed rather than copying bytes or writing
plaintext.

## Alternatives considered

- **Copy the input ciphertext and envelope.** Rejected: output region indices
  are authenticated associated data, so two input objects can make identical
  input indices collide in one output object.
- **Put a concrete KMS provider in `oqueue-compact`.** Rejected: the compact
  crate depends on core and must remain independent of provider and cipher
  implementations; the composition root owns those choices.
- **Decrypt and re-seal inside the compactor.** Rejected: it would move key
  material and cryptographic policy into the compaction crate and violate the
  existing seam boundary.

## Consequences

Customer compaction has an explicit, testable security boundary and cannot
silently downgrade to plaintext or stale ciphertext. Re-sealing may do more
cryptographic work than copying, and a key revocation makes that domain's
operation fail, but those are the required safety outcomes. The default-domain
path keeps its existing behavior and format.
