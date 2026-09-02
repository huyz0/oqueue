# 0031. Idempotent-producer state lives with the offset allocator, not in a retroactive tombstone

Status: accepted; 2026-09-02 (`M11.2`): point 4's field shape is stale — `CommittedSpan` gained one grouped field, `producer: Option<ProducerIdentity>`, not the three separate optional fields below, to stay under `clippy.toml`'s five-argument threshold. See `M11.2`'s own commit message.
Date: 2026-09-02
Requirements: FR-14, FR-15 (scope)

## Context

Doc 06 §6 surveys the field's answers to Kafka's idempotent-producer
contract — deduplicate a retried batch by `(producer_id, partition)` sequence
number:

- **WarpStream's retroactive tombstone.** Agents PUT batches without
  coordinating on sequence numbers first; a centralized, externally-hosted
  Cloud Metadata Store — itself a single serialization point, not a
  leaderless one — makes the final sequencing and dedup decision *after* the
  bytes already exist in object storage, by retaining the last few sequence
  IDs per `(producer_id, partition)` and simply never returning metadata for
  a batch it identifies as a replay. The bytes physically remain until
  compaction reclaims them. ⚠️ **This is not a consequence of WarpStream
  lacking a serialization point** — an earlier draft of this ADR claimed
  that, and it does not survive a close read of the source it cited: the
  metadata store *is* one. The consequence is a **design choice**
  WarpStream's own docs frame as deliberate — accept every write
  unconditionally and decide visibility afterwards — and the costs are ones
  that choice carries regardless of where the serialization point lives:
  batch merging disabled while idempotency is on, dead batches consuming
  storage and read bandwidth until compaction, and (as of the surveyed
  sources) transactions not yet shipped on top of it.
- **Redpanda Cloud Topics keeps Raft locally** specifically to retain a
  stable per-partition leader, and gets ordinary Kafka idempotency and
  transactions "for free" as a consequence — at the cost of the
  leader-per-partition complexity this project has already declined
  (`AGENTS.md`'s mission constraint, `ADR-0020`'s Option C rejection).
- **KIP-1150 defers the mechanism** to a follow-up KIP and is not yet
  considered solved even at the design-document stage.

Doc 10 open question #5 frames this as a choice among those. What actually
decides it for oqueue is narrower: `ADR-0020` already built a serialization
point of oqueue's own —

> "Each metadata shard has exactly one active coordinator, in-process
> (`oqueue-coordinator`), and that coordinator is the sole writer of that
> shard's one metadata log."

— fenced by `CoordinatorEpoch` so a failover replaces the coordinator rather
than letting two run concurrently, and already gating the *commit* that
turns a PUT into a durable offset: `ADR-0020` point 2 has offsets assigned
"retroactively at commit time... never at flush time", so a commit can
already be refused after the object is written (`M10.9`'s orphan-object
crash point). Accepting a write unconditionally and deciding visibility
afterwards, at a *second* point, is what a system reaches for when its one
serialization point cannot also gate admission — and oqueue's already can,
for exactly the reason offsets are assigned where they are. Building
WarpStream's separate deferred-visibility mechanism next to a commit gate
that already exists would buy the same guarantee twice, at the real costs
that choice documents, for no reason specific to oqueue's own shape.

## Decision

**Producer sequence state is tracked by the shard's allocator, at the same
serialization point that already assigns offsets** —
`oqueue-coordinator::allocator::Allocator`, alongside `end_offsets`. A
duplicate or out-of-order sequence is rejected (or transparently
deduplicated) at commit time, not before the PUT and not after it via a
second, later decision.

Concretely:

1. `Allocator` gains a `producer_state: HashMap<(ProducerId, TopicId,
   PartitionId), ProducerState>` beside `end_offsets`, where `ProducerState`
   holds the last accepted `(ProducerEpoch, i32 sequence, Offset base_offset,
   u32 record_count)`. Same fold shape as the offset line: read to decide,
   never mutated until a journal append actually lands.
