---
title: "Competitive Landscape: Streaming Systems Built on Object Storage"
slug: competitive-landscape
status: draft
last_updated: 2026-08-12
tags: [competitive-analysis, redpanda, automq, bufstream, pulsar, streamnative, s2dev, iggy, fluvio, tansu, danube, kip-1150]
related: [01-warpstream-architecture, 02-kafka-protocol-compatibility, 04-object-storage-s3-gcs, 06-distributed-systems-design-challenges]
summary: >
  Survey of every notable object-storage-native or tiered-storage streaming
  system as of Aug 2026 (Redpanda, AutoMQ, Bufstream, Confluent Kora/Freight/
  WarpStream, Pulsar, StreamNative Ursa, S2.dev, Iggy, Fluvio, Tansu, Danube,
  KIP-1150/Inkless), with a comparison table and implications for a new
  Rust-native entrant.
---

# Competitive Landscape: Streaming Systems Built on Object Storage

> **⚠️ CORRECTION (2026-08-13) — AutoMQ claims in this document.** Statements here about AutoMQ's **EBS-backed WAL** (sub-10ms produce latency) and **EBS Multi-Attach failover** ("within milliseconds") were taken from AutoMQ's blog and **do not match their shipping code**. A source study at commit `eccde72` found the block-device WAL removed entirely (`BlockWALService`, `SlidingWindowService`, `WALBlockDeviceChannel` — zero hits; the WAL factory throws on any protocol but S3). Failover is now "a healthy broker re-uploads the dead broker's WAL objects from shared storage", with a **1-minute minimum grace period** and **one failover at a time cluster-wide**. Per their 1.5.0 notes, EBS support moved to their Enterprise Edition. See [16-automq-deep-dive.md](16-automq-deep-dive.md) §2 for the full correction list.


*Research brief for the design of a new Rust-based, open-source, Kafka-compatible streaming system built on object storage (WarpStream-style "diskless" architecture)*

*Prepared: 2026-08-12*

> **How this fits the project:** this is the "who else is doing this and how" doc. Read [01-warpstream-architecture.md](01-warpstream-architecture.md) first for the reference design, then this file for the rest of the field. [10-open-questions.md](10-open-questions.md) tracks where we still need to differentiate.

---

## 1. Executive Summary

