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
| Compaction's planning half costs no object-storage operation | `read_amp`, `plan` and `sweep` take a `MaterializedIndex` and no store, and `check-sans-io.sh`'s planning leg forbids that seam's name — the fake and the three wrappers included — in **every** file under `src/` except the executor's (`merge.rs`, `merge/outcome.rs`, `layout.rs`), which are named in the script. It fails if the directory has moved, if it finds no file to check, or if an exempted file does not exist; `ADR-0036`, `M5.38`, widened by `M5.47` after `plan.rs` and `sweep.rs` each claimed the property with nothing holding it. ⚠️ **Not** the dependency set — `oqueue-core` exports `ObjectStore` — and **not** the gate's SDK patterns, which a call through the core trait does not match |

## Notes for whoever touches this

- **Compaction rewrites data that has already been acknowledged.** NFR-20 — no acknowledged record is ever lost — is the requirement everything here yields to.
- **Read amplification is the only trigger.** Never object count: `M5.md` task 1 forbids it, because object count is a coordinator cost and a partition whose many objects are never read together has no amplification to fix.
