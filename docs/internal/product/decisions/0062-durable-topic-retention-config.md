# 0062. Durable topic retention configuration

Status: accepted
Date: 2026-09-21
Requirements: FR-40, FR-53

## Context

`M12.5` exposed the authoritative default for `retention.ms`, but the
configuration write APIs need a durable setting that survives restart and is
visible to every broker serving the topic. The topic catalog already owns
durable existence, creator ownership, key-domain metadata, and deletion
tombstones. Its existing four entry formats are deployed metadata shapes; a
configuration update must not make an older entry undecodable or silently
replace unrelated topic metadata.

Retention is also a live owner: the expiry heap must use an override for new
and idle partitions, while a reset must return to the authoritative default.
The write must be acknowledged only after the durable configuration value is
stored.

## Decision

Add `retention.ms` to the `TopicCatalog` contract as a topic-scoped optional
value. `None` means the effective value is
`oqueue_compact::DEFAULT_RETENTION_MS`; `Some(value)` is a validated override.
The object-store catalog stores the override in a separate config object at
`<shard-prefix>/config/<lowercase-hex-topic-name>`. Its body is exactly one
format byte (`1`) followed by the signed 64-bit value in big-endian order.
Missing objects mean default; an unknown format or wrong length is a
malformed metadata error. Reset deletes the object. Topic lookup hides
configuration for missing or tombstoned topics, and normal topic deletion
deletes the config object with the live indexes. A tombstone always wins over
a stale config write racing with deletion, so such a write can never become
visible or affect a replacement (names remain permanently reserved). The fake
catalog keeps the same semantics in memory.

The write operation returns a distinct `Applied` or `Missing` outcome. The
stored implementation rechecks liveness after its conditional write, so a
tombstone racing the update cannot be reported as a successful configuration
change. Handlers reject `Missing` and only acknowledge an `Applied` update.

The broker validates `retention.ms` before calling the catalog: values must be
at least `DELETION_DELAY_MS`, preserving the retention lower bound named by
M12's safety requirement, and must fit the Kafka `LONG` value accepted by the
wire API. Full `AlterConfigs` treats
the supplied set as replacement: omitting `retention.ms` resets it. Null also
resets it. `IncrementalAlterConfigs` supports `SET` and `DELETE`; `APPEND`
and `SUBTRACT` are invalid for this scalar setting. Any unsupported key or
resource fails the resource without changing it. The handler serializes
topic lifecycle updates, so validation and the durable catalog write complete
before a success response.

The durable update is followed, before the success response, by an ordered
`TopicRetentionChanged` metadata-log event. The coordinator is the cross-node
serialization point; retention followers receive the event through their
existing delta stream and rebuild only that topic's armed deadlines before the
next round. The follower confirms the current catalog value when applying the
event, so an out-of-order durable write cannot install an older value. A lagged
subscriber rebuilds from the catalog while replaying the log. Startup and a
lag rebuild load values only for topics encountered while rebuilding the
follower index; ordinary rounds remain heap-bounded and do not scan all armed
topics. A tombstone therefore remains authoritative even if a stale config
event arrives after deletion.

The catalog write and event append are one logical update from the client's
perspective: the handler acknowledges success only after both succeed. The
catalog write is authoritative and deliberately is not compensated if event
journaling fails, because an unconditional rollback could overwrite a newer
last-writer-wins update from another node; the handler returns the dependency
error and the fixed-budget reconciliation below repairs the live retention
heap. A lost response after both operations is safe: a retry writes the same
catalog value and emits another event, and duplicate events are harmless
because the event handler reads the current catalog value before changing a
deadline.

In addition to event delivery, each retention round probes at most a fixed
`CONFIG_RECONCILIATION_BATCH` of armed topics in a rotating order. This is a
bounded repair path for a dropped event or a failed append, not a scan: it
eventually observes every armed topic without making one round's work grow
with the catalog or the number of armed partitions.

`TopicRetentionChanged` is a replay-safe, non-data metadata record. It gets a
new segment tag; `Allocator` advances only the version line, and
`IndexState` ignores it because it names no partition bytes. All current
readers must understand it. An older binary that sees the new tag refuses the
metadata segment as malformed rather than guessing, so mixed binaries fail
closed instead of silently losing a configuration event.

## Alternatives considered

- **Add fields to the existing topic-entry formats.** Rejected because it
  couples a mutable setting to four established metadata encodings and makes
  every old entry require a format migration or ambiguous trailing-field
  decoding.
- **Keep configuration only in the broker or in a process-local map.**
  Rejected because restart and another serving node would lose the setting,
  violating the durable catalog boundary.
- **Let the retention task read configuration only when a partition commits or
  scan every armed topic each round.** Rejected because the first misses idle
  partitions and the second changes `M5`'s expiry-heap work from
  expiring-partition bounded to all-armed-partition work. The ordered metadata
  event handles the live-update event across nodes.
- **Accept `APPEND` and `SUBTRACT` as aliases for `SET`.** Rejected because
  they are list operations in Kafka's incremental protocol and silently
  accepting them for a scalar setting would make an invalid client request
  appear durable.

## Consequences

The topic-entry format remains backward compatible and the catalog contract is
the single persistence seam for configuration. Config updates cost one small
object write or delete, one catalog lookup, and one ordered metadata event.
The retention round remains bounded by due heap entries; updates add work only
for the topic named by the event. Cross-node concurrent updates are ordered by
the coordinator; the event handler reads the catalog's last-writer-wins value
before changing a deadline, and a tombstone still makes any late config
object unreachable. The `M12` minimum of `DELETION_DELAY_MS` is a product
lower bound, not a restatement of the GC inequality: post-trim deletion safety
continues to be checked by `GcTerms::CONFIGURED` and is independent of how
long a partition retains records before trimming.
