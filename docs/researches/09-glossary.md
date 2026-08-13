---
title: "Glossary"
slug: glossary
status: draft
last_updated: 2026-08-12
tags: [glossary, reference]
related: [README]
summary: >
  Quick-reference definitions for terms used throughout this research corpus,
  so an LLM (or human) can resolve an unfamiliar acronym without re-reading
  a full source document.
---

# Glossary

Terms as used across this research corpus, not general dictionary definitions. Where a term is contested or system-specific, the system is named.

**Agent** — WarpStream's term for its stateless data-plane process (no local disk for log data). See [01-warpstream-architecture.md](01-warpstream-architecture.md).

**BSL / BUSL (Business Source License)** — a source-available license with a licensor-defined "excluded purpose" (commonly "no competing hosted service") that auto-converts to a fully open license after a fixed period (often 4 years). Not OSI-approved. See [07-licensing-strategy.md](07-licensing-strategy.md) §2.

**BYOC (Bring Your Own Cloud)** — deployment model where the vendor's data-plane software runs inside the customer's own cloud account/VPC, while the vendor typically operates the control plane. WarpStream's deployment model.

**CAS (Compare-And-Swap)** — an atomic conditional operation: "write X only if the current value is still Y." On object storage, implemented via conditional PUT (S3 `If-Match`/`If-None-Match`, GCS generation preconditions). Central coordination primitive for turbopuffer ([08](08-turbopuffer-lessons.md)) and a candidate primitive for offset sequencing ([06](06-distributed-systems-design-challenges.md) §1, Option B).

**Diskless topic** — Apache Kafka's own term (KIP-1150) for a topic type that stores data in object storage with no per-broker persistent disk. See [02-kafka-protocol-compatibility.md](02-kafka-protocol-compatibility.md) §5.

**Group commit** — batching many concurrent logical writes into a single physical write (here, a single object-storage PUT) to amortize per-request latency/cost. The core technique behind every diskless design's write path. See [06](06-distributed-systems-design-challenges.md) §2 and [08](08-turbopuffer-lessons.md) §2.

**ISR (In-Sync Replica set)** — classic Kafka's durability unit: the set of replicas current enough to be eligible for leader election; `acks=all` waits for every current ISR member. See [02-kafka-protocol-compatibility.md](02-kafka-protocol-compatibility.md) §4.1.

**KIP (Kafka Improvement Proposal)** — Apache Kafka's formal design-proposal process. Numbers referenced throughout: **KIP-98** (transactions/EOS, [02](02-kafka-protocol-compatibility.md) §4.3), **KIP-405** (Tiered Storage, [02](02-kafka-protocol-compatibility.md) §5.1), **KIP-429** (cooperative-sticky rebalancing, [02](02-kafka-protocol-compatibility.md) §3.2), **KIP-482** (flexible versions/tagged fields, [02](02-kafka-protocol-compatibility.md) §1.5), **KIP-848** (next-gen consumer rebalance protocol, [02](02-kafka-protocol-compatibility.md) §3.3), **KIP-1150/1163/1164/1165** (the "Diskless Topics" family, [02](02-kafka-protocol-compatibility.md) §5.2–5.5, [03](03-competitive-landscape.md), [06](06-distributed-systems-design-challenges.md)).

**Lightning Topics** — WarpStream feature that decouples durability (write to object storage) from sequencing (offset assignment in the metadata store), acking the producer before the offset is finalized, for lower latency at the cost of idempotency/transactions/external consistency. See [01](01-warpstream-architecture.md) and [06](06-distributed-systems-design-challenges.md) §2.

**Metadata store / control plane** — the strongly-consistent component that decides partition-offset ordering, tracks which objects contain which offset ranges, and (in most designs) holds consumer-group offsets. The central design decision of this whole project — see [06-distributed-systems-design-challenges.md](06-distributed-systems-design-challenges.md) §1 for the three main approaches (external store, CAS-native, hybrid-local-consensus).

**object_store** — the Apache Arrow project's unified async Rust trait over S3/GCS/Azure/local-disk object storage, with a `PutMode` abstraction covering conditional writes. Leading candidate storage-layer crate. See [05-rust-ecosystem.md](05-rust-ecosystem.md) §1.

**RecordBatch (message format v2)** — Kafka's on-wire record container since KIP-98 (0.11+): a 61-byte fixed header, varint-delta-encoded records, whole-batch compression, CRC-32C. See [02-kafka-protocol-compatibility.md](02-kafka-protocol-compatibility.md) §2.

**S3 Express One Zone** — AWS's single-AZ, low-latency (single-digit ms), higher-per-GB-cost S3 storage class. Diskless systems (WarpStream, S2.dev) write a quorum across multiple Express buckets in different AZs to reconstruct multi-AZ durability while keeping Express's latency. See [04-object-storage-s3-gcs.md](04-object-storage-s3-gcs.md) §2, §7–8.

**WAL (Write-Ahead Log)** — a durable, low-latency local (or attached-disk) log that a produce ack can depend on before the async upload to object storage completes. AutoMQ's core latency mitigation ([03](03-competitive-landscape.md), [06](06-distributed-systems-design-challenges.md) §2); turbopuffer instead treats its object-storage WAL as directly synchronous ([08](08-turbopuffer-lessons.md) §2).
