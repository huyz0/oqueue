# 0042. A manifest per partition, and the arithmetic that picks it

Status: accepted
Date: 2026-09-18
Requirements: FR-13 (a fetch is one ranged GET), FR-34 (compaction), NFR-11
(index growth quota)

## Context

`ADR-0041` decided no shape. Three were derived for it and review refuted all
three, each on a quantity that draft had not priced — two on state size, one
on manifest bytes, and none against write rate. It required the next proposal
to be priced **first**, in three quantities at doc 14 §3's own working set.
This is that pricing, and then the shape it supports.

### The working set, from doc 14 §3

| Quantity | Value | Source |
|---|---|---|
| Active partitions | 1,000,000 | 100 agents × 10k each |
| Flushed objects | ~400/s | agents × flush rate |
| Partitions per object | ~10,000 | 4M ÷ 400 |
| `(object, partition)` pairs | ~4M/s | doc 14 §3's table |
| Spans per partition | 4/s | 4M ÷ 1M |
| Records per partition | ~4/s | doc 14 §2's "≥1 record per 250 ms" |
| Retention | 7 days = 604,800 s | `M3.md`'s own figure |
| An `ObjectRef` | ~40 B | `index_state.rs`'s inline budget |
| A `TailEntry` | ~56 B | `ObjectRef` + an inline `ByteRange` |

### Quantity 1 — coordinator state

| Shape | Entries | Bytes |
|---|---|---|
| Tail, every shape (`TAIL_WINDOW_ENTRIES` = 128 × 1M partitions) | 1.28e8 | **7.2 GB** |
| History, per `(object, partition)`, 7 days | 2.4e12 | **97 TB** |
| History, per object, 7 days | 2.4e8 | 9.7 GB |
| History, one manifest reference per partition | 1e6 | **40 MB** |

⚠️ **The status quo is not 160 MB/s of growth, it is 97 TB of state**, and
that is the fact the whole question turns on: per-`(object, partition)`
history is not large, it is impossible. ⚠️ **And the tail's 7.2 GB is the
term that survives every shape**, at 128 entries per partition — which at 4
spans/s is **32 seconds** of tail. Nothing about keying moves it.

### Quantity 2 — bytes read on a cold fetch

A partition's history, if it is a list of that partition's objects, is
~`24 B` per entry (an object key reference, an offset, a length) — ⚠️ **an
estimate made before the format existed, and `M5.61` measured ~36 B**, which
moves both figures below up by half and neither column's verdict:

| State | Objects in the partition | Manifest |
|---|---|---|
| Between compaction rounds (30 min at 4/s) | 7,200 | **169 KiB** |
| After compaction (2.42M records ÷ `COMPACTED_OBJECT_RECORDS`) | ~4.6 | **111 B** |

⚠️ **Compaction is what keeps the manifest small**, and the uncompacted
backlog is bounded by the sweep interval rather than by retention — which is
why 169 KiB is the worst case and ~111 B the steady state. ⚠️ **A span here
holds one record**, which falls out of doc 14's own two numbers — 4 spans/s
and ~4 records/s per partition — and is worth saying out loud, because it
means this working set is a million tiny partitions rather than a few large
ones. A workload with fewer, fatter partitions moves the compacted figure up
and the entry count down, and neither direction changes which column binds. It also lands
just over Redpanda's ~128 KiB live-manifest cap, which is the empirical
number doc 14 §7's friction row 3 cites, and is why that row's mitigation chains rather than grows.

A cold fetch is therefore **one GET of ≤128 KiB plus one ranged GET of the
component** — two, or three across a chain hop. Doc 14 §3's "1–3 GETs",
arrived at rather than asserted.

### Quantity 3 — manifest writes per key

One manifest per partition, rewritten once per compaction round:

| Quantity | Value | Against |
|---|---|---|
| Writes per key | 1 per 1,800 s = **5.6e-4/s** | §7 friction 4's ~15/s S3, ~1/s GCS |
| Writes in total | 556/s across 1M distinct keys | — |

