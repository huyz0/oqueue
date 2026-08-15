# `oqueue-buf`

## What is it?

Buffer primitives: refcounted slices, and the pooling that keeps record bytes from being copied on every hop.

## Why does it exist?

Because a broker's throughput is decided by how many times a byte is copied between the socket and the object store, and the answer has to be *once*. Centralising the buffer type is what lets every other crate hand bytes along without owning them.

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
| No `unsafe` outside a budgeted block | `scripts/check-unsafe.sh` against `baselines/unsafe.txt` |
| A byte is copied once between socket and object store | no gate — review, and `M14`'s benchmarks |

## Notes for whoever touches this

- **`unsafe` is permitted here and is budgeted.** `check-unsafe.sh` lists this crate in `ALLOWED_CRATES` and every block needs an entry in `baselines/unsafe.txt`. ⚠️ Doc 18 §0.5: bounds-check elimination is worth 1-3%. If a block is trading soundness for 2%, it does not belong.
- **Every `unsafe` block gets a differential property test** against a naive safe implementation — `testing.md` rule 21.
