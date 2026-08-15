# `oqueue-testkit`

## What is it?

Test harness and data generators. **No fakes.**

## Why does it exist?

Because the property tests and the conformance suites need seeded generators and a common harness, and duplicating those across ten crates would guarantee they drift.

## Upstream

- `oqueue-core` — the types, IDs, errors and trait seams this crate is written against.

## Downstream

Nothing at runtime. Every crate's `[dev-dependencies]`, and nothing else.

⚠️ **The opposite rule applies here to every other crate.** Any crate may take
this one as a **dev**-dependency, and none may take it as a runtime one —
`check-layering.sh` refuses `oqueue-testkit` in any `[dependencies]`, because a
runtime dependency would ship test helpers in the deployed binary. ⚠️ It never
reads `[dev-dependencies]`, so what lands there is review's.

## Invariants

| Must stay true | Held by |
|---|---|
| Holds no fake | ⚠️ **no gate** — `contracts.md` rule 11 calls a fake here a layering violation to flag in review; `m0-complete.sh` (`M0.18`) asserts fakes live in `oqueue-core` |
| Appears in no runtime `[dependencies]` | `scripts/check-layering.sh` |

## Notes for whoever touches this

- ⚠️ **No fake lives here, and that is the point.** `contracts.md` rule 9 puts every fake beside its trait in `oqueue-core`, so a downstream crate can be tested without depending on this one; rule 11 calls a fake found here a layering violation to flag in review. ⚠️ `architecture.md`'s crate table said "Fakes, generators, harness" until `M0.8` corrected it.
- **`publish = false`, and it appears in no crate's runtime `[dependencies]`.** `check-layering.sh` enforces the second half — a runtime dependency on this crate would ship test helpers in the deployed binary.
