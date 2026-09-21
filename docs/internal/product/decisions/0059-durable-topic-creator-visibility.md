# 0059. Durable topic creator visibility

Status: accepted
Date: 2026-09-21
Requirements: FR-40, FR-53, NFR-12

## Context

`CreateTopics` must leave the creating principal able to use the topic after a
broker restart. `TopicGrants` is an in-memory forward index, so updating it
only in the request handler loses that visibility when the process exits.
Loading the whole catalog at startup would violate the catalog's bounded,
looked-up architecture. The existing `TopicCatalog` seam also cannot report
whether a create won its conditional write, allowing an idempotent retry to be
mistaken for a new creator.

## Decision

1. Store an optional creator principal in the durable topic entry. Existing
   entries and topics created by non-admin paths keep no creator.
2. Maintain create-only owner-index objects beneath the catalog prefix,
   keyed by principal and topic name. `TopicCatalog::list_owned` pages this
   index, and the broker hydrates only the authenticated principal's grants,
   lazily after authentication. A per-principal cancellation-safe async cell
   makes the catalog walk single-flight and shares its successful result across
   connections. The request path remains bounded by that principal's own
   owned-topic set rather than the whole catalog; failed or cancelled loads
   remain retryable.
3. Extend the catalog seam with an owned-create operation that returns whether
   the caller won the conditional create. A losing caller returns
   `TOPIC_ALREADY_EXISTS` and never receives the existing owner's visibility.
4. Owner-index objects are written before the topic entry. A crash can leave a
   stale owner index, but `list_owned` verifies each candidate against the
   durable topic entry before granting visibility; a crash cannot hide a
   successfully published creator association.

## Alternatives considered

- **Rebuild every grant by scanning the catalog at startup.** Rejected: it
  makes startup proportional to all topics and defeats the reason the catalog
  is a paged lookup seam at the project's scale target.
- **Treat administrative create authority as topic visibility.** Rejected:
  mutation authority and tenant visibility are separate decisions under
  ADR-0058, and this would expose every topic to every topic administrator.
- **Keep creator visibility only in `TopicGrants`.** Rejected: it loses a
  successful creator's access after restart and makes the restart guarantee
  false.
- **Probe existence before the existing idempotent create call.** Rejected:
  the probe races with concurrent creators and cannot identify which caller
  won the conditional write.

## Consequences

Creator visibility survives restart without a whole-catalog scan, and a
duplicate create cannot grant cross-tenant access. The catalog format and
contract gain durable ownership metadata, so implementations and fakes must
evolve together. Stale owner-index objects are harmless and remain for the
future deletion/tombstone task to clean up.
