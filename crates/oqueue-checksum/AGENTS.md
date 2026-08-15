# `oqueue-checksum` — working notes

Read [`README.md`](README.md) first for what this crate is and why. This file is
what is specific to *changing* it.

## Which gates run

`check-layering.sh`, `check-sans-io.sh`, `check-unsafe.sh`, `check-file-size.sh`,
`check-readmes.sh`, and `scripts/check-crate.sh oqueue-checksum` for fmt, clippy and tests.

## Easy to get wrong here

1. ⚠️ **The polynomial is Castagnoli, not IEEE.** Getting this wrong produces batches every Kafka client rejects, with no error anywhere on this side. A known-answer test against published vectors is not optional.
2. **Runtime dispatch, resolved once.** Doc 18 §4.2: the baseline is `x86-64-v2`, which makes SSE4.2 `crc32` statically available; anything above it goes through a dispatch resolved at startup, not per call.

## ⚠️ This crate is empty

`M0.8` created the skeleton so the workspace shape exists before any behaviour
does. Adding code here means the milestone that owns it has started — check
[`backlog.md`](../../docs/internal/product/backlog.md) rather than assuming.
