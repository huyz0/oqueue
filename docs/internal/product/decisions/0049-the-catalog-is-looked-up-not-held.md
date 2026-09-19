# 0049. The catalog is looked up per shard, not held by a node

Status: accepted
Date: 2026-09-20
Requirements: NFR-4, NFR-10, NFR-11, NFR-12

⚠️ **Taken without the user's answer, and saying so**, as `ADR-0046` was: the
user's standing instruction is that a milestone ends against its written gate
on a small number of tasks, and this record is what keeps M7 to that. The user
can override any point before the code that depends on it lands.

## Context

Every topic a node knows about is an entry in `Cluster.topics`, an in-memory
`HashMap` that grows with the whole catalog and is persisted nowhere. At 1M
topics that is the node's memory; at 100M it is not a node at all. NFR-11 asks
for the opposite: a node's metadata cost proportional to the partitions active
on it. Doc 15 §2 already settled *how*: a topic's existence is derived from an
object at a derived key, and creating one provisions nothing else. Doc 15 §3
settled that metadata shards are internal and rebalanceable. What was left
open is how much of that M7 builds to meet its gate, and how the gate measures
"memory" and "latency" in a suite that may not time anything.

## Decision

1. **A topic's shard is a lookup, not a hash in a key.** `MetadataShardId`
   names a shard; a `ShardMap` answers topic → shard. Every object key that is
   per-shard (`meta/<shard>`, `groups/<shard>`, `catalog/<shard>`) is derived
   from the id the map returns. ⚠️ **Amended by M7's closing review**: that
   does not make a move free. A topic's catalog objects sit under its shard's
   prefix, so moving it copies them; an id-addressed request carries no name
   for `ShardMap::shard_of`; and nothing calls `ShardMap` yet — `bin/oqueue`
   uses shard 0 as a constant. Settling where a moved topic's entry lives and
   how an id finds its shard is `M7.12`, handed to M15 with rebalance. ⚠️ **M7 runs one shard**: the map answers shard 0 for every
   topic. Rebalance, a second coordinator, and the safety rules a second shard
   makes reachable (`M7.md` 17a, 17b, 17d) are handed on — see point 6.
2. **The catalog is a seam, `TopicCatalog`, in `oqueue-core`**: look up a
   topic by name, by id, and create it if absent. The broker reads through it
   and holds no map of the whole catalog; what a node keeps is the entries of
   topics it has served, bounded by what it serves.
3. **The object-store catalog writes two create-only objects per topic**
   (`catalog/<shard>/topic/<hex of the name>`, and `catalog/<shard>/id/<hex
   id>` for id-addressed requests, written first — amended by `M7.3`: hex so
   any name is a valid key and key order is name order). Creating a topic writes those and provisions
   nothing — no coordinator, task, timer or log. A topic's UUID is derived
   from its name, so two nodes creating the same topic race to write the same
   bytes and either answer is right. ⚠️ **This makes a deleted-and-recreated
   topic reuse its id**, which KIP-516 clients treat as the same topic. Topic
   deletion does not exist yet; whoever builds it (M12) must change the
   derivation, and this record is where it says so.
4. **An unscoped all-topics `Metadata` is bounded.** With topic grants
   configured, M9's principal index answers it. Without them — no principal
   to scope by — the answer is the first `MAX_UNSCOPED_TOPICS` topics in
   name order, listed from the catalog in pages, never the whole catalog.
   An unscoped cluster is a development shape; M12's admin surface is where
   `--list` at scale gets a real answer (`M7.md` task 12).
5. **The gate measures memory with an allocator and cost with counts.**
   NFR-10/11 are asserted by the live bytes a node holds after serving the
   same active topics over a synthetic catalog of 1M and of 10M topics — a
   `TopicCatalog` that derives each entry on lookup instead of storing it, so
   the test measures the node rather than the fixture. CPU (NFR-12) and
   produce latency (NFR-4) are asserted through catalog lookups and
   allocations per request: `testing.md` rule 11 forbids timing assertions,
   and `check-hot-path-bench.sh` puts hot-path benchmarks in M14. ⚠️ **NFR-4's
   verification method is a benchmark**, so M14 re-measures it in time; M7
   asserts that nothing on the produce path does work that grows with the
   topic count.
6. **Everything else in `M7.md`'s provisional list is handed on**, recorded in
   `roadmap.md`'s deferral table and received by `M15.md`: rebalance, RF-3
   shard replication, node classes, the metadata-role boundary, the
   reconciliation watermark, the billing sweep, KIP-951 hints, the client
   documentation, the per-connection observable, the parked-fetch
   measurement, and 17a/17b/17d with the 404 refresh. Each of 17a, 17b and 17d
   is reachable only once a second shard or a follower cache exists, and none
   does after this milestone.

## Consequences

- A cold node answers its first request for a topic with one catalog GET;
  every request after that is served from the node's own entries.
- `topic_name_by_id`, a linear scan today, becomes one catalog lookup.
- The scale evidence is about the node. The catalog's own cost at 100M topics
  is object-storage cost — one small object per topic — which M14 prices.

## Contract

`TopicCatalog` is a new `pub trait` in `oqueue-core` (`M7.2`, `contracts.md`
rule 12): `lookup`, `lookup_id`, `create`, `list`, with `FakeTopicCatalog`
beside it and its four guarantees as contract cases in `catalog/tests.rs`,
which every implementation runs. ⚠️ **Async, unlike `MaterializedIndex`**:
the catalog is object storage's (point 3), so a lookup is a network read, and
that seam's argument for being synchronous — every implementation is a local
fold — does not hold here.

`MaintenanceStore::list(prefix, after, limit)` is added by `M7.3a`, a new
`pub trait` in `oqueue-core` (`contracts.md` rule 12) and the separate listing
seam `ADR-0009` §2 named and deferred. `TopicCatalog::list` must page topic
names in order, and the store it reads had no way to enumerate anything:
`ObjectStore` still carries no `list`, deliberately, so only a component
explicitly handed a `MaintenanceStore` can LIST. The method answers keys under
a plain string prefix, strictly after `after`, in byte order, at most `limit`
— the shape both backends already have natively, since S3's `ListObjectsV2`
and GCS's list both page lexically from a start-after, so each page is one
request and no implementation buffers a listing. ⚠️ **`ADR-0046`'s rule is
unchanged**: the metadata log still opens without a LIST, and nothing on the
produce, fetch or recovery path is handed a `MaintenanceStore`; it exists for
the catalog alone.
