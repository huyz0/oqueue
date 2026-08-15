# `oqueue-store` — working notes

Read [`README.md`](README.md) first for what this crate is and why. This file is
what is specific to *changing* it.

## Which gates run

`check-layering.sh`, `check-sans-io.sh`, `check-unsafe.sh`, `check-file-size.sh`,
`check-readmes.sh`, and `scripts/check-crate.sh oqueue-store` for fmt, clippy and tests.

## Easy to get wrong here

1. ⚠️ **The in-memory backend here is a real implementation, not a fake.** `testing.md` rule 6 and `contracts.md` rule 9: the *fake* lives beside the trait in `oqueue-core`. Two things that look alike and are not — the fake is for downstream unit tests, this is a backend that must pass the same conformance suite as S3.
2. ⚠️ **Conditional-write semantics are the highest-risk surface in the project** (doc 10 #33). If this backend and the fake disagree about a failed precondition, the result is an architectural error, not a test gap.

## ⚠️ This crate is empty

`M0.8` created the skeleton so the workspace shape exists before any behaviour
does. Adding code here means the milestone that owns it has started — check
[`backlog.md`](../../docs/internal/product/backlog.md) rather than assuming.
