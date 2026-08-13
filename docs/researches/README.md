---
title: "Research Index: A Kafka-Compatible Streaming System on Object Storage"
slug: index
status: living
last_updated: 2026-08-13
tags: [index, navigation]
summary: >
  Navigation hub for this research corpus. Read this file first — it maps
  every document, its dependencies, and the current state of open decisions.
---

# Research: A Kafka-Compatible Streaming System on Object Storage

This is a local, LLM-first knowledge base researching how to build a **Kafka-compatible message broker/queue that uses S3/GCS-style object storage as its primary log storage** — the architecture pattern pioneered by [WarpStream](https://warpstream.com), and now also pursued by AutoMQ, Redpanda Cloud Topics, and Apache Kafka's own [KIP-1150 "Diskless Topics"](https://cwiki.apache.org/confluence/display/KAFKA/KIP-1150:+Diskless+Topics). The target project (working name in this repo: **oqueue**) is a **Rust**, **open source** implementation of this pattern.

**Purpose of this directory:** compile enough grounded, cited research that any LLM (or human) picking up this project — in this conversation or a future one — can get oriented quickly without re-researching from scratch, and can jump straight to the specific document that answers the question at hand.

## If you're an LLM navigating this for the first time

1. Read this file fully — it's short and tells you where everything is.
2. If you need general orientation before diving into a specific question, read in this order: [01](01-warpstream-architecture.md) → [04](04-object-storage-s3-gcs.md) → [06](06-distributed-systems-design-challenges.md) → [03](03-competitive-landscape.md). That's the reference architecture, the storage-layer physics that constrain it, the hard design problems, then the rest of the field for comparison.
3. If you have a specific question, use the **document map** below or the **tag index** at the bottom — don't re-read everything.
4. Check [10-open-questions.md](10-open-questions.md) before assuming a design decision has been made — most haven't yet; this corpus is mostly background research, not a finalized architecture. **The exceptions are [15](15-scale-architecture-position.md)** (scale architecture), **[21](21-ai-development-loop.md)** (the AI development loop), and **[22](22-encryption-byok-and-fips.md)** (encryption); all three are explicitly marked as positions rather than research, and each carries its own open questions.
5. Every claim in every doc carries an inline citation and each doc ends with a Sources list. Treat vendor blog numbers (latency, cost, "Nx cheaper") as vendor-reported, not independently verified, unless a doc says otherwise.
6. **Watch for ⚠️ CORRECTION banners** near the top of some documents. [16](16-automq-deep-dive.md) is a source-code study that invalidated several claims earlier docs took from AutoMQ's blog — the affected docs carry banners pointing at it. When code and marketing disagree, the code wins.

## Document map

| # | Document | What it answers | Depends on |
|---|---|---|---|
| 01 | [warpstream-architecture.md](01-warpstream-architecture.md) | How does the reference design (WarpStream) actually work end to end — Agents, metadata store, write/read path, cost model, licensing history? | — |
| 02 | [kafka-protocol-compatibility.md](02-kafka-protocol-compatibility.md) | What does a broker need to implement to speak real Kafka wire protocol — API keys, RecordBatch format, consumer groups, transactions, and Kafka's own KIP-405/1150/1163/1164/1165 diskless-topic work? | — |
| 03 | [competitive-landscape.md](03-competitive-landscape.md) | Who else is building this — Redpanda, AutoMQ, Bufstream, Confluent, Pulsar, StreamNative Ursa, S2.dev, Iggy, Fluvio, Tansu, Danube — and how do they compare? | 01, 02 |
| 04 | [object-storage-s3-gcs.md](04-object-storage-s3-gcs.md) | What are S3/GCS's actual consistency, latency, throughput, pricing, and conditional-write characteristics — the physics every design in this corpus is bounded by? | — |
| 05 | [rust-ecosystem.md](05-rust-ecosystem.md) | What Rust crates exist for each layer (object storage client, Kafka protocol, async runtime, metadata/consensus, caching, deterministic simulation testing)? | 04 |
| 06 | [distributed-systems-design-challenges.md](06-distributed-systems-design-challenges.md) | What are the actual hard design problems (offset sequencing without atomic append, batching vs. latency, caching on stateless nodes, compaction on immutable storage, exactly-once, multi-tenancy, failure modes) and how do real systems solve each? **Most directly actionable document in this corpus.** | 01, 03, 04 |
| 07 | [licensing-strategy.md](07-licensing-strategy.md) | What license should an open-source project like this use — survey of MongoDB/Elastic/Redis/CockroachDB/HashiCorp/Redpanda/AutoMQ precedent, BSL mechanics, AGPL tradeoffs. No recommendation is made; decision is the user's. | 03 |
| 08 | [turbopuffer-lessons.md](08-turbopuffer-lessons.md) | What can a *different* object-storage-native system (a search DB, not a queue) teach us — especially its CAS-only coordination model as an alternative to WarpStream's external metadata store? | 04 |
| 09 | [glossary.md](09-glossary.md) | What does this acronym/term mean? | — |
| 10 | [open-questions.md](10-open-questions.md) | What hasn't been decided yet, and what's blocking a given decision? **Living document — check here before assuming something is settled.** | all |
| 11 | [low-latency-tiers-and-interaz-costs.md](11-low-latency-tiers-and-interaz-costs.md) | Exact pricing/architecture for S3 Express One Zone and GCS Rapid Bucket; three-way AWS/GCP/Azure inter-AZ transfer cost comparison; why cross-AZ cost dominates classic Kafka; techniques to eliminate it (incl. the quorum-write cost nuance) | 04, 01 |
| 12 | [object-discovery-and-api-cost.md](12-object-discovery-and-api-cost.md) | Once writes bundle many partitions per object, how does a reader find its data? Why LIST is both semantically useless and 12–38x a GET; the offset→object index pattern used by every real system; why immutable objects make the reader-side metadata cache trivially consistent; the staleness hazards that remain; worked cost model (~354x). **Read with 06.** | 06, 04, 11 |
| 13 | [coordinator-recovery.md](13-coordinator-recovery.md) | What happens when the coordinator node dies? Two-layer durability model; three restart scenarios with different RTOs; snapshot-vs-compaction (KIP-630); locating a snapshot without enumeration (and the correction that KRaft doesn't); checkpoint cadence from KRaft/etcd/openraft/Delta/Flink; storage-engine selection (SlateDB/redb/RocksDB/fjall/SQLite); availability during failover; metastable failure modes to design against. | 12, 05, 06 |
| 14 | [metadata-scale-and-tiering.md](14-metadata-scale-and-tiering.md) | Can we reach turbopuffer-class catalog scale without a central coordinator? Verdict on the tiered design (coordinator for the hot tail, self-describing per-partition manifests for compacted history); why KIP-405 is weaker counter-evidence than it looks; the five frictions and their mitigations; what does and doesn't transfer from 250M namespaces; and the finding that the **Kafka protocol, not storage, binds first** — with client memory breaking before that. | 12, 13, 08, 02 |
| 15 | [scale-architecture-position.md](15-scale-architecture-position.md) | **⚠️ Not research — our working design position.** Target shape (1M–100M topics, tenant≈topic, ≤1000 partitions each); three-level structure; why metadata shards are internal and rebalanceable rather than client-visible virtual clusters; **principal-indexed metadata** as the load-bearing requirement; catalog sizing; co-located roles with no dedicated metadata tier. Lists what still needs verification. | 14, 12, 13 |
| 16 | [automq-deep-dive.md](16-automq-deep-dive.md) | **Source study, not blog-reading** — the only close comparable whose code we can read (Apache-2.0). Answers definitively why their partition ceiling is ~10⁵ (KRaft as metadata plane, per their own OOM issue). **Contains corrections to claims docs 03/06/11/12/13/14 previously cited from their marketing**, including that the EBS WAL no longer exists in shipping code. Extracts the transferable ideas: offset-aligned object naming, contiguity-of-keys as durability boundary, composite objects, miss-driven readahead. | 03, 12, 13, 14 |
| 17 | [latency-budget.md](17-latency-budget.md) | "How fast can this actually be?" Decomposes produce latency step by step and explains why tail reads are single-digit ms while produce cannot be. **Core finding: single-digit-ms produce and object-storage-only durability are mutually exclusive** — every system claiming otherwise uses a faster durable medium or weakens a guarantee. Cross-system comparison table, our four options with costs, and what to be skeptical of in vendor latency claims. | 16, 04, 11 |
| 18 | [rust-performance-methodology.md](18-rust-performance-methodology.md) | How do we measure, build, and optimize? Instruction-count benchmarking with **gungraun** (the renamed `iai-callgrind`) for noisy CI — and the hard limit that **Valgrind cannot execute AVX-512 or SVE and silently misreports CPUID**, making the benchmarking and SIMD work partly incompatible. Multi-crate build config (crate boundaries are optimization barriers; LTO is mandatory, not marginal), five-profile `Cargo.toml`, PGO/BOLT/allocator evidence. SIMD dispatch across x86/ARM, with **`crc32fast` being the wrong polynomial for Kafka** and "don't decode varints at all" as the real win. Unsafe policy grounded in a source study of Polars that **falsifies 2 of 5 claims in the widely-shared video**. | 05, 02 |
| 19 | [workspace-engineering.md](19-workspace-engineering.md) | How do we run a multi-crate workspace? Crate split for build parallelism (DAG **depth**, not crate count, sets the floor); enforcing unidirectional dependencies; contracts as traits in `core` with fakes beside them; sans-I/O as the constraint that makes isolated testing, determinism, and mutation-testability fall out together; the four test tiers; Linux/Mac/ARM portability (**ARM CI is a correctness gate for atomics, not a nicety**); eliminating flaky tests; and making mutation testing cost **O(change) not O(codebase)**. Stated as this project's engineering standard rather than as a survey. | 18, 05, 15 |
| 20 | [build-and-release-portability.md](20-build-and-release-portability.md) | Can someone *build* it, and will the artifact *run* where it's deployed? The host-toolchain tax; the **glibc floor** as the real Linux distribution problem (and `cargo-zigbuild`'s target-suffix fix); why **musl's default allocator costs 10–40x under concurrency**, promoting doc 18's allocator choice from optimization to mandatory; why free arm64 runners make native builds beat cross-compilation; macOS as a dev platform rather than a release target. **Resolves open question #29 (`target-cpu` baseline).** | 19, 18 |
| 21 | [ai-development-loop.md](21-ai-development-loop.md) | **⚠️ Not research — a design position.** How to run an unattended, fully AI-authored loop where no human reads the code. Two gaps in the inherited system: the reviewer is the agent that wrote the code, and nothing enforces that review happened. Fixes both — a **context-isolated reviewer subagent** that never sees the author's reasoning, and a **review artifact keyed to the staged diff hash** so a pre-commit gate can require it, making review tamper-evident rather than claimed. Plus the allocation rule (anything a script can decide is never an agent's job; the reviewer's budget goes only where scripts can't reach), a taxonomy of AI slop mapped to the gate that catches each, and the promotion of **mutation testing to the primary anti-slop gate**. | 19, 18 |
| 22 | [encryption-byok-and-fips.md](22-encryption-byok-and-fips.md) | **⚠️ A design position.** Per-topic BYOK portable across AWS/GCP KMS, and FIPS 140-3 as a separate build. **SSE-KMS cannot satisfy the requirement** — one object carries many tenants, so an object cannot carry one key; broker-side envelope encryption is forced. Per-topic keys collide with multi-topic batching, resolved by sealing each region inside the object independently. ⚠️ **AWS KMS caps customer-managed keys at 100,000 per region**, so "per topic" must mean per-topic *configuration*, not per-topic KMS key. The portable seam is **wrap/unwrap, not generate-data-key**, because GCP has no `GenerateDataKey`. KMS must never sit on the per-batch path. | 15, 12, 20 |

