# Requirements

Functional and non-functional requirements, with stable IDs.

## How to read this

**A requirement that cannot be verified is not a requirement.** It is a wish,
and wishes do not survive contact with an autonomous loop. So every entry
carries a **verification** column naming what would prove it holds — a test, a
gate, a benchmark, or a conformance suite.

**A non-functional requirement without a number is not a requirement either.**
"Low latency" is a sentiment. `p99 produce ≤ 500 ms` is checkable. Where a
number is not yet known, the entry says **UNDERIVED** and names what blocks it.
⚠️ **Do not invent one.** An invented number becomes an unexamined constraint
the moment somebody designs against it.

**Status** is one of:

| | |
|---|---|
| `agreed` | Stated and stable |
| `provisional` | Stated, but derived from research rather than from a stakeholder — may move |
| `underived` | The requirement is real; the number or detail is not yet known |
| `deferred` | Explicitly out of scope for v1, with the milestone that would take it up |

IDs are **stable and never reused**. Specs cite them, tasks cite the spec, and
commits cite the task — that chain is what makes the loop auditable, and it is
the reason these have IDs at all.

---

## Functional requirements

### Protocol

| ID | Requirement | Verification | Status |
|---|---|---|---|
| FR-1 | Speak the Kafka wire protocol over TCP such that unmodified Kafka clients interoperate. | Conformance suite against real client libraries (`librdkafka`, `kafka-python`, the Java client) | agreed |
| FR-2 | Support `ApiVersions` negotiation and advertise only APIs actually implemented. | Protocol conformance test asserting no advertised API returns `UNSUPPORTED_VERSION` | agreed |
| FR-3 | Encode and decode RecordBatch v2, including CRC-32C (Castagnoli), varint fields, and compression codecs. | Differential test against a reference encoder; property test round-tripping arbitrary batches | agreed |
| FR-4 | Serve `Metadata` requests scoped to the authenticated principal. | Test asserting a principal sees only its own topics; cost test asserting response size is O(principal's topics) | agreed |

### Produce and fetch

| ID | Requirement | Verification | Status |
|---|---|---|---|
| FR-10 | Acknowledge a produce request only after the data is durable in object storage. | Fault-injection test: kill between PUT and ack, assert no acked record is lost | agreed |
| FR-11 | Assign offsets that are monotonic and gap-free per partition. | Property test over concurrent producers; invariant check in the coordinator | agreed |
| FR-12 | Serve tail reads from cache without an object-storage round trip. | Test asserting a fetch at the high watermark issues zero GETs | agreed |
| FR-13 | Serve historical reads by resolving offset→object through the index and issuing a ranged GET. | Test asserting a cold fetch issues a bounded number of GETs and **zero LIST** operations | agreed |
| FR-14 | Support idempotent producers (`enable.idempotence=true`). | Client conformance test: duplicate sequence numbers are deduplicated | provisional |
| FR-15 | Support transactions and exactly-once semantics. | Client conformance test with `transactional.id` | **deferred** — see open question #5; candidate for post-v1 |

### Consumer groups

| ID | Requirement | Verification | Status |
|---|---|---|---|
| FR-20 | Support consumer groups: join, sync, heartbeat, rebalance. | Conformance test with multiple consumers joining and leaving | agreed |
| FR-21 | Store and serve committed offsets durably. | Restart test: offsets survive a full broker fleet restart | agreed |
| FR-22 | Support the KIP-848 next-generation rebalance protocol. | Conformance test against a KIP-848-capable client | provisional — v1 may ship the classic protocol only |

### Storage and lifecycle

| ID | Requirement | Verification | Status |
|---|---|---|---|
| FR-30 | Use object storage as the primary log store, with no local disk in the durability path. | Test asserting the durability path issues no filesystem write; sans-I/O gate | agreed |
| FR-31 | Support S3 and GCS behind one seam, with an in-memory implementation for tests. | Conformance suite run against every backend, recording which ones it has been run against | agreed |
| FR-32 | Batch records from many topics into one object. | Test asserting a flush with N topics issues one PUT | agreed |
| FR-33 | Enforce time- and size-based retention, including on partitions nobody is writing to. | Test asserting an idle partition's data is deleted on schedule | agreed |
| FR-34 | Compact objects so that historical reads stay bounded in cost. | Test asserting read amplification after compaction is within bound | agreed |
| FR-35 | Delete objects only after no reader can still reference them. | Invariant test on the GC safety inequality — see doc 12 §4.6 | agreed |

### Multi-tenancy and security

| ID | Requirement | Verification | Status |
|---|---|---|---|
| FR-40 | Authenticate clients and scope every operation to the authenticated principal. | Test asserting cross-principal access is refused on every API | agreed |
| FR-41 | Support customer-supplied keys (BYOK) configured per topic, portable across AWS KMS and GCP Cloud KMS. | Round-trip test through the same seam against both providers | agreed |
| FR-42 | Segregate BYOK data into objects scoped to one key domain. | Test asserting no object contains regions from two key domains | agreed |
| FR-43 | Provide a FIPS 140-3 build using a validated cryptographic module. | Runtime assertion that FIPS mode is enabled; differential test proving FIPS and non-FIPS builds read each other's data | agreed |
| FR-44 | Never allow key material or credentials to reach a log, span, metric label, error variant, or admin response. | Static gate on secret types; log scan in the end-to-end suite | agreed |

### Operations

| ID | Requirement | Verification | Status |
|---|---|---|---|
| FR-50 | Run as a single binary whose role is chosen by configuration. | Smoke test starting each role from one artifact | agreed |
| FR-51 | Tolerate the loss of any node without data loss or manual intervention. | Chaos test killing nodes under load | agreed |
| FR-52 | Expose metrics, structured logs, and traces sufficient to diagnose a stalled partition. | Review against a written list of diagnosable scenarios | provisional |
| FR-53 | Provide an admin API for topic lifecycle and configuration. | Conformance test against the Kafka `AdminClient` | agreed |

---

## Non-functional requirements

### Latency

| ID | Requirement | Verification | Status |
|---|---|---|---|
| NFR-1 | Produce **p99 ≤ 500 ms**, p50 ≤ 300 ms, on S3 Standard. | Benchmark on a fixed workload, gated in CI as a regression check | provisional — derived in doc 17, not from a stakeholder |
| NFR-2 | Tail-read **p99 ≤ 10 ms** when the data is in cache. | Benchmark asserting zero object-storage round trips at the high watermark | provisional |
| NFR-3 | Historical-read p99 within one object-storage GET plus index resolution. | Benchmark with a cold cache | provisional |
| NFR-4 | Produce latency must not degrade with total topic count. | Benchmark at two topic counts an order of magnitude apart | agreed |

⚠️ **NFR-1 is the project's most consequential public claim.** It is roughly
fifty times a local-disk Kafka ack, and the mission states it plainly for that
reason. It must be measured, not asserted.

### Scale

| ID | Requirement | Verification | Status |
|---|---|---|---|
| NFR-10 | Support **1M–100M topics**, up to ~1000 partitions each. | Scale test with a synthetic catalog; memory and metadata cost measured, not extrapolated | agreed |
| NFR-11 | Per-node metadata cost proportional to **partitions active on that node**, never to total cluster partitions. | Test asserting node memory is flat as cluster-wide partition count grows | agreed |
| NFR-12 | `Metadata` response cost O(topics the principal can see). | Test asserting response size is independent of catalog size | agreed |
| NFR-13 | Aggregate ingest throughput. | Benchmark | **UNDERIVED** — blocked on open question #25; gates the fast-tier decision (#7) and shared-WAL sizing |
| NFR-14 | BYOK on ~10,000 topics without affecting the default path. | Test asserting default-path object format and cost are unchanged when BYOK topics exist | agreed |

### Durability and availability

| ID | Requirement | Verification | Status |
|---|---|---|---|
| NFR-20 | No acknowledged record is ever lost. **This is the requirement everything else yields to.** | Fault injection across every failure point in the write path; deterministic simulation with seeded schedules | agreed |
| NFR-21 | A consumer must never observe a record that a crash would erase. | Invariant test: the cache is never ahead of durability | agreed |
| NFR-22 | Recovery time objective after coordinator loss. | Timed failover test | **UNDERIVED** — open question #15; hot-standby failover and cold rebuild are separate numbers |
| NFR-23 | Availability target. | Measured over a soak test | **UNDERIVED** — no stakeholder figure |

### Cost

| ID | Requirement | Verification | Status |
|---|---|---|---|
| NFR-30 | No LIST operation on the read path. | Gate asserting the read path issues zero LIST calls | agreed |
| NFR-31 | Object-storage API cost per GB ingested must stay within a stated bound. | Cost model test counting API calls per GB on a fixed workload | provisional — bound UNDERIVED, blocked on NFR-13 |
| NFR-32 | No cross-AZ transfer charge on the default write path. | Architecture review plus a test asserting single-AZ egress | agreed |
| NFR-33 | KMS operations must not scale with flush rate. | Test asserting KMS call count is a function of DEK rotation, not of produce volume | agreed |

### Portability and operability

| ID | Requirement | Verification | Status |
|---|---|---|---|
| NFR-40 | Run on Linux x86-64 and aarch64, both first-class. | CI matrix building and testing on both | agreed |
| NFR-41 | Binaries run on glibc 2.28 and newer. | Build with a pinned glibc floor; smoke test on the oldest supported distribution | agreed |
| NFR-42 | Build with only cargo and a C compiler, except the FIPS build. | Clean-container build test | agreed |
| NFR-43 | Develop and run the fast test tiers on Linux and macOS. | CI matrix including macOS | agreed |
| NFR-44 | Nodes hold no durable local state; a lost node is replaced, not recovered. | Test asserting a node starts clean and serves correctly with an empty disk | agreed |

### Engineering

These constrain the codebase rather than the product, and are enforced by gates
rather than tests. Full statements in `../standards/`.

| ID | Requirement | Verification | Status |
|---|---|---|---|
| NFR-50 | Every commit leaves the tree green. | Pre-commit hooks | agreed |
| NFR-51 | Business logic is sans-I/O. | `check-sans-io.sh` | agreed |
| NFR-52 | Crate dependencies are unidirectional. | `check-layering.sh` | agreed |
| NFR-53 | `unsafe` confined to three named crates. | `check-unsafe.sh` | agreed |
| NFR-54 | Every commit reviewed by an agent that did not author it. | `check-reviewed.sh` | agreed |
| NFR-55 | Per-crate line coverage threshold. | `check-coverage.sh` against a constant | **UNDERIVED** — constant not yet chosen |
| NFR-56 | Pre-commit suite completes within a time budget. | `check-budget.sh` against a constant | **UNDERIVED** — constant requires a workspace to measure |

---

## What is not required

Recording these prevents them being reintroduced as assumptions:

- **Beating Kafka on produce latency.** Structurally impossible on object storage; see NFR-1.
- **Running in production on macOS or Windows.** macOS is a development platform; Windows is reached through WSL2.
- **A managed service.** Open source under Apache-2.0, and a third party offering it as a service is an accepted consequence.
- **Sub-100 ms produce on S3 Standard.** Requires a faster durable medium or a weakened guarantee. Kept as a pluggable tier, not a target.
- **Feature parity with Kafka's full API surface in v1.** FR-15 and FR-22 are explicitly staged.

---

## Traceability

```
requirement (FR-n / NFR-n)
  └── milestone            roadmap.md, with a completion condition that is a command
       └── task            backlog.md, with acceptance criteria citing the requirement
            └── commit     subject names the task ID
                 └── test  names the requirement it verifies
```

⚠️ **A requirement with no milestone is not scheduled. A milestone with no
requirement is not justified.** Both are findings for the milestone-boundary
review, not facts to live with.

⚠️ Requirements marked **UNDERIVED** are the project's real open risk: NFR-13
(throughput) gates several architectural decisions and blocks NFR-31, and both
NFR-55 and NFR-56 are constants that cannot be chosen until there is something
to measure. They are tracked in
[docs/researches/10-open-questions.md](../../researches/10-open-questions.md).
