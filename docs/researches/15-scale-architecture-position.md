---
title: "Working Position: Scale Architecture for a Very Large Topic Catalog"
slug: scale-architecture-position
status: working-position
last_updated: 2026-08-13
tags: [architecture, position, scale, metadata-sharding, principal-indexed-metadata, tenancy, deployment, co-located-roles]
related: [14-metadata-scale-and-tiering, 12-object-discovery-and-api-cost, 13-coordinator-recovery, 08-turbopuffer-lessons, 10-open-questions]
summary: >
  First document in this corpus stating OUR design rather than surveying
  others. Captures the working position reached for a 1M–100M topic catalog:
  a three-level structure (metadata shard / topic-as-tenant / partition),
  internal-and-rebalanceable metadata shards rather than client-visible
  virtual clusters, principal-indexed metadata as the load-bearing protocol
  requirement, and co-located roles with no dedicated metadata tier.
  Derived from research in 12–14; several claims still need verification.
---

# Working Position: Scale Architecture for a Very Large Topic Catalog

**⚠️ This document is different in kind from 01–14.** Those are research: they survey what others have built, with citations, and deliberately avoid prescribing. This one states **our current working position** — choices made, with reasoning, derived from that research. It is a position, not a decision: nothing here has been implemented or validated, several figures are derived rather than measured, and §8 lists what still needs verifying before any of it should be treated as settled.

*Reached through design discussion on 2026-08-13, grounded in [12](12-object-discovery-and-api-cost.md), [13](13-coordinator-recovery.md), and [14](14-metadata-scale-and-tiering.md).*

---

## 1. The target shape

This is the input that had been missing, and it resolves a great deal. Open question #16 asked for it explicitly.

| Dimension | Target |
|---|---|
| **Tenancy model** | **One tenant ≈ one topic** |
| Topic count | **1M – 100M** |
| Partitions per topic | **≤1000 (max)**; median expected to be far lower |
| Active fraction | Unknown — assumed turbopuffer-shaped (most idle) |
| Aggregate throughput | Not yet specified |

The two numbers still needed are the **active fraction** and the **median vs. max partitions per topic**. Both materially affect sizing; neither is guessed at below.

**Why this shape is favorable:** it is turbopuffer's shape — enormous catalog, small per-unit footprint, overwhelmingly idle — which means more of their mechanisms transfer than the earlier analysis in [14](14-metadata-scale-and-tiering.md) §9 assumed for a large *active* working set.

---

## 2. Three levels, only two client-visible

| Level | Count | Purpose | Client-visible | Rebalanceable |
|---|---|---|---|---|
| **Metadata shard** | ~10s–100s | Catalog + coordinator sharding | **No** | **Yes** |
| **Topic** (= tenant) | 1M–100M | The logical unit | Yes | — |
| **Partition** | ≤1000/topic | | Yes | — |

The middle level must be **cheap to the point of being free**: a prefix, a manifest, and a catalog row. No dedicated coordinator, no Raft group, no thread, no timer. If creating a topic provisions anything, 100M is unreachable. Existence should be a derived property — hash the identity, and the topic exists because an object exists at the derived key. That is turbopuffer's core property ([08](08-turbopuffer-lessons.md) §1, [14](14-metadata-scale-and-tiering.md) §9.1) and it is what makes the catalog size cost-free to address.

---

## 3. Metadata shards are internal, not client-visible virtual clusters

An earlier iteration of this position had the shard boundary being client-visible, WarpStream-Virtual-Cluster style. **That was rejected**, and the reasoning is worth recording because the two options look similar and behave very differently.

| | Client-visible VC | Internal metadata shard *(chosen)* |
|---|---|---|
| Scopes the `Metadata` response | Yes, structurally | **No** — needs another mechanism (§4) |
| Moving a topic between them | Data migration + offset remap + client reconfig — **a one-way door** | **Routing change**, absorbed by normal metadata refresh |
| Granularity decision | Near-permanent, must be right up front | **Revisable** — start coarse, split later |
| Interaction with write batching | Fine granularity destroys PUT amortization | **None** — batching happens at the shard level across many topics |

The rebalanceability is decisive. It converts the shard-count decision from an up-front commitment into an operational knob, and it removes the tension between metadata isolation and write batching entirely: batching happens across topics *within* a shard, so shards can be coarse for cost while topics stay fine-grained for isolation.

**What we give up** by making shards internal is the free `Metadata`-scoping that a client-visible boundary would have provided. That has to be recovered another way, which is §4 and is the single most important requirement in this document.

---

## 4. Principal-indexed metadata — the load-bearing requirement

With internal shards, a client issuing `Metadata` with a null topic array is nominally asking about every topic in the system. At 100M topics that is fatal.

