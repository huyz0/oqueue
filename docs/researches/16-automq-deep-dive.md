---
title: "AutoMQ Deep Dive: Source-Grounded Study of the Closest Comparable"
slug: automq-deep-dive
status: draft
last_updated: 2026-08-13
tags: [automq, s3stream, source-study, wal, recovery, fencing, compaction, cache, kraft, metadata-scale, corrections]
related: [03-competitive-landscape, 12-object-discovery-and-api-cost, 13-coordinator-recovery, 14-metadata-scale-and-tiering, 15-scale-architecture-position]
summary: >
  Study of AutoMQ's actual source (commit eccde72, Aug 2026) rather than its
  blog posts. Answers definitively why their partition ceiling is ~10^5 — it's
  KRaft as the metadata plane, evidenced by their own OOM issue. Contains
  several CORRECTIONS to claims this corpus previously cited from their
  marketing material, including that the EBS WAL no longer exists in the
  shipping code. Also extracts the transferable ideas: offset-aligned object
  naming, contiguity-of-keys as the durability boundary, composite objects,
  and miss-driven readahead.
---

# AutoMQ Deep Dive: Source-Grounded Study

*Compiled 2026-08-13 from four parallel agents reading the source at commit `eccde72`, version 3.9.0-SNAPSHOT (AutoMQ 1.7.3-era). Clone was ~109 MB; the genuinely novel code (`s3stream/`) is ~20k lines.*

Marked **[Code]** (verified in the clone, with paths), **[Web]** (external source, linked), or **[Inference]** (agent or corpus reasoning, explicitly not verified).

> **Why this document exists.** AutoMQ is the **only** close comparable whose implementation we can read. WarpStream is closed, turbopuffer is closed, Bufstream was acquired and withdrawn, Redpanda's relevant parts are BSL. It is also Apache-2.0, so if we land there ([07](07-licensing-strategy.md)) its ideas are freely usable, not merely observable. Everything the corpus previously said about AutoMQ came from their blog and docs — which, as §2 shows, is materially out of date in places.

---

## 1. The verdict on the scale ceiling

