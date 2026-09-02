---
title: "Kafka Wire Protocol Compatibility: A Research Reference"
slug: kafka-protocol-compatibility
status: draft
last_updated: 2026-08-12
tags: [kafka-protocol, wire-format, record-batch, consumer-groups, kip-405, kip-1150, kip-848, transactions, rust-crates]
related: [01-warpstream-architecture, 05-rust-ecosystem, 06-distributed-systems-design-challenges]
summary: >
  What it takes to build a Kafka-wire-protocol-compatible broker from scratch:
  API keys and version negotiation, the RecordBatch v2 byte layout, consumer
  group protocols (classic + KIP-848), producer semantics (idempotence,
  transactions), KIP-405 Tiered Storage vs. the KIP-1150/1163/1164/1165
  "Diskless Topics" family, and the Rust crates available for the wire layer.
---

# Building a Kafka-Wire-Protocol-Compatible Broker: Research for a Rust, Object-Storage-Backed Implementation

This document surveys what is required to build a broker that speaks the Apache Kafka wire protocol well enough to interoperate with real Kafka clients (librdkafka, the Java client, Sarama/franz-go, etc.), with an emphasis on the design questions relevant to an S3/object-storage-backed system in the vein of WarpStream, Bufstream, and AutoMQ. It draws on Apache Kafka's official protocol documentation, the Kafka Improvement Proposal (KIP) process, and engineering write-ups from teams that have already built Kafka-protocol-compatible systems from scratch.

---

## 1. Kafka Wire Protocol Fundamentals

### 1.1 Framing

