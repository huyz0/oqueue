# `oqueue-broker`

## What is it?

The I/O shell: the connection loop, request dispatch, and the batching that turns many small produces into few large PUTs.

## Why does it exist?

Because *almost* everything else in this workspace is sans-I/O — ⚠️ **"by construction" overstates it, and `M1.38` found this variant of the same universal**: `check-sans-io.sh` exempts `oqueue-store` from the object-storage pattern because it implements those backends, and never scans `bin/oqueue` at all — it walks `crates/*.rs` only. So there are two exceptions besides this crate, which is itself exempt from all three patterns; every *remaining* library crate is held to all three. And the sockets have to be somewhere. This crate is that somewhere, and it is generic over its seams so it can be tested without any of them being real.

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
| Names no **seam implementation** — no concrete `ObjectStore` or `KeyProvider`. ⚠️ **`Clock` was called contested here until `M1.29` read the three documents side by side and found no disagreement to settle**: they answer different questions. `check-sans-io.sh`'s clock exemption says where the real implementation *lives* — "in `oqueue-broker` per the same reasoning as the socket exemption". `architecture.md` says where concrete types are *chosen* — `bin/oqueue`, the composition root. Those are the same two answers `ObjectStore` already has, and nobody calls that contested: `S3Store`'s code lives in `oqueue-store`, and `bin/oqueue` is where it would be picked (⚠️ *would* — that wiring is not written yet; `bin/oqueue` does not depend on `oqueue-store` today). ADR-0004 takes no position on where the *real* implementation lives; its one mention of "`M1`'s real `Clock`" is about NTP clamping having no gate. (It does place the *fake*, beside the trait in `oqueue-core`, rejecting `oqueue-testkit` — a location claim about a different thing.) **So this row does not claim `Clock` because `check-sans-io.sh` says the real one lives here** — asserting "names no concrete `Clock`" would be a negative invariant this crate is expected to break, the same defect `M0.31` fixed for sockets. ⚠️ Not because two documents disagree about where it goes, and not merely because the implementation is unwritten: *unwritten* is the condition under which the claim would be true today, which is exactly why it would be the wrong thing to write down. | ⚠️ **No gate, and specifically not `check-sans-io.sh`** — that script *exempts* this directory. Review is the only thing holding it, and this is the crate where that matters most. ⚠️ **Sockets are excluded from this row on purpose**, and saying so is the point: "What is it?" above calls this crate the connection loop and "Why does it exist?" says the sockets have to be somewhere and this is that somewhere. The row said "no concrete socket type" from `M0.8` until `M0.31` removed it. That wording asserted the negation of the crate's own reason for existing — stated in the same file, above this table — and would have been falsified by the first commit that wrote the loop. `architecture.md` names three seams and none is a socket |
| Depends only on `oqueue-core` and its siblings as a composer | `scripts/check-layering.sh` |

## Notes for whoever touches this

- ⚠️ **A composer.** `check-layering.sh` names this crate and `bin/oqueue` as the only two allowed to depend on workspace crates other than `oqueue-core`.
- **Generic over its seams, not hardwired to them.** The concrete `ObjectStore` and `KeyProvider` are chosen by `bin/oqueue`; this crate names neither (NFR-51). ⚠️ **`Clock` is left out of this bullet on purpose** — see the Invariants row above. `check-sans-io.sh`'s exemption says the real one lives here; this file asserted the opposite until `M0.31`. ⚠️ **Where it lives is not who constructs it** — that is the same distinction the Invariants row above turns on, and nothing here decides the wiring.
- ⚠️ **Backpressure between connections and the object-store write path is bespoke** — doc 05 §3 notes the real bottleneck is PUT throughput and request-rate limits, not socket I/O, and no crate provides that off the shelf.
