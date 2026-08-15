# `oqueue-testkit` — working notes

Read [`README.md`](README.md) first for what this crate is and why. This file is
what is specific to *changing* it.

## Which gates run

`check-layering.sh`, `check-sans-io.sh`, `check-unsafe.sh`, `check-file-size.sh`,
`check-readmes.sh`, and `scripts/check-crate.sh oqueue-testkit` for fmt, clippy and tests.

## Easy to get wrong here

1. ⚠️ **No fake lives here, and that is the point.** `contracts.md` rule 9 puts every fake beside its trait in `oqueue-core`, so a downstream crate can be tested without depending on this one; rule 11 calls a fake found here a layering violation to flag in review. ⚠️ `architecture.md`'s crate table said "Fakes, generators, harness" until `M0.8` corrected it.
2. **`publish = false`, and it appears in no crate's runtime `[dependencies]`.** `check-layering.sh` enforces the second half — a runtime dependency on this crate would ship test helpers in the deployed binary.

## ⚠️ This crate is empty

`M0.8` created the skeleton so the workspace shape exists before any behaviour
does. Adding code here means the milestone that owns it has started — check
[`backlog.md`](../../docs/internal/product/backlog.md) rather than assuming.
