# `oqueue-broker`

## What is it?

The I/O shell: the connection loop, request dispatch, and the batching that turns many small produces into few large PUTs.

## Why does it exist?

Because everything else in this workspace is sans-I/O by construction, and the sockets have to be somewhere. This crate is that somewhere, and it is generic over its seams so it can be tested without any of them being real.

## Upstream

- `oqueue-core` — the types, IDs, errors and trait seams this crate is written against.

## Downstream

`bin/oqueue`, the composition root.

⚠️ **Only a composer may consume this crate.** `check-layering.sh` allows a
non-composer to depend on `oqueue-core` alone, so a sibling that needs a type
from here does not depend on here — the type belongs in `oqueue-core`.

## Invariants

| Must stay true | Held by |
|---|---|
| Names no concrete backend, clock or socket type | ⚠️ **No gate, and specifically not `check-sans-io.sh`** — that script *exempts* this directory, because the I/O shell and the real `Clock` are exactly what live here. Review is the only thing holding it, and this is the crate where that matters most |
| Depends only on `oqueue-core` and its siblings as a composer | `scripts/check-layering.sh` |

## Notes for whoever touches this

- ⚠️ **A composer.** `check-layering.sh` names this crate and `bin/oqueue` as the only two allowed to depend on workspace crates other than `oqueue-core`.
- **Generic over its seams, not hardwired to them.** The concrete `ObjectStore`, `Clock` and `KeyProvider` are chosen by `bin/oqueue`; this crate names none of them (NFR-51).
- ⚠️ **Backpressure between connections and the object-store write path is bespoke** — doc 05 §3 notes the real bottleneck is PUT throughput and request-rate limits, not socket I/O, and no crate provides that off the shelf.
