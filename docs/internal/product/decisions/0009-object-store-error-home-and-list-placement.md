# 0009. Where `ObjectStore`'s errors live, and why `list()` is not on the trait

Status: accepted
Date: 2026-08-16
Requirements: FR-31, NFR-30

## Context

Two questions M0's boundary review found unanswered (`a4d96f6ba946`), both
blocking `M1.4`'s error taxonomy and `M1.3`'s trait shape:

**Whose enum holds `ObjectStore`'s errors.** `contracts.md` rule 17 and
`error-handling.md` rule 3 both say each crate defines its own error enum;
rule 8 says classification belongs to the seam that produces the error. M0
ended with both existing seams (`Clock`, `ObjectStore`) returning
`oqueue-core::Error`, and `oqueue-crypto` returning
`oqueue-core::Error::EncryptionDisabled` — a variant `oqueue-core` itself
cannot produce, meaning `oqueue-crypto` has no error enum of its own at all.
`crates/oqueue-core/src/error.rs`'s own doc comment names the tension
directly: `M1` brings six new classified variants
(`NotFound`/`PreconditionFailed`/`SlowDown`/`Throttled`/`Transient`/
`Permanent`), and writing `oqueue-store`'s backend first — without deciding —
would settle the question by default, in the crate `architecture.md` warns
must not grow past roughly a dozen traits' worth of surface (open question
#34).

**Whether `list()` belongs on `ObjectStore` at all.** Doc 12 §8's design
recommendation #1: *"No `list()` in the object-store trait. Follow KIP-1163:
`upload`, `fetch(key, ByteRange)`, `delete(keys)`. Put listing behind a
separate `MaintenanceStore` trait used only by the GC reconciler. This makes
'never LIST on the read path' a compile-time property, not a code-review
convention."* NFR-30 already names this as a requirement (no LIST on the read
path); the question is whether the seam's own shape enforces it or only a
runtime gate does.

## Decision

**Two separate decisions, and neither adds a trait method this milestone.**

### 1. `oqueue-core::Error` keeps the classified taxonomy; implementing crates get their own error type for what is specific to them

⚠️ **This reads as landing exactly where finding `a4d96f6ba946` warned against**,
and the divergence needs stating rather than passing silently: that finding's
own evidence says a taxonomy landing in `oqueue-core::Error` is "precisely the
outcome" the finding exists to prevent. This ADR makes that call anyway,
because the finding did not have to reconcile it against a constraint that only
becomes visible once every actual holder of the trait is named: `contracts.md`
rule 9 puts `ObjectStore`'s one fake beside the trait, **in `oqueue-core`**;
`M0.2`'s manifest rule gives `oqueue-core` **zero** workspace dependencies; and
ADR-0005 already commits `bin/oqueue`'s composition root to one
non-generic `Arc<dyn ObjectStore>` chosen **at startup**, meaning the fake and
every real backend must be interchangeable behind the exact same concrete type
at runtime, not selected at compile time. Put the three together: whatever
error type the trait returns must be nameable by `oqueue-core` (for the fake)
without `oqueue-core` depending on `oqueue-store` (impossible by the layering
rule) or the fake moving out of `oqueue-core` (forbidden by rule 9). The only
type everything can share without breaking one of those is one `oqueue-core`
defines itself. Rule 17 is written for crates that each own an independent
seam; `ObjectStore` is not that shape, because its highest-risk implementation
(the fake) is required to live beside the trait rather than beside a backend.

`architecture.md`'s crate table names `oqueue-core`'s charter as "types, IDs,
errors, and every trait seam" — the classified vocabulary a seam's callers
match on is part of the seam's own contract, and the seam lives in
`oqueue-core` (the star's center, per `AGENTS.md`'s layering rule: every crate
depends on it, it depends on nothing here). So `ObjectStore::get`/`put`/
`delete` keep returning `oqueue_core::Result<T>`, and `oqueue-core::Error`
gains the six variants `M1.4` names, exactly where its own doc comment already
anticipated them.

This does **not** waive rule 17 for `oqueue-store`. What it means concretely:
`oqueue-store` defines its **own** error type — call it
`oqueue_store::BackendError` — for whatever is specific to a backend
(`object_store::Error`'s variants, an HTTP status, a request id worth logging).
That type is `oqueue-store`'s own contract, built with `thiserror`, and it is
**not** returned across the trait boundary. Each backend maps its
`BackendError` into `oqueue_core::Error`'s classified variant at the point
where it implements `ObjectStore` — the classification `error-handling.md`
rule 8 requires happens in `oqueue-store`'s code, but the type that carries the
verdict is the seam's own, because that is what the trait signature already
committed to. A caller matching on `oqueue_core::Error::Throttled` never needs
to know or care which backend produced it; a maintainer debugging why gets the
detail from whatever `BackendError` variant is wrapped in, or logged, alongside
it.

`oqueue-crypto`'s pre-existing violation (returning
`oqueue-core::Error::EncryptionDisabled` with no enum of its own) is **not**
fixed by this ADR. It is a real instance of the same finding, but `oqueue-store`
is what `M1` is building; `oqueue-crypto`'s fix is scoped to whichever
milestone next touches that crate's error handling, not smuggled in here.

