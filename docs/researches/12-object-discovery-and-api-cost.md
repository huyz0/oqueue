---
title: "S3 API Cost and the Read-Path Object-Discovery Problem (Avoiding LIST)"
slug: object-discovery-and-api-cost
status: draft
last_updated: 2026-08-13
tags: [s3-api-cost, list-cost, metadata-cache, object-discovery, consistency, compaction, ranged-get, distributed-mmap, kip-1164, iceberg-manifest, tail-reads, push-vs-pull, soft-affinity]
related: [06-distributed-systems-design-challenges, 04-object-storage-s3-gcs, 11-low-latency-tiers-and-interaz-costs, 01-warpstream-architecture, 08-turbopuffer-lessons]
summary: >
  The consequence of write-side bundling: once many partitions share one
  object, LIST cannot answer "which object holds offset N of partition P" —
  and costs 12-38x a GET anyway. How WarpStream, AutoMQ, Kafka Diskless,
  Iceberg, and turbopuffer all solve this with an authoritative offset→object
  index instead; why immutable objects make the reader-side metadata cache
  trivially consistent; a dedicated treatment of the tail-read path (§4.5 —
  why sync fan-out is wrong, why async-push + blocking-read is equivalent
  from the consumer's view, and how turbopuffer's single-node routing only
  partially transfers); and the specific staleness hazards that still need
  handling. Includes a worked cost model (~354x difference).
---

# S3 Object-Discovery on the Read Path: Avoiding LIST in a Kafka-Compatible, Object-Storage-Native Broker

> **⚠️ CORRECTION (2026-08-13) — AutoMQ figures in this document.** A source study at commit `eccde72` ([16-automq-deep-dive.md](16-automq-deep-dive.md) §2) found several claims here, taken from AutoMQ's blog, do not match their code: compaction runs every **5 minutes** not 20 (the 120-min constant is *force-split*, a different mechanism); the stream-graduation threshold is **8 MiB** not 16; there are **five** compaction types, not two; and `DataBlockIndex`'s 36 bytes are `streamId(8) + startOffset(8) + endOffsetDelta(4, a u32 delta) + recordCount(4) + startPosition(8) + size(4)` — §3.2 below omits `recordCount` and wrongly gives `endOffset` as absolute. Also, AutoMQ avoids LIST on the **data path** only; their **WAL path does LIST** (`objectStorage.list(nodePrefix)` at startup and on epoch change).

**Research date:** 2026-08-13. Prices are US East (N. Virginia) / us-central1 list prices unless noted.

**Reading guide:** sections marked **[Documented]** describe what real systems verifiably do, with citations. Sections marked **[Synthesis]** are analysis, cost models, and design recommendations — treat those numbers as parameterized estimates, not published figures.

> **How this fits the project:** this is the direct consequence of the write-path batching justified in [06](06-distributed-systems-design-challenges.md) §2 and [11](11-low-latency-tiers-and-interaz-costs.md). Read [06](06-distributed-systems-design-challenges.md) §1 (metadata problem) and §3 (read/caching problem) first — this doc is the detailed answer to "how does a reader find its data, and what does that cost."

---

## 0. Executive summary

The write-path optimization (bundle many topic-partitions into one large object to amortize PUTs) and the read-path problem (find which object holds offset range `[N, M)` of partition P) are two halves of the same design. Once you bundle, **LIST is not merely expensive — it is semantically useless**: the object key contains no partition identity at all, so listing a prefix tells you nothing about which objects contain your partition. You would have to LIST *and then* open every candidate object's footer. Every production system in this space (WarpStream, AutoMQ, Kafka Diskless, and by analogy Iceberg/Delta/turbopuffer) reaches the same conclusion: **maintain an explicit, authoritative offset→object index outside of object storage, and never list.**

The three load-bearing facts:

1. **LIST costs the same as PUT on every major cloud** — 12.5× a GET on S3 Standard, ~13× on GCS, 12.5× on R2. Ranged GETs cost the *same request price* as full GETs, which is what makes "one big bundled object, many small ranged reads" economically viable.
2. **Immutable objects make the metadata cache trivially safe.** A cached index entry can never be *wrong* about content — only *incomplete*. And "incomplete" maps exactly onto Kafka's existing tail-polling semantics (empty fetch → retry), so stale-but-monotonic reads need no new client-visible behavior.
3. **The two things that must NOT be served stale** are `ListOffsets(LATEST)` / high-watermark reporting and post-GC object existence. Both have well-understood fixes (route those to the authoritative coordinator; soft-delete-then-reap with a delay exceeding the staleness bound).

---

## 1. Object-storage API request pricing, precisely

### 1.1 Amazon S3 Standard (us-east-1) **[Documented]**

| Operation class | Price | Per-request |
|---|---|---|
| PUT, COPY, POST, **LIST** | **$0.005 / 1,000** | $5.0×10⁻⁶ |
| GET, SELECT, and all other | **$0.0004 / 1,000** | $4.0×10⁻⁷ |
| DELETE, CANCEL | **free** | $0 |

