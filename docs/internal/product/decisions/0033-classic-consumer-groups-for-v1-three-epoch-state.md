# 0033. Classic consumer-group protocol for v1, on a three-epoch state shape

Status: accepted
Date: 2026-09-03
Requirements: FR-20, FR-21, FR-22

## Context

`M4.md`'s "Decisions required first" table names three open questions, all of
which gate every task in the milestone: whether v1 ships the classic
JoinGroup/SyncGroup/Heartbeat protocol only or also KIP-848's
`ConsumerGroupHeartbeat` redesign; what internal state shape the group
coordinator is built on; and how `FindCoordinator` resolves a group to the
node that owns it.

**Protocol choice.** Doc 02 §3.3's own measurement is the real signal:
Instaclustr's benchmark found a 100→1,000-partition rebalance dropping from
103 s under the classic protocol to 5 s under KIP-848 — a genuine, large
improvement, not a marginal one. Against that: KIP-848 was GA for
server-side assignors only in Kafka 4.0, client-side assignors remain
unimplemented upstream, and — the number that actually gates adoption —
librdkafka 2.10 carries only an early-access implementation (doc 02 §3.3,
citing Confluent's own KIP-848 writeup). A protocol this broker cannot make a
real client speak is not a capability; FR-22's own status in
`requirements.md` is already `provisional`, worded "v1 may ship the classic
protocol only," anticipating exactly this call.

**State shape.** The classic protocol's own `GenerationId` is a single
counter bumped on every rebalance. KIP-848 replaces it with three cooperating
epochs — Group, Assignment, Member (doc 02 §3.3) — because a single counter
cannot express "this member's assignment has caught up with the group's
latest state" independently of whether a *new* assignment computation has
even started. `M4.md`'s own plan already flags this: "decide this even if
KIP-848 is deferred."

**Shard resolution.** `M4.md`'s plan says `FindCoordinator` must resolve
group → shard → owning node, and that the answer must be allowed to change,
"because M7's shards are internal and rebalanceable." `MetadataShardId` does
not exist yet — `roadmap.md`'s deferred table (`M7.md` tasks 17a/17b) already
records that every M3-era shard-identity gap waits on M7, and this project's
own precedent for "a mechanism with no live caller is not this milestone's to
build" is `M3.11`'s enforcement gap and `M6.md` task 15's rebuild-off-the-
ack-path deferral, both parked for exactly that reason.

## Decision

1. **v1 ships the classic protocol only.** `JoinGroup`, `SyncGroup`,
   `Heartbeat`, `LeaveGroup`, the fencing error family. KIP-848 is not built
   in M4.
2. **The group state machine is built on the three-epoch model** — Group
   Epoch, Assignment Epoch, Member Epoch — from the first commit, even though
   only the classic protocol drives it in v1. The classic protocol's
   `GenerationId` is derived from Group Epoch (they advance together;
   `GenerationId` is Group Epoch's classic-protocol name), not tracked as a
   second, independent counter.
3. **`FindCoordinator` resolves every group to this node — the only node —
   for v1.** No `MetadataShardId`, no group→shard table. The seam is real
   (`FindCoordinator`'s response names a node id and this decision is what
   fills it in), but its body is `Cluster::node_id`, unconditionally, until
   M7 gives it something to resolve.

## Alternatives considered

- **Build KIP-848 alongside the classic protocol in v1.** Rejected on doc 02
  §3.3's own gap: no reference client this project tests against
  (librdkafka, the Java client) can drive it past early-access today, so the
  wire surface would be untested by anything except a hand-rolled harness —
  the opposite of this project's own conformance-first discipline
  (`m2-complete.sh`, `m11-complete.sh`'s own client-harness legs). Real
  payoff, no real caller yet.
- **KIP-848 only, skip classic.** Rejected outright: it would refuse every
  client in the fleet doc 02 §7.1 names as the compatibility bar
  (librdkafka, the Java client, Sarama, franz-go), none of which defaults to
  the new protocol yet.
- **A single `GenerationId` counter, decide the epoch model later if KIP-848
  ever lands.** Rejected because `M4.md`'s own plan already names the
  failure mode: KIP-848 coexistence works by the coordinator translating
  classic calls onto the new group model, which needs the three-epoch shape
  to translate *onto*. Building the narrower model first and expanding it
  later is the rewrite this decision exists to avoid, the same logic
  `ADR-0020`'s multi-topic bundling and `ADR-0023`'s epoch-carrying
  watermark already applied to this project's other coordinator surfaces.
- **A real (if trivial) group→shard table now, sized for one shard.**
  Rejected as unearned infrastructure: `M3`'s own "one implicit shard"
  precedent (recorded across `roadmap.md`'s M7-deferred rows) is that a
  table with one row and no second row to ever compare against cannot be
  tested for the property it exists to hold — "the answer is allowed to
  change" is unfalsifiable with nothing to change it into. `FindCoordinator`
  answering `Cluster::node_id` directly is honest about what this build
  can actually resolve.

## Consequences

- **This makes easy**: shipping something a real client (librdkafka, the
  Java client) can join today, conformance-tested the same way M2/M11
  already are; a state machine KIP-848 can extend rather than replace,
  should a later milestone pick it up.
- **This makes hard**: nothing in M4 itself — the epoch model costs a little
  more code up front than a bare `GenerationId` would, for a benefit v1
  cannot yet spend (no KIP-848 wire API). That cost is deliberate, paid once
  rather than twice.
- **This forecloses**: a hand-rolled group→node resolution shortcut inside
  handler code that bypasses `FindCoordinator`'s own answer — the seam
  exists precisely so M7 can change what fills it in without touching every
  caller. It does not foreclose KIP-848 arriving in a later milestone; that
  is exactly the coexistence path the epoch model is chosen to keep open.
