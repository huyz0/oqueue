# 0035. Committed offsets get their own `GroupMetadataLog`, not a `MetadataRecord` variant

Status: accepted
Date: 2026-09-03
Requirements: FR-21

## Context

`M4.14`'s own backlog row asks for committed offsets to "survive a
coordinator restart... through the same durability mechanism
`oqueue_coordinator::Coordinator` already uses for metadata, not a second
bespoke store." `ADR-0034` (`M4.2`) already anticipated this exact question
and deliberately left it open rather than deciding it:

> "Fold group state through `MetadataLog`/`MaterializedIndex` themselves,
> the way topic metadata folds through the coordinator's own log. Rejected
> **for v1**: `M4.14`/`M4.15` are the durable-storage and rebuild-on-takeover
> tasks this milestone's own plan already schedules separately, and folding
> group state into the *topic* metadata log now would make that later task
> a migration instead of new work."

Two readings of "the same durability mechanism, not a second bespoke store"
are both live:

1. **Literally the same log**: add a variant to `oqueue_core::MetadataRecord`
   (today `BatchCommitted`/`EpochChanged`, both produce-shaped) and route
   offset commits through `oqueue_coordinator::Coordinator::commit`, the
   same running instance and the same `Arc<dyn MetadataLog>` topic metadata
   already uses.
2. **The same kind of mechanism, a separate instance**: a new,
   small `GroupMetadataLog` trait with the identical shape and guarantees as
   `MetadataLog` (append/read_from/last_version, durability-on-`Ok`,
   strictly-increasing versions, all-or-nothing batches), its own small
   closed record enum, and its own fake — mirroring the pattern rather than
   sharing the instance.

Three facts settle it in favor of (2):

- **`MetadataEntry.record` is monomorphic in `MetadataRecord`**
  (`metadata_log.rs:14-16`), not generic. Reusing the trait literally would
  mean widening `MetadataRecord`'s own exhaustive match — and its own module
  doc states the exhaustiveness is deliberate precisely so "a new variant
  here is a new event every applier must be made to consider." Every
  existing applier (`MemoryIndex`, `FakeMaterializedIndex`) is
  partition-offset-shaped; forcing them to grow a match arm for
  `OffsetCommitted` that means nothing to either is the exact entanglement
  `ADR-0034`'s own "would make that later task a migration instead of new
  work" already named as the reason to defer, not to eventually do anyway.
- **`Coordinator::commit` is hard-wired to `(ObjectKey, Vec<CommittedSpan>)`**
  (`coordinator.rs:257-272`), not generic over record kind. Making it
  generic, or adding a second `Request` variant beside `Commit` that a
  produce-shaped `CoordinatorLoop` also has to serve, touches the allocator,
  the request enum, and the serve loop that FR-32/FR-10's own conformance
  suite already exercises — a footprint far larger than "wire a new record
  variant."
  the produce-commit-hardened blast radius `M3`'s own tests exist to guard.
- **Both mechanisms already coexist as separate seams by design.**
  `GroupCoordinator` itself (`ADR-0034`) is already a distinct trait from
  `MaterializedIndex`/`Coordinator`, holding group *state* independently of
  the topic metadata fold. A `GroupMetadataLog` distinct from `MetadataLog`
  is the same split one level down — durable *storage* for group-owned
  facts, independent of the topic metadata log, the same way
  `GroupCoordinator` is independent of `MaterializedIndex` for group-owned
  *state*.

"Not a second bespoke store" is read here as ruling out a hand-rolled,
ad hoc persistence format invented for this one purpose (a JSON blob, a
side file) — not as requiring literal reuse of one running `Coordinator`
instance across two unrelated domains. A second `MetadataLog`-shaped trait,
implemented by the identical `FakeMetadataLog`-style in-memory fake pattern
this workspace already uses six times over (`ObjectStore`, `MetadataLog`,
`MaterializedIndex`, `Clock`, `KeyProvider`, `GroupCoordinator`), is the
established idiom for "a seam, not a store."

## Decision

1. **`GroupMetadataLog` is a new trait in `oqueue-core`**, `metadata_log.rs`'s
   own shape verbatim: `append`, `read_from`, `last_version`, the same five
   numbered guarantees (durability-on-`Ok`, strictly increasing versions,
   all-or-nothing batches, inclusive/ordered reads, no partial pages). Async
   (`BoxFuture`), for the identical reason `MetadataLog` is: every real
   implementation is a network or disk round trip.
