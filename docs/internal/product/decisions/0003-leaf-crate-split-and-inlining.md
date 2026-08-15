# 0003. The four leaf crates, and the inlining policy across them

Status: accepted
Date: 2026-08-15
Requirements: NFR-2, NFR-56

## Context

`M0.8` creates ten crates, four of which sit on the hot path and are the ones in
question: `oqueue-buf` (buffer primitives, refcounted slices), `oqueue-codec`
(the Kafka wire protocol and RecordBatch encode/decode), `oqueue-checksum`
(CRC-32C) and `oqueue-index` (offset→object lookup). A tail read that hits cache
touches all four.

**A crate boundary is an optimization barrier — the corpus says by default, and
that turns out to be the part worth checking.** Doc 18 §0.4 states that a
non-generic `pub fn` without `#[inline]` has no MIR in the rlib and so cannot be
inlined by a caller in another crate, and calls the consequence "ruinous" for a
byte-level codec split across crates. That claim is why four crates is a
question rather than a filing decision — and ⚠️ **on 1.97.1 it holds only past
a size**: below ~100 MIR cost units rustc inlines across the boundary with no
annotation at all. See the Decision section, written from measurement rather
than from the quote.

Two requirements pull against each other:

- **NFR-2** — tail-read p99 ≤ 10 ms with the data in cache. Verified by a
  benchmark asserting *zero object-storage round trips*, so the budget is
  entirely CPU, and a lost inline in a decode loop is paid on every batch.
- **NFR-56** — the pre-commit suite's wall clock.

⚠️ **The NFR-56 direction is the one people get backwards.** Merging the four
would *raise* the pre-commit wall clock, not lower it: it widens the unit that
must rebuild when any of the four changes, and it removes the sideways
parallelism four independent units give. ⚠️ And it does **not** change DAG
depth — doc 19 §1.1–1.2, four siblings in a star are at depth 2 grouped or
split, and depth is what sets the floor on build time.

⚠️ **What the corpus says, precisely.** Doc 18 §3.1's second consequence says
"**Keep the true hot path in one crate** — record-batch encode/decode, CRC,
varint, index lookup, buffer management together", naming exactly these four.
But doc 18 §3.7.5 then says many small crates "pulls **with** §3.1, not against
it" and carries the same carve-out doc 19 §1.2 does — keeping the hot path in
one crate is about the hot path, not the workspace shape — and doc 19 §1.3's
layout table lists all four as separate crates. ⚠️ So the corpus mostly supports
the split; only §3.1's consequence 2, read literally, opposes it.

## Decision

**The four crates stay split**, and:

⚠️ **Every `pub fn` on a hot path that is called from another crate carries
`#[inline]`** — the paths `performance.md` rule 18 names, not every `pub fn`.

Generic functions need no annotation: they are monomorphized in the caller's
crate. ⚠️ Where a public function is generic over something broad — a buffer
type, a store backend — it delegates to a private non-generic inner function, so
only a thin adapter is duplicated per instantiation (doc 18 §3.1).

### ⚠️ What the annotation actually buys

Measured on 1.97.1, building rlibs with `-C opt-level=N -C embed-bitcode=no`:

- **rustc inlines across crates by itself below a threshold.** A non-generic
  `pub fn` under `-Zcross-crate-inline-threshold` — **default 100 MIR cost
  units** — has its MIR encoded and is inlined with no annotation and no LTO.
  The edge is sharp and linear in the flag. Doc 18 §0.4's "even the most trivial
  of functions can't be inlined" predates this mechanism.
- **A varint decode is below it.** A `read_varint` returning `Option<u32>`
  inlines unannotated. ⚠️ So the codec's smallest helpers — the case doc 18
  calls "ruinous" — are the case rustc already handles.
- **At `opt-level = 0`, `#[inline]` does nothing** across a boundary; only
  `#[inline(always)]` does. Workspace members build at `opt-level = 0` in `dev`
  and `test`, because `[profile.dev.package."*"] opt-level = 2` reaches
  dependencies, not members.

⚠️ **And LTO does not subsume the annotation** — the part that is easy to get
backwards. LTO controls whether the callee's IR is *available*; `#[inline]`
additionally sets LLVM's `inlinehint`, raising the inline cost threshold from
225 to 325, and flips rustc from `GloballyShared` to `LocalCopy` instantiation.
Neither effect is removed by LTO. Against a profile matching this repo's
`release` exactly:

⚠️ Shape, so the table is reproducible: a non-generic `pub fn f(x: u64) -> u64`
whose body is N straight-line wrapping-arithmetic statements, called from
**three** sites in another crate with runtime arguments. Call-site count
matters — with a single site, LLVM's last-call-to-static bonus inlines the
annotated form far past this band.