**The mechanism that saves it:** Kafka already filters `Metadata` responses by topic authorization — a principal without `DESCRIBE` on a topic does not see it. With tenant credentials scoped to their own topic, the *response* is naturally one topic no matter how many exist.

**The trap:** Kafka's own implementation holds all topics in a metadata cache and filters per request, i.e. **O(all topics) per call**. The response is small; the work is not. At 100M topics that is fatal in CPU rather than in bandwidth.

> **Requirement: metadata must be indexed by principal, not filtered from a global list.** A `Metadata` request must cost O(topics this principal can see), never O(topics that exist).

This is a genuine departure from how Kafka is built internally, it is invisible at the protocol level, and it is the thing that makes the whole shape work. It should be treated as a first-class architectural constraint, not an optimization.

Consequence: the `hasPatternSubscription() → allTopics()` pathology from [14](14-metadata-scale-and-tiering.md) §10.2 stops being dangerous. Scoped to one tenant's ≤1000 partitions, a full metadata response is ~42 KB — trivial even at a 60s refresh interval.

### Residual: cross-tenant views

Anything needing a view across tenants must **not** go through the normal `Metadata` path:

- **Billing / metering / reconciliation** — certainly exists. Offline batch sweep, turbopuffer-style ([14](14-metadata-scale-and-tiering.md) §9.2e). Design it as a batch job from day one.
- **Admin tooling** — `kafka-topics.sh --list` against 100M topics is a foot-gun. Needs a deliberate answer (pagination, prefix scoping, or refusal) before someone finds it.
- **Cross-tenant consumers / regex spanning tenants** — should be an explicit, privileged, rate-limited path if supported at all.

---

## 5. Catalog sizing

*(Derived, not measured. Assumes ~10 partitions per topic average — sensitive to the median that is still unknown.)*

| Component | Estimate |
|---|---|
| Topic entries (name + UUID + config + partition count) | 100M × ~100 B ≈ **10 GB** |
| Partition entries | 1B × ~40 B ≈ **40 GB** |
| **Total catalog** | **~50 GB** |

At RF 3 that is ~150 GB of replicated state. Sharded across ~100 logical shards it is ~500 MB per shard — comfortable, and disk-backed rather than resident with the storage engines surveyed in [13](13-coordinator-recovery.md) §6.

**Keep this distinct from the read-path index.** The catalog is *what exists* — small, slow-changing, and unavoidably O(topics). The read-path index is *where the bytes are* — large, fast-changing, and bounded to the uncompacted window by the tiered design in [14](14-metadata-scale-and-tiering.md). Conflating them makes the problem look far worse than it is.

---

## 6. Deployment: co-located roles, no dedicated metadata tier

**~100 shards does not mean ~100 nodes.** Shards are logical; nodes host many.

The resource cost of metadata is genuinely small relative to the data plane:

| Dimension | Figure |
|---|---|
| Metadata volume vs. data volume | ~6 KiB per 16 MiB ≈ **0.04%** (KIP-1164) |
| Coordinator commit rate | agents × flush rate ≈ **400/s** at 100 agents |
| State per node, 150 GB across ~20 nodes | **~7.5 GB**, mostly on disk |

**Precedent:** KIP-1164 runs Batch Coordinator instances *on brokers*, not on a separate tier. Kafka's KRaft combined mode does the same. WarpStream's separate hosted control plane is the outlier, and it is a **business-model choice** — it is how they sell BYOC while keeping the control plane proprietary. We have no reason to copy it, and copying it would reintroduce exactly the external-dependency adoption tax that argues against requiring FoundationDB or TiDB.

### Two node classes

| Class | Metadata shards? | Property |
|---|---|---|
| **Metadata-bearing** | A few each | Durable local state, snapshots, replication; slower to churn |
| **Pure data plane** | None | **Fully stateless** — scales freely, spot-safe, instant add/remove |

Rough shape: 3–9 metadata-bearing nodes (RF 3 across a few shard groups) against however many stateless nodes throughput demands.

The reason to keep the split rather than put a shard on every node: **it preserves the stateless-data-plane property for the majority of the fleet**, which is what makes autoscaling and spot instances work — one of the primary cost wins of the diskless architecture. If every node carries durable state, every node becomes slow to add and remove.

The two tiers also scale on different curves — metadata with *tenant count*, data plane with *throughput* — which is why they should be separable even while co-located.

### The discipline that keeps separation available

Co-locate the deployment, but keep the metadata role a **clean logical component with its own resource budget**: own task pool, own memory ceiling, own metrics. Then splitting it out later is a config change rather than a rearchitecture.

The failure mode to avoid: interleaving metadata work into the data-plane hot path with shared mutable state. Do that and separation becomes impossible — and you discover it exactly when a hot tenant's fetch traffic starts starving offset assignment.

