---
title: "Metadata Scale: Tiered Metadata, Deterministic Layout, and the Protocol Ceiling"
slug: metadata-scale-and-tiering
status: draft
last_updated: 2026-08-13
tags: [scale, tiered-metadata, deterministic-layout, manifest, cas, kip-405, redpanda-cloud-topics, thanos, bucket-index, partition-limits, metadata-response, virtual-clusters]
related: [12-object-discovery-and-api-cost, 13-coordinator-recovery, 08-turbopuffer-lessons, 02-kafka-protocol-compatibility, 06-distributed-systems-design-challenges]
summary: >
  Can we reach turbopuffer-class catalog scale without a central coordinator?
  Verdict: the tiered design (coordinator for the hot tail, self-describing
  per-partition manifests at derived keys for compacted history) is sound but
  unbuilt for streaming — each half ships in production separately. KIP-405 is
  much weaker counter-evidence than it looks. Covers the five real frictions,
  what does and doesn't transfer from turbopuffer's 250M namespaces, and the
  finding that the Kafka *protocol* — not storage — is the binding constraint,
  with client memory breaking first.
---

# Metadata Scale: Tiered Metadata, Deterministic Layout, and the Protocol Ceiling

> **⚠️ UPDATE (2026-08-13).** §12 of this document lists "why AutoMQ caps at 2k/20k partitions" as an open question. **It has since been answered** by a source study — see [16-automq-deep-dive.md](16-automq-deep-dive.md) §1. Short version: KRaft as the metadata plane, evidenced by AutoMQ's own [issue #1608](https://github.com/AutoMQ/automq/issues/1608) reporting OOM at 100k partitions / 300k streams, and [#3477](https://github.com/AutoMQ/automq/issues/3477) reporting controller saturation at ~5k topics. Not SKU tiering. Note also that the compaction cadence (5 min, not 20) and graduation threshold (8 MiB, not 16) cited in §8 below are corrected there.

*Compiled 2026-08-13 from three parallel research threads: turbopuffer's namespace-scaling mechanics, the tiered-metadata design question, and measured Kafka protocol partition limits.*