| body size | no attribute | `#[inline]` |
|---|---|---|
| ≤ 26 statements | inlined | inlined |
| **28-32 statements** | **not inlined** | **inlined** |
| ≥ 36 statements | not inlined | not inlined |

The band reproduces under a `dist`-shaped profile with fat LTO and
`codegen-units = 1`. A realistic codec entry point — a `read_record_header`
delegating to two private varint readers — differs too: unannotated it is
reached by an indirect call through the GOT, annotated it is a direct call.

So the policy has a measured justification, not a hedge: there is a real size
band, straddling the size of a codec entry point, where the annotation is the
whole difference — in every profile that ships. ⚠️ It is a *band*, not a floor:
below it rustc inlines anyway and above it nothing does, which is why the
obligation is scoped to `performance.md` rule 18's hot paths rather than applied
everywhere.

⚠️ **The obligation is real and unguarded.** Nothing reports a missing
`#[inline]`, and a regression in that band surfaces as a benchmark number rather
than a failure. `M2` onward is where it gets caught.

## Alternatives considered

**Merge the four into one crate.** Rejected. What it would buy is
within-crate inlining unconditionally — and both the mechanisms that would
otherwise be needed are already present: thin LTO where it ships, and rustc's
own automatic cross-crate inlining for small functions. Against that it costs the two things doc 19 §1.1 says decide build time: the rebuild unit
widens, so touching CRC-32C rebuilds the codec, and four units that compiled in
parallel become one that cannot. It also collapses four units that meet doc 19
§1.2's second and third split criteria — independently testable, and each
removing work from the critical path. ⚠️ **Not the first criterion**: a contract
boundary here means a `pub trait` in `oqueue-core` (`contracts.md` rules 1-2),
and none of the four holds one, so that criterion argues for neither side.

⚠️ **And it means reading doc 18 §3.1's second consequence narrowly**, which
§3.7.5 and doc 19 §1.2 both license. The measurement is what makes that reading
safe: below ~100 MIR cost units rustc inlines across the boundary unannotated,
and in the band above it `#[inline]` restores the inline under every profile
that ships. ⚠️ `M2`'s benchmarks can falsify this, and are the reason to
re-open it.

**Merge only `buf` and `codec`.** The strongest version of the merge case, since
those two share the most traffic per batch. Rejected on the same grounds. ⚠️ It is *not*
rejected on `unsafe` blast radius, which was the tempting extra reason and does
not distinguish the options: `check-unsafe.sh`'s `ALLOWED_CRATES` already
contains `oqueue-codec` as well as `oqueue-buf`, so a merged crate is permitted
exactly the same `unsafe` and NFR-53 is unaffected either way.

**Split further** — a crate per protocol version, or per record-batch codec.
Rejected: more boundaries is more `#[inline]` obligations for no contract that
anyone programs against, and the split criteria in doc 19 §1.2 are met by none
of them.

**Rely on `#[inline]` alone and drop thin LTO from `release`.** Rejected, and
recorded because it is the tempting simplification once the annotations exist.
`#[inline]` is a convention with no gate: an un-annotated `pub fn` added later
compiles, passes every check, and costs a call in the decode loop with nothing
reporting it. LTO is the mechanism that does not depend on anyone remembering.
The two are belt and braces on purpose.

## Consequences

**Easy.** Four crates that can be tested, fuzzed and benchmarked independently,
with `oqueue-checksum`'s known-answer tests and `oqueue-buf`'s `unsafe` isolated
where `check-unsafe.sh` can see them. Build parallelism is kept. Release codegen
is unaffected by whether anyone remembered an annotation.

**Hard.** A convention that is invisible when forgotten: nothing reports a
missing `#[inline]`, and the code still compiles and passes every gate. What it
costs is a lost inline in the 28-32 statement band, on a hot path, in every
shipping profile — visible only to a benchmark.

⚠️ **What checks this, and when.** Nothing today, and that is not a gap this
task can close: there is no hot path to measure until `M2` writes the codec.
`performance.md` rule 18 makes each hot path carry a benchmark added with the
code, and `check-hot-path-bench.sh` already enforces that a benchmark's hot-path
marker names a real row in that table — deliberately narrowed, per its own
header, to the rows whose code exists. So the runtime half of this decision is
checked from `M2` onward, by benchmarks, and `M14` is where the numbers are
argued.

⚠️ **M0 decides NFR-2's exposure here and measures no broker code.** What was measured is compiler behaviour, which is what the decision turned on; NFR-2 itself has nothing to measure until `M2`. That is the point
of deciding now: the alternative is discovering at `M2` that the split costs
something and re-litigating it with code already written against it. This ADR is
falsifiable — a `M2` benchmark showing the boundary crossings dominate would
overturn it, and the overturning would be ADR-0003 superseded rather than a
quiet merge.

**Foreclosed.** Nothing structural. Merging later is mechanical; the annotations
become redundant rather than wrong.
