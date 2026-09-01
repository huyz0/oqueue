# 0030. A per-key delete refusal stays `Transient`, and the question that matters moves to M5

Status: accepted
Date: 2026-09-01
Requirements: FR-51

## Context

`M10.7`'s fault injection measured what `oqueue-store`'s `delete()` does when
one key in a run refuses: earlier keys are gone, the refused one is kept, and
the error names neither which key nor why. `M10.28` was opened to decide
whether that classification — `Transient`, via `classify.rs`'s shared
`classify()` — is the right answer, or whether a delete refusal deserves
something sharper.

**The information genuinely does not reach this crate.** `object_store`
0.14.1's public `Error` has twelve variants and none names a per-key delete
failure. Internally, `aws/client.rs` builds a crate-private `DeleteFailed`
carrying the key, S3's XML `<Error>` code, and its message — but converts all
but two variants of the dependency's own internal error enum to `Generic`
before it ever reaches a caller, with the original boxed as an opaque
`std::error::Error` source. `classify.rs`'s own header already documents why
nothing here can recover more: the type that would carry an HTTP status
(`RetryError`) is `pub(crate)` inside `object_store`, unreachable even by
name, and `error-handling.md` rule 8 bans reading a code out of a
`to_string()` as the workaround. So `Generic → Transient` is not a
classification choice this crate is declining to make more precise — the
input the more precise classification would need was erased one layer down,
by a dependency this crate does not own.

**The consequence is real, but it is bounded per call.** `Error::Transient`
maps to `RetryClass::Bounded` (`oqueue-core/src/retry.rs`), so a single
`delete()` retries a fixed number of times and stops — not forever. What
recurs is the *key*: if nothing calls `delete()` on it again, nothing retries
it again either. Whether that recurrence is a real cost depends entirely on
who calls `delete()` and how often — a question `oqueue-store` cannot answer
about itself, because `M5`, the milestone that owns garbage collection, is
**not started**. Nothing in the tree today calls `delete()` outside this
crate's own tests. `M5.md`'s own plan (task 18) already describes the shape
that would call it repeatedly: a three-state lifecycle — live → logically
deleted → deletion queue — with a TTL sweep and a per-round batch cap, plus
(task 20) a daily mark-and-sweep for orphans. Both are naturally
retry-across-rounds designs.

## Decision

**Keep today's classification. A per-key delete refusal answers `Transient`,
unchanged, through the same shared `classify()` every other `object_store`
error goes through — no special-cased delete path, no attempt to recover the
underlying S3 code.** `classify.rs`'s existing rationale for `Generic →
Transient` ("never retrying forever on what could be a permanent failure
this function failed to recognize would be the worse default") already
covers delete; nothing about delete specifically weakens that argument, and
carving out an exception here would be inventing a distinction this layer
cannot honestly make — `AccessDenied` and `InternalError` both currently
arrive as the identical `Generic { source }`, and no amount of local
cleverness recovers which one actually happened.

**"What should a GC pass do with a key it may never delete" is `M5`'s
question, not `oqueue-store`'s, and it does not need the S3 code to answer
it.** A key that fails every `delete()` attempt across many sweeps is
detectable *locally*, from the sweep's own history, without ever knowing
*why* S3 refused: task 18's deletion queue is exactly the place to count
consecutive failures per key and, past some threshold, stop retrying it on
every sweep and surface it instead — quarantined, alerted, or logged as an
operator-visible signal — the same shape task 20's orphan reconciliation
already uses to catch what routine deletion cannot resolve. This is a
heuristic over *repetition*, not over the error's classification, and it
belongs in the caller that has repetition to observe. `oqueue-store` staying
silent about *why* a key would not delete is not a gap this milestone leaves
open by omission — it is the fact `M5`'s task 18/20 shape was designed to
work without.

**An upstream `object_store` change is out of scope here**, not rejected. Filing
or vendoring a fix to expose S3's per-key delete error code is a real,
separate undertaking with its own review and maintenance cost, disproportionate
to a caller that does not exist yet. If `M5`'s deletion queue is built and the
repetition-based heuristic above turns out not to be enough — an operator needs
to distinguish "will never succeed" from "eventually will" to decide, say,
whether to escalate immediately rather than after N rounds — that is the point
to revisit whether an upstream contribution is worth its cost, informed by
what `M5` actually needed rather than by this row's speculation about it.

## Alternatives considered

**A. Reclassify a delete refusal as `Permanent`.** Rejected. It is exactly as
uninformed a guess as `Transient` — the erased code could equally have been
`InternalError`, which real retries do fix — and `Permanent`'s consequence is
worse in the direction that matters more here: a `Bounded` retry that turns
out unnecessary costs a few wasted requests, while a `Permanent` refusal that
turns out to have been transient means a live, deletable key is never
retried again by anything that trusts the classification, silently leaking
storage the GC safety inequality (`M5.md` task 19) assumed would eventually
succeed.

**B. Add a `DeleteRefused` (or similar) variant to `oqueue-core::Error`
specifically for this case.** Rejected as solving nothing on its own: the
variant would carry no more information than `Transient` already does,
since the underlying S3 code is unavailable regardless of what the local
type is named. A new variant earns its place when it changes what a caller
*does*, not to give an existing behavior a different name.

**C. Parse the S3 code out of the error's `Display`/`to_string()`.**
Rejected outright — `error-handling.md` rule 8 forbids exactly this, and
`object_store`'s own `Generic` variant does not even guarantee the boxed
source's `Display` output is stable across versions, which non-negotiable 2's
"never lower a threshold" spirit extends to: a classification that depends on
matching dependency-internal string formatting is a threshold no one agreed
to and no gate can catch drifting.

**D. Vendor or patch `object_store` now to expose the per-key code.**
Rejected for this milestone. `M5` — the only caller that would use the
information — has not started, so there is nothing yet to prove the extra
precision changes a real decision; see the Decision section's closing
paragraph for when to revisit this.

## Consequences

- `crates/oqueue-store/src/classify.rs` needs no change; this ADR records why
  the row closes without one, which a future reader would otherwise read as
  an oversight.
- `M5.md`'s task 18 (deletion queue) now names this directly, in the same
  commit as this ADR: a per-key delete failure it sees from `oqueue-store` is
  `Transient` and uninformative about cause, so whatever it builds for "a key
  that will not delete" has to work from repetition across sweeps rather than
  from a classification the layer below cannot provide. Task 20 (orphan
  reconciliation) is a different mechanism — it finds objects the index no
  longer references, not objects the index is actively failing to delete —
  and is left unannotated rather than stretching the same note to a task this
  decision does not actually bear on.
- The upstream-`object_store` option stays open and is not foreclosed by this
  decision — recorded so a future session does not re-litigate whether it was
  considered.
