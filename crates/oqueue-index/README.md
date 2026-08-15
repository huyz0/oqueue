# `oqueue-index`

## What is it?

The offset→object index and its search: given an offset, which object holds it and where inside it.

## Why does it exist?

Because a fetch must not enumerate object storage. Doc 12 measures LIST at 12-38x the price of a GET and semantically useless besides, so the index is what turns a read into a bounded number of GETs.

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
| A lookup issues no object-storage LIST | no gate until `M1`; `performance.md` rule 18 gives it a benchmark |

## Notes for whoever touches this

- **Search is on the cached tail-read path** (NFR-2), where the entire budget is CPU. `performance.md` rule 18 gives index lookup a benchmark obligation.
