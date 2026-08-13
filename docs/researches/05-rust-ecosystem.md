---
title: "Rust Crate Ecosystem for a Kafka-Compatible, Object-Storage-Backed Broker"
slug: rust-ecosystem
status: draft
last_updated: 2026-08-12
tags: [rust, crates, object_store, opendal, kafka-protocol, tokio, openraft, foundationdb, moka, foyer, madsim, tansu]
related: [01-warpstream-architecture, 02-kafka-protocol-compatibility, 04-object-storage-s3-gcs, 06-distributed-systems-design-challenges]
summary: >
  Survey of the Rust crate ecosystem for this project — object storage
  clients (object_store vs opendal vs provider SDKs), the kafka-protocol
  crate, async/networking (tokio), metadata/consensus options (openraft,
  etcd, FoundationDB, Postgres, DynamoDB), serialization, observability,
  caching (moka, foyer), deterministic simulation testing (madsim, turmoil,
  loom), and closest Rust prior art (Tansu, RobustMQ, StreamHouse, Iggy, Fluvio).
---

# Research: Rust Crate Ecosystem for a Kafka-Compatible, Object-Storage-Backed Streaming Broker

**Date:** 2026-08-12
**Scope:** Survey of the Rust crate ecosystem for building a WarpStream-style broker — Kafka wire-protocol compatible, with S3/GCS/Azure object storage as the primary durable store rather than local disks. Covers object storage clients, protocol implementation, async runtime, metadata consensus, serialization, observability, prior-art projects, caching, deterministic-simulation testing, and licensing.

**Context — why "Kafka on object storage" at all:** WarpStream's own framing of the problem is a useful backdrop for this whole survey. Traditional Kafka replicates every byte across three availability zones on local/EBS disks, and cross-AZ network transfer costs alone can exceed $0.05/GiB — more than a full month of S3 storage for the same data. A "diskless" design that streams directly to object storage and treats it as the source of truth removes local disks, cross-AZ replication traffic, and most operational burden (no ZooKeeper/KRaft quorum tuning, no partition-to-broker disk affinity, trivial elastic scaling of stateless brokers), at the cost of higher end-to-end latency (WarpStream cites ~1s p99 producer-to-consumer) — an acceptable trade for most non-latency-critical Kafka workloads. This is the design point every crate below is being evaluated against. [warpstream.com/blog/kafka-is-dead-long-live-kafka](https://www.warpstream.com/blog/kafka-is-dead-long-live-kafka), [warpstream.com/blog/the-case-for-shared-storage](https://www.warpstream.com/blog/the-case-for-shared-storage)

---

## 1. Object storage clients

