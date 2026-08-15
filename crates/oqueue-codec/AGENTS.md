# `oqueue-codec` — working notes

Read [`README.md`](README.md) first for what this crate is and why. This file is
what is specific to *changing* it.

## Which gates run

`check-layering.sh`, `check-sans-io.sh`, `check-unsafe.sh`, `check-file-size.sh`,
`check-readmes.sh`, and `scripts/check-crate.sh oqueue-codec` for fmt, clippy and tests.

## Easy to get wrong here

1. **This crate parses untrusted input.** `security.md`'s rules on bounded allocation apply to every length field a client controls, and `testing.md` rule 24 puts a fuzz target on every decoder.
2. ⚠️ **CRC-32C, not CRC-32/IEEE.** Doc 18 §4.3 records that `crc32fast` implements the wrong polynomial — no compile error, no runtime error, just batches every client rejects. The checksum lives in `oqueue-checksum`.
3. **Hot-path `pub fn`s crossing into another crate carry `#[inline]`** — ADR-0003, which measured the band where it matters.

## ⚠️ This crate is empty

`M0.8` created the skeleton so the workspace shape exists before any behaviour
does. Adding code here means the milestone that owns it has started — check
[`backlog.md`](../../docs/internal/product/backlog.md) rather than assuming.
