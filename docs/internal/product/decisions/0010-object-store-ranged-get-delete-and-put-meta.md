# 0010. `ObjectStore` gains ranged `get`, batch `delete`, and a returned `ObjectMeta`

Status: accepted; 2026-08-23: `M2.4` made the range contract hold against
the real backends too — a transient-classified ranged failure is
disambiguated by one `HEAD` on the error path, closing `M1.54`'s
fake-vs-real divergence; the contract itself is unchanged
Date: 2026-08-16
Requirements: FR-30, FR-31

## Context

ADR-0005 shaped `ObjectStore` with exactly `get`/`put`, deliberately minimal,
and named three things it deferred rather than rejected: a conditional-write
method, ranged reads, and — implicitly, since the trait had no batch
operation at all — deletion. Its own words: *"Adding it is a contract change
under `contracts.md` rule 12 and will touch this trait, its fake and every
implementation in one commit."* `M1.3` is that commit, for two of the three:
ranged reads and deletion. The third — a conditional-write parameter on
`put` — stays deferred to `M1.5`/`M1.6`, once `M1.5`'s `Precondition` enum
exists to shape it; adding a stub parameter now would be designing that enum
twice.

`milestones/M1.md`'s plan item 1 names the shape directly: `put`,
`get(ByteRange)`, `delete(set)`. Item 2 names the types: `ObjectKey` (already
built), `ByteRange`, `ObjectMeta`, and an opaque `PreconditionToken`.

## Decision

Three changes to `ObjectStore`, landed together because `contracts.md` rule
12 requires the trait, every implementor, and the ADR in one commit:

**`get` takes a `ByteRange`.** `ByteRange::Full` preserves today's
whole-object behaviour; `ByteRange::Bounded { offset, length }` (built only
through `ByteRange::bounded`, which rejects `length == 0` at construction —
the "unconstructible around" pattern every identifier in this crate already
uses) reads a slice. A range that does not fit inside the object's actual
size is [`Error::ByteRangeOutOfBounds`] — a **distinct** variant from
`ObjectNotFound`, because the object exists and the range does not fit,
which is not the same failure a caller should treat the same way (retrying
`ObjectNotFound` waits for the object to appear; retrying an out-of-bounds
range never helps).

**`delete` takes a slice of keys**, and is **idempotent per key**: deleting a
key with nothing stored under it succeeds. This matches S3's `DeleteObjects`
and GCS's batch delete, both of which treat "already absent" as success
rather than an error a caller has to filter out of a batch result.

**`put` returns `ObjectMeta`** (`{ size, precondition_token }`) instead of
`()`. `PreconditionToken` is opaque — not comparable across backends, and
explicitly **not a content hash**, because a multipart `ETag` is not a hash
of the assembled object and SSE-KMS can change an `ETag` without changing a
content byte. The fake backs it with a **per-key generation counter that
survives an overwrite** rather than resetting, because a real backend's
generation (GCS) or version chain (S3) never resets while a key exists
either — a fake that reset it would let a stale token from before an
overwrite compare equal to a fresh one, silently hiding exactly the race
`M1`'s conformance suite exists to catch. `put` itself stays unconditional in
this commit; the token this returns is what a **future** conditional write
(`M1.5`/`M1.6`) will present back.

## Alternatives considered

**Adding the `Precondition` parameter to `put` in this same commit**, so the
trait only changes once. Rejected: `M1.5` has not chosen `Precondition`'s
shape yet, and guessing it here risks a second breaking change to guess
again, which is a worse trade than one more `contracts.md` rule-12 commit
later. `ADR-0005` already anticipated exactly this split.

**A `head`/`stat` method returning `ObjectMeta` separately from `get` and
`put`.** Rejected as unnecessary scope: nothing on `M1`'s task list needs
metadata without either reading or writing the object, and an unused method
is exactly the premature surface ADR-0009 argues against for
`MaintenanceStore`. `put` returning `ObjectMeta` gives every caller that
needs it a value already in hand, for free.

**`get` returning `(Vec<u8>, ObjectMeta)`.** Rejected for this commit, same
reasoning as the `head` method: nothing yet needs a `get`'s metadata, and
`ByteRange` alone is what the plan's item 1 asks for. Revisit if a future
task needs to read-then-conditionally-overwrite without a preceding `put` of
its own.

**Silently clamping an out-of-bounds range to the object's actual size**
(as some HTTP range implementations do for a suffix range) rather than
erroring. Rejected: a caller asking for `[offset, offset+length)` and
silently getting fewer bytes back than it asked for is a shape of bug that
is invisible until something downstream miscounts, and doc 04's documented
S3/GCS semantics are the standard this project builds to (`milestones/M1.md`'s
completion condition) — a range past the object's end is a client error on
both, not a request to be lenient with.

## Consequences

**Easy.** Every existing call site in this crate's own tests needed
`ByteRange::Full` threading through, which is mechanical; nothing outside
`oqueue-core` implements or calls `ObjectStore` yet, so this is the cheapest
this contract change will ever be to make.

**Hard.** `oqueue-store`'s S3 (`M1.15`) and GCS (`M1.17`) backends both need
to map their own range-request and batch-delete semantics onto this shape,
and `M1.15`'s conditional-`PUT` case will need the `ObjectMeta` this ADR
adds before `M1.5`'s `Precondition` exists to consume it — the token is
produced starting now, consumed starting later.
