# 0043. The coordinator's index is bounded by the sweep interval, not by the tail

Status: accepted
Date: 2026-09-18
Requirements: NFR-11 (per-node metadata cost proportional to partitions active
on that node), FR-34 (compaction), FR-33 (retention)

## Context

`M5.10` inherited `M3.11`'s deferral and, with it, a premise: that the tail —
`TAIL_WINDOW_ENTRIES × partitions`, which `index_state.rs` has named since
`M3.6` — is what no keying fixes, and that a quota therefore needs a mechanism
to bound it. The row says so in those words, and asks this task to decide the
mechanism rather than assume one.

⚠️ **The premise is half wrong, and which half depends on a number nobody had
chosen.** Priced at doc 14 §3's working set, what holds the coordinator's index
is how long a reference waits before a manifest absorbs it — the sweep
interval — and at 30 minutes, the figure `ADR-0042` prices against, that term
is forty-four times the tail and two orders of magnitude larger than any node
can hold. ⚠️ **But the two tiers cross at forty seconds**, and two of the three intervals
this ADR goes on to tabulate sit below that — so which tier is larger is a
property of the interval chosen rather than a fact about the tiers. The row is
right that the tail is real and that no keying fixes it; what it is wrong about
is that the tail is therefore *the* thing to build a mechanism for, since at
the interval `ADR-0042` assumed the other tier is forty-four times larger. Both
halves are true at different intervals, and the first round of review is what
separated them.

This is the third time a quantity in this area turned out to be unpriced.
`ADR-0041` refused to choose a shape because three derivations had each missed
one; `M5.64` found every figure derived from a manifest entry's width wrong
three times over, including the GET count nobody noticed was derived at all.
The rule that came out of those is the rule here: derive it, from a measurement,
and say which measurement.

### What an entry costs, measured

`core::mem::size_of` on the staged types, plus the heap the object key holds —
`bundles/{writer}/{sequence:020}`, 51–54 B, quoted at its wide end because
these figures bound a memory budget:

| Thing | Inline | Heap | Total |
|---|---|---|---|
| `ObjectRef` (a history entry) | 40 B | 54 B | **94 B** |
| `TailEntry` (a tail entry) | 64 B | 54 B | **118 B** |
| A manifest reference `(ObjectKey, Offset)` | 32 B | 54 B | **86 B** |

⚠️ **Inline is not the cost.** `index_state.rs`'s "~40 bytes per entry" is
`size_of::<ObjectRef>()` and omits the `String`'s allocation, which is larger
than the struct. Every figure below uses the totals.

### The three tiers, at doc 14 §3's working set

1,000,000 active partitions, 4M `(object, partition)` pairs per second, so
4 spans per partition per second.

| Tier | Entries | Bytes | Bounded by |
|---|---|---|---|
| Tail | 128 × 1M = **128M** | **15 GB** | `TAIL_WINDOW_ENTRIES`, a constant |
| History, un-absorbed | 4M/s × T | **376 MB/s × T** | the sweep interval T |
| Manifest references | 1 × 1M = **1M** | **86 MB** | partitions, by construction |

At `ADR-0042`'s own 30-minute sweep, the middle row is 4M/s × 1800 s = **7.2
billion entries, 677 GB** — forty-four times the tail, and not a number any
node holds.

⚠️ **That ratio is a statement about 1800 seconds, not about the tiers.** The
tail is a steady state and un-absorbed history is an accumulation, so comparing
them is only meaningful at a stated interval. They are equal at

    15.1 GB ÷ 376 MB/s = **40 s**

and the two smaller intervals the table below recommends sit under it while the
largest does not — at 85 s history is 32 GB against 15 GB of tail, and at 21 s
it is 8 GB against the same 15. Which tier is larger is a property of the
interval chosen, which is the whole point. At the 8 GB row the
node holds 8 GB of history against a permanent 15 GB of tail — so a quota that
bounded history alone would let that node reach 23 GB without firing, which is
decision 1's own "quiet for the right reason" failure landing on the other
tier. `the_two_tiers_cross_at_forty_seconds` derives it.

⚠️ **`ADR-0042`'s "40 B against 97 TB" is two stale numbers, not one true
claim.** The 40 B is the inline width this section refutes, and the 97 TB is
that ADR's *pre-manifest* status quo rather than anything a manifest leaves
behind. What neither figure prices is the state *between* publications — every
reference the fold has taken and no manifest has yet absorbed — which is this
ADR's subject.

### What the sweep interval has to be

The steady-state index is growth rate × time-to-absorption. History grows at
4M/s × 94 B = **376 MB/s**, so a budget B implies

    T ≤ B ÷ 376 MB/s