Sources: [AWS S3 Pricing](https://aws.amazon.com/s3/pricing/); confirmed by [Vantage](https://www.vantage.sh/blog/amazon-s3-express-one-zone) and independently quoted by [AutoMQ](https://www.automq.com/blog/deep-dive-into-the-challenges-of-building-kafka-on-top-of-s3) ("S3 Standard PUT requests are $0.005 per 1000 requests"). WarpStream's own cost model uses the same constants: "S3 PUT costs $0.000005 and S3 GET costs $0.0000004" ([The Case for Shared Storage](https://www.warpstream.com/blog/the-case-for-shared-storage)).

**The ratio that drives the whole design: LIST / GET = 12.5×.** LIST is in the *expensive* tier, budgeted identically to a PUT.

A second, frequently-missed LIST cost: **the response body is metered as egress** if it leaves AWS. A `ListObjectsV2` page returns XML metadata for up to 1,000 objects at roughly 200–300 bytes/object, i.e. ~200–300 KB per full page. One documented case reached ~75 GB/day of egress (~$202/month) purely from LIST response bodies ([S3 cost analysis](https://go-cloud.io/s3-cost-optimization/), [AWS re:Post](https://repost.aws/questions/QUxVRLdhqbTv-7iFiMLsZ_vA/can-s3-listbucket-requests-result-in-data-transfer-out-charges)). For a broker whose agents are in-VPC this is $0, but it matters for any control-plane component running outside.

There is also a bulk alternative: **S3 Inventory** at roughly **$0.0025 per million objects listed** ([HN discussion](https://news.ycombinator.com/item?id=28917705)) — i.e. ~2,000× cheaper per object than LIST-paginating, but delivered daily/weekly as a manifest file. This is the right tool for *reconciliation/GC sweeps*, and completely wrong for read-path discovery.

### 1.2 Amazon S3 Express One Zone (us-east-1, post-April-2025 repricing) **[Documented]**

AWS cut Express pricing sharply on 2025-04-10 ([AWS News Blog](https://aws.amazon.com/blogs/aws/up-to-85-price-reductions-for-amazon-s3-express-one-zone/)). Many secondary sources still quote the *old* numbers; use these:

| Metric | Old | **Current** | Change |
|---|---|---|---|
| Storage / GB-month | $0.16 | **$0.11** | −31% |
| PUT / 1,000 | $0.0025 (≤512 KB) | **$0.00113** | −55% |
| GET / 1,000 | $0.0002 (≤512 KB) | **$0.00003** | −85% |
| Data upload / GB | $0.008 | **$0.0032** | −60% |
| Data retrieval / GB | $0.0015 | **$0.0006** | −60% |

Critically, "per-GB charges … now apply to **all bytes transferred** rather than just portions of requests greater than 512 KB." The old "first 512 KiB free per operation" model is gone.

**LIST on Express** is grouped with PUT/COPY/POST on the AWS pricing page, so ≈ **$0.00113/1,000** — making the LIST/GET ratio on Express **≈ 37.7×**, i.e. *worse* than Standard. ⚠️ The April 2025 announcement does not restate LIST explicitly; verify against the live pricing page before committing this to a cost model.

**Express-specific LIST landmine [Documented]:** for directory buckets, `ListObjectsV2` **does not return objects in lexicographical order** ([AWS API docs](https://docs.aws.amazon.com/AmazonS3/latest/API/API_ListObjectsV2.html)). Any discovery scheme relying on lexicographic key ordering (e.g. encoding offsets into keys and doing an ordered prefix scan) silently breaks on Express.

### 1.3 Google Cloud Storage **[Documented]**

| Class | Standard | Nearline | Coldline |
|---|---|---|---|
| **Class A** (`objects.insert`, `objects.list`, `objects.copy`, `objects.compose`) | **$0.005 / 1,000** ($0.05/10k) | $0.01 / 1,000 | $0.01 / 1,000 |
| **Class B** (`objects.get`, `objects.getIamPolicy`, HEAD/metadata reads) | **$0.0004 / 1,000** ($0.004/10k) | $0.001 / 1,000 | $0.001 / 1,000 |

Sources: [GCS pricing](https://cloud.google.com/storage/pricing), [Economize breakdown](https://www.economize.cloud/blog/gcp-storage-classes-pricing-features-services/), [CloudZero](https://www.cloudzero.com/blog/gcp-storage-pricing/). Note Google bills Class A per 1,000 and Class B per 10,000 in the SKU catalog; normalized here to per-1,000.

⚠️ **Correction to a common error:** several secondary sources claim `objects.list` is Class B. It is **Class A**. Google's own docs list "GET Bucket (List Objects)" under Class A. Do not design against the wrong assumption — the LIST/GET ratio on GCS Standard is **12.5×**, essentially identical to S3.

### 1.4 Cloudflare R2 **[Documented]**

| Class | Standard | Includes |
|---|---|---|
| Class A | **$4.50 / million** ($0.0045/1,000) | `PutObject`, **`ListObjects`**, `CreateMultipartUpload`, `UploadPart` |
| Class B | **$0.36 / million** ($0.00036/1,000) | `GetObject`, `HeadObject` |
| Egress | **$0** | — |

Source: [R2 pricing](https://developers.cloudflare.com/r2/pricing/). LIST/GET ratio: **12.5×**. Zero egress makes R2 interesting for cross-AZ/cross-region reads, but the LIST-vs-GET *ratio* is identical, so the same architecture applies.

### 1.5 The universal invariant **[Synthesis]**

Across S3 Standard, S3 Express, GCS, and R2, without exception:

```
cost(LIST)  ==  cost(PUT)  ≈  12–38 × cost(GET)
cost(HEAD)  ==  cost(GET)
cost(DELETE) == 0 (S3) or Class A (GCS/R2)
```

This is not a coincidence of pricing — LIST touches a shared, sharded key-index that is far more contended than the immutable-blob read path. Design as if this ratio is permanent.

### 1.6 Byte-range GET pricing — the enabling fact **[Documented]**

**A ranged `GET` costs exactly the same as a full `GET`: one request.** Only the *bytes returned* differ, and on S3 Standard bytes returned to the same region are free. Ranged GETs are cheaper only where there is a per-GB component:

- **S3 Standard:** ranged GET = 1 GET request ($4×10⁻⁷), zero data charge in-region. **A 4 KiB ranged read of a 256 MB object costs the same as a 4 KiB read of a 4 KiB object.**
- **S3 Standard-IA / Glacier IR:** retrieval fee is charged only on bytes actually returned, not object size ([AWS re:Post](https://repost.aws/questions/QU7Cv_MwSlQP6sQkViBvISAg/s3-infrequent-access-retrieval-fee-for-range-requests)).
- **S3 Express One Zone:** ranged GET saves real money now, since retrieval is $0.0006/GB on *all* bytes transferred.

**This is the single fact that makes the whole bundled-object architecture work.** You can write one 32 MB object containing 500 partitions' data and let 500 independent readers each pull their own 64 KB slice, paying 500 GETs ($0.0002) — with no penalty whatsoever for the object being large. AutoMQ's format ([IndexBlock + 48-byte footer](https://www.automq.com/blog/parsing-the-file-storage-format-in-automq-object-storage)) and Kafka Diskless's `fetch(ObjectKey key, ByteRange range)` interface ([KIP-1163](https://cwiki.apache.org/confluence/display/KAFKA/KIP-1163:+Diskless+Core)) both depend on it.

---

## 2. Why LIST is specifically wrong for this workload

### 2.1 The semantic objection comes first **[Synthesis]**

Before cost: **with bundled objects, LIST cannot answer the question.** The whole point of bundling is that object `wal/01J8K.../uuid` contains records from partitions `orders-7`, `clicks-33`, `logs-901`, … The key encodes nothing about partition membership. Kafka Diskless makes this explicit: WAL segments get "globally unique (e.g. UUID) names and uploaded without coordination", and "batches within a WAL Segment are ordered by their layout in the object, but do not have any inherent order relative to other WAL Segments" ([KIP-1163](https://cwiki.apache.org/confluence/display/KAFKA/KIP-1163:+Diskless+Core)).

So a LIST-based reader would need: LIST the prefix (N pages) → for each candidate object, GET its footer/index to see if it contains your partition → then ranged-GET the data. That is `pages × LIST + objects × GET_footer + k × GET_data` instead of `1 × index_lookup + k × GET_data`. This is a *worse-than-linear* discovery scan, not merely a pricier one.

The only way to make LIST work is to write one object per partition — which reintroduces exactly the PUT-amplification problem you bundled to avoid. WarpStream quantifies that dead end: a naive per-partition approach writing a file every 250ms costs "**~$50/month in PUT operations alone per partition**" ([Minimizing S3 API Costs with Distributed mmap](https://www.warpstream.com/blog/minimizing-s3-api-costs-with-distributed-mmap)). At 10,000 partitions that is $500k/month in PUTs.

### 2.2 Cost scaling **[Synthesis, using documented unit prices]**

LIST cost scales as `partitions × consumers × poll_rate` in the naive case, or at best `partitions × discovery_rate` with perfect cross-consumer caching.

| Discovery policy | LIST/s | LIST/day | $/day (S3 Std) | $/month | Discovery lag |
|---|---|---|---|---|---|
| Per-fetch, 200 consumers × 50 parts, 2 polls/s | 20,000 | 1.73 B | **$8,640** | $259k | ~0 |
| Per-partition, 1/s, shared cache (10k parts) | 10,000 | 864 M | **$4,320** | $130k | ≤1 s |
| Per-partition, 1/10s | 1,000 | 86.4 M | $432 | $13.0k | ≤10 s |
| Per-partition, 1/60s | 167 | 14.4 M | $72 | $2,160 | ≤60 s |
| **Coordinator index (no LIST)** | **0** | **0** | **$0** | **$0** | ~1 ms |

Note the shape: even at a 60-second discovery interval — which is *already unusable* for tail-latency Kafka semantics — you are paying $2,160/month for pure metadata discovery, and you still haven't fetched a single byte.

### 2.3 Pagination and latency **[Documented]**

`ListObjectsV2` returns **at most 1,000 keys per request**; `MaxKeys` defaults to 1,000 and "the response might contain fewer keys but will never contain more". Continuation is via an opaque `NextContinuationToken`, which is **strictly serial** — you cannot parallelize pagination because you cannot guess the next token ([AWS API docs](https://docs.aws.amazon.com/AmazonS3/latest/API/API_ListObjectsV2.html)).

So listing latency is `ceil(keys/1000) × RTT`, unavoidably serialized. For scale: at WarpStream's 250 ms flush cadence, **one hour of ingest produces 14,400 objects per writer agent** — 15 serial LIST pages minimum per agent-hour of history, per discovery attempt.

Comparison points for latency: S3 `GetObject` median is ~15 ms, p90 ~60 ms (vs. NVMe at 20–100 µs — "1000x faster") per [AutoMQ](https://www.automq.com/blog/deep-dive-into-the-challenges-of-building-kafka-on-top-of-s3). LIST is generally slower than GET because it hits the sharded key index rather than the blob path.

### 2.4 Rate limits and operational pain **[Documented]**

WarpStream hit this directly and wrote it up. From [Taking out the Trash: Garbage Collection of Object Storage at Massive Scale](https://www.warpstream.com/blog/taking-out-the-trash-garbage-collection-of-object-storage-at-massive-scale):

> "listing files in commodity object stores is a notoriously slow and expensive operation that **can easily lead to rate limits being tripped**"

and, on the natural companion operation:

> "obtaining the file's age requires issuing a HEAD request against the file which **costs money as well**"

and on how it degrades with growth:

> as customer clusters scaled, they had to "allow the reconciliation process to run faster and faster, **which increased costs even further**"

AutoMQ states the same conclusion in one sentence: S3 LIST requests, used the way Kafka scans a filesystem directory tree for segments, "**unfortunately, these requests perform poorly**" ([AutoMQ](https://www.automq.com/blog/deep-dive-into-the-challenges-of-building-kafka-on-top-of-s3)).

The Iceberg community reached this independently in the analytics world: "Listing partitions to plan a read is expensive, especially when using S3." Netflix's motivating result was that with manifests, "reading a snapshot requires **O(1) RPC calls**" versus "listing O(n) directories".

### 2.5 Consistency is *not* the problem (anymore) **[Documented]**

Since 2020-12-01, S3 provides strong read-after-write consistency for **GET, PUT, and LIST**, "without changes to performance or availability … and at no additional cost" ([AWS](https://aws.amazon.com/s3/consistency/)). So LIST-based discovery is no longer *incorrect* — it is merely slow, expensive, unparallelizable, and semantically incapable of answering the bundled-object question. Don't let "S3 is strongly consistent now" talk you into it.

---

## 3. How real systems do object discovery without listing **[Documented]**

### 3.1 WarpStream — control-plane metadata store + per-AZ distributed mmap

**Write path** ([docs](https://docs.warpstream.com/warpstream/overview/architecture/write-path)):
- Agents "buffer data for 250 ms, or until 8 MiB of data has been accumulated, whichever comes first". (The 2023 blog cites 4 MiB; the docs now say 8 MiB — the knob has moved.)
- "the WarpStream Agents create individual files that contain data from many different topic-partitions", with data "sorted first by topic, and then by partition" so each partition's bytes are contiguous within the object.
- Then: "the Agent commits the file metadata to the WarpStream Metadata Store, and then acknowledges all of the `Produce()` requests". And crucially: "WarpStream **never acknowledges writes until data is durably persisted in object storage *and* committed to the metadata store**."
- Offset assignment happens at commit, not at flush: "WarpStream determines the order of writes upon *committing* a batch to the WarpStream Metadata Store, not when the data is flushed to object storage." This is what lets many agents PUT in parallel without coordination.

**The index** ([architecture](https://docs.warpstream.com/warpstream/overview/architecture)): each Virtual Cluster "is a replicated state machine which stores **the mapping between files in object storage and ranges of offsets in each Kafka topic-partition**." The control plane holds metadata only — "never payload data" — which is their security story for a hosted control plane and is worth noting for an OSS design where the control plane could be self-hosted.

**Read path** ([docs](https://docs.warpstream.com/warpstream/overview/architecture/read-path)) — the API shape:

> "The Agent queries the Metadata Store for **the file(s) and batches in which these offsets are contained**." The store returns "**an ordered list of files and batches** that the Agent should read" to maintain Kafka ordering semantics.

So the query is roughly `find(topic, partition, start_offset, max_bytes) → ordered [(file_id, batch_descriptor)]`. WarpStream has not published the exact wire schema; that is the level of detail publicly available.

**Distributed mmap** ([Minimizing S3 API Costs with Distributed mmap](https://www.warpstream.com/blog/minimizing-s3-api-costs-with-distributed-mmap), Oct 2023) — three mechanisms:

1. **Consistent hashing ring**: file IDs hash to an Agent, which becomes the caching owner for that file, "ensuring good load distribution amongst all the Agents." Fetch requests are *forwarded* to the owning Agent, not served locally.
2. **4 MiB aligned block paging**: the cache pages from object storage "in fixed (and aligned) size chunks of **4 MiB** *regardless of how large the initiating IO was*", then reuses the cached block.
3. **Scan sharing**: multiple concurrent fetches for *different partitions* land in the same 4 MiB block and are satisfied by **one** object-storage request.

Net effect, in their words: this "completely **decouples the number of partitions and consumers from the number of object storage GET requests**" while holding ~400 ms p99 write latency with no local disks. Their headline comparison: Kafka's inter-zone bandwidth for ~560 MiB/s costs **$641/day**, versus **<$40/day** in object-storage API costs for WarpStream.

**GC without LIST** ([Taking out the Trash](https://www.warpstream.com/blog/taking-out-the-trash-garbage-collection-of-object-storage-at-massive-scale)): file lifecycle is tracked in the metadata store across three logical states — **live** → **logically deleted** (TTL / topic deletion / consumed by compaction) → **deletion queue**. Physical deletion is deferred: "files deleted from the metadata store are first durably enqueued, then deleted later **after a sufficient delay to avoid disrupting live queries**." The delay exists precisely because a query is two steps ("Query the metadata store for relevant files", then "Execute the query on the relevant files") and the file must survive the gap. They run a hybrid: an **optimistic deletion queue** (a large buffered Go channel fed by compaction outputs, since "compactions should be the largest source of deleted files" in their LSM-structured engine) plus a slower **reconciliation / mark-and-sweep** loop as a safety net for orphans.

**Compaction** ([docs](https://docs.warpstream.com/warpstream/overview/architecture)): a pool of Agents "compacts those small files into larger files to make reprocessing historical data … both cost effective and high throughput", and the streaming compaction algorithm "requires little memory and **only results in one additional GET request per input file (no matter how large it is)**." That last clause is the key to compaction economics (§6).

### 3.2 AutoMQ — metadata in KRaft, pushed to every broker

**The explicit rejection of LIST** ([AutoMQ](https://www.automq.com/blog/deep-dive-into-the-challenges-of-building-kafka-on-top-of-s3)): Kafka scans the filesystem directory tree to list a partition's segments; the S3 equivalent is LIST, and "these requests perform poorly." Instead, "Meta S3Stream … records the Segment list and mapping between Segments and Streams. This also helps **avoid sending a LIST request to object storage**."

**Object model** ([AutoMQ docs](https://docs.automq.com/automq/architecture/s3stream-shared-streaming-storage/s3-storage)):
- **Stream Set Object (SSO)** — the bundled object: "When uploading WAL data, the majority of smaller Streams are consolidated and uploaded as a single Stream Set Object."
- **Stream Object (SO)** — single-stream, produced by compaction: "contains data from a single Stream, facilitating precise data deletion for Streams with different lifecycles."
- A Kafka partition maps to a **Metadata Stream** (indices, leader-epoch snapshots, producer snapshots) plus a **Data Stream**.

**Where the index lives** ([Insight: Metadata Management in AutoMQ](https://www.automq.com/blog/insight-metadata-management-in-automq)): AutoMQ "extends KRaft to implement a stream storage metadata management mechanism" — all metadata changes propagate as KRaft Records, and therefore "**each broker caches the latest metadata information**", enabling local indexing without any remote query. This is the purest "replicate the whole index to every reader" model of the three.

Metadata categories: KV (streamId ↔ MetaStream mappings), Stream (epoch, start/end offsets, **Ranges** = offset ranges per node, StreamObjects with their offset ranges), Node (id, epoch, failover mode, StreamSetObjects with offsets), Object (status, size, key, expiration, deletion timestamp).

**The lookup algorithm**, in four steps:
1. Search the local **StreamObjects** cache for continuous segments.
2. **Binary search the Ranges** (ordered offset→node list) to find the owning node.
3. Traverse that node's **StreamSetObjects** to match the segment.
4. Repeat if data remains.

And the honest cost admission: "the majority of indexing costs are spent on **searching the StreamSetObject**" because SSOs aggregate many streams and must be traversed. Their fix is compaction, "enabling most data of the Stream to reside within the StreamObject" — i.e. converting expensive SSO traversal into cheap direct SO lookup. **This is a direct warning for our design: the bundled-object index is the expensive part, and compaction is how you retire it.**

**Two-level indexing** ([AutoMQ object format](https://www.automq.com/blog/parsing-the-file-storage-format-in-automq-object-storage)): the *object itself* carries an index, so the cluster metadata only has to name the object.
- **DataBlocks** (records) + **IndexBlock** (36-byte entries: `streamId, startOffset, endOffset, position, blockSize`) + **48-byte Footer** (location/size of IndexBlock).
- IndexBlock entries are "sorted by (streamId, startOffset), enabling quick location of the actual DataBlock through **binary search**."
- Read path: GET the 48-byte footer → GET the IndexBlock → binary search by `(streamId, offset)` → **ranged GET the identified DataBlock** → scan `StreamRecordBatch` entries.

That's 3 GETs cold, 1 GET warm (footer+index cached). Compare to WarpStream/Diskless where the coordinator holds the byte range directly: 1 GET always, but a much larger central index.

**Compaction cadence** ([docs](https://docs.automq.com/automq/architecture/s3stream-shared-streaming-storage/s3-storage)): SSO compaction every **20 minutes**; streams exceeding **16 MiB** graduate to their own Stream Object; the rest merge into new SSOs via merge-sort (handles up to 15 TiB under 500 MiB of memory). Stream Object compaction uses the **MultiPartCopy** API for range copies, "avoiding network bandwidth waste from read-then-write cycles" — a notable trick: server-side copy means you pay PUT-class requests but no download bandwidth.

### 3.3 Apache Kafka Diskless — KIP-1150 / 1163 / 1164 / 1165

This is the closest published analogue to our design and the most explicit about the consistency model.

**Batch coordinates** ([KIP-1163](https://cwiki.apache.org/confluence/display/KAFKA/KIP-1163:+Diskless+Core)): a coordinate is `(ObjectKey, ByteRange)` where `ByteRange(long offset, long size)`. The pluggable storage interface is exactly three methods:

```java
void        upload(ObjectKey key, InputStream inputStream, long length)
InputStream fetch (ObjectKey key, ByteRange range)
void        delete(Set<ObjectKey> keys)
```

**Note what is absent: there is no `list()`.** The plugin interface for a production diskless Kafka does not expose listing at all. That is the clearest possible statement of the design principle.

**WAL Segment format** ([KIP-1163](https://cwiki.apache.org/confluence/display/KAFKA/KIP-1163:+Diskless+Core)):
- 1-byte version header (currently `0`).
- "Batches are **grouped by partition into contiguous parts of the object**" — same contiguity trick as WarpStream, so one partition = one ranged GET.
- Objects get "globally unique (e.g. UUID) names and **uploaded without coordination**."
- "batches within a WAL Segment are ordered by their layout in the object, but **do not have any inherent order relative to other WAL Segments**" — ordering comes only from the coordinator.

**The Batch Coordinator** is "the source of truth about batch coordinates and WAL Segments, establishing a global order of batches within a diskless topic and serving coordinates to enable retrieval." Brokers "buffer incoming records, fuse them into shared-log segments (typically a few megabytes, tunable), upload to object storage via a single PUT, then transmit a compact **coordinate list** (object ID and byte ranges) to the Coordinator", which "merg[es] multiple local sequences into one monotonic offset stream" ([Aiven's Hitchhiker's Guide](https://aiven.io/blog/guide-diskless-apache-kafka-kip-1150)).

**`DisklessFindBatches`** — the directly relevant API:

> "**DisklessFindBatches**: Find batches in the specified partitions starting from the specified offset."
>
> "This is a **read-only operation with less strict consistency requirements** and **could be served from stale state and by followers**."

On consume: "the broker will call the relevant Batch Coordinator with a `FindBatches` request. The Batch Coordinator will search the requested offsets within its internal state and reply with the batch coordinates (object key, offset within the object)."

The full field-level schema lives in KIP-1164's "Public Interfaces" section. ⚠️ **Citation correction (2026-08-13):** the 404s encountered during this research were not transient — **the KIP was renamed** from "Topic Based Batch Coordinator" to "**Diskless Coordinator**", and the old URL is permanently dead. The live page is [KIP-1164: Diskless Coordinator](https://cwiki.apache.org/confluence/display/KAFKA/KIP-1164:+Diskless+Coordinator), and it *does* specify a snapshot mechanism ("identical to the one in KRaft, see KIP-630") — see [13-coordinator-recovery.md](13-coordinator-recovery.md) §1 and §3 for what the current revision says, including one substantive difference from the older text.

**Storage & caching model of the coordinator** ([KIP-1164](https://cwiki.apache.org/confluence/display/KAFKA/KIP-1164:+Topic+Based+Batch+Coordinator), via search extraction):

- Source of truth is a Kafka topic: **`__diskless_metadata`**.
- "The topic serves as storage and replication medium but **cannot be directly queried performantly**, so the state needs to be **materialized locally**" — and since state may exceed memory, "the materialization mechanism [must] be backed by a disk", leading to **SQLite** as the embedded engine.
- Explicitly: "The SQLite DB will be a **local metadata cache, not the source of truth**, with the source of truth being the `__diskless_metadata` topic."
- "The Batch Coordinator instance will run on the broker that is the **leader** of the partition, while **read-only Batch Coordinator instances will run on all brokers that are in-sync replicas**", the leader handling state-modifying requests and the read-only replicas serving read-only requests.
- "Read-only operations that could be served from a stale view of data, such as `DisklessFindBatches`, could be served **right away after replication step 2, also by a follower**." A distributed cache makes reads efficient.
- Aiven has a working implementation using **Postgres** instead of SQLite.
- [2minutestreaming](https://blog.2minutestreaming.com/p/diskless-kafka-topics-kip-1150) adds: the distributed cache is **Infinispan**, with a copy per availability zone, and "on writes, the cache is also populated. This means a broker writes simultaneously to the cache and to S3." Metadata volume: **~6 KiB of metadata per 16 MiB write**, i.e. ~377.6 KiB/s of coordination traffic for their example workload — negligible.

**This is exactly the architecture hypothesized in the original question**, validated by an Apache KIP: replicated log as source of truth → local materialized index on every reader → stale-tolerant, follower-servable point queries → zero LIST.

**Latency budget** ([Aiven](https://aiven.io/blog/guide-diskless-apache-kafka-kip-1150)): finding coordinates ≈ 10 ms p99; first read of cold history ≈ 100 ms (object-store RTT); subsequent neighboring offsets from RAM at **<20 ms**, "indistinguishable from classic Kafka" for real-time workloads. Segment size is "an economic knob": <128 KiB lowers latency but raises PUT fees; >10 MiB does the opposite. Brokers cache **per-rack** to avoid redundant cross-AZ downloads.

### 3.4 Apache Iceberg — the manifest pattern **[Documented]**

Iceberg solved *precisely* this problem for analytics a decade ago, and the pattern transfers cleanly.

The hierarchy:
```
catalog pointer  →  metadata.json  →  manifest list (1 per snapshot)  →  manifest files (Avro)  →  data files
```

- A **manifest list** "is a file that lists manifest files; one per snapshot" and "includes **summary metadata** that can be used to **avoid scanning all of the manifests** in a snapshot when planning a table scan" ([Iceberg spec](https://iceberg.apache.org/spec/)).
- **Manifest files** are Avro files listing a subset of data files with per-file statistics (row counts, partition values, min/max column bounds) for pruning.
- Design principle: "Iceberg's design addresses specific problems with the hive layout: **file listing is no longer used to plan jobs** and files are created in place without renaming."
- The payoff: "Instead of listing O(n) directories in a table to plan a job, **reading a snapshot requires O(1) RPC calls**." "If a query needs to scan 1 out of 1000 partitions, Iceberg will open maybe a couple of small files … whereas a Hive approach might have to list and open hundreds of file system directories."
- Commits are an **atomic swap of the metadata pointer** in the catalog, with optimistic concurrency and retry-with-validation ([Iceberg reliability](https://iceberg.apache.org/docs/latest/reliability/)).
- LIST is *not* eliminated entirely — it survives in **orphan file cleanup**, a rare maintenance sweep, exactly analogous to WarpStream's reconciliation loop.

**What a streaming broker should borrow [Synthesis]:**
1. **Two-level index.** Iceberg's manifest-list summary is a *pruning* layer over the manifest detail. In our broker: a coarse per-partition "segment table" (partition → list of index-chunk IDs, each covering an offset range) over a fine "offset table" (offset range → object + byte range). A reader needing offsets 1M–1.1M reads one index chunk, not the whole index. This is what keeps the agent-side cache bounded.
2. **Immutable, content-addressed metadata files.** Iceberg manifests are immutable and never rewritten in place; only the pointer moves. Same property we want.
3. **The snapshot/sequence number is the version.** Every Iceberg snapshot has a monotonic `sequence-number`. Cache staleness is a single integer comparison. (§4.1)
4. **Per-file statistics enable skipping.** Our equivalent: store `(base_offset, last_offset, timestamp_min, timestamp_max)` per entry so `ListOffsets(BY_TIMESTAMP)` and retention/TTL decisions are answerable from the index without touching S3.

**Delta Lake variant [Documented]:** `_delta_log/` holds ordered JSON commits; every 10th commit writes a **Parquet checkpoint** encoding the full table state; `_last_checkpoint` points at the newest one ([Databricks](https://www.databricks.com/blog/2019/08/21/diving-into-delta-lake-unpacking-the-transaction-log.html), [Denny Lee](https://dennyglee.com/2024/01/09/computing-delta-lake-state-quickly-with-checkpoint-files/)). A reader does `listFrom` on the (small, bounded) log directory, jumps to the newest checkpoint, and replays only the JSON commits after it. Note the difference from Iceberg: Delta *does* still list, but only a tiny bounded log directory, never the data files. **The log-plus-periodic-checkpoint structure is a near-exact match for the `__diskless_metadata` + SQLite-materialization pattern** — and for what we'd build if we wanted our metadata store to be S3-native with no external database.

### 3.5 turbopuffer — manifest + CAS on object storage **[Documented]**

turbopuffer ([architecture docs](https://turbopuffer.com/docs/architecture)) is the "no external metadata database at all" end of the spectrum: S3/GCS is both data store *and* coordination primitive. Full treatment in [08-turbopuffer-lessons.md](08-turbopuffer-lessons.md).

- **Per-namespace prefix** containing sequential WAL files (`01.bin`, `02.bin`, …), index files under `/index/`, metadata files, and branch refs.
- **CAS as the commit primitive:** an atomic read-modify-write — GET the manifest with its ETag, modify locally, PUT with `If-Match`. Match → success; someone else won → **412 Conflict** → retry. "For low-frequency coordination — indexing job dispatch, metadata updates, leader election — **S3 CAS eliminates the need for a separate consensus system**."
- **Two cursors:** an "index cursor" (how far indexing has progressed) and a "**CAS commit point**" (how far data is durably committed). Data between them is committed-but-unindexed and remains queryable by exhaustive log scan at ~10 ms overhead. **This is a clean, directly-borrowable pattern: the commit point is our high watermark; the index cursor is our compaction watermark.**
- **Discovery without LIST:** "Queries avoid expensive S3 LIST calls through deterministic routing" — the load balancer routes a namespace to a specific query node holding the manifest and cache. Cold query = 3 roundtrips (~100 ms each, ~400 ms total for 1M docs); warm NVMe cache p50 = **14 ms**; cold direct-from-S3 p50 = 874 ms — a **60×** cache benefit.
- **Consistency:** strong by default (writes visible to subsequent queries); optionally relaxed to eventual for sub-10 ms warm latency, with **worst-case staleness bounded at ~1 hour**. They expose staleness as an explicit, *bounded*, user-selectable knob — a good model.
- **Rate limit:** 1 WAL entry per second per namespace; concurrent writes batch into one entry. This is the fundamental cost of CAS-on-object-storage coordination and is why it doesn't generalize to a high-partition-count broker without sharding.

**[Synthesis] Assessment for oqueue:** CAS-on-S3 gives a zero-dependency control plane (no Postgres, no Raft) — extremely attractive for an OSS project. But **1 commit/s per CAS'd object** is far too slow for a broker committing every 250 ms across many writers. The realistic hybrid: use CAS-on-S3 for *low-frequency* metadata (topic configs, compaction manifests, epoch/leadership, the "checkpoint pointer") and a replicated log (Raft, or a Kafka-topic-style log à la KIP-1164) for the *high-frequency* offset→object commits. S3 conditional writes (`If-Match` / `If-None-Match`) are now supported by S3, GCS, and R2, so this is portable.

### 3.6 Comparison table **[Synthesis of documented facts]**

| | WarpStream | AutoMQ | Kafka Diskless | Iceberg | turbopuffer |
|---|---|---|---|---|---|
| Source of truth for index | Hosted metadata store (replicated state machine per Virtual Cluster) | KRaft log | `__diskless_metadata` Kafka topic | Catalog pointer → `metadata.json` | `manifest` in object storage |
| Distribution to readers | **Pull** (agent queries per fetch) | **Push** (KRaft records to every broker) | **Push** (log replication) + local SQLite materialization | Pull (read manifest files) | Pull (routed node holds it) |
| Reader-side cache | Data cache (4 MiB blocks, consistent-hashed) | Full metadata in every broker's memory | SQLite on disk + Infinispan per-AZ | Engine-side manifest cache | Memory + NVMe per query node |
| Index granularity | file + batch | object + (streamId, offset) via in-object IndexBlock | object key + `ByteRange(offset,size)` | manifest entry + file stats | index cursor + WAL offsets |
| Stale reads allowed? | Not documented | Yes (KRaft lag) | **Yes, explicitly, incl. followers** | Yes (snapshot isolation) | Configurable, ~1 h bound |
| LIST used for | GC reconciliation only | not documented | not in the plugin API at all | orphan-file cleanup only | not used |

---

## 4. Metadata cache consistency — keeping readers correct without adding latency

This section is mostly **[Synthesis]**, anchored on the documented mechanisms above.

### 4.1 Monotonic version / epoch on the metadata snapshot

Every system above has exactly one monotonic integer that defines "how much of the world I have seen":

| System | Version scalar | **[Documented]** |
|---|---|---|
| Kafka Diskless | offset in `__diskless_metadata` | ✅ SQLite is "a local metadata cache, not the source of truth" |
| AutoMQ | KRaft record offset | ✅ brokers cache latest metadata via KRaft records |
| Iceberg | snapshot `sequence-number` | ✅ |
| Delta Lake | commit version `N` in `_delta_log/N.json` | ✅ |
| turbopuffer | CAS commit point (ETag generation) | ✅ |

**Recommended design [Synthesis]:**

```rust
type CommitVersion = u64;   // monotonic, assigned by the coordinator at commit time

// Reader's cached view
struct IndexCache {
    applied_upto: CommitVersion,   // everything < this is fully materialized
    entries: BTreeMap<(TopicPartition, Offset), ObjectRef>,
}

struct ObjectRef {
    object_id:    ObjectId,   // opaque, never reused
    byte_offset:  u32,
    byte_len:     u32,
    base_offset:  i64,
    last_offset:  i64,
    ts_min:       i64,        // for ListOffsets(BY_TIMESTAMP)
    ts_max:       i64,
    epoch:        u32,        // GC/compaction fencing, see §4.6
}
```

Every coordinator response carries the coordinator's current `commit_version`. Every reader-to-coordinator request may carry `min_version` ("I need a view at least this fresh"). **Staleness detection is a single `u64` comparison, never a data comparison** — which is the whole reason to use a scalar version rather than timestamps or hashes.

Three read modes fall out naturally:
- **`Stale` (default, zero-latency):** serve from local cache regardless of version. Used for `Fetch`.
- **`AtLeast(v)`:** block until `applied_upto >= v`, then serve locally. Used for read-your-writes.
- **`Linearizable`:** round-trip to the coordinator leader. Used for `ListOffsets(LATEST)` and any transactional metadata.

This is the same three-tier structure KIP-1164 arrives at (leader for writes, ISR followers for stale reads).

### 4.2 Why immutability makes this dramatically easier

**The core argument [Synthesis]:**

Objects in this architecture are **write-once, never modified**. A committed `ObjectRef` says: "bytes `[o, o+len)` of object `X` are exactly records `[base, last]` of partition P." Because `X` is immutable and its ID is never reused, **that statement is true forever** — or until `X` is physically deleted, which is the *only* failure mode (§4.6).

Therefore a stale cache has exactly one defect: **it is a prefix of the truth.** It knows about entries `[0, applied_upto)`; the truth is `[0, current_version)`. It can never contain a *wrong* entry, only *fewer* entries. Formally, the index is a **grow-only, append-only, monotonic set** — a CRDT in the trivial sense. Merging a delta is idempotent and commutative; there is no invalidation, no write-through, no coherence protocol, no TTL guessing.

Contrast with a mutable-blob design: if objects were rewritten in place, a cached `(object, byte_range)` could point at *different bytes* than when cached. That would require versioned reads, ETag validation on every GET, and true cache invalidation. **Immutability buys the entire consistency story for free.** Every system surveyed relies on it: Diskless WAL Segments are explicitly "immutable"; Iceberg manifests are never rewritten; turbopuffer WAL files are append-only.

Practical corollary: **cache entries never need eviction for correctness**, only for memory. You can evict by LRU with zero risk.

### 4.3 Why stale-but-monotonic is exactly right for Kafka Fetch semantics

**[Synthesis, grounded in documented Kafka behavior]**

Consider a consumer at offset `N` and an agent whose index cache is stale by `Δ` versions:

1. Agent looks up `(P, N)` in its cache.
2. **Case A — entry present** (offset `N` was committed before `applied_upto`): agent returns the data. Correct, because immutability guarantees the entry is right.
3. **Case B — entry absent, `N == cached_log_end_offset`**: the agent returns an **empty FetchResponse**. The consumer waits and retries.

Case B is *literally identical to normal Kafka behavior at the tail of the log.* A real Kafka broker in this exact situation does the same thing: `fetch.max.wait.ms` (default 500 ms) is the maximum time the broker holds the fetch waiting for `fetch.min.bytes` to accumulate; if it doesn't, it returns whatever it has, possibly nothing ([Conduktor](https://www.conduktor.io/kafka/kafka-consumer-important-settings-poll-and-internal-threads-behavior)). A consumer that polls faster than the producer produces *always* gets empty fetches. Every Kafka client in existence handles this correctly.

**So the client-visible effect of a stale metadata cache is: added end-to-end latency of at most Δ, and nothing else.** No data loss, no reordering, no duplicates, no gaps. The consumer's own offset advances monotonically; the log it sees is a prefix of the true log; prefixes of a totally-ordered log are exactly what Kafka already guarantees.

**Two conditions must hold for this argument:**

- **(C1) Monotonic prefix.** The cache must never expose offset `N+1` while missing `N`. If the coordinator assigns offsets at commit and deltas are applied **in commit-version order**, this holds by construction. ⚠️ It is violated if you apply deltas out of order or partition the delta stream by topic-partition without per-partition ordering. Apply in log order; it's cheap.
- **(C2) The high watermark reported must be `≤` the true HWM, never `>`.** Derive HWM from the cache itself (`max last_offset` seen), never from an independently-updated field. Reporting a HWM higher than what the cache can serve makes the agent claim data it cannot produce.

### 4.4 Push vs pull, and mapping onto `fetch.max.wait.ms`

**[Documented] the two camps:**
- **Push:** AutoMQ (KRaft records to every broker → "each broker caches the latest metadata"), Kafka Diskless (log replication to read-only coordinators on ISR brokers, plus write-through into the Infinispan cache: "a broker writes simultaneously to the cache and to S3").
- **Pull:** WarpStream (agent queries the metadata store per fetch; the *data* cache is what's distributed, not the metadata).

**[Synthesis] Recommendation: long-poll subscription for the tail, point query for history.**

The elegant part is that Kafka's own long-poll gives the shape for free. The agent's `Fetch` handler already blocks up to `fetch.max.wait.ms`; use that same wait to block on a metadata condition variable:

```rust
// Agent Fetch handler, sketch
async fn handle_fetch(&self, req: FetchRequest) -> FetchResponse {
    let deadline = Instant::now() + req.max_wait;
    loop {
        if let Some(refs) = self.index.lookup(req.partition, req.offset) {
            return self.read_and_assemble(refs).await;   // ranged GETs, cache-hit likely
        }
        // Nothing at this offset yet. Wait for either new metadata or the deadline —
        // NOT for a poll interval. Zero added latency in the common case.
        if self.index.wait_for_advance(deadline).await.is_timeout() {
            return FetchResponse::empty(self.index.high_watermark(req.partition));
        }
    }
}
```

`wait_for_advance` is satisfied by a **push** from the coordinator (a streaming gRPC/long-poll subscription delivering `(commit_version, [IndexDelta])`). Because the delta stream is small — Diskless measures **~6 KiB of metadata per 16 MiB of data**, ~378 KiB/s of coordination traffic for a substantial workload — pushing the full tail delta to every agent is cheap.

Design points:
- **Tail (hot) → push.** Every agent subscribes to the delta stream. `Δ` ≈ one network RTT, ~1 ms in-AZ. Fetch latency is unaffected.
- **History (cold) → pull.** A consumer replaying from offset 0 asks the coordinator for a range; the coordinator returns a bounded page of `ObjectRef`s. Don't cache the entire history in every agent (see the sizing analysis in §6.3).
- **Fan-in the subscription.** One subscription per agent, not per partition. Deltas are naturally batched by commit.
- **Bootstrap via snapshot + delta.** New agent: fetch a compacted index snapshot (from object storage, Delta-Lake-checkpoint style) at version `V`, then subscribe from `V`. This avoids a thundering herd of full-index queries on rolling restarts.

### 4.5 The tail-read path specifically **[Synthesis]**

The preceding subsections treat reads generically. But **the overwhelming majority of real Kafka consumption is tail consumption** — live consumers a few hundred milliseconds behind the producer. Historical replay (backfills, new consumer groups, DR) is comparatively rare. So the tail path deserves its own treatment: it is the hot path, and it is where a naive metadata-cache design would hurt most.

**First, the non-problem: a cache miss never falls back to LIST.** There are exactly two resolutions for "my cache doesn't have offset N":

1. **N doesn't exist yet** (genuine tail) → return an empty fetch, precisely as vanilla Kafka does.
2. **N exists, this agent hasn't seen it** → query the coordinator — an in-memory/materialized index lookup over RPC, not an object-storage operation.

Neither path touches LIST. This is why §8.1 recommends keeping `list()` out of the object-store trait entirely: it makes "fall back to LIST under cache pressure" un-expressible rather than merely discouraged. The real concern in this area is not S3 cost, it is **coordinator RPC load** if every tail fetch that misses cache triggers a round trip — addressed below.

#### Why the tail is the *easy* case

The load-bearing property: **an offset does not exist until the coordinator assigns it.** The write path is PUT → commit to coordinator → *then* ack the producer, and ordering is decided at commit time, not flush time (§3.1, WarpStream: "determines the order of writes upon *committing* … not when the data is flushed").

Consequently the coordinator is never stale relative to reality — it *is* the linearization point. Only reader caches lag, and only by propagation delay. There is no window in which a consumer can legitimately request an offset that exists but the coordinator doesn't know about. Contrast this with a design where offsets were assigned at flush time by independent writers: then the coordinator itself could lag reality, and tail reads would need a genuine consensus read. Committing offsets at the coordinator is what buys the cheap tail.

#### The number that reframes the concern

| Latency source | Magnitude | Source |
|---|---|---|
| Write batching window (unavoidable by design) | **~250 ms** | WarpStream/KIP-1163 defaults, §3.1/§3.3 |
| Object-storage PUT | ~50–200 ms | [04](04-object-storage-s3-gcs.md) §2 |
| Metadata push, coordinator → agent (in-AZ) | **~1 ms** | one network RTT |
| Coordinate lookup, if a round trip is needed | ~10 ms p99 | Aiven, [KIP-1150 guide](https://aiven.io/blog/guide-diskless-apache-kafka-kip-1150) |

**Metadata staleness sits two orders of magnitude below the batching latency already inherent in the architecture.** Optimizing propagation below the batch window buys nothing a consumer can perceive. This should be the first sanity check applied to any proposal to make metadata propagation stronger/more synchronous — it is almost certainly optimizing the wrong term.

#### Why synchronous fan-out is the wrong answer

The tempting fix — "make the commit synchronously update every agent's cache so tail reads never miss" — fails on all three axes:

- **Latency** becomes the slowest agent in the fleet. A single GC pause or a slow NIC anywhere stalls every producer, cluster-wide.
- **Availability** becomes the *product* of all agents' availability rather than the coordinator quorum's. One unhealthy agent blocks all writes.
- Structurally it is **two-phase commit across the entire fleet**, executed every 250 ms. The failure modes (participant timeout, partial commit, recovery) are exactly the ones the diskless design was meant to avoid by centralizing ordering in one small quorum.

Note that KIP-1164 deliberately goes the *other* way: `DisklessFindBatches` is explicitly marked as servable "from stale state and by followers" — the design leans into staleness rather than engineering it away.

#### The mechanism: async push + blocking read ≈ synchronous, without the coupling

Kafka's own long-poll supplies the shape for free. `fetch.max.wait.ms` (default 500 ms) means the fetch handler is *already* parked. Block on a metadata condition variable inside that existing wait (the `handle_fetch` sketch in §4.4):

```
Consumer sends Fetch(P, offset = N)
  → agent cache lacks N
  → agent blocks  ............................. (vanilla Kafka behavior, no new semantics)
  → coordinator delta arrives ~1 ms later, containing N
  → agent wakes, resolves ObjectRef, ranged GET, returns data
```

Added latency ≈ 1 ms — not a poll interval, not `fetch.max.wait.ms`. **From the consumer's perspective this is indistinguishable from a synchronous cache update**, while retaining fully asynchronous, fire-and-forget fan-out on the write path.

An agent does **not** need to distinguish "N doesn't exist yet" from "my cache is stale" — both resolve identically by waiting, and both terminate correctly (data arrives, or the deadline expires and an empty fetch is returned with a cache-derived HWM, which is `≤` truth per condition C2 in §4.3). The distinction matters only for **observability**: instrument the fraction of fetches that resolved *only after* a metadata push arrived. A rising rate is the early-warning signal that propagation is degrading, well before it becomes user-visible latency.

#### How the fan-out scales

Two properties make broadcast-to-every-agent viable rather than exotic:

1. **The delta is tiny.** KIP-1164 measures **~6 KiB of metadata per 16 MiB of data**. At 100 MB/s ingest that is ~38 KiB/s of delta; broadcast to 100 agents is ~3.8 MB/s aggregate — negligible against a 100 MB/s data plane.
2. **The metadata log can be self-hosting.** KIP-1164's `__diskless_metadata` *is itself a Kafka topic*. Metadata fan-out therefore reduces to ordinary Kafka consumption, inheriting every read-scaling mechanism the broker already implements (fetch-from-follower, per-rack/per-AZ caching, consumer-group fan-out). No bespoke fan-out tree is required. For additional headroom, KIP-1164 runs read-only coordinator replicas on every ISR broker, so coordinate lookups scale horizontally as well.

#### Soft affinity, not hard ownership

turbopuffer resolves cache coherence by **eliminating it**: each namespace is consistent-hash routed to exactly one query node, so writer and reader are the same process and there is no second cache to keep coherent ([08](08-turbopuffer-lessons.md) §1, §4). Their queries "avoid expensive S3 LIST calls through deterministic routing." Their second mechanism is the **two-cursor model** — a CAS commit point (durable) and an index cursor (indexed) — where data in between is served by scanning the un-indexed WAL tail directly (~10 ms) rather than blocking until indexing catches up, giving strong read-your-writes by default with eventual consistency as a bounded opt-in.

**What transfers:** the two-cursor split maps onto our high watermark (commit point) and compaction watermark (index cursor); serving the tail from pre-compaction state rather than waiting for reorganization is the same instinct; and routing a partition's tail to a preferred agent captures the locality win.

**What does not transfer:** turbopuffer's documented ceiling of **1 WAL entry/sec per namespace** — inherent to CAS-on-object-storage — is fatal for a broker committing every 250 ms across many concurrent writers. CAS belongs in our low-frequency control plane (topic configs, compaction-job claims, leader epochs, checkpoint pointers), never the offset stream (§3.5). Hard single-node ownership also contradicts the statelessness that makes failover trivial: any agent must be able to serve any partition.

**Synthesis — soft affinity:**

| Case | Frequency | Metadata staleness | Object-storage reads |
|---|---|---|---|
| Preferred agent serves the tail (writer == reader) | Common | Zero — data is in its own write buffer | None |
| Affinity miss / failover / cross-AZ consumer | Uncommon | ~1 ms via push | One GET, amortized across the AZ by the shared chunk cache (§5.3) |

This preserves turbopuffer's locality benefit without surrendering the interchangeability that makes a diskless fleet elastic and cheap to fail over. It is also, in effect, what WarpStream already does — consistent-hash *file* ownership for the data cache, combined with zone-aligned client routing — just stated as an explicit tail-path policy.

#### Residual cases that genuinely must hit the coordinator

Not everything can be served from a stale cache. These are low-QPS and affordable as round trips:

- **`ListOffsets(LATEST)` / `EARLIEST`** — hazard H1 in §4.6. Serving a stale log-end-offset breaks `endOffsets()`, `auto.offset.reset=latest`, and produces negative consumer lag. Always authoritative.
- **A consumer group joining fresh** and locating the tail — one-time per rebalance.
- **Read-your-writes after an explicit produce** — handled by the session watermark (`AtLeast(v_produced)`), hazard H2.

### 4.6 What actually breaks under staleness — and the fixes

**[Synthesis]** These are the failure modes to design against explicitly.

| # | Hazard | Consequence | Fix |
|---|---|---|---|
| **H1** | `ListOffsets(LATEST)` served from stale cache | Reports a log-end-offset **lower than truth**. Breaks `consumer.endOffsets()`, breaks `auto.offset.reset=latest` seek-to-end, and makes lag monitoring report **negative lag** for a consumer that is genuinely ahead of the stale view. | **Route `ListOffsets(LATEST)` and `ListOffsets(EARLIEST)` to the authoritative coordinator** (or serve with `AtLeast(coordinator_version)`). It is a rare, low-QPS request — the round trip is affordable. **Never derive it from a stale cache.** |
| **H2** | Producer produces, then immediately consumes (read-your-writes) | Consumer sees an empty log despite a successful ack. Common in tests, request-reply patterns, and Kafka Streams repartition topics. | The `Produce` response already carries the commit version (the coordinator assigned it). Have the client/agent carry a **session watermark**: any subsequent Fetch on that connection uses `AtLeast(v_produced)`. Classic "read-your-writes via session token" pattern; adds latency *only* to the read-after-write case. |
| **H3** | Transactional / `read_committed` consumers | LSO (last stable offset) must not exceed what's actually readable. In-flight transactions already gate this in vanilla Kafka. | Treat LSO like the HWM: derive from the cache, never report ahead of it. `read_committed` consumers already cannot read to the HWM when transactions are in flight, so a lagging LSO is within existing semantics. |
| **H4** | **GC race**: stale cache references an object that compaction/retention already deleted | GET returns **404**. If naively treated as "no data", could be misread as end-of-log or as data loss. | See below. |
| **H5** | Rewound/forked metadata after a coordinator failover | A cache built from a log that got truncated could hold entries the new leader never committed. | The commit log must be a **single totally-ordered replicated log** with no uncommitted-entry exposure (Raft committed-index / Kafka HWM semantics). Additionally carry a **coordinator epoch**; on epoch bump, readers must revalidate any entries above the new leader's HWM. |

**H4 in detail — the deletion-delay pattern [Documented + Synthesis]:**

WarpStream's documented answer is soft-delete-then-reap: a query is two steps ("Query the metadata store for relevant files" → "Execute the query on the relevant files"), so "files that were logically deleted by compaction continued to exist in the object store for some period of time **so that in-flight queries could continue to use them**", with physical deletion "after a sufficient delay" ([Taking out the Trash](https://www.warpstream.com/blog/taking-out-the-trash-garbage-collection-of-object-storage-at-massive-scale)). KIP-1165 similarly has the compaction agent enforce "a deletion deadline."

The safety condition, made precise:

```
deletion_delay  >  max_metadata_staleness  +  max_in_flight_fetch_duration  +  clock_skew
```

Since a metadata cache makes `max_metadata_staleness` potentially unbounded (an agent could be partitioned from the coordinator for minutes), you need **both** a delay *and* a fence:

1. **Never reuse object IDs.** UUIDs (as Diskless does) or a monotonic sequence. A 404 then unambiguously means "reaped", never "not yet written".
2. **Bound cache staleness explicitly.** If `now - last_delta_received > staleness_limit`, the agent stops serving from cache and either errors out or forces a coordinator round-trip. This converts unbounded staleness into a hard bound you can put in the inequality above. turbopuffer does exactly this with its ~1 h eventual-consistency bound.
3. **Treat 404 as "cache too stale", never as "end of log".** On 404: invalidate the entry, force-refresh the index for that partition from the coordinator, retry. If the coordinator confirms the offset moved past retention, return `OFFSET_OUT_OF_RANGE` — the correct, existing Kafka error. **Emit a metric on this path; a nonzero rate means the deletion delay is too short.**
4. **Compaction epoch fencing.** Give each `ObjectRef` a compaction epoch. When compaction rewrites objects, it bumps the partition's epoch. A cached ref with an older epoch is still *valid* (the object still exists during the delay window) but *suboptimal*; the agent can opportunistically refresh. This gives graceful degradation rather than a cliff, and — importantly — **compaction never invalidates correctness, only efficiency**, because the old objects still exist.
5. **Keep the reconciliation sweep.** Even with an optimistic deletion queue, run a slow S3-Inventory-driven mark-and-sweep for orphans (objects PUT by an agent that crashed before committing). This is the *one* legitimate use of listing, it runs daily not per-fetch, and at ~$0.0025 per million objects via S3 Inventory it is essentially free.

---

## 5. Reducing GET cost on the read path

### 5.1 Ranged GETs into bundled objects **[Documented + Synthesis]**

Both the write layout and the index format exist to make one partition's data **contiguous** within the object, so one partition-fetch = one ranged GET:

- WarpStream: data "sorted first by topic, and then by partition."
- Kafka Diskless: "Batches are **grouped by partition into contiguous parts of the object**."
- AutoMQ: IndexBlock entries "sorted by (streamId, startOffset)".

Since a ranged GET costs the same as a full GET (§1.6), the ideal is: **coordinator returns `(object, offset, len)` → exactly one GET.** AutoMQ's variant (coordinator returns object only; footer+IndexBlock GETs locate the range) trades 2 extra GETs cold for a much smaller central index — a legitimate choice if the central index is the bottleneck.

### 5.2 Coalescing multiple partitions from one object **[Synthesis]**

A Kafka `FetchRequest` covers many partitions at once. If a consumer subscribes to 50 partitions and they were all produced in the same 250 ms window, **their data is in the same object, and often within a few hundred KB of each other** (because it's sorted by topic then partition). Coalescing rules for an agent:

1. Group the resolved `ObjectRef`s by `object_id`.
2. Within an object, if two needed ranges are separated by a gap smaller than `coalesce_threshold` (a good default: 1 MiB — the discarded bytes are free in-region on S3 Standard, and the request saved is worth 2500 discarded KB at Express pricing), **merge them into one GET** and slice locally.
3. Round the merged range out to a fixed alignment so it is cacheable and shareable.

This turns a 50-partition fetch into ~1–3 GETs instead of 50. Combined with §5.3, GET count becomes a function of **bytes moved**, not of `partitions × consumers`.

### 5.3 Read-through caching: N consumers → 1 GET **[Documented]**

This is WarpStream's distributed mmap, and it is the single highest-leverage read-path optimization. From [Minimizing S3 API Costs with Distributed mmap](https://www.warpstream.com/blog/minimizing-s3-api-costs-with-distributed-mmap):

- **Fixed 4 MiB aligned paging.** The cache pages "in fixed (and aligned) size chunks of 4 MiB *regardless of how large the initiating IO was*". Alignment is what makes chunks *shareable* — an arbitrary `[o, o+len)` range is a cache key almost nobody else will ask for; a 4 MiB-aligned chunk index is asked for by everyone.
- **Consistent hashing on file ID.** Determines the owning agent; requests are *forwarded* to the owner. Not a local cache — a *distributed* one, so aggregate capacity scales with the fleet and each chunk is fetched from S3 exactly once cluster-wide.
- **Scan sharing.** Their worked example: three concurrent Kafka fetches for three different partitions all resolve into File 3; all three sub-fetches go to the owning Agent; because the data sits in one 4 MiB chunk, **Agent 2 fetches once and serves all three**.
- **Per-AZ.** The cache is per-availability-zone, which also eliminates cross-AZ transfer (Diskless's per-rack caching is the same idea — see [11](11-low-latency-tiers-and-interaz-costs.md)).

The stated result: this "completely decouples the number of partitions and consumers from the number of object storage GET requests."

**[Synthesis] Why immutability makes this cache trivially coherent:** the cache key is `(object_id, chunk_index)` and the value is *immutable bytes*. There is no invalidation protocol, no TTL, no versioning, no write-through, no coherence traffic between agents. The only eviction reason is memory pressure, and eviction is always safe. **Compare this to caching a mutable database page — the entire complexity of cache coherence simply does not arise.** This is the strongest single argument for the object-storage-native design.

**Implementation notes for Rust [Synthesis]:**
- Single-flight (`tokio::sync::Mutex` over an in-flight map, or `moka`'s `try_get_with`) so 500 simultaneous requests for a cold chunk issue **one** GET, not 500. Without this, a cache miss under fan-out is *worse* than no cache.
- Use `foyer` or `moka` for the in-memory tier (see [05-rust-ecosystem.md](05-rust-ecosystem.md) §8); add an NVMe tier (turbopuffer reports p50 14 ms warm vs 874 ms cold from S3 — a 60× win) if local disk is available. Note this doesn't violate a "zero disk" stance: the disk is a pure cache, losable at any time.
- Consistent hashing with bounded loads (or rendezvous hashing) for owner selection; use the object ID, not the partition, so hot partitions don't hotspot a single agent.
- Prefetch the next chunk on sequential access — trivially detected, and historical replay is almost entirely sequential.

### 5.4 GET-cost summary **[Synthesis]**

| Strategy | GET count scales with | Relative cost at 200 consumers |
|---|---|---|
| Naive: one GET per partition per fetch | `partitions × consumers × poll_rate` | 1× (baseline, ruinous) |
| + coalescing within a fetch | `objects_touched × consumers × poll_rate` | ~1/20× |
| + distributed aligned cache | **`bytes_read / 4 MiB`** | **~1/800×** (see §7.2) |

---

## 6. The compaction interaction

### 6.1 What systems document **[Documented]**

- **WarpStream:** compaction "compacts the small files created at ingestion time into much larger files which have significantly higher amounts of data per-partition", giving "massively reduced number of GET requests required to read data for individual partitions." Cost side: streaming compaction "requires little memory and **only results in one additional GET request per input file (no matter how large it is)**."
- **AutoMQ:** SSO compaction every **20 min**; streams >**16 MiB** become standalone Stream Objects; the rest merge-sort into new SSOs (15 TiB handled in <500 MiB memory). Motivation stated plainly: "the majority of indexing costs are spent on searching the StreamSetObject", and compaction enables "most data of the Stream to reside within the StreamObject." Stream Object compaction uses **MultiPartCopy** for server-side range copies, "avoiding network bandwidth waste from read-then-write cycles."
- **KIP-1165:** compaction agents "reorder batches by offset, regroup them by topic-partition, and rewrite them into fewer, larger objects optimized for reads", in a streaming manner with minimal buffering, enforcing a deletion deadline, and notifying the coordinator of the new structure. Motivation: "small SLSOs create read amplification (a consumer of one partition would touch many objects)."
- **WarpStream tiered variant:** land in S3 Express One Zone for latency, "then compacted out to S3 Standard buckets asynchronously" — "a form of tiered storage within S3 itself" ([AWS Storage Blog](https://aws.amazon.com/blogs/storage/how-warpstream-enables-cost-effective-low-latency-streaming-with-amazon-s3-express-one-zone/)). Note Express's single-AZ durability forces quorum writes across multiple directory buckets (3 copies) for AZ-failure tolerance — see [11](11-low-latency-tiers-and-interaz-costs.md) §6.

### 6.2 A quantified break-even model **[Synthesis]**

Assumptions, stated so they can be re-run: S3 Standard us-east-1; L0 objects 2.5 MB each (250 ms flush at 10 MB/s per agent); 100 partitions per object; compaction merges 100 L0 objects → 1 × 250 MB L1 object; multipart upload with 16 MB parts.

**Cost of compacting 250 MB:**
```
GETs:  100 input objects × 1 GET each      = 100 × $4.0e-7  = $0.000040
PUTs:  CreateMPU + 16 UploadPart + Complete = 18 × $5.0e-6  = $0.000090
                                                       total = $0.000130
                                                             = $0.00052 / GB compacted
```

**Savings on one full re-read of that 250 MB, per-partition:**
```
Before: each of 100 partitions needs 100 ranged GETs (one per L0 object holding its 25 KB)
        = 100 partitions × 100 GETs = 10,000 GETs = $0.004000
After:  each partition's 2.5 MB is contiguous in the L1 object
        = 100 partitions × 1 GET   =    100 GETs = $0.000040
                                             savings = $0.003960
```

**Break-even: `$0.000130 / $0.003960 = 0.033` full re-reads.** Compaction pays for itself if the data is read even **3.3% of one additional time** beyond ingest. Since retention-period data is typically read at least once (and often 2–5× across consumer groups), **compaction is essentially always profitable on GET cost alone** — before counting its two larger benefits:

- **Read latency:** 100 serial-ish GETs at ~15 ms median → 1 GET. This is often the *real* motivation.
- **Metadata volume:** the index shrinks 100×. Given §6.3, this is frequently the binding constraint.

**When compaction does *not* pay [Synthesis]:**
- Data never re-read (pure tail-consumption, all reads served from the write-path cache before the data is even cold). Here compaction is pure cost — though you probably still want it for index size.
- Very short retention (< 1 compaction interval).
- Data already contiguous enough (very few, very high-throughput partitions — each 250 ms object already holds ~all of one partition).

**Triggering heuristic [Synthesis]:** trigger on estimated *read amplification*, not object count. For partition P over offset range R, `read_amp(P,R) = number_of_distinct_objects(P,R)`. Compact when `read_amp > threshold` (start at 8–16) **and** the range is older than `latency_slo` (don't compact data still being tail-read from cache — you'd pay to rewrite data nobody will re-read). AutoMQ's "16 MiB per stream → graduate to a Stream Object" is the same idea expressed as a size threshold. Their 20-minute cadence is a reasonable starting default.

### 6.3 Index-size sizing — why compaction is about metadata too **[Synthesis]**

This is the constraint people underestimate.

```
Index entry ≈ 40 bytes (partition, base/last offset, object_id, byte_offset, byte_len, ts range)

Uncompacted (L0), 40 objects/s × 100 partitions/object:
    4,000 entries/s → 345 M entries/day → ~13.8 GB/day of index

Over 7-day retention: ~97 GB of index. Will not fit in agent memory.
```

Three consequences:
1. **Do not replicate the full index to every agent** (AutoMQ's KRaft-to-every-broker model works because their metadata is coarser — object-level, with in-object indexes doing the fine work). Push only the tail delta; pull history.
2. **Compaction is what makes the historical index tractable.** 100:1 compaction turns 97 GB into ~1 GB — cacheable.
3. **This is also the argument for AutoMQ's two-level scheme.** If the coordinator index stores only `partition → [object_ids]` (no byte ranges) and the object footer resolves the byte range, entries drop from 40 B to ~12 B *and* you get 100:1 fewer of them post-compaction. The price is 1–2 extra GETs on a cold read, which the chunk cache absorbs after the first reader.

**[Synthesis] Recommendation for oqueue:** store byte ranges in the coordinator for the **recent/tail** window (latency matters, index is small), and degrade to object-only refs + in-object footer index for **historical** ranges (index size matters, latency doesn't). Iceberg's manifest-list/manifest split is the same two-tier idea.

---

## 7. Concrete cost model: LIST-based vs coordinator-based discovery

**[Synthesis]** All unit prices are documented (§1); the workload shape is illustrative. Recompute for actual parameters.

### 7.1 Workload

| Parameter | Value |
|---|---|
| Ingest | 100 MB/s |
| Topic-partitions | 10,000 |
| Writer agents | 10 |
| Flush policy | 250 ms or 8 MiB → 2.5 MB objects, 4 objects/s/agent |
| Consumer instances | 200, each subscribed to 50 partitions |
| Effective poll rate | 2 fetch rounds/s per consumer (`fetch.max.wait.ms=500`) |
| Consumer groups | 3 (each reading the full stream) |
| Storage | S3 Standard, us-east-1 |

### 7.2 Results

**Write path (identical under both designs):**
```
40 PUT/s × 86,400 s = 3.456 M PUT/day × $5.0e-6 = $17.28/day  = $518/month
```
Sanity check against WarpStream's published "<$40/day in object storage API costs" for ~560 MiB/s — same order of magnitude, confirming this is the correct regime.

**(a) LIST-based discovery** — best realistic case: one LIST per partition per second, perfectly shared across all consumers (already a fiction, since bundled keys make LIST unable to answer the question at all — §2.1):
```
10,000 LIST/s × 86,400 = 864 M LIST/day × $5.0e-6 = $4,320/day = $129,600/month
```
Plus the follow-on GETs to open candidate object footers, not even modeled because the number is absurd.

**(b) Coordinator-based discovery:**
```
S3 API calls for discovery:                                          $0/day
Coordinator RPC load: 200 × 2 = 400 findBatches/s (batched per fetch,
  not per partition) — or ~0/s with tail-delta push
Coordinator egress (in-AZ): ~6 KiB metadata per 16 MiB written
  ≈ 400 KiB/s of delta stream                                 negligible
```

**Read path GETs:**
```
Naive (1 GET per partition per fetch round, 3 groups):
   200 × 50 × 2 × 3 = 60,000 GET/s = 5.184 B/day × $4.0e-7 = $2,074/day

With distributed 4 MiB aligned cache (GETs ∝ bytes, shared across all
consumers and all groups):
   100 MB/s ÷ 4 MiB = ~24 GET/s = 2.07 M/day × $4.0e-7      =    $0.83/day
```

### 7.3 Bottom line

| Component | LIST-based + naive reads | **Coordinator index + distributed cache** |
|---|---|---|
| Write PUTs | $518 /mo | $518 /mo |
| Discovery | **$129,600 /mo** | **$0 /mo** |
| Read GETs | $62,200 /mo | **$25 /mo** |
| **Total S3 API** | **~$192,300 /mo** | **~$543 /mo** |
| **Ratio** | — | **~354× cheaper** |

At 100 MB/s ≈ 253 TB/month ingested, that is **$0.76/TB vs $0.0021/TB** in API costs. In the coordinator design, S3 API cost becomes a rounding error next to storage ($0.023/GB-mo → ~$5,800/mo for a 7-day retention working set) — which is the correct end state: **you should be paying for bytes, not for requests.**

**Published figures that corroborate the direction [Documented]:**
- WarpStream: Kafka inter-zone bandwidth **$641/day** at ~560 MiB/s vs **<$40/day** in WarpStream S3 API costs.
- WarpStream: naive per-partition file-per-250 ms ≈ **$50/month per partition** in PUTs alone.
- AutoMQ: 10,000 writes/s ≈ **$130,000/month** in PUT charges alone, without batching.
- Kafka Diskless (via [2minutestreaming](https://blog.2minutestreaming.com/p/diskless-kafka-topics-kip-1150)): ~**$882k → ~$200k** annually (5×) for a reference deployment, primarily by eliminating cross-zone networking; and ~11.2× cheaper than Confluent Freight ($100k/yr vs $2M/yr).

---

## 8. Design recommendations for oqueue **[Synthesis]**

1. **No `list()` in the object-store trait.** Follow KIP-1163: `upload`, `fetch(key, ByteRange)`, `delete(keys)`. Put listing behind a separate `MaintenanceStore` trait used only by the GC reconciler. This makes "never LIST on the read path" a compile-time property, not a code-review convention.

2. **Coordinator index keyed by `(topic, partition, base_offset)` → `ObjectRef`**, with the object-level `ByteRange` stored inline for recent data and elided (footer-resolved) for compacted history.

3. **Source of truth = a replicated log.** Either an internal Raft or a Kafka-topic-shaped log (KIP-1164's `__diskless_metadata`). Materialize locally — `redb`/`sled`/RocksDB is the Rust analogue of their SQLite choice. **The materialized store is a cache, not the truth**, exactly as KIP-1164 states.

4. **One monotonic `CommitVersion`**, threaded through every response. Three read modes: `Stale` (Fetch), `AtLeast(v)` (read-your-writes), `Linearizable` (ListOffsets, transactional). Never serve `ListOffsets(LATEST)` from a stale cache — that's hazard H1 and it's the one that produces visibly wrong behavior.

5. **Push tail deltas via long-poll subscription; pull historical ranges.** Wire the subscription's wakeup into the `Fetch` handler's existing `fetch.max.wait.ms` wait so metadata freshness costs zero added latency in the steady state.

6. **Distributed, consistent-hashed, 4 MiB-aligned chunk cache with single-flight.** This is the difference between $62k/mo and $25/mo in GETs. Alignment is what makes chunks shareable; single-flight is what prevents a cold-start stampede.

7. **Soft-delete then reap** with `deletion_delay > bounded_staleness + max_fetch_duration + skew`. Non-reusable object IDs. 404 ⇒ "refresh index and retry", never "end of log", and always instrumented. Keep an S3-Inventory-driven reconciliation sweep as the orphan safety net.

8. **Compact on read-amplification, not object count.** Threshold ~8–16 distinct objects per partition-range; skip ranges still hot in cache. Budget ~$0.0005/GB; it breaks even at 3% of one re-read.

9. **Size the index before designing the cache.** ~40 B/entry × 4,000 entries/s is ~14 GB/day uncompacted. Decide early whether the coordinator holds byte ranges (fatter index, 1 GET) or object refs only (leaner index, footer-resolved, 1–3 GETs cold). The AutoMQ hybrid — fat for tail, lean for history — is likely right.

10. **Consider CAS-on-S3 for the low-frequency control plane** (topic configs, compaction manifests, epoch/leadership, checkpoint pointers) à la turbopuffer, so the OSS story is "no external database required." But do **not** put the 4-commits-per-second offset stream through CAS — turbopuffer's documented 1 WAL entry/s per namespace is the ceiling of that technique.

---

## 9. Open questions this research did not settle

- **KIP-1164's exact `DisklessFindBatches` wire schema.** The Confluence page was intermittently unavailable (404 / socket hang-up across multiple attempts, from both WebFetch and curl); only prose descriptions were recoverable via search extraction. Re-fetch [KIP-1164](https://cwiki.apache.org/confluence/display/KAFKA/KIP-1164:+Topic+Based+Batch+Coordinator) before finalizing our own RPC design — it is the closest published spec to what we're building.
- **WarpStream's metadata-store query API surface** is described functionally in their docs but never published as a schema. Their control plane is proprietary; there is no more detail to be had publicly.
- **S3 Express One Zone LIST pricing** post-April-2025 is inferred from the pricing page's PUT/COPY/POST/LIST grouping; the reduction announcement doesn't restate it. Verify against the live page.
- **GCS Archive-class operation pricing** was inconsistently reported across secondary sources; irrelevant to this workload, but don't copy a number from a blog.
- **LIST latency percentiles** — no authoritative published p50/p99 for `ListObjectsV2` was found. The structural argument (serial `ContinuationToken` pagination, 1,000 keys/page, shared key index) is solid; the specific milliseconds are not.

---

## Sources

**Pricing**
- [Amazon S3 Pricing](https://aws.amazon.com/s3/pricing/)
- [Up to 85% price reductions for Amazon S3 Express One Zone — AWS News Blog](https://aws.amazon.com/blogs/aws/up-to-85-price-reductions-for-amazon-s3-express-one-zone/)
- [Unpacking Amazon S3 Express One Zone — Vantage](https://www.vantage.sh/blog/amazon-s3-express-one-zone)
- [Google Cloud Storage pricing](https://cloud.google.com/storage/pricing)
- [GCP Storage Classes & Pricing — Economize](https://www.economize.cloud/blog/gcp-storage-classes-pricing-features-services/)
- [Google Cloud Storage Pricing Guide — CloudZero](https://www.cloudzero.com/blog/gcp-storage-pricing/)
- [Cloudflare R2 Pricing](https://developers.cloudflare.com/r2/pricing/)
- [S3-IA retrieval fees for range requests — AWS re:Post](https://repost.aws/questions/QU7Cv_MwSlQP6sQkViBvISAg/s3-infrequent-access-retrieval-fee-for-range-requests)
- [Can S3 ListBucket requests result in Data Transfer Out charges? — AWS re:Post](https://repost.aws/questions/QUxVRLdhqbTv-7iFiMLsZ_vA/can-s3-listbucket-requests-result-in-data-transfer-out-charges)
- [S3 Cost Optimization 2026](https://go-cloud.io/s3-cost-optimization/)
- [S3 Inventory vs LIST cost — Hacker News](https://news.ycombinator.com/item?id=28917705)

**S3 API semantics**
- [ListObjectsV2 — Amazon S3 API Reference](https://docs.aws.amazon.com/AmazonS3/latest/API/API_ListObjectsV2.html)
- [Amazon S3 Strong Consistency](https://aws.amazon.com/s3/consistency/)
- [High ListObjectsV2 cost — mountpoint-s3 issue #1354](https://github.com/awslabs/mountpoint-s3/issues/1354)

**WarpStream**
- [Minimizing S3 API Costs with Distributed mmap](https://www.warpstream.com/blog/minimizing-s3-api-costs-with-distributed-mmap)
- [Read Path — WarpStream docs](https://docs.warpstream.com/warpstream/overview/architecture/read-path)
- [Write Path — WarpStream docs](https://docs.warpstream.com/warpstream/overview/architecture/write-path)
- [Architecture — WarpStream docs](https://docs.warpstream.com/warpstream/overview/architecture)
- [Taking out the Trash: Garbage Collection of Object Storage at Massive Scale](https://www.warpstream.com/blog/taking-out-the-trash-garbage-collection-of-object-storage-at-massive-scale)
- [S3 Express is All You Need](https://www.warpstream.com/blog/s3-express-is-all-you-need)
- [The Case for Shared Storage](https://www.warpstream.com/blog/the-case-for-shared-storage)
- [How WarpStream enables cost-effective low-latency streaming with S3 Express One Zone — AWS Storage Blog](https://aws.amazon.com/blogs/storage/how-warpstream-enables-cost-effective-low-latency-streaming-with-amazon-s3-express-one-zone/)

**AutoMQ**
- [Deep dive into the challenges of building Kafka on top of S3](https://www.automq.com/blog/deep-dive-into-the-challenges-of-building-kafka-on-top-of-s3)
- [Insight: Metadata Management in AutoMQ](https://www.automq.com/blog/insight-metadata-management-in-automq)
- [Parsing the file storage format in AutoMQ object storage](https://www.automq.com/blog/parsing-the-file-storage-format-in-automq-object-storage)
- [S3 Storage — AutoMQ Documentation](https://docs.automq.com/automq/architecture/s3stream-shared-streaming-storage/s3-storage)

**Kafka Diskless (KIP-1150 family)**
- [KIP-1150: Diskless Topics](https://cwiki.apache.org/confluence/display/KAFKA/KIP-1150:+Diskless+Topics)
- [KIP-1163: Diskless Core](https://cwiki.apache.org/confluence/display/KAFKA/KIP-1163:+Diskless+Core)
- [KIP-1164: Topic Based Batch Coordinator](https://cwiki.apache.org/confluence/display/KAFKA/KIP-1164:+Topic+Based+Batch+Coordinator)
- [KIP-1165: Object Compaction for Diskless](https://cwiki.apache.org/confluence/display/KAFKA/KIP-1165:+Object+Compaction+for+Diskless)
- [The Hitchhiker's Guide to Diskless Apache Kafka — Aiven](https://aiven.io/blog/guide-diskless-apache-kafka-kip-1150)
- [How KIP-1150 Diskless Topics makes Kafka stateless — 2 Minute Streaming](https://blog.2minutestreaming.com/p/diskless-kafka-topics-kip-1150)
- [KIP-1150 in Apache Kafka is a big deal — TopicPartition](https://topicpartition.io/blog/kip-1150-diskless-topics-in-apache-kafka)
- [[DISCUSS] KIP-1164 — kafka dev mailing list](https://www.mail-archive.com/dev@kafka.apache.org/msg149311.html)

**Table formats**
- [Apache Iceberg Table Spec](https://iceberg.apache.org/spec/)
- [Apache Iceberg Reliability](https://iceberg.apache.org/docs/latest/reliability/)
- [Apache Iceberg Maintenance](https://iceberg.apache.org/docs/latest/maintenance/)
- [Diving Into Delta Lake: Unpacking the Transaction Log — Databricks](https://www.databricks.com/blog/2019/08/21/diving-into-delta-lake-unpacking-the-transaction-log.html)
- [Computing Delta Lake State Quickly with Checkpoint Files — Denny Lee](https://dennyglee.com/2024/01/09/computing-delta-lake-state-quickly-with-checkpoint-files/)
- [Delta Lake PROTOCOL.md](https://github.com/delta-io/delta/blob/master/PROTOCOL.md)

**turbopuffer**
- [turbopuffer Architecture](https://turbopuffer.com/docs/architecture)
- [TurboPuffer: Object Storage-First Vector Database Architecture](https://jxnl.co/writing/2025/09/11/turbopuffer-object-storage-first-vector-database-architecture/)

**Kafka client semantics**
- [Kafka Consumer Poll & Timeout Settings — Conduktor](https://www.conduktor.io/kafka/kafka-consumer-important-settings-poll-and-internal-threads-behavior)
- [Kafka Consumer Configuration Reference — Confluent](https://docs.confluent.io/platform/current/installation/configuration/consumer-configs.html)
