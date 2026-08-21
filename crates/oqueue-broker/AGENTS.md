# `oqueue-broker` — working notes

Read [`README.md`](README.md) first for what this crate is and why. This file is
what is specific to *changing* it.

## Which gates run

`check-layering.sh`, ⚠️ *not* `check-sans-io.sh` (this directory is exempt — the I/O shell lives here), `check-unsafe.sh`, `check-file-size.sh`,
`check-readmes.sh`, and `scripts/check-crate.sh oqueue-broker` for fmt, clippy and tests.

## Easy to get wrong here

1. ⚠️ **A composer.** `check-layering.sh` names this crate and `bin/oqueue` as the only two allowed to depend on workspace crates other than `oqueue-core`.
2. **Generic over its seams, not hardwired to them.** The concrete `ObjectStore` and `KeyProvider` are chosen by `bin/oqueue`; this crate names neither (NFR-51). ⚠️ **`Clock` is deliberately not claimed here** — not because the documents disagree, which `M1.29` found they do not, but because `check-sans-io.sh` says the real one lives here, so asserting the negative would be an invariant this crate is expected to break — the socket defect `M0.31` fixed. `check-sans-io.sh`'s exemption says where the real one *lives* (this crate); `architecture.md` says where concrete types are *chosen* (`bin/oqueue`); ADR-0004 takes no position on the real one's home. `README.md`'s Invariants row has the full reading.
3. ⚠️ **Backpressure between connections and the object-store write path is bespoke** — doc 05 §3 notes the real bottleneck is PUT throughput and request-rate limits, not socket I/O, and no crate provides that off the shelf.

## ⚠️ This crate is empty

`M0.8` created the skeleton so the workspace shape exists before any behaviour
does. Adding code here means the milestone that owns it has started — check
[`backlog.md`](../../docs/internal/product/backlog.md) rather than assuming.
