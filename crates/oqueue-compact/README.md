# `oqueue-compact`

## What is it?

Compaction planning and execution: deciding which objects to merge, and doing it.

## Why does it exist?

Because objects arrive small and reads want them large, and the gap between those is the whole cost model. Planning is separable from execution and both are testable without touching a network.

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
| No acknowledged record is lost by a rewrite | `M5`'s tests; NFR-20 |

## Notes for whoever touches this

- **Compaction rewrites data that has already been acknowledged.** NFR-20 — no acknowledged record is ever lost — is the requirement everything here yields to.
