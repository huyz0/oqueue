# ADR-0069: Functional binary roles

Status: accepted
Date: 2026-09-22

## Context

The single binary already composes the coordinator and data-plane in one
process, but a role label alone would not change deployment responsibility.
Operators need to start a coordinator-only or data-plane-only process from the
same artifact and have contradictory protocol traffic rejected locally.

## Decision

Select `coordinator`, `data-plane`, or `combined` with `OQUEUE_ROLE`, defaulting
to `combined`. The selected role is injected into the broker cluster and
dispatcher. Coordinator mode admits coordination, group, admin, and config
APIs but rejects data reads and writes. Data-plane mode admits metadata, fetch,
and list-offsets reads but rejects mutation, admin/config, and group
coordination APIs. Combined mode admits both sets. ApiVersions and SASL setup
remain available in every mode so clients can discover and authenticate before
the role-specific request is evaluated.

Unknown or undecodable role values fail startup. The process remains one
artifact and the role check is at the dispatcher boundary, before request-body
decoding, so an unsupported responsibility cannot reach a handler by accident.
The role label is also carried by `Cluster::health()` for supervisors.

## Consequences

Role smoke tests prove an actual request is accepted or rejected by each mode,
rather than only checking startup text. A data-plane process runs the
read-only metadata replay needed to hydrate its index, but does not renew a
lease or run retention. Produce and producer initialization remain
combined-mode responsibilities until a remote coordinator transport is
introduced, rather than pretending that two local coordinators can safely
share a store.