**Split when** metadata state outgrows what nodes can hold alongside their read caches, or coordinator p99 begins tracking data-plane load, or independent blast radius is wanted. None of those bite at the numbers above.

**Deployment story:** single binary, roles enabled by config, co-located by default. `docker run oqueue` works with no separate tier and no external database — which is also the better OSS adoption story.

---

## 7. What this resolves

| Open question | Resolution |
|---|---|
| **#16** Active working set / scale target | Shape stated in §1. Two sub-numbers still needed. |
| **#8** Index granularity | Coarse coordinator index + in-object footer index — forced by the state arithmetic in [14](14-metadata-scale-and-tiering.md) §3. |
| **#19** Metadata-namespace sharding as tenancy unit | **Revised.** Shards are internal and rebalanceable, not client-visible VCs (§3). |
| **#20** Client protocol posture | **Reframed.** The answer is principal-indexed metadata (§4), not throttling `allTopics()`. |
| Coordinator deployment | Co-located roles, no dedicated tier (§6). |

Carried forward unchanged from [14](14-metadata-scale-and-tiering.md): shared WAL remains mandatory; the tiered read-path index (coordinator for the hot tail, self-describing manifests for compacted history) still stands; idle partitions still cost more in Kafka than in turbopuffer because retention is time-driven and attached-but-idle consumers still poll.

---

## 8. What needs verification before this is settled

Listed plainly, because this document makes claims that research has not yet confirmed:

1. ~~**Kafka's `Metadata` authorization-filtering behavior.**~~ — **verified 2026-09-02, `M9.1`, against `apache/kafka` trunk's `KafkaApis.handleTopicMetadataRequest`.** Both claims hold. **Protocol-level**: for the all-topics case (`isAllTopics`, the null/empty topic array), `unauthorizedForDescribeTopicMetadata` is built as `Set.empty` and the code comment says why — "do not disclose the existence of topics unauthorized for Describe, so we've not even checked if they exist or not." A principal genuinely does not see a topic it cannot `DESCRIBE`, exactly as asserted. **Implementation-level**: `val topics = if (metadataRequest.isAllTopics) metadataCache.getAllTopics.asScala` enumerates every topic in the cache first, then `authHelper.filterByAuthorized(...)` runs the `DESCRIBE` check across that whole set — the response is small, the work is O(all topics), exactly the trap this document names. ⚠️ **One nuance the two-line claim did not carry**: this is the *null-topic-array* behavior specifically. An **explicitly-named** topic list gets a different answer — an unauthorized topic in that list is not omitted but returned with `Errors.TOPIC_AUTHORIZATION_FAILED` (the client already named it, so there is nothing left to hide), which is the shape `M9.10`'s own null-topic-array task and `M9.13`'s cross-principal test need to distinguish, not conflate.
2. **Whether WarpStream agents can serve multiple Virtual Clusters, and whether VCs share a bucket.** Not found in their public docs. Would inform whether the shared-WAL-across-shards approach has precedent.
3. **Median vs. max partitions per topic**, and the **active fraction**. §5's sizing is sensitive to both.
4. **Whether ≤1000 partitions is a hard product ceiling or a soft expectation.** librdkafka at 1000 partitions with default `queued.max.messages.kbytes` (64 MB *per partition*) has a 64 GB nominal budget — the one place the top of the range hits a real client-side wall. Needs opinionated client defaults documented.
5. **Aggregate throughput target** — still unspecified, and it determines shared-WAL sizing and whether the S3-Express fast path from [11](11-low-latency-tiers-and-interaz-costs.md) is warranted.
6. **The Redpanda Cloud Topics question** from [14](14-metadata-scale-and-tiering.md) §12 — why they chose an LSM metastore for compacted L1 despite owning derivable manifests. Still the one piece of evidence that someone well-positioned went the other way.

---

## 9. Open design threads not yet addressed

- **Bootstrap and tenant routing.** With 1M–100M tenants you cannot enumerate endpoints in a load balancer. Working hypothesis: shared bootstrap endpoint, tenant identified by SASL credentials, `Metadata` scoped to the authenticated principal — no protocol extension needed, and the tenancy boundary falls out of authentication rather than addressing. Not yet researched.
- **Retention on idle partitions at 100M-topic scale.** Time-driven retention must fire on partitions nobody is writing to. Lazy evaluation (on next access) or folding into the daily reconciliation sweep are the candidates; neither is worked through.
- **Consumer-group state at this tenancy shape.** One group per tenant is the natural pattern, but `__consumer_offsets` equivalents at 100M groups need their own sizing.
- **Topic deletion and reuse.** turbopuffer allows immediate name reuse after delete because existence is just object presence. Kafka clients have expectations about topic identity (topic IDs, KIP-516) that may complicate this.
