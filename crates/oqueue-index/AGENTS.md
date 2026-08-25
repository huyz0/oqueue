# `oqueue-index` — working notes

Read [`README.md`](README.md) first for what this crate is and why. This file is
what is specific to *changing* it.

## Which gates run

`check-layering.sh`, `check-sans-io.sh`, `check-unsafe.sh`, `check-file-size.sh`,
`check-readmes.sh`, and `scripts/check-crate.sh oqueue-index` for fmt, clippy and tests.

## Easy to get wrong here

1. **Search is on the cached tail-read path** (NFR-2), where the entire budget is CPU. `performance.md` rule 18 gives index lookup a benchmark obligation.

## What is here

`M3.5` gave this crate its first code: [`MemoryIndex`](src/memory.rs), the
in-memory fold of the metadata log a fetch queries.

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
