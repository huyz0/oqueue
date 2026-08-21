---
title: "Coordinator Durability, Restart, and Recovery"
slug: coordinator-recovery
status: draft
last_updated: 2026-08-13
tags: [recovery, snapshots, checkpoints, kip-630, kraft, raft, openraft, etcd, slatedb, redb, rocksdb, fjall, sqlite, metastable-failure, jepsen, availability, ripcord]
related: [12-object-discovery-and-api-cost, 06-distributed-systems-design-challenges, 05-rust-ecosystem, 01-warpstream-architecture, 08-turbopuffer-lessons]
summary: >
  How the metadata coordinator survives node kill and restart. The two-layer
  durability model; three restart scenarios with very different RTOs; why
  snapshot beats compaction (KIP-630); the correction that KRaft does NOT
  avoid enumeration and the pointer-in-the-log design that does; checkpoint
  cadence tuning from KRaft/etcd/openraft/Delta/Flink; embedded storage-engine
  selection for the materialized index (SlateDB vs redb vs RocksDB vs fjall
  vs SQLite); availability during failover; and the metastable failure modes
  (Jepsen/Bufstream lease expiry, startup-dependency traps) that must be
  designed against.
---

# Coordinator Durability, Restart, and Recovery

> **⚠️ CORRECTION (2026-08-13) — AutoMQ claims in this document.** Statements here about AutoMQ's **EBS-backed WAL** (sub-10ms produce latency) and **EBS Multi-Attach failover** ("within milliseconds") were taken from AutoMQ's blog and **do not match their shipping code**. A source study at commit `eccde72` found the block-device WAL removed entirely (`BlockWALService`, `SlidingWindowService`, `WALBlockDeviceChannel` — zero hits; the WAL factory throws on any protocol but S3). Failover is now "a healthy broker re-uploads the dead broker's WAL objects from shared storage", with a **1-minute minimum grace period** and **one failover at a time cluster-wide**. Per their 1.5.0 notes, EBS support moved to their Enterprise Edition. See [16-automq-deep-dive.md](16-automq-deep-dive.md) §2 for the full correction list.


*Compiled 2026-08-13 from three parallel research threads: Kafka/Raft snapshot mechanics, Rust embedded storage engines, and peer-system recovery in practice.*