| Budget for un-absorbed history | Sweep interval it implies |
|---|---|
| 1 GB | 2.7 s |
| 8 GB | 21 s |
| 32 GB | 85 s |
| 677 GB | 1800 s (`ADR-0042`'s figure) |

⚠️ **Two orders of magnitude from where the plan had it.** `M5.64` found the
same interval making `ADR-0042`'s read-cost column exceeded — five chained
manifests where doc 14 §3 asks for one to three GETs — and named `M5.16` as
the lever. It is the same lever, and this is the second column it binds.

## Decision

**The coordinator's index is bounded by how often compaction publishes, and
that is the number `M5.16` must choose against a memory budget rather than
against a read-amplification threshold alone.** Concretely:

1. ⚠️ **The quota's subject is the total, and the accounting is per tier.**
   Neither tier can be the subject on its own: above a forty-second sweep
   history dominates, below it the tail does, and a ceiling on one lets the
   other run. What the tiers need separately is *attribution* — they have
   different widths and different growth laws, so a number that moved says
   nothing about which lever to pull unless it says which tier moved. `M5.71`
   builds that accounting; the ceiling is over the sum.
2. **NFR-11 is a scaling property, not a byte ceiling.** "Proportional to
   partitions active on that node, never to total cluster partitions" is met by
   every tier above *provided a node folds only its own partitions* — and in
   this architecture the coordinator folds the whole log, so it holds every
   partition in the cluster. ⚠️ **That is `M7`'s, and saying so is the point**:
   NFR-11 cannot be satisfied by anything in `M5`, because the property it
   names is about which partitions a node is responsible for, and nothing in
   `M5` divides them. What `M5` can do is make each tier's cost proportional to
   the partitions it *does* hold, which is what `ADR-0042` did for history and
   what item 1 above measures.
3. **No eviction, and the reason `M3.11` gave still stands.** Evicting index
   entries gives back range a rebuild cannot restore — replaying the log
   reproduces the same count and sheds the same entries again, so the degraded
   mode's own recovery is a loop that cannot converge. The degraded mode
   refuses new growth and names the partition. `M5.72` is that mode.
4. ⚠️ **The tail's levers are named and none is chosen here.** The row proposed
   demoting an idle partition's tail into the chain. Priced, that buys
   118 B → 94 B per entry — **3.0 GB of 15.1 GB**, which is three times the
   1 GB history budget in the table above and is not the rounding error a
   first draft of this ADR called it. It is also not free: it needs a notion of
   idleness the fold does not have. The other lever is node-scoping, which is
   `M7`'s and is the only one that changes the multiplier rather than the
   constant. ⚠️ **Shrinking `TAIL_WINDOW_ENTRIES` is not among them** — see
   Alternatives below, where it is refused rather than deferred, because it
   moves a threshold in its weakening direction. ⚠️ **Choosing between the two needs
   `M5.71`'s accounting first**, and deciding it here without that is the shape
   `ADR-0041` refused three times over.

## Alternatives considered

- **Shrink `TAIL_WINDOW_ENTRIES`.** Halving it saves 7.5 GB and doubles the
  fetches that must resolve a footer, which is a threshold moved in its
  weakening direction for the read column (`AGENTS.md` non-negotiable 2 names
  the direction). It also does nothing about the 677 GB.
- **A disk-backed index** (doc 10 #12's engine choice). It changes what "holds"
  means rather than what the index costs, and the arithmetic above is what
  would size it. Still open, still `M6`'s.
- **Publish a manifest per commit.** Bounds history at zero and costs one
  conditional write per partition per commit — doc 14 §7 friction 4 gives ~15
  conditional writes/s per key on S3, and this asks for 4M/s. Refused on the
  write-rate column, which is the column `ADR-0041`'s third shape died on.

## Consequences

- `M5.16`'s sweep interval acquires a second constraint, in bytes, and a table
  above to pick from. It was a read-amplification decision; it is now also a
  memory one.
- ⚠️ **`ADR-0042`'s state column is superseded, not merely extended**, and
  every row of it moves. It priced an `ObjectRef` at 40 B and a `TailEntry` at
  56 B — inline widths, omitting the object key's own allocation — so its
  tail 7.2 GB is 15 GB here, its per-`(object, partition)` 97 TB is 226 TB, its
  per-object 9.7 GB is 22.8 GB, and its 40 MB of manifest references is 86 MB.
  A node sized from that table is provisioned under half what it needs.
  `M5.73` corrects the table there; this ADR is where the widths are derived.
- `M5.71` (per-tier accounting), `M5.72` (the quota, its alarm and its degraded
  mode) and `M5.73` (`ADR-0042`'s stale state table) carry the rest. `M5.10` ships the pricing and this
  decision, which is the shape `M5.9`/`ADR-0041` set for exactly this
  situation: a number nobody had, priced before a mechanism is built on it.
- NFR-11's *verification* stays `M7`'s, as `roadmap.md`'s coverage table always
  had it — and item 2 above records that its enforcement is `M7`'s too, which
  the roadmap's deferral table did not say.
