# 0017. kafka-protocol for the message layer; oqueue-codec owns the frame and the batch hot path

Status: **superseded by [ADR-0019](0019-own-the-protocol-codec.md)**,
2026-08-24 (`M2.28`): `M2.26`'s fuzz harness found an unbounded-allocation
DoS the generated decoder cannot be made not to have, and the milestone
review ranked it blocking; `oqueue` now owns the message codec and
`kafka-protocol` becomes a test-only differential oracle. The Decision
below (the dependency as the single runtime protocol surface) no longer
holds. ⚠️ Earlier amendment, 2026-08-24 (`M2.21`): `oqueue-broker` had
joined `oqueue-codec` as the second crate importing `kafka-protocol`;
`ADR-0019` removes both from the runtime surface
Date: 2026-08-23
Requirements: FR-1, FR-2, FR-3

## Context

Doc 10 #4: build on the `kafka-protocol` crate vs hand-roll. Doc 02 §6.4's
finding shapes the whole question: Kafka's own broker and clients are
**generated** from JSON message specs (`versions`, `nullableVersions`,
`flexibleVersions` per field), and the two biggest sources of protocol
compatibility bugs are exactly what hand-rolling reintroduces — manually
tracked version-gated field presence, and hand-implemented flexible-versions
rules per message.

`kafka-protocol` 0.18.0, measured 2026-08-23 against the downloaded crate:
codegen from Kafka 4.1.0's own JSON specs; MIT/Apache-2.0; with
`default-features = false, features = ["broker"]` its dependencies are
`anyhow`, `bytes`, `crc`, `crc32c`, `indexmap`, `uuid` — pure Rust, no build
script, no C toolchain, so doc 20 §1's portability constraint holds.
Compression is feature-gated (`lz4`/`zstd` would pull C) and stays **off**;
compression is `oqueue-codec`'s seam. Doc 05 §2: the crate is the only viable
pure-Rust broker-side foundation, actively maintained, used by Shotover and
by `rustfs-kafka` — the latter this project's exact pattern. Known risk: a
small maintainer base.

What no dependency can own here: doc 18 §4.4's header-only offset rewrite
(raw bytes, decodes zero varints — the produce path must never decode
records), the CRC differential gate, the frame codec with correlation-ID
pipelining, and `security.md`'s bounded-input discipline at the socket edge.

## Decision

- `oqueue-codec` depends on `kafka-protocol` (`default-features = false`,
  `broker`) and is the workspace's **single protocol surface**: request and
  response headers, message bodies, per-(API, version) metadata and flexible
  encoding all come from the dependency, wrapped or re-exported here. No
  other crate imports `kafka-protocol` directly.
- `oqueue-codec` hand-rolls what the hot path owns: the length-delimited
  frame codec (correlation-ID extraction, the `ApiVersions` v0-header
  special case), the RecordBatch v2 header layout, CRC verify through
  `oqueue-checksum`, the header-only offset rewrite, the opt-in record
  iterator (SWAR varints), and the compression seam preferring pure-Rust
  codecs.
- The version is pinned; the golden-byte corpus (`M2.25`) is the drift
  detector. Upstream gaps get upstream contributions; forking is a new ADR.

## Alternatives considered

- **Hand-roll everything** — rejected: reintroduces both compat-bug classes
  doc 02 names, across dozens of message types times versions, roughly
  doubling the milestone for negative compatibility value.
- **Vendor or fork the crate/generator** — rejected for now: control this
  project does not yet need, at a permanent maintenance cost. The revisit
  trigger is the bus factor actually biting.
- **`rdkafka`** — not viable: an FFI client binding; cannot serve the broker
  side, and pulls the C toolchain doc 20 §1 budgets against.
- **Use the dependency's RecordBatch decoder on the produce path** —
  rejected: doc 18 §4.4's structural decision is that offset assignment
  decodes nothing; a decoder there is the wrong tool however good.

## Consequences

`M2.13`, `M2.15` and `M2.16` shrink to what stays ours (their rows say so);
`M2.21`-`M2.24` build bodies from generated types. Golden tests must pin the
dependency's bytes at every advertised version, because its correctness is
now this project's compatibility claim. `unsafe` in `oqueue-codec` remains
budgeted by non-negotiable 7 for the SWAR/hot path only.