2. **The sequence check is a filter run *before* `Allocator::stage`, not a
   change to `stage`'s own per-span loop — and this is the answer `M11.md`
   task 9 asked for** ("interaction with the flush batcher... shaped by the
   #5 ADR and not before it"). `Allocator::stage(object, spans)` returns one
   `Result` for the *whole* commit today (`a_refused_stage_consumes_nothing`
   pins this), and `FR-32` bundles more than one producer's spans into one
   object — so if the sequence check lived inside `stage`'s loop, one
   producer's ordinary retry landing in the same flush as an unrelated
   producer's fresh batch would fail the whole PUT's commit for every
   tenant sharing it. A new step, `Allocator::admit(spans) ->
   AdmittedSpans`, runs first and partitions `spans` against the *current*
   `producer_state` (a producer's own spans checked against each other in
   bundle order, exactly as `stage`'s own `running` map already tracks
   offsets within one call before touching committed state):
   - **Next in sequence** (`sequence == last + 1`, same epoch): admitted.
     Carries its pending `(epoch, sequence)` forward so `stage` can record
     the matching `producer_state` write alongside the offset it assigns —
     same all-or-nothing schedule `Staged`'s offset writes already use, so
     an offset and its producer-state advance land together or not at all.
   - **Exact replay** (`sequence == last`, same epoch, same record count):
     resolved by `admit` alone, without ever reaching `stage` or the
     journal. The **already-recorded** `base_offset` is returned directly —
     the transparent-success case a real duplicate must produce, at zero
     added cost to the commit path other than the lookup.
   - **Anything else** (a gap, a lower sequence, an older epoch): excluded
     from what `admit` forwards to `stage`. Reported to the client on that
     one `(topic, partition)` independently — `OUT_OF_ORDER_SEQUENCE_NUMBER`
     for a gap, `DUPLICATE_SEQUENCE_NUMBER` for a stale replay that does not
     match the recorded record count (task 6 of `M11.md`) — exactly the
     shape a per-partition `ProduceResponse` already has for any other
     partition-scoped failure, so one producer's rejected retry never
     touches whether a *different* producer's span in the same bundled PUT
     gets an offset.
3. **The bytes still land in object storage before the rejection is known.**
   Producers PUT independently of the coordinator (`ADR-0020` point 2), so a
   duplicate's object exists in storage by the time its commit is evaluated,
   exactly as an ordinary refused commit's object does today. This is the
   *existing* orphan-object path `M10.9`'s crash-point enumeration already
   built and tests (`a_refused_journal_between_the_put_and_the_commit_leaves_
   an_unreferenced_object`) — a duplicate's PUT is not a new failure mode to
   invent, it is the same one with a different cause. Reclaiming the bytes
   is `M5`'s compaction, unchanged by this decision; no tombstone-visibility
   mechanism is added anywhere.
4. **Producer state is recoverable, not durable-in-memory-only.**
   `CommittedSpan` (`oqueue-core`) gains the producer identity and sequence
   a caller attaches to a span — `producer_id: Option<ProducerId>`,
   `producer_epoch: Option<ProducerEpoch>`, `sequence: Option<i32>` — so a
   committed `MetadataEntry` carries what `Allocator::apply` needs to
   reconstruct `producer_state` from the log on coordinator restart, the
   same way it already reconstructs `end_offsets`. A `None` triple is an
   ordinary non-idempotent produce and costs nothing. This is a change to
   an `oqueue-core` type, not a `pub trait`'s method set, so it does not
   invoke `contract-change`'s trait-and-fakes procedure — but it is exactly
   the "changes an oqueue-core contract" case `adr.md` requires an ADR for,
   which this is.
5. **`CoordinatorEpoch` fences producer state exactly as it fences offset
   assignment.** A coordinator that has lost its lease must not accept a
   sequence check against `producer_state` any more than it may assign an
   offset — both are the same "am I still the one writer" question, answered
   once, not twice. `M11.md` task 10's fencing test is this property, stated
   for sequences instead of offsets.
6. **Producer-state expiry is bounded**, per `M11.md` task 7 and its own
   Risks section — a `HashMap` keyed by every producer that has ever
   written is the quota problem `M3.11`'s deferred index-growth row already
   named for the offset index, one structure over. `M11`'s own task decides
   the eviction policy; this ADR only requires that whatever it picks does
   not evict state a subsequent retry still needs within the client's own
   retry window (`request.timeout.ms`-scale, not `session.timeout.ms`-scale).

## Alternatives considered

**WarpStream's retroactive tombstone**, rejected not because oqueue lacks
what it needs — it does not — but because oqueue's coordinator already has
a commit-time admission gate the tombstone mechanism exists to substitute
for, and building both would carry the tombstone's own documented costs
(batch merging disabled while idempotency is on, dead batches until
compaction) for a guarantee already available for less. WarpStream has also
**not shipped transactions** on top of this mechanism, which is the outcome
`M11.md`'s own decision point warns a mechanism choice here could produce
("choosing a mechanism that forecloses transactions is a decision about v2,
made here") — not proof this mechanism forecloses them, but reason enough
not to default to it when a cheaper alternative is available.

**Redpanda's local Raft-backed leader**, rejected on the same grounds
`ADR-0020` already rejected Option C: it requires exactly the
leader-per-partition, attached-disk complexity `AGENTS.md`'s mission
constraint declines. Not reconsidered here — nothing about idempotency
changes that trade.

**Deferring the mechanism decision, shipping only `InitProducerId` and
stubbing dedup**, rejected because `M11.md`'s own framing is the reason not
to: librdkafka enables the idempotent path *by default*, so `M2`'s
`enable.idempotence=false` workaround is a real, user-visible gap until this
ships, not an optional feature. A milestone whose whole point is closing
that gap should not open by deferring the one decision that unblocks the
rest of its own task list.

**Keeping producer state entirely out of `oqueue-core`, private to
`oqueue-coordinator`**, rejected because it is not recoverable across a
restart without it — `producer_state` folds from the same durably-journaled
log `end_offsets` already folds from, and a fold needs its source data in
the record it folds. The alternative (a fully separate durability path for
producer state) would duplicate `ADR-0020`'s entire "assign → journal →
ack" machinery for one more piece of state, for no reason this ADR can
find.

## Consequences

**Makes easy.** No new visibility mechanism, no compaction-time tombstone
processing, no second sequencing point to keep consistent with the first.
`Allocator::stage`'s existing "compute without taking, apply only after the
journal accepts" discipline (`M3`'s own design) is untouched by the
rejection case, which `admit` resolves before `stage` ever sees it — one
producer's gap or replay cannot fail an unrelated producer's offset
assignment in the same bundled commit, which a check inside `stage`'s own
all-or-nothing loop could not have given for free. What *is* staged (a
next-in-sequence advance) rides the exact schedule offset writes already
use.

**Makes hard.** `CommittedSpan` (and therefore `MetadataEntry`,
`MetadataRecord`) grows three optional fields every existing caller and
every `FakeMetadataLog`/real-engine implementor must carry, even for
non-idempotent produces — a real but bounded refactor across `oqueue-core`
and its conformance suite, landing with `M11`'s first task rather than
retrofitted later. `Allocator::admit` is a new function with its own
per-producer, in-bundle-order accumulation (mirroring `stage`'s `running`
map, one structure over) that a caller must invoke before `stage` rather
than a change hidden inside an existing one — the flush composer now has
two allocator calls to sequence correctly, not one.
`producer_state`'s eviction policy is a real design
question `M11`'s own task 7 still owes; this ADR fixes *where* the state
lives, not how large it is allowed to grow.

**Forecloses.** WarpStream-style deferred/retroactive visibility as a
*mechanism* for idempotency specifically — not because it could not be
built here, but because oqueue's existing commit-time admission gate makes
it redundant. ⚠️ **What this ADR can honestly claim about FR-15 is narrower
than "does not foreclose it".** A single-partition transaction — every
write inside one shard's log — fits this mechanism directly: `PREPARE`/
`COMMIT` markers are more state at the same serialization point,
admitted or refused by the same gate `admit`/`stage` already are. A
**cross-shard** transaction is not addressed by anything here — `ADR-0020`
already scopes a `CommitVersion` to one shard, and atomic visibility across
several shards' independent logs is the multi-log-transaction problem
`ADR-0020`'s own "flush must not span shards" rule exists to avoid, not
something this ADR's producer-sequence mechanism has an answer for either
way. FR-15's cross-shard case is exactly as open after this decision as
before it; its single-shard case has a more natural home than the
retroactive-tombstone alternative would have given it.