Since roughly 2023, "diskless" or "object-storage-native" architectures have moved from a niche idea (WarpStream, 2023) to a mainstream direction endorsed by the Apache Kafka project itself via **[KIP-1150 "Diskless Topics"](https://cwiki.apache.org/confluence/display/KAFKA/KIP-1150:+Diskless+Topics)**, accepted March 2026. The field now spans a spectrum:

- **Disk-primary with tiered offload** (Redpanda core, classic Apache Pulsar/BookKeeper) — object storage is a cold-tier archive, not the primary write path.
- **Hybrid WAL + object storage** (AutoMQ, S2.dev's channel design, StreamNative Ursa's optional BookKeeper path) — a small, fast durable buffer (local disk, EBS, or a batching WAL) absorbs writes before async/near-real-time flush to object storage.
- **Object-storage-primary / fully diskless** (WarpStream, Bufstream, Confluent Freight Clusters, StreamNative Ursa's cost-optimized mode, Tansu, Aiven's Inkless reference implementation of KIP-1150) — new data is durably committed to object storage on (or near) the write path, with no persistent local broker disk and no per-partition Raft/BookKeeper quorum.
- **Non-Kafka log primitives** (S2.dev) — object-storage-native but deliberately *not* Kafka-protocol compatible, treating "the log" as a first-class cloud primitive rather than rebuilding a Kafka-shaped broker.
- **Rust-native prior art that is *not* (yet) object-storage-based** (Iggy, Fluvio) — valuable for engine/runtime/protocol design lessons, but still local-disk-first.

The consistent architectural trade-off across nearly every object-storage-primary design is **cost and operational simplicity vs. tail latency**: eliminating cross-AZ replication and local disks routinely cuts infrastructure cost 80–95%, but produce/consume p99 latency moves from single-digit-to-tens of milliseconds (Raft/BookKeeper-quorum systems) to hundreds of milliseconds to low seconds (pure object-storage-primary systems), unless a fast local/attached-disk WAL is reintroduced (AutoMQ's EBS WAL, S2's "Express" tier) — which then reintroduces some of the statefulness diskless designs try to avoid.

---

## 2. System-by-System Analysis

### 2.1 Redpanda (core engine + Tiered Storage + Cloud Topics)

**What it is.** A from-scratch (not a Kafka fork), Kafka-API-compatible streaming engine built in C++ on the Seastar framework (the same async runtime underlying ScyllaDB), designed to avoid ZooKeeper/JVM overhead and exploit modern NVMe hardware ([Redpanda architecture docs](https://docs.redpanda.com/current/get-started/architecture/)).

**Core storage architecture (disk-primary).** Redpanda uses a **thread-per-core / shard-per-core** execution model — each CPU core owns its own I/O queues, memory, and Raft groups, avoiding locks and context switches. Every topic partition is its own **Raft consensus group**; the leader acknowledges a write only after a majority of replicas persist it to **local NVMe SSD**. Metadata and Raft state also live on local disk, not an external coordination service ([Redpanda architecture](https://docs.redpanda.com/current/get-started/architecture/)).

**Tiered Storage (a.k.a. Shadow Indexing).** This is a cold-tier *offload*, not primary storage: a `scheduler_service` on the partition leader periodically uploads sealed log segments to S3/GCS/Azure Blob/ADLS, throttled by a PID regulator and using randomized/hashed object-key prefixes to avoid S3 request-rate throttling. Every upload is itself recorded as a Raft configuration batch, so archival state replicates through the same Raft log used for data — any replica can resume uploads on failover. Reads for local/hot data are served from NVMe; reads for archived data require a fetch-and-cache round trip to object storage, so cold-read latency is materially higher ([Redpanda: tiered storage deep dive](https://www.redpanda.com/blog/tiered-storage-architecture-shadow-indexing-deep-dive)). Tiered Storage requires an **enterprise license** ([Redpanda docs: tiered storage](https://docs.redpanda.com/current/manage/tiered-storage/)).

**Cloud Topics / "Redpanda One" (2024–2026): moving toward object-storage-primary, but deliberately not "diskless."** Starting with an announcement in September 2024 ([Redpanda: Cloud Topics](https://www.redpanda.com/data-streaming/cloud-topics-write-to-object-storage)) and continuing through the **Iceberg Topics GA (Redpanda 25.1, April 2025)**, **25.2 (Aug 2025)**, and **Redpanda 26.1 "R1 Vision" (March 2026)** releases, Redpanda introduced Cloud Topics: a write-pass-through mode where message payloads go directly to object storage rather than local disk first. Redpanda explicitly frames this as *not* fully diskless: **Raft consensus and partition metadata deliberately remain on local NVMe** ("the brains of the partition"), because Redpanda's stated position is that moving ordering/consensus off-box to an external metadata service (as WarpStream/Bufstream-style designs do) creates dependency and ordering-guarantee risk. Metadata operations stay sub-10ms because they never leave the local Raft-replicated control plane; the trade-off is accepted only on the bulk-data path ([Redpanda: Cloud Topics](https://www.redpanda.com/data-streaming/cloud-topics-write-to-object-storage)).

**Consistency/latency.** Raft quorum (majority-ack) gives strong per-partition ordering and Kafka-compatible durability guarantees. Benchmarks cite roughly 6–8ms p99 publish latency on NVMe with `acks=all` ([Redpanda: what makes Redpanda fast](https://www.redpanda.com/blog/what-makes-redpanda-fast); [independent benchmark discussion](https://jack-vanlightly.com/blog/2023/5/15/kafka-vs-redpanda-performance-do-the-claims-add-up)).

**License.** **Business Source License (BSL)** — free, source-available, but forbids offering Redpanda as a competing hosted multi-tenant streaming service; converts to Apache 2.0 automatically after a fixed period (currently 4 years). Enterprise features (including Tiered Storage) require a paid license key ([Redpanda: BSL license post](https://www.redpanda.com/blog/bsl-source-available-license)).

**Differentiation from WarpStream-style diskless designs.** Redpanda retains a Raft-elected leader as the single ordering authority for every partition and keeps consensus state on local disk even in its most object-storage-forward mode; WarpStream-style designs eliminate per-partition consensus entirely, using a stateless metadata/control-plane service to assign offsets and treating every broker as symmetric/interchangeable.

---

### 2.2 AutoMQ

**What it is.** A **Java** fork of the Apache Kafka codebase — it reuses Kafka's compute layer (broker request handling, group coordination, Connect, Streams, wire protocol) and reimplements only the storage layer (`UnifiedLog`, `LocalLog`, `LogSegment`) to target object storage instead of local disk. Marketed as "Diskless Kafka® on S3" ([AutoMQ GitHub](https://github.com/automq/automq); [AutoMQ wiki: introducing AutoMQ](https://github.com/AutoMQ/automq/wiki/Introducing-AutoMQ:-a-cloud-native-replacement-of-Apache-Kafka)).

**WAL design.** AutoMQ's core storage abstraction, **S3Stream**, pairs a **write-ahead log (WAL)** module with an object-storage module, because raw S3/object storage has too much per-request latency and too little IOPS for Kafka's synchronous produce-ack path ([AutoMQ: S3 WAL integration](https://www.automq.com/blog/automq-s3-wal-integration); [AutoMQ architecture overview](https://docs.automq.com/automq/architecture/overview)):
- **Open-source edition**: WAL is **S3-only** — no persistent local/attached disk at all, giving a fully stateless broker, but with looser produce latency (one cited benchmark: ~170ms average / ~346ms p99).
- **Commercial/BYOC editions**: support a **small EBS volume (commonly ~10GB) as WAL** (also EFS/FSx options), achieving single-digit-millisecond p99 produce latency. On broker failure, AutoMQ uses **EBS Multi-Attach** to reattach the WAL volume to a healthy broker within milliseconds, substituting for Raft-driven failover ([AutoMQ wiki: WAL storage](https://github.com/AutoMQ/automq/wiki/WAL-Storage); [AWS blog: sub-10ms with AutoMQ + FSx](https://aws.amazon.com/blogs/storage/achieving-sub-10ms-latency-and-94-cost-savings-with-diskless-kafka-using-automq-and-amazon-fsx-for-netapp-ontap/)).
- Durability is **delegated to the underlying cloud block-storage service's own replication** (e.g., EBS's built-in multi-AZ durability) rather than AutoMQ implementing its own Kafka-style multi-replica ISR mechanism — a materially different durability strategy from both Kafka/Redpanda's self-managed quorum replication and WarpStream's direct reliance on S3's own durability.

**"Stateless broker" design.** Because durable state lives on EBS/S3 rather than the broker's own disk, brokers can be added, removed, or replaced without data migration, partition ownership can move in seconds (only metadata pointers change), and brokers can safely run on **spot/preemptible instances** ([AutoMQ wiki: stateless broker](https://github.com/AutoMQ/automq/wiki/Stateless-Broker)). AutoMQ still uses Kafka/KRaft-style cluster metadata and retains **leader-based partition ordering** — its own comparative writeup explicitly describes its own model as "leaderful," distinguishing it from a fully leaderless design ([AutoMQ: diskless Kafka architecture trade-offs](https://www.automq.com/blog/diskless-kafka-architecture-tradeoffs)).

**KIP-1150 relationship — important correction.** AutoMQ is **not** the primary author of KIP-1150. The KIP's authors (Greg Harris, Ivan Yurchenko, Jorge Quilcate, Giuseppe Lillo, Anatolii Popov, Juha Mynttinen, Josep Prat, Filip Yonov) are predominantly **Aiven** engineers, and Aiven has published the reference implementation as an open-source fork called **Inkless** (AGPL-licensed) ([KIP-1150 wiki](https://cwiki.apache.org/confluence/display/KAFKA/KIP-1150:+Diskless+Topics); [Aiven: guide to diskless Kafka](https://aiven.io/blog/guide-diskless-apache-kafka-kip-1150); [github.com/aiven/inkless](https://github.com/aiven/inkless)). AutoMQ instead positions itself as an **earlier, already-production** implementation of the same broad idea and has proposed its own competing/complementary KIP, **KIP-1183 "Unified Shared Storage,"** to let local-disk and shared/object-storage architectures coexist under one abstraction in mainline Kafka ([AutoMQ: KIP-1150 explained](https://www.automq.com/blog/kip-1150-explained-diskless-topics-kafka-future)). KIP-1150 itself was **accepted March 2, 2026** (9 binding / 5 non-binding votes) — the first time the Kafka community formally endorsed object storage as a first-class data layer — but implementation detail is deferred to follow-on KIPs 1163/1164, and its proposed design (leaderless writes, an "Ingestion Engine" that assigns offsets outside local disk) is structurally closer to WarpStream/Bufstream than to AutoMQ's own leader-based model.

**Language/license.** **Java**, forked from Apache Kafka. **Apache License 2.0** for the open-source core (explicitly marketed as a differentiator vs. BSL/SSPL competitors) — no time-delayed conversion clause. A commercial **BYOC** offering adds enterprise tooling (UI, RBAC, SSO, DR) ([AutoMQ licensing](https://docs.automq.com/automq/what-is-automq/licensing)).

**Differentiation from pure diskless designs.** AutoMQ's default high-performance configuration is architecturally "small attached-disk WAL + object storage," not WarpStream's zero-persistent-disk model; only its S3-only-WAL open-source configuration approaches true statelessness, at a real latency cost.

---

### 2.3 Bufstream (Buf / buf.build) — now discontinued as an independent product

**What it was.** A **Go**-based, fully Kafka-protocol-compatible broker built directly on object storage (S3/GCS/Azure) with an off-the-shelf metadata store (Postgres, Google Cloud Spanner, or etcd), launched 2024 ([Bufstream product page](https://buf.build/product/bufstream); [Bufstream: Kafka at 8x lower cost](https://webflow.buf.build/blog/bufstream-kafka-lower-cost)).

**Architecture.** Bufstream used a **leaderless** design: because every broker connects to the same object-storage bucket and metadata store, any broker could serve reads/writes for any partition — a sharp departure from Kafka's leader-per-partition model. On produce, brokers fanned in requests across topics/partitions into a single write-optimized "intake" file, uploaded it to object storage, and recorded its contents in the metadata store; a produce request was acknowledged only after **both** writes succeeded ([Buf docs: Kafka data flow](https://buf.build/docs/bufstream/architecture/kafka-flow/)). Bufstream was notable for being **independently Jepsen-verified**: the [Jepsen analysis](https://jepsen.io/analyses/bufstream-0.1.0) tested durability, ordering, transactional atomicity, and isolation, and found five Bufstream-specific bugs (stuck consumers/producers from etcd lease-expiry "metastable failures," spurious zero-offsets, lost transaction writes, server-side filtering bugs) — all fixed by v0.1.3 — plus several **unresolved issues inherent to the Kafka protocol itself** (aborted-read visibility, torn transactions) that affect any Kafka-compatible implementation, not just Bufstream.

**License/OSS model.** Proprietary/commercial self-hosted software (not open source in the OSI sense), distributed as a licensed binary.

**Status as of August 2026.** **CoreWeave acquired Bufstream from Buf in May 2026** and folded it into CoreWeave's internal platform (W&B Models/Weave product lines) rather than continuing it as a standalone product; Buf itself remains independent and has refocused on its Protobuf/Schema Registry tooling ([Buf: CoreWeave acquires Bufstream](https://buf.build/blog/coreweave-acquires-bufstream)). This is a useful data point: **Bufstream is no longer an actively marketed, generally-available third-party Kafka-on-object-storage product** — a gap in the market that a new open-source entrant could target.

---

### 2.4 Confluent's Object-Storage Efforts (Kora, Freight Clusters, and the WarpStream Acquisition)

**Kora.** Confluent's cloud-native Kafka engine (powers Confluent Cloud) evolved to support a **"direct write" mode**: data is written straight to object storage (e.g., S3), bypassing local broker storage and inter-broker replication. Confluent describes a replicated, fault-tolerant read/write cache sitting in front of the object store so clients still interact with stateful Kora brokers at low-millisecond latency, while data is aggressively and asynchronously written to object storage ([Jack Vanlightly: hybrid transactional/analytical storage](https://jack-vanlightly.com/blog/2024/5/2/hybrid-transactional-analytical-storage)). Kora also materializes topics directly as **Apache Iceberg tables** via Tableflow, bridging operational and analytical storage.

**Confluent Cloud Freight Clusters (GA ~2025).** Built on Kora's direct-write mode plus **Fetch-From-Follower** (serving reads from same-AZ followers instead of cross-AZ leaders), Freight Clusters trade latency for cost: **sub-100ms latency in standard Confluent Cloud clusters becomes "up to a second or two" on Freight**, in exchange for eliminating inter-AZ replication traffic (cited as up to 88% of self-managed Kafka infrastructure cost) and adding aggressive autoscaling. Confluent reports early customers seeing **~90% lower infrastructure cost** vs. self-managed Kafka ([Confluent: Freight Clusters GA](https://www.confluent.io/blog/freight-clusters-are-generally-available/)). Freight is explicitly positioned for "freight" workloads — log ingestion, clickstreams, large-scale ETL — not latency-sensitive transactional streaming.

**WarpStream acquisition.** Confluent announced the acquisition of WarpStream on **September 9, 2024** (deal value undisclosed) ([Confluent press release](https://www.confluent.io/press-release/confluent-acquires-warpstream-to-advance-next-gen-byoc-data-streaming/); [TechCrunch coverage](https://techcrunch.com/2024/09/09/confluent-acquires-streaming-data-startup-warpstream/)). WarpStream (co-founded by Richard Artoul and Ryan Worl) is now sold as **"Confluent WarpStream,"** positioned as a distinct tier sitting between fully-managed Confluent Cloud and self-managed Confluent Platform, targeting BYOC deployments for latency-tolerant, high-volume workloads (logging, observability, data-lake feeding). Confluent's product portfolio now effectively spans three object-storage-adjacent tiers: **Kora direct-write / Freight Clusters** (fully managed, moderate latency relaxation), **Confluent WarpStream** (BYOC, fully diskless, most latency-relaxed), and standard Confluent Cloud/Platform (disk-based, lowest latency). This gives Confluent competitive coverage across the entire disk-to-diskless spectrum described in this document — worth noting as the most complete incumbent response to the diskless trend. See [01-warpstream-architecture.md](01-warpstream-architecture.md) for the full WarpStream architecture writeup.

---

### 2.5 Apache Pulsar (classic architecture)

**What it is.** A pub/sub platform that, unlike Kafka, separates a **stateless serving layer** (brokers) from a **stateful storage layer** (Apache BookKeeper) from day one, with cluster/ledger metadata historically tracked in ZooKeeper (Pulsar 3.3+/4.0 also supports StreamNative's Oxia or RocksDB in standalone mode) ([Pulsar architecture overview](https://pulsar.apache.org/docs/next/concepts-architecture-overview/); [StreamNative: Pulsar 4.0](https://streamnative.io/blog/announcing-apache-pulsar-tm-4-0-towards-an-open-data-streaming-architecture)). Because brokers hold no partition data themselves, adding a broker adds serving capacity instantly with zero data movement — a real advantage over Kafka's broker-owns-its-partitions model.

**Segmented storage model.** A Pulsar topic partition maps to a sequence of BookKeeper **ledgers** ("segments"), each replicated across an **ensemble** of bookies and composed of bounded fragments. Ledgers are **append-only and immutable** — once sealed, a new one opens — which makes the segment a natural, cloud-friendly offload unit ([StreamNative: ledgers & bookies](https://streamnative.io/blog/pulsar-newbie-guide-for-kafka-engineers-part-3-ledgers-bookies); [DZone: BookKeeper storage](https://dzone.com/articles/apache-bookkeeper-what-makes-a-qualified-storage-system-for-apache-pulsar)). Critically, storage on bookies is still **local disk** — Pulsar decouples storage from serving, but does not eliminate the stateful disk tier.

**Tiered storage offload.** Pulsar offloads **sealed, already-historical** ledgers to S3/GCS/Azure Blob/Aliyun OSS via **Apache jclouds**-based offloader plugins, triggered by a size threshold on the topic's BookKeeper-resident data, while remaining transparently readable through the same topic API ([Pulsar tiered storage overview](https://pulsar.apache.org/docs/next/tiered-storage-overview/); [Pulsar S3 offloader docs](https://pulsar.apache.org/docs/next/tiered-storage-s3/)). New writes always land on BookKeeper first — there is no direct-to-S3 write path in classic Pulsar.

**Consistency.** BookKeeper uses per-ledger tunable **Ensemble/Write-Quorum/Ack-Quorum** parameters and a Flexible-Paxos-like protocol; **ledger fencing** prevents split-brain during failover by permanently sealing a ledger against a former writer once a quorum of bookies acknowledge the fence ([BookKeeper replication protocol](https://bookkeeper.apache.org/archives/docs/r4.4.0/bookkeeperProtocol.html); [Apache wiki: fencing](https://cwiki.apache.org/confluence/display/BOOKKEEPER/Fencing); [Jack Vanlightly: log replication disaggregation survey](https://jack-vanlightly.com/blog/2025/3/13/log-replication-disaggregation-survey-apache-pulsar-and-bookkeeper)).

**Tech stack/license.** **Java** (broker and BookKeeper both), **Apache License 2.0**.

**Differentiation.** Pulsar was already more "cloud-friendly" than Kafka's monolithic broker-owns-partition design — its segment/ledger abstraction anticipated the value of small, immutable, offloadable units — but it still requires and operates a disk-backed, quorum-replicated BookKeeper tier for all *recent* data, plus a separate metadata service, meaning three tiers to run (brokers, bookies, ZooKeeper/Oxia) versus a true diskless design's two (stateless data-plane process, metadata control plane) or fewer.

---

### 2.6 StreamNative Ursa Engine

**What it is.** StreamNative's from-the-ground-up rearchitecture of Pulsar to remove BookKeeper as a hard dependency, announced in **public preview on October 30, 2024** at Data Streaming Summit 2024 ([StreamNative press release](https://streamnative.io/press-releases/streamnative-announces-ursa-engine-public-preview-at-data-streaming-summit-2024)). Motivation: cut cross-AZ replication cost (claimed up to 90% of infra spend at scale), simplify operations by removing the three-tier Pulsar stack, and unify streaming with lakehouse/batch analytics. The underlying paper won the **VLDB 2025 Best Industry Paper** award ([Ursa VLDB paper](https://www.vldb.org/pvldb/vol18/p5184-guo.pdf)).

**Architecture.**
- **Metadata**: replaces ZooKeeper with **Oxia**, StreamNative's own scalable metadata/coordination store (open source, Apache 2.0, written in **Go**) ([Oxia GitHub](https://github.com/streamnative/oxia); [Introducing Oxia](https://streamnative.io/blog/introducing-oxia-scalable-metadata-and-coordination)).
- **Data**: a **pluggable WAL**. The cost-optimized profile writes directly to S3/GCS/Azure Blob as the primary WAL — any broker can serve any partition (leaderless, stateless brokers), eliminating inter-broker/inter-AZ replication. A latency-optimized profile can instead use BookKeeper (Pulsar) or KRaft+local disk (Kafka) per-topic, so **BookKeeper becomes optional, not eliminated** — "Adaptable Topics" let disk-backed and diskless topics coexist in one cluster ([Ursa: reimagine Apache Kafka](https://streamnative.io/blog/ursa-reimagine-apache-kafka-for-the-cost-conscious-data-streaming)).
- **Write path**: producers hit a local-AZ broker, which buffers (~200ms or 4MB threshold), flushes a mixed-partition batch to S3, synchronously updates Oxia metadata, then acks. A background process compacts the row-based S3 objects into columnar **Parquet/Iceberg/Delta Lake** tables, giving "stream-table duality" — the same data is simultaneously a live stream and a queryable lakehouse table with no separate ETL ([Stanislav Kozlovski: Ursa analysis](https://stanislavkozlovski.medium.com/ursa-a-new-diskless-lakestream-engine-for-kafka-6d7b60c72ee3)).
- Marketed as **100% Kafka-API compatible** (protocol versions 0.9–3.4) alongside native Pulsar support.

**Latency/cost trade-off.** StreamNative's own benchmark (5 GB/s OpenMessaging workload) claims **~$54/hr** infra cost vs. **$303/hr AWS MSK** (5.6x) and **$988/hr Redpanda** (18x) ([StreamNative: $50/hr benchmark](https://streamnative.io/blog/how-we-run-a-5-gb-s-kafka-workload-for-just-50-per-hour)) — but the diskless write path carries **~500ms p99 latency** per StreamNative's own materials, with an independent analyst estimating **1–2 second p99 end-to-end** vs. 50–100ms for traditional Kafka — a roughly 10–20x latency increase for a ~10x cost reduction, the same fundamental trade-off seen across this category ([Kozlovski analysis](https://stanislavkozlovski.medium.com/ursa-a-new-diskless-lakestream-engine-for-kafka-6d7b60c72ee3)).

**Open-source status.** **Ursa Engine itself is currently proprietary** — available only via StreamNative Cloud (Serverless/Dedicated/BYOC) or StreamNative Private Cloud license, not released as standalone open source like classic Apache Pulsar ([Datanami coverage](https://www.datanami.com/2024/05/14/streamnative-bolsters-pulsar-data-streaming-platform-with-ursa/)). Oxia (metadata store) is open source (Apache 2.0); data is stored in open formats (Iceberg/Delta/Parquet). Independent reporting suggests StreamNative intends to eventually open-source a Kafka-focused variant, unconfirmed as of this writing.

---

### 2.7 S2.dev

**What it is.** "The API for the log" — S2 elevates the log to a first-class cloud storage primitive (like the object in S3) rather than shipping a broker cluster to operate. Team's own framing: "if Kafka and S3 had a baby." It is **explicitly not Kafka-protocol compatible**, exposing instead a simple, record-oriented gRPC/HTTP API over **streams** grouped into **basins**, with SDKs in TypeScript, Python, Go, and Rust ([S2 architecture docs](https://s2.dev/docs/platform/architecture); [S2: intro blog](https://s2.dev/blog/intro)). A Kafka-compatibility shim is discussed only as a possible future add-on layer, not the core product — a clear philosophical split from Kafka-protocol-first systems like Bufstream/AutoMQ/WarpStream.

**Architecture.** Deployments run as isolated regional "cells." Writes flow through frontend replicas into "channel" pods that act as a WAL, multiplexing many streams into shared chunks flushed to object storage on a time/size trigger; a produce request acks **only once durable in object storage** (no separate fast local buffer trusted ahead of the object store — the speed comes from tight batching, not from acking-before-durable). Chunks are later compacted into larger per-stream segments for efficient historical reads. A purpose-built open-source read-through cache, **cachey** (Rust, built on `foyer`, per-zone, rendezvous-hashed), sits in front of object storage for the read path ([cachey GitHub](https://github.com/s2-streamstore/cachey)).

**Latency.** Standard storage class ≈ sub-500ms p99; a premium **Express** tier ≈ sub-50ms p99 (stated goal sub-5ms), reflecting the same fast-tier-vs-cost trade-off seen elsewhere ([S2 pricing/blog](https://s2.dev/pricing)).

**Language/license.** **Rust**, with deterministic-simulation testing via the `turmoil` framework for correctness under fault injection. The core hosted service is **closed-source commercial SaaS**; open-sourced pieces include the client SDKs, the `cachey` cache, and a newly released MIT-licensed self-hostable single-node variant, **s2-lite**, built on **SlateDB** (an embedded LSM engine on object storage) ([s2-lite launch](https://news.ycombinator.com/item?id=46708055)).

**Positioning.** S2 is infrastructure-primitive-first, not Kafka-replacement-first — most relevant to this project as an architectural reference for the "batch-then-flush WAL on object storage" write path and for its two-tier (standard/express) latency-cost model, rather than as a direct competitor.

---

### 2.8 Iggy.rs / Apache Iggy (Incubating)

**What it is.** A high-performance, persistent message-streaming platform built from scratch in **Rust**, explicitly **not** a Kafka protocol implementation — it defines its own binary wire protocol plus QUIC, TCP, WebSocket, and HTTP/REST transports ([iggy.apache.org](https://iggy.apache.org/); [Apache Iggy GitHub](https://github.com/apache/iggy)). Started by Piotr Gankiewicz in April 2023 as an MIT-licensed personal project; accepted into the **Apache Software Foundation Incubator in February 2025** (21–0 vote), now relicensed **Apache 2.0** and governed under the ASF as an Incubating (not yet top-level) project ([Iggy Incubator proposal](https://cwiki.apache.org/confluence/display/INCUBATOR/Iggy+Proposal)).

**Storage — confirmed local-disk-primary today.** Classic append-only `.log` segment files with companion index files, data flowing from in-memory journals through `io_uring`-based vectored I/O to disk; segments rotate at 1 GiB. **No production object-storage tiering exists today** — relevant as protocol/runtime prior art, not as an object-storage architecture reference.

**Concurrency/tech.** **Thread-per-core, shared-nothing** architecture — each CPU core owns pinned, NUMA-aware shards with no locks on hot paths and no GC pauses; built on `io_uring` (with `compio` mentioned as a cross-platform alternative to raw tokio); custom zero-copy (de)serialization. A **Kafka Gateway/bridge is in development** for migration purposes, but the native protocol remains custom by design ([iggy.apache.org](https://iggy.apache.org/); [Building message streaming in Rust](https://blog.iggy.rs/posts/building-message-streaming-in-rust/)).

**Performance claims.** 2M+ msgs/sec single-node throughput, sub-millisecond average latency (producer ~0.466ms, consumer ~0.357ms) on AWS i4i.4xlarge.

**Relevance as Rust prior art.** The strongest available reference for Rust async I/O engineering at the storage-engine layer: thread-per-core sharding, `io_uring` usage, lock-free hot paths, and transparent benchmarking methodology — directly applicable to designing a fast local buffer/WAL tier in a new object-storage-backed system, even though Iggy itself hasn't gone object-storage-native.

---

### 2.9 Fluvio (InfinyOn)

**What it is.** A distributed, programmable streaming platform written in **Rust**, pairing a Kafka-like log core with WebAssembly-based inline processing ("SmartModules"). InfinyOn's self-reported benchmarks claim 20–38x better p99 latency than Kafka and ~95% lower per-partition memory overhead (vendor claim, unverified independently) ([InfinyOn: introducing Fluvio](https://www.infinyon.com/blog/2021/06/introducing-fluvio/)).

**Architecture.** Clean control/data-plane split: **SC (Stream Controller)** orchestrates cluster/topic/partition metadata and rebalancing; **SPU (Stream Processing Unit)** is the data plane, storing/serving records via local commit-log storage and running WASM SmartModules inline on the data path.

**Storage — confirmed local-disk-primary, explicitly declined object-storage tiering.** SPUs persist to local commit-log storage (Kafka-like append-only segments); a GitHub feature request for S3 tiering (issue #3847, opened Jan 2024) was **closed as "not planned."** This is a useful negative data point: being Rust-native did not lead Fluvio toward the object-storage-first model that WarpStream/Bufstream/S2/AutoMQ represent ([Fluvio issue #3847](https://github.com/infinyon/fluvio/issues/3847)).

**License.** Fluvio (the engine) is **Apache License 2.0** ([Fluvio LICENSE](https://github.com/infinyon/fluvio/blob/master/LICENSE)); InfinyOn's commercial layer ("Stateful DataFlow") and InfinyOn Cloud sit on top as an additive paid offering, not a relicense.

**Relevance as Rust prior art.** Its clean control-plane/data-plane separation is directly analogous to what a Kafka-compatible object-storage system needs (metadata service vs. data-serving nodes), and its WASM SmartModule design is a notable optional differentiator to consider. It is a cautionary example that Rust-native alone doesn't imply object-storage-native.

---

### 2.10 WarpStream (brief pointer — covered in depth elsewhere)

WarpStream is a Kafka-protocol-compatible platform whose stateless **Go** data-plane "agents" write records directly to S3-compatible object storage with **zero local disk**, while cluster/partition metadata is managed by a separate control plane — eliminating per-partition Raft/BookKeeper consensus in favor of a metadata service. Its **BYOC (Bring Your Own Cloud)** model runs the data plane in the customer's own cloud account, claiming up to ~80% lower cost than traditional Kafka for latency-tolerant workloads (logging, observability, data-lake ingestion) ([warpstream.com](https://www.warpstream.com/)). Acquired by Confluent in September 2024 and now sold as **"Confluent WarpStream,"** positioned as Confluent's BYOC/diskless tier between fully-managed Confluent Cloud and self-managed Confluent Platform ([Confluent press release](https://www.confluent.io/press-release/confluent-acquires-warpstream-to-advance-next-gen-byoc-data-streaming/)). As the category's original reference design, WarpStream now sits alongside a growing field (Bufstream before its 2026 discontinuation, AutoMQ, StreamNative Ursa, Aiven's Inkless/KIP-1150) that it substantially inspired. Full detail in [01-warpstream-architecture.md](01-warpstream-architecture.md).

---

### 2.11 Other Notable / Adjacent Systems

- **Tansu** ([github.com/tansu-io/tansu](https://github.com/tansu-io/tansu)) — a **Rust**-based, Apache-2.0-licensed, Kafka-API-compatible broker (recently rebranded "Nisshi" in some sources) with **pluggable storage backends**: PostgreSQL, libSQL/SQLite, Amazon S3, or in-memory. Stateless/leaderless design; can write validated records directly as Apache Iceberg or Delta Lake tables in Parquet, and ships as a single statically-linked binary. This is the closest existing prior art to the target project's exact profile (Rust + Kafka-compatible + object-storage-backed + open source) and merits close study.

- **turbopuffer** ([turbopuffer.com](https://turbopuffer.com)) — not a queue/log system (it's a serverless vector/full-text search database on S3), but architecturally adjacent and independently researched for transferable lessons — see [08-turbopuffer-lessons.md](08-turbopuffer-lessons.md).

- **Danube** ([github.com/danube-messaging/danube](https://github.com/danube-messaging/danube)) — an open-source, **Rust**, Tokio-based pub/sub platform explicitly inspired by Apache Pulsar's concepts. Uses embedded Raft (`openraft`) for metadata/consensus rather than an external store like ZooKeeper/etcd, and supports storage backends including local WAL, shared filesystem, and **cloud object storage (S3/GCS/Azure Blob)** with tiered replay, plus a schema registry and an Iceberg lakehouse connector. Early-stage and small (a few hundred GitHub stars) but a close architectural cousin worth monitoring.

- **KIP-1150 "Diskless Topics"** ([cwiki.apache.org](https://cwiki.apache.org/confluence/display/KAFKA/KIP-1150:+Diskless+Topics)) — accepted March 2, 2026, primarily championed by **Aiven** engineers, not AutoMQ. Introduces a new diskless topic type with a pluggable **Ingestion Engine** that assigns offsets and writes leaderlessly, directly to object storage, coexisting with classic disk-based topics in the same cluster. Aiven's reference implementation, **Inkless** ([github.com/aiven/inkless](https://github.com/aiven/inkless)), is **AGPL-licensed** and already offered inside Aiven's BYOC product. Trade-off explicitly acknowledged in the KIP: projected ~500ms p99 latency vs. traditional Kafka's sub-100ms, in exchange for ~80% cost reduction. Implementation detail is still being split into follow-on KIPs 1163/1164 as of mid-2026. See [02-kafka-protocol-compatibility.md](02-kafka-protocol-compatibility.md) for protocol-level detail.

- **Responsive.dev (RS3)** — not a broker replacement. Builds **RS3**, an object-store-native **state store for Kafka Streams**, using Kafka itself as a WAL for recent writes and tiering bulk state into S3-compatible storage via **SlateDB**. Complementary to Kafka rather than competitive with WarpStream-style brokers ([docs.responsive.dev/storage/rs3](https://docs.responsive.dev/storage/rs3)). Note: their public site indicates a pivot toward SlateDB/OpenData.dev and away from actively promoting Kafka Streams content as of this research.

- **Estuary Flow** — a real-time CDC/ETL/data-integration platform, not a Kafka-protocol broker. Built on the **Gazette** project's append-only "journals" abstraction: new writes sit in a replicated in-memory buffer for low-latency reads, then persist as fragment files to S3/GCS/Azure for durable/historical access, giving each "collection" dual nature as both stream and batch dataset. Architecturally interesting (buffer + object-storage-backed fragments pattern) but positioned as a data-movement tool, not a broker replacement.

- **"Turbine"** — investigated and found **not relevant**: no credible object-storage-native streaming broker by this name exists. The name is used by unrelated projects (Meta's internal stream-processing orchestration platform; Meroxa's low-code pipeline framework), neither of which is an object-storage log system.

- **StreamNative Ursa / Aiven Inkless / Redpanda 26.1** are, per independent industry analysis, the current market's few systems supporting **mixed "adaptable" topics** (disk-based and diskless coexisting in one cluster) — a pattern likely worth considering for a new system's migration story.

---

## 3. Comparison Table

| Name | Storage model | Language | License | Kafka API compat? | Notable latency characteristics | Notes |
|---|---|---|---|---|---|---|
| **Redpanda (core)** | Local NVMe primary; Raft per partition | C++ (Seastar) | BSL (source-available, converts to Apache 2.0 after 4 yrs) | Yes (native reimplementation) | ~6–8ms p99 produce (NVMe, acks=all) | Fastest of the disk-based systems; Tiered Storage is a cold-tier add-on, enterprise-licensed |
| **Redpanda Tiered Storage / Cloud Topics** | Object storage for cold data / direct-write payloads; Raft + metadata stay local | C++ | BSL / Enterprise | Yes | Cold reads: object-store fetch latency; metadata ops sub-10ms | Deliberately not "diskless" — Redpanda argues keeping consensus local preserves ordering guarantees |
| **AutoMQ** | WAL (S3-only OSS, or EBS/EFS/FSx commercial) + object storage primary | Java (Kafka fork) | Apache 2.0 (core); commercial BYOC add-ons | Yes (Kafka fork) | ~170–350ms (S3-only WAL) to single-digit ms (EBS WAL) | Stateless brokers via WAL detach/reattach (EBS Multi-Attach); "leaderful," not KIP-1150 author |
| **Bufstream** | Object storage primary + external metadata store (Postgres/Spanner/etcd) | Go | Proprietary/commercial | Yes (Jepsen-verified) | Not publicly quantified; leaderless design | **Discontinued as standalone product** — acquired by CoreWeave, May 2026 |
| **Confluent Kora (direct-write) / Freight Clusters** | Object storage primary with replicated read/write cache | Not disclosed (Confluent Cloud proprietary) | Proprietary/SaaS | Yes | Standard Kora: low-ms; Freight: up to 1–2s | Freight trades latency for ~90% cost cut vs. self-managed Kafka; also does Iceberg materialization via Tableflow |
| **Confluent WarpStream (post-acquisition)** | Object storage only, zero local disk | Go | Proprietary/commercial (BYOC) | Yes | Hundreds of ms typical | Original diskless reference design; acquired by Confluent Sept 2024, now a Confluent product tier |
| **Apache Pulsar (classic)** | BookKeeper local-disk ledgers (primary) + tiered offload of sealed segments to object storage | Java | Apache 2.0 | No (native Pulsar protocol; Kafka-on-Pulsar bridge exists separately) | BookKeeper quorum writes: low-ms; tiered reads: higher | Segment/ledger model anticipated cloud-friendliness but still requires a stateful disk-backed quorum tier |
| **StreamNative Ursa Engine** | Object storage primary WAL (cost-optimized) or optional BookKeeper/KRaft (latency-optimized); Oxia metadata | Not publicly confirmed (Oxia is Go; broker language unconfirmed) | Proprietary (StreamNative Cloud/Private Cloud) | Yes (also native Pulsar) | ~500ms p99 (vendor); independent estimate 1–2s p99 | Writes directly into open Iceberg/Delta tables ("stream-table duality"); VLDB 2025 Best Industry Paper |
| **S2.dev** | Object storage primary via batching WAL "channels" + read-through cache (cachey) | Rust | Proprietary SaaS core; SDKs/cachey/s2-lite open source (MIT/Apache 2.0) | No — own log-primitive API, not Kafka protocol | Standard: sub-500ms p99; Express tier: sub-50ms p99 | Positions "log as cloud primitive" rather than Kafka-broker replacement; s2-lite built on SlateDB |
| **Apache Iggy (Incubating)** | Local disk, append-only segments; `io_uring` | Rust | Apache 2.0 | No — custom binary/QUIC/HTTP protocol (Kafka gateway in progress) | Sub-ms average latency (single node) | No object-storage tiering yet; best Rust runtime/thread-per-core prior art |
| **Fluvio (InfinyOn)** | Local commit-log storage (SPU); no object-storage tiering (feature request explicitly declined) | Rust | Apache 2.0 (core); commercial add-ons | No — Kafka-like but own protocol; separate connectors | Vendor claims 20–38x better p99 than Kafka (unverified) | Clean control/data-plane split + WASM SmartModules are relevant design patterns |
| **Tansu (Nisshi)** | Pluggable: Postgres, libSQL, S3, or in-memory | Rust | Apache 2.0 | Yes | Not independently benchmarked | Closest existing prior art to the target project's profile (Rust + Kafka-compatible + object storage + OSS) |
| **Danube** | Local WAL, filesystem, or object storage (S3/GCS/Azure), tiered replay; embedded Raft (`openraft`) metadata | Rust | Open source (license not fully confirmed) | No — Pulsar-inspired own protocol | Not published | Early-stage, small community; Rust + object storage + Pulsar-like semantics |
| **KIP-1150 / Aiven Inkless** | Object storage primary, leaderless Ingestion Engine, coexists with classic topics | Java (Kafka mainline fork) | AGPL (Inkless reference impl.) | Yes (native to Kafka) | Projected ~500ms p99 | Accepted into Apache Kafka March 2026; championed by Aiven, not AutoMQ |
| **Responsive.dev (RS3)** | Object storage (via SlateDB) for Kafka Streams **state**, Kafka as WAL | Not confirmed | Not confirmed | N/A (state-store layer, not a broker) | N/A | Complementary to Kafka, not a broker replacement |
| **Estuary Flow (Gazette)** | In-memory buffer + object-storage-backed fragment files (journals) | Go (Gazette) | Mixed OSS/commercial | No — own CDC/ETL platform | <100ms claimed for recent data | Data-integration platform, not a Kafka-protocol broker |

---

## 4. Implications for a New Rust, Object-Storage-Native, Kafka-Compatible System

1. **The competitive gap is real but narrowing fast.** As of August 2026, no actively-maintained, fully open-source, Rust-native, Kafka-protocol-compatible, object-storage-primary system exists at the maturity of WarpStream/AutoMQ. Tansu is the closest match in profile but is comparatively new/unproven; Bufstream (Go) is gone as an independent product; StreamNative Ursa and Confluent's offerings are proprietary SaaS; Redpanda remains disk-primary with a BSL license.
2. **The fast-WAL-before-object-storage pattern is close to universal** among latency-competitive diskless designs (AutoMQ's EBS/S3 WAL, S2's channel/Express tier, Ursa's optional BookKeeper path) — a new system should assume it needs a comparable answer rather than accepting KIP-1150-class ~500ms p99 by default, unless targeting only latency-tolerant workloads.
3. **KIP-1150's eventual arrival in mainline Apache Kafka is a genuine long-term threat/validation signal** — it legitimizes the category but could also commoditize "basic diskless Kafka" over the next 1–3 years, arguing for differentiation via latency (fast WAL tier), operational simplicity, true open-source licensing (Apache 2.0, not AGPL/BSL), or lakehouse-native features (Iceberg/Delta write path, as Bufstream/Ursa/Kora all now do).
4. **Rust prior art (Iggy, Fluvio, S2, Tansu, Danube) offers concrete, reusable engineering patterns** — thread-per-core/`io_uring` I/O (Iggy), clean control/data-plane separation and WASM extensibility (Fluvio), batching-WAL-to-object-storage with a read-through cache (S2/cachey), and pluggable storage backends (Tansu) — even though none of them is itself the target system's exact combination.

---

## Sources

- [Redpanda: Tiered Storage architecture deep dive](https://www.redpanda.com/blog/tiered-storage-architecture-shadow-indexing-deep-dive)
- [Redpanda architecture docs](https://docs.redpanda.com/current/get-started/architecture/)
- [Redpanda: Use Tiered Storage (docs)](https://docs.redpanda.com/current/manage/tiered-storage/)
- [Redpanda: Cloud Topics — write to object storage](https://www.redpanda.com/data-streaming/cloud-topics-write-to-object-storage)
- [Redpanda: BSL source-available license](https://www.redpanda.com/blog/bsl-source-available-license)
- [Redpanda: What makes Redpanda fast](https://www.redpanda.com/blog/what-makes-redpanda-fast)
- [Jack Vanlightly: Kafka vs Redpanda performance](https://jack-vanlightly.com/blog/2023/5/15/kafka-vs-redpanda-performance-do-the-claims-add-up)
- [AutoMQ GitHub](https://github.com/automq/automq)
- [AutoMQ wiki: Introducing AutoMQ](https://github.com/AutoMQ/automq/wiki/Introducing-AutoMQ:-a-cloud-native-replacement-of-Apache-Kafka)
- [AutoMQ wiki: Stateless Broker](https://github.com/AutoMQ/automq/wiki/Stateless-Broker)
- [AutoMQ wiki: WAL Storage](https://github.com/AutoMQ/automq/wiki/WAL-Storage)
- [AutoMQ architecture overview (docs)](https://docs.automq.com/automq/architecture/overview)
- [AutoMQ: 100 Lines of Code / S3 WAL integration](https://www.automq.com/blog/automq-s3-wal-integration)
- [AutoMQ: licensing](https://docs.automq.com/automq/what-is-automq/licensing)
- [AutoMQ: KIP-1150 explained](https://www.automq.com/blog/kip-1150-explained-diskless-topics-kafka-future)
- [AutoMQ: diskless Kafka architecture trade-offs](https://www.automq.com/blog/diskless-kafka-architecture-tradeoffs)
- [AWS blog: sub-10ms latency with AutoMQ + FSx](https://aws.amazon.com/blogs/storage/achieving-sub-10ms-latency-and-94-cost-savings-with-diskless-kafka-using-automq-and-amazon-fsx-for-netapp-ontap/)
- [Apache Kafka KIP-1150: Diskless Topics (cwiki)](https://cwiki.apache.org/confluence/display/KAFKA/KIP-1150:+Diskless+Topics)
- [Aiven: guide to diskless Apache Kafka (KIP-1150)](https://aiven.io/blog/guide-diskless-apache-kafka-kip-1150)
- [Aiven Inkless GitHub](https://github.com/aiven/inkless)
- [2 Minute Streaming: KIP-1150 diskless Kafka topics](https://blog.2minutestreaming.com/p/diskless-kafka-topics-kip-1150)
- [Factor House: KIP-1150 explained](https://factorhouse.io/articles/kip-1150-diskless-topics-explained)
- [Medium: The Good, the Bad, and the AutoMQ](https://medium.com/fresha-data-engineering/the-good-the-bad-and-the-automq-5aa7a8748e71)
- [Bufstream product page](https://buf.build/product/bufstream)
- [Buf: Bufstream at 8x lower cost](https://webflow.buf.build/blog/bufstream-kafka-lower-cost)
- [Buf docs: Kafka data flow in Bufstream](https://buf.build/docs/bufstream/architecture/kafka-flow/)
- [Jepsen: Bufstream 0.1.0 analysis](https://jepsen.io/analyses/bufstream-0.1.0)
- [Buf: CoreWeave acquires Bufstream](https://buf.build/blog/coreweave-acquires-bufstream)
- [Confluent: Freight Clusters GA](https://www.confluent.io/blog/freight-clusters-are-generally-available/)
- [Jack Vanlightly: Hybrid transactional/analytical storage (Kora)](https://jack-vanlightly.com/blog/2024/5/2/hybrid-transactional-analytical-storage)
- [Confluent: press release — acquires WarpStream](https://www.confluent.io/press-release/confluent-acquires-warpstream-to-advance-next-gen-byoc-data-streaming/)
- [Confluent: blog — Confluent acquires WarpStream](https://www.confluent.io/blog/confluent-acquires-warpstream/)
- [TechCrunch: Confluent acquires WarpStream](https://techcrunch.com/2024/09/09/confluent-acquires-streaming-data-startup-warpstream/)
- [WarpStream homepage](https://www.warpstream.com/)
- [Apache Pulsar: architecture overview](https://pulsar.apache.org/docs/next/concepts-architecture-overview/)
- [StreamNative: Pulsar newbie guide — ledgers & bookies](https://streamnative.io/blog/pulsar-newbie-guide-for-kafka-engineers-part-3-ledgers-bookies)
- [Jack Vanlightly: log replication disaggregation survey (Pulsar/BookKeeper)](https://jack-vanlightly.com/blog/2025/3/13/log-replication-disaggregation-survey-apache-pulsar-and-bookkeeper)
- [DZone: Apache BookKeeper storage for Pulsar](https://dzone.com/articles/apache-bookkeeper-what-makes-a-qualified-storage-system-for-apache-pulsar)
- [Apache Pulsar: tiered storage overview](https://pulsar.apache.org/docs/next/tiered-storage-overview/)
- [Apache Pulsar: S3 offloader docs](https://pulsar.apache.org/docs/next/tiered-storage-s3/)
- [Apache BookKeeper: replication protocol](https://bookkeeper.apache.org/archives/docs/r4.4.0/bookkeeperProtocol.html)
- [Apache BookKeeper wiki: fencing](https://cwiki.apache.org/confluence/display/BOOKKEEPER/Fencing)
- [StreamNative: Announcing Ursa Engine public preview](https://streamnative.io/press-releases/streamnative-announces-ursa-engine-public-preview-at-data-streaming-summit-2024)
- [StreamNative: Ursa — reimagine Apache Kafka](https://streamnative.io/blog/ursa-reimagine-apache-kafka-for-the-cost-conscious-data-streaming)
- [StreamNative: Ursa Everywhere](https://streamnative.io/blog/ursa-everywhere-lakehouse-native-future-data-streaming)
- [StreamNative: Ursa wins VLDB 2025 Best Industry Paper](https://streamnative.io/blog/ursa-wins-vldb-2025-best-industry-paper-the-first-lakehouse-native-streaming-engine-for-kafka)
- [VLDB 2025: Ursa paper (PDF)](https://www.vldb.org/pvldb/vol18/p5184-guo.pdf)
- [Oxia GitHub](https://github.com/streamnative/oxia)
- [StreamNative: Introducing Oxia](https://streamnative.io/blog/introducing-oxia-scalable-metadata-and-coordination)
- [Stanislav Kozlovski: Ursa — a new diskless Lakestream engine](https://stanislavkozlovski.medium.com/ursa-a-new-diskless-lakestream-engine-for-kafka-6d7b60c72ee3)
- [StreamNative: $50/hour 5GB/s Kafka workload benchmark](https://streamnative.io/blog/how-we-run-a-5-gb-s-kafka-workload-for-just-50-per-hour)
- [Datanami: StreamNative bolsters Pulsar with Ursa](https://www.datanami.com/2024/05/14/streamnative-bolsters-pulsar-data-streaming-platform-with-ursa/)
- [S2.dev: platform architecture docs](https://s2.dev/docs/platform/architecture)
- [S2.dev: intro blog](https://s2.dev/blog/intro)
- [S2.dev: pricing](https://s2.dev/pricing)
- [cachey GitHub](https://github.com/s2-streamstore/cachey)
- [Hacker News: s2-lite launch](https://news.ycombinator.com/item?id=46708055)
- [Apache Iggy (iggy.apache.org)](https://iggy.apache.org/)
- [Apache Iggy GitHub](https://github.com/apache/iggy)
- [Apache Incubator: Iggy proposal](https://cwiki.apache.org/confluence/display/INCUBATOR/Iggy+Proposal)
- [Iggy blog: building message streaming in Rust](https://blog.iggy.rs/posts/building-message-streaming-in-rust/)
- [Iggy blog: joining Apache Incubator](https://blog.iggy.rs/posts/apache-incubant/)
- [InfinyOn: introducing Fluvio](https://www.infinyon.com/blog/2021/06/introducing-fluvio/)
- [Fluvio GitHub](https://github.com/infinyon/fluvio)
- [Fluvio GitHub issue #3847 (S3 tiering, closed not-planned)](https://github.com/infinyon/fluvio/issues/3847)
- [Fluvio LICENSE](https://github.com/infinyon/fluvio/blob/master/LICENSE)
- [Tansu GitHub](https://github.com/tansu-io/tansu)
- [InfoQ: Tansu stateless Kafka-compatible broker](https://www.infoq.com/news/2026/03/tansu-stateless-kafka-compatible/)
- [Danube GitHub](https://github.com/danube-messaging/danube)
- [dev-state.com: Danube introduction](https://dev-state.com/posts/danube_intro/)
- [Responsive.dev: RS3 docs](https://docs.responsive.dev/storage/rs3)
- [Estuary docs: journals (Gazette)](https://docs.estuary.dev/concepts/advanced/journals/)
- [Estuary: new reference architecture for CDC](https://estuary.dev/blog/new-reference-architecture-for-cdc/)
- [Meta engineering: Turbine](https://engineering.fb.com/2020/04/21/core-infra/turbine/)
- [AutoMQ: top 7 diskless Kafka / object-storage platforms 2026](https://www.automq.com/blog/top-7-diskless-kafka-object-storage-streaming-platforms-2026)
- [Kai Waehner: the rise of diskless Kafka](https://www.kai-waehner.de/blog/2025/08/11/the-rise-of-diskless-kafka-rethinking-brokers-storage-and-the-kafka-protocol/)
