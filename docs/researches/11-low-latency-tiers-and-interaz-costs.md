---
title: "Low-Latency Storage Tiers (S3 Express, GCS Rapid) and Inter-AZ/Inter-Zone Transfer Costs"
slug: low-latency-tiers-and-interaz-costs
status: draft
last_updated: 2026-08-13
tags: [s3-express-one-zone, gcs-rapid, rapid-bucket, rapid-cache, inter-az, cross-zone, networking-cost, kip-392, kip-881, quorum-write]
related: [04-object-storage-s3-gcs, 01-warpstream-architecture, 06-distributed-systems-design-challenges, 03-competitive-landscape]
summary: >
  Deep dive on AWS S3 Express One Zone and GCP's new Cloud Storage Rapid
  (Rapid Bucket + Rapid Cache) — architecture, pricing, latency, durability —
  plus a three-way (AWS/GCP/Azure) inter-AZ/inter-zone transfer cost
  comparison, why it dominates traditional Kafka infrastructure cost, and
  concrete techniques (including the diskless architecture itself) to
  eliminate or bound it.
---

# Low-Latency Storage Tiers and Inter-AZ/Inter-Zone Transfer Costs

> **⚠️ CORRECTION (2026-08-13) — AutoMQ claims in this document.** Statements here about AutoMQ's **EBS-backed WAL** (sub-10ms produce latency) and **EBS Multi-Attach failover** ("within milliseconds") were taken from AutoMQ's blog and **do not match their shipping code**. A source study at commit `eccde72` found the block-device WAL removed entirely (`BlockWALService`, `SlidingWindowService`, `WALBlockDeviceChannel` — zero hits; the WAL factory throws on any protocol but S3). Failover is now "a healthy broker re-uploads the dead broker's WAL objects from shared storage", with a **1-minute minimum grace period** and **one failover at a time cluster-wide**. Per their 1.5.0 notes, EBS support moved to their Enterprise Edition. See [16-automq-deep-dive.md](16-automq-deep-dive.md) §2 for the full correction list.


*Compiled 2026-08-13, extending [04-object-storage-s3-gcs.md](04-object-storage-s3-gcs.md) §2 and §7–8 with a dedicated deep dive requested to inform the write-path cost/latency design.*

> **How this fits the project:** this is the detail layer under [04](04-object-storage-s3-gcs.md)'s latency/cost summary and [06](06-distributed-systems-design-challenges.md) §2's batching discussion. Read those first for context; come here when you need exact current pricing or the inter-AZ networking math.

---

## 1. TL;DR comparison

| | S3 Express One Zone (AWS) | Cloud Storage Rapid Bucket (GCP) | Azure equivalent |
|---|---|---|---|
| Scope | Single AZ, opt-in "directory bucket" | Single zone, opt-in `RAPID` storage class | No confirmed direct equivalent found |
| Storage cost | ~$0.11/GB-mo (post Apr-2025 cut, was $0.16) | ~$0.11/GB-mo | — |
| vs. standard tier storage cost | ~4.8x S3 Standard ($0.023) | ~5.5x GCS Standard ($0.02) | — |
| Write request cost | $0.00113/1,000 (post-cut) | $0.00113/1,000 (Class A) — **numerically identical** | — |
| Read request cost | $0.00003/1,000 (post-cut, 85% cut) | $0.0002/1,000 (Class B) — **~6.7x more than S3's cut price** | — |
| Published latency | Single-digit ms (p50≈3ms, p99≈15ms independent bench) | "Sub-millisecond" (Google claim, no p50/p99 table published) | — |
| Durability/SLA | 11 nines intra-AZ; 99.9% availability SLA | 11 nines intra-zone; 99.9% availability SLA | — |
| GA date | Nov 2023 | Rapid Bucket GA Apr 2026 (preview Apr 2025 as "Rapid Storage") | — |
| Cross-zone access from wrong zone | Billed as standard $0.01/GB-each-way inter-AZ transfer | Not specifically priced; architecturally discouraged | — |

**Notable finding:** GCS Rapid's storage price and write-request price are numerically identical to AWS's post-cut S3 Express One Zone figures — suggesting Google price-matched AWS on those two dimensions specifically, while pricing reads ~6.7x higher than AWS's now-heavily-discounted GET rate.

**Inter-AZ/inter-zone transfer, same-region:**

| Provider | Same-zone/AZ | Cross-zone/AZ, same region | Compute ↔ regional object storage, same region |
|---|---|---|---|
| AWS | Free | $0.01/GB each direction ($0.02/GB round-trip) | Free (any AZ, S3 Standard) |
| GCP | Free | $0.01/GiB each direction | Free (any zone, regional GCS) |
| Azure | Free | **Free since May 21, 2024** | Free |

