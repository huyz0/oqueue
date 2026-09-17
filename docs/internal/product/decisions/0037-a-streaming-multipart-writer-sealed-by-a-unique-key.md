# 0037. A streaming multipart writer on the seam, sealed by a unique key rather than a precondition

Status: accepted
Date: 2026-09-17
Requirements: FR-31, FR-34, NFR-20

## Context

`ADR-0013` (`M1.16`) deferred two things and `ADR-0026` (`M3.12`) re-targeted
them to this milestone: a **streaming** multipart writer — doc 04 §5's own
motivating scenario, "accumulate a segment, flush parts as they fill, seal
without ever knowing the final size up front" — and a **conditional seal** on
that writer's completion. `M1` deferred both for want of a caller. `M3` deferred
them again because the caller it was promised did not need either: `M3.13`'s
flush knows its size when the write starts, and `ADR-0020` point 4 forbids a
conditional write on the offset stream.

`M5.4`'s merge executor is the first genuine caller of the first half. It reads
one input at a time, so the input side is bounded, and accumulates the whole
merged payload in a `BundleBuilder` before a single `put` — so peak memory is
one input plus the whole output, and the only thing bounding the output is
`COMPACTION_PLAN_RECORDS_BUDGET`, which `M5.4` lowered to one compacted object's
worth precisely because the writer does not exist.

The second half has not moved upstream. Re-checked against the pinned
`object_store` 0.14.1's vendored source, as `ADR-0013` asked each milestone to
do rather than reading the issue tracker: `CompleteMultipartMode` is still
`pub(crate)`, both public completion paths still pass `Overwrite` literally, and
`PutMultipartOptions` still carries no `PutMode`.
[`apache/arrow-rs-object-store#289`](https://github.com/apache/arrow-rs-object-store/issues/289)
is still the tracking issue. Both live mechanisms — hand-rolled SigV4 signing,
and intercepting the request through a custom `HttpConnector` — are the same two
`ADR-0013` rejected, for reasons that have not changed.

## Decision

**1. The `ObjectStore` seam gains a streaming writer.** A new
`open_multipart(key) -> Box<dyn MultipartWriter>`, and a `MultipartWriter` trait
with `write_part(bytes)` and `finish()`. A caller streams parts as they fill and
seals without declaring a size. This is a contract change — the ADR,
the trait, every fake and every implementation in one commit
(non-negotiable 6, `contracts.md` rule 12).

**2. The seal is unconditional, and a unique key is what makes that safe.** The
hazard a conditional seal guards against is one writer overwriting bytes an
index entry already names. A key that is never reused removes the hazard at its
source rather than defending against it at the last request — and non-reusable
object IDs are not a new invention here: they are the first of the GC safety
inequality's three enablers (`M5.md` task 19, doc 12 §4.6), which this milestone
is building anyway. A compaction output is named per attempt; a retry writes a
new key; `M5.13`'s atomic commit decides which key the index points at, and
`M5.23`'s orphan reconciliation reclaims the losers.

**3. `put` keeps its precondition and its behaviour.** Nothing about the
whole-payload path changes, including `ADR-0013` point 1's refusal of a
conditioned payload above `max_single_put`. A caller that knows its size and
wants `IfAbsent` still has it; what the streaming path cannot offer, it does not
pretend to.

## Alternatives considered

**Hand-roll a SigV4-signed `CompleteMultipartUpload`.** Rejected, as in
`ADR-0013`, and the caller's arrival does not change the argument: credential
resolution, signing and retry would be reimplemented for one call, with a
security review burden `security.md` would have to hold, to buy a property
decision 2 gets for the price of a key suffix.

**Intercept the request through a custom `HttpConnector`/`HttpService`.**
Rejected, as in `ADR-0013`. It rests on an unverified assumption about SigV4's
signed header set, matches requests by URL-shape sniffing rather than any
documented contract, and sits under a library to defeat a choice its maintainers
made deliberately. ⚠️ **It is also the mechanism `M10.2` already uses for the
simulated transport**, which is an argument against reaching for it here rather
than for: one interception point that a test controls is reviewable, and a
second that production depends on is a place where the two can disagree.

**Wait for upstream #289.** Still genuinely worth revisiting and still not
decidable: no committed timeline, and this milestone cannot block on it. ⚠️
**Decision 2 changes what closing it would buy**: with unique keys, a
conditional seal becomes a defence in depth rather than a requirement, so a
future pickup is an improvement rather than a repair.

**Condition the seal by writing to a temporary key and copying.** Rejected: a
server-side copy of a compacted object is the egress `M5.7` is about, paid on
every compaction to buy what a unique key already gives.

**Keep accumulating in memory and raise the budget instead.** Rejected, and it
is the status quo this ADR exists to end: peak memory would scale with the
budget, the budget is what bounds a round's work, and the two would have to be
traded against each other for ever. `M5.4`'s own lowered budget is the receipt.

## Consequences

**Easy.** A merge's output side stops scaling with the plan's size, so
`COMPACTION_PLAN_RECORDS_BUDGET` can rise on the argument that a round should
do more work rather than on how much memory a writer holds. Any later caller
that assembles an object incrementally — `M6`'s snapshots, `M8`'s
encrypted regions — has the seam it needs.

**Hard.** Two traits on the seam instead of one, and a `Box<dyn MultipartWriter>`
is an allocation per output object; that is per compaction round, not per
record, and `performance.md`'s rule against optimizing ahead of a benchmark
applies. A streaming writer also cannot be retried as a unit: a part that fails
mid-object leaves an incomplete upload, and cleaning those up is a bucket
lifecycle rule rather than this seam's promise — the same trade `ADR-0013`
point 1 already records for the non-streaming path.

**Foreclosed.** A caller cannot condition a streamed object's creation, and any
future requirement that one must be conditioned reopens this record rather than
being satisfied by a workaround at the call site.