### `object_store` (Apache Arrow project)
- **Maturity/adoption:** v0.14.1 (2026-07-15), MIT/Apache-2.0. ~6–18M downloads/month depending on the measurement window (78.3M total lifetime downloads). GitHub [`apache/arrow-rs-object-store`](https://github.com/apache/arrow-rs-object-store): ~310 stars, 610 commits, last push 2026-08-03. Originally built by InfluxData, donated to Apache Arrow, later split into its own repo. MSRV 1.85.0, releases roughly every 2 months.
- **Ecosystem gravity:** 486 reverse dependencies including `datafusion`, `deltalake-core`/`deltalake-aws`/`deltalake-gcp` (delta-rs), `polars`, `parquet`, `lance` (LanceDB), `delta_kernel`, `surrealdb-core` — it is the de facto storage-abstraction standard across the Rust data-infra stack, not just a niche utility.
- **API shape:** Single `ObjectStore` async trait — `put_opts`/`get_opts`, `put_multipart_opts` returning a `MultipartUpload` handle (`put_part()`, `complete()`, explicit `abort()` — S3/GCS do **not** auto-GC orphaned parts), `list()`/`list_with_delimiter()`, `delete()`/`delete_stream()`, `copy_opts()`/`rename_opts()`.
- **Conditional writes (critical for this project):** a `PutMode` enum gives `Overwrite`, `Create` (atomic put-if-absent, returns `Error::AlreadyExists` on conflict), and `Update(UpdateVersion)` (compare-and-swap against a known ETag, returns `Error::Precondition` on mismatch) — mapped per-backend (`S3ConditionalPut::ETagMatch` using standard `If-Match`/`If-None-Match`, native GCS/Azure equivalents). This directly matches AWS S3's native conditional-write feature, GA since **August 20, 2024** ([AWS announcement](https://aws.amazon.com/about-aws/whats-new/2024/08/amazon-s3-conditional-writes/)) — before that date, S3 had no atomic CAS primitive at all, which is why WarpStream-style architectures only became broadly buildable on vanilla S3 in the last two years. One caveat carried through both the SDK and `object_store`: only the *finishing* call (`PutObject`/`CompleteMultipartUpload`) can carry a conditional header — `UploadPart` cannot — a general S3 API constraint, not a library gap.
- **Retry/backoff:** built-in `RetryConfig`/`BackoffConfig` with exponential backoff + decorrelated jitter, handled centrally rather than left to the caller.
- **Relevance:** this is exactly the primitive a WarpStream-style commit protocol needs — atomic put-if-absent/CAS for fencing and metadata-commit races, uniform multipart for large segment files, and one code path across S3/GCS/Azure/local-disk-for-tests.

### `opendal` (Apache OpenDAL)
- **Maturity/adoption:** v0.58.1 (2026-07-31), Apache-2.0. ~1.3–3.4M downloads/month (13.2M lifetime). GitHub [`apache/opendal`](https://github.com/apache/opendal): ~5,300 stars, 804 forks, 4,556 commits — a full Apache top-level project, not a sub-project, and considerably higher GitHub star count than `object_store`.
- **Breadth:** far wider backend coverage — object storage (S3, GCS, Azure Blob, Aliyun OSS, Huawei OBS, Tencent COS, B2, IPFS…), filesystems (HDFS, ADLS), SaaS (Google Drive, Dropbox, OneDrive, Hugging Face), protocols (HTTP, FTP, WebDAV, SFTP), and even KV/DB backends (Postgres, MySQL, Redis, RocksDB, TiKV, **FoundationDB**, etcd) — 50+ services total.
- **API shape:** a single `Operator` with composable *layers* (retry, timeout, logging, tracing, metrics, throttling, concurrency-limit) around a core trait; multipart/writer API included. Consistency-guarantee documentation and a "who uses this in production" list were not clearly surfaced in the README during this research — worth checking `opendal.apache.org/docs` directly before committing.
- **Interop bridge:** the `object_store_opendal` crate wraps any OpenDAL service behind the `ObjectStore` trait, so a project can standardize on `object_store` and still reach for an OpenDAL backend later without a rewrite.

### `aws-sdk-s3` (official AWS SDK for Rust)
- **Maturity/adoption:** v1.141.0 (2026-08-06), Apache-2.0, ~5.9–17M downloads/month. GitHub [`awslabs/aws-sdk-rust`](https://github.com/awslabs/aws-sdk-rust): ~3,300 stars, active. GA (1.0) since Nov 2023, generated from AWS's Smithy service models — full API-surface coverage including newer features (S3 Express One Zone, Tables, Object Lambda) that `object_store` doesn't expose.
- **Conditional writes:** `if_none_match("*")` available directly on `PutObject`/`CompleteMultipartUpload` builders.
- **Tradeoff:** full fidelity to every S3-specific feature at the cost of zero portability to GCS/Azure — a second client/abstraction is needed per cloud.

### GCS client: `google-cloud-storage`
- **Maturity/adoption:** v1.17.0 (2026-07-30), Apache-2.0, ~1.8–4.7M downloads/month. Part of the **official** [`googleapis/google-cloud-rust`](https://github.com/googleapis/google-cloud-rust) umbrella (~945 stars, maintained by Google employees + `cloud-sdk-rust-bot`), described by Google as API-stable. This supersedes the older community `cloud-storage` crate, which should be avoided in favor of this one. Smaller star count reflects GCS's smaller Rust mindshare overall, not lower quality.

### Tradeoffs: unified abstraction vs. provider SDKs

| Concern | `object_store` | `opendal` | Provider SDKs |
|---|---|---|---|
| Conditional writes / CAS | First-class `PutMode` abstraction, mapped per backend | Present per-backend, less centrally documented | Full native fidelity but re-implemented per cloud |
| Multipart upload | Built-in, uniform across clouds | Built-in, broadest backend list | Full native control, S3-only |
| Retry/backoff | Built-in, centrally configured | Composable "retry" layer | AWS SDK has strong native retry (Smithy standard); GCS crate less documented |
| Cross-cloud portability | Yes — ecosystem standard | Yes — broadest backend coverage of any option | No — one crate per cloud |
| Ecosystem fit | Same crate DataFusion/Delta Lake/Lance/Polars already use — relevant if the broker ever adds SQL-over-segments or Iceberg/Delta export | Broadest surface, more moving parts to reason about for a correctness-critical broker | High fidelity, duplicated effort |
| Known gaps | Conditional-put support is backend/config-dependent; some S3-compatible stores can't do atomic put-if-absent at all | Consistency guarantees not clearly documented | Multipart parts can't be conditioned (an S3-family constraint, not SDK-specific) |

**Recommendation shape:** `object_store` is the stronger default — it is purpose-built around the exact primitive a WarpStream-style commit protocol needs (atomic CAS, cross-cloud multipart, centralized retry) and its adoption inside DataFusion/delta-rs/Lance/Polars means any future tiered-storage or SQL-over-segments feature reuses the same trait. Keep `opendal` as a fallback for storage backends outside object_store's four (the `object_store_opendal` bridge keeps that door open), and provider SDKs as an escape hatch behind a thin trait for cloud-specific features.

---

## 2. Kafka wire protocol in Rust

### `kafka-protocol`
- **Maturity/adoption:** v0.17.0 (2025-11-27), MIT/Apache-2.0, ~28K–75K downloads/month (7.4M lifetime). GitHub, now under its own org [`kafka-protocol-rs/kafka-protocol-rs`](https://github.com/kafka-protocol-rs/kafka-protocol-rs) (moved from `tychedelia/kafka-protocol-rs`): ~115 stars, 168 commits, last push **2026-08-04** — small but genuinely active, and the move to a dedicated org is itself a maturity signal.
- **Code generation:** confirmed — messages are generated straight from Apache Kafka's own protocol JSON schema files (the same approach the official Java/Scala Kafka codebase uses), run via `cargo run -p protocol_codegen`. Generated messages currently target **Kafka 4.1.0**; exported items are `#[non_exhaustive]` for forward compatibility. Because coverage tracks upstream Kafka's own spec rather than being hand-maintained, it aims for the full API-version surface rather than a curated subset.
- **Record batch handling:** exposes `RecordBatchEncoder`/`RecordBatchDecoder` for the actual message-set wire format (the hard part), with per-message `VERSIONS` metadata for negotiating API versions per client.
- **Lineage & real users:** built on top of @Diggsey's "Franz" minimal client. Reverse-dependency graph (19 crates) includes **Shotover** (Instaclustr's production L7 database proxy, which lists Kafka as one of its supported wire protocols alongside Cassandra/Valkey) and **`rustfs-kafka`** (a Kafka-compatible ingestion front-end for [RustFS](https://github.com/rustfs/rustfs), a ~31k-star Apache-2.0 S3-compatible object-storage project) — the latter is a close precedent for exactly this project's "Kafka protocol in front of object storage" pattern.
- **Assessment:** this is essentially the only viable pure-Rust foundation for broker-side protocol work. It's a small maintainer base (bus-factor risk to plan for — likely need to contribute upstream for gaps), but the codegen-from-spec approach and recent push cadence are stronger health signals than its modest star count suggests.

### Contrast: `rdkafka`
v0.39.0, MIT, ~1.9M–33.7M downloads/month, [`fede1024/rust-rdkafka`](https://github.com/fede1024/rust-rdkafka) ~2,000 stars. This is an FFI binding over the C **librdkafka**, used for building Kafka *clients* — architecturally irrelevant to implementing broker-side protocol handling; it cannot be used to build the server end of the protocol. Mentioned only as the expected contrast: high adoption because it's the standard way Rust apps *consume from* Kafka, not because it helps build a broker.

---

## 3. Async runtime & networking

### `tokio`
v1.53.1 (2026-07-20), MIT, **~72–202M downloads/month** (871M+ lifetime) — by an order of magnitude the most-downloaded async runtime in Rust and the substrate nearly the entire async-network-services ecosystem (hyper, tonic, axum) is built on. GitHub [`tokio-rs/tokio`](https://github.com/tokio-rs/tokio): ~32,900 stars, active daily. De facto standard status is not really contestable.

### `tokio-util` and framing
`tokio-util` v0.7.19, MIT, ~53–149M downloads/month, provides `codec::{Encoder, Decoder, Framed}` turning an `AsyncRead + AsyncWrite` into a `Stream + Sink` of application frames, including a pre-built **`LengthDelimitedCodec`** — directly relevant since Kafka's wire protocol is a 4-byte big-endian length prefix followed by the message body. In practice a Kafka broker needs a thin custom `Decoder`/`Encoder` layered on top (to extract correlation IDs, negotiate per-API versions via `kafka-protocol`) rather than raw `LengthDelimitedCodec` alone, but it removes essentially all manual buffer/partial-read bookkeeping. The `bytes` crate (`Bytes`/`BytesMut`, ~72–120M downloads/month) underlies all of this and matters for avoiding repeated copies of record-batch payloads as they move socket → parse → object-store upload.

### Concurrency model for many connections
- **Task-per-connection** (`tokio::spawn` per accepted socket) is the natural default and what most Rust network servers use; Tokio scales well past 100k concurrent connections this way. Backpressure *within* a connection falls out of `Framed`'s bounded `Sink`/`Stream`; backpressure *between* connections and the object-store write path needs deliberate handling (bounded `mpsc` channels or semaphores gating in-flight uploads), since the real bottleneck in this architecture is object-store PUT/multipart throughput and request-rate limits, not socket I/O.
- Kafka allows request pipelining per connection (especially Produce/Fetch), so the per-connection handler typically needs to demultiplex correlation IDs and potentially respond out of order — a common pattern is splitting the socket into read/write halves with a dedicated response-writer task fed by `tokio::sync::mpsc`.
- The concurrency-critical design work specific to this architecture is the **batching/coalescing layer** — turning many small producer writes into fewer, larger object-store PUTs — which is bespoke broker logic on top of Tokio, not something any crate provides off the shelf.

---

## 4. Consensus/coordination options for metadata management

A WarpStream-style broker needs a strongly-consistent metadata layer (partition existence, batch ordering/offsets, leader/coordinator assignment) that is small relative to data-plane volume — WarpStream reports keeping data:metadata byte ratios above 10,000:1 and metadata-store utilization under 10% even at 4.5 GiB/s peak data-plane traffic. WarpStream's public posts describe the *properties* required (strongly consistent, extremely durable, highly available, low latency, quadruply replicated across 3 AZs per region) but **do not publicly name the underlying technology** — they explicitly say they considered and rejected CockroachDB/Spanner for cross-region use "because we had no previous experience with these technologies," implying a custom or different system, but this is not documented. Treat this as a genuine open question, not a fact. [warpstream.com/blog/the-case-for-shared-storage](https://www.warpstream.com/blog/the-case-for-shared-storage), [warpstream.com/blog/multiple-regions-single-pane-of-glass](https://www.warpstream.com/blog/multiple-regions-single-pane-of-glass)

### `openraft` (build-your-own Raft)
- v0.10.0-alpha.33 (2026-08-05, pre-1.0/API not yet stable), MIT/Apache-2.0, ~226K downloads/month. GitHub [`databendlabs/openraft`](https://github.com/databendlabs/openraft): ~2,000 stars, 130+ reverse dependencies.
- **Production users, several directly analogous to this project:** **Databend** (its meta-service cluster), **CnosDB** (distributed time-series DB), **RobustMQ** (a Rust "unified messaging engine" — MQTT/Kafka/NATS/AMQP over one broker with Raft-based meta-service, RocksDB/file storage, and **automatic S3 cold-tiering**; ~1.8k stars, Apache-2.0, Kafka support currently in development), **Walrus** (message streaming, Raft-based metadata), **RocketMQ-rust**, **Hiqlite**.
- **API shape:** pluggable `AsyncRuntime` trait (Tokio default, also Compio/Monoio), generalized/joint membership changes, reports 33K–5.6M writes/sec depending on batching, but the project itself flags chaos-testing as incomplete.
- **Tradeoff:** most control and lowest latency (in-process, no network hop) but highest operational burden — you own leader election, snapshotting, log compaction, membership changes, and monitoring as part of the broker's control plane.

### `etcd-client` (external etcd)
- v0.19.0 (2026-06-09), Apache-2.0/MIT, ~475K downloads/month, built on tonic/prost. Implements the full etcd v3 API: KV, Watch, Lease, Auth, Maintenance, Cluster, Lock, Election.
- **Tradeoff:** offloads consensus entirely to etcd itself — the same extremely battle-tested system underpinning Kubernetes (52k GitHub stars, Apache-2.0). Gives watch streams (useful for propagating partition/leader metadata to brokers), locks, and leader election out of the box instead of hand-building them on openraft. Cost: operate (or pay for) a separate etcd cluster; etcd's single-Raft-group model and ~8GB default quota may need sharding consideration at very large metadata volumes (unlikely to bind given the >10,000:1 data:metadata ratio target).

### FoundationDB (`foundationdb` crate)
- v0.11.0 (2026-06-24, pre-1.0/API "may be in constant flux"), MIT/Apache-2.0, ~95K downloads/month. GitHub [`foundationdb-rs/foundationdb-rs`](https://github.com/foundationdb-rs/foundationdb-rs): ~227 stars — modest compared to sqlx/tokio-postgres, but this reflects Rust-binding niche-ness, not the underlying database's maturity.
- **API shape:** futures-based wrapper over FoundationDB's C client, multi-crate (`foundationdb`, `foundationdb-sys`, `foundationdb-tuple`), supports FDB 5.1–7.4, runs FDB's own BindingTester correctness suite across thousands of seeds hourly.
- **Relevance:** FoundationDB itself is arguably the most battle-tested strongly-consistent transactional KV store on this list — it underpins Snowflake's metadata layer specifically because of serializable multi-key transactions plus its pioneering deterministic-simulation testing (see §9). The real cost is operational: running FDB yourself is a nontrivial distributed system (coordinators, process classes, cluster files), and managed FDB offerings are far less common than managed Postgres/DynamoDB.

### Simpler externally-managed stores
- **Postgres** via `sqlx` (v0.9.0, Apache-2.0/MIT, **~11M downloads/month — the most-adopted DB crate surveyed**, 5,220 reverse deps) or `tokio-postgres` (v0.7.18, ~5.4M downloads/month, 1,523 reverse deps): ACID/serializable transactions, decades of managed-HA tooling (RDS/Aurora/Cloud SQL).
- **DynamoDB** via `aws-sdk-dynamodb` (v1.120.0, Apache-2.0, ~1.66M downloads/month): strongly-consistent reads + transactional writes, fully managed, AWS-only.
- **Tradeoff:** lowest operational complexity of all four approaches — no consensus code to build or run yourselves — at the cost of an extra network hop per metadata operation and (for DynamoDB) single-cloud lock-in. For partition/offset metadata specifically (a small fraction of total data-plane bytes, per WarpStream's own numbers), this latency cost is likely acceptable, similar to how a managed metadata store is "fine" as long as it isn't on the hot data path.

### Comparison summary

| Approach | Operational complexity | Latency | Ecosystem maturity | Best fit when |
|---|---|---|---|---|
| openraft (DIY Raft) | High — own election, snapshotting, compaction | Lowest (in-process) | Pre-1.0 but proven in analogous streaming systems (RobustMQ, Walrus) | Want full control, willing to operate a Raft cluster as part of the broker itself |
| etcd-client (external etcd) | Medium — operate/consume etcd | Extra hop, but etcd optimized for small-KV + watch | Extremely mature server, mature client | Want watch/lock/election primitives without building them |
| FoundationDB | High to self-host; managed options scarce | Low, strictly serializable | Best-tested core DB; niche Rust binding | Want the strongest consistency pedigree and can operate FDB (or find a managed offering) |
| Postgres (sqlx) | Low with managed RDS/Aurora | Extra hop, ACID | Largest ecosystem of the five | Want the most pragmatic, "boring," ops-light default |
| DynamoDB | Lowest (fully managed) | Very low | Mature AWS SDK | Already AWS-committed, want zero ops |

---

## 5. Serialization

### `prost` + `tonic` (internal gRPC)
- `prost` v0.14.4, Apache-2.0, ~40.5M downloads/month, 12,499 reverse deps — the standard Protobuf codegen for Rust, though lib.rs flags it **"passively maintained"** (bug/security fixes only; the maintainers expect the official protobuf project's own Rust library may eventually supersede it — worth watching, not urgent).
- `tonic` v0.14.6, MIT, ~28.3M downloads/month, 7,175 reverse deps — maintained by the **Hyperium** org (same group behind hyper/h2/tower), built on Tokio + Hyper 1.0 + h2 + Tower, pluggable TLS. Notably, `etcd-client` itself is built on prost+tonic — reinforcing this as the expected toolchain for Rust inter-service RPC (broker↔metadata-service calls, partition assignment, leader-epoch propagation, heartbeats).

### `serde` + `bincode` — **bincode flagged unmaintained, verified**
- `serde` v1.0.229, MIT/Apache-2.0, ~97M downloads/month — the universal serialization framework, no concerns.
- **`bincode` is confirmed unmaintained** as of this research: the GitHub repo [`bincode-org/bincode`](https://github.com/bincode-org/bincode) was **archived by its owner on 2025-08-15**, with maintainers stating development moved to SourceHut over objections to "GitHub's rampant and inherently immoral integration of generative AI." Separately, **RUSTSEC-2025-0141** (issued 2026-01-07, type INFO/Unmaintained, applies to all versions) states development ceased permanently following **"a doxxing and harassment incident,"** designates v1.3.3 as the last complete release, and confirms **no future patches**. The v3.0.0 crates.io release (2025-12-16) intentionally contains only a README and a compiler error to force users to notice. **Recommendation: do not adopt bincode for new work.** RUSTSEC's suggested replacements: `wincode` (drop-in), `postcard`, `bitcode`, `rkyv`.
- **`postcard`** (v1.1.3, MIT/Apache-2.0, ~7.08M downloads/month, 5,576 reverse deps): serde-compatible, `no_std`-first, documented stable wire format (Mozilla-sponsored spec work) — the closest drop-in bincode replacement for compact metadata encoding.
- **`rkyv`** (v0.8.18, MIT, ~11.6M downloads/month, 2,736 reverse deps): true zero-copy deserialization — data is used directly from the archived buffer with no deserialize step, with a safe validated path via `bytecheck`. Best fit where hot-path performance matters (e.g., reading back cached partition metadata or replaying WAL segments without a parse step).

### `flatbuffers`
v25.12.19, Apache-2.0, ~6.97M downloads/month, maintained under the Google flatbuffers org — but **Rust support is explicitly flagged "experimental"** with possible API changes between minor versions. Enables zero-copy field access straight out of a wire buffer without a full deserialize pass.
- **vs. bincode/postcard:** flatbuffers avoids materializing full owned structs — valuable for large or selectively-read record-batch metadata.
- **vs. prost:** prost/protobuf fits whole-message RPC (control-plane calls); flatbuffers fits avoiding the parse/copy step for data embedded in object-storage segment files.
- **vs. rkyv:** both zero-copy, but flatbuffers is schema-first/cross-language (useful if a non-Rust client library ever needs to read the same segment-index format), while rkyv is Rust-only, more idiomatic, and more actively iterated — flatbuffers' "experimental" Rust label is a real maturity gap by comparison.
- **Relevance:** best suited to record-batch index/metadata embedded in object-storage segments where partial zero-copy reads and eventual cross-language compatibility matter; for purely internal Rust-to-Rust metadata (in-memory partition maps, Raft log entries), `rkyv` or `postcard` are simpler choices.

---

## 6. Observability

### `tracing`
v0.1.44 (2025-12-18), MIT, ~60M downloads/month (387M+ lifetime for the ecosystem overall). GitHub [`tokio-rs/tracing`](https://github.com/tokio-rs/tracing): ~6,800 stars, maintained by the Tokio core team though it doesn't require the Tokio runtime. Spans + structured events (vs. plain log lines), `#[instrument]` macro, pluggable `Subscriber`/`Layer` model; layers exist for Honeycomb, Sentry, Loki, CloudWatch, and OpenTelemetry (`tracing-opentelemetry`). This is the standard choice for structured logs/spans across a broker's request paths (produce/fetch/S3 upload) and for correlating a request across async tasks.

### `metrics` (metrics-rs)
v0.24.6 (2026-05-13), MIT, ~6.2M downloads/month. GitHub `metrics-rs/metrics` ~1.5k stars. A facade crate (analogous to `log` but for metrics) — libraries emit `counter!`/`gauge!`/`histogram!` macros, the app wires up a backend at startup. Standard companion: `metrics-exporter-prometheus` (~37M lifetime downloads, MIT/Apache-2.0, latest release ~2 months old at research time). This is the operational norm for exposing broker metrics (produce/fetch throughput, S3 PUT/GET latency, cache hit rate, replication lag) to Prometheus/Grafana, which is the expected ops surface for Kafka-compatible systems.

### OpenTelemetry Rust
`opentelemetry` v0.32.0 (2026-05-08), Apache-2.0, ~18M downloads/month despite **not yet being fully 1.0**: Metrics and Logs API/SDK are stable, but the **Traces API/SDK remains Beta** (the "Tracing API Stable" milestone was only ~32% complete as of mid-2025). GitHub [`open-telemetry/opentelemetry-rust`](https://github.com/open-telemetry/opentelemetry-rust): ~2,700 stars, active but still shipping breaking changes as APIs graduate from experimental flags. **Recommendation:** adopt now via `tracing-opentelemetry` + `opentelemetry-otlp` for cross-node distributed tracing, but pin versions and isolate the wiring behind a thin internal module — expect API churn until tracing stabilizes. (`foyer`, among others, already ships optional OTel export, showing this is a normal pairing in this ecosystem.)

**Bottom line:** `tracing` + `metrics`/`metrics-exporter-prometheus` is the safe, production-proven combination; layer OpenTelemetry on top for distributed tracing with the caveat that it's "adopt with version pinning," not "fully stabilized," as of 2025/2026.

---

## 7. Existing Rust-native streaming/queue projects (prior art)

### Fluvio (InfinyOn)
[`infinyon/fluvio`](https://github.com/infinyon/fluvio), ~5.2k stars, Apache-2.0, active since 2021. **SC/SPU architecture**: a central System Controller (`fluvio-sc`) handles cluster/topic/partition metadata and rebalancing; Stream Processing Units (`fluvio-spu`) are the data plane, handling leader/follower replication and WASM-based "SmartModule" in-stream transforms. **Storage is local-disk based**, not object-storage backed — the standalone `fluvio-storage` crate appears stale (last release Feb 2021), suggesting storage logic folded into `fluvio-spu` over time. **Protocol is custom, not Kafka wire-compatible** — `fluvio-protocol` (Apache-2.0, latest v0.50.1 Jul 2025, ~7.6K downloads/month) implements Fluvio's own binary protocol over `fluvio-socket`, meaning Fluvio's client ecosystem is not a Kafka drop-in. **Relevance:** useful reference for control-plane/data-plane separation (SC vs. SPU maps conceptually onto a metadata/coordination layer vs. broker-storage layer), and the WASM SmartModule pattern is a possible reference for stream transforms — but its protocol codec and local-disk storage design are not directly reusable for a Kafka-wire + object-storage broker.

### Iggy / Apache Iggy (Incubating)
[`apache/iggy`](https://github.com/apache/iggy) (formerly `iggy-rs/iggy`), ~4.5k stars, Apache-2.0, accepted into the **Apache Incubator in Feb 2025**. **Architecture:** thread-per-core, shared-nothing design built on `io_uring` via the `compio` runtime (moved off pure Tokio for the hot path), each core pinned via `sched_setaffinity`, inter-shard messaging via bounded `crossfire` channels. **Protocol is custom, not Kafka-compatible** — four transports: QUIC (`quinn`), custom TCP, HTTP/REST, WebSocket. **Storage design is the most directly reusable prior art found in this survey:**
- Segment-based layout mirroring Kafka's own approach: `local_data/streams/{id}/topics/{id}/partitions/{id}/{offset}.index` + `{offset}.log`, default 1 GiB segments.
- Fixed 64-byte little-endian message header (checksum, unique ID, offset, timestamp, payload length).
- **Three configurable index-caching strategies** (all-in-memory / active-segment-only / on-demand disk read) — a directly transferable pattern for how a broker might cache local indexes in front of S3-resident segments.
- Supports backup/archival **to** S3-compatible storage as a cold tier — the inverse of a WarpStream design where S3 is primary, but evidence the segment format is already storage-agnostic in spirit.
- Serialization stack includes `postcard`, `prost`, `rmp-serde`, and `flatbuffers`, plus `arrow`/`parquet`/`apache-avro` for interop — a useful signal for which serialization crates a comparable Rust streaming project actually reaches for in practice.
**Relevance:** the segment `.log`/`.index` split, fixed binary header, and tiered index-caching strategy are worth studying directly when designing local WAL/cache segments in front of S3 or per-partition S3 object naming/indexing. The thread-per-core + io_uring architecture is a strong reference if extreme single-node throughput is a goal, but is a much bigger architectural commitment than a conventional multi-threaded Tokio design.

### Rust "Kafka on object storage" attempts (the closest direct analogs to this project)
- **Tansu** ([`tansu-io/tansu`](https://github.com/tansu-io/tansu), ~1.8k stars, Apache-2.0): a **stateless, genuinely Kafka-wire-protocol-compatible** broker in Rust, single static binary, with **pluggable storage engines — PostgreSQL, libSQL/SQLite, S3, or in-memory** — and schema-backed topics that can write directly into **Apache Iceberg or Delta Lake** tables. Demonstrated compatible with stock `kafka-topics`/`kafka-console-producer`/`kafka-console-consumer` CLIs. This is the closest known Rust analog to WarpStream's actual thesis (stateless brokers, object storage as source of truth, real Kafka protocol) and is worth studying directly as prior art before designing this project's architecture.
- **StreamHouse** ([`gbram1/streamhouse`](https://github.com/gbram1/streamhouse), ~68 stars, Apache-2.0): a much younger, smaller "S3-native event streaming, one binary replaces Kafka" project — implements 23 Kafka APIs, dual durability modes (~1ms buffered vs. ~150ms S3-durable), claims benchmarks of 2.21M records/sec WAL writes and 769K records/sec full S3 path. Early-stage; worth watching, not yet a mature reference.
- **RobustMQ** ([`robustmq/robustmq`](https://github.com/robustmq/robustmq), ~1.8k stars, Apache-2.0): a unified multi-protocol broker (MQTT/Kafka/NATS/AMQP/mq9 sharing one storage layer), with a Raft-based Meta Service (`openraft`), pluggable storage (Memory/RocksDB/File) and stated support for **"automatic cold data tiering to S3."** Kafka protocol compatibility is explicitly listed as in-progress/roadmap, not production-ready yet, but its metadata-consensus + S3-tiering architecture is directly relevant design prior art (see §4).
- Broader market context (non-Rust, for positioning only): WarpStream itself is proprietary Go (now part of Confluent); other "diskless Kafka" efforts — AutoMQ, Aiven Inkless (KIP-1150), StreamNative Ursa — are JVM/Kafka *forks*, not Rust rewrites, reinforcing that a genuinely Kafka-wire-compatible Rust broker on object storage is still a comparatively open niche (Tansu being the most mature current occupant).

---

## 8. Caching layer options

### `moka`
v0.12.16 (2026-08-09), MIT/Apache-2.0 (with `frequency_sketch.rs`/`timer_wheel.rs` Apache-2.0-only, ported from Caffeine), ~11.5M downloads/month, 3,595 reverse deps, **#1 in crates.io's Caching category**. GitHub [`moka-rs/moka`](https://github.com/moka-rs/moka) ~2,700 stars. Two flavors: `moka::sync::Cache` (blocking, thread-safe) and `moka::future::Cache` (async, Tokio/async-std/actix-rt compatible), using **Window-TinyLFU admission** (LFU-based admission + LRU eviction) for near-optimal hit ratios under skewed access. Notable adopter: **crates.io itself** uses Moka in its API service (~85% hit rate). **Relevance:** best fit for a pure in-memory cache — partition metadata, consumer-group state, hot small index blocks in front of S3 GETs. TinyLFU admission suits Kafka-like access patterns (recent/tail-of-log data read far more than cold history).

### `foyer` (foyer-rs/foyer) — the most directly on-point crate found in this whole survey
v0.22.3 (2026-01-23), Apache-2.0, ~726K downloads/month. GitHub [`foyer-rs/foyer`](https://github.com/foyer-rs/foyer) ~1.8k stars. Built by the **RisingWave** team specifically to unify memory and disk caching for large-object, object-storage-backed workloads (RisingWave's own state store is object-storage-backed). **API shape:** `HybridCacheBuilder` configures memory and disk capacity independently; in-memory tier is sync/zero-copy, hybrid (disk-inclusive) tier is async; pluggable eviction (LRU with high-priority pool ratio, FIFO-based disk region picker, TinyLFU/S3-FIFO-style admission, custom filters/reinsertion), configurable I/O engine and LZ4 compression, built-in Prometheus/Grafana/OpenTelemetry/Jaeger integration. **Known adopters directly relevant here:** RisingWave, **SlateDB** (a cloud-native embedded storage engine that is itself S3-object-store-backed — architecturally close to what this project needs at the storage layer), Chroma, ZeroFS (filesystems on S3), Percas, and a project explicitly named "Cachey" described as an "object storage read-through cache" — i.e., someone already used foyer to solve essentially this project's exact caching problem. **Relevance:** caching fetched S3 segments in memory with disk spillover under memory pressure, plus admission control to avoid cache pollution from one-off scan reads, is exactly foyer's stated design target.

### Other options
Fronting S3 with an `object_store` `LocalFileSystem` instance as a naive write-through/read-through cache is possible but has no built-in eviction/admission policy (hand-rolled TTL/size reaping required) — reasonable for a first cut, but foyer/moka provide eviction policy, metrics, and concurrency handling for free. Lightweight single-tier crates (`lru`, `quick_cache`) exist but don't match moka's concurrent throughput or foyer's hybrid disk-spillover design.

**Recommended combination:** `moka` for hot in-memory metadata/index caching + `foyer` for the hybrid large-segment S3 read cache — not a bespoke LRU.

---

## 9. Testing/simulation tooling for distributed-systems correctness

### `madsim`
v0.2.34 (2025-10-11), Apache-2.0, ~169K downloads/month. GitHub [`madsim-rs/madsim`](https://github.com/madsim-rs/madsim) ~1.1k stars, 343 commits. **How it works:** a drop-in replacement *runtime* for Tokio (not a wrapper) — compiled under a `madsim` cfg it swaps Tokio, Tonic, `etcd-client`, `rdkafka`, `aws-sdk-s3` (plus patched `quanta`/`getrandom`/`tokio-postgres` forks) for simulation-aware equivalents with identical APIs. A seeded deterministic RNG, a virtual-time priority-queue scheduler (runs at large multiples of wall-clock speed), a deliberately "first-in-random-out" task scheduler to expose concurrency bugs, and environment simulators for network/disk and even **Kafka, etcd, and S3 themselves** let an entire multi-node cluster run deterministically inside one OS thread — any failing seed replays exactly. **Confirmed production user: RisingWave** (documented, flagship user, full end-to-end simulation including simulated Kafka/S3/etcd). MadRaft is the other named consumer. Note: this research could **not confirm TiKV uses madsim** (a GitHub code search turned up nothing) — treat that specific claim as unconfirmed.

### `turmoil`
v0.7.2 (2026-04-24), MIT, ~186K downloads/month. GitHub [`tokio-rs/turmoil`](https://github.com/tokio-rs/turmoil) ~1.2k stars, maintained by the Tokio team. A narrower approach than madsim: runs multiple simulated "hosts" concurrently on a single thread with deterministic replacements specifically for networking (`turmoil-net`, a `tokio::net`-compatible stack), filesystem (`turmoil-fs`), and io_uring (`turmoil-io-uring`) — injecting latency/packet-loss/partitions/crashes/torn-writes manually or via seeded randomness. Because it doesn't replace the scheduler the way madsim does, it's lighter-weight and easier to bolt onto an existing Tokio codebase, at the cost of not achieving madsim's all-encompassing scheduling determinism. This research could not confirm Iroh currently depends on turmoil (code search came up empty) — treat as unconfirmed.

### `loom`
v0.7.2 (2024-04-23 — notably staler release cadence than madsim/turmoil), MIT, ~4M downloads/month, 3,424 reverse deps. GitHub [`tokio-rs/loom`](https://github.com/tokio-rs/loom) ~2,800 stars. A different category entirely: exhaustive **thread-interleaving/memory-model permutation testing** for low-level concurrent primitives (lock-free structures, custom `Mutex`/`Arc`-likes) under the C11 memory model, not a distributed-systems simulator. Documented limitations (no full SeqCst support, potential false positives). **Relevance to this project:** validating any hand-rolled concurrent structures (ring buffers, lock-free queues, write-ahead-buffer synchronization) — a different job from madsim/turmoil's protocol-level correctness testing.

### Why deterministic simulation testing matters specifically for a Kafka-on-object-storage broker
- **Reproducing rare races deterministically:** commit/consensus bugs (in a Raft metadata layer, or the leader-epoch/offset-commit protocol) are often only triggered by rare interleavings of network delay, disk latency, and concurrent requests — a bug that took days to hit in production can be replayed from one seed in milliseconds.
- **Network partition/retry testing:** brokers must handle reconnects, leader failover, and metadata propagation correctly under partitions — turmoil's explicit fault injection and madsim's network simulator let you assert no-data-loss/no-split-brain under adversarial conditions without real multi-node clusters.
- **Simulating S3 failures/retries deterministically:** madsim ships simulators specifically for **S3-like services** (plus Kafka and etcd) — directly applicable here: deterministically inject PUT failures, slow multipart uploads, or retry-storm scenarios, and verify the commit/compaction protocol never loses or duplicates data, far more cheaply and exhaustively than testing against real S3/LocalStack.
- **Catching commit-protocol bugs:** the core hard problem in a Kafka-on-object-storage system is deciding what's durably committed (which S3 PUTs are visible as committed offsets) — exactly the class of algorithm this testing style is built for: thousands of nightly seeds with heavy fault injection (crash mid-commit, PUT failure after the object landed but before ack, leader flip during a commit).

**Lineage — FoundationDB as the direct ancestor of this whole category:** FDB's Flow actor language + simulator runs an entire cluster deterministically inside a single thread, compressing time and injecting failures at network/machine/datacenter granularity, running "tens of thousands" of simulations nightly (an estimated ~1 trillion cumulative CPU-hours). FDB engineers have stated it's unlikely they could have built FoundationDB without this technology. Both madsim's and turmoil's own READMEs cite FoundationDB explicitly as the inspiration. [apple.github.io/foundationdb/testing.html](https://apple.github.io/foundationdb/testing.html)

**TigerBeetle** (Zig, not Rust, but the most-cited modern industry example) independently arrived at the same pattern with its **VOPR** fuzzer — a deterministic simulator that runs an entire cluster with injected network/storage/process faults at ~1000x real-time across large CPU-core fleets, also distributed as a playable "game." [docs.tigerbeetle.com/concepts/safety](https://docs.tigerbeetle.com/concepts/safety/)

**WarpStream:** no public engineering writing describing a madsim/turmoil/VOPR-equivalent internal testing framework was found — treat as an open/unconfirmed question, not a documented fact, rather than assuming they do or don't use one.

**Recommendation:** given the commit-protocol correctness stakes of this project, `madsim` is the stronger fit than `turmoil` specifically because it already ships S3/etcd/Kafka-aware simulators — RisingWave's precedent (a comparable Rust streaming system built on object storage) using madsim for full end-to-end simulation is a strong, directly relevant validation of this approach. `loom` remains complementary for any hand-rolled lock-free structures underneath.

---

## 10. Licensing survey

All crates surveyed are **MIT and/or Apache-2.0** — the standard permissive dual-license norm in the Rust ecosystem — with **no AGPL/BSL/SSPL/GPL-licensed crates found**. The one real flag is **maintenance status**, not license terms: `bincode` (MIT) is confirmed unmaintained (RUSTSEC-2025-0141) and its GitHub mirror archived — avoid for new work despite historically high adoption. A few others are pre-1.0/API-unstable (`openraft`, `foundationdb`) or "passively maintained" (`prost`) — fine to depend on but with an eye on churn/bus-factor. Full list in the summary table below.

---

## Summary table

| Crate | Purpose | Maturity (as of 2026-08) | License |
|---|---|---|---|
| `object_store` | Unified async object storage client (S3/GCS/Azure/local) | Very mature — Apache Arrow project, ~6–18M dl/mo, ecosystem standard (DataFusion/Delta/Polars/Lance) | MIT/Apache-2.0 |
| `opendal` | Unified storage abstraction, 50+ backends | Mature — Apache top-level project, ~5.3k★, broader backend coverage than object_store | Apache-2.0 |
| `aws-sdk-s3` | Official AWS S3 SDK | Mature — GA since 2023, ~5.9–17M dl/mo | Apache-2.0 |
| `google-cloud-storage` | Official GCS client | Mature, actively maintained by Google | Apache-2.0 |
| `kafka-protocol` | Kafka wire protocol encode/decode, codegen from spec | Small but active (~115★, pushed within days of research) — the only real pure-Rust option | MIT/Apache-2.0 |
| `rdkafka` | librdkafka FFI bindings (client only, not broker-side) | Very mature, ~2k★ | MIT |
| `tokio` | Async runtime | De facto standard, ~32.9k★, ~72–202M dl/mo | MIT |
| `tokio-util` | Codecs/framing (`LengthDelimitedCodec`) | Mature, tightly coupled to tokio | MIT |
| `openraft` | Raft consensus for DIY metadata layer | Pre-1.0 (alpha) but proven in analogous streaming systems (RobustMQ, Walrus, Databend) | MIT/Apache-2.0 |
| `etcd-client` | Client for external etcd metadata store | Mature client for an extremely mature server | Apache-2.0/MIT |
| `foundationdb` | FoundationDB bindings for metadata store | Pre-1.0 Rust binding over a very mature database (Snowflake-grade) | MIT/Apache-2.0 |
| `sqlx` | Postgres/MySQL/SQLite async client | Very mature, most-adopted DB crate surveyed (~11M dl/mo) | Apache-2.0/MIT |
| `aws-sdk-dynamodb` | DynamoDB client for managed metadata store | Mature, active AWS SDK cadence | Apache-2.0 |
| `prost` | Protobuf codegen | Mature but "passively maintained" | Apache-2.0 |
| `tonic` | gRPC framework | Mature, Hyperium-maintained | MIT |
| `serde` | Serialization framework | Universal standard, ~97M dl/mo | MIT/Apache-2.0 |
| `bincode` | Binary serialization | **Unmaintained (RUSTSEC-2025-0141), GitHub archived 2025-08-15 — avoid** | MIT |
| `postcard` | Compact serde-based binary format (bincode alternative) | Mature, actively maintained | MIT/Apache-2.0 |
| `rkyv` | Zero-copy serialization | Mature, actively maintained, ~11.6M dl/mo | MIT |
| `flatbuffers` | Zero-copy cross-language serialization | Mature format, Rust bindings flagged "experimental" | Apache-2.0 |
| `tracing` | Structured logging/spans | De facto standard, ~6.8k★, ~60M dl/mo | MIT |
| `metrics` | Metrics facade | Mature, standard w/ `metrics-exporter-prometheus` | MIT |
| `opentelemetry` | Distributed tracing/metrics SDK | Metrics/Logs stable, **Traces still Beta** as of 2025/2026 | Apache-2.0 |
| `moka` | In-memory cache (TinyLFU) | Very mature, ~2.7k★, #1 in crates.io Caching category | MIT/Apache-2.0 (2 files Apache-2.0-only) |
| `foyer` | Hybrid memory+disk cache | Mature, purpose-built for this exact use case by RisingWave team | Apache-2.0 |
| `madsim` | Deterministic simulation testing (full runtime replacement, incl. S3/Kafka/etcd sims) | Proven in RisingWave production testing; ~1.1k★ | Apache-2.0 |
| `turmoil` | Deterministic network/filesystem simulation | Lighter-weight tokio-team tool, ~1.2k★ | MIT |
| `loom` | Concurrency permutation testing (not distributed sim) | Mature but slower release cadence, ~2.8k★ | MIT |
| Fluvio (`fluvio-protocol`) | Prior-art Rust streaming platform (custom protocol, local disk) | Mature, ~5.2k★ | Apache-2.0 |
| Iggy (`apache/iggy`) | Prior-art Rust streaming platform (custom protocol, segment storage) | Apache Incubating, ~4.5k★, actively released | Apache-2.0 |
| Tansu | Closest Rust "Kafka on object storage" prior art (real Kafka protocol, S3/Postgres pluggable storage) | ~1.8k★, active | Apache-2.0 |
| RobustMQ | Prior art: openraft metadata + S3 cold tiering, multi-protocol incl. in-progress Kafka | ~1.8k★, active | Apache-2.0 |

---

## Sources

- https://www.warpstream.com/blog/kafka-is-dead-long-live-kafka
- https://www.warpstream.com/blog/the-case-for-shared-storage
- https://www.warpstream.com/blog/multiple-regions-single-pane-of-glass
- https://www.warpstream.com/blog
- https://crates.io/crates/object_store, https://github.com/apache/arrow-rs-object-store, https://docs.rs/object_store/latest/object_store/, https://docs.rs/object_store/latest/object_store/enum.PutMode.html, https://docs.rs/object_store/latest/object_store/aws/enum.S3ConditionalPut.html, https://www.influxdata.com/blog/rust-object-store-donation/
- https://aws.amazon.com/about-aws/whats-new/2024/08/amazon-s3-conditional-writes/
- https://crates.io/crates/opendal, https://github.com/apache/opendal, https://lib.rs/crates/opendal
- https://crates.io/crates/aws-sdk-s3, https://github.com/awslabs/aws-sdk-rust, https://lib.rs/crates/aws-sdk-s3
- https://crates.io/crates/google-cloud-storage, https://github.com/googleapis/google-cloud-rust, https://lib.rs/crates/google-cloud-storage
- https://crates.io/crates/kafka-protocol, https://github.com/kafka-protocol-rs/kafka-protocol-rs, https://lib.rs/crates/kafka-protocol, https://github.com/shotover/shotover-proxy, https://github.com/rustfs/rustfs
- https://crates.io/crates/rdkafka, https://github.com/fede1024/rust-rdkafka, https://lib.rs/crates/rdkafka
- https://github.com/tokio-rs/tokio, https://lib.rs/crates/tokio
- https://lib.rs/crates/tokio-util, https://docs.rs/tokio-util/latest/tokio_util/codec/index.html
- https://github.com/databendlabs/openraft, https://lib.rs/crates/openraft
- https://lib.rs/crates/etcd-client, https://github.com/etcd-io/etcd
- https://lib.rs/crates/foundationdb, https://github.com/foundationdb-rs/foundationdb-rs
- https://lib.rs/crates/sqlx, https://lib.rs/crates/tokio-postgres, https://lib.rs/crates/aws-sdk-dynamodb
- https://github.com/robustmq/robustmq
- https://lib.rs/crates/prost, https://lib.rs/crates/tonic
- https://lib.rs/crates/serde, https://lib.rs/crates/bincode, https://rustsec.org/advisories/RUSTSEC-2025-0141.html, https://github.com/bincode-org/bincode, https://lib.rs/crates/postcard, https://lib.rs/crates/rkyv
- https://lib.rs/crates/flatbuffers, https://github.com/google/flatbuffers
- https://github.com/tokio-rs/tracing, https://lib.rs/crates/tracing, https://tokio-rs.github.io/tracing/
- https://github.com/metrics-rs/metrics, https://lib.rs/crates/metrics, https://crates.io/crates/metrics-exporter-prometheus
- https://github.com/open-telemetry/opentelemetry-rust, https://lib.rs/crates/opentelemetry, https://lib.rs/crates/opentelemetry-otlp
- https://github.com/infinyon/fluvio, https://www.fluvio.io/docs/fluvio/concepts/architecture/overview, https://lib.rs/crates/fluvio-protocol, https://lib.rs/crates/fluvio-storage
- https://github.com/apache/iggy, https://iggy.apache.org/, https://iggy.apache.org/docs/introduction/architecture/, https://cwiki.apache.org/confluence/display/INCUBATOR/Iggy+Proposal, https://lib.rs/crates/iggy
- https://github.com/tansu-io/tansu
- https://github.com/gbram1/streamhouse
- https://www.automq.com/blog/top-open-source-diskless-kafka-alternatives
- https://github.com/moka-rs/moka, https://lib.rs/crates/moka
- https://github.com/foyer-rs/foyer, https://lib.rs/crates/foyer, https://foyer-rs.github.io/foyer/docs/case-study/risingwave, https://blog.mrcroxx.com/posts/foyer-a-hybrid-cache-in-rust-past-present-and-future/
- https://github.com/madsim-rs/madsim, https://lib.rs/crates/madsim, https://www.risingwave.com/blog/deterministic-simulation-a-new-era-of-distributed-system-testing/
- https://github.com/tokio-rs/turmoil, https://lib.rs/crates/turmoil
- https://github.com/tokio-rs/loom, https://lib.rs/crates/loom
- https://apple.github.io/foundationdb/testing.html
- https://docs.tigerbeetle.com/concepts/safety/, https://github.com/tigerbeetle/tigerbeetle
