# 0034. `GroupCoordinator`: a sans-I/O seam mirroring `MaterializedIndex`

Status: accepted; 2026-09-11: `M4.25` — this header's `FR-22` is superseded.
`ADR-0033` deferred KIP-848 and `M4.24` moved the register to `deferred`, and
this ADR's body never reasons about KIP-848 at all; the seam it decides is
FR-20 and FR-21 work. `ADR-0033`'s own identical citation stays, being the
decision *about* FR-22. The reasoning here is untouched
Date: 2026-09-03
Requirements: FR-20, FR-21, FR-22

## Context

`M4.2`'s own backlog row asks for "a `pub trait GroupCoordinator`
(`contracts.md`) sans-I/O, holding group state and driving `M4.1`'s state
machine — `oqueue_coordinator::Coordinator`'s own shape (durable log +
in-memory materialized state) is the precedent to mirror, not a new one to
invent." `contracts.md` rule 12 requires this trait's own ADR to land in the
same commit as the trait itself, since a brand-new trait is a contract
change (rule 15: it "changes what an implementor is expected to guarantee").

Two existing seams are candidate shapes to mirror:

- [`MetadataLog`]: async (`BoxFuture`), because every real implementation is
  a network or disk round trip — durability crosses a process boundary.
- [`MaterializedIndex`]: synchronous, because every implementation (today:
  memory; doc 10 #12's engine choice later) is a local fold with no network
  call in it. Its own doc names exactly this reasoning: "an async read here
  would put object-storage reads *inside* the index."

`M4.1`'s `GroupState::transition` is already synchronous and sans-I/O by
construction (`NFR-51`) — it is a pure function of `(state, event,
generation, assignment_epoch)`. A `GroupCoordinator` that merely *holds* one
`GroupState`/`GenerationId`/`AssignmentEpoch` triple per group and drives
them through that same function has no I/O of its own either: v1 ships no
durable group-state store (`M4.14`'s own task is exactly that gap, not yet
built), so today's only implementation is in-memory, the same "memory today,
a real engine later" position `MaterializedIndex`'s own doc already states
for the metadata index.

**Also found while writing this row**: the backlog's own `M4.2` acceptance
criterion said "a fake in `oqueue-testkit`" — `contracts.md` rules 9 and 11
are unambiguous that a fake lives beside its trait in `oqueue-core`, never in
`oqueue-testkit`, and every existing fake in this workspace
(`FakeObjectStore`, `FakeMetadataLog`, `FakeMaterializedIndex`, `FakeClock`,
`FakeKeyProvider`) already follows that. The backlog row is corrected in the
same commit as this ADR, per this project's own "the spec is wrong, fix it
and say so" discipline (`sdd.md`) rather than building a rule violation to
match a task description written before the standard was re-read.

## Decision

1. **`GroupCoordinator` is synchronous, mirroring [`MaterializedIndex`], not
   [`MetadataLog`].** Every method returns `Result<T>` directly, no
   `BoxFuture`.
2. **One method drives the state machine, one method reads it** — the same
   "apply + query" shape `MaterializedIndex` already uses:
   - `transition(&self, group: &GroupId, event: GroupEvent) -> Result<GroupRecord>`
     — the *only* way a group's own state, generation, or assignment epoch
     ever changes. A group with no prior record starts from `(GroupState::Empty,
     GenerationId::INITIAL, AssignmentEpoch::INITIAL)` before the transition
     is attempted, so `GroupEvent::Join` on an unknown group id is how a
     group comes into existence — there is no separate `create`.
   - `record(&self, group: &GroupId) -> Option<GroupRecord>` — `None` for a
     group that has never transitioned (never joined) or that a caller with
     the ability to forget dead groups has since dropped; `M4`'s own scope
     does not yet build that eviction, so today `None` means only "never
     joined."
3. **`GroupRecord` bundles the three values `M4.1`'s own state machine
   threads together** (`GroupState`, `GenerationId`, `AssignmentEpoch`) — a
   plain data type, not a seam of its own (`contracts.md` rule 2).
4. **The fake, `FakeGroupCoordinator`, lives in `crates/oqueue-core/src/group_coordinator.rs`**,
   beside the trait — corrected from the backlog row's original "in
   `oqueue-testkit`" wording, per this ADR's own Context section.

## Alternatives considered

- **Async (`BoxFuture`), mirroring `MetadataLog`.** Rejected: nothing this
  trait's own v1 implementation does crosses a process boundary — `M4.14`'s
  durable-storage task is real, separate, not-yet-built work, the same
  "async is a later contract change with its own ADR" escape hatch
  `MaterializedIndex`'s own doc already reserves for its own eventual
  disk-backed engine.
- **A trait per group** (`GroupCoordinator: Send + Sync` scoped to one
  already-resolved group, constructed by a factory keyed on `GroupId`).
  Rejected: real Kafka's own coordinator is one component serving every
  group hashed to it (doc 02 §3.1's `hash(groupId) % numPartitions`), and
  `ADR-0033`'s own "every group resolves to this node" decision means v1's
  single coordinator instance already serves every group on this broker —
  a per-group trait object would need the same map this design already
  needs, just built by the caller instead of the implementor.
- **Fold group state through `MetadataLog`/`MaterializedIndex` themselves**,
  the way topic metadata folds through the coordinator's own log. Rejected
  for v1: `M4.14`/`M4.15` are the durable-storage and rebuild-on-takeover
  tasks this milestone's own plan already schedules separately, and folding
  group state into the *topic* metadata log now would make that later task
  a migration instead of new work — the same "don't build the merge before
  there is something to merge" instinct `ADR-0026`'s own deferred streaming
  writer already used.

## Consequences

- **This makes easy**: a fake exercised by a property test with no I/O
  harness at all (`M4.2`'s own acceptance criterion), and every later M4
  handler (`M4.7`-`M4.10`) driving group state through one call rather than
  reading and writing three separate fields itself.
- **This makes hard**: nothing new — the durability gap this decision
  inherits (`M4.14`) is already `M4`'s own scheduled task, not a cost this
  ADR adds.
- **This forecloses**: a handler constructing or mutating a `GroupState`
  value directly. `transition` is the only route in, the same "no setter"
  discipline `MaterializedIndex`'s own doc already states for the metadata
  index.

[`MetadataLog`]: ../../../crates/oqueue-core/src/metadata_log.rs
[`MaterializedIndex`]: ../../../crates/oqueue-core/src/materialized_index.rs