Every Kafka request and response on the wire is a single TCP-framed message: a 4-byte big-endian `int32` length prefix, followed by that many bytes of message body. As the protocol guide puts it, "the client can read requests by first reading this 4 byte size as an integer N, and then reading and parsing the subsequent N bytes" — the length field itself is not included in the count ([Apache Kafka Protocol Guide](https://kafka.apache.org/43/design/protocol/)). Connections are persistent TCP connections, and while the protocol is logically request/response, real clients pipeline multiple in-flight requests per connection (bounded by `max.in.flight.requests.per.connection`) rather than waiting for each response before sending the next request ([Apache Kafka Protocol Guide](https://kafka.apache.org/43/design/protocol/)). All multi-byte numeric fields are big-endian.

### 1.2 Request/response header versions

Every request begins with a common header, whose shape depends on whether the specific API+version being invoked is a "flexible version" (see §1.5):

- **Header v0** — `api_key(int16), api_version(int16), correlation_id(int32)`, no `client_id`. Effectively retired; used only by long-deprecated request/version combinations.
- **Header v1** — the header used by almost all requests historically: `request_api_key(int16), request_api_version(int16), correlation_id(int32), client_id(nullable string)`.
- **Header v2** (KIP-482, flexible-version header) — the same fields as v1, but with a trailing tagged-fields section appended.

Response headers are simpler: **v0** is just `correlation_id(int32)` used to match a response to its outstanding request on the connection; **v1** adds a trailing tagged-fields buffer for flexible responses ([RequestHeader.json / ResponseHeader.json, apache/kafka](https://github.com/apache/kafka/tree/trunk/clients/src/main/resources/common/message)). One deliberate wrinkle worth flagging for implementers: `ApiVersionsResponse` is always emitted with response header **v0**, even when its own body is a flexible version (v3+) — this sidesteps the chicken-and-egg problem of a client needing to parse a response before it has learned whether the broker speaks flexible framing at all.

### 1.3 API keys

Kafka identifies each RPC by a 16-bit `ApiKey`. The complete registry (current trunk) runs from 0 to 92+; the subset a Kafka-compatible broker actually needs to implement is far smaller. Key entries, numbered ([Apache Kafka Protocol Guide](https://kafka.apache.org/41/design/protocol/)):

| # | API | # | API | # | API |
|---|-----|---|-----|---|-----|
|0|Produce|17|SaslHandshake|32|DescribeConfigs|
|1|Fetch|18|ApiVersions|33|AlterConfigs|
|2|ListOffsets|19|CreateTopics|36|SaslAuthenticate|
|3|Metadata|20|DeleteTopics|37|CreatePartitions|
|8|OffsetCommit|21|DeleteRecords|42|DeleteGroups|
|9|OffsetFetch|22|InitProducerId|44|IncrementalAlterConfigs|
|10|FindCoordinator|23|OffsetForLeaderEpoch|68|ConsumerGroupHeartbeat|
|11|JoinGroup|24|AddPartitionsToTxn|69|ConsumerGroupDescribe|
|12|Heartbeat|25|AddOffsetsToTxn| | |
|13|LeaveGroup|26|EndTxn| | |
|14|SyncGroup|27|WriteTxnMarkers| | |
|15|DescribeGroups|28|TxnOffsetCommit| | |
|16|ListGroups|29-31|DescribeAcls/CreateAcls/DeleteAcls| | |

The higher-numbered keys (68+) are recent additions: KIP-848's new consumer-group protocol, telemetry, share groups (KIP-932), the streams-group protocol, and KRaft/Raft-voter APIs — mostly irrelevant to a minimum-viable broker.

### 1.4 ApiVersions and version negotiation

`ApiVersions` (key 18) is the linchpin of the whole protocol's extensibility story. The negotiation handshake works as follows ([Apache Kafka Protocol Guide](https://kafka.apache.org/43/design/protocol/); [KIP-511](https://cwiki.apache.org/confluence/display/KAFKA/KIP-511:+Collect+and+Expose+Client's+Name+and+Version+in+the+Brokers)):

1. Immediately after opening a TCP connection (and completing TLS if used), the client sends an `ApiVersionsRequest`. Versions 0–2 of this request carry no body fields at all — it must be decodable even by very old brokers, which is why its early versions are minimal.
2. The broker responds with `ErrorCode` plus an array of every `ApiKey` it supports, each with a `[MinVersion, MaxVersion]` range — sent "regardless of current authentication state," i.e. before SASL completes.
3. The client intersects its own supported range for each API against the broker's advertised range and picks the highest mutually-supported version for every subsequent call. The docs' own guidance: "clients are recommended to use the latest version supported by the broker and itself."
4. **Version-mismatch fallback**: for brokers ≥ 2.4.0, if a client sends an `ApiVersionsRequest` at a version newer than the broker understands, the broker responds with a **version-0** `ApiVersionsResponse` carrying `UNSUPPORTED_VERSION`, letting the client retry at a version it knows the broker can parse.
5. `ApiVersionsRequest` v3+ (KIP-511) adds `ClientSoftwareName`/`ClientSoftwareVersion` fields purely for broker-side telemetry/logging of which client libraries are connecting — useful for a Rust broker's own observability.
6. For SASL-authenticated connections, clients typically issue `SaslHandshakeRequest` (key 17, negotiates the mechanism string) → mechanism-specific `SaslAuthenticateRequest` exchange (key 36) → normal traffic; `SaslHandshakeRequest` is deliberately **never** made a flexible version (`flexibleVersions: "none"`) since it must stay parseable during the earliest bootstrap phase.

### 1.5 Flexible versions and tagged fields (KIP-482)

[KIP-482](https://cwiki.apache.org/confluence/display/KAFKA/KIP-482:+The+Kafka+Protocol+should+Support+Optional+Tagged+Fields) introduced a mechanism for extending message schemas without bumping the version number for every field addition. Every flexible-version struct ends with a **tagged-fields section**: an `UNSIGNED_VARINT` count of tagged fields present, followed by, for each one, `tag (UNSIGNED_VARINT) + size-in-bytes (UNSIGNED_VARINT) + raw data`, serialized in ascending tag order. A reader that doesn't recognize a given tag can skip it safely using the declared size — this is what makes the extension mechanism forward- *and* backward-compatible without a version bump.

Flexible versions also switch several primitive encodings to more compact forms:
- `COMPACT_STRING` — length encoded as `N+1` via unsigned varint (vs. classic `STRING`'s fixed `int16` length prefix).
- `COMPACT_NULLABLE_STRING` — null is represented by encoding length `0`.
- `COMPACT_ARRAY` / `COMPACT_NULLABLE_ARRAY` — same `N+1` varint-length convention (vs. classic arrays' 4-byte `int32` length prefix).

Which header version and which primitive encoding apply is determined per-API-per-version by the `flexibleVersions` field of that message's schema — e.g. `ProduceRequest` is flexible from v9+, `FetchRequest` from v12+, `ApiVersionsRequest`/`Response` from v3+, while `SaslHandshakeRequest` is never flexible. A broker implementation needs to track this per (API, version) pair, not assume a single global cutover point.

### 1.6 Minimum viable broker API set

To interoperate with mainstream clients (librdkafka, kafka-java, Sarama/franz-go) for the core produce/consume/consumer-group path, a broker needs, at minimum:

- **Bootstrapping**: `ApiVersions` (18), `Metadata` (3)
- **Data path**: `Produce` (0), `Fetch` (1), `ListOffsets` (2)
- **Consumer groups (classic protocol)**: `FindCoordinator` (10), `JoinGroup` (11), `SyncGroup` (14), `Heartbeat` (12), `LeaveGroup` (13), `OffsetCommit` (8), `OffsetFetch` (9)
- **Idempotence bootstrap**: `InitProducerId` (22) — many modern clients enable the idempotent producer path by default, which forces an `InitProducerId` call even when the application never asked for transactions. ⚠️ **CORRECTION (2026-09-02), from `M11.15`:** "librdkafka in particular" is wrong — a wire capture against confluent-kafka's librdkafka 2.15.0 found its own default for `enable.idempotence` is `false`, not `true`; only Kafka's Java client defaults it on, since KIP-679 (Kafka 3.0). The broader point stands (a client that opts in forces the call regardless of application intent), but the specific vendor claim does not.
- **Admin surface most tooling expects**: `CreateTopics` (19), `DeleteTopics` (20), `DescribeConfigs` (32)
- **Auth, if used**: `SaslHandshake` (17), `SaslAuthenticate` (36)

Notable per-API version details worth building against: `Produce` versions 0–2 were formally removed from Kafka trunk by [KIP-896](https://cwiki.apache.org/confluence/display/KAFKA/KIP-896%3A+Remove+old+client+protocol+API+versions+in+Kafka+4.0) (current `validVersions "3-13"`); `Fetch` gained `IsolationLevel` at v4+ (for transactions) and fetch-session `SessionId`/`SessionEpoch` at v7+ ([KIP-227](https://cwiki.apache.org/confluence/display/KAFKA/KIP-227%3A+Introduce+Incremental+FetchRequests+to+Increase+Partition+Scalability)); `Metadata`'s `AllowAutoTopicCreation` field (v4+) is what a broker must honor to support librdkafka/kafka-java's default auto-create-on-produce behavior; `FindCoordinator` v4+ ([KIP-699](https://cwiki.apache.org/confluence/display/KAFKA/KIP-699%3A+Update+FindCoordinatorRequest+to+support+batching)) turned it into a batched request taking an array of keys instead of one key per call, but older single-key requests must still be handled for older clients.

### 1.7 Optional / advanced request types

Everything below is safely deferrable for an initial minimum-viable broker, and can be layered on once the core path is solid:

- **Transactions / exactly-once semantics**: `InitProducerId` (with a `transactional.id`), `AddPartitionsToTxn` (24), `AddOffsetsToTxn` (25), `EndTxn` (26), `WriteTxnMarkers` (27), `TxnOffsetCommit` (28) — see §4.3.
- **ACLs**: `DescribeAcls`/`CreateAcls`/`DeleteAcls` (29–31) — only needed for multi-tenant authorization.
- **SCRAM credentials**: `DescribeUserScramCredentials`/`AlterUserScramCredentials` (50–51) — only if SASL/SCRAM specifically is supported (as opposed to SASL/PLAIN or mTLS).
- **Delegation tokens** (38–41) — niche short-lived-credential flow.
- **Quotas**: `DescribeClientQuotas`/`AlterClientQuotas` (48–49) — multi-tenant rate limiting.
- **Config management beyond the basics**: `AlterConfigs` (33), `IncrementalAlterConfigs` (44) — needed for admin-client parity (`kafka-configs.sh`, Confluent Control Center) but not for the data path.
- **KIP-848's new consumer-group protocol** (`ConsumerGroupHeartbeat`=68, `ConsumerGroupDescribe`=69) — see §3.3.

---

## 2. Record Batch Format (Message Format v2)

Kafka's on-wire/on-disk record container was redesigned in [KIP-98](https://cwiki.apache.org/confluence/display/KAFKA/KIP-98+-+Exactly+Once+Delivery+and+Transactional+Messaging) (Kafka 0.11) into the "RecordBatch" / message format v2, which every modern client and broker uses, replacing the older per-message v0/v1 "message set" format.

### 2.1 RecordBatch byte layout

The exact byte offsets, confirmed against Apache Kafka's `DefaultRecordBatch.java` source constants ([apache/kafka](https://github.com/apache/kafka/blob/trunk/clients/src/main/java/org/apache/kafka/common/record/DefaultRecordBatch.java); cross-checked against [kafka.apache.org message-format guide](https://kafka.apache.org/25/implementation/message-format/)):

| Field | Type | Byte offset | Size |
|---|---|---|---|
| `baseOffset` | int64 | 0 | 8 |
| `batchLength` | int32 | 8 | 4 |
| `partitionLeaderEpoch` | int32 | 12 | 4 |
| `magic` | int8 (currently `2`) | 16 | 1 |
| `crc` | uint32 (CRC-32C) | 17 | 4 |
| `attributes` | int16 | 21 | 2 |
| `lastOffsetDelta` | int32 | 23 | 4 |
| `baseTimestamp`/`firstTimestamp` | int64 | 27 | 8 |
| `maxTimestamp` | int64 | 35 | 8 |
| `producerId` | int64 | 43 | 8 |
| `producerEpoch` | int16 | 51 | 2 |
| `baseSequence` | int32 | 53 | 4 |
| `recordsCount` | int32 | 57 | 4 |
| `records` | `[Record]` | 61 | variable |

The fixed header overhead before the variable-length records array is exactly **61 bytes**.

### 2.2 Attributes bitfield

The 16-bit `attributes` field ([kafka.apache.org message-format guide](https://kafka.apache.org/25/implementation/message-format/)):

- bits 0–2 (`COMPRESSION_CODEC_MASK = 0x07`): compression codec — `0`=none, `1`=gzip, `2`=snappy, `3`=lz4, `4`=zstd
- bit 3 (`TIMESTAMP_TYPE_MASK = 0x08`): `0`=CreateTime, `1`=LogAppendTime
- bit 4 (`TRANSACTIONAL_FLAG_MASK = 0x10`): batch is part of a transaction
- bit 5 (`CONTROL_FLAG_MASK = 0x20`): batch is a control batch (transaction commit/abort marker, not application data)
- bit 6 (`DELETE_HORIZON_FLAG_MASK = 0x40`): a later addition beyond the original KIP-98 layout, used for tombstone/compaction delete-horizon tracking
- bits 7–15: unused/reserved

### 2.3 Individual record encoding

Records within a batch are compactly delta-encoded relative to the batch header, using Protobuf-style zigzag varints ([kafka.apache.org message-format guide](https://kafka.apache.org/25/implementation/message-format/)):

```
Record =>
  Length          => varint          (size of everything below, i.e. excludes this field)
  Attributes      => int8            (currently unused/reserved)
  TimestampDelta  => varint          (relative to baseTimestamp)
  OffsetDelta     => varint          (relative to baseOffset)
  KeyLength       => varint
  Key             => byte[]
  ValueLength     => varint
  Value           => byte[]
  Headers         => [Header]

Header =>
  HeaderKeyLength    => varint
  HeaderKey          => string
  HeaderValueLength  => varint
  Value              => byte[]
```

The docs state plainly: "we use the same varint encoding as Protobuf" — meaning implementers can reuse an existing zigzag-varint implementation rather than inventing one.

### 2.4 Compression codecs

Compression is applied to the **entire records portion as a single blob**, not per-message as in the old format: "when compression is enabled, the compressed record data is serialized directly following the count of the number of records" ([kafka.apache.org message-format guide](https://kafka.apache.org/25/implementation/message-format/)). The four supported codecs are gzip, snappy, lz4, and zstd, the last added by [KIP-110](https://cwiki.apache.org/confluence/display/KAFKA/KIP-110%3A+Add+Codec+for+ZStandard+Compression). zstd support is gated to magic-byte-2 (v2) batches only — "Instantiating MemoryRecords with magic < 2 is disallowed" for zstd — and a broker serving an old client a zstd batch is expected to return `UNSUPPORTED_COMPRESSION_TYPE` (error 74) rather than attempting an expensive on-the-fly decompress/recompress downconversion, which KIP-110 explicitly rejected as too costly.

### 2.5 CRC validation

The checksum is **CRC-32C (Castagnoli)**, not the plain CRC-32 used by the legacy format, and covers everything from the `attributes` field (byte offset 21) through the end of the batch — i.e. it explicitly **excludes** the first 21 bytes (`baseOffset` + `batchLength` + `partitionLeaderEpoch` + `magic` + the `crc` field itself). This is confirmed directly from the `DefaultRecordBatch.java` computation call `Crc32C.compute(buffer, ATTRIBUTES_OFFSET, buffer.limit() - ATTRIBUTES_OFFSET)` and restated in prose by the official guide: "the CRC covers the data from the attributes to the end of the batch" ([apache/kafka source](https://github.com/apache/kafka/blob/trunk/clients/src/main/java/org/apache/kafka/common/record/DefaultRecordBatch.java); [kafka.apache.org message-format guide](https://kafka.apache.org/25/implementation/message-format/)). Excluding `partitionLeaderEpoch` from the checksum is deliberate — brokers rewrite that field on certain operations (e.g. leader-epoch bumps during log rebuilds) without needing to touch the producer-computed CRC.

Control batches (the `isControlBatch` attribute bit set) carry a distinct record key schema used only by the transaction coordinator:
```
ControlMessageKey => Version ControlMessageType
  Version             => int16
  ControlMessageType  => int16   (0 = COMMIT, 1 = ABORT)
```

### 2.6 Comparison to legacy v0/v1

The old "message set" format wrapped each message individually with its own offset and CRC, rather than batching:

```
MessageSet (v0/v1) => [offset message_size message]
  offset        => int64
  message_size  => int32

Message (v0) => Crc MagicByte Attributes Key Value
Message (v1) => Crc MagicByte Attributes Timestamp KeyLength Key ValueLength Value   (KIP-32 added Timestamp)
```

In v0/v1 the CRC is plain CRC-32 (not Castagnoli), computed per message over everything from the magic byte through the value ("the crc field contains the CRC32 (and not CRC-32C) of the subsequent message bytes, i.e. from magic byte to the value" — [kafka.apache.org message-format guide](https://kafka.apache.org/25/implementation/message-format/)). Compression in v0/v1 was achieved via "recursive messages" — a wrapper message whose value field itself contains a compressed embedded message set — a much clunkier scheme than v2's single whole-batch compression. v2's batch-level header, CRC-32C, producer/sequence fields, and varint-delta record encoding together give substantially better space efficiency and are what idempotent/transactional producers depend on structurally, since `producerId`/`producerEpoch`/`baseSequence` simply don't exist as fields in the old format.

---

## 3. Consumer Group Protocol

### 3.1 The classic protocol

**Coordinator election.** A broker hashes the consumer group's `group.id` and takes `hash(groupId) % numPartitions` of the internal `__consumer_offsets` topic; whichever broker leads that partition's replica becomes the group coordinator ([Confluent: Consumer Group Protocol](https://developer.confluent.io/courses/architecture/consumer-group-protocol/)). Clients discover their coordinator by sending `FindCoordinator` (with `group.id`) to any broker, which resolves and returns the coordinator's id/host/port. Because `__consumer_offsets` is a normal replicated topic, coordinator failover is just partition-leader failover — if the coordinator broker dies, a follower replica's broker takes over, and clients discover the change the next time a request to the old coordinator fails.

**`__consumer_offsets`** is a compacted internal topic storing both committed offsets (per group/topic/partition) and group metadata (membership, generation, assignment); a coordinator rebuilds its in-memory group state by replaying this log on taking over leadership.

**JoinGroup / SyncGroup / Heartbeat / LeaveGroup flow:**
1. Consumer resolves its coordinator via `FindCoordinator`.
2. Consumer sends `JoinGroup`, including its subscribed topics and the assignment strategies it supports.
3. The coordinator designates a **group leader** (conventionally the first member to join) and picks the one assignment protocol name every current member advertises support for.
4. All members get back their member ID and the negotiated `GenerationId`; only the **leader's** `JoinGroupResponse` includes the full member list plus each member's subscription metadata.
5. The leader computes the actual partition assignment **client-side** and submits it via `SyncGroup`; non-leader members send `SyncGroup` too, but with an empty assignment payload.
6. The coordinator relays each member's own slice of the assignment back in the `SyncGroupResponse`.
7. Members then heartbeat periodically; missing `session.timeout.ms` triggers eviction and a new rebalance. `LeaveGroup` allows a clean, immediate departure instead of waiting out the session timeout.

The group's internal state machine moves through named states: **Empty** (no members) → **PreparingRebalance** (waiting for JoinGroups) → **CompletingRebalance** (waiting for the leader's SyncGroup) → **Stable** (steady state) → **Dead** (terminal, on group deletion/cleanup). `GenerationId` increments on every completed rebalance and is echoed by clients as a fencing token; a stale generation gets `ILLEGAL_GENERATION`, and an unrecognized member ID gets `UNKNOWN_MEMBER_ID`.

### 3.2 Partition assignment strategies and cooperative rebalancing

- **Range**: assigns partitions to consumers per-topic in contiguous ranges (first partition of each topic to the first consumer, etc.) — good for co-partitioned joins, but can be uneven when partition counts aren't evenly divisible, and unevenness compounds across topics.
- **RoundRobin**: lays all subscribed partitions from all topics into one flat list and hands them out round-robin regardless of topic — more even than Range but sacrifices topic co-location and any notion of stickiness across rebalances.
- **Sticky**: a balanced assignment that additionally makes "a best effort at sticking to the previous assignment" on rebalance, minimizing partition movement — but it still runs under the classic **eager** protocol, meaning every rebalance is still a full stop-the-world revoke-then-reassign, just with less actual reshuffling once it completes.
- **CooperativeSticky** ([KIP-429](https://cwiki.apache.org/confluence/display/KAFKA/KIP-429:+Kafka+Consumer+Incremental+Rebalance+Protocol), Kafka 2.4+): combines sticky assignment with the **incremental cooperative** rebalance protocol. Each member reports its currently-`ownedPartitions`, and the assignor classifies partitions into three buckets — still-owned-by-same-consumer (no action), newly-assigned-and-previously-unowned (hand over immediately), and owned-but-no-longer-assigned (must be explicitly revoked first). The invariant enforced is that **a partition must be revoked by its previous owner before being handed to a new one**, which is why a cooperative rebalance can take two rounds instead of one.

Under the eager protocol, *any* membership change causes *every* consumer to revoke *all* of its partitions and rejoin — Confluent's own Connect-cluster benchmark found 900 tasks took 12–14 minutes to stabilize under eager rebalancing versus ~1 minute under cooperative, with aggregate throughput up 113% during the transition window ([Confluent: Incremental Cooperative Rebalancing](https://www.confluent.io/blog/incremental-cooperative-rebalancing-in-kafka/)). Migrating a live group from eager to cooperative requires two rolling deploys (first add `cooperative-sticky` alongside the existing assignor in the client's configured list so the group still runs eager because that's the only protocol every member supports yet; then remove the old assignor once every member has upgraded) — a staged rollout needed because mixed-protocol groups are unsafe ([KIP-429](https://cwiki.apache.org/confluence/display/KAFKA/KIP-429:+Kafka+Consumer+Incremental+Rebalance+Protocol)).

### 3.3 KIP-848: the next-generation consumer rebalance protocol

[KIP-848](https://cwiki.apache.org/confluence/display/KAFKA/KIP-848:+The+Next+Generation+of+the+Consumer+Rebalance+Protocol) replaces the entire JoinGroup/SyncGroup/Heartbeat dance with a fundamentally different, server-driven design, motivated by the classic protocol putting rebalance logic in thick clients (hard to fix/debug across a fleet of languages and versions) and by the fact that even cooperative rebalancing still depends on a group-wide synchronization barrier that a single slow member can stall.

**New API — `ConsumerGroupHeartbeat`.** A single RPC now folds together subscription updates, liveness, and assignment delivery. Members send only *changed* fields after their first join (full state resent only after an error); the coordinator's response carries assigned/pending partitions and a broker-dictated heartbeat interval. There is no separate leader-computes-then-syncs step for the default path — assignment is computed centrally by the coordinator.

**Epoch model** replaces the single generation ID with three cooperating epochs:
- **Group Epoch** — bumped on any group-metadata-affecting event (join/leave, subscription change, topic metadata change).
- **Assignment Epoch** — the Group Epoch value that produced the current target assignment; a new computation is triggered whenever Group Epoch advances past it.
- **Member Epoch** — each member's own progress toward the target, used as a fencing token (a stale epoch on a heartbeat gets `FENCED_MEMBER_EPOCH`).

**Declarative, asynchronous reconciliation.** The assignor computes a single **target assignment** for the whole group; each member's **current assignment** converges toward it independently through revoke-then-assign steps driven by its own heartbeats — critically, members whose assignment doesn't change are never blocked or involved. This is the mechanism that eliminates the stop-the-world pause: Instaclustr's benchmark of scaling a topic from 100 to 1,000 partitions found this dropped rebalance time from 103 seconds under the classic protocol to 5 seconds under KIP-848 ([Instaclustr](https://www.instaclustr.com/blog/rebalance-your-apache-kafka-partitions-with-the-next-generation-consumer-rebalance-protocol/)).

**New APIs**: `ConsumerGroupHeartbeat` (68), `ConsumerGroupDescribe` (69, an operational introspection API exposing group state, epochs, chosen assignor, and per-member current/target assignment without needing client-side logs). Default server-side assignors are `RangeAssignor` (co-partitioning) and `UniformAssignor` (the new default, conceptually similar to the old Sticky assignor). A client-side assignor path exists in the design (for power users like Kafka Streams, via `ConsumerGroupPrepareAssignment`/`ConsumerGroupInstallAssignment`) but is **not implemented as of Kafka 4.0**.

**Migration/compatibility.** Old and new protocol members can coexist in the same group during a rolling upgrade — the coordinator internally translates classic JoinGroup/SyncGroup/Heartbeat calls onto the new group model. A group's type flips from "classic" to "consumer" the moment the first new-protocol member joins, and back again if the last one leaves. Clients opt in via `group.protocol=consumer`; a new-protocol client talking to a broker that doesn't support it fails fast at startup rather than silently falling back. Regex topic subscriptions are now evaluated **server-side** with RE2/J for consistent semantics across client languages, and `OffsetFetch`/`OffsetCommit` were bumped to v9 to carry topic IDs (shipped Kafka 4.2) so that a deleted-and-recreated topic is correctly treated as a new entity rather than silently resuming stale offsets.

**Status**: Early Access in Kafka 3.7, GA for server-side assignors in **Kafka 4.0**. Known gaps as of that release: client-side assignors unimplemented, rack-aware assignment unsupported (tracked as KAFKA-17747). librdkafka 2.10 has an early-access implementation ([Confluent: KIP-848](https://www.confluent.io/blog/kip-848-consumer-rebalance-protocol/)).

---

## 4. Producer Semantics

### 4.1 `acks` and `min.insync.replicas`

The `acks` field on `ProduceRequest` (historically named `RequiredAcks` in the schema) takes three values:

- **`acks=0`** — fire-and-forget; the producer doesn't wait for any broker response and can silently lose data if the leader fails before persisting.
- **`acks=1`** — the producer waits only for the partition leader's acknowledgment; data can still be lost if the leader crashes before followers replicate it.
- **`acks=all`** (a.k.a. `acks=-1`) — the producer waits for every current in-sync replica (ISR) to persist the record; this is the strongest durability option this field alone provides.

A common misconception worth calling out explicitly: `min.insync.replicas` is **not** the number of replicas the broker waits for under `acks=all` — the broker always waits for *every* member of the current ISR set. `min.insync.replicas` is instead a **gate**: if the current ISR size falls below this threshold, the broker refuses the write outright (returning `NotEnoughReplicas`/`NotEnoughReplicasAfterAppend`) rather than accepting it with fewer acks than required ([kafka.apache.org broker configs](https://kafka.apache.org/41/configuration/broker-configs/); [2 Minute Streaming: acks & min.insync.replicas](https://blog.2minutestreaming.com/p/kafka-acks-min-insync-replicas-explained)). The commonly recommended durable configuration is replication factor 3, `min.insync.replicas=2`, `acks=all` — tolerating one broker failure while still requiring 2-of-3 replicas to acknowledge.

### 4.2 Idempotent producer

Enabled via `enable.idempotence=true` (the default for Kafka's Java client since KIP-679; ⚠️ **CORRECTION (2026-09-02), from `M11.17`:** not for many client configurations generally — a wire capture against confluent-kafka's librdkafka 2.15.0 found its own default is `false`, the same correction §1.6's "Idempotence bootstrap" bullet already carries), the idempotent producer guarantees that resending a message on retry does not create a duplicate in the log and does not reorder it, by having the broker deduplicate on a **(producer ID, partition, sequence number)** key ([Confluent: Delivery Semantics](https://docs.confluent.io/kafka/design/delivery-semantics.html)).

- **Bootstrap**: the producer calls `InitProducerId`, receiving a broker-assigned `PID` (int64) and `Epoch` (int16). For a plain idempotent producer (no `transactional.id`), this identity is not persistent — each new producer instance simply gets a fresh PID with no cross-restart continuity.
- **Sequence numbers** are scoped per `(PID, TopicPartition)`, starting at zero and incrementing by exactly one per batch. The broker rejects a produce request "if its sequence number is not exactly one greater than the last committed message from that PID/TopicPartition pair" ([KIP-98](https://cwiki.apache.org/confluence/display/KAFKA/KIP-98+-+Exactly+Once+Delivery+and+Transactional+Messaging)). In practice the broker's `ProducerStateManager` caches metadata for the last several recent batches per (PID, partition) — not just a single highwater number — so an exact retransmit of a batch that already landed is recognized as a duplicate and the previously-recorded offset is returned without a second append; a **lower** sequence than expected is a harmless duplicate, while a **higher** sequence than expected (a gap) is fatal and surfaces as `OutOfOrderSequenceException`, since it means a batch was lost in between and in-order delivery can no longer be guaranteed.
- This mechanism is durable across leader failover because the dedup state is persisted through the replicated log itself, not just kept in the leader's memory — "even if the leader fails, any broker that takes over will also know if a resend is a duplicate" ([Confluent: Exactly-Once Semantics](https://www.confluent.io/blog/exactly-once-semantics-are-possible-heres-how-apache-kafka-does-it/)).

### 4.3 Transactional producer / exactly-once semantics (KIP-98)

Transactions build directly on top of idempotence, adding atomicity across multiple partitions (and, optionally, a consumer group's offset commit) as a single unit.

**Transaction coordinator.** A module running inside every broker, analogous to the consumer-group coordinator: whichever broker leads a given partition of the internal `__transaction_state` topic acts as transaction coordinator for the transactional IDs hashed to that partition. It owns PID/epoch assignment, tracks which partitions are part of each in-flight transaction, and drives the commit/abort sequence including writing control markers to data partitions ([Confluent: Transactions in Apache Kafka](https://www.confluent.io/blog/transactions-apache-kafka/); [KIP-98](https://cwiki.apache.org/confluence/display/KAFKA/KIP-98+-+Exactly+Once+Delivery+and+Transactional+Messaging)). `__transaction_state` defaults to 50 partitions, replication factor 3, min ISR 2 — deliberately analogous to `__consumer_offsets`.

**Transactional ID and zombie fencing.** A transactional producer sets a stable, user-supplied `transactional.id` that survives process restarts (unlike the opaque PID). On `InitProducerId` with a `transactional.id`, the coordinator looks up (or creates) the PID mapped to that ID and **increments its epoch** — this is the fencing mechanism. Any earlier producer instance holding the same `transactional.id` but an older epoch is now a "zombie": further writes or control RPCs it attempts are rejected, preventing the classic problem of two instances of the same logical producer (e.g. before and after a stream-processing rebalance) both writing concurrently.

**Full RPC flow**:
1. `FindCoordinator` (`CoordinatorType=1`) locates the transaction coordinator for the `transactional.id`.
2. `InitProducerId` obtains/refreshes the PID+epoch, aborting any dangling transaction left by a prior instance.
3. `AddPartitionsToTxn` registers each new partition the first time the transaction writes to it (starts the transaction timer on the first partition added).
4. Normal `Produce` requests write records, carrying `PID`/`ProducerEpoch`/`FirstSequence` and the transactional attribute bit.
5. For read-process-write patterns, `AddOffsetsToTxn` + `TxnOffsetCommit` fold a consumer group's offset commit into the same transaction, writing to `__consumer_offsets` but keeping those offsets invisible until commit.
6. `EndTxn` (commit or abort) drives a two-phase-commit-like sequence: the coordinator first durably writes `PrepareCommit`/`PrepareAbort` to `__transaction_state` — this is the actual point of no return — then issues `WriteTxnMarkers` to every partition leader involved (writing COMMIT/ABORT **control batches**, distinguished by a dedicated attribute bit and excluded from normal application-facing delivery), and finally writes `CompleteCommit`/`CompleteAbort` once all markers land.

**`read_committed` isolation and the aborted-transaction index.** A consumer's `isolation.level` defaults to `read_uncommitted` (sees everything, including data from transactions that later abort). `read_committed` instead only surfaces committed data, using two `FetchResponse` fields to avoid unbounded buffering: **LastStableOffset (LSO)** — the highest offset below which every transaction's outcome is already resolved, which bounds how far a `read_committed` consumer can read — and an **`AbortedTransactions`** array of `(PID, FirstOffset)` pairs that lets the consumer splice out aborted data directly rather than buffering it until it happens to encounter the ABORT marker ([KIP-98](https://cwiki.apache.org/confluence/display/KAFKA/KIP-98+-+Exactly+Once+Delivery+and+Transactional+Messaging)).

Confluent's own benchmark reported roughly a 3% throughput decline for the transactional producer versus plain `acks=all`, and 20% versus `acks`-minimal at-most-once defaults — a modest overhead for the guarantee provided ([Confluent: Exactly-Once Semantics](https://www.confluent.io/blog/exactly-once-semantics-are-possible-heres-how-apache-kafka-does-it/)). Note that KIP-98's wiki text itself defers the exact byte-level schema of `__transaction_state` records to the Kafka source (`TransactionLog.scala`/`TransactionMetadata.scala`) rather than documenting it directly — worth pulling from source if an exact-byte reimplementation is needed.

---

## 5. Tiered Storage (KIP-405) and Diskless Topics (the KIP-1150 family)

This is the most directly relevant area of Kafka's own roadmap to an object-storage-backed reimplementation, because the Kafka community is independently converging on very similar architecture — driven substantially by engineers from Aiven (with AutoMQ's existing product referenced as prior art in the discussion).

### 5.1 KIP-405: Tiered Storage — what it does and does not solve

[KIP-405](https://cwiki.apache.org/confluence/display/KAFKA/KIP-405:+Kafka+Tiered+Storage) (production-ready since Kafka 3.9, discussion started February 2021, JIRA KAFKA-7739) splits each partition's log into a **local tier** and a **remote tier**. The local tier keeps only recent segments (retention measured in hours via `local.retention.ms`/`local.retention.bytes`) and continues to serve low-latency tail reads exactly as classic Kafka does today; the remote tier holds older, already-closed segments in object storage (S3, GCS, Azure Blob, HDFS, etc.), with retention measured in days to months.

Three components implement this: **RemoteLogManager (RLM)**, the internal broker component that reacts to leadership changes and schedules segment copy/cleanup on a leader-side thread pool; **RemoteStorageManager (RSM)**, the pluggable public interface a vendor/operator implements per backend (`copyLogSegmentData`, `fetchLogSegment`, `fetchIndex`, `deleteLogSegmentData`, all required to be idempotent); and **RemoteLogMetadataManager (RLMM)**, which tracks segment lifecycle metadata with strong consistency, defaulting to an internal `__remote_log_metadata` topic (50 partitions by default) ([KIP-405 wiki](https://cwiki.apache.org/confluence/display/KAFKA/KIP-405:+Kafka+Tiered+Storage); search-corroborated component summary).

The crucial limitation, stated directly in KIP-1150's own motivation section: **tiered storage only moves closed/inactive segments to object storage after the fact.** The active write path — every append to the currently-open segment — still goes through ordinary Kafka replication onto local block storage across brokers, potentially spanning multiple availability zones. Tiered storage is a retention/cold-storage cost optimization; it does nothing to reduce the cross-AZ replication cost of the hot write path itself. Local segments are also never deleted until confirmed-copied remotely, even past local retention thresholds, and enabling tiered storage on a topic is irreversible; compacted topics and JBOD are unsupported with it enabled.

### 5.2 KIP-1150: Diskless Topics — motivation and architecture

[KIP-1150 "Diskless Topics"](https://cwiki.apache.org/confluence/display/KAFKA/KIP-1150:+Diskless+Topics) is explicitly a **motivational/umbrella KIP** — modeled on how KRaft (KIP-500) was split into a top-level consensus proposal plus separate implementation KIPs — establishing *whether* Kafka should support this architecture at all, with the actual mechanics deferred to sibling KIPs. Authored by a group including several Aiven engineers (Ivan Yurchenko, Jorge Quilcate, Anatolii Popov, Juha Mynttinen, Josep Prat) alongside others; **status is Accepted** (JIRA KAFKA-19161), following a restarted vote in January 2026 after a DISCUSS thread that opened in April 2025.

**Definition**: "A Diskless Topic is a topic which does not append directly to block storage devices, and does not utilize direct replication... Diskless is to 'No Disks' as Serverless is to 'No Servers' — the attached disks become a less important abstraction for operators but are still functionally present." Data for a diskless topic is durably stored in object storage at all times; local disk is retained only for KRaft metadata, coordinator-shard state, temporary staging, and read caching.

**Motivation — cited cross-AZ pricing** (directly from the KIP text): AWS charges $0.02/GiB for same-region cross-AZ transfer, Google Cloud $0.01/GiB, Azure nothing. In a standard multi-AZ deployment, a message can cross zone boundaries up to four times in its lifecycle (producer→leader, leader→each follower, leader→consumer), each hop paying this tax — the [Aiven blog on the design](https://aiven.io/blog/guide-diskless-apache-kafka-kip-1150) frames removing this as capable of cutting total cost of ownership by up to 80%, though this figure is Aiven's own claim, not an independently audited benchmark. The KIP text also poses the strategic question directly: should Kafka natively adopt an object-storage-native design similar to the protocol-compatible alternatives already on the market, rather than cede this ground to forks and proprietary reimplementations (an implicit reference to WarpStream, Bufstream, and AutoMQ)? The KIP answers yes, with "do nothing" explicitly listed and rejected as an alternative for exactly this reason.

**Design constraints stated explicitly**: (1) no API changes required of clients — a topic simply flips a per-topic flag; (2) diskless and classic topics coexist in the same cluster, so adoption is an upgrade, not a migration; (3) the feature is built for upstream Apache Kafka, not a fork.

**Compatibility promise**: diskless topics are intended to be "semantically interchangeable" with classic topics — preserving ordering, idempotency, transactions, consumer groups/offsets, share groups (KIP-932), and Tiered Storage integration, all through the existing protocol surface.

### 5.3 KIP-1163 (Diskless Core): the produce/consume mechanics

[KIP-1163 "Diskless Core"](https://cwiki.apache.org/confluence/display/KAFKA/KIP-1163:+Diskless+Core) (status: Under Discussion at time of research) defines the actual data path. Its own framing: "Kafka delegates replication of diskless topics to object storage, and does not perform replication itself" — durability comes from the object store's own redundancy (commonly cited as "eleven nines"), not from `replication.factor`.

**Core abstractions**:
- **WAL Segments** — immutable objects, each containing batches from *multiple* topic-partitions batched together (to amortize the cost of an object-storage PUT, since per-partition-per-object would be prohibitively small and numerous).
- **Diskless Coordinator** — the stateful "source of truth about batch coordinates and WAL Segments," which assigns global ordering and offsets (detailed in §5.4).
- A pluggable **object storage abstraction** — put/delete/list/ranged-get — for which "Apache Kafka will not provide a production-grade implementation" in open source, mirroring KIP-405's posture of shipping only the RSM interface, not a bundled S3 implementation.

**Produce path** (from KIP-1163's stated design):
1. A producer sends to **any broker in its availability zone** — not necessarily a designated partition "leader"; diskless produce is leaderless.
2. The broker buffers incoming records until a size threshold (default 4 MiB) or time threshold (default 250 ms) is hit.
3. The broker packs the buffered batches from potentially many partitions into a shared WAL segment object and uploads it to object storage.
4. The broker commits the resulting batch coordinates (object ID + byte ranges) to the Diskless Coordinator.
5. The coordinator assigns monotonic global offsets and timestamps to each batch and confirms the commit.
6. The broker responds to all pending producer requests for that flush.

Stated target latency: P50 ≈ 500 ms, P99 ≈ 1–2 seconds for the produce path — an explicit, acknowledged trade-off versus classic Kafka's typically sub-50ms produce latency, to be mitigated by client-side pipelining (more outstanding requests in flight; see the related [KIP-1269](https://cwiki.apache.org/confluence/display/KAFKA/KIP-1269) on configurable in-flight batch retention).

**The consistency/ordering answer — decoupling data path from metadata path.** This is the key design insight enabling multiple brokers to accept concurrent writes to the same partition while still producing a strictly ordered offset sequence: batches are uploaded to object storage **without** offsets pre-assigned, and only become "real" once the broker's batch-coordinate commit is acknowledged by the coordinator, which centrally and sequentially assigns the actual offset/timestamp. Because offset assignment is centralized in one component even though the data upload itself is fully parallel and decentralized, per-partition ordering stays gapless and monotonic without requiring the brokers themselves to coordinate with each other.

**Consume path**: replicas maintain local caches built from object storage under the coordinator's direction, and fetches prefer serving from a local (or in-rack replica's) cache; only genuinely cold reads or an out-of-sync replica fall back to a direct object-storage GET. "In-sync replica" is redefined to mean "in sync with the Diskless Coordinator" rather than with a partition leader — meaning a leader failure no longer threatens read availability the way it does in classic Kafka, since every replica independently pulls from the same durable object-storage source of truth.

**New/changed protocol surface**: `Metadata` API bumped to v14, adding `RackId` to the request and `IsDiskless`/`PreferredProduceBrokers` to the response, so zone-aware clients can route produce traffic to a broker in their own AZ without any new RPC type — the whole feature is layered onto the existing Metadata exchange. Legacy clients that can't be upgraded get a fallback: appending `,diskless_rack_id=<rack_id>` to their client ID string as an out-of-band hint.

**Deletion model**: two-level — *logical* deletion (the coordinator marks batches expired per retention policy without touching the immutable WAL object) followed by *physical* deletion (an async process removes the underlying object only once every batch within it, across however many partitions share that object, has been logically expired).

### 5.4 KIP-1164 (Diskless Coordinator): how it's actually built

[KIP-1164 "Diskless Coordinator"](https://cwiki.apache.org/confluence/display/KAFKA/KIP-1164:+Diskless+Coordinator) (Under Discussion) specifies the coordinator itself — and, notably, does **not** introduce a brand-new consensus system. It is built on an **internal Kafka topic**, `__diskless_metadata`, using the existing partition-leader-as-coordinator pattern already familiar from consumer groups and transactions: each partition of this topic is an independent coordinator "shard," its leader broker serving as coordinator for whichever user-topic-partitions are assigned to that shard, discoverable via the same `FindCoordinator` pattern.

**Local materialized state uses embedded SQLite**, justified because expected coordinator state size runs to "hundreds of megabytes or even gigabytes" — too large to keep purely in memory — with the underlying `__diskless_metadata` log remaining the actual source of truth and SQLite serving as a rebuildable local view. This specific choice drew committer pushback during the DISCUSS thread — Christo Lolov explicitly challenged the SQLite dependency, suggesting an internal mechanism instead — flagging it as a live design contention point rather than settled consensus.

**Replication/consistency pipeline**, deliberately compared by the authors to KRaft's own pipeline: validate against current-plus-pending state → append to the coordinator's log → wait for `acks=all` replication → apply to local state → respond. This trades slightly higher latency for a bounded, log-driven recovery path.

**Seven new coordinator-only APIs**, all requiring `CLUSTER_ACTION` permission: `DisklessCreatePartitions`, `DisklessDeleteTopics`, `DisklessCommitFile` (the core offset-assignment call), `DisklessDeleteRecords`, `DisklessListOffsets` (strongly consistent), `DisklessFindBatches` (stale-tolerant, follower-readable), `DisklessDescribeFiles`.

**Scaling and locality**: the design includes a "produce gateway" optimization — specific brokers per (coordinator-shard × rack) are designated preferred produce targets, populated into the `PreferredProduceBrokers` Metadata field from §5.3, sized as `max(1, n_brokers / n_coordinator_shards / n_racks)` — intended to bound the fan-out of brokers talking to coordinator shards as a cluster grows.

### 5.5 KIP-1165 (Object Compaction) and the broader sibling-KIP family

[KIP-1165 "Object Compaction for Diskless"](https://cwiki.apache.org/confluence/display/KAFKA/KIP-1165:+Object+Compaction+for+Diskless) addresses a consequence of the multi-partition-interleaved WAL segment design: over time, reads accumulate many small, poorly-organized objects. Per the Aiven blog's description, broker-resident "compaction agents" reorganize batches by offset and topic-partition, stream the results to bound memory usage, and notify the coordinator of the new object layout — explicitly likened by the blog to disk defragmentation.

A wider family of related-but-separate KIPs is accumulating around this effort, several also touching Tiered Storage: **KIP-1279** (efficient diskless topic mirroring across clusters via object-storage references), **KIP-1272** (compacted-topic support under Tiered Storage), **KIP-1269** (configurable in-flight batch retention, mitigating diskless's higher produce latency), **KIP-1248**/**KIP-1254** (broker/consumer support for remote tiered-storage fetch). Note: **KIP-1147 is unrelated** — it is "Improve consistency of command-line arguments," shipped in Kafka 4.2 — a naming collision worth explicitly avoiding when citing the diskless family.

### 5.6 Status, open questions, and relation to WarpStream/Bufstream/AutoMQ

As of this research, KIP-1150 itself is **Accepted**; KIP-1163 and KIP-1164 remain **Under Discussion**, meaning the concrete implementation is still being iterated on the mailing list, not shipped. Explicitly acknowledged open points from the design documents and discussion threads:

- **Latency is a first-order, accepted trade-off** — P50 ~500ms/P99 ~1-2s for diskless produce versus classic Kafka's much lower baseline; the authors are explicit that diskless is unsuitable for latency-sensitive workloads without further mitigation.
- **Transactions/EOS for diskless topics are still described as "in preview"** by Aiven's own blog, pending further follow-up KIPs — not fully specified yet in the current text.
- Early discussion (April 2025) flagged an apparent inconsistency between KIP-1163 and KIP-1164 over exactly which component owns object-storage lifecycle management — illustrating friction inherent in splitting a large architecture across multiple sub-KIPs.
- The default partition count for `__diskless_metadata` is explicitly marked TBD.

On relation to existing systems: KIP-1150's own "rejected alternatives" section frames *not* pursuing this work as a risk of diskless becoming "the single most substantial missing feature from the upstream implementation," pushing users toward forks and proprietary reimplementations — an implicit but clear acknowledgment of WarpStream, Bufstream, and AutoMQ as the systems already occupying this space. The Aiven blog draws one explicit (if unnamed) architectural contrast: systems that "push batching logic into the client library or shard coordination across a fleet of tiny consensus nodes" are called out as effective for greenfield stacks but painful for organizations running thousands of existing polyglot Kafka clients — KIP-1150's own choice is to keep all new logic broker-side and expose it through ordinary Kafka protocol extensions (the Metadata v14 fields), requiring zero client-library changes. AutoMQ, which already ships a Kafka-protocol-compatible S3-backed broker as a commercial product, similarly published its own reaction piece situating KIP-1150 as validating the diskless architecture pattern it had already built, while noting AutoMQ's own product uses a pluggable local WAL layer (S3 directly, or regional EBS/NFS for sub-10ms latency) that KIP-1150 does not yet offer as an option ([AutoMQ: KIP-1150 Explained](https://www.automq.com/blog/kip-1150-explained-diskless-topics-kafka-future)).

For a from-scratch Rust implementation, the practical takeaway is that KIP-1150's architecture — leaderless produce to any broker, a centralized offset-assignment coordinator, shared multi-partition WAL segment objects, and a compaction/defragmentation pass to bound object-read fan-out — is essentially the same shape of problem this project needs to solve, and its publicly-discussed trade-offs (produce latency, coordinator state sizing, transaction support ordering) are a useful checklist of design decisions to make deliberately rather than discover the hard way.

---

## 6. Existing Rust Crates for the Kafka Protocol

### 6.1 `kafka-protocol`

The [`kafka-protocol`](https://crates.io/crates/kafka-protocol) crate ([GitHub: tychedelia/kafka-protocol-rs](https://github.com/tychedelia/kafka-protocol-rs), formerly maintained under a different namespace) is the most directly relevant building block. It is described as "a Rust implementation of the Kafka wire protocol" that uses **code generation to cover the entire Kafka API surface across protocol versions**, rather than hand-written encode/decode logic for each message. Its schemas are generated directly against Apache Kafka's own upstream protocol message definitions (the JSON files under `clients/src/main/resources/common/message/` in the `apache/kafka` source tree — the same files this document's research pulled ground-truth field lists from in §1 and §2), currently tracking Kafka 4.1.0 as of the latest release. It exposes `Encodable`/`Decodable` traits, per-message request/response structs constructible via `Default::default()` plus builder-style setters, and marks all exported items `#[non_exhaustive]` for forward compatibility as new protocol versions land. It has seen substantial real-world adoption — crates.io reports over 7.2 million all-time downloads across 25+ published versions — and states an MSRV policy of supporting the last 3 Rust releases. Licensed MIT OR Apache-2.0. This is a strong candidate to sit underneath a Rust broker's wire layer instead of hand-rolling message encoding, since it removes the single largest and most error-prone slice of "getting Kafka protocol compatibility right": producing byte-exact encodings across dozens of messages and their many versioned variants.

### 6.2 `rdkafka`

[`rdkafka`](https://crates.io/crates/rdkafka) is a Rust binding around the C library `librdkafka`. It is the most mature and battle-tested Kafka **client** library available in Rust, but it is a binding to a client-side C library, not a protocol implementation usable to build broker-side (server) logic — relevant here mainly as a testing/validation tool (running `rdkafka` as a client against a from-scratch broker is a natural compatibility check, given librdkafka's own broad adoption as the reference C implementation many other language bindings wrap).

### 6.3 Other Rust options and prior art

- **Fluvio's protocol history**: Fluvio (InfinyOn), a Rust-and-WASM streaming platform, originally maintained strict Kafka protocol compatibility via its own crate (`flv-kf-protocol`, hosted at `github.com/infinyon/flv-kf-protocol`), but the team later **explicitly abandoned strict Kafka wire-protocol compatibility** to reduce maintenance burden and pursue features (like its SmartModule/SmartStream WASM processing) unconstrained by Kafka protocol fidelity. Fluvio currently uses only "a subset of Kafka protocol with a compatible storage format" and no longer claims Kafka compatibility as a primary goal. This is a useful cautionary data point: even a well-resourced Rust team found full protocol fidelity expensive enough to consciously trade away once their product diverged from being a drop-in Kafka replacement.
- **`rskafka`**, **`korken89/kafka-rust`**, and a handful of other older or narrower crates exist but are comparatively unmaintained or narrower in scope than `kafka-protocol`; none has comparable adoption or version coverage.
- **Redpanda** (C++/Seastar) and **WarpStream**/**Bufstream** (both Go, the latter atop `franz-go`) do not use any Rust crate, for obvious reasons, but are useful architectural reference points regardless of implementation language (see §7).

### 6.4 The underlying strategy: code-generation from Kafka's own JSON message specs

Independent of whether the existing `kafka-protocol` crate is adopted wholesale, the strategy it embodies is itself the recommended approach: Apache Kafka's own Java client and broker are generated from machine-readable JSON schema files (`clients/src/main/resources/common/message/*.json` in `apache/kafka`) that declare every request/response message, its fields, per-field `versions`/`nullableVersions`/`flexibleVersions` ranges, and default values. These are the same files this research fetched directly (§1.3–1.6) to get exact, version-precise field lists — they are the actual ground-truth protocol specification, arguably more authoritative and certainly more precise than the prose protocol guide, and they update automatically with every new Kafka release. A Rust implementation — whether via `kafka-protocol` directly, a fork of it, or a custom code generator built against the same JSON files — avoids the two biggest sources of protocol-compatibility bugs: manually tracking version-gated field presence, and manually implementing the flexible-versions/tagged-fields/compact-encoding rules from §1.5 correctly for every message.

---

## 7. What "Compatibility" Means in Practice

### 7.1 Which clients get tested

Every vendor examined validates against the same small set of reference client implementations, because they cover the overwhelming majority of real-world usage patterns and internal quirks:

- **librdkafka** (C) and its many language bindings (`confluent-kafka-python`, `confluent-kafka-dotnet`, `rdkafka` in Rust, etc.) — the de facto reference non-Java client, and often the strictest about certain protocol details.
- **The Apache Kafka Java client** (`kafka-clients`) — the reference implementation, and what Kafka Streams/Connect are built on.
- **Sarama** and the newer **franz-go** in Go — franz-go in particular is what WarpStream itself is built on, and is increasingly the Go client of choice for new projects.
- **segmentio/kafka-go** — a lighter-weight Go client, occasionally exercising different code paths than franz-go/Sarama.

Redpanda's own compatibility documentation states plainly that "clients developed for Kafka versions 0.11 or later are compatible with Redpanda," that the Java client is validated at each release against Redpanda's `ducktape` and chaos test suites, and — tellingly — that "production compatibility still depends on the specific client versions, APIs, security settings, ecosystem components, and operational tools your workload uses," concluding that the right question isn't "is client X compatible" in the abstract but "which Kafka surfaces does your workload actually exercise, and have you tested each one against both the source and target" ([Redpanda: Kafka Compatibility](https://docs.redpanda.com/current/develop/kafka-clients/)).

### 7.2 WarpStream: protocol quirks encountered building a stateless, object-storage-only broker

WarpStream's own engineering write-up on protocol compatibility ([WarpStream: Hacking the Kafka PRoTocOL](https://www.warpstream.com/blog/hacking-the-kafka-protocol)) is directly relevant because WarpStream's architecture — a fleet of stateless "Agent" processes with no local disk, talking only to object storage and a control plane — is the closest existing analog to what this project intends to build. The blog's central observation: "the Kafka protocol assumes an incredibly stateful system," where each broker owns specific partitions with a fixed identity, whereas WarpStream needed *any* Agent to be able to serve *any* partition at *any* time. Concrete quirks they had to work around:

- **Client hostname sensitivity**: some client implementations inspect both the node ID *and* the hostname of a broker to decide whether a genuine broker topology change has occurred, which conflicted with deploying a fleet of interchangeable Agents behind a single load balancer (all sharing the LB's hostname while having distinct node IDs).
- **A DNS case-sensitivity exploit as the fix**: "hostnames in the Kafka protocol client implementations are all treated as case-sensitive, but DNS is case-insensitive" — letting WarpStream mint distinct-looking hostnames per Agent (that all resolve via DNS to the same load-balanced address) to satisfy clients' hostname-change detection logic without needing per-Agent DNS infrastructure.
- **Repurposing the `client_id` field as an out-of-band metadata channel**: unable to extend the wire protocol itself, WarpStream encodes availability-zone information directly into the client ID string sent by the *broker* side of the equation, achieving zone-aware routing without any new RPC.

WarpStream separately maintains a public [protocol and feature support matrix](https://docs.warpstream.com/warpstream/kafka/reference/protocol-and-feature-support) documenting known, deliberate gaps versus a stock Kafka broker (fields it ignores, request timeout caps, etc.) — a useful model for how this project should document its own compatibility surface once it has one.

### 7.3 Redpanda: reimplementing the protocol in C++ from scratch

Redpanda's own experience (reimplementing Produce/Fetch/Metadata/consumer-group coordination and more in C++ on Seastar, entirely independent of the JVM broker's code) reinforces the same lesson as Redpanda's compatibility docs above: broad client-library compatibility is achievable, but real production parity is a long tail of specific API surfaces, specific client library versions, and specific configuration combinations, validated continuously rather than declared once. Their public documentation is structured explicitly as a per-client, per-feature compatibility table rather than a blanket "Kafka compatible" claim ([Redpanda: Kafka Compatibility](https://docs.redpanda.com/current/develop/kafka-clients/)).

### 7.4 Bufstream: leaderless, metadata-store-backed, and claiming full protocol fidelity including transactions

Bufstream (from Buf) is architecturally close to both WarpStream and the KIP-1150 design: a **leaderless** broker fleet where "any broker can take a produce request for any topic and partition," fanning in all incoming produce traffic regardless of partition into a single write-optimized "intake file" that is uploaded to object storage, with an off-the-shelf strongly-consistent metadata store (Postgres, Google Cloud Spanner, or etcd) recording what landed where — a produce isn't acknowledged until *both* the object-storage write and the metadata-store write succeed ([Bufstream: Kafka data flow](https://buf.build/docs/bufstream/architecture/kafka-flow/)). Buf's own marketing claims "100% compatible with the Kafka protocol, including support for exactly-once semantics (EOS) and transactions" and reports benchmark figures of roughly 8x lower cost than Apache Kafka at a stated median produce latency of 260ms and p99 of 500ms ([Buf: Bufstream](https://webflow.buf.build/blog/bufstream-kafka-lower-cost)) — broadly the same latency envelope KIP-1163 targets for diskless topics, suggesting this ~hundreds-of-milliseconds range is close to an inherent floor for any object-storage-backed produce path rather than an implementation-specific limitation.

### 7.5 Fluvio: a cautionary example of compatibility as an ongoing cost, not a one-time achievement

As noted in §6.3, Fluvio's team started with strict Kafka protocol compatibility and consciously walked away from it once the maintenance cost of tracking the protocol's evolution stopped being worth it relative to the product features they wanted to build instead. This is worth internalizing directly: protocol compatibility is not a feature you finish, it is an ongoing commitment that grows with every new Kafka release, every new client library version, and every KIP that lands — and a from-scratch implementation should budget for that as a permanent, not one-time, cost.

### 7.6 Common compatibility pitfalls to plan for

Synthesizing across the above sources, the recurring categories of trouble for a from-scratch Kafka-protocol-compatible broker are:

- **Version negotiation edge cases** — correctly handling the `ApiVersions` version-0-fallback-on-`UNSUPPORTED_VERSION` behavior (§1.4) for very old or very new clients; correctly special-casing `ApiVersionsResponse`'s header version (§1.2); getting the exact `flexibleVersions` cutover right per API-per-version rather than assuming one global switchover point (§1.5).
- **Broker/topology identity assumptions baked into clients** — as WarpStream found, some clients treat broker hostname and node ID changes as meaningful signals; a stateless or dynamically-scaling broker fleet needs a deliberate strategy here, not just "return whatever's convenient" in `Metadata` responses.
- **Auto-topic-creation and metadata refresh behavior** — librdkafka and kafka-java both have specific expectations around `Metadata`'s `AllowAutoTopicCreation` flag and how quickly a broker's metadata cache reflects newly created topics/partitions.
- **Record batch downconversion for old clients** — a broker that only ever produces/stores message-format v2 batches must decide whether (and how expensively) to downconvert to v0/v1 for very old consumers, and Kafka's own precedent (KIP-110 rejecting downconversion for zstd) suggests it's reasonable to simply refuse unsupported combinations with a clear error rather than pay for universal downconversion support.
- **Consumer group protocol edge cases** — generation/epoch fencing correctness (`ILLEGAL_GENERATION`, `UNKNOWN_MEMBER_ID`, `FENCED_MEMBER_EPOCH`), and, if supporting both classic and KIP-848 consumer-group protocols, correctly handling mixed-membership groups during a client fleet's rolling migration between the two.
- **Idempotent/transactional producer sequence-number bookkeeping** — the (PID, partition, sequence) dedup logic in §4.2 is easy to get subtly wrong (especially the "cache the last N batches, not just a highwater mark" detail that real Kafka brokers implement) and is exactly the kind of correctness bug that surfaces only under retries/failover, i.e. in production, not in a happy-path test.
- **Latency expectations** — every object-storage-backed system surveyed here (Bufstream, AutoMQ's default S3 WAL mode, and KIP-1150's own diskless-core target) converges on a produce-latency floor in the **hundreds of milliseconds** range, driven by object-storage PUT/GET round-trip time and batching windows. Client libraries and applications tuned against classic Kafka's typically sub-50ms latency will need explicit guidance (larger `linger.ms`, more in-flight requests) to get acceptable throughput against a system in this latency class — this is a compatibility consideration in the practical sense even though it's not a wire-protocol bug.

---

## Sources

**Apache Kafka official documentation and protocol guide**
- [A Guide to the Kafka Protocol / Protocol Guide](https://kafka.apache.org/protocol.html) / [current mirror](https://kafka.apache.org/43/design/protocol/) / [Kafka 4.1 mirror](https://kafka.apache.org/41/design/protocol/)
- [Kafka Message Format (v2) / Implementation guide](https://kafka.apache.org/25/implementation/message-format/)
- [Kafka broker configuration reference (4.1)](https://kafka.apache.org/41/configuration/broker-configs/)
- [Consumer Rebalance Protocol operations guide (4.1)](https://kafka.apache.org/41/operations/consumer-rebalance-protocol/)
- [Apache Kafka source: message JSON schemas](https://github.com/apache/kafka/tree/trunk/clients/src/main/resources/common/message)
- [Apache Kafka source: DefaultRecordBatch.java](https://github.com/apache/kafka/blob/trunk/clients/src/main/java/org/apache/kafka/common/record/DefaultRecordBatch.java)

**KIPs (Kafka Improvement Proposals)**
- [KIP-98: Exactly Once Delivery and Transactional Messaging](https://cwiki.apache.org/confluence/display/KAFKA/KIP-98+-+Exactly+Once+Delivery+and+Transactional+Messaging)
- [KIP-32: Add timestamps to Kafka message](https://cwiki.apache.org/confluence/display/KAFKA/KIP-32+-+Add+timestamps+to+Kafka+message)
- [KIP-33: Add a time based log index](https://cwiki.apache.org/confluence/display/KAFKA/KIP-33+-+Add+a+time+based+log+index)
- [KIP-110: Add Codec for ZStandard Compression](https://cwiki.apache.org/confluence/display/KAFKA/KIP-110%3A+Add+Codec+for+ZStandard+Compression)
- [KIP-482: The Kafka Protocol should Support Optional Tagged Fields](https://cwiki.apache.org/confluence/display/KAFKA/KIP-482:+The+Kafka+Protocol+should+Support+Optional+Tagged+Fields)
- [KIP-511: Collect and Expose Client's Name and Version in the Brokers](https://cwiki.apache.org/confluence/display/KAFKA/KIP-511:+Collect+and+Expose+Client's+Name+and+Version+in+the+Brokers)
- [KIP-227: Introduce Incremental FetchRequests to Increase Partition Scalability](https://cwiki.apache.org/confluence/display/KAFKA/KIP-227%3A+Introduce+Incremental+FetchRequests+to+Increase+Partition+Scalability)
- [KIP-699: Update FindCoordinatorRequest to support batching](https://cwiki.apache.org/confluence/display/KAFKA/KIP-699%3A+Update+FindCoordinatorRequest+to+support+batching)
- [KIP-896: Remove old client protocol API versions in Kafka 4.0](https://cwiki.apache.org/confluence/display/KAFKA/KIP-896%3A+Remove+old+client+protocol+API+versions+in+Kafka+4.0)
- [KIP-429: Kafka Consumer Incremental Rebalance Protocol](https://cwiki.apache.org/confluence/display/KAFKA/KIP-429:+Kafka+Consumer+Incremental+Rebalance+Protocol)
- [KIP-345: Introduce static membership protocol to reduce consumer rebalances](https://cwiki.apache.org/confluence/display/KAFKA/KIP-345:+Introduce+static+membership+protocol+to+reduce+consumer+rebalances)
- [KIP-848: The Next Generation of the Consumer Rebalance Protocol](https://cwiki.apache.org/confluence/display/KAFKA/KIP-848:+The+Next+Generation+of+the+Consumer+Rebalance+Protocol)
- [KIP-405: Kafka Tiered Storage](https://cwiki.apache.org/confluence/display/KAFKA/KIP-405:+Kafka+Tiered+Storage)
- [KIP-1150: Diskless Topics](https://cwiki.apache.org/confluence/display/KAFKA/KIP-1150:+Diskless+Topics)
- [KIP-1163: Diskless Core](https://cwiki.apache.org/confluence/display/KAFKA/KIP-1163:+Diskless+Core)
- [KIP-1164: Diskless Coordinator](https://cwiki.apache.org/confluence/display/KAFKA/KIP-1164:+Diskless+Coordinator)
- [KIP-1165: Object Compaction for Diskless](https://cwiki.apache.org/confluence/display/KAFKA/KIP-1165:+Object+Compaction+for+Diskless)
- [KIP-1147: Improve consistency of command-line arguments](https://cwiki.apache.org/confluence/display/KAFKA/KIP-1147:+Improve+consistency+of+command-line+arguments) (unrelated to diskless topics — noted to avoid confusion)
- KIP-1150 mailing list: [DISCUSS opening](https://www.mail-archive.com/dev@kafka.apache.org/msg149163.html), [DISCUSS reply (Christo Lolov)](https://www.mail-archive.com/dev@kafka.apache.org/msg149275.html), [VOTE restarted](http://www.mail-archive.com/dev@kafka.apache.org/msg153653.html)

**Engineering blogs and vendor documentation**
- [WarpStream: Hacking the Kafka PRoTocOL](https://www.warpstream.com/blog/hacking-the-kafka-protocol)
- [WarpStream: Architecture](https://docs.warpstream.com/warpstream/overview/architecture)
- [WarpStream: Protocol and feature support](https://docs.warpstream.com/warpstream/kafka/reference/protocol-and-feature-support)
- [Redpanda: Kafka Compatibility (Self-Managed)](https://docs.redpanda.com/current/develop/kafka-clients/)
- [Aiven: The Hitchhiker's Guide to Diskless Kafka (KIP-1150)](https://aiven.io/blog/guide-diskless-apache-kafka-kip-1150)
- [AutoMQ: KIP-1150 Explained — Diskless Topics, Kafka's Future](https://www.automq.com/blog/kip-1150-explained-diskless-topics-kafka-future)
- [Bufstream: Kafka data flow in Bufstream](https://buf.build/docs/bufstream/architecture/kafka-flow/)
- [Bufstream: Kafka at 8x lower cost](https://webflow.buf.build/blog/bufstream-kafka-lower-cost)
- [Confluent: Transactions in Apache Kafka](https://www.confluent.io/blog/transactions-apache-kafka/)
- [Confluent: Exactly-once Semantics are Possible: Here's How Kafka Does It](https://www.confluent.io/blog/exactly-once-semantics-are-possible-heres-how-apache-kafka-does-it/)
- [Confluent: Delivery Semantics (design docs)](https://docs.confluent.io/kafka/design/delivery-semantics.html)
- [Confluent: Incremental Cooperative Rebalancing in Kafka](https://www.confluent.io/blog/incremental-cooperative-rebalancing-in-kafka/)
- [Confluent: KIP-848, the Next Generation of the Consumer Rebalance Protocol](https://www.confluent.io/blog/kip-848-consumer-rebalance-protocol/)
- [Confluent: Consumer Group Protocol (architecture course)](https://developer.confluent.io/courses/architecture/consumer-group-protocol/)
- [2 Minute Streaming: acks and min.insync.replicas explained](https://blog.2minutestreaming.com/p/kafka-acks-min-insync-replicas-explained)
- [Instaclustr: Rebalance your Apache Kafka partitions with the next-generation consumer rebalance protocol](https://www.instaclustr.com/blog/rebalance-your-apache-kafka-partitions-with-the-next-generation-consumer-rebalance-protocol/)
- [Medium: Kafka Without Disks — Let's Talk About KIP-1150 Diskless Topics](https://medium.com/@dobeerman/kafka-without-disks-lets-talk-about-kip-1150-diskless-topics-036d29d9cf7e)

**Rust ecosystem**
- [`kafka-protocol` crate on crates.io](https://crates.io/crates/kafka-protocol)
- [`kafka-protocol` on docs.rs](https://docs.rs/kafka-protocol/latest/kafka_protocol/)
- [GitHub: tychedelia/kafka-protocol-rs](https://github.com/tychedelia/kafka-protocol-rs)
- [`rdkafka` crate on crates.io](https://crates.io/crates/rdkafka)
- [GitHub: infinyon/flv-kf-protocol (Fluvio's Kafka protocol crate)](https://github.com/infinyon/flv-kf-protocol)
- [Fluvio / Kafka interoperability discussion](https://github.com/infinyon/fluvio/discussions/1195)
