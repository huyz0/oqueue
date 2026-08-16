# `oqueue-core`

## What is it?

The types, IDs, errors, and trait seams every other crate in the workspace is
written against. It is the one crate everything depends on, and the one crate
that depends on **no other crate in the workspace**. Externally it takes exactly
one dependency, `thiserror`, for the reason under *Upstream*.

## Why does it exist?

Because the alternative is that each crate defines its own idea of an offset, a
topic identifier, or "the thing that reads the clock", and the first time two of
them have to agree, somebody writes a conversion. Putting the vocabulary and the
seams in one crate is what makes [`architecture.md`](../../docs/internal/product/architecture.md)'s
star topology possible: every crate depends on this one and on nothing else in
the workspace, so the build DAG stays two links deep however many crates are
added sideways, and every crate can be tested without constructing the system
around it.

Merging its contents into a neighbour would collapse that. The seams have to
live somewhere no implementation lives, or the dependency inversion that makes
them useful stops working — a trait defined next to its S3 implementation is not
a seam, it is a description of that implementation.

## Upstream

**No workspace crate**, ever. That is the rule rather than a coincidence: the
star topology holds only if its centre is a leaf. `check-layering.sh` enforces
the direction of every other crate's dependencies; this crate having none is
what makes depth 2 achievable at all.

Externally, exactly one:

- `thiserror` — `error-handling.md` rule 3 requires each crate to define its
  error enum with it, and `M0.5` brought the first one. A proc macro, so no C toolchain. ⚠️ The
  runtime graph is `thiserror` alone: `proc-macro2`, `quote`, `syn` and
  `unicode-ident` are host-only, nested under `thiserror-impl`.

`proptest` is a **dev**-dependency, for the invariant tests, so it is absent
from the list above and from the shipped graph. ⚠️ `check-layering.sh` and
`check-readmes.sh` both read `[dependencies]` only, so nothing gates what lands
in `[dev-dependencies]`.

⚠️ **Adding a dependency here is a decision that costs the whole workspace.**
Every crate rebuilds when this one changes, so a dependency added here is a
dependency every crate now waits for. Each one needs a reason written beside it
in `Cargo.toml`.

## Downstream

Everything. Every library crate, `oqueue-broker`, and `bin/oqueue`.

That is the cost model for a breaking change: there is no such thing as a cheap
one. Changing a `pub trait`'s method set here is one commit containing the
trait, every fake, every implementation, every call site, and an ADR —
[`contracts.md`](../../docs/internal/standards/contracts.md) rules 12-15, held
by `check-core-contract.sh`.

## Invariants

| Must stay true | Held by |
|---|---|
| No `unsafe` | `#![forbid(unsafe_code)]` in `lib.rs`, and `scripts/check-unsafe.sh` |
| No workspace dependency, and nothing depends on it in the wrong direction | `scripts/check-layering.sh` |
| No I/O, no real clock, no object storage | `scripts/check-sans-io.sh` |
| A method-set change carries every implementor and an ADR in one commit | `scripts/check-core-contract.sh` |
| Every `pub trait` has a fake beside it, here | `scripts/gates/m0-complete.sh` (`M0.18`), at the milestone boundary — **not** per commit. `check-core-contract.sh` checks implementors and the ADR and knows nothing about fakes: `contracts.md` rule 14 says it cannot, and rule 11 says a misplaced fake is "a layering violation ... flag it in review". ⚠️ So between boundaries this is still review's job, and the gate's scanner is a line pattern — a fake behind `#[cfg(test)]` or inside a `/** */` block satisfies it, recorded in `M0.18`'s notes |
| No file over 500 lines | `scripts/check-file-size.sh` |

⚠️ **Watch this crate's size.** A comparable service holds around a dozen traits
comfortably; if ours approaches ~40, the pre-planned split is `core-types`
(rarely changes) from `core-traits`. Tracked as open question #34.

## Notes for whoever touches this

- **A fake belongs here, beside its trait — not in `oqueue-testkit`.** That is
  what lets a downstream crate be tested without a testkit dependency, and a
  fake found in the testkit is a layering violation even though the script that
  would catch it checks dependency direction rather than file location.
  `contracts.md` rules 9 and 11.
- **A fake's job is fidelity to the trait's documented contract**, not
  convenience for the test that happens to use it first. An over-permissive fake
  is worse than no fake, and the `ObjectStore` one is the highest-risk component
  in the project for exactly this reason.
- **`KeyProvider` is wrap/unwrap only** — `M0.11`, ADR-0006. ⚠️ No
  `generate_data_key`: AWS KMS has one, GCP Cloud KMS has no equivalent, so a
  seam carrying it is a seam only one cloud can implement.
- **`ObjectStore` is the seam everything else leans on** — `M0.10`, ADR-0005.
  ⚠️ Exactly one fake exists in this tree and `M1` **rewrites** it rather than
  adding another: two fakes with divergent conditional-write semantics is the
  highest-risk defect class in the project.
- **`Clock` is the first seam, and its fake is beside it** — `M0.9`, ADR-0004.
  `FakeClock::advance` is the only way its time moves, and it refuses a
  negative delta so a test cannot build a clock no real implementor could be.
- **The crate is nearly empty today and that is the plan, not neglect.** `M0.5`
  brings the core types and IDs, `M0.6` the error taxonomy with FR-44's
  redaction rules, and `M0.9` through `M0.11` the `Clock`, `ObjectStore` and
  `KeyProvider` seams — each with its own ADR in its own commit, because
  `contracts.md` rule 15 makes a wholly new trait a contract decision and
  `check-core-contract.sh` refuses the commit without one.
