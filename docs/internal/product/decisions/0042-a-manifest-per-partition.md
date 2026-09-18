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
**84 B per entry**, and that number is measured rather than estimated:
`an_entry_costs_thirty_bytes_plus_its_object_key` in
`crates/oqueue-core/tests/it/partition_manifest.rs` seals two manifests and
subtracts. It is 30 B the format decides — two bytes of key length, an offset,
a record count, and a range — plus the object key the caller brings, which for
`BundleNamer`'s `bundles/{writer}/{sequence:020}` is 51–54 B — quoted at its
wide end, because the figure bounds a cold read.

⚠️ **This paragraph has been wrong three times, in the same direction.** It
said `24 B` first, an estimate made before the format existed; `M5.61`
corrected it to ~36 B, which was the encoder measured against a four-character
*test* key; and `M5.64`'s own first draft said 80 B, from a key whose
timestamp field was four digits short. All three understated it, and the rule that came out of `M5.64` is the one above:
derive the figure from the encoder and a real key, in a test, and quote the
test.

| State | Objects in the partition | Manifest |
|---|---|---|
| Between compaction rounds (30 min at 4/s) | 7,200 | **590 KiB** |
| After compaction (2.42M records ÷ `COMPACTED_OBJECT_RECORDS`) | ~4.6 | **386 B** |

⚠️ **Compaction is what keeps the manifest small**, and the uncompacted
backlog is bounded by the sweep interval rather than by retention — which is
why 590 KiB is the worst case and ~386 B the steady state. ⚠️ **A span here
holds one record**, which falls out of doc 14's own two numbers — 4 spans/s
and ~4 records/s per partition — and is worth saying out loud, because it
means this working set is a million tiny partitions rather than a few large
ones. A workload with fewer, fatter partitions moves the compacted figure up
and the entry count down, and neither direction changes which column binds. ⚠️ **The worst case is
about four and a half times Redpanda's ~128 KiB live-manifest cap**, the
empirical number doc 14 §7's friction row 3 cites — not "just over" it, which
is what this said while the entry was priced at 24 B. The correction
strengthens the row's own conclusion rather than weakening it: chaining is not
a margin call, it is the only thing that keeps a cold fetch from reading half
a megabyte.

A cold fetch is therefore **one GET of ≤128 KiB per chain link followed, plus
one ranged GET of the component**. The cap bounds each link — at 84 B an entry
it holds 1,560 of them — and how many links there are is what the entry width
decides.

⚠️ **Which is where doc 14 §3's "1–3 GETs" holds and where it does not**, and
this paragraph claimed the bound rather than the steady state until `M5.64`
recomputed it. Compacted, a partition's manifest is ~386 B: one manifest GET
and one ranged GET, which is the 2 that row asks for, and 3 across a hop. The
*uncompacted* backlog is 590 KiB, which is **five** chained manifests, so a
consumer cold-fetching the oldest offset late in a sweep interval pays six.
`an_uncompacted_backlog_is_five_chained_manifests` derives it.

⚠️ **It is bounded, and it is bounded by the sweep interval rather than by
retention** — the same fact that makes the manifest small is what makes the
chain short, and `MAX_MANIFEST_HOPS` refuses a chain past sixteen whatever the
data says. But six is not three, and the honest statement is that the read
column this shape was chosen on is met in the steady state and exceeded
against a backlog nothing has swept yet. Shortening the sweep interval is the
lever, and it is `M5.16`'s.

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
   spillover shape, because the uncompacted backlog reaches 590 KiB between
   rounds — five links, not a margin.
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
  entries after compaction, ~3.9 MB at 84 B an entry, GET whole per fetch —
  against ~386 B. This is shape 3 with a different key, and it fails the same
  column.

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
