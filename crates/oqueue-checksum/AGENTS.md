# `oqueue-checksum` — working notes

Read [`README.md`](README.md) first for what this crate is and why. This file is
what is specific to *changing* it.

## Which gates run

`check-layering.sh`, `check-sans-io.sh`, `check-unsafe.sh`, `check-file-size.sh`,
`check-readmes.sh`, and `scripts/check-crate.sh oqueue-checksum` for fmt, clippy and tests.

## Easy to get wrong here

1. ⚠️ **The polynomial is Castagnoli, not IEEE.** Getting this wrong produces batches every Kafka client rejects, with no error anywhere on this side. A known-answer test against published vectors is not optional.
2. **Runtime dispatch, resolved once.** Doc 18 §3.4: the baseline is `x86-64-v2`, which makes SSE4.2 `crc32` statically available; anything above it goes through a dispatch resolved at startup, not per call (§4.2). ⚠️ Both sections, and this cited only the second until `M0.29` — the baseline recommendation is §3.4's.

## Filled by `M2.12`

`M0.8` created the skeleton; `M2.12` filled it: `crc32c`, `crc32c_combine`,
the iSCSI known-answer vectors, a combine property test, and a scalar
differential reference. The `unsafe` budget is allocated and deliberately
unused — the SIMD lives in `crc-fast`.
