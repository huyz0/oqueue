# `oqueue-coordinator`

## What is it?

Metadata, offset sequencing, and recovery.

## Why does it exist?

Because offsets must be totally ordered per partition and object storage offers no atomic append. Sequencing is the one place that ordering is decided, and M3 is where it is built.

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
| Offsets are totally ordered per partition | `M3`'s tests; `Offset::add` refuses to wrap |
| Metadata cost is proportional to partitions active on this node | no gate — doc 15, doc 16 |

## Notes for whoever touches this

- **Monotonicity is the invariant everything rests on.** `oqueue_core::Offset::add` refuses to wrap for this reason.
- ⚠️ **Metadata cost must be proportional to partitions active on this node**, never to cluster-wide partition count — doc 15 and doc 16. That rule is what rules out the obvious designs at the 1M-100M topic target.
