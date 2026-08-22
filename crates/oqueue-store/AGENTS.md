# `oqueue-store` — working notes

Read [`README.md`](README.md) first for what this crate is and why. This file is
what is specific to *changing* it.

## Which gates run

`check-layering.sh`, `check-sans-io.sh`, `check-unsafe.sh`, `check-file-size.sh`,
`check-readmes.sh`, and `scripts/check-crate.sh oqueue-store` for fmt, clippy and tests.

## Easy to get wrong here

1. ⚠️ ~~**The in-memory backend here is a real implementation, not a fake.**~~ — **there is no in-memory backend here, and `M1.37` found the distinction had dissolved rather than been abandoned.** `ADR-0005` planned two things that look alike: a *fake* beside the trait in `oqueue-core` (`contracts.md` rule 9) "modelling no failure and no latency at all", and a real in-memory backend here passing the same conformance suite as S3. `M1` built one thing that does both jobs. `M1.8` gave `oqueue-core`'s `FakeObjectStore` a `FaultConfig` — latency, error storms, `crash_after_put_before_ack` — and `M1.10` runs it through the full conformance suite at `Capabilities::FULL`, recorded `verified` in `baselines/conformance-matrix.txt`. So the second implementation was never built because nothing was left for it to do. This crate holds `S3Store` and `GcsStore`.
2. ⚠️ **Conditional-write semantics are the highest-risk surface in the project** (doc 10 #33). If a real backend and the fake disagree about a failed precondition, the result is an architectural error, not a test gap.

## Status

`M0.8` created the skeleton; `M1.15`/`M1.17` landed both real backends (S3,
GCS). ⚠️ **"Every backend passes the same conformance suite" is not yet
actually held for GCS** — ADR-0014: no available emulator round-trips
`object_store`'s GCS requests yet, so `GcsStore` is verified at T0 only,
unlike `S3Store`'s MinIO-verified path. Check
[`backlog.md`](../../docs/internal/product/backlog.md) for what is done.
