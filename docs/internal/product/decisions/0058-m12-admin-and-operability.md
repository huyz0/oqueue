# 0058. M12 admin authorization and operability boundaries

Status: accepted
Date: 2026-09-21
Requirements: FR-40, FR-44, FR-45, FR-50, FR-52, FR-53, NFR-12

## Context

M12 adds the first live administrative API to a broker whose catalog is durable
and looked up through a bounded page seam. It also receives the five group APIs
that M4 could not scope to a principal, and must expose enough operational
evidence to distinguish a stalled partition from a storage, sequencing,
coordination, compaction, or key-domain failure. The existing `TopicGrants`
index only answers visibility; it is not an authorization model for mutation or
group ownership.

## Decision

1. **Kafka listing remains principal-scoped.** M12 does not add an unrestricted
   cluster-wide enumeration path. `Metadata` and the Kafka admin topic listing
   expose only the authenticated principal's granted topics. A cross-tenant
   view is refused rather than routed through `Metadata`; future maintenance
   enumeration needs its own explicitly bounded seam.
2. **Authorization is deny-by-default once credentials are configured.** Topic
   visibility, topic mutation, and configuration are separate decisions. M12
   adds value indexes for topic administration and `GroupGrants` for group
   ownership, reusing the existing dispatcher decision point. A new grants
   value type is not a new `oqueue-core` trait method; any trait change is a
   separate contract-change task with its ADR, fakes, implementations, and
   callers in one commit.
3. **Deletion is a durable tombstone, and names are never reused in v1.** A
   successful delete makes the topic unavailable immediately, revokes its live
   grants and cache entries, and records a tombstone that survives restart.
   The derived topic UUID is therefore never reused, so a stale client cannot
   address replacement data. Physical object cleanup remains the existing GC
   responsibility after the reader-safety delay.
4. **Configuration is explicit and bounded.** M12 supports only settings that
   have a live owner and validation rule; unsupported keys are refused rather
   than accepted as no-ops. Config updates are durable before acknowledgement,
   and incremental operations share the same invariant validation as full
   replacement. Secrets and key references never appear in responses or
   telemetry.
5. **Telemetry is aggregate by default and scoped on demand.** Routine metric
   labels are bounded and do not create one permanent series per catalog topic
   or partition. A named diagnostic request may scope a bounded view to one
   topic and partition. Every M12 diagnostic scenario has a fault, expected
   signals, a query/runbook, and a distinguishing observation.
6. **Roles are real composition modes.** The single artifact accepts
   `coordinator`, `data-plane`, and `combined` configurations. A role starts
   only the responsibilities it owns; a role label without a changed
   responsibility is not a valid implementation. Separate roles communicate
   through the existing seams, while `combined` preserves the current local
   composition.

## Alternatives considered

- **Global topic listing through the existing catalog path.** Rejected because
  it makes a tenant-facing request cost proportional to the whole catalog and
  recreates the cross-tenant isolation hole M9 deferred.
- **Treating visibility as mutation authority.** Rejected because granting a
  consumer read access must not let it delete or reconfigure a topic.
- **Immediate name reuse after deletion.** Rejected because the current
  name-derived UUID would make a stale UUID indistinguishable from a new topic
  and could expose replacement data.
- **Accepting arbitrary Kafka settings and ignoring unsupported ones.**
  Rejected because an operator would receive a successful response for a
  setting the broker did not enforce.
- **Per-partition permanent telemetry series.** Rejected because the catalog
  target is 1M–100M topics and the operational surface must not become the
  metadata cache it is meant to observe.
- **One process mode with a role label.** Rejected because FR-50 verifies
  responsibilities, not a string in startup output.

## Consequences

The first M12 implementation can be tested through existing principal and
catalog seams, while deletion and durable configuration require explicit
contract work. Operators get a bounded, tenant-safe baseline and a clear
extension point for maintenance enumeration. Name reuse is unavailable in v1,
and old topic tombstones consume durable catalog metadata until a later
compaction policy removes them without making identifiers reusable.
