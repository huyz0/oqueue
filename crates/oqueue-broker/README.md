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
| Names no **seam implementation** — no concrete `ObjectStore` or `KeyProvider`. ⚠️ **`Clock` is contested and this row does not claim it**: `check-sans-io.sh`'s clock exemption says in as many words that the real implementation "lives in `oqueue-broker` per the same reasoning as the socket exemption", while `architecture.md` says `bin/oqueue` chooses the concrete types and ADR-0004 calls it "`M1`'s real `Clock`". Asserting either side here re-creates the defect `M0.31` fixed for sockets — an invariant M1 must break. **M1 decides, and whichever way it goes, the loser is a document to correct in the same commit** | ⚠️ **No gate, and specifically not `check-sans-io.sh`** — that script *exempts* this directory. Review is the only thing holding it, and this is the crate where that matters most. ⚠️ **Sockets are excluded from this row on purpose**, and saying so is the point: "What is it?" above calls this crate the connection loop and "Why does it exist?" says the sockets have to be somewhere and this is that somewhere. The row said "no concrete socket type" from `M0.8` until `M0.31` removed it. That wording asserted the negation of the crate's own reason for existing — stated in the same file, above this table — and would have been falsified by the first commit that wrote the loop. `architecture.md` names three seams and none is a socket |
| Depends only on `oqueue-core` and its siblings as a composer | `scripts/check-layering.sh` |

## Notes for whoever touches this

- ⚠️ **A composer.** `check-layering.sh` names this crate and `bin/oqueue` as the only two allowed to depend on workspace crates other than `oqueue-core`.
- **Generic over its seams, not hardwired to them.** The concrete `ObjectStore` and `KeyProvider` are chosen by `bin/oqueue`; this crate names neither (NFR-51). ⚠️ **`Clock` is left out of this bullet on purpose** — see the Invariants row above: `check-sans-io.sh`'s exemption says the real one lives here, and this file asserted the opposite until `M0.31`.
- ⚠️ **Backpressure between connections and the object-store write path is bespoke** — doc 05 §3 notes the real bottleneck is PUT throughput and request-rate limits, not socket I/O, and no crate provides that off the shelf.