2. **`GroupMetadataRecord` is its own small, exhaustive enum** — one variant
   for `M4.14`: `OffsetCommitted { group: GroupId, topic: TopicId, partition:
   i32, offset: i64 }`. Exhaustive for the same reason `MetadataRecord` is: a
   silently-absorbed new variant is a replay bug that does not surface until
   it produces the wrong offset.
3. **`FakeGroupMetadataLog` is the only implementation for v1** — same
   status as `FakeMetadataLog`: satisfies guarantee 1 vacuously, nothing
   durable across a process restart. A real engine is `M6`'s own
   already-scheduled work (doc 10 #12), the same one `MetadataLog` itself is
   waiting on. `CommittedOffsets` (`M4.12`) is therefore durable in the same
   *sense* `Coordinator`'s own topic-offset allocation already is today —
   correctly ordered (ack after append, never before), replayable from the
   log — and not yet durable in the sense of surviving an actual process
   restart, which needs the same not-yet-built real engine either seam does.
4. **No hook into `oqueue-coordinator::Coordinator::open`.** Group-state
   bootstrap is its own, separate replay step
   (`CommittedOffsets::replay(log)`), called once by whatever composes a
   `Cluster` — mirroring `Coordinator::open`'s own role without sharing its
   code, since the two logs are unrelated. `COORDINATOR_LOAD_IN_PROGRESS`
   (`M4.11`, already a real wire code) is not wired to this replay step by
   this task — `M4.15`'s own row ("rebuild on shard reassignment — the
   `COORDINATOR_LOAD_IN_PROGRESS` window") is where a live request during an
   in-progress replay gets that answer instead of racing ahead of it.

## Alternatives considered

- **Add `OffsetCommitted` to `MetadataRecord`, route through
  `Coordinator::commit`.** Rejected: entangles two unrelated appliers behind
  one exhaustive match (the reason `ADR-0034` already deferred this),
  requires `Coordinator::commit` to stop being produce-shaped, and turns
  `M4.15`'s own later rebuild-on-shard-reassignment task into a migration of
  already-shipped code rather than new work — the exact cost `ADR-0034`
  named and asked this task to avoid.
- **A single, more generic `MetadataLog<R>` parameterized over record
  type**, reused for both domains. Rejected: `MetadataLog` is an already-`
  pub` `contracts.md`-governed seam with a real implementation surface
  (`FakeMetadataLog`, and `oqueue-coordinator`'s own consumer of it); making
  it generic is a contract change to an *existing*, heavily-depended-on
  trait for the sake of a *new* consumer, a larger and riskier edit than
  adding a new, small, independent trait beside it.
- **No log at all — keep `CommittedOffsets` purely in-memory, defer
  everything to `M6`.** Rejected: `M4.14`'s own backlog row is explicit
  acceptance criteria (a fault-injection test, a restart-replay test); a
  seam that does not exist cannot be tested against, and `M3`'s own
  precedent (`MetadataLog` exists and is tested well before `M6`'s real
  engine does) is exactly this same "the seam first, the engine later"
  sequencing applied one milestone earlier for the same reason.

## Consequences

**Easier**: `M4.15`'s own group-state-durability task extends
`GroupMetadataRecord` with new variants (group state, generation) without
touching `MetadataRecord` or `Coordinator` at all — the split this ADR
makes is exactly what keeps that task additive. A real engine chosen for
`MetadataLog` in `M6` is a natural, independent candidate for
`GroupMetadataLog` too (same shape), but the two are never forced to share
an instance, so a future decision to route topic and group metadata through
physically different storage systems is not foreclosed by anything built
here.

**Harder**: two `MetadataLog`-shaped traits now exist with near-identical
method sets and no shared code — a maintenance cost this ADR accepts
explicitly rather than solve with an early, unproven abstraction (a
`MetadataLog<R>` generic) over two data points.

**Foreclosed**: nothing yet reads group metadata and topic metadata as one
ordered stream (useful for, say, a single consistent snapshot across both).
If that is ever needed, it is a new capability layered over two logs, not
free from this design — named so a later reader does not assume it already
holds.