Marked **[Documented]** (primary source, cited), **[Vendor claim]** (asserted, unverified), or **[Synthesis]** (this corpus's reasoning). Unpublished figures are named as such, never estimated.

> **How this fits the project:** [12](12-object-discovery-and-api-cost.md) established the coordinator-index pattern; [13](13-coordinator-recovery.md) covered its recovery. This document asks whether that coordinator survives a very large partition catalog, and answers a specific proposal: bound coordinator state to the uncompacted tail by making compacted history self-describing.

---

## 1. Verdict

**The tiered design is sound but unbuilt for streaming — not blocked.** Both halves ship in production, separately:

| Half of the design | Status |
|---|---|
| Per-partition manifest at a **derivable** object key, bucket self-sufficient | **Shipping in Redpanda since 2022** |
| Coordinator state bounded to the **uncompacted window**, not retention | **Adopted in Apache Kafka Diskless (KIP-1163/1164)** |
| **The combination** (bundled hot tail + zero-service derived-path history) | **Nobody has built it** |

The one fact that should give pause: **Redpanda Cloud Topics built the age split, already owned per-partition derivable manifests in the adjacent product, and still chose an LSM metastore for compacted L1 — without documenting why.** That undocumented choice by the best-positioned team is the single open risk worth probing.

And one correction to the claim as stated: the coordinator does not get to be *ignorant* of history, only *off the read path*. Retention enforcement, GC, `ListOffsets` by timestamp, and consumer-group state still need an authority. The defensible claim is **O(uncompacted window) for the read-path index** — which is still the part that scales with retention × partitions and dominates.

---

## 2. Decomposing the scale target

**[Synthesis]** "Millions × millions" is fine as a *catalog* size and hard as an *active* size. The number that sizes the coordinator is **how many partitions receive a write in a given flush window**, not how many exist.

turbopuffer's 250M+ namespaces work because the overwhelming majority are idle. Their published limits ([turbopuffer Limits](https://turbopuffer.com/docs/limits)) **[Documented]**:

| Limit | Value |
|---|---|
| Max namespaces | Unlimited (seen: **250M+**) |
| Max write throughput, per namespace | 10k writes/s @ 32 MB/s |
| Max write throughput, global | Unlimited (seen: **10M+ writes/s @ 32 GB/s**) |
| Max shards per namespace | 256 |

⚠️ **Correction to a common misreading (including this corpus's earlier framing):** the documented "1 WAL entry per second per namespace" is a **commit-rate ceiling, not a throughput ceiling**. One commit carries up to ~10k documents / 32 MB, hence 10k writes/s per namespace. What it actually bounds is *latency* — a 200ms–1s floor per write. That latency is invisible to turbopuffer's users and fatal to Kafka's.

**[Synthesis]** Two regimes, and they need separating before choosing an architecture:

- **10M partitions, ~10k active per window** → tractable. Coordinator load is bounded by active set.
- **1M partitions all simultaneously active** → each producing ≥1 record per 250ms is ≥4M records/sec, i.e. multi-GB/s ingest. The coordinator is not the first problem.

---

## 3. The coordinator's real bottleneck is state, not throughput

**[Synthesis]** Coordinator *transaction* rate is inherently low and scalable: one commit covers one flushed object containing many partitions, so commits/sec ≈ agents × flush rate — perhaps 400/sec at 100 agents. What breaks is **state size**, and it hinges entirely on index granularity:

| Coordinator stores | Entries/sec (100 agents × 10k active partitions each) | Index growth | Cold read |
|---|---|---|---|
| Per-(object, partition) byte ranges | ~4M/s | **~160 MB/s** | 1 GET |
| Per-object only; partition→range in the object footer | ~400/s | **~16 KB/s** | 1–3 GETs |

Four orders of magnitude. **Fine-grained coordinator state does not survive a large active working set.** At this scale the granularity question from [12](12-object-discovery-and-api-cost.md) §6.3 is forced: coarse coordinator state plus an in-object index (AutoMQ's two-level design), accepting 1–3 GETs on a cold read that the chunk cache absorbs after the first reader.

Sizing context from KIP-1164 **[Documented]**: diskless coordinator state is "up to hundreds of megabytes or even gigabytes," which is why they chose SQLite over memory. Community analysis puts per-partition metadata at ~118 bytes ([Instaclustr](https://www.instaclustr.com/support/documentation/announcements/apache-kafka-and-kafka-connect/kafka-diskless-proposals-status-insights/)).

---

## 4. Why KIP-405 is weaker counter-evidence than it appears

Kafka's own tiered storage chose a metadata *topic* (`__remote_log_metadata`) rather than a self-describing layout for cold segments. That looks like a direct refutation. It isn't.

### 4.1 What the KIP actually says **[Documented]**

> "One of the problems of relying on remote storage to maintain metadata is that tiered-storage needs to be strongly consistent, with an impact not only on the metadata itself (e.g. LIST in S3) but also on the segment data (e.g. GET after a DELETE in S3). Also, the cost (and to a lesser extent performance) of maintaining metadata in remote storage needs to be factored in. In the case of S3, frequent LIST APIs incur huge costs."
> — [KIP-405](https://cwiki.apache.org/confluence/display/KAFKA/KIP-405%3A+Kafka+Tiered+Storage)

### 4.2 Object-storage-resident metadata was the *original* design, and this is what killed it **[Documented]**

The first version of KIP-405 kept its index in remote storage (`RemoteLogIndex`, with a pull-based `listRemoteSegments()`). Jun Rao's DISCUSS-thread review ended it:

> "S3 list object requests cost $0.005 per 1000 requests. If you have 100,000 partitions and want to pull the metadata for each partition at the rate of 1/sec. It can cost $0.5/sec, which is roughly **$40K per day**."
>
> "S3 list objects are **eventually consistent**. So, when you do a list object request, there is no guarantee that you can see all uploaded objects."
> — Jun Rao, [dev@kafka msg103639](https://www.mail-archive.com/dev@kafka.apache.org/msg103639.html)

**There is no formal rejected-alternatives entry for "deterministic path layout" in KIP-405.** The rationale exists only in this thread. If citing KIP-405 as counter-evidence, cite the mailing list, not a rejected-alternatives block — there isn't one.

### 4.3 Why both reasons miss our design **[Synthesis]**

**(a) The consistency argument expired.** S3 gained strong read-after-write consistency *including for LIST* in December 2020, after these comments were written ([04](04-object-storage-s3-gcs.md) §1). That half of the objection cites a dead fact.

**(b) The cost argument is about LIST-polling, not derived-key GET.** Jun's $40K/day is explicitly "pull the metadata for each partition at the rate of 1/sec" *via LIST*. Our design derives one manifest key and issues one GET — 12.5× cheaper per request ([12](12-object-discovery-and-api-cost.md) §1), and trivially cacheable with unbounded TTL because compacted objects are immutable. **KIP-405 rejected discovery by enumeration, which is also what we reject.**

**(c) Kafka got the metadata topic nearly free; we wouldn't.** KIP-405 bolts tiering onto leader-based Kafka, where brokers already hold per-partition state, already run a replicated log, and already need a consistency authority for retention. One internal topic was marginal cost. For a system whose premise is *not* having per-partition durable broker state, the same choice costs far more. Different starting point, different optimum.

### 4.4 The reasons that DO still hold **[Documented]**

Three requirements survive, and they're what the metadata topic genuinely buys:

- **A two-phase upload/delete state machine.** `RemoteLogSegmentMetadata` cycles `COPY_SEGMENT_STARTED → COPY_SEGMENT_FINISHED → DELETE_SEGMENT_STARTED → DELETE_SEGMENT_FINISHED`. **[Synthesis]** A bare object cannot express "I began uploading and crashed" — a half-written object is indistinguishable from a complete one without an external witness.
- **Leader-epoch reconstruction.** Followers must rebuild leader-epoch caches and producer-ID snapshots from remote metadata; `segmentLeaderEpochs` exists for this.
- **Aggregate queries.** Total remote log size for size-based retention is an aggregation over all segments — hence [KIP-852](https://cwiki.apache.org/confluence/display/KAFKA/KIP-852:+Optimize+calculation+of+size+for+log+in+remote+tier) as a dedicated optimization.

**[Synthesis]** Note what these share: **none is the record read path.** They are the write-commit protocol, the replication protocol, and the retention protocol. We can concede all three to the coordinator and still keep the read path service-free — which is exactly the concession Redpanda made (§5.2).

---

## 5. Prior art: who built which half

### 5.1 Object key schemes across implementations **[Documented]**

| System | Key format | Derivable from (topic, partition, offset)? |
|---|---|---|
| **Redpanda** (classic TS) | `{xxhash32("{ntp_path}_{rev}") & 0xF0000000 :08x}/meta/{ns}/{topic}/{partition}_{revision}/manifest.json` | **Yes, fully** |
| Aiven RSM | `{prefix}/{topic}-{uuid}/{partition}/{020d startOffset}-{segmentUUID}.log` | Partially — trailing UUID blocks derivation |
| Pulsar offloader | Offload-attempt `UUID`; index at `{uuid}-index` | No |
| AutoMQ | Opaque object IDs; index in KRaft | No |

([redpanda `partition_path_utils.cc`](https://raw.githubusercontent.com/redpanda-data/redpanda/dev/src/v/cloud_storage/partition_path_utils.cc), [Aiven](https://aiven.io/blog/apache-kafka-tiered-storage-in-depth-how-writes-and-metadata-flow), [PIP-17](https://github.com/apache/pulsar/wiki/PIP-17:-Tiered-storage-for-Pulsar-topics))

Redpanda's hash prefix exists for object-store shard spreading, not identity: "Object stores shard workloads based on an object name prefix… Redpanda inserts a randomized prefix into every object" ([Redpanda deep dive](https://www.redpanda.com/blog/tiered-storage-architecture-deep-dive)). It remains a pure function of the NTP + revision — i.e. **exactly the derived manifest key we want.**

**The UUID suffix elsewhere is load-bearing, not laziness.** Aiven states the reason: multiple brokers may upload the same segment during leader failover, so "each upload attempt receives a distinct UUID… allowing proper recovery handling when brokers fail mid-upload." **[Synthesis]** But this is a 2019-vintage constraint. `PutObject` with `If-None-Match: *` (S3, Nov 2024 — see [04](04-object-storage-s3-gcs.md) §6) turns "two writers racing to a deterministic key" from data corruption into a losing writer getting a 412. A 2026 design can name deterministically where a 2019 design could not.

### 5.2 Redpanda: the manifest half, shipping **[Documented]**

- "For every topic partition, we also maintain a separate partition manifest that has a list of all log segments uploaded to the cloud storage."
- **The bucket is self-sufficient**: Remote Read Replicas work because "the data in cloud storage includes topic and partition manifests, making it self-sufficient and portable" ([Remote Read Replicas](https://docs.redpanda.com/current/manage/remote-read-replicas/)). Topic Recovery and [Whole Cluster Restore](https://docs.redpanda.com/current/manage/disaster-recovery/whole-cluster-restore/) reconstruct topics from the bucket alone.
- **But the manifest is not the source of truth on the origin cluster.** Authoritative state is `archival_metadata_stm`, a Raft-replicated STM; the manifest is an *upload* of it. Only *foreign* readers (read replicas, recovery) go through the manifest.

**[Synthesis]** So Redpanda proves the manifest is *sufficient* to serve reads — read replicas do exactly that in production — while declining to depend on it on the hot path, because they already have Raft there. We have no per-partition Raft, so the manifest would be our only path: the read-replica path, already exercised.

Redpanda also independently invented tiered metadata. **Spillover manifests** (23.2+): "Only metadata for recently-updated segments is kept in memory or on local disk, while the rest is safely stored in object storage and cached locally as needed" ([Tiered Storage docs](https://docs.redpanda.com/current/manage/tiered-storage/)), with the live manifest capped near 128 KiB via `cloud_storage_spillover_manifest_size`.

### 5.3 Kafka Diskless: the coordinator-bounding half, adopted **[Documented]**

The strongest statement of our thesis anywhere in the Kafka corpus:

> "even with infinite data retention, the lifetime of each batch and WAL file metadata inside DC is **finite** and determined by user configuration (mainly `segment.ms` and `segment.bytes`)"
> — [KIP-1164](https://cwiki.apache.org/confluence/display/KAFKA/KIP-1164:+Diskless+Coordinator)

> "In order to control the number of many small WAL files, the batch coordinator memory size, and the rebuild time of local segments after a replica change, data is moved to Tiered Storage using the existing KIP-405 interfaces."
> — [KIP-1163](https://cwiki.apache.org/confluence/display/KAFKA/KIP-1163:+Diskless+Core)

**Coordinator state is O(segment.ms/segment.bytes), not O(retention)** — our exact goal, in the Apache Kafka mainline. The only difference is what indexes history: `__remote_log_metadata` rather than derived-path manifests.

KIP-1164's rejected alternatives are worth noting: "Use external system" (rejected to preserve self-sufficiency), "Use cluster KRaft quorum" (rejected — mixing low-throughput cluster metadata with high-throughput diskless metadata risks bottlenecking leadership changes), "Use separate KRaft quorum" (no advantage). **Object storage as the metadata substrate is not among them — it appears never to have been proposed.**

### 5.4 Redpanda Cloud Topics: the closest match, and it chose differently **[Documented]**

This is our architecture, built:

- **L0 = bundled hot tail.** "We collect this data across all partitions and topics simultaneously… by aggregating smaller writes into larger batches, we significantly reduce the number of PUT requests."
- **L1 = reorganized history.** "All data for a specific partition range is physically together", sorted, offset-organized, "optimized for high-throughput object storage reading."
- **Explicit age boundary.** Reads route on a **"Last Reconciled Offset"**: below it read L1, above it follow pointers in local Raft logs to L0.

([Cloud Topics architecture](https://www.redpanda.com/blog/cloud-topics-architecture))

**The divergence:** L1 metadata lives in a custom LSM metastore "heavily inspired by LevelDB and RocksDB" — memtable → SSTables flushed to object storage, Raft-replicated ([Cloud Topics: the Metastore](https://www.redpanda.com/blog/cloud-topics-metastore)). **They never state why derivable per-partition paths weren't used for L1.**

**[Synthesis]** My reading of the likely cause: Cloud Topics must also serve leader-term boundaries, compaction state, and "a myriad of other information required to correctly serve the Kafka protocol." Once you need a metastore for protocol state anyway, putting the offset index in it is free. That is inference, not their statement, and it is the open risk in §1.

### 5.5 KIP-1165: direct counter-evidence on the reorganization step **[Documented]**

> "Objects uploaded by the merging stage will contain data from **multiple partitions**, stored together to amortize object access costs."
> — [KIP-1165](https://cwiki.apache.org/confluence/display/KAFKA/KIP-1165:+Object+Compaction+for+Diskless)

Metadata reduction *is* a stated motivation, but achieved by merging *batches*, not by making objects self-describing. The KIP does not discuss self-description as an option, so there is no documented rationale for rejecting it — only a documented reason for the multi-partition choice.

### 5.6 Round-trip comparison **[Documented, assembled]**

| System | Round trips to reach a record range in history | Service required? |
|---|---|---|
| Kafka TS (topic-based RLMM) | 0 remote metadata (in-memory) + 1 index read + 1 ranged GET | **Yes** — every broker consumes the whole metadata topic |
| Pulsar offload | 1 metadata-store read + 1 index GET + N ranged GETs | **Yes** |
| Iceberg | 1 catalog call + ~3–5 metadata GETs + data GETs | **Yes** (catalog) |
| Delta Lake (path tables) | 1 GET (`_last_checkpoint`) + checkpoint read + 1 LIST + data GETs | No |
| turbopuffer | 3–4 GETs total, ~400 ms cold | No |
| **Redpanda read replica** | **1 GET (topic manifest) + 1 GET (partition manifest, derived key) + ranged GETs** | **No** |
| **This proposal** | **1 GET (derived manifest) + ranged GETs** | **No** |

### 5.7 The LSM analogy is exact **[Synthesis]**

RocksDB: L0 files overlap, so "for a Get() request on L0, it needs to look up every L0 file due to the key range overlaps"; L1+ are non-overlapping sorted runs where one binary search locates the candidate ([RocksDB](https://rocksdb.org/blog/2017/06/26/17-level-based-changes.html)). The index strategy differs by level too — `pin_l0_filter_and_index_blocks_in_cache` pins L0 index blocks while deeper levels page in on demand.

Our bundled hot-tail objects **are** L0 (overlapping partition ranges → must be indexed exhaustively → coordinator). Our compacted per-partition objects **are** L1+ (non-overlapping, offset-ordered → binary search over a manifest, no service). Thirty years of LSM literature says this split is the natural one.

---

## 6. The Thanos cautionary tale — and why it's good news

**[Documented]** The closest operational failure mode, worth reading carefully:

- Thanos/Cortex blocks are **self-describing** — a ULID-named prefix containing `chunks/`, `index`, and `meta.json`.
- **But discovery is by LIST, and it broke.** Cortex reports a 10,000-tenant cluster with ~4M blocks; on a 6,000-tenant test cluster, querier cold starts took "tens of minutes," and raising concurrency hit object-store rate limits and 5xx. Deletion-mark discovery required a GET per block that "can't be cached for a long time," which was "a significant % of bucket API calls baseline costs, **regardless the tenants QPS** (costs we have even if the cluster has 0 queries)" ([Cortex bucket index proposal](https://cortexmetrics.io/docs/proposals/blocks-storage-bucket-index/)).
- **Thanos quantified it in dollars:** block metadata sync taking "4m15s" and "1 Billion requests to object store monthly," costing **~$5,000/month** against ~$23/month for 1 GB of Redis ([thanos#6468](https://github.com/thanos-io/thanos/issues/6468)).
- **The fix:** a per-tenant `bucket-index.json` listing all non-partial blocks and deletion marks, ~150 bytes/block, maintained by the compactor, lazily loaded, in-memory cached, with a hard `max-stale-period` fence that **fails the query** rather than returning partial results.

**[Synthesis] The transferable lesson is precise and favorable:** their problem was never self-description — it was that **ULIDs are not derivable**, so you must enumerate to find blocks. Cortex's bucket index is exactly "one small manifest at a derivable per-tenant path, listing what exists." **Our per-partition manifest at a derived key is the bucket index, adopted from day one instead of after a $5,000/month bill.** We start where they ended up.

Their staleness discipline transfers wholesale: max staleness = min(upload window, deletion delay), and **the deletion delay must exceed the staleness bound** so a cached reader never points at a reaped object — the same inequality as [13](13-coordinator-recovery.md) §8.

**[Synthesis]** One thing that makes our staleness story *easier* than Mimir's: compacted objects are immutable, so a stale manifest can only be **incomplete, never wrong** — the same property that makes the reader-side index cache safe in [12](12-object-discovery-and-api-cost.md) §4.2.

---

## 7. The five frictions

**[Synthesis, each with a documented mitigation]**

| # | Friction | Evidence | Mitigation, and who does it |
|---|---|---|---|
| 1 | **Duplicate-upload fencing.** Every existing system suffixes a UUID because two brokers may upload the same range; a deterministic key means collision. | [Aiven](https://aiven.io/blog/apache-kafka-tiered-storage-in-depth-how-writes-and-metadata-flow), [PIP-17](https://github.com/apache/pulsar/wiki/PIP-17:-Tiered-storage-for-Pulsar-topics) | `If-None-Match: *` conditional PUT, or a fenced single-writer compactor. Note this applies to *segments* — compaction is single-writer by construction, so it may not apply to us at all. |
| 2 | **The tail/history boundary must itself be coordinated.** A reader must know whether offset N is compacted or hot. | Redpanda's "Last Reconciled Offset" | One watermark per partition — O(partitions), tiny, coordinator already tracks it. |
| 3 | **Manifest size growth.** A flat per-partition manifest is unbounded in retention. | [Iceberg bloat](https://www.e6data.com/blog/apache-iceberg-million-files-metadata); Redpanda spillover | **Two-level manifest, mandatory.** Redpanda caps the live manifest at ~128 KiB and chains archived ones. |
| 4 | **CAS write-rate ceiling per object.** | See below | Fine at compaction pace; **never let the produce path touch a manifest.** |
| 5 | **GC still needs an authority.** A manifest says what exists; it can't safely say what's orphaned. | KIP-405's delete state machine; [WarpStream GC](https://www.warpstream.com/blog/taking-out-the-trash-garbage-collection-of-object-storage-at-massive-scale) | Keep deletion in the coordinator. Soft-delete with delay > max manifest staleness. |

### Friction 4 in detail — the hardest number in this research **[Documented]**

Measured conditional-write goodput, per single object ([Chris Douglas, *Conditional Operations in Object Stores*](https://cdouglas.github.io/posts/2026/01/conditional)):

| Store | Successful conditional writes/sec on one key |
|---|---|
| **Google Cloud Storage** | **~1 op/s** |
| Azure Blob Standard | 9.2–10.2 |
| **Amazon S3** | **14.7–15.3** |
| S3 Express One Zone | 68.6–75.3 |

Conflicts are expensive: "A pipeline of conditional requests is mostly waste: a successful write invalidates all outstanding requests, so only one can succeed per round-trip," and each conflict costs 3 round trips. The author's conclusion: "another service needs to be in front of the object store to batch operations and reduce conflicts to achieve higher throughput."

**[Synthesis]** ~15 CAS/s per manifest is *ample* for a compaction-paced writer (one update per partition per compaction round) and *fatal* for anything near tail write rates. This is a strong quantitative argument for exactly the boundary proposed: **manifests touched only by compaction, never by produce.** And note **GCS at ~1 op/s is a portability hazard** — budget for it being 15× worse if a compaction storm or backfill needs more than one manifest commit per second per partition.

### Manifest PUT cost at high partition counts **[Synthesis — nobody publishes this]**

At 100,000 partitions, one manifest PUT per partition per compaction round:

| Compaction cadence | Manifest PUTs/s | S3 cost/day (@ $0.005/1k) |
|---|---|---|
| 60 s | 1,667 | ~$720 |
| 5 min | 333 | ~$144 |
| 30 min | 56 | ~$24 |

Linear in both partition count and cadence, and **fixed regardless of throughput**. A real argument for slow compaction cadence at high partition counts, additive to data-rewrite PUT cost.

Redpanda's documented trade on the segment side: adjacent segment merging at 500 MiB "doubles the amount of data that Redpanda uploads… but it also reduces the memory footprint of the partition, which results in better scalability because **98% less memory** is needed" — 2× write amplification bought for 50× metadata reduction.

---

## 8. Compaction as metadata reduction — explicit statements **[Documented]**

Multiple systems say this outright, which supports the core mechanism:

- **AutoMQ:** "the majority of indexing costs are spent on searching the StreamSetObject"; "By compacting multiple small objects into larger ones, the amount of metadata that needs to be maintained is effectively reduced"; "To reduce the scale of metadata in scenarios involving a large number of partitions, the S3Stream internally provides two compaction mechanisms." Mechanics: SSO compaction every 20 minutes; **streams exceeding 16 MiB graduate to their own Stream Object**, the rest merge into a new SSO ([AutoMQ metadata management](https://www.automq.com/blog/insight-metadata-management-in-automq), [S3 Storage](https://docs.automq.com/automq/architecture/s3stream-shared-streaming-storage/s3-storage)). **That size threshold is precisely the per-stream reorganization step in our design.**
- **WarpStream:** compaction ensures "the number of batches the metadata store has to track remains proportional to the overall write throughput, partition count, and retention" — and the metadata store is "the most expensive component in the stack in terms of cost-per-byte stored."
- **KIP-1165:** "Storing fewer distinct batches will reduce the batch metadata overhead."

**[Documented — and a gap]** The only hard before/after ratio published by anyone is Redpanda's "98% less memory" at 50:1 segment merging. AutoMQ, WarpStream and the KIPs all assert metadata reduction qualitatively with no numbers. **A before/after figure for a design doc would have to be measured; it does not exist in the literature.**

---

## 9. What transfers from turbopuffer's 250M namespaces

### 9.1 Transfers cleanly **[Synthesis, grounded in documented mechanisms]**

| Mechanism | Evidence |
|---|---|
| **Partition = object-storage prefix, addressed by hash** | "A namespace to us is a directory on S3, and which compute node that goes to is essentially just a consistent hash of the name of the prefix and the User ID" ([Postgres FM](https://postgres.fm/episodes/turbopuffer/transcript)) |
| **Implicit creation on first write** | Namespaces "implicitly created when the first document is inserted"; no create API exists |
| **Per-unit manifest committed by CAS; no consensus plane** | "Object storage is the only stateful dependency" ([Guarantees](https://turbopuffer.com/docs/guarantees)) |
| **Consistent-hash routing for cache locality, any node can serve any unit** | "any query node can serve queries from any namespace" ([Architecture](https://turbopuffer.com/docs/architecture)) |
| **Cache as FIFO ring buffer with busy-unit prioritization** | "namespaces that are really busy will prioritize warming over others" ([CMU seminar](https://turbopuffer.com/blog/video-andy-pavlo-cmu)) |

### 9.2 Does NOT transfer **[Synthesis]**

**(a) The per-namespace WAL.** turbopuffer's WAL is strictly per-namespace. Applied to 1M *active* partitions each taking 1 PUT/s: 1M PUT/s ≈ **$13M/month in S3 request charges alone**, before data. They never pay this because namespaces are overwhelmingly idle. **We must invert to one shared WAL per broker per flush multiplexing many partitions** — the WarpStream/AutoMQ shape. Note turbopuffer's own sharding decision points the same way: a 256-shard namespace keeps **one shared WAL** to preserve atomic commit. They chose atomicity over commit parallelism.

**(b) The ~1 commit/s latency floor.** Invisible to their users, fatal to Kafka's `acks=all` expectations. They list "Heavy writes" among poor fits ([Tradeoffs](https://turbopuffer.com/docs/tradeoffs)) — a Kafka broker *is* the heavy-write case.

**(c) Monotonic offsets add a fencing requirement they never had.** Kafka needs a dense monotonic offset assigned at commit, plus per-`(producerId, partition)` sequence dedup with epoch fencing. A stale turbopuffer node can only lose a CAS race; a stale offset assigner can corrupt a sequence. This forces decoupling offset *assignment* (cheap, in-memory, single owner) from offset *durability* (batched into the shared WAL), plus a fencing mechanism.

**(d) Kafka mandates a catalog.** `Metadata` with a null topic array means "list all topics," and clients issue it routinely. `CreateTopics` needs exact partition counts; `DescribeConfigs`/ACLs need queryable rows. turbopuffer avoids this only by controlling its own client and load balancer. **The transferable lesson is narrower: keep the catalog off the data path.**

**(e) Their own list-namespaces experience validates avoiding LIST.** At 250M namespaces, "**the list call to get a thousand entries takes somewhere between 500 milliseconds and a second**"; they cope with "thousands of concurrent list calls" and "a read-through cache of a bunch of the starting points in the bucket" — and run it **once daily, offline, for billing and reconciliation**, never on a request path ([CMU seminar](https://turbopuffer.com/blog/video-andy-pavlo-cmu)).

**(f) "Idle costs nothing" is weaker for Kafka.** Their background work is dispatched by WAL-vs-index progress, so idle ⇒ zero work. Kafka's `retention.ms` must fire on idle partitions, and an attached-but-idle consumer generates continuous `Fetch` traffic. Two per-partition cost classes with no turbopuffer analogue.

---

## 10. The protocol ceiling — the constraint that binds first

**[Synthesis]** The storage layer is not the interesting problem. The Kafka protocol and its clients are.

### 10.1 What breaks, in order **[Documented unless noted]**

| Order | Ceiling | Threshold |
|---|---|---|
| 1 | **librdkafka per-partition fetch queues** — `queued.min.messages`=100k, `queued.max.messages.kbytes`=64 MB, **per partition** | ~10² partitions per consumer process at defaults |
| 2 | **Java producer** — one 16 KB `batch.size` buffer per active partition vs 32 MB `buffer.memory` | **~2,000 actively-produced partitions per producer**, then stalls on `max.block.ms` |
| 3 | **Classic group-metadata record vs `message.max.bytes`** (~1 MB) | Not published |
| 4 | **Metadata response size** | ~4 MB at 100k partitions; **~42–100 MB at 1M** |
| 5 | Fetch session cache eviction → fallback to full O(partitions) fetch | Not published |
| 6 | Broker/OS: `vm.max_map_count` (~32,765), FDs, JVM heap | ~600k/broker after tuning |

Sources: [librdkafka CONFIGURATION.md](https://github.com/confluentinc/librdkafka/blob/master/CONFIGURATION.md), [KafkaProducer javadoc](https://kafka.apache.org/31/javadoc/org/apache/kafka/clients/producer/KafkaProducer.html), [KAFKA-19519](https://issues.apache.org/jira/browse/KAFKA-19519), [Instaclustr Part 3](https://www.instaclustr.com/blog/apache-kafka-kraft-abandons-the-zookeeper-part-3-maximum-partitions-and-conclusions/).

**[Synthesis]** Note that every failure Instaclustr hit at 600k partitions — `vm.max_map_count`, file descriptors, JVM heap — is a JVM/OS artifact a Rust broker does not have.

### 10.2 The dominant factor: subscription style **[Documented]**

From `ConsumerMetadata.newMetadataRequestBuilder()` in the Java client:

```java
if (subscription.hasPatternSubscription())
    return MetadataRequest.Builder.allTopics();   // ← full cluster, every 5 min
```

Client-side regex → **full-cluster metadata, unconditionally**. A Metadata response costs ~42 bytes/partition at RF=3 (independently confirmed at ~50 B on the [Kafka dev list](https://www.mail-archive.com/dev@kafka.apache.org/msg147981.html)). At 1M partitions with 1,000 such clients on the default 300s refresh, that's **~140–330 MB/s of pure metadata traffic with zero payload**.

**With explicit topic lists**, each client's cost is bounded by its own subscription and the protocol isn't binding until ~10⁶ partitions. **This is a 100× swing** — the difference between O(1) and O(cluster) per client per refresh.

**KIP-848's RE2J `SubscribedTopicRegex` is the fix**: regex evaluated server-side, client requests metadata only for assigned topic IDs. It also converts the ~1 MB-per-*group* ceiling into ~1 MB-per-*member*.

**⚠️ Topic-per-tenant is the trap.** At 1M single-partition topics, topic names and UUIDs cost roughly as much as the partition records — ~100 MB per full Metadata response, within a factor of one of the broker's default `socket.request.max.bytes` (100 MB).

### 10.3 WarpStream amplifies this deliberately **[Documented]**

WarpStream recommends **shortening** metadata refresh to 60 s across all clients (`metadata.max.age.ms: 60000`) so the cluster can rebalance load quickly ([Tuning for Performance](https://docs.warpstream.com/warpstream/kafka/configure-kafka-client/tuning-for-performance)). **[Synthesis]** That is a 5× amplification of metadata cost, adopted specifically by an object-storage-native system — the diskless architecture *wants* faster metadata refresh, pushing harder on exactly this bottleneck. They do not discuss the interaction with high partition counts anywhere found.

### 10.4 Virtual Clusters are the proven answer **[Documented + Synthesis]**

A WarpStream **Virtual Cluster** is "an isolated metadata namespace" with its own replicated state machine ([Architecture](https://docs.warpstream.com/warpstream/overview/architecture)). **[Synthesis]** The key property their docs don't state: a metadata namespace boundary is also a **Metadata-response boundary**. A client bootstrapped against one never sees the others' topics, so the O(topics) response is bounded per tenant rather than per fleet.

**Agent Groups do NOT help** — they isolate compute and network, and explicitly still expose all topics: "clients in each VPC will be able to write and read data for all topics and partitions" ([Agent Groups](https://docs.warpstream.com/warpstream/kafka/advanced-agent-deployment-options/agent-groups)).

### 10.5 Vendor claims vs. the empty record **[Documented]**

| System | Published partition claim |
|---|---|
| AutoMQ | Dev 2,000 / Pro 20,000 / Enterprise "unlimited" |
| WarpStream | A limit exists, tier-dependent, **not published** |
| Bufstream, StreamNative Ursa | **No number published** |
| Confluent Cloud Enterprise | 3,000/eCKU, 96,000/cluster |
| AWS MSK | 4,000/broker (m5.4xlarge+) |

**[Synthesis]** Nobody has published rebalance times above 1,000 partitions, any fetch-session-cache thrash threshold, or a protocol-side partition ceiling. Publishing those measurements would be a genuine contribution and a differentiator.

---

## 11. Design recommendations

1. **Adopt the tiered split**: coordinator for the uncompacted hot tail; per-partition manifest at a derived key for compacted history. Both halves are production-proven separately (§5).
2. **Two-level manifests from day one** — non-negotiable (friction 3). A flat per-partition manifest is viable only at short retention, which defeats the purpose.
3. **Manifests written only by compaction, never by produce** (friction 4). ~15 CAS/s on S3, ~1/s on GCS.
4. **Coordinator holds a reconciliation watermark per partition** — the tail/history boundary (friction 2).
5. **Soft-delete GC with delay > max manifest staleness** (friction 5), same inequality as [13](13-coordinator-recovery.md) §8.
6. **Coarse coordinator index + in-object footer index** — forced by the state arithmetic in §3 at large active working sets.
7. **State the narrower claim**: O(uncompacted window) for the *read-path index*, not for all coordinator state. Retention, GC, `ListOffsets`-by-timestamp, and consumer-group state still need an authority.
8. **Prefer derived-key GET over LIST everywhere** — turbopuffer's own 500ms–1s-per-1000-entries experience (§9.2e) and Thanos's $5,000/month bill (§6) both point the same way.
9. **Make metadata-namespace sharding (Virtual Clusters) the default unit of tenancy**, not an enterprise add-on — it is the proven answer to protocol-side metadata blowup (§10.4).
10. **Treat "does this client request all topics?" as a first-class per-connection observable.** It determines whether the cluster scales to 10⁴ or 10⁶ (§10.2).
11. **Make KIP-848 + RE2J the strongly-preferred subscription path**; consider client-side pattern subscription a degraded mode.
12. **Implement KIP-951 semantics from day one** — inline leader hints in Produce/Fetch error responses keep metadata refresh off the leadership-change path, which matters more when leadership is cheap and mobile.

---

## 12. Open questions and unpublished figures

**The one open risk:** why Redpanda Cloud Topics chose an LSM metastore for L1 rather than derivable per-partition paths, having already built the latter. Undocumented. Worth probing actively — it is the only evidence that someone who could have built this looked at it and went another way.

**Not published anywhere** (stated rather than estimated):
- Any before/after metadata-size ratio for compaction, except Redpanda's 98%-memory figure.
- Manifest size, update frequency, or aggregate storage cost at turbopuffer's 250M namespaces.
- What fraction of turbopuffer namespaces are active at any time.
- Measured rebalance times above 1,000 partitions, on any protocol.
- Any fetch-session-cache thrash threshold.
- Protocol-side partition ceilings from WarpStream, Bufstream, or Ursa.
- Whether anyone multiplexes many logical streams onto fewer Kafka partitions behind a protocol-compatible routing layer — appears to be genuinely undocumented territory.

**Ambiguity flagged:** turbopuffer's changelog notes "Listing namespaces is now consistent" (Jan 2026). Since S3/GCS LIST has been strongly consistent since 2020, a *previously inconsistent* listing implies something derived — possibly a secondary index for the list API specifically. Cannot rule out that a namespace index exists.

---

## Sources

**Kafka KIPs and mailing list**
- [KIP-405: Kafka Tiered Storage](https://cwiki.apache.org/confluence/display/KAFKA/KIP-405%3A+Kafka+Tiered+Storage) · [Jun Rao's DISCUSS comment (LIST cost + consistency)](https://www.mail-archive.com/dev@kafka.apache.org/msg103639.html) · [RLMM default implementation debate](https://www.mail-archive.com/dev@kafka.apache.org/msg103043.html) · [strong vs eventual consistency split](https://www.mail-archive.com/dev@kafka.apache.org/msg110160.html)
- [KIP-852: Optimize calculation of size for log in remote tier](https://cwiki.apache.org/confluence/display/KAFKA/KIP-852:+Optimize+calculation+of+size+for+log+in+remote+tier)
- [KIP-1163: Diskless Core](https://cwiki.apache.org/confluence/display/KAFKA/KIP-1163:+Diskless+Core) · [KIP-1164: Diskless Coordinator](https://cwiki.apache.org/confluence/display/KAFKA/KIP-1164:+Diskless+Coordinator) · [KIP-1165: Object Compaction for Diskless](https://cwiki.apache.org/confluence/display/KAFKA/KIP-1165:+Object+Compaction+for+Diskless)
- [KIP-848](https://cwiki.apache.org/confluence/display/KAFKA/KIP-848%3A+The+Next+Generation+of+the+Consumer+Rebalance+Protocol) · [KIP-227](https://cwiki.apache.org/confluence/display/KAFKA/KIP-227:+Introduce+Incremental+FetchRequests+to+Increase+Partition+Scalability) · [KIP-516](https://cwiki.apache.org/confluence/display/KAFKA/KIP-516:+Topic+Identifiers) · [KIP-526](https://cwiki.apache.org/confluence/display/KAFKA/KIP-526:+Reduce+Producer+Metadata+Lookups+for+Large+Number+of+Topics) · [KAFKA-15868 / KIP-951](https://issues.apache.org/jira/browse/KAFKA-15868)
- [KAFKA-19519: group coordinator max record size](https://issues.apache.org/jira/browse/KAFKA-19519) · [KAFKA-19427: coordinator OOM](https://issues.apache.org/jira/browse/KAFKA-19427)
- [MetadataResponse.json](https://github.com/apache/kafka/blob/trunk/clients/src/main/resources/common/message/MetadataResponse.json) · [ConsumerMetadata.java](https://github.com/apache/kafka/blob/trunk/clients/src/main/java/org/apache/kafka/clients/consumer/internals/ConsumerMetadata.java) · [dev@kafka: metadata for subset of partitions](https://www.mail-archive.com/dev@kafka.apache.org/msg147981.html)
- [Instaclustr: Kafka Diskless proposals](https://www.instaclustr.com/support/documentation/announcements/apache-kafka-and-kafka-connect/kafka-diskless-proposals-status-insights/)

**Redpanda**
- [Tiered storage architecture deep dive](https://www.redpanda.com/blog/tiered-storage-architecture-deep-dive) · [Cloud Topics architecture](https://www.redpanda.com/blog/cloud-topics-architecture) · [Cloud Topics: the Metastore](https://www.redpanda.com/blog/cloud-topics-metastore)
- [Tiered Storage docs](https://docs.redpanda.com/current/manage/tiered-storage/) · [Object storage properties](https://docs.redpanda.com/current/reference/properties/object-storage-properties/) · [Remote Read Replicas](https://docs.redpanda.com/current/manage/remote-read-replicas/) · [Whole Cluster Restore](https://docs.redpanda.com/current/manage/disaster-recovery/whole-cluster-restore/)
- [`partition_path_utils.cc`](https://raw.githubusercontent.com/redpanda-data/redpanda/dev/src/v/cloud_storage/partition_path_utils.cc) · [`remote_path_provider.cc`](https://raw.githubusercontent.com/redpanda-data/redpanda/dev/src/v/cloud_storage/remote_path_provider.cc)

**Other implementations**
- [Aiven: writes and metadata flow](https://aiven.io/blog/apache-kafka-tiered-storage-in-depth-how-writes-and-metadata-flow) · [reads and deletes flow](https://aiven.io/blog/kafka-tiered-storage-in-depth-how-reads-and-deletes-flow) · [Red Hat: tiered storage deep dive](https://developers.redhat.com/articles/2024/03/13/kafka-tiered-storage-deep-dive)
- [PIP-17: Tiered storage for Pulsar topics](https://github.com/apache/pulsar/wiki/PIP-17:-Tiered-storage-for-Pulsar-topics)
- [AutoMQ: Metadata Management](https://www.automq.com/blog/insight-metadata-management-in-automq) · [Efficient Data Organization: Compaction](https://www.automq.com/blog/automq-efficient-data-organization-in-object-storage-compaction) · [S3 Storage docs](https://docs.automq.com/automq/architecture/s3stream-shared-streaming-storage/s3-storage) · [AutoMQ FAQ](https://www.automq.com/faq)
- [WarpStream Architecture](https://docs.warpstream.com/warpstream/overview/architecture) · [Taking out the Trash](https://www.warpstream.com/blog/taking-out-the-trash-garbage-collection-of-object-storage-at-massive-scale) · [Tuning for Performance](https://docs.warpstream.com/warpstream/kafka/configure-kafka-client/tuning-for-performance) · [Agent Groups](https://docs.warpstream.com/warpstream/kafka/advanced-agent-deployment-options/agent-groups)

**turbopuffer**
- [Architecture](https://turbopuffer.com/docs/architecture) · [Limits](https://turbopuffer.com/docs/limits) · [Guarantees](https://turbopuffer.com/docs/guarantees) · [Concepts](https://turbopuffer.com/docs/concepts) · [Sharding](https://turbopuffer.com/docs/sharding) · [Tradeoffs](https://turbopuffer.com/docs/tradeoffs)
- [CMU Database Seminar transcript](https://turbopuffer.com/blog/video-andy-pavlo-cmu) · [Postgres FM transcript](https://postgres.fm/episodes/turbopuffer/transcript) · [launch post](https://turbopuffer.com/blog/turbopuffer)

**Table formats and TSDB analogues**
- [Dremio: Iceberg architectural look](https://www.dremio.com/blog/apache-iceberg-an-architectural-look-under-the-covers/) · [e6data: Iceberg metadata at massive scale](https://www.e6data.com/blog/apache-iceberg-million-files-metadata) · [IOMETE maintenance runbook](https://iomete.com/resources/blog/iceberg-maintenance-runbook)
- [Delta Lake PROTOCOL.md](https://github.com/delta-io/delta/blob/master/PROTOCOL.md)
- [Cortex: bucket index proposal](https://cortexmetrics.io/docs/proposals/blocks-storage-bucket-index/) · [Mimir: bucket index](https://grafana.com/docs/mimir/latest/references/architecture/bucket-index/) · [thanos#6468: objmeta proposal](https://github.com/thanos-io/thanos/issues/6468) · [Thanos Store Gateway](https://thanos.io/tip/components/store.md/)
- [InfluxDB 3 storage engine architecture](https://docs.influxdata.com/influxdb3/clustered/reference/internals/storage-engine/)

**Object storage primitives / LSM**
- [Chris Douglas: Conditional Operations in Object Stores](https://cdouglas.github.io/posts/2026/01/conditional) · [AWS: multi-writer applications on S3](https://aws.amazon.com/blogs/storage/building-multi-writer-applications-on-amazon-s3-using-native-controls/)
- [RocksDB: Level-based Compaction Changes](https://rocksdb.org/blog/2017/06/26/17-level-based-changes.html) · [Partitioned Index Filters](https://github.com/facebook/rocksdb/wiki/Partitioned-Index-Filters)

**Client limits and multi-tenancy**
- [librdkafka CONFIGURATION.md](https://github.com/confluentinc/librdkafka/blob/master/CONFIGURATION.md) · [KafkaProducer javadoc](https://kafka.apache.org/31/javadoc/org/apache/kafka/clients/producer/KafkaProducer.html) · [Conduktor: librdkafka vs Java client](https://www.conduktor.io/blog/librdkafka-vs-java-client)
- [Confluent: 200K partitions per cluster](https://www.confluent.io/blog/apache-kafka-supports-200k-partitions-per-cluster/) · [Kafka without ZooKeeper sneak peek](https://www.confluent.io/fr-fr/blog/kafka-without-zookeeper-a-sneak-peek/)
- [Instaclustr: KRaft max partitions](https://www.instaclustr.com/blog/apache-kafka-kraft-abandons-the-zookeeper-part-3-maximum-partitions-and-conclusions/) · [KIP-848 rebalance benchmark](https://www.instaclustr.com/blog/rebalance-your-apache-kafka-partitions-with-the-next-generation-consumer-rebalance-protocol/)
- [AWS MSK best practices](https://docs.aws.amazon.com/msk/latest/developerguide/bestpractices.html) · [Confluent Cloud cluster types](https://docs.confluent.io/cloud/current/clusters/cluster-types.html)
- [FOLIO RFC 0002: Kafka tenant collection topics](https://github.com/folio-org/rfcs/blob/master/text/0002-kafka-tenant-collection-topics.md)