**Their own issue tracker answers it.** [Issue #1608](https://github.com/AutoMQ/automq/issues/1608), July 2024, verbatim **[Web]**:

> "In a cluster with a scale of **100,000 partitions**, each partition has at least MetaStream, DataStream, and TimeIndexStream, resulting in a total of **300,000 Streams**. As partitions migrate, the RangeMetadata maintained by metadata will increase, **ultimately leading to an OOM**."

And [Issue #3477](https://github.com/AutoMQ/automq/issues/3477) (open, July 2026) shows real production instability far lower — a user on 3 controllers / 4 brokers / **~5,000 topics** hitting `controller event queue overloaded`, cascading into broker fencing and "individual brokers restart dozens of times in a single incident."

**The cause is KRaft as the metadata plane**, which forces four things simultaneously:

| # | Constraint | Evidence |
|---|---|---|
| A | **Every broker holds the whole-cluster stream image.** `S3StreamsMetadataImage` (streamMetadataMap, stream2partition, partition2streams, streamEndOffsets) is part of `MetadataImage`, materialized on *every* broker. A broker serving 10 partitions holds metadata for all 1M. | `image/S3StreamsMetadataImage.java:87-99` **[Code]** |
| B | **Single-writer, single-threaded controller.** One `QuorumController` event loop serializes every stream open/close, object commit, compaction commit and lifecycle tick. | `OverloadCircuitBreaker.java`; issues #1446, #3477 **[Code+Web]** |
| C | **Atomic commits bounded at 8 MB / 50,000 records.** Forces the 20,000-stream and 10,000-object commit caps. | `KafkaRaftClient.java:162`, `QuorumController.java:247` **[Code]** |
| D | **O(cluster) snapshots.** A snapshot re-serializes 1 record per stream + 1 per range + 1 per stream object + 1 per live S3 object. At 1M partitions / 4M streams that's ≥8M records before any object records — replayed on every restart. | `S3StreamMetadataImage.write():79-93` **[Code]** |

**Every AutoMQ scaling redesign has moved data *out from under* that constraint while keeping the constraint.** Three generations of the read-path index (in-record → S3-offloaded sparse index → bloom-filter/lazy-load), and [#2731](https://github.com/AutoMQ/automq/issues/2731) is *still open* in 2026.

**The single most transferable rule from this research:**

> Metadata cost must be proportional to **active partitions on this node**, not **total partitions in the cluster.**

AutoMQ violates that in at least six places. That ratio — not object storage, not compaction, not the leaderful model — is why their number is ~10⁵ and ours needs to be 10⁹–10¹¹.

⚠️ **One thing not to conclude.** `S3_MAX_STREAM_NUM_PER_STREAM_SET_OBJECT = 20000` numerically matches the Pro tier's "20,000 partitions." The agent investigated and judged this **coincidental** — that constant is per-object and per-cache-block, and at 3–4 streams per partition the units differ by 3–4×. Do not build on the match.

### Cost arithmetic **[Inference, from field layouts — estimates, not measurements]**

Per stream, resident on every node: ~450–550 bytes minimum (including **two `new Object()` locks per stream** = 32 B of pure lock overhead). At 4 streams/partition ≈ **~2 KB per partition, on every broker and controller**:

| Partitions | Streams | Floor heap per node |
|---|---|---|
| 20,000 (Pro tier) | 80,000 | ~40 MB |
| 100,000 | 400,000 | ~200 MB |
| 1,000,000 | 4,000,000 | **~2 GB** |
| 100,000,000 | 400,000,000 | **~200 GB** |

And a second, sharper per-broker cap: `LogCacheBlock.isFull()` returns true at **20,000 distinct streams** regardless of bytes (`S3Storage.java:224`, `LogCache.java:472`) **[Code]**. Past ~5,000 partitions per broker, blocks seal on *stream count* rather than size — producing more, smaller objects, more lifecycle churn, and precisely the controller saturation of #3477.

---

## 2. ⚠️ Corrections to this corpus

Several claims we cited from AutoMQ's marketing material do not match the shipping code. **These need fixing in docs 03, 06, 11, 12 and 13.**

| We cited | Actual code | Where |
|---|---|---|
| **EBS-backed WAL, sub-10ms produce latency; EBS Multi-Attach reattach "within milliseconds"** | **The EBS/block-device WAL no longer exists.** Repo-wide grep for `BlockWALService`, `SlidingWindowService`, `WALBlockDeviceChannel`, `MultiAttach`, `attachVolume` → **zero hits**. `DefaultWalFactory` throws on any protocol but `S3`. | 03, 06, 11, 13 |
| Failover reattaches a volume in milliseconds | Failover = a healthy broker **reads the dead broker's WAL objects from shared object storage and re-uploads them**. Hard constraints: **1-minute minimum grace period** (`DONT_FAILOVER_AFTER_NEW_EPOCH_MS`), 1-second controller poll, and **one failover at a time cluster-wide** (`maxInflight = 1`). Seconds to tens of seconds. | 13 |
| Compaction "every 20 minutes" | **5 minutes** (`streamSetObjectCompactionInterval`). The 120-min constant is the *force-split* period — a different mechanism. | 12, 14 |
| "Streams >16 MiB graduate to their own object" | **8 MiB.** Two knobs meaning the same thing: `streamSplitSize` (s3stream default 16 MiB, broker overrides to 8 MiB) and `streamSetObjectCompactionStreamSplitSize` (8 MiB). | 12, 14 |
| "Two compaction mechanisms" | **Five**, across two generations, plus force-split. | 12 |
| `DataBlockIndex` = streamId, startOffset, endOffset, position, blockSize | 36 bytes, but: `streamId(8) + startOffset(8) + **endOffsetDelta(4, u32 delta)** + **recordCount(4, we missed it)** + startPosition(8) + size(4)` | 12 §3.2 |
| AutoMQ "avoids LIST entirely" | True for the **data path** (object keys derived from controller-issued IDs; `ObjectManager` has no list operation). **False for the WAL path** — `objectStorage.list(nodePrefix)` at startup and on epoch change. | 12, 13 |

The EBS-WAL correction matters most: it removes AutoMQ as evidence for the "attached-disk WAL gives you single-digit-ms acks" design option that docs 06 and 11 present as a live alternative. Per their 1.5.0 release notes **[Web]**, *"1.5.0 and later versions offer full S3 support, while EBS and Regional EBS support have been moved to the Enterprise Edition"* — so it may survive commercially, but it is not in the open-source tree and cannot be studied.

---

## 3. The Stream abstraction

**[Code]** `s3stream/.../api/Stream.java` is ~120 lines and **13 methods** — the entire contract:

```java
long streamId(); long streamEpoch(); long startOffset();
long confirmOffset(); void confirmOffset(long); long nextOffset();
CompletableFuture<AppendResult> append(AppendContext, RecordBatch);
CompletableFuture<FetchResult>  fetch(FetchContext, long start, long end, int maxBytesHint);
CompletableFuture<Void> trim(long newStartOffset);
default void beforeClose();
CompletableFuture<Void> close(); CompletableFuture<Void> destroy();
CompletableFuture<AppendResult> lastAppendFuture();
```

No seek, no iterator, no compaction, no ack level, no replication factor at the data plane. A stream is an append-only sequence of record batches in a dense 64-bit offset space, owned by one writer, fenced by a monotonic epoch. Retention is expressed **only** as `trim(newStartOffset)`.

**Because streams are offset-addressable by construction, Kafka's offset index disappears entirely** — there is no `.index` equivalent. Segments are *slices*: `DefaultElasticStreamSlice` holds `startOffsetInStream` and rebases, with exactly one writable slice at the tail. **[Inference]** That is a genuine win from the abstraction, not from being a fork, and it's the cleanest single idea in their design.

**Partition → 4 streams** (`meta`, `log`, `tim`, `txn`), mapped in the Kafka fork, not in s3stream. The KV store holds exactly one entry per partition: `namespace/topicId/partition → 8-byte metaStream id`. Everything else is discovered by replaying the meta stream. **[Code]** `ElasticLog.scala:873,968`; `ElasticLogSegment.java:97-114`.

**[Inference]** The 4-stream layout is fork baggage — `tim` and `txn` exist only because Kafka has `.timeindex` and `.txnindex` files on disk. Folding them into the data object's footer (they already have `DataBlockIndex`/`ObjectReader` machinery for exactly this) saves 3–4× on every per-stream cost immediately.

---

## 4. The object format

**[Code]** `ObjectWriter.java` / `ObjectReader.java`.

```
[DataBlock] [DataBlock] ... [IndexBlock] [Footer(48B)]
```

- **DataBlock header, 10 B**: magic `0x5A`, flag `0x02`, recordCount u32, dataLength u32.
- **IndexBlock**: bare array of 36-byte `DataBlockIndex` entries — **no header, no count, no magic**. Reader derives count as `len / 36`.
- **Footer, 48 B**: indexStartPosition u64, indexBlockLength u32, 28 reserved zero bytes, magic `0x88e241b785f4cff7`.
- **Record header, 33 B**: magic `0x22`, streamId u64, epoch u64, baseOffset u64, count u32, payloadLength u32.

Three findings worth acting on:

**No checksums anywhere.** Grepped the whole `s3/` tree — no CRC, no digest, per-block or otherwise. Integrity is two magic-byte checks. This is only defensible for the `log` stream because its payload is a whole Kafka RecordBatch carrying Kafka's own CRC; **the meta, time and txn streams have zero integrity protection.**

**No compression.** The DataBlock `flag` byte is hardcoded `0x02` with `// TODO: first n bit is the compressed flag`, plus a dangling Zstd comment from a removed path. Records go to object storage exactly as they arrived.

**Two nested batch framings.** `StreamRecordBatch(33B) { Kafka RecordBatch(61B) { records } }` — duplicating `baseOffset` and `count`, and repeating `streamId`+`epoch` in every record inside a block that is single-stream by construction. 16 wasted bytes per batch.

**Reading the index is a guess-and-retry tail read:** `guessIndexBlockSize = 8192 + objectSize/1MiB * 36`, range-read the tail, retry once if the footer says the index starts earlier. Cold read = 1–2 GETs for metadata + 1 per data block.

### CompositeObject — the best idea they haven't published

**[Code]** `CompositeObject.java`, `CompositeObjectWriter/Reader.java`. An object containing **no record data at all** — only pointers to other live objects plus a copy of their index entries. Layout: `[ObjectsBlock][IndexesBlock][Footer]`, footer magic `…f8` (one greater than normal).

`CompactByCompositeObject` writes a few KB of index and marks each component `KEEP_DATA` instead of `DELETE` — so **N objects collapse into 1 from the controller's perspective with zero data movement and zero egress**. Compaction of the *metadata plane* decoupled from compaction of the *bytes*.

**[Inference]** Given that object count is a controller-side cost while object bytes are a cloud-bill cost, that separation is exactly right. It's gated behind `isStreamObjectCompactV1Supported()` and force-triggered when cluster object count hits 90% of `MAJOR_V1_COMPACTION_MAX_OBJECT_THRESHOLD = 400000` — i.e. it exists to solve a metadata scaling problem, bolted on after the fact. **Build it from day one.**

---

## 5. Write path, WAL, and where the ack happens

### The WAL is S3-only

**[Code]** `WriteAheadLog` interface: `append`, `get(RecordOffset)`, `confirmOffset`, `recover()`, `reset()`, `trim()`, `uri()`. Two implementations: `ObjectWALService` (production) and `MemoryWriteAheadLog` (tests). `DefaultWalFactory` throws on any protocol but `S3`, with a `//noinspection SwitchStatementWithTooFewBranches` acknowledging the single-valued pluggability.

### Offset-aligned object naming — the cleverest mechanism in the codebase

**[Code]** Offsets are ceil-aligned to `DATA_FILE_ALIGN_SIZE = 64 MiB` at every object boundary, so the key is computable:

```
{md5(nodeId)}/_kafka_{clusterId}/{nodeId}/{epoch}/wal/{startOffset}-{endOffset}
```

`floorAlignOffset(recordOffset)` → the containing object's start offset → its key. A random read is **one ranged GET on a computed path** — no index, no metadata round-trip, no LIST. The md5 prefix is key-space fan-out against S3 partition hot-spotting.

Cost: a single record may not exceed 64 MiB, and offset space is consumed in 64 MiB quanta. Cheap.

### Where the caller is acked

**After the WAL object PUT is durable, plus a post-PUT lease re-verification, plus insertion into the in-memory `LogCache`.** Everything downstream — stream-set object upload, controller commit, compaction — is asynchronous and off the ack path (`uploadDeltaWAL()` carries `@SuppressWarnings("UnusedReturnValue")`). **The WAL PUT is the only synchronous durable write in the system.**

### Batching — not what the config name suggests

**[Code]** `ObjectWALConfig` defaults: `batchInterval` **250 ms**, `maxBytesInBatch` **8 MiB**, object roll at **64 MiB**, `maxUnflushedBytes` **1 GiB** (throws `OverCapacityException`), `maxInflightUploadCount` **50**.

But the timer is a **minimum-interval-between-PUTs** scheme, not a linger:

```java
long forceUploadDelayNanos = min(max(minBulkUploadIntervalNanos,
    lastBulkForceUploadNanos + batchNanos - startNanos), batchNanos);
```

So an isolated append after idle flushes in **~10 ms**; under continuous load the timer path converges to ~4 PUTs/s. The comment is explicit: *"Try batch the requests in a short time window to save the PUT API."* **[Inference]** A token bucket over PUT operations would express the same cost constraint far more legibly.

---

## 6. Recovery and fencing

### Contiguity of keys is the durability boundary

**[Code]** `RecoverIterator.getContinuousFromTrimOffset` finds the first object past the trim offset, then walks forward accepting objects only while `objects[i].startOffset == objects[i-1].endOffset`, truncating at the first gap. **Since offsets are in the key, this needs no reads at all — the LIST result alone determines the valid prefix.**

> **Because S3 PUTs are atomic, there are no partial objects — only missing ones.**

That single property replaces every torn-write, trailer-record and log-truncation mechanism a block-device WAL needs. **[Inference]** This is the most transferable insight in the codebase and our recovery design should be built around it.

Belt-and-braces underneath: per-record `headerCRC` over the first 20 bytes, magic `0x87654321`, and a `bodyCRC32` — unreachable in practice given the above, defending against storage-side corruption rather than truncation.

Trim offset piggybacks in every WAL object header (48 B, magic `0xEDCBA987`), so recovery reads it from the newest object's first 48 bytes. Trimming with no traffic forces out a synthetic `(-1, -1)` zero-length record purely to carry the new trim offset — an ugly hack around an otherwise good mechanism.

The **controller is the authority on what was committed**: `getOpeningStreams()` yields each stream's durable `endOffset`, and `WALRecovery` filters the replay against it, dropping records already committed and logging `[BUG]` on any gap.

### The fencing weakness

**[Code]** `ObjectReservationService` writes a **21-byte** lease object (`magic | nodeId | nodeEpoch | failoverFlag`); `verify` is a ranged read plus exact byte comparison, failing closed. The failover flag being part of the compared bytes means a failover-mode acquire fences a normal-mode writer even at identical nodeId and epoch — neat.

But the check runs **after** the PUT:

```
PUT object → verify lease (extra GET) → if stale, fence and fail the appends
```

Correctness holds — appends are failed rather than acked, and the new owner deletes overlapping objects at startup. But it costs an extra GET per bulk on the ack path, and a partitioned zombie can keep writing objects nobody sweeps until the next restart.

**[Inference] The fix is available to us:** put a conditional-write precondition (`If-Match`/`If-None-Match`, see [04](04-object-storage-s3-gcs.md) §6) on the WAL PUT itself. Fencing becomes synchronous, the extra GET disappears, and the whole `skipOverlapObjects` cleanup path becomes unnecessary. **This is the biggest single improvement available over their design.**

### PREPARED → COMMITTED confirmed

**[Code]** `S3ObjectState = {PREPARED(1), COMMITTED(2), MARK_DESTROYED(3)}` — three states, not two. IDs come from `TimelineLong nextAssignedObjectId` in the controller. TTL is **60 minutes, hard-coded**. Commit is idempotent (`REDUNDANT_OPERATION` on re-commit) and refuses resurrection from any state but PREPARED. A 1000 ms controller sweep marks expired PREPARED objects destroyed, capped at `MAX_DELETE_BATCH_COUNT = 2000` per round *"to avoid blocking the event loop"*, with physical deletion deferred by a double-buffer swap.

**[Inference]** This is the right way to avoid LIST on the data path, and the deferred-destroy window is exactly the soft-delete-then-reap discipline [13](13-coordinator-recovery.md) §8 argues for. Copy the state machine wholesale. But note: if object creation rate exceeds 2,000/round the destroy queue grows unbounded — which is the pathology in issue #3477.

---

## 7. Cache and compaction

### Three caches **[Code]**

| Cache | Granularity | Eviction | Default |
|---|---|---|---|
| `LogCache` (write/tail) | decoded `StreamRecordBatch` | FIFO with explicit free-after-upload; drops from head only above 90% capacity | 200 MiB (broker: memory-derived) |
| `DataBlockCache` (read) | one DataBlock per `DataBlockIndex` | LRU + 1-min TTL, sharded by `streamId % CPU_CORES` | 100 MiB |
| `ObjectReaderLRUCache` | parsed index blocks | LRU | 100 MiB hard-coded |

**Tailing reads never touch S3**: `S3Storage.read0` checks `LogCache` first, and because uploaded blocks stay cached until pressure forces them out, the write cache *is* the tail buffer. `fastRead` makes a miss fail fast rather than falling through — an explicit "never block on object storage" contract.

**The `StreamReader` is a stateful cursor**, not a stateless range fetcher — keyed by `(streamId, startOffset)` and re-registered under its new offset after each read. `markReadCompleted()` frees blocks the instant a sequential consumer passes them, so a catch-up consumer barely consumes cache capacity at all.

### Readahead — worth copying verbatim **[Code]**

- Unit **512 KiB**, max **32 MiB**, grows **additively on cache misses only** (hits don't grow it).
- Skipped if the consumer hasn't consumed into the previously prefetched region.
- **Backpressure**: skipped entirely if `dataBlockCache.available() < nextReadaheadSize + 32 MiB`.
- **Reset on unread eviction**, with a 1-minute cooldown — and it distinguishes `EXPIRED` ("consumer reading too slowly") from `CAPACITY` ("increase the block cache") in the log.
- Readahead reads are tagged `CATCH_UP` priority; demand reads `BYPASS`. Priority order `BYPASS(0) < COMPACTION(1) < TAIL(2) < CATCH_UP(3)`.

### Ranged GET merging — real numbers for our cost model **[Code]**

`rangeRead()` doesn't issue a request; it parks a task. A scheduler tick every **5 ms** sorts waiting tasks by `(objectPath, start)` and greedily merges:

| Parameter | Value |
|---|---|
| merge tick | **5 ms** |
| `MAX_MERGE_READ_SIZE` | **4 MiB** per merged GET |
| sparsity gate | **0.5** — will pull up to 2× wanted bytes to save a request, not more |
| compaction read batch | 16 MiB (separate path) |

Plus four independent throttling layers, an AIMD write regulator over a 60 s control loop, and **hedged writes** — a duplicate PUT fires after `p99(objectSize)` from a live latency histogram, first-wins, capped at 5 concurrent.

### Five compaction types **[Code]**

Corrected from what we cited: SSO compaction every **5 min** (max 500 objects), force-split after **120 min**, stream-object `MINOR` every 5 min / `MAJOR` every 30–60 min at 10 GiB, plus V1 variants where `MAJOR_V1` uses composite objects (zero data movement). Stream-set compaction downloads and re-uploads every byte; only `MINOR`/`MAJOR` use S3 `UploadPartCopy` server-side copy.

`isSanityCheckFailed` verifies every input range is covered by output ranges before committing — cheap insurance on the one operation that can silently lose data. **[Inference]** Copy that.

---

## 8. What to copy, what to avoid

### Copy

1. **The `Stream` interface**, near-verbatim — 13 methods, offsets as the only addressing, `trim` as the only retention primitive, epoch-fenced single writer.
2. **Offset-aligned object naming** so keys are computable from an offset. Index-free random reads.
3. **Contiguity-of-keys as the durability boundary** (§6). The correct object-storage-native answer to "where does valid data end."
4. **PREPARED/COMMITTED/MARK_DESTROYED with TTL-based GC and IDs from consensus** — avoids LIST on the data path, idempotent commit, deferred physical delete.
5. **Composite objects** — metadata compaction decoupled from data compaction. Build it from day one.
6. **Miss-driven readahead** with cache-availability gating and the EXPIRED-vs-CAPACITY distinction.
7. **The cursor-shaped reader** with immediate post-read block freeing.
8. **Two-tier cache** where the write cache doubles as the tail buffer.
9. **`fastRead` as a first-class option** — an explicit never-touch-object-storage mode.
10. **Sanity-check range coverage before every compaction commit.**
11. **Sparse index eviction shape** — keep head + last N, thin the middle. Right for a log: readers at the tail, retention at the head.

### Do differently

1. **Checksum the data.** Their absence is only survivable because Kafka's inner batch carries a CRC.
2. **Version the format explicitly.** Bumping footer magic by one isn't versioning; spend the 28 reserved bytes.
3. **Put a header on the IndexBlock** — deriving entry count from `len / 36` hardcodes entry width forever.
4. **Kill the guess-and-retry tail read** — write the index size into object metadata or use a fixed generous tail read.
5. **Don't nest two batch framings** — hoist streamId/epoch to the DataBlock header where they're invariant.
6. **Synchronous fencing via conditional writes** (§6) instead of post-PUT verification.
7. **Implement compression or delete the flag.**
8. **A batched multi-stream fetch** — every read is currently one stream, one range.
9. **One compaction model, not five across two generations.**

### Fork-inherited baggage — don't copy

The 4-streams-per-partition layout; `MetaStream` as a JSON-blob KV log (Jackson-serialized `ElasticLogMeta` re-appended wholesale on every segment roll); `CreateStreamOptions.replicaCount` (validated, threaded through, then ignored — object storage does the replication); `Client.failover()` leaking an EBS-specific operation into the universal client API; dead EBS scaffolding (`WALUtil`'s block-device half, ring-buffer arithmetic, three deprecated configs that synthesize a URI the factory rejects); manual Netty refcounting throughout, which Rust erases for free.

---

## 9. Latent bugs found

Worth noting both as cautions and as evidence of what this class of design gets wrong **[Code, agent-identified]**:

1. **`DefaultRecordOffset.compareTo` ignores `epoch`** — it compares `offset` only, while `equals`/`hashCode` include `epoch`. So `compareTo(x) == 0` does not imply `equals(x)`: a `Comparable` contract violation that will bite anyone putting these in a `TreeSet`.
2. **`ObjectUtils.skipOverlapObjects` never advances `lastObject`** after the first iteration, so it effectively compares every object against the *first* one only. Dirty objects from a fenced epoch other than the first would not be detected. Worth a second pair of eyes before copying the cleanup logic.
3. **`Runtime.getRuntime().halt(1)` as error handling** in `commitDeltaWALUpload`'s exception path, alongside `System.err.println` and `printStackTrace`.

---

## 10. Their benchmarks — be skeptical **[Code + Inference]**

**There are no AutoMQ JMH benchmarks.** `grep -rli "automq\|s3stream" jmh-benchmarks/src/main/java/` returns **zero**. That directory is 100% inherited Apache Kafka microbenchmarks — no coverage of the S3 storage layer, metadata plane, or controller.

The real tool is `tools/.../automq/perf/`, an OpenMessaging-style end-to-end driver. Published methodology **[Web]** uses **r6in.large (2 vCPU, 16 GB)** and **1,000 partitions** in every test.

**[Inference]** 1,000 partitions is 1/20th of the Pro tier limit and 1/100th of where their own issue #1608 reports OOM. At 1,000 partitions the metadata image is ~2 MB — trivially fine on 16 GB. **They have published no benchmark at the partition scale where their architecture is known to break**, and the instance choice conveniently avoids the constraint. The Kafka baseline (GP3 EBS at 156 MiB/s) is also a storage-tier comparison dressed as an architecture comparison.

Two claims that *are* structurally true and worth taking seriously: reassignment in 2.2 s vs Kafka's 12 min (a metadata pointer swap vs a 30 GB copy), and catch-up-read isolation.

---

## 11. Design evolution — where the lessons are **[Web]**

| Era | Change | Trigger |
|---|---|---|
| Jun 2024 | **"Huge cluster" mode (V2)** — strip the range index out of `S3StreamSetObjectRecord`, offload a sparse index to S3 ([PR #1464](https://github.com/AutoMQ/automq/pull/1464)) | KRaft record volume |
| Jun 2024 | `OverloadCircuitBreaker` — controller **stops fencing brokers** when saturated, i.e. degrades correctness guarantees to survive | [#1446](https://github.com/AutoMQ/automq/issues/1446) |
| Jul 2024 | `RangeMetadata` GC | **OOM at 100k partitions** ([#1608](https://github.com/AutoMQ/automq/issues/1608)) |
| Oct 2024 | KRaft down from oversized commit batches — at **3.2 billion** KRaft records | [#2057](https://github.com/AutoMQ/automq/issues/2057) |
| 2024–25 | ~10 sparse-index bugfixes | the offload was leaky in production |
| May 2025 | **EBS/Regional EBS moved to Enterprise Edition**; full S3 support in OSS | consolidation |
| Open | Third redesign of the SSO lookup path (bloom filters, lazy load) | [#2731](https://github.com/AutoMQ/automq/issues/2731) |
| Open | Controller saturation at 5k topics | [#3477](https://github.com/AutoMQ/automq/issues/3477) |
| Open | MetadataImage OOM, 24 GB heap dump | [#2332](https://github.com/AutoMQ/automq/issues/2332) |

**[Inference]** The pattern is unmistakable: every redesign moves metadata *out of* KRaft while leaving the per-partition and per-object records *in* KRaft. That's where the ceiling lives, and it's still there.

---

## 12. What this resolves for us

- **Open question on AutoMQ's ceiling** (raised in [14](14-metadata-scale-and-tiering.md) §12): **answered** — KRaft as metadata plane, evidenced by #1608 and #3477. Not SKU tiering.
- **Validates [15](15-scale-architecture-position.md) §4's principal-indexed requirement** from the opposite direction: AutoMQ's failure is precisely "every node holds all metadata."
- **Validates the conditional-write fencing direction** — their post-PUT verification is the weakness we can design out.
- **Supplies real numbers** for the cost model: 5 ms merge tick, 4 MiB merge cap, 0.5 sparsity gate, 512 KiB→32 MiB readahead, 250 ms/8 MiB/64 MiB WAL batching.
- **Removes the attached-disk-WAL option from AutoMQ's evidence base** — docs 06 and 11 present it as a live alternative citing AutoMQ; that citation no longer holds.

---

## Sources

**Source clone** — `AutoMQ/automq` @ `eccde72`, version 3.9.0-SNAPSHOT, shallow clone Aug 2026. Key paths:
`s3stream/src/main/java/com/automq/stream/` — `api/Stream.java`, `s3/{ObjectWriter,ObjectReader,DataBlockIndex,CompositeObject*,S3Storage,WALRecovery}.java`, `s3/wal/impl/object/{DefaultWriter,RecoverIterator,ObjectReservationService,ObjectUtils}.java`, `s3/cache/{LogCache,blockcache/*}.java`, `s3/compact/{CompactionManager,CompactionAnalyzer,StreamObjectCompactor}.java`, `s3/index/*.java`, `s3/operator/AbstractObjectStorage.java`, `s3/Config.java`
`metadata/src/main/{resources/common/metadata/*.json, java/org/apache/kafka/{controller/stream/*,image/S3Streams*}.java}`
`core/src/main/{scala/kafka/log/streamaspect/*, java/kafka/automq/{AutoMQConfig.java,failover/*}}`
`raft/src/main/java/org/apache/kafka/raft/KafkaRaftClient.java`

**Issues and PRs**
- [#1608 — RangeMetadata OOM at 100k partitions](https://github.com/AutoMQ/automq/issues/1608)
- [#3477 — controller event-queue saturation at ~5,000 topics](https://github.com/AutoMQ/automq/issues/3477)
- [#2057 — KRaft down, commit recordbatch exceeds limit](https://github.com/AutoMQ/automq/issues/2057)
- [#1446 — quota for low-priority controller mutations](https://github.com/AutoMQ/automq/issues/1446)
- [#2731 — open redesign of S3StreamsMetadataImage.getObjects](https://github.com/AutoMQ/automq/issues/2731)
- [#2332 — MetadataImage OOM](https://github.com/AutoMQ/automq/issues/2332)
- [PR #1464 — optimize streamsetobject to support huge cluster](https://github.com/AutoMQ/automq/pull/1464)
- [PR #2709 — remove empty RangeMetadata](https://github.com/AutoMQ/automq/pull/2709)
- [PR #3379 — dedupe S3 metadata after snapshot replay](https://github.com/AutoMQ/automq/pull/3379)

**Docs and marketing** (for contrast with the code)
- [AutoMQ FAQ — tier partition limits](https://www.automq.com/faq) · [Pricing](https://www.automq.com/pricing) · [Performance benchmark methodology](https://docs.automq.com/automq-cloud/appendix/performance-benchmark)
- [Insight: Metadata Management in AutoMQ](https://www.automq.com/blog/insight-metadata-management-in-automq) · [Deep dive into the challenges of building Kafka on top of S3](https://www.automq.com/blog/deep-dive-into-the-challenges-of-building-kafka-on-top-of-s3) · [Releases 1.5.0/1.6.0/1.7.0](https://github.com/AutoMQ/automq/releases)
