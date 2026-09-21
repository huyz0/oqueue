# 0060. Durable, non-reusable topic tombstones

Status: accepted
Date: 2026-09-21
Requirements: FR-40, FR-53

## Context

The topic UUID is currently derived from the topic name. That is useful for
creation without coordination, but it means deleting a live catalog row and
then accepting the same name would make a stale UUID indistinguishable from a
replacement topic. The broker also caches resolved topics and hydrates creator
visibility from durable owner indexes, so deletion must invalidate all three
views consistently.

## Decision

1. `TopicCatalog` gains one deletion method that accepts a name and optional
   expected UUID. Implementations return a typed outcome for deleted,
   already-tombstoned, missing, and UUID-mismatch requests.
2. Deletion writes a create-only tombstone containing the topic UUID before
   removing live catalog indexes. Lookups, id lookups, and listings consult the
   tombstone before any live entry, so a partially cleaned deletion is already
   absent from serving and listing. The tombstone is never removed in v1.
3. A tombstoned name cannot be created again. The permanent tombstone, rather
   than the name-derived UUID, is the reservation that prevents reuse.
4. Each broker evicts its local resolved-topic and creator-load caches and
   revokes the deleted name from its live `TopicGrants` index after the durable
   tombstone is acknowledged. Because cache invalidation is not broadcast,
   resolved-topic entries have a short lease and creator-grant hydration
   revalidates the durable catalog before restoring a name. This bounds the
   cross-broker stale-serving window without adding an object-store call to
   every warm produce request. Data/log object cleanup remains the existing GC
   responsibility; this operation removes only catalog indexes.
5. DeleteTopics serves protocol versions 1–6. Versions 1–5 are name-based;
   version 6 additionally carries a nullable name and UUID. A supplied UUID
   must match the durable entry/tombstone, otherwise the request is refused as
   stale and cannot affect another topic.

## Alternatives considered

- **Delete the row and allow name reuse.** Rejected: the derived UUID would
  address replacement data through a stale client and violate tenant safety.
- **Keep deletion only in the broker cache.** Rejected: restart would
  resurrect the topic and another node would continue serving it.
- **Delete the tombstone during physical cleanup.** Rejected: cleanup timing
  must not change identifier safety; the tombstone is the durable history that
  makes non-reuse true.
- **Check UUIDs only in the DeleteTopics handler.** Rejected: direct catalog
  callers and future admin paths could bypass the check; the seam owns the
  invariant.

## Consequences

Deleted names consume a small durable catalog record for the lifetime of the
v1 namespace. Index cleanup may be retried independently after the tombstone
is visible, and old log objects remain governed by GC's reader-safety delay.
