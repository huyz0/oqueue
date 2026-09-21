# ADR-0068: Role-aware health and readiness

Status: accepted
Date: 2026-09-22

## Context

The broker already has authoritative signals for replay readiness, the
coordinator/group-transition task, and bounded dependency metrics. Returning a
single boolean would hide the distinction between a node that must not receive
traffic and one that can still route requests while a dependency is degraded.
KMS failure is also scoped to a customer-key domain and must not make unrelated
domains appear unavailable.

## Decision

Expose `Cluster::health()` as a role-tagged, in-process snapshot. Replay still
in progress or a stopped coordinator task is `NotReady`. A live task with an
observed object-storage, coordinator, or KMS-domain failure is `Degraded`.
Otherwise the node is `Ready`. KMS observations are counters and never expose
customer key identifiers; they therefore degrade the node without claiming
that all domains are down. The snapshot includes the role, replay/task state,
and the bounded failure counters needed by an operator or future HTTP probe.

The initial role is `Combined`; `with_role` provides the explicit seam for the
functional coordinator/data-plane role wiring in M12.14. Health is
observational and does not issue a storage probe, because a probe would turn a
diagnostic read into an unbounded dependency operation and would conflate
readiness with an unrelated request path.

## Consequences

Supervisors can distinguish startup/replay failure from degraded routing, and
KMS failures cannot falsely take unrelated key domains out of service. The
current API is usable without an HTTP server and can be attached to one when
the binary role work lands. A zero failure counter means only that no failure
has been observed since construction; it is not a fresh liveness proof.
