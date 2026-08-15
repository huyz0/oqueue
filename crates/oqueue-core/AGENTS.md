# `oqueue-core` — working notes

Read [`README.md`](README.md) first for what this crate is and why. This file is
what is specific to *changing* it.

## Which standards bite hardest here

| Standard | Why here |
|---|---|
| [`contracts.md`](../../docs/internal/standards/contracts.md) | This is the only crate it fully applies to. Every rule about seams, fakes, and atomic contract changes is about code in this directory. |
| [`error-handling.md`](../../docs/internal/standards/error-handling.md) | The error taxonomy lives here, and every crate's failures cross it. |
| [`rust-style.md`](../../docs/internal/standards/rust-style.md) | Lints are declared in the root `Cargo.toml` and opted into with `lints.workspace = true`. Never add a `#![warn(...)]` block to `lib.rs`. |

## Which gates run

`check-layering.sh`, `check-sans-io.sh`, `check-unsafe.sh`,
`check-core-contract.sh`, `check-file-size.sh`, `check-readmes.sh`, and
`scripts/check-crate.sh oqueue-core` for fmt, clippy and tests.

⚠️ **`check-core-contract.sh` is scoped to the staged diff**, unlike the others,
which scan the tracked tree. It skips entirely when no `.rs` file is staged — so
a green run says nothing about this crate unless this crate is in the commit.

## What a reviewer will check that no gate can

- **Whether the trait is the right seam at all.** The gate checks that a
  method-set change carries every implementor and an ADR. It cannot check that
  the split is in the right place, that the ADR's stated reason is the real one,
  or that every call site was updated *correctly* rather than merely updated.
- **Whether a fake is faithful to the documented contract** or merely convenient
  for the first test that used it. Nothing mechanical distinguishes those.
- **Whether a new type belongs on the seam or beside it.**

## Easy to get wrong here

1. **Adding a dependency.** `[dependencies]` is empty on purpose. Every crate in
   the workspace rebuilds when this one changes, so anything added here is
   added to everyone's critical path. If it seems necessary, that is usually a
   sign the thing being written belongs in a crate downstream of this one.
2. **A wholly new `pub trait` needs an ADR in the same commit.**
   `check-core-contract.sh` treats a new trait as a changed method set — the
   old signature set is `None`, the new one is not — so the commit is refused
   unless a file under `docs/internal/product/decisions/` is staged with it. An
   ADR that landed in an *earlier* commit does not satisfy it: the gate reads
   `git diff --cached`. `contracts.md` rule 15 says this is correct, not a
   false positive: a new trait defines what an implementor must guarantee.
3. **Putting a fake in `oqueue-testkit`.** It goes here, beside its trait.
4. **Reaching for the clock or the filesystem in a test.** If a test here needs
   I/O, the logic under test is in the wrong crate.
