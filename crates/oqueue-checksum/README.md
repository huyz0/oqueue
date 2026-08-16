# `oqueue-checksum`

## What is it?

CRC-32C (Castagnoli), with runtime dispatch to whatever the CPU supports.

## Why does it exist?

Because Kafka's v2 RecordBatch mandates CRC-32C and the spread between a scalar and a hardware implementation is large enough to matter on every batch. One crate, one algorithm, one known-answer test.

## Upstream

- `oqueue-core` — the types, IDs, errors and trait seams this crate is written against.

## Downstream

`oqueue-broker`, and through it `bin/oqueue`.

⚠️ **Only a composer may consume this crate.** `check-layering.sh` allows a
non-composer to depend on `oqueue-core` alone, so a sibling that needs a type
from here does not depend on here — the type belongs in `oqueue-core`.

## Invariants

| Must stay true | Held by |
|---|---|
| The polynomial is Castagnoli, never IEEE | `M2`'s known-answer test against published vectors — ⚠️ nothing today |
| Dispatch is resolved once, not per call | no gate — review, and `M14`'s benchmarks |

## Notes for whoever touches this

- ⚠️ **The polynomial is Castagnoli, not IEEE.** Getting this wrong produces batches every Kafka client rejects, with no error anywhere on this side. A known-answer test against published vectors is not optional.
- **Runtime dispatch, resolved once.** Doc 18 §3.4: the baseline is `x86-64-v2`, which makes SSE4.2 `crc32` statically available; anything above it goes through a dispatch resolved at startup, not per call (§4.2). ⚠️ Both sections, and this cited only the second until `M0.29` — the baseline recommendation is §3.4's.
