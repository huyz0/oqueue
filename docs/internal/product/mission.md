---
title: "Mission"
description: >
  Read first. What this is, what it costs, the scale target, and what it must never do.
tags: [product, purpose, constraints]
---

# Mission

oqueue is a Kafka-protocol-compatible message broker whose primary log storage
is object storage. A producer's records become an object in S3 or GCS, and that
object is the log — not a cache of it, not a tier below it.

## What that buys

Classic Kafka's cost is dominated by two things that have nothing to do with
storing bytes: **cross-AZ replication traffic**, and **local disks sized for
peak retention**. Writing straight to a regional object store removes both. The
object store already replicates across availability zones, charges nothing for
the replication, and bills only for what is stored. There are no partition
leaders to elect, no follower lag, no rebalance storms when a broker dies, and
no local state to rebuild when one comes back.

The nodes are stateless. A node that dies is replaced, not recovered.

## What it costs, stated plainly

**Produce latency.** A durable PUT to S3 Standard is 60–250 ms, and no amount
of engineering makes it faster. Our honest position is ~300–500 ms p99 produce,
which is competitive with WarpStream and with Kafka's own KIP-1150 projection —
and roughly fifty times slower than a local-disk Kafka ack. Systems claiming
single-digit milliseconds on object storage are using a faster durable medium or
have weakened a guarantee. See
[docs/researches/17](../../researches/17-latency-budget.md).

Tail reads are the opposite case and are genuinely single-digit ms, because the
write cache *is* the tail buffer and is populated at the moment of durability.

## The scale target

Tenant ≈ topic. **1M–100M topics**, up to ~1000 partitions each. That is
turbopuffer-class catalog scale, and it is the requirement that rules out the
obvious designs: a central metadata store that must hold every partition, or a
metadata plane whose per-node cost is proportional to total cluster partitions
rather than to the partitions that node is serving.

AutoMQ's ceiling is ~10⁵ partitions, and it is there because KRaft forces a full
cluster image onto every broker. **Metadata cost must be proportional to active
partitions on this node.** See
[docs/researches/15](../../researches/15-scale-architecture-position.md) and
[16](../../researches/16-automq-deep-dive.md).

## Encryption

Two optional capabilities, neither of which may burden deployments that do not
use them:

- **BYOK** — customers supply key material, configured **per topic**, portable
  across AWS KMS and GCP Cloud KMS. Expected on **~10,000 topics** out of
  1M–100M, so it is a rare opt-in path: BYOK data is segregated into its own
  objects by key domain, leaving the >99% default path unchanged. Server-side
  encryption cannot express per-topic keys, because one object carries many
  tenants' data, so encryption is broker-side envelope encryption with a DEK
  per topic.
- **FIPS 140-3** — a **separate build**, using the validated `aws-lc-rs` module.
  Separate because it requires a Go toolchain at build time, which no
  non-FIPS user should pay for.

See [docs/researches/22](../../researches/22-encryption-byok-and-fips.md).

## What it must never do

- **Never acknowledge a write that is not durable.** The cache is populated
  after the PUT succeeds, never before. A consumer must never observe a record
  that a crash would erase.
- **Never break Kafka's ordering or offset guarantees** to gain latency. Lazy
  sequencing buys ~100 ms and costs idempotent producers and transactions —
  which are exactly the features that separate a Kafka-compatible system from a
  lookalike.
- **Never make a client's cost proportional to the size of the cluster.** A
  `Metadata` request must cost O(topics this principal can see), never O(topics
  that exist).
- **Never require LIST on the read path.** It is semantically useless for
  finding a partition's data and priced at 12–38× a GET.
- **Never lose a record that was acknowledged.** Everything else is negotiable.
- **Never let a data encryption key reach a log, span, metric label, error
  variant, or admin response**, and never let one outlive its use unzeroized.
- **Never reuse an AEAD nonce.** Nonces are constructed from writer epoch,
  object sequence, and region index — never drawn at random.

## Non-goals

- Beating Kafka on produce latency. We will not win that and should not pretend
  to; see the cost section.
- Running on a developer's laptop as a production target. macOS is a development
  platform; Linux x86-64 and aarch64 are the release targets.
- A managed service. This is open source under Apache-2.0, and a cloud provider
  offering it as a service is an accepted consequence of that choice.
