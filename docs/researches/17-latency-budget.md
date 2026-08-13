---
title: "Latency Budget: What Is Actually Achievable on Object Storage"
slug: latency-budget
status: draft
last_updated: 2026-08-13
tags: [latency, produce-latency, tail-reads, cache, s3-express, lazy-sequencing, budget, tradeoffs]
related: [04-object-storage-s3-gcs, 11-low-latency-tiers-and-interaz-costs, 16-automq-deep-dive, 12-object-discovery-and-api-cost, 01-warpstream-architecture]
summary: >
  Decomposes produce and consume latency for an object-storage-native broker
  and answers "how fast can this actually be." Core finding: single-digit-ms
  produce and object-storage-only durability are mutually exclusive — every
  vendor claiming otherwise is using a faster durable medium or weakening a
  guarantee. Tail reads are the opposite: genuinely single-digit ms via cache,
  because the cache is populated exactly at durability.
---

# Latency Budget: What Is Actually Achievable on Object Storage

*Compiled 2026-08-13. Synthesizes the write-path findings in [16](16-automq-deep-dive.md), the storage physics in [04](04-object-storage-s3-gcs.md), and the low-latency tier options in [11](11-low-latency-tiers-and-interaz-costs.md) into one budget.*

> **Why this document exists.** "How fast can it be?" recurs constantly, and the honest answer requires separating two questions that get conflated in every vendor claim: **produce latency** and **tail-read latency**. They have completely different answers and completely different mechanisms.

---

## 1. The headline finding

> **Single-digit-millisecond produce latency and object-storage-only durability are mutually exclusive.**

Every system claiming single-digit-ms produce is doing one of three things: writing to a faster durable medium (attached disk, EBS, FSx, S3 Express), acking before durability, or acking before sequencing. None of them beat the physics of a durable PUT to S3 Standard.

**Tail reads are the opposite case** — genuinely single-digit ms, achieved purely with in-memory caching, and that mechanism transfers cleanly.

---

## 2. Tail reads: why they're fast, and why it's sound

**[Documented, from AutoMQ source — [16](16-automq-deep-dive.md) §7]** A tailing read never touches object storage. `S3Storage.read0` checks `LogCache` first; because uploaded blocks stay resident until ~90% capacity pressure evicts them, **the write cache *is* the tail buffer**. A tail fetch returns a completed future synchronously with `CacheAccessType.DELTA_WAL_CACHE_HIT` — no S3 call, no index lookup, no block cache.

**The property that makes this sound rather than a data-loss window:** the cache is populated *at* the moment of durability, never before it. The producer is acked only after the WAL object PUT succeeds **and** the record is inserted into `LogCache`. So the cache is never ahead of durability — reads are fast because the data is already durable and happens to still be in RAM.

**[Synthesis]** This is the correct ordering and we should copy it exactly. The tempting inversion — populate the cache on receipt, ack after durability, serve reads from cache in between — creates a window where a consumer can observe a record that a crash would erase. That is not a Kafka-legal state.

**LIST is not in the read path at all.** Tail reads hit cache; catch-up reads go coordinator-index → ranged GET ([12](12-object-discovery-and-api-cost.md)). AutoMQ's WAL path does LIST, but only at startup and on epoch change — never per-read. Any "500 ms–1 s" figure in this space is the cold object-storage round trip, not a LIST cost.

---

## 3. Produce latency, decomposed

Using AutoMQ's S3-only path as the worked example, since we have its source **[Code, [16](16-automq-deep-dive.md) §5]** and it is the honest floor for a pure-object-storage design:

| Step | Cost | Source |
|---|---|---|
| Batch wait | **~10 ms if idle, up to 250 ms** under sustained load | `ObjectWALConfig.batchInterval`, but see note below |
| S3 PUT (the WAL object) | **p50 ~60–70 ms, p99 130–250 ms** | [04](04-object-storage-s3-gcs.md) §2 |
| Lease re-verify (an extra **GET**, on the ack path) | ~15–60 ms | `ObjectReservationService.verify` |
| LogCache insert → ack producer | negligible | |
| **Total** | **~85 ms floor, ~300–500 ms typical** | |

⚠️ **The batching knob is not a linger.** AutoMQ's `batchInterval` (250 ms) implements a *minimum interval between PUTs*, not a wait-before-flush:

```java
forceUploadDelayNanos = min(max(minBulkUploadIntervalNanos,
    lastBulkForceUploadNanos + batchNanos - startNanos), batchNanos);
```

So an isolated append after idle flushes in ~10 ms, while under continuous load the timer path converges to ~4 PUTs/s. The stated intent is *"batch the requests in a short time window to save the PUT API"* — it is a **cost** knob wearing a latency knob's name. **[Synthesis]** A token bucket over PUT operations would express the same constraint far more legibly, and we should do that instead.

**The lease-verify GET is pure overhead we can delete.** It exists because AutoMQ fences *after* the PUT. Conditional-write fencing ([16](16-automq-deep-dive.md) §6, [04](04-object-storage-s3-gcs.md) §6) folds the check into the PUT itself — saving 15–60 ms on every ack and removing the dirty-object cleanup path.

---

## 4. Cross-system comparison

**[Documented / vendor-reported]** All figures as published; see the source docs for caveats.

