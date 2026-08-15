---
title: "Contracts"
description: >
  Read when defining or changing a `pub trait` in `oqueue-core`, adding a fake, or deciding whether a new type belongs on a seam or beside one.
tags: [code, traits, seams, core, fakes, versioning]
applies_to: ["*oqueue-core/*", "*oqueue-buf/*", "*oqueue-codec/*", "*oqueue-checksum/*", "*oqueue-index/*", "*oqueue-store/*", "*oqueue-coordinator/*", "*oqueue-crypto/*", "*oqueue-compact/*", "*oqueue-broker/*", "*oqueue-testkit/*", "bin/oqueue/*", "*/bin/oqueue/*", "docs/internal/product/decisions/*"]
---

# Contracts

Rules for the seams between crates: what a `pub trait` in `oqueue-core` must
look like, what changing one costs, and where the line sits between "this is a
contract" and "this is an implementation detail." Evidence in
[docs/researches/19](../../researches/19-workspace-engineering.md) §3.

⚠️ **This standard governs the trait, not the error type it returns.** What an
error variant looks like and how it propagates is
[error-handling.md](error-handling.md). What may run inside a trait
implementation — blocking calls, locks, spawned tasks — is
[async-concurrency.md](async-concurrency.md).

## What makes something a contract

1. **A seam is a `pub trait` in `oqueue-core`, and nowhere else.** If two
   crates need to agree on a boundary, the boundary is a trait in `core`, not
   a shared concrete type one of them owns. → `check-layering.sh`
2. **Not every shared type is a contract.** A struct passed across a boundary
   (`ObjectKey`, `ByteRange`) is data, not a seam — it has no implementors to
   keep in sync and changing its fields is an ordinary commit, reviewed like
   any other. The atomic-change rule below is about *trait method sets*, not
   about every `pub` item in `core`.
3. **A trait exists because more than one implementation is expected**, now
   or foreseeably — `ObjectStore` has three, `Clock` has at least two (real,
   simulated). A trait with exactly one implementation and no second one
   planned is a struct pretending to be a seam; inline it.
4. **Split `core` before it becomes the bottleneck.** ⚠️ **UNDERIVED for
   oqueue**, which has zero `core` traits as of this standard being written.
   [Doc 19](../../researches/19-workspace-engineering.md) §11 says a
   comparable service holds on the order of a dozen traits in `core`
   comfortably, and separately poses `~40` as its own open question — a
   speculative threshold for *this* project, not a number measured against
   either the comparable service or oqueue itself. Carried forward as a
   watch threshold, not a derived constant: revisit once `oqueue-core`
   actually holds enough traits to check it against. What doc 19 does
   establish is the mechanism — every crate depends on `core`, so a change
   there invalidates the whole build, and the risk grows with trait count.
   If growth toward that borrowed figure is observed, the split is
   `core-types` (data, rarely changes) and `core-traits` (seams) — not a
   reason to avoid adding a trait that belongs there well before this
   becomes relevant.

## Shape every trait must have

5. **Every trait carries `Send + Sync + fmt::Debug`.** `Debug` is not
   cosmetic — it is what lets any component be logged in an error path
   without a `where`-clause fight. → `missing_debug_implementations = "warn"`
   once `Cargo.toml`'s workspace lints land (M0); [rust-style.md](rust-style.md) rule 5
6. **A trait's methods return `Result`, never a panic, for anything the
   implementation does not fully control** — a network call, a store
   operation, anything crossing process or trust boundaries. See
   [error-handling.md](error-handling.md) rule 1.
7. **A trait is generic over nothing it doesn't have to be.** An
   object-safe, `dyn`-compatible trait is the default for a seam meant to be
   swapped at runtime (backend selection, test injection); an associated-type
   or generic-parameter design is the exception, and the exception needs a
   reason in the ADR.
8. **A trait method's parameters and return types are types this workspace
   owns**, not a re-export of a dependency's type reaching across the seam.
   A dependency swap should never ripple through every implementor's
   signature.

## Fakes live beside the trait

9. **Every fake implementation lives beside the trait it implements, in
   `oqueue-core`.** This is what makes every downstream crate testable in
   isolation without a `oqueue-testkit` dependency, and it is why that crate
   should stay nearly empty — growth there means fakes have drifted away
   from the contracts they stand in for. → [testing.md](testing.md) rule 4
10. **A fake's job is fidelity to the trait's *documented* contract, not
    convenience for the test that happens to use it first.** An
    over-permissive fake is worse than no fake — see
    [testing.md](testing.md) rule 27 and the risk it names for `ObjectStore`
    specifically.
11. **`oqueue-testkit` holds harness and generators, never a trait
    implementation that belongs beside its trait.** A fake found there
    instead of in `core` is a layering violation even though the script that
    would catch it checks dependency direction, not file location — flag it
    in review.

## A contract change arrives whole

12. **Changing a `pub trait`'s method set is one commit**, containing: the
    trait, every fake, every implementation, every call site the compiler
    forces, and the ADR recording why. → `scripts/check-core-contract.sh`
    (M-1.10); non-negotiable 6 in `AGENTS.md`
13. **The gate compares the `fn` signature set inside each `pub trait`
    block**, HEAD against the index — not file mtime, not the whole diff.
    Editing a doc comment on a trait is not a contract change; a rule that
    fires on doc comments becomes noise and gets disabled. [Doc 19](../../researches/19-workspace-engineering.md) §3.2
14. ⚠️ **The gate covers only what is mechanical: implementors present, and
    an ADR file in the same commit.** It does not and cannot verify that
    every call site was updated correctly or that a dependent spec was
    revised — those are not mechanically distinguishable from ordinary
    edits. That is review's job, not a reason to widen the gate until it
    overclaims and gets routed around.
15. **A contract change that is only additive** (a new method with a
    default body, a new trait entirely) still gets an ADR if it changes what
    an implementor is expected to guarantee — adding a method silently
    narrows what "implements this trait" means for existing code that must
    now also satisfy it.

## What crosses the seam and what doesn't

16. **A concrete socket type, the real clock, or an object-store call never
    appears on either side of a seam signature in a library crate.** The
    trait is the injection point; see `AGENTS.md` non-negotiable 5 and
    [testing.md](testing.md) rule 5.
17. **Each crate defines its own error enum; only `bin/oqueue` uses
    `anyhow`.** [Doc 19](../../researches/19-workspace-engineering.md) §3.3: leaking a
    dependency's error type or a catch-all `anyhow::Error` across a crate
    boundary destroys the ability to match on failure modes at the seam. See
    [error-handling.md](error-handling.md) rules 3–4.

## What has no gate

**Whether a trait is the right seam**, as opposed to a correctly-shaped
trait in the wrong place. `check-layering.sh` and `check-core-contract.sh`
both assume the trait boundary is already correctly drawn; neither can tell
you that `ObjectStore` should not have grown a `list()` method, only that
the change to add one was made atomically. That is an ADR-time decision —
see [sdd.md](sdd.md) and the `adr` skill.

**Whether the ADR's stated reason for a contract change is actually the
reason.** The gate checks that a file exists in the commit; it cannot check
that the file is honest.

## See also

- Crate map, dependency rule, seam list: [architecture.md](../product/architecture.md)
- Contracts and the star topology: [docs/researches/19](../../researches/19-workspace-engineering.md) §1, §3
- Recording a decision: [`adr`](../../../.agents/skills/adr/SKILL.md)