⚠️ **Three orders of margin on GCS, the tighter of the two**, and it holds
because the key is per partition and the writer is compaction. §7 friction 4's own conclusion — *"manifests touched only by compaction, never by produce"* —
is a constraint this shape satisfies by construction rather than by rule.

## Decision

**A partition's history is a manifest keyed by that partition**, holding one
entry per object that partition has records in: the object's key, the
partition's base offset, and its byte range. The coordinator holds one
reference to it.

1. ⚠️ **Per *partition*, not per object set.** This is what the three refuted
   shapes all missed and what the pricing shows: a manifest listing every
   partition of every component is ~1M ranges, while a manifest listing one
   partition's objects is ~4.6 entries after compaction. Same mechanism, five
   orders of magnitude apart, and the difference is what the manifest is
   keyed by.
2. **Chained when it exceeds `PARTITION_MANIFEST_BYTES`**, Redpanda's
   spillover shape, because the uncompacted backlog reaches 169 KiB between
   rounds.
3. **`M5.8`'s composites are orthogonal and both are needed.** A composite
   collapses *object count*, which is the cloud-side cost doc 14 prices in
   PUTs and GETs; a partition manifest collapses *coordinator state*. Neither
   substitutes for the other, and `ADR-0041`'s third shape failed by trying to
   make one do both.
4. **The seam says where, the reader reads.** `MaterializedIndex` gains one
   method, `manifest(topic, partition) -> Option<(ObjectKey, Offset)>`, and
   nothing else: the key and how far it covers. It does **not** gain a way to
   resolve one. That seam is synchronous by contract because every
   implementation of it is a local fold, and resolving a manifest means a GET
   — so putting it behind the index would put object-storage reads inside the
   thing whose whole job is to say which objects to read. `M5.63` reads it in
   `oqueue-broker`, where the other tiers' reads already are.

   ⚠️ **Below `upto`, `find_batches` names nothing**, and that is what makes
   the two tiers abut rather than overlap: the entries it would have named are
   exactly what the manifest replaced. A reader asks for both and concatenates
   — the manifest tier from `start`, the index tier from `start` — because a
   fetch that stopped at the boundary would park a consumer there forever.
5. **The tail is unchanged and unbounded in total.** 7.2 GB at the working
   set, 32 seconds deep, untouched by any of this. It is `M5.10`'s, and this
   decision makes that the *only* remaining half of NFR-11 rather than one of
   two.

## Alternatives considered

- **The three shapes `ADR-0041` refuted.** Each is priced above and each fails
  a column: per-`(object, partition)` on state, per-object-with-anchors on
  cold-read bytes, the composite chain on manifest bytes.
- **Keep history in the coordinator and bound it by eviction.** 97 TB is not a
  number eviction reaches, and `M3.11` already found eviction gives back range
  a rebuild cannot restore.
- **A manifest per topic.** Divides the write rate by partitions-per-topic and
  multiplies the manifest by it: a 10k-partition topic's manifest is ~46k
  entries after compaction, ~1.1 MB, GET whole per fetch — against ~111 B. This is shape 3 with
  a different key, and it fails the same column.

## Consequences

- `M5.61`, `M5.62` and `M5.63` are the implementation: the format, the
  coordinator's single reference, and the fetch path with its read count
  asserted.
- ⚠️ **NFR-11's remaining half is the tail alone**, which `M5.10` owns with a
  mechanism rather than a ceiling.
- `M5.59`'s ordering gap is answered for *reads*: a fetch reads a partition
  manifest, whose entries carry that partition's offsets, so nothing depends
  on a composite manifest's order. The composite's own ordering claim stays as
  `M5.8` left it, and `M5.59` stays open for what it actually governs.
- ⚠️ **`PARTITION_MANIFEST_BYTES` is undecided here.** 128 KiB is Redpanda's
  measured cap and this repository has measured nothing; `M5.61` picks a
  starting value with its direction named, the way
  `COMPACTION_READ_AMP_THRESHOLD` did.
- Forecloses: a coordinator that answers a history fetch without reading an
  object. The pricing says that is not available at this working set.
