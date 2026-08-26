# `oqueue-index`

## What is it?

The materialization a broker serves reads from, and what fills it.

## Why does it exist?

Because a fetch must not enumerate object storage. Doc 12 measures LIST at 12-38x the price of a GET and semantically useless besides, so the index is what turns a read into a bounded number of GETs.

## Upstream

- `oqueue-core` — the types, IDs, errors and trait seams this crate is written against.

## Downstream

`oqueue-broker`, and through it `bin/oqueue`.

⚠️ **Only a composer may consume this crate.** `check-layering.sh` allows a
non-composer to depend on `oqueue-core` alone, so a sibling that needs a type
from here does not depend on here — the type belongs in `oqueue-core`.

## ⚠️ The fold and the search are not here

They are in `oqueue-core`'s `IndexState`, and that is forced rather than
convenient: `contracts.md` rule 9 puts every fake beside its trait, rule 10
makes a fake's job fidelity to the *documented* contract, and two copies of a
fold this subtle — order checking, all-or-nothing application, non-wrapping
addition, tier demotion, the paging rule — would drift into two meanings of one
contract. So one fold lives beside the trait and both materializations wrap it.

⚠️ **The consequence is worth knowing rather than discovering** (`M3.24`, from
M3's checkpoint review): `MemoryIndex` here and `FakeMaterializedIndex` in
`oqueue-core` are byte-identical today once doc comments are stripped, so the
conformance suite in `tests/` proves the contract **once**, not twice. The
foreseeable second implementation — the one `contracts.md` rule 3 is actually
satisfied by — is doc 10 #12's engine, and `M3.11` is where `MemoryIndex` first
has behaviour a test double must not have: a quota, and an eviction policy.

## Invariants

| Must stay true | Held by |
|---|---|
| A lookup issues no object-storage LIST | no gate — and the seam taking no store does **not** buy it: an implementor may hold an `ObjectStore`, and `SlateDB` is one of doc 10 #12's candidates |
| The index is refillable from the log alone | `tests/it/applier.rs`; `MaterializedIndex`'s guarantee 3 |

## Notes for whoever touches this

- **Search is on the cached tail-read path** (NFR-2), where the entire budget is CPU. `performance.md` rule 18 gives index lookup a benchmark obligation — ⚠️ **against `oqueue-core::IndexState::find_batches`, which is where the lookup is**, not against this crate. `M3.18` owns writing it.
- ⚠️ **`LogApplier` is the one writer of an index it holds.** So is a `Coordinator` of the index it was opened with, and they must not be the same index — `M3.9` decided that, `M3.23` makes it type-checked.