| System / configuration | Produce latency | Mechanism |
|---|---|---|
| AutoMQ, S3-only WAL | ~85 ms floor, 300–500 ms typical | S3 PUT + lease GET |
| WarpStream, S3 Standard | **~400–600 ms p99**, <1.5 s p99 end-to-end | 250 ms batch + PUT + metadata commit |
| KIP-1150 Diskless (projected) | **P50 ~500 ms, P99 1–2 s** | 250 ms / 4 MiB batch + PUT + coordinator |
| S2.dev, Standard tier | **p99 <500 ms** | batching WAL → S3 |
| turbopuffer | 200 ms – 1 s commit | group commit + CAS manifest |
| **WarpStream + S3 Express quorum** | **p99 ~169 ms, median ~105 ms** | 3-bucket quorum on Express One Zone |
| S2.dev, Express tier | **p99 <50 ms** | quorum of 3 Express buckets |
| **WarpStream Lightning + Express** | **33 ms median, p99 <50 ms** | ack before sequencing |
| AutoMQ + EBS/FSx WAL | **sub-10 ms** | ⚠️ **not in the OSS tree** — moved to Enterprise Edition ([16](16-automq-deep-dive.md) §2) |

**[Synthesis]** The pattern is unambiguous: **nothing achieves under ~100 ms on S3 Standard.** The sub-100 ms entries all use S3 Express One Zone or a non-object durable medium; the sub-50 ms entries additionally weaken a guarantee.

---

## 5. Our four options, with what each costs

| Approach | Produce latency | What it costs |
|---|---|---|
| **S3 Standard only** | ~85 ms floor, 300–500 ms typical | Cheapest and simplest. No fast tier, no extra storage premium, no weakened guarantees. |
| **S3 Express One Zone quorum** | p99 ~169 ms, median ~105 ms | ~5× storage price on a transient buffer, plus **bounded** cross-AZ transfer — quorum writes are billed as ordinary inter-AZ traffic, ~$0.016/GiB ([11](11-low-latency-tiers-and-interaz-costs.md) §6). Needs compaction down to Standard. |
| **Local / attached WAL** | single-digit ms | Reintroduces per-broker durable state, broker affinity, and failover complexity — the things diskless exists to eliminate. AutoMQ moved this out of OSS entirely. |
| **Ack before sequencing** (Lightning-style) | 33 ms median | Forfeits idempotent producer, transactions, and external consistency; returns offset 0 in the produce response; requires a background scanner that **does** enumerate object storage ([13](13-coordinator-recovery.md) §7). |

**[Synthesis] Recommendation shape**, pending the throughput target still open in [10](10-open-questions.md):

Start with **S3 Standard only** and treat ~300–500 ms p99 as the product's honest position. It is competitive with WarpStream's default and KIP-1150's projection, it is the cheapest and simplest, and it keeps every Kafka guarantee intact. Make the storage tier pluggable so **S3 Express quorum** is a configuration rather than a rewrite — that is the one upgrade that buys ~3–4× latency without weakening semantics, and per [11](11-low-latency-tiers-and-interaz-costs.md) the cost is bounded and modelable.

Treat lazy sequencing as a last resort. The latency win is real, but it costs idempotency and transactions — and those are exactly the features that make a Kafka-compatible system credible rather than a lookalike.

---

## 6. What to be skeptical of

**[Synthesis]** Whenever a system in this space advertises a latency number:

1. **Ask which durable medium.** Sub-10 ms always means something other than S3 Standard. Marketing rarely names the configuration — AutoMQ's sub-10 ms figures required EBS or FSx, and that path is no longer in their open-source code at all.
2. **Ask produce or end-to-end.** WarpStream's ~400 ms p99 *produce* corresponds to <1.5 s p99 *end-to-end*. Different numbers, both real.
3. **Ask which guarantees are still on.** 33 ms median with idempotence disabled is not comparable to 105 ms with it enabled.
4. **Ask at what partition count.** Every AutoMQ benchmark uses 1,000 partitions — 1/20th of their own Pro tier limit ([16](16-automq-deep-dive.md) §10).

---

## Sources

Figures are drawn from the source documents rather than re-cited here:
- Produce-path decomposition and AutoMQ code figures — [16-automq-deep-dive.md](16-automq-deep-dive.md) §5–7
- S3/GCS latency distributions — [04-object-storage-s3-gcs.md](04-object-storage-s3-gcs.md) §2
- S3 Express One Zone figures and quorum-write cost — [11-low-latency-tiers-and-interaz-costs.md](11-low-latency-tiers-and-interaz-costs.md) §2, §6
- WarpStream latency and Lightning Topics — [01-warpstream-architecture.md](01-warpstream-architecture.md), [06-distributed-systems-design-challenges.md](06-distributed-systems-design-challenges.md) §2
- KIP-1150 projections — [02-kafka-protocol-compatibility.md](02-kafka-protocol-compatibility.md) §5, [14-metadata-scale-and-tiering.md](14-metadata-scale-and-tiering.md)
- turbopuffer commit latency — [08-turbopuffer-lessons.md](08-turbopuffer-lessons.md) §2
- Read-path caching and LIST avoidance — [12-object-discovery-and-api-cost.md](12-object-discovery-and-api-cost.md) §4.5, §5