### 2. `list()` is not on `ObjectStore`. No `MaintenanceStore` trait is added this milestone.

Doc 12 §8's recommendation is adopted as stated: `ObjectStore` carries `get`,
`put`, `delete` and nothing that enumerates a prefix. "Never LIST on the read
path" (NFR-30) becomes something the type system enforces — nothing holding
only an `Arc<dyn ObjectStore>` can issue a LIST, full stop, rather than a
convention a reviewer has to remember to check.

A `MaintenanceStore` trait (or equivalent) for the GC reconciler's future use
is **named here and not built.** Nothing in this workspace needs it yet — the
reconciler is M5/M6 work — and adding an unused trait now would be exactly the
kind of speculative surface `code-structure.md` and `architecture.md`'s
size warning argue against. This ADR exists so that whichever milestone first
needs listing reaches for a `MaintenanceStore` seam rather than adding `list()`
to `ObjectStore` out of local convenience, which would silently reopen the
property this decision buys.

> **Amended by `M7.3a` (`ADR-0049`), 2026-09-20.** The seam named above is now
> built, as `MaintenanceStore` in `oqueue-core`, with one method:
> `list(prefix, after, limit)`. Its first holder is the topic catalog, which
> must page topic names in order — not the GC reconciler this section
> anticipated. `ObjectStore` still carries no `list`, and the metadata log and
> the read path are never handed a `MaintenanceStore`, so NFR-30 remains a
> property of the seam's shape.

## Alternatives considered

**An associated error type on the trait** (`type Error: std::error::Error`,
parameterizing `Result<T, Self::Error>`). Rejected, but not for the reason it
first looks like: an associated type fixed at the object site
(`Arc<dyn ObjectStore<Error = E>>`) *is* `dyn`-compatible on stable Rust — the
naive "breaks `dyn`-compatibility" objection does not hold up, and does not
appear in the final text above for that reason. The real problem is what `E`
would have to be: `bin/oqueue`'s composition root needs **one** concrete `E`
that the fake (in `oqueue-core`) and every real backend (in `oqueue-store`)
all implement, chosen at startup rather than at compile time (ADR-0005), and
the same three-way constraint from the section above applies — `E` must be
nameable from `oqueue-core` without `oqueue-core` depending on `oqueue-store`.
An associated type does not relax that constraint at all; it only adds a
type parameter to every call site for a freedom (a per-implementor error type)
the runtime-backend-selection requirement does not actually allow anyone to
use.

**`list()` on `ObjectStore`, gated by a runtime flag or a review convention.**
Rejected — this is the status quo doc 12 §8 is explicitly arguing against, and
`NFR-30`'s own verification method ("gate asserting the read path issues zero
LIST calls") is strictly weaker evidence than "the read path cannot call it,"
which is what removing the method buys for free.

**Building `MaintenanceStore` now, even with no caller.** Rejected as
premature: an unused trait with an unused fake is surface `contracts.md` rule
9 forces into existence (a fake beside every trait) for something nothing
tests yet, and `architecture.md`'s size warning applies to unused traits as
much as used ones. The decision costs nothing kept as a decision; the trait
costs real surface built ahead of its user.

**A single `oqueue-core::Error` shared verbatim by every crate, including
`oqueue-store`'s internal detail** (i.e., no `BackendError` at all, backends
construct `oqueue_core::Error` variants directly). Rejected: it would mean
`oqueue-store` genuinely has no error enum of its own, which is the exact shape
of the `oqueue-crypto` violation this ADR is explicitly not repeating. A
backend-specific detail (which HTTP status, which `object_store::Error`
variant) has nowhere honest to live without it, and a future second backend
implementation would have no local type to build its own mapping logic against.

## Consequences

**Easy.** `M1.4`'s error taxonomy lands in the file that already anticipated
it, with no crate boundary to invent. `M1.3`'s trait shape is exactly `get`/
`put`/`delete` — smaller than if `list()` had stayed, and nothing in `M1`'s
task list depends on listing existing. `NFR-30`'s verification method
(`requirements.md`: "gate asserting the read path issues zero LIST calls") is
not yet built — nothing in the tree calls `list()` today, so there is nothing
to gate — but whenever it is, this decision means it is defense in depth
rather than the only thing standing between the read path and a LIST call.

**Hard.** Every `oqueue-store` backend (`M1.15`'s S3, `M1.17`'s GCS) needs its
own mapping code from `BackendError` (or directly from `object_store::Error`)
into `oqueue_core::Error`'s six variants, rather than being able to bubble a
foreign error type straight through — `error-handling.md` rule 5's "a crate's
error enum classifies, it doesn't just wrap" applies at this boundary
specifically. `oqueue-core::Error` grows by six variants this milestone, moving
it closer to `architecture.md`'s open question #34 about splitting
`core-types` from `core-traits`; not close enough to force that split now, but
the next seam to bring its own taxonomy should re-check the count against it.