AWS and GCP remain cost-equivalent on cross-zone networking; Azure removed the fee entirely as a stated resiliency/competitive play. This matters if the project ever targets Azure — the whole "avoid cross-AZ cost" design pressure explored in this doc is Azure-irrelevant.

---

## 2. S3 Express One Zone deep dive

### 2.1 Architecture

S3 Express One Zone introduces a distinct bucket type — the **directory bucket** — architecturally different from S3 Standard's general-purpose buckets:

- **Genuine hierarchical namespace** (real directories), not the flat-key-with-delimiter simulation of general-purpose buckets.
- **AZ is baked into bucket identity**: names follow `bucket-name--azid--x-s3` (e.g. `logs--use1-az1--x-s3`) — you cannot create one without picking a specific AZ. [AWS docs](https://docs.aws.amazon.com/AmazonS3/latest/userguide/directory-bucket-high-performance.html)
- **Session-based auth (`CreateSession`)**: unlike general-purpose buckets' per-request SigV4, Express One Zone issues short-lived (5-minute) bucket-scoped session credentials, reused across many zonal calls — specifically to shave the per-request auth overhead that would otherwise dominate at single-digit-ms speeds. SDKs handle this transparently.
- **Regional vs. Zonal endpoints**: bucket-management calls (`CreateBucket`, policy) go through a *Regional* endpoint under standard IAM; object operations (`PutObject`, `GetObject`) go through a *Zonal* endpoint tied to the AZ — this split is part of the speed mechanism.
- **No per-prefix throughput partitioning.** S3 Standard scales request rate per key *prefix* (from a 3,500 PUT/5,500 GET baseline, requiring key-space sharding to scale further). Directory buckets provision capacity **per bucket** at creation — hundreds of thousands of TPS instantly, no prefix-sharding tricks needed. [Sedai](https://sedai.io/blog/getting-started-s3-express-one-zone)
- **Bucket-level-only access control** — no prefix/tag-scoped policy, no ACLs, Object Ownership fixed to bucket-owner-enforced.
- **Reduced feature surface**: no SSE-C/DSSE-KMS, no MD5 checksums (CRC32/CRC32C/SHA-1/SHA-256 only), no S3 server access logs (CloudTrail data events instead).
- **Free gateway VPC endpoint access**, same as S3 Standard.

### 2.2 Pricing — including the April 2025 restructuring

AWS cut Express One Zone pricing sharply effective **April 10, 2025**, across all 7 supported regions at the time. This was not just a rate cut — it changed the billing *model*:

- **Before**: PUT/GET billed as a flat per-request fee covering the first 512 KB, plus a separate per-GB charge only for bytes *beyond* 512 KB.
- **After**: the per-GB data-transfer charge applies to **all bytes**, not just the excess — while the per-GB rate itself dropped 60%. Net effect for small-object-heavy workloads (Kafka's typical batch sizes): cheaper on both the flat per-request fee and the broadened-but-cheaper per-byte charge.

US East (N. Virginia), before → after:

| Metric | Pre-Apr-2025 | Post-Apr-2025 | Cut |
|---|---|---|---|
| Storage ($/GB-mo) | $0.16 | $0.11 | 31% |
| PUT/COPY/POST/LIST (per 1,000) | $0.0025 | $0.00113 | 55% |
| GET/HEAD (per 1,000) | $0.0002 | $0.00003 | 85% |
| Upload overage ($/GB) | $0.008 | $0.0032 | 60% |
| Retrieval overage ($/GB) | $0.0015 | $0.0006 | 60% |

[AWS blog](https://aws.amazon.com/blogs/aws/up-to-85-price-reductions-for-amazon-s3-express-one-zone/), corroborated by [HyperFRAME Research](https://hyperframeresearch.com/2025/04/24/aws-s3-express-one-zone-just-a-price-cut/) and [Network World](https://www.networkworld.com/article/3960252/aws-slashes-amazon-s3-express-one-zone-pricing-by-up-to-85.html).

Regional price variance exists (percentage cuts were uniform across regions, absolute dollar figures aren't) — check the live AWS pricing page/calculator for a specific target region rather than assuming US East figures apply everywhere.

### 2.3 Latency and throughput

AWS's own claim: "consistent single-digit millisecond first-byte read and write latencies — up to 10x faster than S3 Standard." [AWS docs](https://docs.aws.amazon.com/AmazonS3/latest/userguide/s3-express-performance.html)

Independent benchmark (Nixiesearch), objects <1MB: **p50≈3ms, p95≈8ms, p99≈15ms**; 1–10MB objects: p50≈5ms, p95≈12ms, p99≈20ms. Still 5–10x slower than local EBS/instance store, with a warm-up effect as request volume increases. [Nixiesearch](https://nixiesearch.substack.com/p/benchmarking-read-latency-of-aws)

Request-rate quotas: **200,000 reads/sec and 100,000 writes/sec default per bucket**, raisable via support request up to a documented ceiling of **2,000,000 GET TPS / 200,000 PUT TPS per bucket** — provisioned per-bucket, not per-prefix, eliminating S3 Standard's key-sharding requirement. [AWS docs](https://docs.aws.amazon.com/AmazonS3/latest/userguide/s3-express-performance.html)

### 2.4 Durability and availability

**Explicitly single-AZ, no cross-AZ replication at all** — redundancy is only across multiple devices *within* the chosen AZ. [AWS docs](https://docs.aws.amazon.com/AmazonS3/latest/userguide/directory-bucket-high-performance.html)

- Designed for 99.95% availability within the AZ; **contractual SLA is 99.9%** monthly uptime, with tiered service credits (10% below 99.9%, 25% below 99.0%, 100% below 95.0%), calculated against charges *in the affected AZ only*. [AWS S3 SLA](https://aws.amazon.com/s3/sla/)
- On an AZ outage: data in that AZ's directory buckets becomes **inaccessible** for the outage duration; data is expected to remain intact and become accessible again on recovery (assuming no physical loss), but there's **no automatic failover** — no other-AZ copy exists to fail over to. AWS added Fault Injection Service support (Aug 2025) specifically so customers can rehearse this failure mode. [AWS FIS docs](https://docs.aws.amazon.com/fis/latest/userguide/az-availability-scenario.html), [AWS announcement](https://aws.amazon.com/about-aws/whats-new/2025/08/amazon-s3-express-one-zone-supports-resilience-testing-aws-fault-injection-service)
- The "11 nines" durability figure applies to surviving concurrent *device* failures within the AZ — not AZ-level disasters (fire, flood, power loss). This is exactly why WarpStream/S2.dev layer their own multi-bucket quorum scheme on top (§5).

---

## 3. GCS Rapid deep dive (Rapid Bucket + Rapid Cache)

### 3.1 Product identity — two distinct, often-conflated offerings

- **Rapid Bucket** — a genuine new **zonal storage location type**, storage class `RAPID`. The direct GCS analogue to S3 Express One Zone. Previously called "Rapid Storage" during preview; "Rapid" remains the storage-class API name. [Rapid Bucket docs](https://docs.cloud.google.com/storage/docs/rapid/rapid-bucket)
- **Rapid Cache** — formerly "Anywhere Cache." **Not a storage class** — an SSD-backed, zonal, read-through cache sitting in front of *any* existing bucket (multi-region/dual-region/regional). Orthogonal to Rapid Bucket, not competing with it. [Rapid Cache docs](https://docs.cloud.google.com/storage/docs/rapid/rapid-cache)

**Timeline:** preview announced Google Cloud Next '25 (Apr 9–11, 2025) as "Rapid Storage," built on Colossus ([InfoQ](https://www.infoq.com/news/2025/05/google-cloud-rapid-storage/), [Colossus deep-dive](https://cloud.google.com/blog/products/storage-data-transfer/how-the-colossus-stateful-protocol-benefits-rapid-storage)); Rapid Bucket reached **GA at Next '26 (April 23, 2026)**, alongside Rapid Cache's new "ingest-on-write" feature. [GA blog](https://cloud.google.com/blog/products/storage-data-transfer/cloud-storage-rapid-turbocharges-object-storage-for-ai-analytics)

### 3.2 Architecture — how sub-millisecond latency is achieved

Rapid Bucket runs on **Colossus** (Google's internal cluster filesystem, also underlying Spanner/Bigtable/BigQuery), exposed via a **stateful gRPC streaming protocol** instead of GCS's normal stateless REST/JSON:

- A client opens a **gRPC stream** against an object; at stream-open the Colossus Curator does auth/metadata resolution *once* and returns a handle with the object's physical replica locations.
- Subsequent reads/appends go **directly to Colossus storage nodes** via an RDMA-like protocol (Snap networking), bypassing per-request metadata/auth overhead — this is the main latency win vs. REST GCS.
- **Native appendable objects**: only `BidiWriteObject` (append mode) can write; only one writer stream may hold an object at a time, with transactional fencing/version-increment on failover. 5 TiB object size cap.

**Requires hierarchical namespace + uniform bucket-level access** (both mandatory). **Incompatible features**: soft delete, Object Versioning, cross-bucket replication, Autoclass, Bucket/Object Retention Lock, resumable uploads via JSON, composite objects, CSEK, HMAC keys, BigQuery integration, Requester Pays, XML multipart uploads. [Rapid Bucket docs](https://docs.cloud.google.com/storage/docs/rapid/rapid-bucket)

**⚠️ Unresolved inconsistency flagged by research**: Google's GA marketing blog claims access via "high-performance gRPC **and S3-compatible APIs**," but the Rapid Bucket product docs describe writes as gRPC-only (`BidiWriteObject`) with no documented S3-compatible write path. **Verify directly with Google before depending on S3-compatible produce-path semantics.**

### 3.3 Pricing (confirmed, `us-central1`)

| Metric | RAPID class (zonal) | Standard class (regional) | Ratio |
|---|---|---|---|
| Storage | $0.000150685/GiB-hr → **≈$0.11/GB-mo** | $0.000027397/GiB-hr → **≈$0.02/GB-mo** | ~5.5x |
| Class A ops (writes/lists), per 1,000 | **$0.00113** | $0.005 | ~4.4x cheaper |
| Class B ops (reads), per 1,000 | **$0.0002** | $0.0004 | 2x cheaper |
| Retrieval/min-duration fees | None | None | — |

[Cloud Storage pricing](https://cloud.google.com/storage/pricing), [storage classes reference](https://docs.cloud.google.com/storage/docs/storage-classes). No itemized per-GB data-transfer charge analogous to S3 Express's separate upload/retrieval fee was found published — confirm on the live regional pricing page before finalizing a cost model, since GCS pricing pages render dynamically per-region.

### 3.4 Latency and throughput

Rapid Bucket published figures: sub-millisecond read/write (no p50/p99 table published), **15+ TB/s aggregate bandwidth per bucket**, **20M requests/sec per bucket**, plus workload-specific claims (50% reduced GPU blocked time, 2.5x faster data loading, 5x faster checkpoint restores — these are ML-training-oriented benchmarks, not streaming/queue-relevant). Google also claims "5x lower latency... compared to other leading hyperscalers" with no published methodology — **treat as unverified marketing, not a benchmark**. [GA blog](https://cloud.google.com/blog/products/storage-data-transfer/cloud-storage-rapid-turbocharges-object-storage-for-ai-analytics), [InfoQ](https://www.infoq.com/news/2025/05/google-cloud-rapid-storage/)

Rapid Cache: up to 2.5 TB/s aggregate read throughput, bandwidth starting at 100 Gbps and scaling ~20 Gbps per 1 TiB cached, 2MB chunk-based ingestion, TTL eviction (24hr–7day configurable).

**Gap flagged**: unlike AWS's published single-digit-ms figures for S3 Express, **no official Google p50/p99 millisecond latency table exists** for Rapid Bucket in any source found — only qualitative "sub-millisecond" claims.

### 3.5 Durability and availability

- **11 nines annual durability**, same headline figure Google quotes for all classes/locations — for zonal data this is against hardware failures only (disk/host/rack), not zone failure.
- **Availability SLA: 99.9%** — same as regional Standard/Nearline/Coldline/Archive, and *lower* than the 99.95% SLA for multi-region/dual-region Standard buckets. [Storage classes docs](https://docs.cloud.google.com/storage/docs/storage-classes)
- **Zone outage**: Google's own docs state zonal data "may become unavailable or permanently lost" in a zone outage — explicit documented risk, no cross-zone redundancy exists to fail over to.
- **Parity with S3 Express**: both offerings land at essentially the same durability/SLA shape (11 nines intra-zone, 99.9% availability) — the two clouds made the same tradeoff.

---

## 4. Why inter-AZ transfer cost dominates traditional Kafka infrastructure cost

**Quantified cost share (AWS, Confluent's own analysis).** Cross-AZ transfer "can account for more than 50%" of total infrastructure cost for self-managed multi-AZ Kafka; once tiered storage has already reduced storage spend, **networking can rise to ~90% of remaining infrastructure cost**. [Confluent](https://www.confluent.io/blog/understanding-and-optimizing-your-kafka-costs-part-1-infrastructure/)

Confluent's worked formula: `cross-AZ throughput (MB/s) × 2,592,000 sec/mo × 0.001 GB/MB × $/GB-cross-AZ`. At the standard $0.02/GB round-trip rate: **$4,838/month at 20 MB/s ingress**, **$24,192/month at 100 MB/s ingress**, from cross-AZ networking alone.

**Where the traffic comes from — three multiplicative sources**, with brokers/clients spread across 3 AZs and RF=3:
1. **Producer → leader**: ~2/3 of the time the partition leader is in a different AZ than the producer.
2. **Leader → followers (replication)**: RF=3 typically places followers in the other 2 AZs — 2 cross-AZ copies made per byte produced.
3. **Leader → consumer**: absent follower-fetching, ~2/3 of the time the consumer is in a different AZ than the leader — called out as the single largest contributor once consumer fan-out (multiple consumer groups re-reading the same stream) is factored in.

Independent worked example (getkafkanated): a 10 MB/s write / 50 MB/s read workload modeled at **$36.9k/year** total cross-AZ cost, of which **consumer read traffic alone was $20.5k/year** — larger than producer+replication combined, because fan-out multiplies read traffic in a way producer/replication traffic doesn't. [getkafkanated](https://getkafkanated.substack.com/p/how-kip-881-and-kip-392-reduce-inter)

**Concrete comparables:**
- AutoMQ's worked example: a traditional 3-broker Kafka cluster pays **$4,050/month** cross-AZ transfer against only **$272/month** compute — cross-AZ networking runs ~**15x** the compute bill. [AutoMQ](https://www.automq.com/blog/how-automq-reduces-nearly-100-of-kafka-cross-zone-data-transfer-cost)
- Redpanda claims bypassing inter-broker replication (writing direct to object storage) cuts cross-AZ/infra cost up to 90% vs. traditional replicated-broker Kafka. [Redpanda](https://www.redpanda.com/blog/calculate-cloud-data-transfer-costs)

---

## 5. Techniques to reduce inter-AZ/inter-zone cost

### 5.1 Within classic (non-diskless) Kafka

- **Rack-awareness (`broker.rack`) + Fetch-from-Follower (KIP-392).** Setting `broker.rack` per broker (mapped to AZ), plus `client.rack` on consumers and `replica.selector.class=RackAwareReplicaSelector` (Kafka 2.4+), lets consumers fetch from an in-AZ follower instead of a possibly-remote leader — safe because consumers only ever see data below the high-watermark, already present on in-sync followers. Directly attacks the leader→consumer hop.
- **KIP-881 rack-aware partition assignment** (Kafka 3.5+) goes further: the consumer-group assignor itself prefers AZ-local assignments — matters especially when AZ count exceeds replication factor, where fetch-from-follower alone can't guarantee an in-AZ replica exists for every assigned partition.
- Combined impact estimated at **~50% reduction in consumer-side cross-AZ cost**. [getkafkanated](https://getkafkanated.substack.com/p/how-kip-881-and-kip-392-reduce-inter)

### 5.2 Networking-layer (cloud-agnostic pattern, AWS specifics shown)

- **VPC Gateway Endpoints for S3** to avoid NAT Gateway cost: NAT Gateway costs $0.045/hr + $0.045/GB processed even for same-region traffic to an AWS service; a Gateway VPC Endpoint routes privately at **zero hourly and zero per-GB** charge, while ordinary cross-AZ/egress rules still apply on top. Both S3 Standard and Express One Zone directory buckets support this at no extra cost. Typical guidance cites 50–80%+ reduction in network-egress-adjacent spend. [Vantage](https://www.vantage.sh/blog/nat-gateway-vpc-endpoint-savings)
- **GCP equivalent**: Private Google Access / Private Service Connect for GCS endpoints on VMs without external IPs, avoiding Cloud NAT processing charges ($0.045/GB, separate from egress).
- **GCP network service tier**: Premium Tier (default, Google's global backbone) costs materially more for *internet* egress than Standard Tier (~$0.12/GiB vs ~$0.085/GiB in North America, with only 1 GiB/mo free on Premium vs. 200 GiB/mo on Standard) — relevant to client-facing produce/consume traffic from outside GCP, but **has no bearing on intra-region zone-crossing charges**, which are a separate pricing table.

### 5.3 Diskless architecture: eliminating the mechanism at its root

Rather than tuning around cross-AZ traffic, WarpStream/AutoMQ/Redpanda remove broker-to-broker replication from the write path entirely — this is the most important entry in this section for this project specifically:

- **Mechanism precisely stated**: writing to a *regional* object-storage bucket (S3 Standard / GCS Standard) avoids cross-AZ replication cost because the provider performs its own internal cross-AZ replication *inside* the storage service, not billed to the customer as network transfer — from the client's perspective there is no AZ-to-AZ hop at all (confirmed §1 table: compute↔regional storage is free from any AZ/zone on all three clouds). Multi-AZ durability is obtained "for free" (in the billing sense) as a property of the storage service, rather than something the application must construct and pay to replicate itself, as classic Kafka's ISR mechanism must.
- **WarpStream**: stateless Agents, no local disks, write directly to a regional S3 Standard bucket; service-discovery layer also zone-aligns clients (both Produce and Fetch) with same-AZ Agents, eliminating cross-AZ traffic on both write and read path by default. Claims ~100% elimination of the cross-AZ replication charge a broker-replicated cluster would incur. [WarpStream](https://www.warpstream.com/blog/kafka-is-dead-long-live-kafka)
- **AutoMQ**: keeps a broker-replica architecture superficially like Kafka, backed by S3. Producers route via consistent hashing to a broker in their *own* AZ regardless of which broker is the actual partition leader; that broker buffers/flushes to S3 (~8MB or 250ms trigger), then issues a small cross-AZ RPC notifying the real leader of the new object location — the leader reads it back from S3 (free, regional) and appends it logically. Only residual cross-AZ traffic is this small control-plane RPC, not bulk data — hence "nearly 100%" cross-AZ cost elimination. [AutoMQ](https://www.automq.com/blog/how-automq-reduces-nearly-100-of-kafka-cross-zone-data-transfer-cost)
- **Redpanda**: similarly bypasses inter-node replication in its object-storage-direct modes, claiming up to 90% cross-AZ/infra cost reduction.

### 5.4 GCP-specific note on Rapid Bucket zone-affinity

Because Rapid Bucket ties data to a **single zone at bucket-creation time** (not per-object), a Kafka-rack-awareness equivalent for a GCP-targeting design would need **one Rapid Bucket per zone** the system wants a fast-path presence in, with the partition-assignment/sequencer layer routing each partition's hot segments to the zone-colocated bucket and falling back to a shared regional Standard bucket for colder/compacted data — directly analogous to WarpStream's Agent Groups/zone-alignment pattern (§5.3), just applied to bucket selection instead of broker selection.

---

## 6. The S3 Express One Zone quorum-write pattern — and the cost nuance to get right

WarpStream and S2.dev both use S3 Express One Zone for their **low-latency ingestion tier**, needing latency below regional S3 Standard while reconstructing multi-AZ durability (since one Express bucket lives in one AZ only, per §2.4).

**Mechanism**: write each batch to **multiple Express One Zone buckets, each in a different AZ** — WarpStream documents a minimum of 3 buckets, acking once **≥2 of 3** succeed (majority quorum). Only after quorum is the producer acked. S2.dev's "Express" tier is similarly backed by a quorum of three S3 Express One Zone buckets. [AWS/WarpStream blog](https://aws.amazon.com/blogs/storage/how-warpstream-enables-cost-effective-low-latency-streaming-with-amazon-s3-express-one-zone/), [WarpStream docs](https://docs.warpstream.com/warpstream/kafka/advanced-agent-deployment-options/low-latency-clusters/s3-express)

**⚠️ The important nuance (a correction to a natural assumption)**: quorum-write traffic across AZ-pinned Express buckets is **not free or specially priced — it is billed as ordinary cross-AZ transfer**, stacking on top of Express One Zone's own request/storage pricing. There is no discounted "S3 Express quorum" transfer tier. WarpStream's own cost analysis: writing a GiB redundantly to two different-AZ Express buckets costs **~$0.016/GiB** in combined transfer charges — "suspiciously close to the cost of manually replicating a GiB between two AZs at the application layer ($0.02/GiB)." I.e., **quorum-writing across Express buckets costs roughly the same, per GiB replicated, as doing manual EC2-to-EC2 cross-AZ replication would.** [WarpStream: S3 Express is All You Need](https://www.warpstream.com/blog/s3-express-is-all-you-need)

Additional cost-stacking notes:
- Express One Zone's request-price discount (vs. S3 Standard) is largely "a wash" once multiplied by 3x the request volume from writing to 3 buckets instead of 1.
- Storage cost is **not** tripled in practice because both WarpStream and S2.dev treat Express One Zone as a thin, short-lived ingestion buffer — data is asynchronously compacted out to regular S3 Standard shortly after ingestion, so the ~5–8x-more-expensive Express storage rate applies only to a small rolling window, not the full retained log.
- Net latency payoff justifying the cost: WarpStream reports ~4x lower end-to-end p99 latency using the Express quorum tier vs. S3-Standard-only mode (from <1.5s p99 down to ~375ms p99, and as low as 33ms median with Lightning Topics — see [01-warpstream-architecture.md](01-warpstream-architecture.md)).

**Net implication for design**: the S3 Express quorum pattern is best understood as *trading one cross-AZ cost problem (broker replication) for a smaller, bounded version of the same problem (quorum-write replication across 2–3 Express buckets)*, in exchange for materially lower latency than pure regional-S3-Standard. It does **not** eliminate cross-AZ billing the way "write once to regional S3 Standard" does (§5.3) — it only shrinks and bounds it. Model it explicitly as `(replication factor − 1) × cross-AZ-rate × ingested-bytes` plus the Express storage/request premium for the transient buffer window, when comparing this pattern against a pure-regional-Standard design.

---

## 7. Gaps and unresolved questions flagged by this research

- **GCS Rapid regional pricing variance** — only `us-central1` figures were confirmed; GCS pricing pages render dynamically per region and other regions weren't fully retrievable via automated fetch. Re-verify for the target deployment region.
- **No official Google p50/p99 latency table for Rapid Bucket** — only qualitative "sub-millisecond" and throughput/QPS claims exist, unlike AWS's published single-digit-ms figures for Express One Zone.
- **Conflicting info on Rapid Bucket's S3-compatible API support** — Google's GA marketing blog claims S3-compatible API access; the product docs describe gRPC-only (`BidiWriteObject`) writes. Needs direct confirmation before depending on it.
- **No public head-to-head benchmark exists** comparing GCS Rapid Bucket to S3 Express One Zone on latency, throughput, or cost for an equivalent workload — any comparison in this doc's §1 table is a side-by-side of each vendor's own published numbers, not an independent apples-to-apples test.
- **No public statement found** from StreamNative Ursa or Aiven Inkless committing to use GCS Rapid Bucket for a fast write path (Ursa supports GCS generically; Inkless's "Diskless Express" tier naming is suggestive but untied to a specific GCS product in any source found) — this appears to be open design space, not a documented pattern.
- **No itemized GCS Rapid data-transfer ($/GB upload or retrieval) line item** was found analogous to S3 Express's separate upload/retrieval charges — may be folded into ops pricing or simply not yet broken out; confirm before finalizing a cost model.

---

## Sources

**S3 Express One Zone / AWS inter-AZ:**
- [AWS docs: High performance workloads (S3 Express One Zone)](https://docs.aws.amazon.com/AmazonS3/latest/userguide/directory-bucket-high-performance.html)
- [AWS docs: Optimizing S3 Express One Zone performance](https://docs.aws.amazon.com/AmazonS3/latest/userguide/s3-express-performance.html)
- [AWS blog: Up to 85% price reductions for S3 Express One Zone](https://aws.amazon.com/blogs/aws/up-to-85-price-reductions-for-amazon-s3-express-one-zone/)
- [AWS blog: New S3 Express One Zone launch announcement](https://aws.amazon.com/blogs/aws/new-amazon-s3-express-one-zone-high-performance-storage-class/)
- [AWS Storage Blog: How WarpStream enables cost-effective low-latency streaming with S3 Express One Zone](https://aws.amazon.com/blogs/storage/how-warpstream-enables-cost-effective-low-latency-streaming-with-amazon-s3-express-one-zone/)
- [AWS S3 SLA](https://aws.amazon.com/s3/sla/)
- [AWS Fault Injection Service: AZ Availability scenario](https://docs.aws.amazon.com/fis/latest/userguide/az-availability-scenario.html)
- [AWS: S3 Express One Zone resilience testing with FIS](https://aws.amazon.com/about-aws/whats-new/2025/08/amazon-s3-express-one-zone-supports-resilience-testing-aws-fault-injection-service)
- [AWS S3 Pricing](https://aws.amazon.com/s3/pricing/)
- [Confluent: Uncovering Kafka's Hidden Infrastructure Costs](https://www.confluent.io/blog/understanding-and-optimizing-your-kafka-costs-part-1-infrastructure/)
- [getkafkanated: How KIP-881 and KIP-392 reduce Inter-AZ Networking Costs](https://getkafkanated.substack.com/p/how-kip-881-and-kip-392-reduce-inter)
- [AutoMQ: How AutoMQ Reduces Nearly 100% of Kafka Cross-Zone Data Transfer Cost](https://www.automq.com/blog/how-automq-reduces-nearly-100-of-kafka-cross-zone-data-transfer-cost)
- [AutoMQ: Cross-AZ Replication Spend — Cost Drivers Kafka Teams Should Model](https://www.automq.com/blog/cross-az-replication-spend-cost-drivers-kafka-teams-should-model-before-scaling)
- [Redpanda: Minimizing your cloud bill](https://www.redpanda.com/blog/calculate-cloud-data-transfer-costs)
- [WarpStream: Kafka Is Dead, Long Live Kafka](https://www.warpstream.com/blog/kafka-is-dead-long-live-kafka)
- [WarpStream: S3 Express is All You Need](https://www.warpstream.com/blog/s3-express-is-all-you-need)
- [WarpStream docs: S3 Express (low-latency clusters)](https://docs.warpstream.com/warpstream/kafka/advanced-agent-deployment-options/low-latency-clusters/s3-express)
- [S2.dev: One weird trick to durably replicate your KV store](https://s2.dev/blog/kv-store)
- [Nixiesearch: Benchmarking read latency of AWS S3, S3 Express, EBS and Instance store](https://nixiesearch.substack.com/p/benchmarking-read-latency-of-aws)
- [Vantage: Unpacking S3 Express One Zone — Balancing Low Latency with Costs](https://www.vantage.sh/blog/amazon-s3-express-one-zone)
- [Sedai: Amazon S3 Express One Zone key insights](https://sedai.io/blog/getting-started-s3-express-one-zone)
- [CloudZero: AWS Data Transfer Costs](https://www.cloudzero.com/blog/reduce-data-transfer-costs/)
- [Vantage: Save by Using Anything Other Than a NAT Gateway](https://www.vantage.sh/blog/nat-gateway-vpc-endpoint-savings)
- [AWS re:Post: S3↔EC2 same-region transfer is free](https://repost.aws/questions/QUSK0wffM5ThKE5jESeM2kLQ/is-there-a-data-transfer-charge-when-downloading-data-from-s3-bucket-to-ec2-instance-in-the-same-region)

**GCS Rapid / GCP inter-zone:**
- [Rapid Bucket — Cloud Storage docs](https://docs.cloud.google.com/storage/docs/rapid/rapid-bucket)
- [Rapid Cache — Cloud Storage docs](https://docs.cloud.google.com/storage/docs/rapid/rapid-cache)
- [Create zonal buckets — Cloud Storage docs](https://docs.cloud.google.com/storage/docs/rapid/create-zonal-buckets)
- [Read and append to objects in zonal buckets](https://docs.cloud.google.com/storage/docs/rapid/use-objects-in-zonal-buckets)
- [Storage classes reference](https://docs.cloud.google.com/storage/docs/storage-classes)
- [Data availability and durability](https://docs.cloud.google.com/storage/docs/availability-durability)
- [Cloud Storage pricing](https://cloud.google.com/storage/pricing)
- [Cloud Storage Rapid turbocharges object storage for AI, analytics (GA blog, May 2026)](https://cloud.google.com/blog/products/storage-data-transfer/cloud-storage-rapid-turbocharges-object-storage-for-ai-analytics)
- [Next '26 storage announcements roundup](https://cloud.google.com/blog/products/storage-data-transfer/next26-storage-announcements)
- [How the Colossus stateful protocol benefits Rapid Storage](https://cloud.google.com/blog/products/storage-data-transfer/how-the-colossus-stateful-protocol-benefits-rapid-storage)
- [InfoQ: Google Cloud Announces Rapid Storage](https://www.infoq.com/news/2025/05/google-cloud-rapid-storage/)
- [GCP VPC network pricing](https://cloud.google.com/vpc/network-pricing)
- [EgressCost.com: GCP Premium vs Standard network tier](https://egresscost.com/gcp/premium-vs-standard/)
- [DataCenterDynamics: Microsoft removes Azure inter-AZ egress fees](https://www.datacenterdynamics.com/en/news/microsoft-removes-egress-fees-for-moving-data-between-availability-zones-in-same-azure-cloud-region/)
- [InfoQ: Azure AZ transfer fee removal coverage](https://www.infoq.com/news/2024/06/azure-az-transfer-fees/)
- [StreamNative: Announcing Ursa public preview on GCP](https://streamnative.io/blog/announcing-ursa-engine-preview-on-gcp)
- [Aiven: Announcing Inkless clusters](https://aiven.io/blog/announcing-inkless-clusters-cloud-kafka-done-right)
- [WarpStream S3 Express One Zone Benchmark and TCO (Medium)](https://medium.com/@warpstream/warpstream-s3-express-one-zone-benchmark-and-total-cost-of-ownership-458791998677)
