# 0025. A materialized index says how large it is

Status: accepted
Date: 2026-08-26
Requirements: NFR-11, NFR-2

## Context

`M3.11` set out to make index growth an enforced quota and found the
enforcement unachievable at M3's index keying — eviction gives back range that a
rebuild cannot restore, because replaying the metadata log reproduces the same
entry count and sheds the same entries again. `roadmap.md` carries the
enforcement to `M5`, beside the coarse per-object re-keying that makes a bound
feasible, and `M7` is where NFR-11 is verified.

What M3 ships instead is the **number**: growth as a fact somebody can read
rather than one discovered by running out of memory. `IndexState` maintains a
running count and `MemoryIndex` exposes it.

⚠️ **And that is worth almost nothing where it matters, because of
`ADR-0024`.** A coordinator takes its index as `Box<dyn MaterializedIndex>`
precisely so the caller keeps no writable handle, and hands back an
`IndexReader`. Neither the trait nor that reader has an entry count, and a
`Box<dyn Trait>` cannot be cloned or downcast — so once `M3.14` composes
`Coordinator::open(log, Box::new(MemoryIndex::new()), epoch)`, **no expression
anywhere yields the entry count of the index that folds every partition on the
shard**. That is the one this measurement is about. The follower path is fine
only by accident: `LogApplier::new` takes an `Arc`, so its caller may keep a
concrete handle.

A number that exists and cannot be read on the path it was written for is a
dashboard nobody is wired to.

## Decision

**`MaterializedIndex` gains `entries()`, and `IndexReader` exposes it.**

```rust
fn entries(&self) -> usize;
```

1. It is a **read**, so it belongs on the reader too — every holder that can
   ask how far an index has folded can ask how much it is holding.
2. Every implementation already has it: both materializations delegate to
   `IndexState`, which maintains the count rather than traversing.
3. It is *not* a quota, a limit, or a policy. An implementation that cannot
   count cheaply may return an estimate, and the doc says so; what it may not
   do is traverse its partition map, because this is read on paths NFR-2
   bounds.

⚠️ **Additive, and it still narrows what an implementor guarantees**, which is
why `contracts.md` rule 15 asks for this ADR rather than letting it land as an
ordinary commit.

## Alternatives considered

**Leave it on the concrete type and record the limitation.** Rejected: the
limitation is that the measurement does not reach the index it was written for.
Recording it would leave `M3.11` delivering, on its own main path, nothing —
and the row's whole amended purpose is that growth stops being invisible.

**Give `Coordinator` an `index_entries()` of its own.** Rejected as the same
method spelled twice. The coordinator holds an `Arc<dyn MaterializedIndex>`, so
it would have to reach the number through the trait anyway; putting it there
once serves the follower path, `M12`'s admin API and `M5`'s quota alike.

**Have `Coordinator::open` take a concrete type or return the `Arc`.**
Rejected: that is `ADR-0024` undone. The `Box` is what makes the sole-writer
rule structural, and trading it for an accessor would be a large regression for
a small convenience.

**Add a richer statistics method** — entries, bytes, partitions, evictions.
Rejected for now on `error-handling.md` rule 6's shape: a field nothing can
produce is a claim that is false the day it is written. `MemoryIndex` can count
entries; it cannot cheaply say how many *bytes* it holds without walking every
`ObjectKey`, and eviction does not exist. `M12`'s admin API is where a
statistics surface is designed against real consumers.

## Consequences

**Makes easy:** `M5`'s quota reads the same number its enforcement acts on, and
`M12` can expose it without a seam change. A coordinator's index stops being the
one materialization nobody can measure.

**Makes hard:** every future `MaterializedIndex` — doc 10 #12's engine above
all — must be able to answer this cheaply. For a disk-backed engine that is a
maintained counter rather than a `SELECT count(*)`, and the trait doc says so.

**Forecloses:** nothing. It is additive, and the estimate escape hatch keeps it
implementable by an engine whose exact count is expensive.
