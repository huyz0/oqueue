# `oqueue-coordinator` — working notes

Read [`README.md`](README.md) first for what this crate is and why. This file is
what is specific to *changing* it.

## Which gates run

`check-layering.sh`, `check-sans-io.sh`, `check-unsafe.sh`, `check-file-size.sh`,
`check-readmes.sh`, and `scripts/check-crate.sh oqueue-coordinator` for fmt, clippy and tests.

## Easy to get wrong here

1. **Monotonicity is the invariant everything rests on.** `oqueue_core::Offset::add` refuses to wrap for this reason.
2. ⚠️ **Metadata cost must be proportional to partitions active on this node**, never to cluster-wide partition count — doc 15 and doc 16. That rule is what rules out the obvious designs at the 1M-100M topic target.

## ⚠️ This crate is empty

`M0.8` created the skeleton so the workspace shape exists before any behaviour
does. Adding code here means the milestone that owns it has started — check
[`backlog.md`](../../docs/internal/product/backlog.md) rather than assuming.