Sections are marked **[Documented]** (stated in a primary source, cited inline), **[Vendor claim]** (asserted by a vendor without independent verification), or **[Synthesis]** (this corpus's own reasoning). Where a figure is not published, that is stated rather than estimated.

> **How this fits the project:** [12](12-object-discovery-and-api-cost.md) establishes that the coordinator holds the authoritative offset→object index and that readers cache it. This document answers what happens when a coordinator node dies. It is the availability counterpart to [12](12-object-discovery-and-api-cost.md)'s cost analysis, and it feeds open questions #8 (index granularity) and #11 (bounded staleness) in [10](10-open-questions.md).

---

## 1. The two-layer model, and the invariant that makes recovery possible

**[Synthesis, grounded in documented designs]** Every system surveyed splits coordinator state into two layers with different durability properties:

| Layer | What it is | On node kill |
|---|---|---|
| **Source of truth** | A replicated log — Raft group, or a Kafka-topic-shaped log (KIP-1164's `__diskless_metadata`) | Survives. Committed = replicated to quorum. |
| **Materialized state** | Local queryable index (SQLite / redb / RocksDB / an object-storage LSM) | Rebuildable. Losing it costs *time*, not *correctness*. |

KIP-1164 states this without hedging: "The SQLite DB will be a local metadata cache, **not the source of truth**. It could be dropped when needed to be refilled again from the log and snapshots" ([KIP-1164: Diskless Coordinator](https://cwiki.apache.org/confluence/display/KAFKA/KIP-1164:+Diskless+Coordinator)). WarpStream describes the same discipline: "Every Virtual Cluster metadata operation is journaled to our strongly-consistent log storage system **before** being executed by a Virtual Cluster replica and acknowledged back to the Agent" ([WarpStream Architecture](https://docs.warpstream.com/warpstream/overview/architecture)).

So a node kill is never data loss. The entire question is **how long until something can serve again** — and the answer differs by an order of magnitude depending on which of three scenarios you're in (§2).

**⚠️ Note on the KIP-1164 citation.** The KIP was **renamed** from "Topic Based Batch Coordinator" to "**Diskless Coordinator**"; the old URL returns a hard 404. This explains the retrieval failures noted in [12](12-object-discovery-and-api-cost.md) §3.3 and §9 — that document's citation should be read as pointing at the renamed page. One substantive difference between revisions: the older text explicitly described "read-only Batch Coordinator instances running on all brokers that are in-sync replicas"; the current revision drops the named role and instead expresses it per-API — `DisklessFindBatches` "could be served from stale state and by followers," while `DisklessListOffsets` requires strict consistency. **[Synthesis]** The design intent looks unchanged (ISR members materialize the log and can be promoted warm), but it's now a consistency contract rather than a role.

---

## 2. Three restart scenarios, wildly different costs

**[Synthesis]** This distinction is the single most important thing in this document, and it is where the naive framing of "do we need checkpoints?" goes wrong. Checkpoints are the answer to only *one* of these:

**(a) Warm restart** — planned rolling restart, local materialized store intact at `applied_upto`. Replay only `committed − applied_upto`: seconds of log. No checkpoint involved.

**(b) Hot-standby failover** — node killed, but other replicas were already materializing the log continuously. Failover is a *promotion*, not a rebuild. Recovery ≈ leader-election time. **This is the path that should handle routine node kills.**

**(c) Cold start** — new node, empty disk, or disk loss. The only scenario that actually needs a snapshot, and it is a capacity-add/disaster path where tens of seconds is acceptable.

The architectural consequence: **do not let failover depend on cold rebuild.** Keep hot standbys, and checkpoint cadence stops being a latency-critical parameter and becomes a cost/convenience one.

This is exactly how KRaft achieves what Confluent calls "near-instantaneous controller failover" — every controller in the quorum is already materializing the log, so promotion requires no state load ([Confluent KRaft docs](https://docs.confluent.io/platform/current/kafka-metadata/kraft.html)).

**A caution on the published figures.** WarpStream's ~10s multi-region failover ([No record left behind](https://www.warpstream.com/blog/no-record-left-behind-how-warpstream-can-withstand-cloud-provider-regional-outages)) is scenario (b) — failover to an already-warm region — not scenario (c). **[Documented]** WarpStream publishes **no** RTO figure for single-region replica rebuild, which is the case most analogous to our scenario (c). Treat (b) and (c) as separate numbers, design for both, and measure both.

---

## 3. Snapshot, don't compact

**[Documented]** KIP-630 rejects log compaction for `__cluster_metadata` for four reasons, all of which apply to an offset-assignment coordinator ([KIP-630: Kafka Raft Snapshot](https://cwiki.apache.org/confluence/display/KAFKA/KIP-630:+Kafka+Raft+Snapshot)):

1. **Write amplification.** Compaction is O(data + tombstones); snapshotting is O(data). Their worked example: 100 MB of state changing at 1 MB/s needs ~5 MB/s of I/O under compaction vs. 1 MB/s under snapshotting.
2. **The state machine is event/delta-shaped, not key-value-shaped.** "We don't believe that a key-value and offset based compaction model can be used for events/deltas in a way that it is intuitive to program against."
3. **Tombstone-driven divergence.** Tombstones "are deleted after a timeout and are not guarantee[d] to have been replicated to all of the replicas before they are deleted."
4. **Network efficiency of deltas.** Their example collapses 113.29 MB of replication traffic into 0.77 KB.

**[Synthesis]** Point 2 is the one that decides it for us. Our log carries "batch committed at offsets [a,b) → object key + byte range," plus deletion and compaction events. That is precisely the event/delta shape KIP-630 says compaction handles badly — compaction keys would have to be synthetic and merge semantics non-obvious.

**[Documented]** KIP-1164 independently reached the same conclusion for the diskless coordinator, and says so explicitly: snapshots are taken periodically, followers fetch them, and once an offset is in a snapshot it can be pruned — "**This mechanism is identical to the one in KRaft (see KIP-630)**" ([KIP-1164](https://cwiki.apache.org/confluence/display/KAFKA/KIP-1164:+Diskless+Coordinator)).

### The SQLite snapshot trick

**[Documented]** KIP-1164's mechanism for taking a *consistent* snapshot without pausing writes is worth stealing: "To make an asynchronous snapshot point-in-time consistent snapshot, **SQLite read-only transactions or the backup API** will be used."

**[Synthesis]** This is a clean answer to Raft's classic snapshot-pause problem. The Raft paper names copy-on-write as the fix — either `fork()` or persistent data structures ([raft.pdf](https://raft.github.io/raft.pdf) §7). SQLite's WAL-mode MVCC gives you a third option where readers don't block writers, so `build_snapshot` becomes a background copy of a consistent page image with **no state-machine pause**, and the resulting artifact is a database file a recovering node can `open()` rather than deserialize.

That last property matters more than it first appears — see §6.

---

## 4. Locating the snapshot without enumeration

This is the direct answer to "do we need checkpoints to avoid LIST in recovery," and it contains a correction to a natural assumption.

### The correction: KRaft does NOT avoid enumeration

**[Documented]** KRaft snapshots are named `<EndOffset>-<Epoch>.checkpoint`, zero-padded to 20 and 10 digits respectively — e.g. `00000000000005120793-00000000000000000002.checkpoint` ([KIP-630](https://cwiki.apache.org/confluence/display/KAFKA/KIP-630:+Kafka+Raft+Snapshot), [Snapshots.java](https://raw.githubusercontent.com/apache/kafka/trunk/raft/src/main/java/org/apache/kafka/snapshot/Snapshots.java)). It is tempting to read that deterministic naming as "you can compute the newest snapshot's name." **You cannot.** `KafkaMetadataLog.recoverSnapshots()` calls `Files.newDirectoryStream(log.dir.toPath)` and selects the max `(offset, epoch)` from a TreeMap ([KafkaMetadataLog.scala](https://raw.githubusercontent.com/apache/kafka/3.9/core/src/main/scala/kafka/raft/KafkaMetadataLog.scala)). There is no pointer file.

What the naming actually buys is different and still valuable:
- The name **fully encodes the snapshot's log position**, so ranking candidates needs zero file opens.
- Lexicographic order equals numeric order (hence the zero-padding).
- Distinct suffixes (`.checkpoint` / `.checkpoint.part` / `.checkpoint.deleted`) make torn and tombstoned artifacts identifiable **by name alone**, so the same scan that finds the newest snapshot also garbage-collects.
- Combined with write-to-`.part`-then-atomic-rename, any file named `.checkpoint` is known complete.

**[Synthesis]** This is a fine model *when the snapshot directory is local disk* — the scan is microseconds, no network, no per-request cost. It does **not** transfer to object storage, where the equivalent is a paginated LIST with per-request cost ([12](12-object-discovery-and-api-cost.md) §1).

### What each mechanism actually does

| System | Mechanism | Enumeration on recovery? | Where |
|---|---|---|---|
| **Iceberg (catalog)** | CAS'd pointer in metastore/DB | **None** — one lookup, one GET | External catalog |
| **Iceberg (`HadoopTableOperations`)** | `version-hint.text`, advisory only | Probes forward from the hint | Object store (being deprecated) |
| **Delta Lake** | `_last_checkpoint` pointer file | **Yes, bounded** — paginated `start-after` LIST for the commit tail | Object store |
| **KRaft** | `<offset>-<epoch>.checkpoint` naming | **Yes, bounded** — directory scan | Local disk |
| **etcd** | `seq-index.wal` + `.snap` naming | **Yes, bounded** — max 5 files each | Local disk |

**[Documented]** Delta is the one most often miscited as avoiding LIST. Its spec says readers can "locate a recent checkpoint by looking at the `_delta_log/_last_checkpoint` file" — but immediately adds that the version from that file "can be used on storage systems that support **lexicographically-sorted, paginated directory listing** to enumerate any delta files or newer checkpoints" ([Delta PROTOCOL.md](https://raw.githubusercontent.com/delta-io/delta/master/PROTOCOL.md)). The pointer converts an unbounded listing into a small seeked one; it does not eliminate it.

**[Documented]** Iceberg's catalog pointer is the only genuinely enumeration-free mechanism: "The atomic swap needed to commit new versions of table metadata can be implemented by storing a pointer in a metastore or database that is updated with a **check-and-put** operation" ([Iceberg spec](https://iceberg.apache.org/spec/)). The instructive counterexample is within Iceberg itself — `HadoopTableOperations`' `version-hint.text` is explicitly advisory, so readers must be able to make progress without it, which means probing forward. That variant is [being deprecated](https://www.mail-archive.com/dev@iceberg.apache.org/msg07012.html), largely because pointer atomicity can't be guaranteed on object stores.

**[Synthesis]** The lesson: **a pointer only eliminates enumeration if it is atomic and authoritative.** If advisory, readers need a fallback, and the fallback is always enumeration or probing.

### The answer for us: put the snapshot pointer in the log

**[Synthesis]** Delta and Iceberg need pointer *files* precisely because they have no replicated log — the directory **is** their log. We have one. So:

```
SnapshotCommitted { end_offset, epoch, object_key }   ← a record in the metadata log
```

A recovering node reads the log (which it must do anyway), finds the newest `SnapshotCommitted`, issues **one GET** for that exact key, and replays forward. Zero enumeration, zero external dependency, and the pointer is automatically consistent with the log position it describes because it *is* a log position. The log already provides linearizable CAS semantics, so there is no second system to keep in sync.

This dominates the alternative of CAS-ing a fixed-key pointer object in object storage: same guarantee, one fewer moving part, and no hot fixed key.

**The invariant to enforce**, stated in our terms and modelled on KIP-630's ([KIP-630](https://cwiki.apache.org/confluence/display/KAFKA/KIP-630:+Kafka+Raft+Snapshot)):

> If `log_start_offset > 0`, there exists a committed `SnapshotCommitted` record naming a snapshot whose `end_offset ≥ log_start_offset`.

Check it before ever advancing `log_start_offset`. That single invariant makes recovery-without-enumeration a structural guarantee rather than an optimization — and it is the coordinator-side analogue of the "never let an object be discoverable only by listing" rule from [12](12-object-discovery-and-api-cost.md) §8.1.

---

## 5. Checkpoint cadence

### Defaults as revealed preference

**[Documented]**

| System | Trigger | Default |
|---|---|---|
| KRaft | bytes in log since last snapshot | **20 MiB** (`metadata.log.max.record.bytes.between.snapshots`) |
| KRaft | elapsed time | **1 hour** (`metadata.log.max.snapshot.interval.ms`, added by [KIP-876](https://cwiki.apache.org/confluence/display/KAFKA/KIP-876:+Time+based+cluster+metadata+snapshots)) |
| KRaft | total log+snapshot retention | **100 MiB / 7 days** |
| etcd | applied Raft entries | **10,000** in v3.6 — *reduced from 100,000* |
| openraft | log entries since last snapshot | **5,000** (`SnapshotPolicy::LogsSinceLast`) |
| Delta Lake | commits | **10** OSS / **100** on DBR 11.1+ |
| Flink | — | **no default; checkpointing is off until enabled** |

([Kafka broker configs](https://kafka.apache.org/41/configuration/broker-configs/), [etcd v3.6 config](https://etcd.io/docs/v3.6/op-guide/configuration/), [openraft config.rs](https://raw.githubusercontent.com/databendlabs/openraft/main/openraft/src/config/config.rs), [Databricks KB](https://kb.databricks.com/delta/vacuum-best-practices-on-delta-lake), [Flink checkpointing docs](https://nightlies.apache.org/flink/flink-docs-master/docs/dev/datastream/fault-tolerance/checkpointing/))

**[Synthesis]** The convergence is striking: KRaft's 20 MiB against a 100 MiB retention budget, etcd's 10k entries, openraft's 5k entries all land in the same neighbourhood. **Nobody ships a default allowing more than roughly 100 MB of replay.** If you want a rule of thumb from revealed preference: *size cadence so the replay tail stays in the tens of megabytes.*

**Two default changes are worth more than the defaults themselves**, because they encode operational experience:

- **etcd went 10k → 100k → back to 10k** after memory-pressure experience ([etcd #13889](https://github.com/etcd-io/etcd/issues/13889)). The most battle-tested Raft system decided *more frequent* snapshotting was the better default.
- **Delta went 10 → 100** on DBR 11.1+, and the published reasoning is not write cost: "fewer checkpoint files are created. With less checkpoint files to index, the **faster the listing time in the transaction log directory**" ([Databricks KB](https://kb.databricks.com/delta/vacuum-best-practices-on-delta-lake)).

**[Synthesis]** Those two point in opposite directions, and the reason is that they're optimizing different things — etcd for memory, Delta for *metadata directory cardinality*. Delta's reasoning is the one that transfers to an object-storage design: checkpoint frequency is a directory-cardinality problem as much as a write-amplification problem. Our design sidesteps it (the snapshot pointer is in the log, so no directory is ever scanned — §4), which means we can afford to lean toward etcd's frequent end without paying Delta's cost.

### The tradeoff, quantified — Flink's changelog backend

**[Documented]** This is the best-quantified checkpoint tradeoff published anywhere, and it is a warning ([Flink: generic log-based incremental checkpoints](https://flink.apache.org/2022/05/30/improving-speed-and-stability-of-checkpointing-with-generic-log-based-incremental-checkpoints/)):

| Metric | Without changelog | With changelog |
|---|---|---|
| Checkpoint duration p50 | 5 s | **311 ms** |
| Checkpoint duration p99.9 | 10 s | **1 s** |
| **Recovery time** | 20–21 s | **35–65 s (+66% to +225%)** |
| Storage amplification (ValueState) | — | +30% |
| Storage amplification (Window workload) | — | **45×** |

Flink's own summary of the third row is unusually honest: "**Increased recovery time**, which may or may not be compensated by more frequent checkpoints."

**[Synthesis]** That sentence is the crux of the whole topic. Faster checkpoints let you checkpoint more often, shrinking the replay tail — but if each checkpoint is a *diff chain*, restore cost grows. There is a genuine optimum, it is workload-dependent, and the 45× storage amplification shows it can be catastrophically bad in some shapes.

### The pause hazard

**[Documented]** Flink's most transferable operational guidance is to tune the *pause between* checkpoints rather than the interval: "it is often easier to configure applications by defining the 'time between checkpoints' than the checkpoint interval, because the 'time between checkpoints' is not susceptible to the fact that checkpoints may sometimes take longer than on average" (`execution.checkpointing.min-pause`). The failure mode it prevents: when checkpoints "end up frequently taking longer than the base interval," the system gets **stuck in continuous checkpointing** ([Flink large-state tuning](https://nightlies.apache.org/flink/flink-docs-master/docs/ops/state/large_state_tuning/)).

**[Synthesis]** Directly applicable. If snapshot duration grows with state size and you schedule on fixed wall-clock, you can cross into a regime where you are always snapshotting. **Schedule on completion + pause, not on a fixed period.**

### Trigger on journal volume, not wall-clock

**[Synthesis]** Our command rate is roughly proportional to produce throughput. A fixed *time* interval therefore yields a **load-dependent RTO** — recovery gets slowest exactly when you're busiest. Triggering on accumulated journal bytes/commands makes RTO load-independent. This is the reasoning behind Delta's commit-count-based interval and KRaft's byte-based trigger, and it argues against openraft's entry-count-only default if our entries vary much in size (a batch-commit record vs. a compaction record).

### No published interval→RTO formula exists

**[Documented]** Flink deliberately declines to give interval numbers, offering a problem-driven method instead. The only rigorous statement is the identity: **RPO ≈ checkpoint interval**; **RTO ≈ (state reload time) + (replay since last checkpoint)**. Academic models exist ([ACM DL](https://dl.acm.org/doi/abs/10.1007/s11036-020-01729-7), [arXiv:1911.11915](https://arxiv.org/pdf/1911.11915)) but optimize throughput utilization, not an SLA-driven RTO target.

---

## 6. Choosing the materialized-state engine

The engine choice is a recovery decision, because **cold-start time is dominated by replay/load rate**, not by steady-state write throughput. At our modeled ~4,000 entries/sec the coordinator writes ~160 KB/s — trivial for every engine below. The binding constraint is how fast it can *apply* entries during replay.

### The two anchors for state-reload speed

**[Documented]** Only two hard numbers exist across all the surveyed systems:

- **etcd: ~2 GB of state recovers in ~20 s on good hardware** — the reasoning behind their 8 GiB quota, arrived at via MTTR ([etcd #15354](https://github.com/etcd-io/etcd/issues/15354)). ≈ **100 MB/s**.
- **KRaft: a 100,000-topic metadata snapshot loads in 110.8 ms**, rising to **219.7 ms** after switching to persistent collections ([apache/kafka PR #13280](https://github.com/apache/kafka/pull/13280)).

**[Synthesis]** These are consistent — KRaft's image at 100k topics is tens of MB, loading in ~100–200 ms, the same ~100 MB/s ballpark. **Planning heuristic: budget ~100 MB/s for snapshot load** when the design deserializes into memory, and size max state so `(state ÷ 100 MB/s) + replay tail` fits the RTO target. For a 5-second failover budget that implies keeping materialized state under ~500 MB — *unless* restore is `open()` rather than deserialization, which is exactly why KIP-1164 chose SQLite for state expected to reach "hundreds of megabytes or even gigabytes."

**The KAFKA-14735 tradeoff is the cautionary tale.** Switching to persistent/immutable collections made incremental mutation orders of magnitude faster (31.1 ms/op → ~500 ns/op at 100k topics) but **doubled snapshot load time**. Steady-state mutation speed and cold-load speed pull in opposite directions; pick deliberately.

### Engine comparison

Insert rates below marked *(derived)* are computed from published millisecond timings in redb's benchmark harness — the only neutral, same-hardware, same-workload comparison available. Workload: **24-byte random keys, 150-byte values**, Ryzen 9950X3D + NVMe ([redb README](https://raw.githubusercontent.com/cberner/redb/master/README.md)). Not our 40-byte entries, and *random* rather than sequential.

**Bulk load — 5M records, single transaction** (proxy for unbounded replay):

| Engine | Time | Rate *(derived)* |
|---|---|---|
| LMDB | 9,232 ms | ~542k/s |
| RocksDB | 13,969 ms | ~358k/s |
| SQLite | 15,341 ms | ~326k/s |
| redb | 17,063 ms | ~293k/s |
| fjall | 18,619 ms | ~269k/s |

**Batch writes — 100 durable commits × 1,000 records** (proxy for *checkpointed* replay, where you commit periodically so progress survives a crash mid-replay):

| Engine | Time | Rate *(derived)* |
|---|---|---|
| fjall | 353 ms | ~283k/s |
| RocksDB | 451 ms | ~222k/s |
| LMDB | 942 ms | ~106k/s |
| redb | 1,595 ms | ~63k/s |
| SQLite (rollback journal) | 2,625 ms | ~38k/s |

**[Synthesis]** Note the **ordering inversion**: redb wins single-record commits by 2× and loses batched commits by 4.5×. A copy-on-write B-tree pays a fixed per-commit price and amortizes poorly; LSMs pay almost nothing per record but need a commit to amortize over. **Our replay strategy decides which column matters** — commit every N≥1,000 entries and the LSMs win decisively.

⚠️ **Two methodology caveats on that harness**, read from its source: the SQLite adapter **never sets `journal_mode = WAL`**, running in default rollback-journal mode throughout — so the SQLite column is materially pessimistic. And RocksDB is run through `OptimisticTransactionDB`, a heavier path than plain `DB`. Independent figures: RocksDB's official bulk load is **1,003,732 ops/s** ([RocksDB wiki](https://github.com/facebook/rocksdb/wiki/Performance-Benchmarks)); SQLite in proper WAL mode with `synchronous=NORMAL` sustains **~80,000 inserts/s** ([SQLite in Production](https://shivekkhurana.com/blog/sqlite-in-production/)).

### Summary

| Engine | Maturity | License | Recovery behavior | Key risk |
|---|---|---|---|---|
| **SlateDB** 0.15.0 | Pre-1.0, CNCF Sandbox, 19 named adopters (Dropbox, Prisma, s2-lite, Responsive). **No published stability statement.** | Apache-2.0 | **Near-instant open** — work ∝ un-compacted WAL tail, not DB size. Zero-copy O(1) checkpoints. | Cache miss = **50–100 ms on S3 Standard** (5–10 ms on Express One Zone) — 5–10× our p99 budget. [#1302](https://github.com/slatedb/slatedb/issues/1302): **scan latency grows with record count, closed unresolved.** |
| **redb** 4.1.0 | Stable since 2023, stable file format, 16 open issues | MIT/Apache-2.0 | **Quick repair is O(1)**; full repair walks live trees. No WAL to replay. | Worst batched-write rate; largest on-disk footprint (4.00 GiB vs RocksDB's 893 MiB). v4.1.0 fixed two corruption bugs. |
| **RocksDB** (`rocksdb` 0.24.0) | Battle-tested C++; **crate last released Aug 2025**, 188 open issues | Apache-2.0 | WAL replay bounded by `max_total_wal_size`. **`kPointInTimeRecovery` is the right mode** for a log-backed coordinator. | Compaction/L0 tuning burden; ~700k LoC C++; ~40 s hello-world compile; blocking calls need `spawn_blocking`; no Rust-side profiling into C++ frames. |
| **fjall** 3.1.8 | v3 format <1 yr old, single primary maintainer, no independent production reports found | MIT/Apache-2.0 | Single DB-level journal, replayed on crash. **Default durability = OS buffers only.** | Youngest option; benchmark story leans on author's own numbers — though it *wins batch writes in a competitor's harness*. |
| **SQLite** (`rusqlite` 0.40.2) | Maximally mature; **chosen by KIP-1164 for exactly this problem** | MIT / public domain | WAL recovery ∝ WAL size, not DB size. **Restore is `open()`, not deserialization.** | Checkpoint starvation under continuous readers; **transactions >1 GB can fail** so replay must be chunked; one writer at a time. |

**[Documented]** 4,000 inserts/sec is ~5% of SQLite's durable WAL-mode capacity — comfortable, *with transaction batching*. The individual-write figure (~142 commits/s) would **not** meet our rate, so batching is mandatory, not optional.

### The SlateDB bet

**[Synthesis]** SlateDB — an LSM whose SSTables live directly in object storage — is the most interesting option because it appears to dissolve the checkpoint problem: a new node just opens the DB. The honest assessment is that **it eliminates the checkpoint *write* problem completely and the recovery problem substantially, but moves cost into read latency, and the movement is not small.**

What genuinely goes away: snapshot creation becomes O(1) (a UUID + manifest ID + expiry, per [RFC 0004](https://slatedb.io/rfcs/0004-checkpoints/)); recovery work is bounded by the un-compacted WAL tail rather than state size; snapshot *distribution* disappears entirely since every reader sees the same bytes.

What it costs: **[Documented]** cache hits respond in "<1ms" but misses see "latency spikes similar to object storage latency levels (50-100ms for S3 standard)" ([SlateDB Tuning](https://slatedb.io/docs/operations/tuning/)). SlateDB's own benchmarking concedes "p99 latencies drag our throughput well below RocksDB." An independent test found **RocksDB "more than 2x faster in raw read/write throughput"** ([Nixiesearch](https://nixiesearch.substack.com/p/benchmarking-slatedb-vs-rocksdb)).

**[Synthesis]** So the bet is really about *which* recovery metric you're optimizing. It converts "time until the process accepts queries" from minutes to seconds, and converts "time until the process serves at 10 ms p99" into a **cache-warming problem whose duration nobody has published**. A node that opens instantly but then serves 50–100 ms p99 for ten minutes has not recovered. You have not removed local state — you have made it a cache instead of a source of truth, which is the checkpoint problem in a new costume. `preload_disk_cache_on_startup` exists precisely because of this.

Three further findings that bear on it:
- **[Documented]** `DurabilityLevel::Local` is **not implemented** — there is no local-disk WAL, so you cannot trade S3 write latency for local durability inside SlateDB today.
- **[Documented]** [Issue #684](https://github.com/slatedb/slatedb/issues/684) documents slow startup for *low-throughput* databases, where infrequent memtable flushes let hundreds of tiny WAL SSTs accumulate. Our ~160 KB/s is exactly that regime. Fixed via PR #699, but the shape of the failure is instructive: **SlateDB startup is fast in proportion to how aggressively you flush.**
- **[Documented]** [Issue #1302](https://github.com/slatedb/slatedb/issues/1302): scan latency grows with record count while point ops stay flat; bloom filters don't help scans. **Closed without documented resolution.** Our `(topic, partition, base_offset)` range lookups are scans. Validate this hands-on before committing.

**What the field actually chose.** **[Documented]** KIP-1164 picked local SQLite + periodic snapshots. WarpStream, with the most mature commercial version of this architecture, put its metadata in **DynamoDB + S3** rather than an object-storage LSM ([AutoMQ analysis](https://www.automq.com/blog/warpstream-architecture-explained-kafka-teams)). Neither chose the SlateDB path for coordinator metadata. That is not proof it's wrong — SlateDB postdates both decisions, and Responsive's RS3 is a working counterexample at **20,000 records/sec with 2 TB of state** on a single m5.2xlarge ([RS3 docs](https://docs.responsive.dev/storage/rs3)) — but it means we would be **ahead of the published state of the practice**, not following it.

### The cheapest way to resolve this

**[Synthesis]** `crates/redb-bench` already runs redb, LMDB, RocksDB, fjall and SQLite through one interface, and already contains an `insert_sorted` benchmark (1M ascending 32-byte keys) that the published README table omits. Fork it, set key/value sizes to our 40-byte layout, fix the SQLite adapter to use WAL mode, and we get the exact head-to-head sequential-apply number that does not exist publicly — on our hardware, in an afternoon. That beats further literature search.

---

## 7. Availability during recovery

### Reads survive; writes pause

**[Synthesis, grounded in documented behavior]** The asymmetry is a strong property worth designing for explicitly:

- **Writes** pause during leader election — offset assignment needs the leader. **[Documented]** KRaft's bounds: `controller.quorum.fetch.timeout.ms` default **2000 ms** (time without a successful fetch before becoming a candidate), `controller.quorum.election.timeout.ms` default **1000 ms** ([Kafka broker configs](https://kafka.apache.org/41/configuration/broker-configs/)). So the floor is low single-digit seconds for detection plus election.
- **Reads keep working.** Agents hold their own cached index ([12](12-object-discovery-and-api-cost.md) §4.5), objects in storage are immutable, and KIP-1164 explicitly permits `DisklessFindBatches` to be served "from stale state and by followers."

A tail consumer may not see *new* offsets during the gap, but existing ones serve fine — which, per [12](12-object-discovery-and-api-cost.md) §4.3, is indistinguishable from ordinary Kafka tail behavior.

### Peer failover figures

**[Vendor claim]** unless noted:

| System | Event | Figure |
|---|---|---|
| WarpStream | Multi-region control-plane failover (region killed) | **~10 s latency spike**, no data loss, producers never stopped ([No record left behind](https://www.warpstream.com/blog/no-record-left-behind-how-warpstream-can-withstand-cloud-provider-regional-outages)) |
| WarpStream | Multi-region SLA | 99.999% ≈ **26 s/month** budget |
| WarpStream | Multi-region cost | **+80–100 ms p50** on control-plane writes |
| AutoMQ | EBS Multi-Attach WAL takeover | "**within milliseconds**"; WAL typically 500 MB; recovery upload "500 MiB within 2 seconds" |
| AutoMQ | Partition reassignment | **~1.5 s average**, "independent of the partition's data volume" ([AutoMQ](https://www.automq.com/blog/automq-achieving-auto-partition-reassignment-in-kafka-without-cruise-control)) |
| turbopuffer | Node death → replacement serving | Cold query **~500 ms**; cold/warm p50 **874 ms vs 14 ms** (~60×) ([Architecture](https://turbopuffer.com/docs/architecture)) |
| turbopuffer | Jan 2026 us-west-2 outage | **1 h 27 m** total; fix deployed 16 min in, full restoration 71 min later. **RCA not public.** |

**[Synthesis]** Note that WarpStream's ~10 s consumes roughly 40% of a monthly five-nines budget in a single event — a tight coupling worth designing around. And turbopuffer's ~60× cold-cache penalty is the clearest published measurement of the cache-warming cost that §6 flags as SlateDB's hidden risk.

### Ripcord: designing for total coordinator loss

**[Documented]** WarpStream's Ripcord mode (flag `-enableRipcord`, Agent v748+) is the most interesting artifact in this survey because it is an explicit admission that the coordinator *will* go down, plus a designed answer. It makes Agents treat every topic as a Lightning Topic — universal delayed sequencing. During a full control-plane outage ([Ripcord docs](https://docs.warpstream.com/warpstream/kafka/advanced-agent-deployment-options/low-latency-clusters), [Lazy(log)](https://www.warpstream.com/blog/the-art-of-being-lazy-log-lower-latency-and-higher-availability-with-delayed-sequencing)):

| Operation | Behavior |
|---|---|
| Produce | **Works** — journaled to object storage, durability intact |
| Fetch / consume | **Fails entirely** |
| Offsets in produce responses | Always `0` |
| Idempotent producers, transactions, consumer groups | Must be disabled / fail |
| **New or restarting Agents** | **Cannot start** |

Recovery afterward: Agents "replay the journal backlog from object storage into the topics asynchronously," with `warpstream.agent_ripcord_oldest_replay_age` and `..._outstanding_replays_count` exposed for tracking. **[Documented]** The catch-up *rate* is not published.

**⚠️ The Ripcord scanner does enumerate object storage.** "The Control Plane's scheduler periodically assigns **scan jobs** to Agents, instructing them to **list the available sequences**." Cost is bounded by writing into sequence folders of **~1,000 consecutive files per Agent**, so it's a small prefix scan rather than a bucket crawl. **[Vendor claim]** "zero increase in costs," unquantified.

**[Synthesis] The fork in the design space.** AutoMQ takes the opposite approach: a **`PREPARED` → `COMMITTED` state machine in the controller** with expiry-based GC. An object ID is allocated and recorded as `PREPARED` before upload; commit flips it to `COMMITTED`; "if an exception occurs during the upload process… the Controller will delete the objects that exceed the expiration time and are still not submitted" ([AutoMQ metadata management](https://www.automq.com/blog/insight-metadata-management-in-automq)). This **avoids object-store LIST entirely** — but requires the coordinator alive *at write time*, and is therefore fundamentally incompatible with a Ripcord-style mode. Pick deliberately:

| | WarpStream (scanner) | AutoMQ (PREPARED state) |
|---|---|---|
| Survives total coordinator outage for writes | **Yes** (Ripcord) | **No** |
| Requires object-store enumeration | Yes, prefix-bounded | **No** |
| Coordinator on the write critical path | No (in Ripcord mode) | Yes, always |

### The metadata store as availability ceiling

**[Documented]** The sharpest published critique comes from inside the community. Jack Vanlightly on KIP-1150: the Batch Coordinator "is the component that contains the most complexity, with challenges around scaling, failovers, reliability," and — the single best sentence on the topic — "**Once durability moves to shared storage, sequencing and metadata consistency become the new limits of scalability**" ([A Fork in the Road](https://jack-vanlightly.com/blog/2025/10/22/a-fork-in-the-road-deciding-kafkas-diskless-future)).

He also warns specifically against Postgres as a coordinator backend: "Given the row locks are maintained until the transaction commits, I imagine that given enough contention, it could cause a convoy effect of blocking… **I would therefore caution against the use of Postgres as a Batch Coordinator implementation.**" Worth weighing, since Aiven's Inkless MVP uses exactly that.

**[Synthesis]** The structural argument assembled: object storage is extremely available; moving durability there removes replication as a failure domain; but sequencing *cannot* live in object storage at Kafka commit rates, so it moves to a coordinator — which is now the least-available component, and **every produce and every fetch touches it**. turbopuffer is the existence proof of the alternative: object storage as its only stateful dependency, achieved by accepting **one WAL entry per second per namespace**. A Kafka-protocol broker cannot accept that per partition.

So the honest framing is not "WarpStream made a mistake" but: **any design offering Kafka-rate sequencing on object storage owns an availability ceiling set by its coordinator, and should be judged on how gracefully it degrades when that ceiling is hit.** By that standard Ripcord — with its honestly-documented limitations — is a better answer than pretending the ceiling isn't there.

---

## 8. Failure modes to design against

**[Documented]** The Jepsen analysis of Bufstream is the most valuable single source here, because Bufstream is architecturally closest to us: stateless agents + object store + external coordination service ([Jepsen: Bufstream 0.1.0](https://jepsen.io/analyses/bufstream-0.1.0)).

### Metastable failure via lease expiry (Jepsen #2)

Root cause, verbatim:

> "Bufstream uses etcd leases to keep track of active Bufstream agents… Despite setting long timeouts, brief pauses or partitions caused etcd to delete keys tied to an agent's lease, but **updates reflecting those deletions were not necessarily relayed by the client to the agent itself. In essence, an agent would be unaware that it had lost its lease.**"

Symptoms: `poll()` returning successfully with **no records**; `InitProducerId` timeouts; states that "persisted despite the cluster being healthy, requiring manual restart." Jepsen's framing: a brief interruption "could cause **long-lasting partial (or total!) unavailability**."

**[Synthesis] The defining property: the outage outlives its cause.** The trigger is transient; the resulting bad state is stable, self-sustaining, and does not clear when the trigger goes away.

Design rules that follow:
1. **Never trust watch/subscribe alone for lease liveness.** A watch stream can silently fail to deliver the one notification that matters — your own expiry. Pair it with active polling.
2. **A node must detect lease loss autonomously.** Track your own lease TTL locally and self-fence when it lapses. **Longer timeouts do not fix this** — Jepsen notes the bug occurred "despite setting long timeouts."
3. **Caches amplify metastability.** Two of the five Bufstream bugs were stale-cache problems producing *silent wrongness* (empty polls) rather than loud errors.

### The startup-dependency trap — hit independently by two systems

**[Documented]** Bufstream "requests a shared file in storage on startup, as a safety check… Bufstream exits if it can't complete this request. This caused Bufstream to crash **roughly one in thirty tests**, due to `404 not found` responses from storage." Jepsen's recommendation: processes should "**try to keep running when their dependencies are unavailable**."

WarpStream has the same class of defect documented as a known limitation: under Ripcord, "**new or restarting Agents cannot start**" because cache initialization requires control-plane connectivity.

**[Synthesis]** Same failure in both: **a hard dependency in the *startup* path converts a transient blip into a fleet that can only shrink** — in Kubernetes, with liveness probes and autoscaling and spot interruptions, exactly when you most need to replace capacity. Our bootstrap path must be able to come up degraded and retry.

### Never emit a plausible offset on an error path

**[Documented]** Bufstream bug #3: "a sent value could be assigned offset `0` (even if offset `0` had already been assigned far earlier)," triggered by an etcd or Bufstream pause. Fixed by returning a proper `-1` error offset.

**[Synthesis]** A one-line choice that turned an *availability* bug into a *safety* bug. Offset-assignment failure must be unambiguously distinguishable from "assigned offset 0."

### Fault-inject pauses, not just kills

**[Synthesis]** Every metastable finding in the Jepsen report came from **pauses and partitions**, not clean crashes. A killed coordinator is the easy case. This is a direct argument for the deterministic-simulation testing surveyed in [05](05-rust-ecosystem.md) §9 — `madsim` in particular, since it ships S3/etcd-aware simulators and can inject pauses deterministically. ⚠️ **CORRECTION (2026-08-21), from `M1.22`:** the argument for deterministic simulation stands, and this paragraph's own subject — coordinator pauses and partitions — is **broker-socket** simulation, which the spike did not rule on. What it did rule out is the *justification offered here*: `madsim` "ships S3/etcd-aware simulators" is true but does not reach this project's object store, because `madsim-aws-sdk-s3` replaces the **AWS SDK** and `ADR-0008` chose `object_store`, for which no `reqwest`/`hyper` shim exists. So madsim stays a live candidate for the sockets this paragraph is about and is not one for object-store I/O. See [05](05-rust-ecosystem.md) §9's own correction and `docs/internal/standards/testing.md`, "Deterministic simulation".

### Growth bounds must be enforced, not monitored

**[Documented]** etcd's failure mode when history isn't compacted: the quota trips and etcd "raises a cluster-wide alarm that puts the cluster into a maintenance mode which **only accepts key reads and deletes**" — read-only until an operator compacts, defragments, *and manually disarms the alarm*. Auto-compaction is **off by default**. And compaction alone doesn't reclaim disk: `etcdctl defrag` "**blocks the system from reading and writing data while rebuilding its states**" ([etcd maintenance](https://etcd.io/docs/v3.5/op-guide/maintenance/)).

Iceberg has the same shape of trap: `write.metadata.delete-after-commit.enabled` defaults to **`false`**, so metadata accumulates indefinitely unless the operator opts in ([Iceberg maintenance](https://iceberg.apache.org/docs/latest/maintenance/)).

**[Synthesis]** Two lessons. First, **cleanup defaults must be ON** — Iceberg's metadata-bloat reputation is entirely a consequence of making the safe choice opt-in, and a broker's commit rate is far higher than an analytics table's. Second, **make the growth bound a first-class, alarmed, enforced quota with an explicit degraded mode.** etcd's failure is unpleasant but *diagnosable*; the version where you simply discover recovery now takes 40 minutes is worse. If we use SQLite, `VACUUM` is our `defrag` and has the same blocking character — consider `auto_vacuum=INCREMENTAL` to avoid ever needing a stop-the-world rebuild.

---

## 9. Recovery budget

**[Synthesis — planning estimates, not measurements.]** The engine-dependent rows assume ~100 MB/s state reload (§6 anchors) and batched replay; they need validation against our own benchmark.

| Scenario | Dominant cost | Rough order |
|---|---|---|
| Hot-standby failover (§2b) | Leader election | **~100s of ms – few s** (KRaft detection bound is ~2 s) |
| Warm restart (§2a) | Replay seconds of log | ~1 s |
| Cold start, snapshot + replay (§2c) | Snapshot load + tail replay | **~10 s** at a tens-of-MB tail |
| Cold start, object-storage LSM | `open()` + cache warm-up | **~1 s to open**, then degraded p99 for an unpublished duration |
| Full log replay from zero | — | **Never do this** — minutes to hours |

---

## 10. Design recommendations

1. **Two-layer model.** Replicated log = source of truth; materialized index = rebuildable cache. Never blur them.
2. **Snapshot, don't compact.** KIP-630's event/delta argument applies directly; KIP-1164 reached the same conclusion for this exact problem.
3. **Put the snapshot pointer in the log** (`SnapshotCommitted{end_offset, epoch, object_key}`). This is the one design that eliminates enumeration structurally, and it's available to us precisely because we have a log.
4. **Enforce the reachability invariant** before advancing `log_start_offset` (§4).
5. **Design failover around hot standbys**, not cold rebuild. Publish and measure the two RTOs separately.
6. **Trigger snapshots on accumulated journal volume, not wall-clock**, so RTO is load-independent. Schedule on completion + pause to avoid continuous-checkpointing lock-up.
7. **Aim for a replay tail in the tens of megabytes** — the convergent default across KRaft, etcd, and openraft.
8. **Prefer an engine where snapshot is cheap and restore is cheaper.** SQLite's read-transaction-as-snapshot gives a non-blocking snapshot whose restore is a file open. Benchmark before committing (§6, the redb-bench fork).
9. **Startup must never hard-depend on the coordinator.** Come up degraded and retry. Both WarpStream and Bufstream got bitten here.
10. **If using leases, poll them; never rely on watch alone**, and make lease loss locally self-detectable.
11. **Error paths must never emit a plausible offset.** `-1`, not `0`.
12. **Cleanup on by default**, with an enforced, alarmed quota and an explicit degraded mode.
13. **Fault-inject pauses and partitions**, not just kills — that's where metastability lives.
14. **Decide the enumeration fork deliberately** (§7): WarpStream's prefix-bounded scanner (survives coordinator outage) vs. AutoMQ's `PREPARED`-state GC (no enumeration, but coordinator on the write path always).

---

## 11. Explicitly unpublished figures

Stated plainly rather than estimated:

- **WarpStream** single-region control-plane replica rebuild time; metadata snapshot cadence and size; Ripcord catch-up rate; scan/replay job LIST/GET cost.
- **AutoMQ** KRaft metadata recovery time, snapshot size, or controller failover duration in their deployments.
- **KIP-1164** coordinator failover time, SQLite cold-rebuild time, or behavior of in-flight requests during a leadership change — the KIP says only that it "will follow the model set by other coordinators in Kafka."
- **turbopuffer** Jan 2026 root cause — RCA went to affected customers only.
- **Confluent's 2M-partition KRaft-vs-ZooKeeper benchmark** exact figures — the originating post 404s and surviving pages publish the chart without values. Widely-circulated "120 s vs 20–30 s" numbers could not be traced to a primary source and are **not** asserted here.
- **Any vendor's official interval→RTO formula.** Flink deliberately declines; only academic models and third-party folklore exist.
- **Wall-clock recovery time for any storage engine** in §6 — not for redb full repair, not for RocksDB WAL replay, not for `SlateDB::open`.
- **Any sequential/sorted-insert benchmark** from a common harness, or any benchmark at our ~40-byte record size.

---

## Sources

**Kafka / KIPs**
- [KIP-630: Kafka Raft Snapshot](https://cwiki.apache.org/confluence/display/KAFKA/KIP-630:+Kafka+Raft+Snapshot)
- [KIP-876: Time based cluster metadata snapshots](https://cwiki.apache.org/confluence/display/KAFKA/KIP-876:+Time+based+cluster+metadata+snapshots)
- [KIP-1164: Diskless Coordinator](https://cwiki.apache.org/confluence/display/KAFKA/KIP-1164:+Diskless+Coordinator) (renamed from "Topic Based Batch Coordinator"; old URL 404s)
- [KIP-1163: Diskless Core](https://cwiki.apache.org/confluence/display/KAFKA/KIP-1163:+Diskless+Core)
- [Apache Kafka 4.1 broker configs](https://kafka.apache.org/41/configuration/broker-configs/)
- [Snapshots.java](https://raw.githubusercontent.com/apache/kafka/trunk/raft/src/main/java/org/apache/kafka/snapshot/Snapshots.java) · [KafkaMetadataLog.scala](https://raw.githubusercontent.com/apache/kafka/3.9/core/src/main/scala/kafka/raft/KafkaMetadataLog.scala)
- [apache/kafka PR #13280 — KAFKA-14735 benchmarks](https://github.com/apache/kafka/pull/13280)
- [KRaft Overview — Confluent](https://docs.confluent.io/platform/current/kafka-metadata/kraft.html)
- [A deep dive into Apache Kafka's KRaft protocol — Red Hat](https://developers.redhat.com/articles/2025/09/17/deep-dive-apache-kafkas-kraft-protocol)

**Raft / etcd / openraft**
- [In Search of an Understandable Consensus Algorithm (extended), §7](https://raft.github.io/raft.pdf)
- [openraft config.rs](https://raw.githubusercontent.com/databendlabs/openraft/main/openraft/src/config/config.rs) · [RaftSnapshotBuilder](https://docs.rs/openraft/latest/openraft/storage/trait.RaftSnapshotBuilder.html) · [issue #437](https://github.com/databendlabs/openraft/issues/437)
- [etcd v3.5 config](https://etcd.io/docs/v3.5/op-guide/configuration/) · [v3.6 config](https://etcd.io/docs/v3.6/op-guide/configuration/) · [maintenance](https://etcd.io/docs/v3.5/op-guide/maintenance/) · [limits](https://etcd.io/docs/v3.5/dev-guide/limit/) · [tuning](https://etcd.io/docs/v3.1/tuning/)
- [etcd #15354 — the 8 GB limit and MTTR](https://github.com/etcd-io/etcd/issues/15354) · [#13889 — default snapshot-count 10k](https://github.com/etcd-io/etcd/issues/13889) · [wal.go](https://raw.githubusercontent.com/etcd-io/etcd/main/server/storage/wal/wal.go)

**Pointer / checkpoint patterns**
- [Delta Lake PROTOCOL.md](https://raw.githubusercontent.com/delta-io/delta/master/PROTOCOL.md) · [VACUUM best practices — Databricks](https://kb.databricks.com/delta/vacuum-best-practices-on-delta-lake) · [Delta OSS optimizations](https://docs.delta.io/latest/optimizations-oss.html)
- [Apache Iceberg spec](https://iceberg.apache.org/spec/) · [maintenance](https://iceberg.apache.org/docs/latest/maintenance/) · [deprecate HadoopTableOperations](https://www.mail-archive.com/dev@iceberg.apache.org/msg07012.html)
- [Flink: log-based incremental checkpoints](https://flink.apache.org/2022/05/30/improving-speed-and-stability-of-checkpointing-with-generic-log-based-incremental-checkpoints/) · [large-state tuning](https://nightlies.apache.org/flink/flink-docs-master/docs/ops/state/large_state_tuning/) · [checkpointing docs](https://nightlies.apache.org/flink/flink-docs-master/docs/dev/datastream/fault-tolerance/checkpointing/)
- [Optimal Checkpointing-Interval for Flink — ACM DL](https://dl.acm.org/doi/abs/10.1007/s11036-020-01729-7) · [arXiv:1911.11915](https://arxiv.org/pdf/1911.11915)

**Storage engines**
- [slatedb/slatedb](https://github.com/slatedb/slatedb) · [Tuning](https://slatedb.io/docs/operations/tuning/) · [RFC 0001 Manifest](https://slatedb.io/rfcs/0001-manifest/) · [RFC 0004 Checkpoints](https://slatedb.io/rfcs/0004-checkpoints/) · [RFC 0008 Synchronous Commit](https://slatedb.io/rfcs/0008-synchronous-commit/) · [issue #684](https://github.com/slatedb/slatedb/issues/684) · [issue #1302](https://github.com/slatedb/slatedb/issues/1302) · [CNCF Sandbox #114](https://github.com/cncf/sandbox/issues/114)
- [Benchmarking SlateDB vs RocksDB — Nixiesearch](https://nixiesearch.substack.com/p/benchmarking-slatedb-vs-rocksdb) · [Responsive RS3](https://docs.responsive.dev/storage/rs3) · [s2-lite](https://s2.dev/docs/s2-lite)
- [cberner/redb README](https://raw.githubusercontent.com/cberner/redb/master/README.md) · [design.md](https://github.com/cberner/redb/blob/master/docs/design.md) · [bench harness](https://raw.githubusercontent.com/cberner/redb/master/crates/redb-bench/benches/common.rs) · [v4.1.0 notes](https://github.com/cberner/redb/releases/tag/v4.1.0)
- [RocksDB Performance Benchmarks](https://github.com/facebook/rocksdb/wiki/Performance-Benchmarks) · [WAL Recovery Modes](https://github.com/facebook/rocksdb/wiki/WAL-Recovery-Modes) · [Ceph RocksDB tuning](https://ceph.io/en/news/blog/2022/rocksdb-tuning-deep-dive/)
- [The cost and complexity of Cgo — Cockroach Labs](https://www.cockroachlabs.com/blog/the-cost-and-complexity-of-cgo/) · [Introducing Pebble](https://www.cockroachlabs.com/blog/pebble-rocksdb-kv-store/)
- [Releasing Fjall 3.0](https://fjall-rs.github.io/post/fjall-3/) · [Fjall 2.8](https://fjall-rs.github.io/post/fjall-2-8) · [fjall README](https://github.com/fjall-rs/fjall/blob/main/README.md)
- [SQLite WAL docs](https://www.sqlite.org/wal.html) · [SQLite in Production benchmark](https://shivekkhurana.com/blog/sqlite-in-production/) · [Inserting One Billion Rows in SQLite](https://avi.im/blag/2021/fast-sqlite-inserts/)
- [rockset/rocksdb-cloud](https://github.com/rockset/rocksdb-cloud) · [OpenAI acquires Rockset](https://openai.com/index/openai-acquires-rockset/) · [tikv/rust-rocksdb PR #517](https://github.com/tikv/rust-rocksdb/pull/517)

**Peer systems**
- [WarpStream Architecture](https://docs.warpstream.com/warpstream/overview/architecture) · [No record left behind](https://www.warpstream.com/blog/no-record-left-behind-how-warpstream-can-withstand-cloud-provider-regional-outages) · [The Art of Being Lazy(log)](https://www.warpstream.com/blog/the-art-of-being-lazy-log-lower-latency-and-higher-availability-with-delayed-sequencing) · [Ripcord / low-latency clusters](https://docs.warpstream.com/warpstream/kafka/advanced-agent-deployment-options/low-latency-clusters) · [Zero Disks is Better](https://www.warpstream.com/blog/zero-disks-is-better-for-kafka)
- [AutoMQ: Insight into Metadata Management](https://www.automq.com/blog/insight-metadata-management-in-automq) · [Auto Partition Reassignment](https://www.automq.com/blog/automq-achieving-auto-partition-reassignment-in-kafka-without-cruise-control) · [Stateless Broker wiki](https://github.com/AutoMQ/automq/wiki/Stateless-Broker) · [WAL Storage wiki](https://github.com/AutoMQ/automq/wiki/WAL-Storage) · [WarpStream Architecture Explained](https://www.automq.com/blog/warpstream-architecture-explained-kafka-teams)
- [Jepsen: Bufstream 0.1.0](https://jepsen.io/analyses/bufstream-0.1.0)
- [turbopuffer Architecture](https://turbopuffer.com/docs/architecture) · [Guarantees](https://turbopuffer.com/docs/guarantees) · [Warm cache](https://turbopuffer.com/docs/warm-cache) · [Jan 2026 incident](https://statuspage.incident.io/turbopuffer/incidents/01KG3WTDRYBC1W6K7GRHKCGFGQ)
- [A Fork in the Road: Deciding Kafka's Diskless Future — Jack Vanlightly](https://jack-vanlightly.com/blog/2025/10/22/a-fork-in-the-road-deciding-kafkas-diskless-future)
- [The Hitchhiker's Guide to Diskless Kafka — Aiven](https://aiven.io/blog/guide-diskless-apache-kafka-kip-1150) · [2 Minute Streaming: KIP-1150](https://blog.2minutestreaming.com/p/diskless-kafka-topics-kip-1150)
- [Productionise Kafka Streams State Stores — OSO](https://oso.sh/blog/productionise-kafka-streams-state-stores-how-unbundled-architecture-transforms-kafka-streams-operations/)
