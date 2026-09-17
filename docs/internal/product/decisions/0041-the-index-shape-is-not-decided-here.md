# 0041. The index's shape is not decided here, and what any shape must be priced in

Status: accepted — ⚠️ **this ADR decides no shape.** It records three that were
derived and refuted, the quantity every one of them was mispriced in, and what
a fourth must be measured against before it is proposed.
Date: 2026-09-18
Requirements: FR-13 (a fetch is one ranged GET), FR-34 (compaction), NFR-11
(index growth quota)

## Context

`M3.6` built the index's two-tier shape and its own doc records what it did
not do: entries are keyed per `(object, partition)`. Doc 14 §3 prices that at
**~4M entries/s and ~160 MB/s** against **~400/s and ~16 KB/s** for a coarser
index. `M3.8` deferred the re-keying into `M5`, and `M5.9` is where it landed.

⚠️ **Three shapes were derived for this ADR and review refuted all three, each
on the same quantity.** That is the finding, and it is worth more than a
fourth draft would be.

1. **One entry per object listing each partition's base offset.** One entry
   carrying P numbers is the same state in a different layout.
2. **One entry per object plus a sparse per-partition anchor.** The anchors
   index a list shared by the topic and a history entry has no partition in
   it, so between two anchors the walker cannot tell which objects touch the
   partition without reading each footer. On a topic holding one object an
   hour beside one object a second, two adjacent anchors 64 apart are ~230,000
   positions apart, so "at most `interval` footer reads" is not a bound. And
   the state is (1)'s divided by the interval, so reaching ~400/s needs an
   interval of ~10,000 — that many footer reads on a cold fetch.
3. **A manifest per partition, chained two levels** (doc 14 §3 friction row 3's
   own mitigation, "two-level manifest, mandatory"). Nesting divides the entry
   *count* by the fan-out and leaves the manifest's *size* alone: every entry
   carries a range per partition, and `CompositeBuilder` copies a component's
   whole region list, so at doc 14's working set a fan-out-100 manifest holds
   ~1M ranges — tens of megabytes, GET whole on a cold fetch, at both levels.
   Redpanda's two-level fix bounds size because a manifest there is
   per-partition; here it bounds only depth.

⚠️ **Every one of the three was priced in entries and killed by something
else.** (1) and (2) died on state size, (3) on manifest bytes, and none of the
three had been priced against writes at all — doc 14 §3's friction 4 measures
**~15 successful conditional writes/s per key on S3 and ~1/s on GCS**, and a
root manifest per partition makes the root-publication rate scale with the
partition count.

## Decision

**No shape is decided here. The next proposal is priced before it is
proposed**, in these three quantities, at doc 14 §3's working set rather than
at a convenient one:

1. **Coordinator state**, in entries *and bytes*, steady-state and at the
   working set's skew — the quantity doc 14 §3's table is denominated in.
2. **Bytes per manifest read on a cold fetch**, including every level a read
   traverses. This is what refuted shape 3 and what neither of the first two
   was asked.
3. **Manifest writes per compaction round per key**, against friction 4's
   ~15/s on S3 and ~1/s on GCS. ⚠️ **Nothing in this repository has ever been
   priced against that number**, and the shape that eventually wins will live
   or die on it, because a per-partition manifest is a per-partition key.

⚠️ **And the pricing is a task rather than a paragraph.** `M5.60` owns it, and
owns proposing the shape that pricing supports. It may conclude that the
per-`(object, partition)` keying stands and that NFR-11 is met some other way
— that is a legitimate outcome and it is why this ADR does not pre-commit to
coarsening.

## Alternatives considered

- **Decide shape 3 anyway and let the size problem surface in implementation.**
  Rejected: it surfaced in review three times, which is the cheap place, and
  an ADR is where a decision's arithmetic is supposed to be checkable.
- **Defer the whole question out of `M5`.** Rejected: `M3.8` already deferred
  it once into this milestone, and `M5.10`'s NFR-11 quota depends on knowing
  the answer. What is deferred here is one commit, not a milestone.

## Consequences

- ⚠️ **`M5.9` is `dissolved` into `M5.60` alone**, not into the four rows the
  refuted shape needed. Those rows described a mechanism that does not hold.
- ⚠️ **`M5.10` cannot assume coarsening.** Its row inherited "three orders of
  magnitude smaller, so a bound is either unnecessary or cheap" from `M3.8`'s
  deferral; that claim rests on a shape nothing has established. Its row now
  says so, and it also owns the tail's own total — `TAIL_WINDOW_ENTRIES ×
  partitions`, 1M × 128 at doc 14's scale — which `index_state.rs` has stated
  plainly since `M3.6` and which no keying fixes.
- `M5.59`'s ordering gap stays open and stays `M5.8`'s: a manifest's order is
  a per-partition guarantee only for the partition the supplied refs describe.
  It was dissolved into a row derived from shape 3; it is restored.
- ⚠️ **Forecloses nothing, and that is the point of stopping here.** Three
  drafts of this ADR each foreclosed a shape on arithmetic that did not hold.
