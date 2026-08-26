# `oqueue-index` — working notes

Read [`README.md`](README.md) first for what this crate is and why. This file is
what is specific to *changing* it.

## Which gates run

`check-layering.sh`, `check-sans-io.sh`, `check-unsafe.sh`, `check-file-size.sh`,
`check-readmes.sh`, and `scripts/check-crate.sh oqueue-index` for fmt, clippy and tests.

## Easy to get wrong here

1. **Search is on the cached tail-read path** (NFR-2), where the entire budget is CPU. `performance.md` rule 18 gives index lookup a benchmark obligation — ⚠️ **and the lookup is `oqueue-core::IndexState::find_batches`, not code in this crate.** Writing the benchmark here would benchmark a `Mutex` and a delegation. `M3.18` owns it.
2. ⚠️ **One writer per index** — `ADR-0024`. A second writer makes the loser's fold refused as `NonMonotonicCommitVersion`, an error saying the log lost ordering when nothing is wrong with it.

## What is here

`M3.5` gave this crate its first code: [`MemoryIndex`](src/memory.rs), the
materialization a broker serves reads from. `M3.8` added
[`LogApplier`](src/applier.rs), which fills it from the log in bounded batches
and keeps no bookmark of its own.

⚠️ **`MemoryIndex` is byte-identical to `oqueue-core`'s `FakeMaterializedIndex`
today**, and M3's checkpoint review found the documents claiming otherwise
(`M3.24`). Both are thin `Mutex<IndexState>` wrappers, so the conformance suite
below proves the contract **once**, not twice — `contracts.md` rule 3 is
satisfied by doc 10 #12's engine, which is foreseeable rather than present, and
⚠️ `M3.11` was to end that with a quota and an eviction policy and could not —
a ceiling on this index's keying gives back range a rebuild cannot restore — so
`roadmap.md` carries the enforcement, and the divergence with it, to `M5`.
Until then, do not read the two types as independent evidence.

⚠️ **Extend it rather than starting beside it.** The fold itself —
order-checking, all-or-nothing application, non-wrapping addition — lives in
`IndexState` in `oqueue-core`, deliberately shared with the
`FakeMaterializedIndex` there so the two cannot drift into two meanings of one
contract. A second index type in this crate that reimplements the fold is the
thing that shared state exists to prevent; `M3.6`'s two-tier layout is a change
to these, not a sibling of them.

⚠️ **The contract is [`MaterializedIndex`](../oqueue-core/src/materialized_index.rs)
in `oqueue-core`**, and `tests/it/index.rs` holds one conformance suite run
against every implementation of it. A new implementation is added to that
suite's runner, not given tests of its own.

⚠️ **This is a cache, never the source of truth.** Everything here is derivable
from the metadata log by replay, which is what lets it be dropped under
pressure. Nothing may be stored here that the log cannot put back.

⚠️ **In-memory is `M3`'s answer, not the project's.** Doc 10 #12 — `SQLite`,
`redb`, `RocksDB`, `fjall` or `SlateDB` — is open and doc 13 §6 asks for a
benchmark rather than an argument. The growth arithmetic in `M3.md` (~14 GB/day,
~97 GB over a 7-day retention) is why: `MemoryIndex` does not fit that and is
not meant to.
