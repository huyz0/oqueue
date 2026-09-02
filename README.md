# oqueue

A Kafka-protocol-compatible message broker in Rust that uses object storage
(S3/GCS) as its **primary** log storage — not a cache, not a tier below a disk.

[![License](https://img.shields.io/badge/license-Apache--2.0-blue.svg)](LICENSE)

---

> ## ⚠️ Status: no implementation yet
>
> **There is nothing to run.** The broker does not exist. What is here is a
> research corpus, the development system that builds it, and — as of M0's
> completion — a Cargo workspace with all eleven crates in it, every
> `oqueue-core` trait seam with a fake beside it, and the coverage and mutation
> gates the workspace now enforces on itself — and M1's completion: the
> object store seam with S3 and GCS backends behind it, verified against
> the fake and MinIO by one conformance suite (GCS live verification is
> M15's) — and M2's completion: a hand-rolled Kafka wire protocol codec
> (`ADR-0019`) a real client can produce and fetch against — and M3's: the
> coordinator, where offsets are sequenced and a reader resolves them through
> an index rather than by listing anything — and M10's: the deterministic
> simulation harness every later failure claim is verified by, and M11's:
> idempotent producers — deduplicating a retried batch by sequence number,
> which a real client's own `enable.idempotence` opt-in needs. M9, in
> progress, is authentication, authorization, and tenant isolation — every
> operation scoped to a principal, and a `Metadata` request costing
> O(topics this principal can see) rather than O(topics that exist).
>
> What is here is worth reading if you are interested in the design space:
> ~110,000 words of cited research on object-storage-native streaming, and an
> unusually explicit engineering standard.

---

## The idea

Classic Kafka's cost is dominated by two things that have nothing to do with
storing bytes: **cross-AZ replication traffic** and **local disks sized for peak
retention**. Writing straight to a regional object store removes both. The
object store already replicates across availability zones, charges nothing for
that replication, and bills only for what is stored.

The consequences are structural rather than incremental:

- **No partition leadership.** Nothing to elect, nothing to fail over.
- **No inter-broker replication.** No follower lag, no ISR, no rebalance storms.
- **Stateless nodes.** A node that dies is replaced, not recovered.
- **No local disk in the durability path.**

This is the architecture WarpStream pioneered and that AutoMQ, Redpanda Cloud
Topics, and Kafka's own [KIP-1150](https://cwiki.apache.org/confluence/display/KAFKA/KIP-1150:+Diskless+Topics)
now pursue.

## What it costs

Stated plainly, because the tradeoff is the whole point:

**Produce latency is ~300–500 ms p99.** A durable PUT to S3 Standard takes
60–250 ms and no amount of engineering makes it faster. That is roughly fifty
times slower than a local-disk Kafka ack. Systems advertising single-digit
milliseconds on object storage are using a faster durable medium or have
weakened a guarantee — see
[docs/researches/17](docs/researches/17-latency-budget.md) for the
decomposition and a cross-system comparison.

**Tail reads are the opposite case** and are genuinely single-digit ms, because
the write cache *is* the tail buffer and is populated at the moment of
durability — never before it.

If you need single-digit-ms produce, you want a different system, and this one
will not pretend otherwise.

## The scale target

Tenant ≈ topic. **1M–100M topics**, up to ~1000 partitions each.

That requirement is what rules out the obvious designs. A central metadata store
holding every partition does not reach it, and neither does a metadata plane
whose per-node cost is proportional to *total cluster* partitions rather than to
the partitions that node actually serves. The rule this produces —
**metadata cost must be proportional to active partitions on this node** — is
derived in [docs/researches/15](docs/researches/15-scale-architecture-position.md)
and [16](docs/researches/16-automq-deep-dive.md).

## Encryption

Two optional capabilities, neither of which burdens deployments that skip them:

- **BYOK, configured per topic**, portable across AWS KMS and GCP Cloud KMS.
  Server-side encryption cannot express this — one object carries many tenants'
  data, and an object can carry only one SSE-KMS key — so encryption is
  broker-side envelope encryption. BYOK is a rare opt-in path, so its data is
  segregated into its own objects and the default path stays unchanged.
- **FIPS 140-3 as a separate build**, using the validated `aws-lc-rs` module.
  Separate because it needs a Go toolchain at build time, which no other user
  should have to install.

Derivation is in
[docs/researches/22](docs/researches/22-encryption-byok-and-fips.md).

## What's in this repository

| Path | Contents |
|---|---|
| [`docs/researches/`](docs/researches/README.md) | 22 documents of cited research compiled before any code: reference architectures, object-storage physics, Kafka protocol, cost models, latency budgets, and this project's engineering standards. **Start with its README** — it maps the rest. |
| [`docs/internal/product/`](docs/internal/product/) | Mission, **requirements** (functional and non-functional, with stable IDs), architecture, roadmap, backlog, and architecture decision records. |
| [`docs/internal/standards/`](docs/internal/standards/) | Four binding families: process (SDD), quality (security, performance, testing), delivery (build, portability), and code style. Each rule names its gate. |
| [`AGENTS.md`](AGENTS.md) | The working agreement. Read this before changing anything. |

Some entry points worth the time even if you never use oqueue:

- [Object discovery and API cost](docs/researches/12-object-discovery-and-api-cost.md) — why LIST is both semantically useless and 12–38× the price of a GET, and what every real system does instead
- [Latency budget](docs/researches/17-latency-budget.md) — what is actually achievable on object storage, and what vendor claims omit
- [AutoMQ deep dive](docs/researches/16-automq-deep-dive.md) — a source-code study that corrected several claims this corpus had taken from marketing material
- [Rust performance methodology](docs/researches/18-rust-performance-methodology.md) — instruction-count benchmarking, and why it is partly incompatible with SIMD work

## How it is built

oqueue is developed **spec-first and entirely by AI agents**, with no manually
written code. That is an unusual constraint and it drives most of the
engineering practice here:

- **Deterministic gates carry as much as possible.** Anything a script can
  decide is never delegated to an agent's judgement, because a check performed
  by an agent cannot be regression-tested.
- **Code and review are performed by different agents.** The reviewer receives
  the task and the diff but never the author's reasoning — a rationale generated
  to make a change look correct is persuasive by construction.
- **Review is an artifact, not a claim.** The verdict is keyed to the hash of
  the staged diff, so a pre-commit gate can require it and amending one byte
  afterwards invalidates it.
- **Mutation testing is the primary anti-slop gate**, because the most
  characteristic AI failure is a test that executes code without constraining
  it — which is definitionally a surviving mutant.

The governing principle throughout: **a rule with no gate is a preference.**

The full design is in
[docs/researches/21](docs/researches/21-ai-development-loop.md), and the
workspace standard in
[19](docs/researches/19-workspace-engineering.md).

## Roadmap

| Milestone | | State |
|---|---|---|
| M-1 | AI development system | complete |
| M0 | Workspace, contracts, quality gates | complete |
| M1 | Object store seam and conformance suite | complete |
| M2 | Kafka wire protocol: produce and fetch | complete |
| M3 | Coordinator: offset sequencing and the index | complete |
| M10 | Deterministic simulation and fault injection | complete |
| M11 | Idempotent producers | complete |
| M9 | Authentication, authorization, tenant isolation | in progress |
| M4 | Consumer groups | not started |
| M5 | Compaction and retention | not started |
| M6 | Recovery and failover | not started |
| M7 | Metadata sharding and scale | not started |
| M8 | Encryption: BYOK and the FIPS build | not started |

Every milestone's completion condition is a command rather than a judgement.
See [roadmap.md](docs/internal/product/roadmap.md).

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Short version: issues and design
discussion are welcome now; code contributions are not being accepted yet,
because there is no implementation to contribute to — the development system
that reviews changes is finished (M-1), the workspace and its crates are built
(M0), the object store seam has real backends (M1), the Kafka wire protocol
is a real client's produce and fetch against (M2), the coordinator sequences
offsets and indexes them (M3), M10's deterministic simulation harness verifies
every later failure claim, and M11, idempotent producers, is in progress.

## License

[Apache-2.0](LICENSE).
