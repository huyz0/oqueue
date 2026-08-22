# `oqueue-codec`

## What is it?

The Kafka wire protocol: request and response framing, and RecordBatch encode/decode.

## Why does it exist?

Because protocol compatibility is the product. Every byte a client sends or expects is decided here, and keeping it in one crate is what makes a golden-byte corpus and a fuzz target possible against a single surface.

## Upstream

- `oqueue-core` — the types, IDs, errors and trait seams this crate is written against.
- `lz4_flex` — the LZ4 frame codec, both directions, pure Rust.
- `ruzstd` — zstd decode, pure Rust; zstd *compression* is deliberately absent (`records.rs`'s module doc records the deferral).
- `flate2` — gzip, behind the `gzip` feature, pure Rust backend.
- `snap` — snappy, behind the `snappy` feature, pure Rust.
- `kafka-protocol` — the generated message layer: headers, bodies, and the
  per-version flexible rules (`ADR-0017`; `Cargo.toml`'s comment records the
  feature choices). This crate is the workspace's single protocol surface.

## The fuzz harness

`fuzz/` is a cargo-fuzz workspace of its own (deliberately outside the root
workspace: libFuzzer needs nightly), holding one target per decoder module —
frame, varint, batch, records, compress — each seeded from `fuzz/seeds/`,
which includes the real librdkafka frames the golden corpus captured.
`scripts/fuzz.sh` runs every target bounded and fails on a crash, a build
failure, or a codec module with neither a target nor an allowlisted reason
(`security.md` rule 5, `testing.md` rule 24); `.github/workflows/fuzz.yml`
schedules it nightly, the rule's "nightly tier" clause made real. ⚠️ The
end-to-end **request** path (the generated per-API body decoders behind the
dispatcher) is `M2.27`'s target, held back because the first thing it found
was a real unbounded-allocation DoS whose fix is its own task.

## Downstream

`oqueue-broker`, and through it `bin/oqueue`.

⚠️ **Only a composer may consume this crate.** `check-layering.sh` allows a
non-composer to depend on `oqueue-core` alone, so a sibling that needs a type
from here does not depend on here — the type belongs in `oqueue-core`.

## Invariants

| Must stay true | Held by |
|---|---|
| Nothing sized by a client-supplied length is allocated unbounded | review; `security.md` |
| Every decoder has a fuzz target | ⚠️ **no gate today** — `scripts/fuzz.sh` is named by `security.md` rule 5 and deferred into `M2` |

## Notes for whoever touches this

- **This crate parses untrusted input.** `security.md`'s rules on bounded allocation apply to every length field a client controls, and `testing.md` rule 24 puts a fuzz target on every decoder.
- ⚠️ **CRC-32C, not CRC-32/IEEE.** Doc 18 §4.3 records that `crc32fast` implements the wrong polynomial — no compile error, no runtime error, just batches every client rejects. The checksum lives in `oqueue-checksum`.
- **Hot-path `pub fn`s crossing into another crate carry `#[inline]`** — ADR-0003, which measured the band where it matters.
