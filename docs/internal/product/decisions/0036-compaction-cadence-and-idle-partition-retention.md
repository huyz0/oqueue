# 0036. Compaction cadence from read amplification, retention from an expiry heap

Status: accepted
Date: 2026-09-17
Requirements: FR-33, FR-34

## Context

`M5.md`'s "Decisions required first" table names two open questions that must
be answered before any of this milestone's code, doc 10 #18 and #22. Both are
about a per-partition cost that is **fixed regardless of throughput**, which is
the property that makes them scale decisions rather than tuning.

**#18 — compaction cadence at high partition counts.** Doc 14 §7's synthesis
prices one manifest PUT per partition per compaction round at 100,000
partitions:

| Cadence | Manifest PUTs/s | S3 cost/day (@ $0.005/1k) |
|---|---|---|
| 60 s | 1,667 | ~$720 |
| 5 min | 333 | ~$144 |
| 30 min | 56 | ~$24 |

Linear in partitions × cadence. Doc 14 §7's friction 4 adds the hard ceiling
underneath it: measured successful conditional writes per second on a single
object are ~15 on S3, ~10 on Azure Blob, and **~1 on GCS** — ample for a
compaction-paced writer and fatal for anything near tail write rates.

**#22 — retention on idle partitions.** Doc 14 §9.2f records this as one of two
per-partition cost classes turbopuffer never pays: their background work is
dispatched by WAL-versus-index progress, so an idle namespace costs nothing,
while Kafka's `retention.ms` must fire on a partition nobody writes to. FR-33's
own acceptance criterion is exactly that — data ages out with no write to
trigger it. Doc 10 #22 names two candidates, lazy evaluation on next access and
folding into the daily reconciliation sweep, and says neither is worked through.

## Decision

**1. Compaction is triggered by read amplification per partition, and the
cadence is only the rate at which candidates are *looked for*.** The sweep
interval is a constant, `COMPACTION_SWEEP_INTERVAL = 30 min`. ⚠️ **It is not
chosen on doc 14 §7's cost table**, because decisions 1 and 2 make that table
independent of it: a sweep that finds no candidate costs no object-storage
operation at all. What the interval actually buys is **detection latency** — a
partition that crosses the read-amplification threshold just after a sweep
serves at that amplification for up to one interval — and 30 min is chosen
against the retention and compaction timescales the rest of this milestone
works in, not against a PUT bill. Candidate selection reads the coordinator's in-memory index and
costs no object-storage operation at all; a partition that is not a candidate
costs zero PUTs in that round. The table above therefore prices the **worst
case** — every partition compacting every round — rather than the steady state,
and the number that actually scales with partition count is a memory scan.

**2. No manifest is ever written by the produce path, and no partition gets a
per-round manifest PUT for having been looked at.** A manifest PUT happens only
when a plan commits. This is doc 14 §7 friction 4's own stated boundary, and it
is what keeps GCS's ~1 CAS/s per object off the critical path.

**3. Retention on idle partitions is driven by a per-partition expiry heap in
the coordinator, not by lazy evaluation and not by a full sweep.** Each
partition carries a deadline derived from the index's `ts_min` plus the topic's
retention, and the coordinator holds those deadlines in a min-heap. A retention
round pops only what has expired, so the cost is O(partitions actually
expiring), not O(partitions). The heap is rebuilt from the materialized index on
coordinator start, so it is derived state and never a second source of truth.

**4. Both constants are constants, not per-deployment knobs** (non-negotiable
2): `COMPACTION_SWEEP_INTERVAL = 30 min` and the retention round's own tick.
`deletion_delay` and the GC inequality's other two terms stay this milestone's
to set, in the task that builds the inequality — `ADR-0021` has already fixed
`max_metadata_staleness` at 5 s, which is that inequality's floor.

## Alternatives considered

**A fixed wall-clock cadence at 60 s, compacting every partition each round.**
Rejected on doc 14 §7's own number: ~$720/day at 100k partitions in request
charges alone, for work that at the tail is rewriting data still served from
cache. It is also the case `M5.md`'s risk section names — compaction does not
always pay, and a fixed cadence is the mechanism that assumes it does.

**A shorter look-only sweep — 1 min or 5 min.** Not rejected on cost, since
decision 1 makes a sweep that finds nothing free, but not chosen either: the
scan it repeats is over the whole in-memory index, so at `M7`'s partition
counts a 1 min sweep is a coordinator CPU cost paid 30 times over to shorten a
detection latency nothing has yet said is too long. ⚠️ **This is the
alternative to revisit first**, and the 404-on-read metric is not the signal
for it — the signal is a read-amplification distribution whose tail sits above
the threshold for a large fraction of an interval, which is a measurement `M14`
is the milestone for.

**A cadence adaptive to partition count** — slow the sweep as partitions grow,
so the PUT rate stays flat. Rejected because it makes the cadence a function of
a quantity that changes underneath a running plan, and because decision 1
already removes the coupling it exists to manage: with candidacy driven by read
amplification, partition count moves the cost of a memory scan, not the PUT
rate. Worth revisiting at `M7`, when shards make partition count an operational
knob and there is something real to measure.

**Triggering on object count rather than read amplification.** Rejected, and
`M5.md` task 1 forbids it outright: object count is a coordinator cost, object
bytes are a cloud-bill cost, and a partition with many objects that are never
read together has no read amplification to fix. Compacting it spends PUTs to
improve a number nobody observes.

**Lazy retention evaluation on next access (#22's first candidate).** Rejected
because it cannot meet FR-33: a partition nobody writes to and nobody reads is
never accessed, so its data never ages out. It is the natural answer for a
system whose idle units cost nothing, which is precisely the turbopuffer
property doc 14 §9.2f says does not transfer.

**Folding retention into the daily reconciliation sweep (#22's second
candidate).** Rejected as the *primary* mechanism on precision rather than on
cost: a daily sweep gives day-granularity retention, and `M5.md`'s own risk
section already records that day-granularity expiry is why lifecycle policies
cannot carry anything with an SLA. The daily sweep is kept for what it is good
at — orphan reconciliation against a storage inventory, `M5.md` task 20 — and
the heap carries retention.

**A per-partition timer.** Rejected at scale: one timer per partition at 100k
partitions is 100k wakeups the heap replaces with one.

## Consequences

**Easy.** A partition that is read only at the tail, or whose data is already
contiguous, is never compacted and costs nothing — the three cases `M5.md`'s
risks section says compaction does not pay for fall out of decision 1 rather
than needing to be special-cased. An idle partition's retention fires on
schedule with no write and no access, which is FR-33's acceptance criterion
stated as a mechanism.

**Hard.** The expiry heap is derived state that must stay consistent with the
index across compaction, trim, and retention-config changes — every one of
those moves a deadline, and a stale heap entry is a partition that expires late
or not at all. The rebuild-on-start path is what bounds that, and it is the
thing this decision makes a test target rather than an assumption.

**Foreclosed for now.** A dynamic cadence is not available without revisiting
this record, and read amplification becomes the single input to compaction
planning — a compaction type motivated by something else (key-based log
compaction, tiering) needs its own trigger and its own decision, which is where
`M5.md`'s deferred list already puts both.
