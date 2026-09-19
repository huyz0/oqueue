# 0046. The metadata log lives in object storage, one conditional write per append

Status: accepted — ⚠️ **taken without the user's answer**. The M6 opening
session put the choice to the user (log in object storage, log on local disk
with standbys, or no durable log in M6) and got no reply; this ADR records the
recommended option so M6 could proceed. It is reversible until M6's first
implementation commit lands, and a user decision overrides it.
Date: 2026-09-19
Requirements: FR-51 (no data loss on node loss), NFR-20, NFR-44 (a node
starts with an empty disk), FR-21 (offsets survive a restart), NFR-1 (produce
p99 ≤ 500 ms — the requirement this puts at risk)

## Context

`ADR-0020` point 5 left the metadata log's durable storage open, and M3
shipped `FakeMetadataLog`, an in-memory `Vec`, as the only implementation. So
a coordinator restart loses every position while the data objects survive, and
every leg of M6's completion condition needs a log that outlives the process.

Doc 13 §1 says the source of truth is a *replicated* log. This project's
constraints close the usual ways of getting one: `mission.md` forbids local
disk in the durability path and `AGENTS.md` forbids inter-broker replication,
which rules out a local engine made durable by replicating it between
coordinators; `ADR-0020` rejected a separately hosted metadata service (doc 15
§6). What remains durable, shared, and already a dependency is object storage.

## Decision

1. **The log is a sequence of segment objects**, one per `append` call:
   `meta/<shard>/log/<seq>`, `seq` contiguous from the base, each holding the
   batch of entries that append carried. Written with
   `Precondition::IfAbsent`, so an `append` is durable exactly when its PUT
   returns, which is `MetadataLog` guarantee 1 held for the first time.
2. **The conditional write is the fence.** Two coordinators that both believe
   they lead race for the same next `seq`; one PUT succeeds and the other is
   refused, and a refused append is a coordinator that must stop serving.
   The lease (M6 task 13) bounds how long a deposed leader keeps *trying*.
   ⚠️ **Amended by `M6.17`: once pruning exists (point 4), the conditional
   write alone is not the fence.** A pruned segment key is absent again, so a
   writer whose view predates the prune is not refused — M6's closing review
   found a node that booted while another led, took the lease later, and
   wrote at a deleted key behind the new base, reusing acknowledged offsets.
   The coordinator now replays from a freshly opened view under each lease
   term before its first write, and a refused append forces that replay.
   ⚠️ **And a refused append does not stop the coordinator** as this point
   first said; it answers `Journal` and replays before its next write.
3. **Group commit is what makes this affordable** — ⚠️ **once it is built,
   which M6 did not do**: `M6.2` was handed on at M6's close, `run` still
   journals one request per append, and every acknowledgement pays one object
   write today, which NFR-1 carries until it lands. What follows is the
   design, not the current behaviour. `CoordinatorLoop` drains
   every queued request into one `append` (M6 task 7b), so the object-storage
   write is paid once per window, not once per commit. ⚠️ **It is still paid on
   the produce-ack path**, and that is this decision's cost: one object PUT is
   added to every acknowledgement. `ADR-0020` rejected per-record CAS on this
   exact ground; batching removes the throughput ceiling and not the latency
   floor. NFR-1 is the requirement at risk, and M14 measures it.
4. **Snapshots are objects, and a base generation says where they are.**
   ⚠️ **Amended by `M6.4`** — the first draft appended a `SnapshotCommitted`
   record and CAS'd one `base` object in place; `get` returns no precondition
   token, so in-place CAS was not available. A checkpoint instead writes
   `meta/<shard>/snap/<version>` (every live entry, in the segment format),
   then a new `meta/<shard>/base/<generation>` only if absent — the commit
   point, naming the first segment the snapshot does not cover — and only
   then deletes the segments and snapshot it replaced. A writer checks its
   own next segment is still absent before committing a generation, so a
   stale writer cannot point the base behind a newer one's pruning.
   ⚠️ **The snapshot holds entries, not a serialized index**: opening is one
   snapshot GET plus the tail's segments, and the fold then replays every
   entry in memory. What this bounds is object-storage round trips, not
   replay CPU; a snapshot of the materialized state itself is not built.
5. **Opening a log is: find the newest base generation (doubling, then
   bisecting — O(log n) GETs), GET its snapshot, then GET segments forward
   until one is absent.** No LIST (`mission.md`), and the number of
   GETs is bounded by the replay tail pruning keeps short (doc 13 §10.7).
6. **Doc 10 #12, the materialized-state engine, is not needed for M6 and is
   deferred to M14.** The materialized index stays in memory (`MemoryIndex`);
   cold start is one snapshot GET plus a bounded replay, and no gate leg
   depends on a local engine's open time. The benchmark doc 13 §6 asks for
   belongs with M14's cost and performance measurements.

## Alternatives considered

**The log on local disk (redb), hot standbys following it.** Fastest ack, but
the log is only as durable as one disk unless coordinators replicate it to each
other — the inter-broker replication the mission declines.

**No durable log in M6.** Leaves nearly every M6 gate leg unmeetable, so the
milestone would be almost entirely deferral.

**A CAS-by-version object per record** (`ADR-0020` Option B). Rejected there,
and still rejected: a 200 ms–1 s floor per *record* is fatal; per *append*,
amortized by group commit, is the cost accepted above.

## Consequences

**Makes easy:** a node with an empty disk starts from object storage alone
(NFR-44); fencing a deposed coordinator needs no extra protocol; a follower
bootstraps from the same snapshot object a cold start uses (M6 task 9a).

**Makes hard:** produce latency gains one object PUT per group-commit window,
against NFR-1's budget. Opening a log costs one GET per segment in the replay
tail, so pruning is not optional (cleanup defaults on, M6 task 5).

**Amends `ADR-0020`'s "Forecloses"**, which forecloses object-storage CAS
"as the offset-assignment hot path": what it rejected is a conditional write
*per record*. One conditional write per group-commit batch, by the one
coordinator that holds the shard, is the durability of the log that
coordinator appends to, not the assignment mechanism.
