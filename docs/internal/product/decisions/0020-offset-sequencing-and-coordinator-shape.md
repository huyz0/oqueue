# 0020. Offset sequencing and coordinator shape

Status: accepted; 2026-08-23 (`M3.4`): point 5's seam is now built —
`MetadataLog` in `oqueue-core` (`append`, `read_from`, `last_version`) with
`FakeMetadataLog` beside it and a conformance suite any later engine inherits.
⚠️ Two guarantees the method names do not carry, recorded here because an
engine must not discover them late: versions are **strictly** increasing both
within a batch and across batches, so a repeat is as much a violation as a
decrease — which is the cell an ack-lost retry re-offering its last version
lands in first; and **a refused append stores nothing**, not even the entries
before the offending one, or a caller retrying after a rejection replays onto
a log already holding part of that batch and the offset fold double-counts in
silence. The engine itself (doc 10 #12) stays open, as point 5 leaves it.
Date: 2026-08-23
Requirements: FR-10, FR-11, FR-12, FR-13, FR-32, NFR-1, NFR-2, NFR-3, NFR-21

## Context

Doc 06 §1 surveys three structurally different answers to "who assigns the
next offset" and states no preference among them:

- **Option A** — an external, strongly-consistent metadata store is the single
  serialization point; offset order is decided *retroactively at
  metadata-commit time*, which is what lets many writers PUT concurrently
  without coordinating (WarpStream's Cloud Metadata Store, KIP-1150's Batch
  Coordinator, Delos's `MetaStore`).
- **Option B** — object-storage-native conditional writes (`If-Match` /
  `If-None-Match`) are the CAS primitive directly on the offset stream: name
  each record's object by its intended offset, let the store reject
  conflicting writers.
- **Option C** — consensus stays local: an in-process Raft leader per
  partition on attached disk (Redpanda Cloud Topics, and AutoMQ's reuse of
  KRaft), with only payload bytes going to object storage.

Doc 10 #1 leaves this open and un-struck. `roadmap.md`'s unmade-decisions
table lists it as blocking M3, coupled to #14 (M6's enumeration fork) and #7
(the latency tier) — deciding it in isolation is named there as a hazard this
ADR must not create.

But the choice is not actually three-way live inside this project. Four
statements narrow it before this ADR is written — ⚠️ **three of them binding
and one not**, a distinction that matters because the non-binding one carries
the whole Option B rejection. `AGENTS.md`'s mission constraints and doc 15's
position are binding; `M3.md` is a *plan*, explicitly a hypothesis under
`sdd.md`'s plan-versus-decomposition rule, so its Risks bullet is evidence to
weigh rather than a constraint to obey. It is cited below because the
measurement behind it holds independently — see the Alternatives section,
where Option B is rejected on the corrected reading of that number rather
than on the plan's authority:

- `AGENTS.md`'s mission line states the architecture pattern directly: *"The
  architecture pattern is WarpStream's."* That is Option A's shape —
  retroactive sequencing through a single serialization point, not per-record
  CAS and not a local per-partition Raft leader.
- `AGENTS.md` also states a hard constraint: *"no inter-broker replication, no
  partition leadership."* That excludes Option C outright — Redpanda's and
  AutoMQ's shape both require a leader-per-partition on local disk, which is
  exactly the broker-affinity/failover class of complexity this project has
  already declined to take on.
- Doc 15 §6 narrows Option A itself: it argues against copying WarpStream's
  *separately-hosted* control plane — *"WarpStream's separate hosted control
  plane is the outlier, and it is a business-model choice... copying it would
  reintroduce exactly the external-dependency adoption tax that argues against
  requiring FoundationDB or TiDB."* `architecture.md`'s crate map already
  reflects this: `oqueue-coordinator` is listed as an in-process crate holding
  "Metadata, offset sequencing, recovery," not a separately deployed service.
- M3.md's own Risks section rules out Option B for the hot path on
  measured grounds — *"Do not put CAS-on-object-storage on the offset
  stream... the published ceiling for the CAS-only approach is about one
  entry per second per namespace"* — while explicitly sanctioning
  conditional writes for a different, low-frequency purpose: *"compaction
  claims, leader epochs, checkpoint pointers."* Leader epoch is the seam this
  ADR needs: coordinator singleton election is exactly a low-frequency
  conditional-write use, not a per-record one.

So what remains open after those four statements is narrower than doc 06's
three-way survey: not "which of A/B/C" but "how is Option A's single
serialization point realized without a separately-hosted external DB and
without per-record CAS."

## Decision

**Each metadata shard has exactly one active coordinator, in-process
(`oqueue-coordinator`), and that coordinator is the sole writer of **that
shard's one metadata log** — not one log per partition. Offsets are assigned retroactively at commit time — the log append
is the serialization point — never at flush time and never by naming an object
after its intended offset.**

⚠️ **The log's scope is load-bearing and is a shard, not a partition.** A
per-partition log would make FR-32 unimplementable: `M3.13`'s flush spans N
topics and issues exactly one PUT, and committing that object's position
across N per-partition logs is a multi-log transaction with no atomicity —
a crash between appends leaves the object committed for some partitions and
not others. One log per shard makes that commit a single append covering
every `(topic, partition)` the object touched, which is also the shape doc 12
§4.1 already assumes (one `commit_version` and one `applied_upto` for a cache
holding many `(TopicPartition, Offset)` entries). Shard, rather than one
global log, because doc 10 #19 is already resolved (doc 15 §3): metadata
shards are internal and rebalanceable, which is what lets offset-assignment
throughput scale past one allocator without reaching for per-record CAS.
⚠️ **A flush must therefore not span shards** — an object bundles topics
within one shard, so one PUT is still one append. `M3.13` owns enforcing that.

Concretely:

1. `CommitVersion(u64)` is the one monotonic scalar the coordinator hands out,
   stamped by a single allocator per metadata log — that is, **per shard** —
   as each commit lands (M3 task 1). It is a counter, not a timestamp or a
   hash: staleness compares as `u64 <= u64`, nothing else. ⚠️ It is
   consequently **only ordered within its shard**, which is what `M3.9`'s
   push subscription and `M3.10`'s `AtLeast(v)` read mode are scoped to;
   comparing versions across shards is meaningless and neither may do it.
   ⚠️ **A `CommitVersion` must therefore never travel without its shard
   identity**, and a watermark held across a *rebalance* — the shard moving
   to another coordinator, which doc 15 §3 makes an ordinary operation —
   needs the receiving coordinator's version line to continue the old one
   rather than restart, or `AtLeast(v)` either stalls forever (new line
   below `v`) or returns stale data believing it is fresh (new line already
   past `v` for unrelated commits). This ADR states the requirement and does
   not choose the mechanism; the candidates are a shard-scoped epoch beside
   the version, or persisting the high-water version with the shard so it
   resumes rather than restarts. **`M3.10` owns picking one**, and cannot
   claim read-your-writes without it.
2. Producers race to accumulate batches and flush independently (M2's
   connection layer already does this); the coordinator does not arbitrate
   who may PUT. It only decides, after a PUT lands, what offset range that
   object's records now occupy — Option A's retroactive-commit shape, stated
   in M3 task 4 ("offset assignment at commit time, not flush time... lets
   many writers PUT concurrently without coordinating").
3. No offset is externally visible before its log record commits (M3 task 5:
   assign → journal → ack ordering). This is what makes FR-10 and NFR-21 hold
   without a fast-path WAL: the ack the client sees already reflects a
   committed, durable position.
4. Object-storage conditional writes are used for two things only, both
   low-frequency control-plane operations, never the offset stream itself:
   coordinator-leadership lease (fencing a stale coordinator out after
   failover — the `CoordinatorEpoch` M3 task 3 puts in every response) and
   compaction/checkpoint claims (M5/M6 territory). This is the one place
   Option B's mechanism is genuinely used, exactly where M3.md's Risks
   section sanctions it.
5. The metadata log's own durable storage is **not** decided by this ADR.
   Doc 10 #12 (materialized-state engine: SQLite/redb/RocksDB/fjall/SlateDB)
   is a separate open question, explicitly *not* listed in M3.md's "decisions
   required first," and doc 13 §6 recommends resolving it empirically by
   benchmarking rather than by argument — that benchmark has not been run.
   M3 task 9 already requires `MaterializedIndex` to sit behind a trait,
   documented as a cache the coordinator can drop and refill, never the
   source of truth. This ADR requires the same seam-first discipline for the
   log itself: a trait for "durably append a batch of metadata entries, read
   them back in order," with an in-memory fake for the sans-I/O tests M3
   needs, and a first concrete engine chosen as an implementation task once
   the coordinator's write shape exists to benchmark against. ⚠️ **This is
   not the hazard `M3.md` warns about** — that warning is about #1, #14 and
   #7, and says deciding *coupled* decisions independently is what produces
   an architecture nobody chose. #12 is not coupled to #1: any of the five
   candidate engines can hold a shard-scoped append-ordered log, so the
   sequencing shape does not select one, and doc 13 §6 asks for a benchmark
   rather than an argument. Deferring it is separating what is genuinely
   separable, not deciding half a coupled pair.
6. High availability of the coordinator itself (failover between coordinator
   processes, the recovery scanner, cold-rebuild RTO) is doc 10 #14/#15
   territory and belongs to **M6** (Recovery and failover), which already
   depends on M3 for exactly this reason. M3 builds a single active
   coordinator correct under concurrent *producers*, not yet fault-tolerant
   to its own crash beyond the fault-injection case FR-10 already requires
   (kill between PUT and ack, lose nothing acknowledged) — that case exercises
   producer-visible durability, not coordinator-process failover.

This closes doc 10 #1 for M3's purposes: Option A's shape, realized in-process
rather than as a separately-hosted service, with Option B's mechanism reserved
for leadership and control-plane claims, and Option C excluded by the
project's own no-partition-leadership constraint.

## Alternatives considered

**Option B, pure CAS on the offset stream** (name each record's object by its
intended offset, let conditional PUT serialize it). Rejected on **latency**,
which is the honest reading of the ceiling: M3.md's Risks section cites ~1
entry/sec/namespace, and doc 14 §2 corrects that number's common misreading
explicitly — it is *"a commit-rate ceiling, not a throughput ceiling. One
commit carries up to ~10k documents / 32 MB, hence 10k writes/s per
namespace. What it actually bounds is latency — a 200ms–1s floor per write.
That latency is invisible to turbopuffer's users and fatal to Kafka's."*
⚠️ So the requirement this fails is **not** FR-11 — CAS-by-offset-name
satisfies gap-free monotonicity by construction, which is exactly its appeal
— but **NFR-1**, produce p99 ≤ 500 ms / p50 ≤ 300 ms: offset assignment sits
on the produce-ack path (Decision point 3's assign → journal → ack), so a
200 ms–1 s floor per assignment consumes the entire budget before the object
PUT that must also happen is counted. Used instead, narrowly, for coordinator
leadership and compaction claims — see Decision point 4.

**Option C, local Raft leader per partition** (Redpanda Cloud Topics,
AutoMQ). Rejected on two independent grounds: it directly contradicts
`AGENTS.md`'s "no inter-broker replication, no partition leadership"
constraint, and it reintroduces the broker-affinity/failover complexity class
this project's mission explicitly declines ("no local disk in the durability
path"). Redpanda and AutoMQ both accept this cost to preserve exactly-once
semantics with less new design; this project already has a different
mission-level answer to that tradeoff.

**Option A, separately-hosted external metadata service** (WarpStream's Cloud
Metadata Store on DynamoDB Global Tables / Spanner, or an externally-run
FoundationDB/TiDB deployment). Rejected per doc 15 §6: it is WarpStream's
business-model choice (they sell a managed service), not a technical
requirement of the architecture pattern, and it reintroduces "the
external-dependency adoption tax" this project has already declined to pay.
`architecture.md`'s crate map already commits to `oqueue-coordinator` as
in-process; this ADR is consistent with, not a change to, that existing
commitment.

**Deciding the durable-log engine in this same ADR** (SQLite / redb / RocksDB
/ fjall / SlateDB). Rejected for now: doc 13 §6 calls for an empirical
benchmark (fork `redb-bench`, our ~40-byte layout, the `insert_sorted`
sequential-apply number) that has not been run, and M3.md does not list the
engine choice among decisions required before M3's code — only the
sequencing *shape* is required first. Picking an engine here without that
measurement would be an invented number wearing an ADR's clothes; the
seam (`MaterializedIndex`, and a matching durable-log trait) is what M3
actually needs settled, and it is.

## Consequences

**Makes easy:** many producers can PUT concurrently to the same partition
without any coordination protocol between them — the coordinator's log append
is the only serialization point, matching M3 task 4's "lets many writers PUT
concurrently without coordinating." A single allocator makes FR-11's
monotonic-gap-free property a local invariant of one component rather than a
distributed-consensus property to prove.

**Makes hard:** the coordinator is a single point of write availability for
its shard's offset stream — if it is down, no new offsets can be assigned for
any partition in that shard, even though object storage itself is still
reachable. Sharding bounds the blast radius to one shard rather than the
cluster, but does not remove it. M6 owns closing that gap (failover, recovery
scanner). This ADR
deliberately does not attempt to make the coordinator itself highly available
inside M3; FR-10's fault-injection case (kill between PUT and ack) is answered
by ordering, not by coordinator redundancy.

**Forecloses:** per-partition broker leadership and inter-broker replication
as a path to durability or ordering — consistent with, not a new restriction
beyond, `AGENTS.md`'s existing mission constraint. It also forecloses ever
using object-storage CAS as the offset-assignment hot path; a future milestone
wanting more offset-assignment throughput than one in-process allocator per
shard can give has to add shards (doc 10 #19's already-resolved answer that
metadata shards are internal and rebalanceable is what makes that an
operational knob rather than a one-way door), not reach for per-record CAS.

## Related

Doc 10 #9 (push tail deltas / pull history) is already resolved by doc 12 §4.4
and needs no ADR of its own — `M3.9` implements it directly. The concrete
`max_metadata_staleness` value that this ADR's `CoordinatorEpoch` and
staleness machinery (Decision point 4) will be enforced against is **not set
here and is not set yet**: it is doc 10 #11, `M3.2`'s to record as `ADR-0021`.
