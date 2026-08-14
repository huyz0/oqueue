---
name: contract-change
description: Change a pub trait's method set in oqueue-core. Use whenever a trait signature in core gains, loses, or changes a method — never for a doc-comment edit or a new struct that isn't a trait. Drives the trait, every fake, every implementation, every call site, and the ADR into the one commit check-core-contract.sh requires.
---

# Contract change

A `pub trait` in `oqueue-core` is a seam every crate depends on: changing its
method set invalidates the whole build, deliberately, so nothing lands with a
missing implementor. Non-negotiable 6; `contracts.md` rules 12–15.

## Before touching the trait

1. **Confirm this is actually a contract change.** A doc-comment edit, a
   rename that leaves the signature set unchanged, or a new struct with no
   `pub trait` involved is not one — `contracts.md` rules 1–3 say what makes
   something a seam at all. If it isn't, this skill doesn't apply; make the
   change as an ordinary commit.
2. **Write the ADR first**, before the trait changes — [`adr`](../adr/SKILL.md).
   It is required, not optional, for a method-set change (`contracts.md` rule
   12), including a purely additive one if it narrows what an implementor
   must guarantee (rule 15). The alternatives are only genuinely live before
   the code exists; an ADR written after is a justification, not a decision.

## The one commit

`scripts/check-core-contract.sh` (M-1.10) fires once any `pub trait`'s
signature set differs between HEAD and the staged index, and enforces two
mechanical obligations: every file that currently implements the trait must
be part of this commit, and an ADR file under
`docs/internal/product/decisions/` must be too. Everything below this line is
what the gate cannot check, and is why the change is driven by a skill and
not left to the gate alone.

1. **Change the trait.** Keep the shape rules: `Send + Sync + fmt::Debug`
   (rule 5), `Result` for anything the implementation doesn't fully control
   (rule 6), no dependency type crossing the seam (rule 8).
2. **Update every fake beside it**, in `oqueue-core` (rule 9) — never in
   `oqueue-testkit` (rule 11). A fake's job is fidelity to the trait's
   *documented* contract, not convenience for whichever test used it first
   (rule 10): bring it up to the new contract, don't patch around the old one.
3. **Update every real implementation the compiler now forces.** Follow the
   compiler errors — that is the exhaustive list, not a search.
4. **Update every call site the new signature touches**, and its tests.
5. **Implement test-first** — [`tdd`](../tdd/SKILL.md) — for whatever
   behavior actually changed. The mechanical parts (implementor present, ADR
   present) are the gate's job, not a test's.
6. **Stage the ADR** written in step 1, in this same commit.

## Before committing

- `scripts/check-core-contract.sh` passes — every implementor and the ADR
  are staged alongside the trait.
- `scripts/check-layering.sh` and `scripts/check-sans-io.sh` still pass — the
  seam doesn't leak a concrete socket, clock, or store type across it
  (`contracts.md` rule 16).
- The commit is reviewed — [`review`](../review/SKILL.md) — for exactly what
  the gate says it cannot see: whether every call site was updated
  *correctly*, whether a dependent spec needs revising (`contracts.md` rule
  14), and whether the ADR's stated reason is the real one (`contracts.md`'s
  own "What has no gate" section). All three are named as review's job, not
  the gate's.

## What this skill does not cover

Whether the trait is the right seam at all. A correctly-shaped trait added in
the wrong place is caught by neither `check-core-contract.sh` nor this
skill's procedure — only by the ADR-time judgement `contracts.md` and
`sdd.md` ask for before this skill starts.
