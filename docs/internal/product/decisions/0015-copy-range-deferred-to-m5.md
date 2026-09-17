# 0015. Server-side `copy_range` is deferred to `M5`; `M1.18` is dissolved

Status: accepted; 2026-09-18 (`M5.7`): **discharged by `ADR-0039`**, which decides not to build `copy_range` at all. The deferral's premise held — the upstream gap is unchanged at the pinned 0.14.1 — and the reason for the decision is one this ADR could not have: a merge reorders regions and rewrites the footer, so a server-side range copy cannot express the operation whatever upstream exposes.
Date: 2026-08-17
Requirements: FR-31

## Context

`M1.18`'s row: "A method on the trait or an extension seam that copies a byte
range without a round trip through the caller... S3 `UploadPartCopy`, GCS
`compose`." `M5.md` task 7 names the actual consumer: "Server-side range copy
for whole-range moves — PUT-class cost, zero egress," used when compaction
consolidates several existing segment objects into fewer, larger ones without
downloading and re-uploading their bytes.

Investigated for `M1.18` and found to have **no buildable surface through
`object_store` for either backend**, not a partial gap like `M1.16`'s or a
testing-only gap like `M1.17`'s:

- **S3 `UploadPartCopy`.** `object_store`'s S3 client has internal support —
  `PutPartPayload::Copy(&Path)` (`aws/client.rs`), used by `copy_opts`'s
  `CopyMode::Create` arm when `S3CopyIfNotExists::Multipart` is configured —
  but three things rule it out. It is `pub(crate)`, unreachable from
  `oqueue-store`. Even internally, it always copies the **whole** source
  object as one part — the request builder (`aws/client.rs`'s `put_part`)
  never sets `x-amz-copy-source-range`, confirmed by its absence anywhere in
  the crate. And neither public multipart trait (`MultipartUpload::put_part`,
  `MultipartStore::put_part`) accepts anything but real payload bytes — there
  is no public parameter to say "this part is a copy," whole-object or
  otherwise.
- **GCS `compose`.** Not present anywhere in `object_store`'s GCS module —
  confirmed by an exhaustive case-insensitive search of `src/gcp/` and the
  whole crate turning up nothing but an unrelated use of the word "compose"
  in `lib.rs`'s own doc comment (about composing `ObjectStore` adapters, not
  the GCS API).

Both are known, tracked upstream gaps, not a misreading:
[`apache/arrow-rs-object-store#121`](https://github.com/apache/arrow-rs-object-store/issues/121)
("Support Object Composition," open since 2023-10-20, explicitly naming S3
`UploadPartCopy`, GCS `compose`, and Azure block-blob composition together as
the same missing capability) and
[`#563`](https://github.com/apache/arrow-rs-object-store/issues/563)
("Enable AWS client to copy objects >5GB in size," open, confirming even
basic multi-part copy is unsupported publicly).

## Decision

**`M1.18` is dissolved.** There is no partial version of "copy a byte range
server-side" this task could land — every path runs through code that is
either `pub(crate)` inside `object_store`, whole-object-only where this
project needs a range, or entirely absent (GCS). The only way to build the
row as written is to hand-roll signed HTTP requests for both `UploadPartCopy`
and `compose` — the exact SigV4/GCS-signing infrastructure ADR-0013 already
declined to build for a narrower problem (conditioning one completion call),
for a wider one (a second server-side operation, on two backends, with no
existing request-signing code in this crate to extend). Building it now,
before `M5` has a concrete caller or a measured cost delta against the
client-side fallback, would be exactly the kind of unearned infrastructure
`M1.16`'s equivalent finding (ADR-0013) already argued against.

**Deferred to `M5`**, task 7, which already names this capability and is the
first (and, per `M5.md`, only currently known) consumer. `M5` decides, with a
real caller and a real cost model in hand: hand-roll the signed requests (now
justified by an actual measured saving, not a projected one), track whether
`object_store` closes #121 by then and depend on it if so, or accept a
client-side copy for whatever fraction of compaction moves this turns out to
matter for — `M5.md`'s own task 7 already warns "does not apply to every
compaction type; do not budget zero egress for all," so a partial answer may
be enough.

## Alternatives considered

**Implement a client-side (read-then-write) `copy_range` now, so `M1.18`
ships *something*.** Rejected: the row's whole point, restated verbatim in
`M5.md`, is avoiding exactly that round trip for its cost property
("PUT-class cost, zero egress"). Shipping a method with the right name and
the wrong cost profile is worse than not shipping one — a caller reading the
signature has no way to tell it does not deliver what it promises, and
`M5`'s own compaction cost model would silently be built on a false premise.

**Hand-roll the signed requests now**, so the capability exists when `M5`
needs it. Rejected for the same reason ADR-0013 rejected it for the narrower
multipart-completion problem: no committed need yet, no existing signing code
in this crate to build on, and a correctness-and-security surface
(`security.md`'s territory once real signing code exists) this project should
not take on speculatively.

**Wait for `object_store` to close #121 before scheduling this at all.**
Not rejected outright, but not a decision this ADR can make: no committed
upstream timeline exists, and `M5` is far enough away that revisiting this
when it opens is strictly better information than guessing now.

## Consequences

**Easy.** `M1` carries no half-built, misleadingly-named capability. `M5`
inherits a clean decision with the actual investigation already done, rather
than a stale row assuming `object_store` provides something it does not —
the same trap `M1.16`/`M1.17` each found and corrected before shipping.

**Hard.** `M5`'s compaction cost model (doc 13 §6-ish territory, decision
#18 in `roadmap.md`'s "decisions that gate this plan" table) cannot assume
zero-egress server-side moves are available on day one; it must build its
plan around confirming this before relying on it, or design compaction to
degrade gracefully to client-side copies where it is not.
