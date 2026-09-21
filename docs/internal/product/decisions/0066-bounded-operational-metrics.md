---
title: "ADR-0066 — Bounded operational metrics"
status: accepted
date: 2026-09-22
---

# Context

FR-52 needs enough telemetry to diagnose a stalled partition, while NFR-12
and the M12 plan forbid turning a large topic catalog into a permanent
per-partition metrics registry. The existing seams already know the relevant
facts: broker flushes and fetches, coordinator replay, the materialized index,
compaction lifecycle, and the two DEK caches.

# Decision

Use a cloneable, in-process `oqueue_core::OperationalMetrics` handle with no
external exporter dependency. Aggregate counters are always retained for
writes, coordinator readiness, compaction backlog, DEK cache hits/misses, and
index entries. Named partition lag and write observations are retained in a
bounded map capped at `MAX_SCOPED_PARTITIONS`; additional names are counted as
dropped scoped samples rather than growing the process without limit.

Each component may receive the shared handle through `with_metrics`, while its
default constructor remains self-contained for existing callers and tests. A
snapshot is an observation for export or diagnostics, never a correctness
input. Atomic fields are intentionally only approximately point-in-time.

The broker records write latency at the flush seam using its I/O-shell clock,
records lag after resolving a fetch partition, and records coordinator health
when replay completes or permanently fails. Index, compaction, and crypto
components publish their exact local state or cache outcomes through the same
handle. No metric contains credentials, key material, or error display text.

# Alternatives rejected

* A global registry would make tests share state and would hide the cardinality
  bound in an exporter configuration.
* A metrics crate would add an exporter and dependency policy before M12 has a
  transport or endpoint decision; the core snapshot is stable across those
  future choices and adds no C toolchain requirement.
* A permanent label for every topic and partition violates the bounded-cost
  requirement. Scoped diagnostics are deliberately capped and report when
  samples are discarded.