## The one-paragraph synthesis

Every object-storage-primary streaming system solves the same core problem — object storage has no atomic-append or low-latency-CAS primitive suitable for assigning a partition's next offset directly at Kafka-classic speed (~50–200ms PUT latency vs. Kafka's ~5–20ms ack) — via one of three patterns: an **external strongly-consistent metadata store** that decides ordering after the fact (WarpStream, KIP-1150's "Batch Coordinator"), **object-storage-native conditional writes** used directly as a CAS primitive for low-frequency coordination (turbopuffer's "no Raft, no Paxos" approach — plausible for compaction-job claims and leader election, unproven at Kafka's offset-assignment throughput), or **keeping consensus local and only offloading bulk bytes** (Redpanda Cloud Topics, AutoMQ's WAL-fronted design), which preserves classic Kafka latency/EOS guarantees at the cost of reintroducing broker-affinity and local disk state. Every design also converges on **batching many producers/partitions into one large object per flush** (WarpStream's multi-tenant files, turbopuffer's group commit, AutoMQ's Stream Set Objects) because object-storage PUT request pricing and latency both favor fewer, larger writes over many small ones. See [06-distributed-systems-design-challenges.md](06-distributed-systems-design-challenges.md) for the full treatment.

## Tag index

- **Architecture/reference design:** [01](01-warpstream-architecture.md), [03](03-competitive-landscape.md), [06](06-distributed-systems-design-challenges.md)
- **Kafka protocol specifics:** [02](02-kafka-protocol-compatibility.md)
- **Object storage physics (S3/GCS):** [04](04-object-storage-s3-gcs.md)
- **Rust implementation / crates:** [05](05-rust-ecosystem.md)
- **Rust performance: benchmarking, build config, SIMD, unsafe:** [18](18-rust-performance-methodology.md)
- **Benchmarking methodology (instruction counts, gungraun, CI noise):** [18](18-rust-performance-methodology.md) §1–2
- **Build/codegen (LTO, PGO, profiles, target-cpu, allocator):** [18](18-rust-performance-methodology.md) §3
- **Build hygiene (target/ disk growth, cargo-sweep, sccache, test scratch, compile time):** [18](18-rust-performance-methodology.md) §3.7
- **SIMD / CRC-32C / cross-arch dispatch:** [18](18-rust-performance-methodology.md) §4
- **Unsafe policy, Miri/fuzzing/sanitizers:** [18](18-rust-performance-methodology.md) §5
- **Metadata/coordination/consensus:** [06](06-distributed-systems-design-challenges.md) §1, [08](08-turbopuffer-lessons.md) §4, [04](04-object-storage-s3-gcs.md) §6 (conditional writes), [05](05-rust-ecosystem.md) §4 (Rust crates: openraft/etcd/FoundationDB), [12](12-object-discovery-and-api-cost.md) §3–4 (the offset→object index and its cache)
- **Read path / object discovery / caching:** [12](12-object-discovery-and-api-cost.md), [06](06-distributed-systems-design-challenges.md) §3
- **Tail reads (the hot path) — push vs sync fan-out, soft affinity:** [12](12-object-discovery-and-api-cost.md) §4.5
- **Metadata cache consistency & staleness hazards:** [12](12-object-discovery-and-api-cost.md) §4 (hazard table in §4.6)
- **Coordinator durability, restart, recovery, snapshots:** [13](13-coordinator-recovery.md)
- **Scale: partition counts, tiered metadata, deterministic layout:** [14](14-metadata-scale-and-tiering.md)
- **Kafka protocol ceilings / client limits:** [14](14-metadata-scale-and-tiering.md) §10, [02](02-kafka-protocol-compatibility.md)
- **Multi-tenancy (Virtual Clusters, topic-per-tenant):** [14](14-metadata-scale-and-tiering.md) §10.4, [06](06-distributed-systems-design-challenges.md) §7
- **Embedded storage engine choice (materialized index):** [13](13-coordinator-recovery.md) §6, [05](05-rust-ecosystem.md) §4
- **Availability / failure modes / metastability:** [13](13-coordinator-recovery.md) §7–8, [06](06-distributed-systems-design-challenges.md) §8
- **Compaction:** [12](12-object-discovery-and-api-cost.md) §6 (break-even model, index sizing), [06](06-distributed-systems-design-challenges.md) §4
- **Cost/pricing:** [04](04-object-storage-s3-gcs.md) §4, [11](11-low-latency-tiers-and-interaz-costs.md) (low-latency tiers + inter-AZ networking), [12](12-object-discovery-and-api-cost.md) §1, §7 (API request cost, LIST vs GET), [01](01-warpstream-architecture.md), [08](08-turbopuffer-lessons.md) §6
- **Latency (what's achievable, produce vs tail-read):** [17](17-latency-budget.md)
- **Low-latency storage tiers (S3 Express / GCS Rapid):** [11](11-low-latency-tiers-and-interaz-costs.md), [04](04-object-storage-s3-gcs.md) §2
- **Inter-AZ / cross-zone networking cost:** [11](11-low-latency-tiers-and-interaz-costs.md) §4–6
- **Encryption / BYOK / KMS / FIPS:** [22](22-encryption-byok-and-fips.md)
- **Compliance builds (FIPS 140-3, aws-lc-rs):** [22](22-encryption-byok-and-fips.md) §7, [20](20-build-and-release-portability.md) §1
- **Licensing/business:** [07](07-licensing-strategy.md)
- **Testing/correctness:** [05](05-rust-ecosystem.md) §9 (deterministic simulation testing), [18](18-rust-performance-methodology.md) §5.6 (Miri, cargo-careful, sanitizers, fuzzing — with a realistic CI budget), [19](19-workspace-engineering.md) §4, §8–9 (test tiers, flake elimination, mutation testing)
- **Workspace/crate structure, layering, contracts:** [19](19-workspace-engineering.md) §1–3
- **AI development loop, review separation, anti-slop:** [21](21-ai-development-loop.md)
- **Deterministic gate vs. agent judgment — the allocation rule:** [21](21-ai-development-loop.md) §3
- **Loop cycle time, gate budgets, build/test speed discipline:** [21](21-ai-development-loop.md) §9
- **Flaky tests / determinism / sans-I/O / DST:** [19](19-workspace-engineering.md) §8
- **Mutation testing at scale:** [19](19-workspace-engineering.md) §9
- **Cross-OS / cross-arch TEST portability and CI matrix:** [19](19-workspace-engineering.md) §7
- **BUILD portability: cross-compilation, glibc floor, musl, packaging:** [20](20-build-and-release-portability.md)
- **Concurrency correctness (loom vs TSan vs ARM CI):** [19](19-workspace-engineering.md) §7.2
- **KIP-1150 / Kafka's own diskless-topics work:** [02](02-kafka-protocol-compatibility.md) §5, [03](03-competitive-landscape.md) §2.11, [06](06-distributed-systems-design-challenges.md) (throughout)

## Status

This is **background/foundational research**, compiled before project-specific requirements were shared. It covers the general problem space thoroughly; it does not yet contain a requirements doc, an architecture-decisions doc, or a roadmap — those come next, once the user shares more context (per their stated intent to do so progressively). See [10-open-questions.md](10-open-questions.md) for exactly what's still missing and what decisions this research sets up but doesn't make.

Documents 01–10 were compiled 2026-08-12; documents 11–22 on 2026-08-13, via extensive web research (WebSearch/WebFetch against primary sources — vendor engineering blogs, official docs, KIP wiki pages, crates.io/GitHub, Hacker News threads with technical depth). Figures and claims are cited inline; treat anything without a citation as this corpus's own synthesis, not a sourced fact. Docs 11 and 12 additionally mark each section **[Documented]** or **[Synthesis]** so cost models and design recommendations are never mistaken for published figures.

**Pricing caveat:** docs 11 and 12 contain a lot of specific per-GB and per-request prices, current as of August 2026. Cloud pricing moves (S3 Express One Zone was cut up to 85% in April 2025 alone). Re-verify against live pricing pages before using any of these numbers in a real cost model or public benchmark.
