# `oqueue-buf` — working notes

Read [`README.md`](README.md) first for what this crate is and why. This file is
what is specific to *changing* it.

## Which gates run

`check-layering.sh`, `check-sans-io.sh`, `check-unsafe.sh`, `check-file-size.sh`,
`check-readmes.sh`, and `scripts/check-crate.sh oqueue-buf` for fmt, clippy and tests.

## Easy to get wrong here

1. **`unsafe` is permitted here and is budgeted.** `check-unsafe.sh` lists this crate in `ALLOWED_CRATES` and every block needs an entry in `baselines/unsafe.txt`. ⚠️ Doc 18 §0.5: bounds-check elimination is worth 1-3%. If a block is trading soundness for 2%, it does not belong.
2. **Every `unsafe` block gets a differential property test** against a naive safe implementation — `testing.md` rule 21.

## ⚠️ This crate is empty

`M0.8` created the skeleton so the workspace shape exists before any behaviour
does. Adding code here means the milestone that owns it has started — check
[`backlog.md`](../../docs/internal/product/backlog.md) rather than assuming.
