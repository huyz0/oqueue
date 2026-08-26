# 0022. The index read surface: `find_batches`, and what a high watermark is

Status: accepted; 2026-08-25 (`M3.24`, from M3's checkpoint milestone review):
⚠️ **decision 1's rule-3 argument named the wrong second implementation.** It
said "two implementations exist today", meaning `MemoryIndex` and
`FakeMaterializedIndex` — and those two are byte-identical once doc comments
are stripped, because both are thin `Mutex<IndexState>` wrappers over the one
fold in `oqueue-core`. Running one fold through two wrappers is **one**
implementation's worth of assurance presented as two, and the conformance suite
in `oqueue-index` proves the contract once. The rule-3 test is still met, and
by the thing that always met it: doc 10 #12's engine — `SQLite`, `redb`,
`RocksDB`, `fjall`, `SlateDB` — is the foreseeable second implementation, which
is why the read side is on the trait at all. ~~`M3.11` is where `MemoryIndex` stops being the fake with another name,
because a quota and an eviction policy are what a broker's materialization has
and a downstream crate's test double must not.~~ ⚠️ **Corrected 2026-08-26
(`M3.11`): it is not, and the divergence moves to `M5` with the enforcement.**
A ceiling on this index's keying can only be met by evicting, and eviction
gives back range a rebuild cannot restore — replaying the log reproduces the
same count and sheds the same entries again. `roadmap.md`'s deferral table
carries the enforcement to `M5`, beside the coarse per-object keying that makes
it feasible. Until then the two remain one fold wrapped twice, exactly as
`M3.24` recorded.
Date: 2026-08-25
Requirements: FR-12, FR-13, NFR-2, NFR-3, NFR-30

## Context

`M3.5` put `MaterializedIndex` in `oqueue-core` with a **write** side and
almost no read side: `apply`, `applied_upto`, `end_offset`, `clear`. That was
enough to prove the cache property and nothing else. `M3.8` is the row that
adds the read side, and `M3.md` task 12 names one method —
`find_batches(topic, partition, start, max_bytes) → ordered [ObjectRef]` —
without saying where it lives or what it returns when the two tiers `M3.6`
built disagree about how much is known.

Three constraints bear on it:

- **FR-13 / NFR-30: a fetch resolves offset→object through the index and never
  lists.** Doc 12 measures LIST at 12–38× the price of a GET and semantically
  useless besides. So the read side has to answer "which objects hold
  `[start, …)`" without consulting object storage at all.
- **`M3.6`'s two tiers are asymmetric in what they know.** A `TailEntry` in the
  window carries its `ByteRange` inline, which is what makes a tail read a
  single GET. A demoted `ObjectRef` carries no range: the range comes from the
  object's own footer, at 1–3 GETs (doc 12 §6.3). The index therefore knows the
  byte size of a tail batch and does **not** know the byte size of a history
  batch.
- **`max_bytes` is a byte budget the caller got from a Kafka client**, and
  Kafka's own rule is that a fetch never returns empty merely because the first
  batch exceeds the budget.

⚠️ **The second constraint is the one with no obvious answer**, and it is why
this is an ADR rather than a signature. A budget cannot be honoured against a
size that is not known until after a read that the budget exists to bound.

## Decision

**1. `find_batches` joins the `MaterializedIndex` trait**, rather than living
on the concrete `IndexState` beside it.

The read side is the reason the seam exists. `oqueue-broker`'s Fetch path
(`M3.14`) holds the index behind this trait so doc 10 #12's engine choice can
be made later without touching it; a `find_batches` reachable only on a
concrete type would put the Fetch path on the concrete type and make that
choice a rewrite. `contracts.md` rule 3's test is met — two implementations
exist today and a third is the open question.

**2. It returns `IndexedBatch`, not a bare `ObjectRef`**, where

```rust
pub enum IndexedBatch {
    Inline(TailEntry),   // range known, one GET
    Footer(ObjectRef),   // range from the object's footer, 1-3 GETs
}
```

⚠️ This is a deliberate deviation from `M3.md` task 12's literal
`→ ordered [ObjectRef]`, and the requirement is unchanged by it. `M3.6` built
the tail tier precisely so that a tail read costs one GET; a return type that
erases the inline range hands the caller no way to spend it, and the caller
would resolve a footer for an object whose region the index was already
holding. The tier is information the index has and the reader needs, so it
crosses the seam.

**3. `max_bytes` bounds what the index can price; a batch count bounds the
rest.** Entries are returned in ascending offset order from the first whose
records reach `start`, and the page ends at whichever of these comes first:

- adding a batch with a **known** length would take the charged total past
  `max_bytes` — unless the page is still empty, in which case it is admitted
  anyway, or a partition whose next object exceeds the budget would return
  empty forever and the consumer would never advance;
- the page holds [`MAX_BATCHES_PER_PAGE`] batches.

A batch whose length is **unknown** charges nothing against `max_bytes` and one
slot against the count.

⚠️ **So `max_bytes` is a bound the index honours where it can, and the caller is
what actually enforces the budget.** That is not a weakening — it is where the
information is. Only the reader learns a history batch's true size, because
learning it *is* the footer read; an index that pretended otherwise would be
enforcing a budget against a number it invented. What `find_batches` returns is
the ordered list of objects a fetch may read, and the reader stops when its
budget fills.

⚠️ **Unknown is not the same as `Footer`.** A `Footer` entry never knows its
length. An `Inline` entry holding `ByteRange::Full` does not know it either —
`Full` names the whole object, whose size only the store knows — and `M3.13` is
the row that stops emitting `Full` for a bundled object at all. The rule is
written on `IndexedBatch::known_len()` rather than on the tier so the page stays
correct however the tiers change.

⚠️ **`MAX_BATCHES_PER_PAGE` is UNDERIVED**, like `TAIL_WINDOW_ENTRIES` beside
it. It exists so a cold read has *a* bound — NFR-30's "bounded GETs, zero
LIST" — and its value is a placeholder with the right shape. `M14` is where the
number is measured against a real workload rather than chosen.

**4. There is no `high_watermark` method, and that is `M3.md` task 13
satisfied rather than skipped.** The high watermark of a partition *is*
`end_offset`: nothing enters this index before its metadata record commits, so
"the end of the committed log" and "where the next record lands" are one
number, derived from the fold of every `CommittedSpan`'s count. Task 13 asks
that it be derived and that there be **no setter**; both hold structurally.
Adding a second method returning the same value would create two names for one
number and, with them, the first opportunity for the two to disagree — which is
precisely the class of bug the task is about. ⚠️ `M3.10`'s LSO is a genuinely
different number (it is never *ahead* of the high watermark) and does need its
own accessor when transactions exist to move the two apart.

## Alternatives considered

**`find_batches` on `IndexState` only, leaving the trait's method set alone.**
Rejected: it avoids this ADR by putting the Fetch path on a concrete type. The
seam would then describe only the write side, and doc 10 #12's engine choice —
the reason the seam exists — would become a rewrite of every reader. The cost
of a contract change is one commit; the cost of this is paid at `M3.14` and
again at `M6`.

**Return a bare `Vec<ObjectRef>`, as task 12 literally says.** Rejected: it
discards the inline `ByteRange` on exactly the entries where the index has one,
so every tail read would resolve a footer it did not need — 2–4 GETs where
`M3.6`'s window was built to buy one. The task's intent (an ordered list of the
objects a fetch must read) is served by a type that also says how much is known
about each.

**Give `ObjectRef` a byte-length field, so every entry has a known size and
`max_bytes` is exact.** Rejected on `M3.6`'s budget: `ObjectRef` is pinned at
40 bytes by a size test, the growth arithmetic in `M3.md` (~14 GB/day, ~97 GB
over a 7-day retention) is computed against that width, and a `u64` per entry
is a 20% increase in the largest data structure this system holds. The two-tier
split exists to *avoid* paying per-entry for what only the tail needs.

**Let one unknown-size entry end the page** — include it, then stop, so the
caller overshoots its budget by at most one object. Rejected, and this ADR
carried it until a test showed what it does: **every** history batch has an
unknown length, so a page that begins in history would be exactly one object,
every time. A consumer replaying a partition from the start would issue one
fetch per object for the whole of its history. The rule was written thinking of
a mostly-sized page with an occasional unknown in it, and the real distribution
is the opposite. Bounding the unknowns by count instead keeps the page useful
and still bounded, which is what NFR-30 actually asks for.

**Stop the page before the first unknown-size entry rather than after it.**
Rejected for the same reason and more sharply: a cold fetch would return
nothing at all, never making progress and turning a cold read into an infinite
poll.

**Charge an unknown-size entry a nominal estimate against the budget** (an
average object size, or the tail window's mean). Rejected: it is a number
nobody measured, `M3.md` already carries NFR-13 as **UNDERIVED**, and a budget
enforced against an invented size is a bound in appearance only. An honest
"one object of overshoot" beats a precise-looking figure with no derivation.

**Make `find_batches` async so it could resolve footers itself and honour
`max_bytes` exactly.** Rejected on **layering**: it would put object-storage
reads inside the index, which is what the seam exists to prevent — the reader
resolves footers, the index says which objects to read. ⚠️ **Not on
allocation**, which is the reason `ADR-0020`'s `M3.5` note gives for the seam
being synchronous at all and which does *not* survive this signature: a
`Result<Vec<IndexedBatch>>` heap-allocates the vector and clones an
`ObjectKey` — a `String` — per entry, so a full page is on the order of 65
allocations against the one boxed future async would have cost. The
synchronous read side is still right; a future row that wants it allocation-free
has to change what `find_batches` returns, not whether it is async.

## Consequences

**Makes easy:** a Fetch that is entirely in the tail window issues one GET per
object and no footer reads, because the ranges it needs are already in the
answer. The engine question (doc 10 #12) stays behind the seam, since the whole
read path is expressed in trait terms.

**Makes hard:** a caller cannot rely on `max_bytes` being an upper bound on
bytes actually fetched. ⚠️ **The overshoot is bounded by
`MAX_BATCHES_PER_PAGE` objects, not by one** — every history batch is
unpriceable, so a page that begins in history can be 64 entries none of which
were charged. Every caller has to be written knowing that, and `M3.14`'s Fetch
handler is the first: it learns each object's size as it reads and stops when
its own budget fills, which is where the real bound is.

**Forecloses:** an exact byte budget over history batches, unless `ObjectRef`
grows a size field (rejected above) or the footer is consulted (rejected
above). If `M14`'s measurements show the overshoot matters, the answer is a
coarser per-object index — doc 15 §7, which `M3.6`'s own doc comment already
names as the thing this index has not yet become — not a wider `ObjectRef`.
